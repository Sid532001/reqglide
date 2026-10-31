//! HTTP transport implementation. This crate deliberately has no dependency on
//! Tauri or the CLI so every adapter executes requests identically.

use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use api_client_core::{
    ApiKeyPlacement, HeaderEntry, MultipartValue, RequestAuth, RequestBody, RequestDefinition,
    RequestValidationError, ResponseBody, ResponseBodyEncoding, ResponseSnapshot, ResponseTiming,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use reqwest::{
    Method, StatusCode, Url,
    cookie::CookieStore as ReqwestCookieStore,
    header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderName, HeaderValue, LOCATION},
    multipart::{Form, Part},
    redirect::Policy,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(windows)]
mod system_proxy;

#[derive(Clone, Default)]
pub struct HttpExecutionContext {
    pub cookie_jar: Option<Arc<CookieJar>>,
    pub proxy: ProxyConfiguration,
    pub root_certificate_pem: Vec<u8>,
    pub client_identity_pem: Vec<u8>,
    pub accept_invalid_certificates: bool,
}

#[derive(Clone, Default)]
pub enum ProxyConfiguration {
    #[default]
    System,
    Disabled,
    Custom {
        url: String,
        username: String,
        password: String,
        no_proxy: String,
    },
}

#[derive(Default)]
pub struct CookieJar {
    inner: RwLock<cookie_store::CookieStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieSnapshot {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires_at_ms: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
}

impl CookieJar {
    pub fn add(&self, set_cookie: &str, url: &Url) -> Result<(), CookieJarError> {
        let cookie = cookie_store::RawCookie::parse(set_cookie)
            .map_err(|error| CookieJarError::InvalidCookie(error.to_string()))?
            .into_owned();
        self.inner
            .write()
            .map_err(|_| CookieJarError::Unavailable)?
            .store_response_cookies(std::iter::once(cookie), url);
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<CookieSnapshot>, CookieJarError> {
        let store = self.inner.read().map_err(|_| CookieJarError::Unavailable)?;
        let mut cookies = store
            .iter_unexpired()
            .map(|cookie| CookieSnapshot {
                name: cookie.name().to_owned(),
                value: cookie.value().to_owned(),
                domain: cookie.domain.as_cow().unwrap_or_default().into_owned(),
                path: cookie.path.as_ref().to_owned(),
                expires_at_ms: match &cookie.expires {
                    cookie_store::CookieExpiration::AtUtc(value) => {
                        value.unix_timestamp().checked_mul(1_000)
                    }
                    cookie_store::CookieExpiration::SessionEnd => None,
                },
                secure: cookie.secure().unwrap_or(false),
                http_only: cookie.http_only().unwrap_or(false),
                same_site: cookie.same_site().map(|value| format!("{value:?}")),
            })
            .collect::<Vec<_>>();
        cookies.sort_by(|left, right| {
            left.domain
                .cmp(&right.domain)
                .then(left.path.cmp(&right.path))
                .then(left.name.cmp(&right.name))
        });
        Ok(cookies)
    }

    pub fn remove(&self, domain: &str, path: &str, name: &str) -> Result<bool, CookieJarError> {
        Ok(self
            .inner
            .write()
            .map_err(|_| CookieJarError::Unavailable)?
            .remove(domain, path, name)
            .is_some())
    }

    pub fn clear(&self) -> Result<(), CookieJarError> {
        self.inner
            .write()
            .map_err(|_| CookieJarError::Unavailable)?
            .clear();
        Ok(())
    }
}

impl ReqwestCookieStore for CookieJar {
    fn set_cookies(
        &self,
        cookie_headers: &mut dyn Iterator<Item = &reqwest::header::HeaderValue>,
        url: &Url,
    ) {
        let cookies = cookie_headers.filter_map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| cookie_store::RawCookie::parse(value).ok())
                .map(cookie_store::RawCookie::into_owned)
        });
        if let Ok(mut store) = self.inner.write() {
            store.store_response_cookies(cookies, url);
        }
    }

    fn cookies(&self, url: &Url) -> Option<reqwest::header::HeaderValue> {
        let store = self.inner.read().ok()?;
        let value = store
            .get_request_values(url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        (!value.is_empty())
            .then(|| reqwest::header::HeaderValue::from_str(&value).ok())
            .flatten()
    }
}

#[derive(Debug, Error)]
pub enum CookieJarError {
    #[error("cookie jar is unavailable")]
    Unavailable,
    #[error("cookie is invalid: {0}")]
    InvalidCookie(String),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HttpExecutor;

pub fn build_reqwest_client(
    options: &api_client_core::RequestOptions,
    context: HttpExecutionContext,
) -> Result<reqwest::Client, HttpExecutionError> {
    build_reqwest_client_inner(options, context, true)
}

pub fn build_streaming_reqwest_client(
    options: &api_client_core::RequestOptions,
    context: HttpExecutionContext,
) -> Result<reqwest::Client, HttpExecutionError> {
    build_reqwest_client_inner(options, context, false)
}

/// Upper bound (ms) for a single connection attempt (DNS + TCP + TLS), kept separate from the
/// overall request timeout. Without this, one unreachable resolved address (e.g. a dead-end IPv6
/// route behind a VPN, common on corporate networks) can silently consume the whole request
/// timeout budget before the connector even tries the next candidate address — unlike browsers
/// and Node, which race/fail over between resolved addresses quickly.
/// This is why the same request can hang and time out here while succeeding elsewhere.
const MAX_CONNECT_TIMEOUT_MS: u64 = 10_000;

fn connect_timeout_ms(total_timeout_ms: u64) -> u64 {
    total_timeout_ms.clamp(1, MAX_CONNECT_TIMEOUT_MS)
}

fn build_reqwest_client_inner(
    options: &api_client_core::RequestOptions,
    context: HttpExecutionContext,
    total_timeout: bool,
) -> Result<reqwest::Client, HttpExecutionError> {
    // The REST path handles redirects explicitly to replay supported bodies and
    // apply credential and method rules to each destination.
    let redirect_policy = if total_timeout {
        Policy::none()
    } else if options.follow_redirects {
        Policy::limited(10)
    } else {
        Policy::none()
    };
    let HttpExecutionContext {
        cookie_jar,
        proxy,
        root_certificate_pem,
        client_identity_pem,
        accept_invalid_certificates,
    } = context;
    let mut client_builder = reqwest::Client::builder()
        .redirect(redirect_policy)
        .danger_accept_invalid_certs(accept_invalid_certificates);
    if total_timeout {
        client_builder = client_builder
            .timeout(Duration::from_millis(options.timeout_ms))
            .connect_timeout(Duration::from_millis(connect_timeout_ms(
                options.timeout_ms,
            )));
    } else {
        client_builder = client_builder.connect_timeout(Duration::from_millis(connect_timeout_ms(
            options.timeout_ms,
        )));
    }
    if let Some(cookie_jar) = cookie_jar {
        client_builder = client_builder.cookie_provider(cookie_jar);
    }
    client_builder = match proxy {
        ProxyConfiguration::System => client_builder,
        ProxyConfiguration::Disabled => client_builder.no_proxy(),
        ProxyConfiguration::Custom {
            url,
            username,
            password,
            no_proxy,
        } => {
            let mut proxy = reqwest::Proxy::all(&url)
                .map_err(|source| HttpExecutionError::InvalidProxy { source })?;
            if !username.is_empty() || !password.is_empty() {
                proxy = proxy.basic_auth(&username, &password);
            }
            if !no_proxy.trim().is_empty() {
                proxy = proxy.no_proxy(reqwest::NoProxy::from_string(&no_proxy));
            }
            client_builder.no_proxy().proxy(proxy)
        }
    };
    if !root_certificate_pem.is_empty() {
        let certificates = reqwest::Certificate::from_pem_bundle(&root_certificate_pem)
            .map_err(|source| HttpExecutionError::InvalidRootCertificate { source })?;
        for certificate in certificates {
            client_builder = client_builder.add_root_certificate(certificate);
        }
    }
    if !client_identity_pem.is_empty() {
        let identity = reqwest::Identity::from_pem(&client_identity_pem)
            .map_err(|source| HttpExecutionError::InvalidClientIdentity { source })?;
        client_builder = client_builder.identity(identity);
    }
    client_builder
        .build()
        .map_err(|source| HttpExecutionError::BuildClient { source })
}

impl HttpExecutor {
    pub async fn execute(
        &self,
        request: RequestDefinition,
    ) -> Result<ResponseSnapshot, HttpExecutionError> {
        self.execute_with_context(request, HttpExecutionContext::default())
            .await
    }

    pub async fn execute_with_context(
        &self,
        request: RequestDefinition,
        context: HttpExecutionContext,
    ) -> Result<ResponseSnapshot, HttpExecutionError> {
        request.validate()?;
        let started_at = Instant::now();
        let deadline = started_at
            .checked_add(Duration::from_millis(request.options.timeout_ms))
            .ok_or(HttpExecutionError::InvalidTimeout)?;

        let mut method = Method::from_bytes(request.method.trim().as_bytes())
            .map_err(|_| HttpExecutionError::InvalidMethod(request.method.clone()))?;
        let mut url = Url::parse(request.url.trim()).map_err(|error| {
            HttpExecutionError::InvalidRequest(RequestValidationError::InvalidUrl(
                error.to_string(),
            ))
        })?;
        let has_query_parameters = request.query_params.iter().any(|parameter| {
            parameter.enabled && !(parameter.name.is_empty() && parameter.value.is_empty())
        }) || matches!(
            &request.auth,
            RequestAuth::ApiKey {
                placement: ApiKeyPlacement::Query,
                ..
            }
        );
        if has_query_parameters {
            let mut query = url.query_pairs_mut();
            for parameter in request
                .query_params
                .iter()
                .filter(|parameter| parameter.enabled)
                .filter(|parameter| !(parameter.name.is_empty() && parameter.value.is_empty()))
            {
                query.append_pair(&parameter.name, &parameter.value);
            }
            if let RequestAuth::ApiKey {
                key,
                value,
                placement: ApiKeyPlacement::Query,
            } = &request.auth
            {
                query.append_pair(key.trim(), value);
            }
        }

        let is_multipart = matches!(&request.body, RequestBody::Multipart { .. });
        let has_body = !matches!(&request.body, RequestBody::Empty);
        let has_content_type = !is_multipart
            && request.headers.iter().any(|header| {
                header.enabled && header.name.trim().eq_ignore_ascii_case("content-type")
            });

        let auth_header_name = match &request.auth {
            RequestAuth::Basic { .. } | RequestAuth::Bearer { .. } => Some("authorization"),
            RequestAuth::ApiKey {
                key,
                placement: ApiKeyPlacement::Header,
                ..
            } => Some(key.trim()),
            RequestAuth::None
            | RequestAuth::Inherit
            | RequestAuth::OAuth2ClientCredentials { .. }
            | RequestAuth::ApiKey { .. } => None,
        };

        // Same-origin redirects retain credentials. A change of host, port, or
        // scheme removes them, matching curl --location rather than --location-trusted.
        let mut static_headers: Vec<(HeaderName, HeaderValue)> = Vec::new();
        for header in request.headers.iter().filter(|header| {
            header.enabled && !(header.name.trim().is_empty() && header.value.is_empty())
        }) {
            if (is_multipart && header.name.trim().eq_ignore_ascii_case("content-type"))
                || (has_body && header.name.trim().eq_ignore_ascii_case("content-length"))
            {
                continue;
            }
            if auth_header_name
                .is_some_and(|auth_name| header.name.trim().eq_ignore_ascii_case(auth_name))
            {
                continue;
            }
            let name = HeaderName::from_bytes(header.name.trim().as_bytes()).map_err(|_| {
                HttpExecutionError::InvalidHeaderName {
                    name: header.name.clone(),
                }
            })?;
            let value = HeaderValue::from_str(&header.value).map_err(|_| {
                HttpExecutionError::InvalidHeaderValue {
                    name: header.name.clone(),
                }
            })?;
            static_headers.push((name, value));
        }

        match &request.auth {
            RequestAuth::None
            | RequestAuth::Inherit
            | RequestAuth::OAuth2ClientCredentials { .. }
            | RequestAuth::ApiKey {
                placement: ApiKeyPlacement::Query,
                ..
            } => {}
            RequestAuth::Basic { username, password } => {
                let credentials = BASE64.encode(format!("{username}:{password}"));
                let value =
                    HeaderValue::from_str(&format!("Basic {credentials}")).map_err(|_| {
                        HttpExecutionError::InvalidHeaderValue {
                            name: "Authorization".to_owned(),
                        }
                    })?;
                static_headers.push((AUTHORIZATION, value));
            }
            RequestAuth::Bearer { token } => {
                let value =
                    HeaderValue::from_str(&format!("Bearer {}", token.trim())).map_err(|_| {
                        HttpExecutionError::InvalidHeaderValue {
                            name: "Authorization".to_owned(),
                        }
                    })?;
                static_headers.push((AUTHORIZATION, value));
            }
            RequestAuth::ApiKey {
                key,
                value,
                placement: ApiKeyPlacement::Header,
            } => {
                let name = HeaderName::from_bytes(key.trim().as_bytes())
                    .map_err(|_| HttpExecutionError::InvalidHeaderName { name: key.clone() })?;
                let value = HeaderValue::from_str(value)
                    .map_err(|_| HttpExecutionError::InvalidHeaderValue { name: key.clone() })?;
                static_headers.push((name, value));
            }
        }

        let mut current_body = request.body.clone();
        let mut carries_body = has_body;
        let mut redirects_remaining: u8 = 10;
        let (response, route) = loop {
            // PAC rules can select a different route for a redirect destination.
            #[cfg(windows)]
            let context = system_proxy::prepare_context(
                context.clone(),
                &url,
                remaining_request_time(deadline)?.as_millis().max(1) as u64,
            )
            .await?;
            #[cfg(not(windows))]
            let context = context.clone();
            let route = transport_route(&context.proxy, &url);
            remaining_request_time(deadline).map_err(|error| error.with_route(&route))?;
            let client = build_reqwest_client(&request.options, context)?;
            let mut builder = client.request(method.clone(), url.clone());
            for (name, value) in &static_headers {
                builder = builder.header(name.clone(), value.clone());
            }
            if carries_body {
                builder = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    attach_body(
                        builder,
                        current_body.clone(),
                        request.options.max_request_bytes,
                        has_content_type,
                    ),
                )
                .await
                .map_err(|_| {
                    HttpExecutionError::RequestDeadlineExceeded {
                        route: String::new(),
                    }
                    .with_route(&route)
                })??;
            }

            let response = builder
                .timeout(
                    remaining_request_time(deadline).map_err(|error| error.with_route(&route))?,
                )
                .send()
                .await
                .map_err(|source| classify_send_error(source).with_route(&route))?;
            let status = response.status();

            if !request.options.follow_redirects || !is_redirect_status(status) {
                break (response, route);
            }
            let Some(location) = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
            else {
                break (response, route);
            };
            if redirects_remaining == 0 {
                return Err(HttpExecutionError::TooManyRedirects);
            }
            let Ok(next_url) = url.join(location) else {
                break (response, route);
            };

            if url.origin() != next_url.origin() {
                static_headers.retain(|(name, _)| {
                    !matches!(
                        name.as_str(),
                        "authorization" | "cookie" | "proxy-authorization" | "host"
                    ) && !auth_header_name
                        .is_some_and(|auth_name| name.as_str().eq_ignore_ascii_case(auth_name))
                });
            }

            if should_downgrade_to_get(status, &method) {
                method = Method::GET;
                carries_body = false;
                current_body = RequestBody::Empty;
                static_headers.retain(|(name, _)| {
                    !matches!(
                        name.as_str(),
                        "content-type"
                            | "content-length"
                            | "content-encoding"
                            | "transfer-encoding"
                    )
                });
            }
            url = next_url;
            redirects_remaining -= 1;
        };

        let status = response.status();
        let status_text = status.canonical_reason().unwrap_or_default().to_owned();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let response_headers = response
            .headers()
            .iter()
            .map(|(name, value)| HeaderEntry {
                name: name.as_str().to_owned(),
                value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
                enabled: true,
            })
            .collect();

        let max_bytes = request.options.max_response_bytes as usize;
        let mut received = Vec::with_capacity(max_bytes.min(64 * 1024));
        let mut stream = response.bytes_stream();
        let mut truncated = false;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|source| {
                if source.is_timeout() {
                    HttpExecutionError::ResponseBodyTimeout {
                        source,
                        route: String::new(),
                    }
                } else {
                    HttpExecutionError::ReadResponse {
                        source,
                        route: String::new(),
                    }
                }
                .with_route(&route)
            })?;
            let available = max_bytes.saturating_sub(received.len());

            if chunk.len() > available {
                received.extend_from_slice(&chunk[..available]);
                truncated = true;
                break;
            }

            received.extend_from_slice(&chunk);
        }

        let received_bytes = received.len() as u64;
        let (content, encoding) = encode_response_body(&received, content_type.as_deref());

        Ok(ResponseSnapshot {
            status: status.as_u16(),
            status_text,
            headers: response_headers,
            body: ResponseBody {
                content,
                encoding,
                content_type,
                truncated,
            },
            timing: ResponseTiming {
                total_ms: started_at.elapsed().as_millis() as u64,
            },
            received_bytes,
        })
    }
}

fn remaining_request_time(deadline: Instant) -> Result<Duration, HttpExecutionError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| HttpExecutionError::RequestDeadlineExceeded {
            route: String::new(),
        })
}

/// Whether a redirect changes the next hop's method to GET with no body, mirroring curl/browser
/// convention: 303 always downgrades (except HEAD, which has no body anyway); 301/302 only
/// downgrade a POST (other methods, and 307/308, keep their original method and body).
fn should_downgrade_to_get(status: StatusCode, method: &Method) -> bool {
    match status {
        StatusCode::SEE_OTHER => *method != Method::HEAD,
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND => *method == Method::POST,
        _ => false,
    }
}

fn is_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

async fn attach_body(
    mut builder: reqwest::RequestBuilder,
    body: RequestBody,
    max_request_bytes: u64,
    has_content_type: bool,
) -> Result<reqwest::RequestBuilder, HttpExecutionError> {
    match body {
        RequestBody::Empty => {}
        RequestBody::Text {
            content,
            content_type,
        } => {
            ensure_payload_within_limit(content.len() as u64, max_request_bytes)?;
            if !has_content_type && let Some(content_type) = content_type {
                builder = builder.header(CONTENT_TYPE, content_type);
            }
            builder = builder.body(content);
        }
        RequestBody::FormUrlEncoded { fields } => {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            for field in fields
                .iter()
                .filter(|field| field.enabled)
                .filter(|field| !(field.name.is_empty() && field.value.is_empty()))
            {
                form.append_pair(&field.name, &field.value);
            }
            let form = form.finish();
            ensure_payload_within_limit(form.len() as u64, max_request_bytes)?;
            if !has_content_type {
                builder = builder.header(CONTENT_TYPE, "application/x-www-form-urlencoded");
            }
            builder = builder.body(form);
        }
        RequestBody::Multipart { parts } => {
            let mut form = Form::new();
            let mut payload_bytes = 0_u64;
            for part in parts.into_iter().filter(|part| part.enabled) {
                match part.value {
                    MultipartValue::Text {
                        content,
                        content_type,
                    } => {
                        payload_bytes = checked_payload_total(
                            payload_bytes,
                            content.len() as u64,
                            max_request_bytes,
                        )?;
                        let mut multipart_part = Part::text(content);
                        if let Some(content_type) = content_type.filter(|value| !value.is_empty()) {
                            multipart_part =
                                multipart_part.mime_str(&content_type).map_err(|source| {
                                    HttpExecutionError::InvalidMultipartContentType { source }
                                })?;
                        }
                        form = form.part(part.name, multipart_part);
                    }
                    MultipartValue::File {
                        path,
                        file_name,
                        content_type,
                    } => {
                        let metadata = tokio::fs::metadata(&path)
                            .await
                            .map_err(|source| HttpExecutionError::RequestFileMetadata { source })?;
                        if !metadata.is_file() {
                            return Err(HttpExecutionError::RequestFileNotRegular);
                        }
                        payload_bytes = checked_payload_total(
                            payload_bytes,
                            metadata.len(),
                            max_request_bytes,
                        )?;
                        let mut multipart_part = Part::file(&path)
                            .await
                            .map_err(|source| HttpExecutionError::OpenRequestFile { source })?;
                        if let Some(file_name) = file_name.filter(|value| !value.is_empty()) {
                            multipart_part = multipart_part.file_name(file_name);
                        }
                        if let Some(content_type) = content_type.filter(|value| !value.is_empty()) {
                            multipart_part =
                                multipart_part.mime_str(&content_type).map_err(|source| {
                                    HttpExecutionError::InvalidMultipartContentType { source }
                                })?;
                        }
                        form = form.part(part.name, multipart_part);
                    }
                }
            }
            builder = builder.multipart(form);
        }
        RequestBody::Binary { path, content_type } => {
            let metadata = tokio::fs::metadata(&path)
                .await
                .map_err(|source| HttpExecutionError::RequestFileMetadata { source })?;
            if !metadata.is_file() {
                return Err(HttpExecutionError::RequestFileNotRegular);
            }
            ensure_payload_within_limit(metadata.len(), max_request_bytes)?;
            let file = tokio::fs::File::open(&path)
                .await
                .map_err(|source| HttpExecutionError::OpenRequestFile { source })?;
            if !has_content_type && let Some(content_type) = content_type {
                builder = builder.header(CONTENT_TYPE, content_type);
            }
            builder = builder
                .header(CONTENT_LENGTH, metadata.len())
                .body(reqwest::Body::from(file));
        }
    }
    Ok(builder)
}

fn ensure_payload_within_limit(actual: u64, limit: u64) -> Result<(), HttpExecutionError> {
    if actual > limit {
        Err(HttpExecutionError::RequestBodyTooLarge { actual, limit })
    } else {
        Ok(())
    }
}

fn checked_payload_total(
    current: u64,
    additional: u64,
    limit: u64,
) -> Result<u64, HttpExecutionError> {
    let actual =
        current
            .checked_add(additional)
            .ok_or(HttpExecutionError::RequestBodyTooLarge {
                actual: u64::MAX,
                limit,
            })?;
    ensure_payload_within_limit(actual, limit)?;
    Ok(actual)
}

fn classify_send_error(source: reqwest::Error) -> HttpExecutionError {
    if source.is_timeout() && source.is_connect() {
        HttpExecutionError::ConnectionTimeout {
            source,
            route: String::new(),
        }
    } else if source.is_timeout() {
        HttpExecutionError::RequestTimeout {
            source,
            route: String::new(),
        }
    } else if source.is_connect() {
        HttpExecutionError::ConnectionFailed {
            source,
            route: String::new(),
        }
    } else {
        HttpExecutionError::SendRequest {
            source,
            route: String::new(),
        }
    }
}

fn safe_proxy_endpoint(value: &str) -> String {
    // Never include proxy userinfo, paths, or query parameters in diagnostics.
    Url::parse(value)
        .ok()
        .filter(|url| url.host_str().is_some())
        .map(|url| {
            format!(
                "{}://{}{}",
                url.scheme(),
                url.host().unwrap(),
                url.port()
                    .map(|port| format!(":{port}"))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_else(|| "configured proxy".to_owned())
}

fn transport_route(proxy: &ProxyConfiguration, url: &Url) -> String {
    match proxy {
        ProxyConfiguration::Disabled => "direct connection (no proxy)".to_owned(),
        ProxyConfiguration::Custom { url, no_proxy, .. } => format!(
            "proxy {}{}",
            safe_proxy_endpoint(url),
            if no_proxy.is_empty() {
                ""
            } else {
                " (bypass rules configured)"
            }
        ),
        ProxyConfiguration::System => {
            let keys = if url.scheme() == "https" {
                ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
            } else {
                ["http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]
            };
            for key in keys {
                if let Ok(value) = std::env::var(key)
                    && !value.is_empty()
                {
                    return format!("system environment {key}: {}", safe_proxy_endpoint(&value));
                }
            }
            "operating-system proxy settings".to_owned()
        }
    }
}

fn encode_response_body(body: &[u8], content_type: Option<&str>) -> (String, ResponseBodyEncoding) {
    let textual_content_type = content_type.is_some_and(|value| {
        let value = value.to_ascii_lowercase();
        value.starts_with("text/")
            || value.contains("json")
            || value.contains("xml")
            || value.contains("javascript")
            || value.contains("graphql")
            || value.contains("x-www-form-urlencoded")
    });

    if textual_content_type || std::str::from_utf8(body).is_ok() {
        (
            String::from_utf8_lossy(body).into_owned(),
            ResponseBodyEncoding::Utf8,
        )
    } else {
        (BASE64.encode(body), ResponseBodyEncoding::Base64)
    }
}

#[derive(Debug, Error)]
pub enum HttpExecutionError {
    #[error("configured request timeout is too large for this platform")]
    InvalidTimeout,
    #[error("request timed out: the total request deadline was exceeded{route}")]
    RequestDeadlineExceeded { route: String },
    #[error("request exceeded the redirect limit (10)")]
    TooManyRedirects,
    #[error(transparent)]
    InvalidRequest(#[from] RequestValidationError),
    #[error("invalid HTTP method '{0}'")]
    InvalidMethod(String),
    #[error("invalid header name '{name}'")]
    InvalidHeaderName { name: String },
    #[error("header '{name}' contains an invalid value")]
    InvalidHeaderValue { name: String },
    #[error("request body is {actual} bytes, exceeding the configured {limit}-byte limit")]
    RequestBodyTooLarge { actual: u64, limit: u64 },
    #[error("request upload file metadata could not be read")]
    RequestFileMetadata {
        #[source]
        source: std::io::Error,
    },
    #[error("request upload path is not a regular file")]
    RequestFileNotRegular,
    #[error("request upload file could not be opened")]
    OpenRequestFile {
        #[source]
        source: std::io::Error,
    },
    #[error("multipart part has an invalid content type")]
    InvalidMultipartContentType {
        #[source]
        source: reqwest::Error,
    },
    #[error("could not initialize the HTTP client")]
    BuildClient {
        #[source]
        source: reqwest::Error,
    },
    #[error("proxy configuration is invalid")]
    InvalidProxy {
        #[source]
        source: reqwest::Error,
    },
    #[error("custom CA certificate bundle is invalid")]
    InvalidRootCertificate {
        #[source]
        source: reqwest::Error,
    },
    #[error("client certificate identity is invalid")]
    InvalidClientIdentity {
        #[source]
        source: reqwest::Error,
    },
    #[error(
        "Windows automatic proxy resolution failed (code {code}); configure a custom proxy in Network & TLS settings"
    )]
    SystemProxyLookup { code: u32 },
    #[error(
        "Windows automatic proxy discovery timed out; configure a custom proxy in Network & TLS settings"
    )]
    SystemProxyTimeout,
    #[error(
        "connection timed out during DNS, proxy, TCP, or TLS setup; check destination reachability, VPN, and proxy settings{route}"
    )]
    ConnectionTimeout {
        route: String,
        #[source]
        source: reqwest::Error,
    },
    #[error(
        "request timed out before response headers were received; check connectivity, proxy settings, and the request timeout{route}"
    )]
    RequestTimeout {
        route: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("the server responded, but reading the response body timed out{route}")]
    ResponseBodyTimeout {
        route: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("could not connect to the target{route}")]
    ConnectionFailed {
        route: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("request failed before a response was received{route}")]
    SendRequest {
        route: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("the response stream ended unexpectedly{route}")]
    ReadResponse {
        route: String,
        #[source]
        source: reqwest::Error,
    },
}

impl HttpExecutionError {
    fn with_route(mut self, description: &str) -> Self {
        match &mut self {
            Self::ConnectionTimeout { route, .. }
            | Self::RequestDeadlineExceeded { route }
            | Self::RequestTimeout { route, .. }
            | Self::ResponseBodyTimeout { route, .. }
            | Self::ConnectionFailed { route, .. }
            | Self::SendRequest { route, .. }
            | Self::ReadResponse { route, .. } => {
                *route = format!("\nTransport route: {description}");
            }
            _ => {}
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client_core::{
        DEFAULT_MAX_RESPONSE_BYTES, KeyValueEntry, MultipartPart, MultipartValue, RequestOptions,
    };
    use std::io::Write;
    use std::sync::mpsc::{Receiver, channel};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn connect_timeout_is_bounded_but_never_exceeds_the_total_budget() {
        assert_eq!(connect_timeout_ms(3_000), 3_000);
        assert_eq!(connect_timeout_ms(30_000), MAX_CONNECT_TIMEOUT_MS);
        assert_eq!(connect_timeout_ms(0), 1);
    }

    #[test]
    fn transport_diagnostics_strip_proxy_credentials_and_url_data() {
        for proxy in [
            "http://user:password@proxy.example.com:8080/private?token=secret",
            "socks5://user:password@proxy.example.com:1080/private?token=secret",
        ] {
            let description = transport_route(
                &ProxyConfiguration::Custom {
                    url: proxy.to_owned(),
                    username: "sensitive-user".to_owned(),
                    password: "sensitive-password".to_owned(),
                    no_proxy: "private-host.example.com".to_owned(),
                },
                &Url::parse("https://example.com/private?token=never-print").unwrap(),
            );
            assert!(description.contains("proxy.example.com"));
            assert!(description.contains("bypass rules configured"));
            for secret in [
                "user",
                "password",
                "private",
                "token",
                "secret",
                "never-print",
            ] {
                assert!(!description.contains(secret), "diagnostic leaked {secret}");
            }
        }
        assert_eq!(
            transport_route(
                &ProxyConfiguration::Disabled,
                &Url::parse("https://example.com").unwrap()
            ),
            "direct connection (no proxy)"
        );
    }

    async fn local_response(response: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await.unwrap();
            socket.write_all(response).await.unwrap();
            socket.shutdown().await.unwrap();
        });

        format!("http://{address}")
    }

    async fn capture_request() -> (String, Receiver<String>) {
        capture_request_with_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        )
        .await
    }

    async fn capture_request_with_response(response: Vec<u8>) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = channel();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let size = socket.read(&mut chunk).await.unwrap();
                if size == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..size]);
                if request_is_complete(&request) {
                    break;
                }
            }
            sender
                .send(String::from_utf8_lossy(&request).into_owned())
                .unwrap();
            socket.write_all(&response).await.unwrap();
            socket.shutdown().await.unwrap();
        });

        (format!("http://{address}"), receiver)
    }

    fn request_is_complete(request: &[u8]) -> bool {
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let body_start = header_end + 4;
        let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
        if let Some(content_length) = headers.lines().find_map(|line| {
            line.strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        }) {
            request.len() >= body_start.saturating_add(content_length)
        } else {
            true
        }
    }

    fn request(url: String) -> RequestDefinition {
        RequestDefinition {
            method: "GET".to_owned(),
            url,
            query_params: Vec::new(),
            headers: Vec::new(),
            auth: api_client_core::RequestAuth::None,
            body: RequestBody::Empty,
            options: RequestOptions::default(),
        }
    }

    #[tokio::test]
    async fn captures_json_response_metadata() {
        let url = local_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
        )
        .await;

        let response = HttpExecutor.execute(request(url)).await.unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body.content, "{\"ok\":true}");
        assert_eq!(response.body.encoding, ResponseBodyEncoding::Utf8);
        assert!(!response.body.truncated);
        assert_eq!(response.received_bytes, 11);
    }

    #[tokio::test]
    async fn bounds_large_responses() {
        let url = local_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef",
        )
        .await;
        let mut definition = request(url);
        definition.options.max_response_bytes = 3;

        let response = HttpExecutor.execute(definition).await.unwrap();

        assert_eq!(response.body.content, "abc");
        assert!(response.body.truncated);
        assert_eq!(response.received_bytes, 3);
        assert_eq!(
            RequestOptions::default().max_response_bytes,
            DEFAULT_MAX_RESPONSE_BYTES
        );
    }

    #[tokio::test]
    async fn public_errors_do_not_echo_sensitive_urls() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        let mut definition = request(format!("http://{address}/private?api_key=must-not-appear"));
        definition.options.timeout_ms = 250;

        let error = HttpExecutor.execute(definition).await.unwrap_err();
        let public_message = error.to_string();

        assert!(!public_message.contains("must-not-appear"));
        assert!(!public_message.contains(&address.to_string()));
    }

    #[tokio::test]
    async fn sends_structured_query_parameters_and_basic_auth() {
        let (url, captured) = capture_request().await;
        let mut definition = request(format!("{url}/users?existing=1"));
        definition.query_params = vec![KeyValueEntry {
            name: "search".to_owned(),
            value: "Ada Lovelace".to_owned(),
            enabled: true,
        }];
        definition.headers.push(HeaderEntry {
            name: "Authorization".to_owned(),
            value: "stale-value".to_owned(),
            enabled: true,
        });
        definition.auth = RequestAuth::Basic {
            username: "user".to_owned(),
            password: "secret".to_owned(),
        };

        HttpExecutor.execute(definition).await.unwrap();
        let request = captured.recv_timeout(Duration::from_secs(1)).unwrap();
        let lowercase = request.to_ascii_lowercase();

        assert!(request.starts_with("GET /users?existing=1&search=Ada+Lovelace HTTP/1.1"));
        assert!(lowercase.contains("authorization: basic dxnlcjpzzwnyzxq="));
        assert!(!request.contains("stale-value"));
    }

    #[tokio::test]
    async fn sends_bearer_tokens_trimmed_and_drops_manual_authorization_headers() {
        let (url, captured) = capture_request().await;
        let mut definition = request(format!("{url}/me"));
        definition.headers.push(HeaderEntry {
            name: "Authorization".to_owned(),
            value: "stale-value".to_owned(),
            enabled: true,
        });
        definition.auth = RequestAuth::Bearer {
            token: "  eyJhbGciOiJIUzI1NiJ9.payload.sig \n".to_owned(),
        };

        HttpExecutor.execute(definition).await.unwrap();
        let request = captured.recv_timeout(Duration::from_secs(1)).unwrap();
        let lowercase = request.to_ascii_lowercase();

        assert!(lowercase.contains("authorization: bearer eyjhbgcioijiuzi1nij9.payload.sig\r\n"));
        assert!(!request.contains("stale-value"));
        assert_eq!(
            lowercase.matches("authorization:").count(),
            1,
            "only one Authorization header should be sent"
        );
    }

    #[tokio::test]
    async fn imported_json_search_matches_the_url_headers_and_body_on_the_wire() {
        let body = "{\n  \"filters\": [{\"field\":\"id\",\"operator\":\"eq\",\"value\":\"example-device\"}]\n}\n ";
        for content_type in ["application/x-www-form-urlencoded", "application/json"] {
            let (url, captured) = capture_request().await;
            let path = "/v1/events/search";
            let command = format!(
                "curl.exe `\n  --location `\n  --max-time `\n  30 `\n  --request `\n  POST `\n  --header `\n  'Authorization: Bearer {{{{access_token}}}}' `\n  --header `\n  'Content-Type: {content_type}' `\n  --data-raw `\n  '{body}' `\n  {url}{path}"
            );
            let imported = api_client_import::import_curl(&command).unwrap();
            let resolution = api_client_core::resolve_request_variables(
                &imported.request,
                &std::collections::BTreeMap::from([(
                    "access_token".to_owned(),
                    "wire-test-token".to_owned(),
                )]),
            );
            assert!(resolution.unresolved.is_empty());
            HttpExecutor
                .execute_with_context(
                    resolution.request,
                    HttpExecutionContext {
                        proxy: ProxyConfiguration::Disabled,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            let raw = captured.recv_timeout(Duration::from_secs(1)).unwrap();
            let (headers, sent_body) = raw.split_once("\r\n\r\n").unwrap();
            assert!(headers.starts_with(&format!("POST {path} HTTP/1.1\r\n")));
            let headers = headers.to_ascii_lowercase();
            assert!(headers.contains(&format!("\r\nhost: {}", url.trim_start_matches("http://"))));
            assert_eq!(headers.matches("authorization:").count(), 1);
            assert!(headers.contains("authorization: bearer wire-test-token"));
            assert_eq!(headers.matches("content-type:").count(), 1);
            assert!(headers.contains(&format!("content-type: {content_type}")));
            assert!(headers.contains(&format!("content-length: {}", body.len())));
            assert!(!raw.contains("{{access_token}}"));
            assert_eq!(sent_body.as_bytes(), body.as_bytes());
        }
    }

    #[tokio::test]
    async fn sends_raw_and_variable_tokens_through_the_configured_proxy() {
        for kind in ["raw", "variable", "header"] {
            let (proxy_url, captured) = capture_request().await;
            // This destination cannot resolve locally: success requires the configured proxy.
            let mut definition = request("http://api.invalid/resource".to_owned());
            definition.options.timeout_ms = 2_000;
            if kind == "header" {
                definition.headers.push(HeaderEntry {
                    name: "Authorization".to_owned(),
                    value: "Bearer {{access_token}}".to_owned(),
                    enabled: true,
                });
            } else {
                definition.auth = RequestAuth::Bearer {
                    token: if kind == "raw" {
                        "test-access-token"
                    } else {
                        "{{access_token}}"
                    }
                    .to_owned(),
                };
            }
            let resolution = api_client_core::resolve_request_variables(
                &definition,
                &std::collections::BTreeMap::from([(
                    "access_token".to_owned(),
                    "test-access-token".to_owned(),
                )]),
            );
            assert!(resolution.unresolved.is_empty());
            let response = HttpExecutor
                .execute_with_context(
                    resolution.request,
                    HttpExecutionContext {
                        proxy: ProxyConfiguration::Custom {
                            url: proxy_url,
                            username: String::new(),
                            password: String::new(),
                            no_proxy: String::new(),
                        },
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert_eq!(response.status, 200);
            let request = captured
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .to_ascii_lowercase();
            assert!(request.starts_with("get http://api.invalid/resource http/1.1"));
            assert_eq!(request.matches("authorization:").count(), 1);
            assert!(request.contains("authorization: bearer test-access-token\r\n"));
            assert!(!request.contains("{{"));
        }
    }

    #[tokio::test]
    async fn distinguishes_waiting_for_headers_from_a_stalled_response_body() {
        for body_started in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut headers = [0u8; 4096];
                assert!(stream.read(&mut headers).await.unwrap() > 0);
                if body_started {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\na")
                        .await
                        .unwrap();
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            });
            let mut definition = request(format!("http://{address}/private?token=never-show-this"));
            definition.options.timeout_ms = 150;
            let error = HttpExecutor
                .execute_with_context(
                    definition,
                    HttpExecutionContext {
                        proxy: ProxyConfiguration::Disabled,
                        ..Default::default()
                    },
                )
                .await
                .unwrap_err();
            if body_started {
                assert!(matches!(
                    error,
                    HttpExecutionError::ResponseBodyTimeout { .. }
                ));
            } else {
                assert!(matches!(error, HttpExecutionError::RequestTimeout { .. }));
            }
            assert!(!error.to_string().contains("never-show-this"));
            server.abort();
        }
    }

    #[tokio::test]
    async fn identifies_connection_setup_timeouts_without_exposing_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            // TCP connects, but the peer never completes its TLS handshake.
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        let error = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_millis(100))
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap()
            .get(format!("https://{address}/private?token=never-show-this"))
            .send()
            .await
            .unwrap_err();
        let error = classify_send_error(error);
        assert!(matches!(
            error,
            HttpExecutionError::ConnectionTimeout { .. }
        ));
        assert!(!error.to_string().contains("never-show-this"));
        server.abort();
    }

    #[tokio::test]
    async fn timeout_reports_the_selected_proxy_without_exposing_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut headers = [0u8; 4096];
            assert!(stream.read(&mut headers).await.unwrap() > 0);
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        let mut definition = request("http://api.invalid/private?token=never-print".to_owned());
        definition.options.timeout_ms = 150;
        let error = HttpExecutor
            .execute_with_context(
                definition,
                HttpExecutionContext {
                    proxy: ProxyConfiguration::Custom {
                        url: format!("http://{address}"),
                        username: "proxy-user".to_owned(),
                        password: "proxy-password".to_owned(),
                        no_proxy: String::new(),
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains(&format!("Transport route: proxy http://{address}")));
        for secret in ["private", "never-print", "proxy-user", "proxy-password"] {
            assert!(!message.contains(secret));
        }
        server.abort();
    }

    #[tokio::test]
    async fn strips_credentials_on_a_redirect_to_a_different_origin() {
        let (final_url, final_captured) = capture_request().await;
        let (initial_url, initial_captured) = capture_request_with_response(
            format!("HTTP/1.1 302 Found\r\nLocation: {final_url}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .into_bytes(),
        )
        .await;

        let mut definition = request(format!("{initial_url}/start"));
        definition.headers.push(HeaderEntry {
            name: "Cookie".to_owned(),
            value: "session=private".to_owned(),
            enabled: true,
        });
        definition.auth = RequestAuth::Bearer {
            token: "secret-token".to_owned(),
        };

        let response = HttpExecutor.execute(definition).await.unwrap();
        assert_eq!(response.status, 200);

        let initial_request = initial_captured
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let final_request = final_captured.recv_timeout(Duration::from_secs(1)).unwrap();

        assert!(
            initial_request
                .to_ascii_lowercase()
                .contains("authorization: bearer secret-token")
        );
        assert!(
            !final_request
                .to_ascii_lowercase()
                .contains("authorization:")
        );
        assert!(!final_request.to_ascii_lowercase().contains("cookie:"));
        assert!(final_request.starts_with("GET /final"));
    }

    #[tokio::test]
    async fn preserves_bearer_auth_on_a_same_origin_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for redirect in [true, false] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 2048];
                    let read = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(read, 0);
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|value| value == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                stream.write_all(if redirect {
                    b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                }).await.unwrap();
            }
            requests
        });
        let mut definition = request(format!("http://{address}/start"));
        definition.auth = RequestAuth::Bearer {
            token: "redirect-test-token".to_owned(),
        };
        HttpExecutor
            .execute_with_context(
                definition,
                HttpExecutionContext {
                    proxy: ProxyConfiguration::Disabled,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let requests = server.await.unwrap();
        assert!(requests[1].starts_with("GET /final HTTP/1.1"));
        for request in requests {
            assert!(request.contains("authorization: Bearer redirect-test-token\r\n"));
        }
    }

    #[tokio::test]
    async fn downgrades_post_to_get_without_body_on_302_but_keeps_it_on_307() {
        let (final_url, final_captured) = capture_request().await;
        let (initial_url, _initial_captured) = capture_request_with_response(
            format!("HTTP/1.1 302 Found\r\nLocation: {final_url}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .into_bytes(),
        )
        .await;

        let mut definition = request(format!("{initial_url}/start"));
        definition.method = "POST".to_owned();
        definition.body = RequestBody::Text {
            content: "original body".to_owned(),
            content_type: Some("text/plain".to_owned()),
        };

        HttpExecutor.execute(definition).await.unwrap();
        let final_request = final_captured.recv_timeout(Duration::from_secs(1)).unwrap();

        assert!(final_request.starts_with("GET /final"));
        assert!(!final_request.contains("original body"));
    }

    #[tokio::test]
    async fn keeps_method_and_body_on_a_307_redirect() {
        let (final_url, final_captured) = capture_request().await;
        let (initial_url, _initial_captured) = capture_request_with_response(
            format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {final_url}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .into_bytes(),
        )
        .await;

        let mut definition = request(format!("{initial_url}/start"));
        definition.method = "POST".to_owned();
        definition.body = RequestBody::Text {
            content: "original body".to_owned(),
            content_type: Some("text/plain".to_owned()),
        };

        HttpExecutor.execute(definition).await.unwrap();
        let final_request = final_captured.recv_timeout(Duration::from_secs(1)).unwrap();

        assert!(final_request.starts_with("POST /final"));
        assert!(final_request.contains("original body"));
    }

    #[tokio::test]
    async fn does_not_follow_redirects_when_disabled() {
        let (initial_url, _initial_captured) = capture_request_with_response(
            b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
        )
        .await;

        let mut definition = request(format!("{initial_url}/start"));
        definition.options.follow_redirects = false;

        let response = HttpExecutor.execute(definition).await.unwrap();
        assert_eq!(response.status, 302);
    }

    #[tokio::test]
    async fn redirect_hops_share_one_total_request_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for index in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                let _ = stream.read(&mut bytes).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                let response: &[u8] = if index < 3 {
                    b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                };
                let _ = stream.write_all(response).await;
            }
        });
        let mut definition = request(format!("http://{address}/start"));
        definition.options.timeout_ms = 240;
        let error = HttpExecutor
            .execute_with_context(
                definition,
                HttpExecutionContext {
                    proxy: ProxyConfiguration::Disabled,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            HttpExecutionError::RequestTimeout { .. }
                | HttpExecutionError::RequestDeadlineExceeded { .. }
        ));
        server.abort();
    }

    #[tokio::test]
    async fn redirect_limit_is_reported_as_an_error_instead_of_a_successful_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut count = 0;
            for _ in 0..=10 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                let _ = stream.read(&mut bytes).await;
                count += 1;
                stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            }
            count
        });
        let error = HttpExecutor
            .execute_with_context(
                request(format!("http://{address}/start")),
                HttpExecutionContext {
                    proxy: ProxyConfiguration::Disabled,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, HttpExecutionError::TooManyRedirects));
        assert_eq!(server.await.unwrap(), 11);
    }

    #[tokio::test]
    async fn sends_api_keys_in_the_query_string() {
        let (url, captured) = capture_request().await;
        let mut definition = request(format!("{url}/protected"));
        definition.auth = RequestAuth::ApiKey {
            key: "api_key".to_owned(),
            value: "local secret".to_owned(),
            placement: ApiKeyPlacement::Query,
        };

        HttpExecutor.execute(definition).await.unwrap();
        let request = captured.recv_timeout(Duration::from_secs(1)).unwrap();

        assert!(request.starts_with("GET /protected?api_key=local+secret HTTP/1.1"));
    }

    #[tokio::test]
    async fn sends_form_urlencoded_bodies() {
        let (url, captured) = capture_request().await;
        let mut definition = request(format!("{url}/token"));
        definition.method = "POST".to_owned();
        definition.body = RequestBody::FormUrlEncoded {
            fields: vec![
                KeyValueEntry {
                    name: "grant_type".to_owned(),
                    value: "client_credentials".to_owned(),
                    enabled: true,
                },
                KeyValueEntry {
                    name: "scope".to_owned(),
                    value: "read write".to_owned(),
                    enabled: true,
                },
            ],
        };

        HttpExecutor.execute(definition).await.unwrap();
        let request = captured.recv_timeout(Duration::from_secs(1)).unwrap();
        let lowercase = request.to_ascii_lowercase();

        assert!(lowercase.contains("content-type: application/x-www-form-urlencoded"));
        assert!(request.ends_with("grant_type=client_credentials&scope=read+write"));
    }

    #[tokio::test]
    async fn streams_binary_file_bodies() {
        let (url, captured) = capture_request().await;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"binary payload").unwrap();

        let mut definition = request(format!("{url}/upload"));
        definition.method = "PUT".to_owned();
        definition.body = RequestBody::Binary {
            path: file.path().to_string_lossy().into_owned(),
            content_type: Some("application/octet-stream".to_owned()),
        };

        HttpExecutor.execute(definition).await.unwrap();
        let request = captured.recv_timeout(Duration::from_secs(1)).unwrap();
        let lowercase = request.to_ascii_lowercase();

        assert!(lowercase.contains("content-type: application/octet-stream"));
        assert!(lowercase.contains("content-length: 14"));
        assert!(request.ends_with("binary payload"));
    }

    #[tokio::test]
    async fn sends_streamed_multipart_bodies_with_generated_boundaries() {
        let (url, captured) = capture_request().await;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"file payload").unwrap();

        let mut definition = request(format!("{url}/upload"));
        definition.method = "POST".to_owned();
        definition.headers.push(HeaderEntry {
            name: "Content-Type".to_owned(),
            value: "application/json".to_owned(),
            enabled: true,
        });
        definition.body = RequestBody::Multipart {
            parts: vec![
                MultipartPart {
                    name: "description".to_owned(),
                    enabled: true,
                    value: MultipartValue::Text {
                        content: "quarterly report".to_owned(),
                        content_type: None,
                    },
                },
                MultipartPart {
                    name: "document".to_owned(),
                    enabled: true,
                    value: MultipartValue::File {
                        path: file.path().to_string_lossy().into_owned(),
                        file_name: Some("report.txt".to_owned()),
                        content_type: Some("text/plain".to_owned()),
                    },
                },
            ],
        };

        HttpExecutor.execute(definition).await.unwrap();
        let request = captured.recv_timeout(Duration::from_secs(1)).unwrap();
        let lowercase = request.to_ascii_lowercase();

        assert!(lowercase.contains("content-type: multipart/form-data; boundary="));
        assert!(!lowercase.contains("content-type: application/json"));
        assert!(request.contains("name=\"description\""));
        assert!(request.contains("quarterly report"));
        assert!(request.contains("name=\"document\"; filename=\"report.txt\""));
        assert!(request.contains("file payload"));
    }

    #[tokio::test]
    async fn rejects_request_bodies_above_the_configured_limit() {
        let mut definition = request("https://example.com/upload".to_owned());
        definition.method = "POST".to_owned();
        definition.options.max_request_bytes = 3;
        definition.body = RequestBody::Text {
            content: "four".to_owned(),
            content_type: Some("text/plain".to_owned()),
        };

        let error = HttpExecutor.execute(definition).await.unwrap_err();
        assert!(matches!(
            error,
            HttpExecutionError::RequestBodyTooLarge {
                actual: 4,
                limit: 3
            }
        ));
    }

    #[test]
    fn cookie_jar_applies_domain_path_and_security_metadata() {
        let jar = CookieJar::default();
        let url = Url::parse("https://api.example.com/v1/login").unwrap();
        jar.add(
            "session=private; Domain=example.com; Path=/v1; Secure; HttpOnly; SameSite=Lax",
            &url,
        )
        .unwrap();

        let cookies = jar.list().unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "session");
        assert_eq!(cookies[0].domain, "example.com");
        assert_eq!(cookies[0].path, "/v1");
        assert!(cookies[0].secure);
        assert!(cookies[0].http_only);
        assert_eq!(cookies[0].same_site.as_deref(), Some("Lax"));
        assert!(jar.remove("example.com", "/v1", "session").unwrap());
        assert!(jar.list().unwrap().is_empty());
    }
}
