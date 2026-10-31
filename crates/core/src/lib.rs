//! Protocol-independent models shared by the desktop application, CLI, and
//! execution engines.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_MAX_REQUEST_BYTES: u64 = 100 * 1024 * 1024;
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDefinition {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub query_params: Vec<KeyValueEntry>,
    #[serde(default)]
    pub headers: Vec<HeaderEntry>,
    #[serde(default)]
    pub auth: RequestAuth,
    #[serde(default)]
    pub body: RequestBody,
    #[serde(default)]
    pub options: RequestOptions,
}

impl RequestDefinition {
    pub fn validate(&self) -> Result<(), RequestValidationError> {
        if self.method.trim().is_empty() {
            return Err(RequestValidationError::EmptyMethod);
        }

        let url = Url::parse(self.url.trim())
            .map_err(|error| RequestValidationError::InvalidUrl(error.to_string()))?;

        if !matches!(url.scheme(), "http" | "https") {
            return Err(RequestValidationError::UnsupportedScheme(
                url.scheme().to_owned(),
            ));
        }

        if !url.username().is_empty() || url.password().is_some() {
            return Err(RequestValidationError::CredentialsInUrl);
        }

        if self.options.timeout_ms == 0 {
            return Err(RequestValidationError::ZeroTimeout);
        }

        if self.options.max_request_bytes == 0 {
            return Err(RequestValidationError::ZeroRequestLimit);
        }

        if self.options.max_response_bytes == 0 {
            return Err(RequestValidationError::ZeroResponseLimit);
        }

        match &self.auth {
            RequestAuth::Bearer { token } if token.trim().is_empty() => {
                return Err(RequestValidationError::EmptyBearerToken);
            }
            RequestAuth::ApiKey { key, .. } if key.trim().is_empty() => {
                return Err(RequestValidationError::EmptyApiKeyName);
            }
            RequestAuth::Inherit => {
                return Err(RequestValidationError::UnresolvedInheritedAuth);
            }
            RequestAuth::OAuth2ClientCredentials { .. } => {
                return Err(RequestValidationError::UnresolvedOAuth2Auth);
            }
            _ => {}
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableResolution {
    pub request: RequestDefinition,
    pub unresolved: Vec<String>,
    pub cycles: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextVariableResolution {
    pub value: String,
    pub unresolved: Vec<String>,
    pub cycles: Vec<String>,
    pub errors: Vec<String>,
}

const MAX_VARIABLE_DEPTH: usize = 64;
const MAX_VARIABLE_SUBSTITUTIONS: usize = 100_000;
const MAX_VARIABLE_EXPANSION_BYTES: usize = 16 * 1024 * 1024;
const VARIABLE_DEPTH_ERROR: &str = "Variable expansion exceeded the nesting depth limit (64).";
const VARIABLE_WORK_ERROR: &str =
    "Variable expansion exceeded the substitution work limit (100000).";
const VARIABLE_BYTES_ERROR: &str =
    "Variable expansion exceeded the cumulative byte limit (16 MiB).";

#[derive(Default)]
struct VariableExpansionBudget {
    substitutions: usize,
    bytes: usize,
    error: Option<&'static str>,
}

impl VariableExpansionBudget {
    fn substitution(&mut self) -> Result<(), ()> {
        if self.substitutions >= MAX_VARIABLE_SUBSTITUTIONS {
            self.error = Some(VARIABLE_WORK_ERROR);
            return Err(());
        }
        self.substitutions += 1;
        Ok(())
    }

    fn append(&mut self, output: &mut String, value: &str, expanded: bool) -> Result<(), ()> {
        // Account before copying, including intermediate nested expansions. Literal request
        // text is governed by the HTTP body limit rather than the variable expansion limit.
        if expanded {
            if value.len() > MAX_VARIABLE_EXPANSION_BYTES - self.bytes {
                self.error = Some(VARIABLE_BYTES_ERROR);
                return Err(());
            }
            self.bytes += value.len();
        }
        output.push_str(value);
        Ok(())
    }

    fn errors(&self) -> Vec<String> {
        self.error.into_iter().map(str::to_owned).collect()
    }
}

pub fn resolve_text_variables(
    input: &str,
    values: &BTreeMap<String, String>,
) -> TextVariableResolution {
    let mut unresolved = BTreeSet::new();
    let mut cycles = BTreeSet::new();
    let mut budget = VariableExpansionBudget::default();
    let value = resolve_variable_text(
        input,
        values,
        &mut Vec::new(),
        &mut unresolved,
        &mut cycles,
        &mut budget,
    )
    .map(Cow::into_owned)
    .unwrap_or_else(|()| input.to_owned());
    TextVariableResolution {
        value,
        unresolved: unresolved.into_iter().collect(),
        cycles: cycles.into_iter().collect(),
        errors: budget.errors(),
    }
}

pub fn resolve_request_variables(
    request: &RequestDefinition,
    values: &BTreeMap<String, String>,
) -> VariableResolution {
    let mut unresolved = BTreeSet::new();
    let mut cycles = BTreeSet::new();
    let mut budget = VariableExpansionBudget::default();
    let mut resolve = |value: &str| {
        resolve_variable_text(
            value,
            values,
            &mut Vec::new(),
            &mut unresolved,
            &mut cycles,
            &mut budget,
        )
        .map(Cow::into_owned)
        .unwrap_or_else(|()| value.to_owned())
    };

    let mut resolved = request.clone();
    resolved.url = resolve(&resolved.url);
    for parameter in resolved
        .query_params
        .iter_mut()
        .filter(|entry| entry.enabled)
    {
        parameter.name = resolve(&parameter.name);
        parameter.value = resolve(&parameter.value);
    }
    for header in resolved.headers.iter_mut().filter(|entry| entry.enabled) {
        header.name = resolve(&header.name);
        header.value = resolve(&header.value);
    }
    match &mut resolved.auth {
        RequestAuth::None | RequestAuth::Inherit => {}
        RequestAuth::Basic { username, password } => {
            *username = resolve(username);
            *password = resolve(password);
        }
        RequestAuth::Bearer { token } => *token = resolve(token),
        RequestAuth::ApiKey { key, value, .. } => {
            *key = resolve(key);
            *value = resolve(value);
        }
        RequestAuth::OAuth2ClientCredentials {
            issuer_url,
            token_url,
            client_id,
            client_secret,
            scope,
            audience,
            extra_parameters,
            ..
        } => {
            *issuer_url = resolve(issuer_url);
            *token_url = resolve(token_url);
            *client_id = resolve(client_id);
            *client_secret = resolve(client_secret);
            *scope = resolve(scope);
            *audience = resolve(audience);
            for parameter in extra_parameters.iter_mut().filter(|entry| entry.enabled) {
                parameter.name = resolve(&parameter.name);
                parameter.value = resolve(&parameter.value);
            }
        }
    }
    match &mut resolved.body {
        RequestBody::Empty => {}
        RequestBody::Text {
            content,
            content_type,
        } => {
            *content = resolve(content);
            if let Some(content_type) = content_type {
                *content_type = resolve(content_type);
            }
        }
        RequestBody::FormUrlEncoded { fields } => {
            for field in fields.iter_mut().filter(|entry| entry.enabled) {
                field.name = resolve(&field.name);
                field.value = resolve(&field.value);
            }
        }
        RequestBody::Multipart { parts } => {
            for part in parts.iter_mut().filter(|entry| entry.enabled) {
                part.name = resolve(&part.name);
                match &mut part.value {
                    MultipartValue::Text {
                        content,
                        content_type,
                    } => {
                        *content = resolve(content);
                        if let Some(content_type) = content_type {
                            *content_type = resolve(content_type);
                        }
                    }
                    MultipartValue::File {
                        path,
                        file_name,
                        content_type,
                    } => {
                        *path = resolve(path);
                        if let Some(file_name) = file_name {
                            *file_name = resolve(file_name);
                        }
                        if let Some(content_type) = content_type {
                            *content_type = resolve(content_type);
                        }
                    }
                }
            }
        }
        RequestBody::Binary { path, content_type } => {
            *path = resolve(path);
            if let Some(content_type) = content_type {
                *content_type = resolve(content_type);
            }
        }
    }

    VariableResolution {
        request: resolved,
        unresolved: unresolved.into_iter().collect(),
        cycles: cycles.into_iter().collect(),
        errors: budget.errors(),
    }
}

fn resolve_variable_text<'a>(
    input: &'a str,
    values: &'a BTreeMap<String, String>,
    stack: &mut Vec<String>,
    unresolved: &mut BTreeSet<String>,
    cycles: &mut BTreeSet<String>,
    budget: &mut VariableExpansionBudget,
) -> Result<Cow<'a, str>, ()> {
    if budget.error.is_some() {
        return Err(());
    }
    if !input.contains("{{") {
        return Ok(Cow::Borrowed(input));
    }
    let mut output = String::new();
    let mut cursor = 0usize;
    while let Some(relative_start) = input[cursor..].find("{{") {
        let start = cursor + relative_start;
        budget.append(&mut output, &input[cursor..start], !stack.is_empty())?;
        let token_start = start + 2;
        let Some(relative_end) = input[token_start..].find("}}") else {
            budget.append(&mut output, &input[start..], !stack.is_empty())?;
            return Ok(Cow::Owned(output));
        };
        let end = token_start + relative_end;
        let raw_key = &input[token_start..end];
        let key = raw_key.trim();
        let token_end = end + 2;
        if key.is_empty() || key.contains(['{', '}']) {
            budget.append(&mut output, &input[start..token_end], !stack.is_empty())?;
        } else if let Some(value) = values.get(key) {
            budget.substitution()?;
            if stack.iter().any(|parent| parent == key) {
                let mut path = stack.clone();
                path.push(key.to_owned());
                cycles.insert(path.join(" → "));
                budget.append(&mut output, &input[start..token_end], true)?;
            } else {
                if stack.len() >= MAX_VARIABLE_DEPTH {
                    budget.error = Some(VARIABLE_DEPTH_ERROR);
                    return Err(());
                }
                stack.push(key.to_owned());
                let replacement =
                    resolve_variable_text(value, values, stack, unresolved, cycles, budget);
                stack.pop();
                budget.append(&mut output, &replacement?, true)?;
            }
        } else {
            budget.substitution()?;
            unresolved.insert(key.to_owned());
            budget.append(&mut output, &input[start..token_end], true)?;
        }
        cursor = token_end;
    }
    budget.append(&mut output, &input[cursor..], !stack.is_empty())?;
    Ok(Cow::Owned(output))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyValueEntry {
    pub name: String,
    pub value: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RequestAuth {
    #[default]
    None,
    Inherit,
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
    },
    ApiKey {
        key: String,
        value: String,
        placement: ApiKeyPlacement,
    },
    OAuth2ClientCredentials {
        #[serde(default)]
        issuer_url: String,
        #[serde(default)]
        token_url: String,
        client_id: String,
        client_secret: String,
        #[serde(default)]
        scope: String,
        #[serde(default)]
        audience: String,
        #[serde(default)]
        client_auth: OAuth2ClientAuth,
        #[serde(default)]
        extra_parameters: Vec<KeyValueEntry>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiKeyPlacement {
    Header,
    Query,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2ClientAuth {
    #[default]
    Basic,
    RequestBody,
}

const fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RequestBody {
    #[default]
    Empty,
    Text {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    FormUrlEncoded {
        #[serde(default)]
        fields: Vec<KeyValueEntry>,
    },
    Multipart {
        #[serde(default)]
        parts: Vec<MultipartPart>,
    },
    Binary {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultipartPart {
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    pub value: MultipartValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MultipartValue {
    Text {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestOptions {
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: u64,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: u64,
    #[serde(default = "follow_redirects_by_default")]
    pub follow_redirects: bool,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            follow_redirects: true,
        }
    }
}

const fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

const fn default_max_response_bytes() -> u64 {
    DEFAULT_MAX_RESPONSE_BYTES
}

const fn default_max_request_bytes() -> u64 {
    DEFAULT_MAX_REQUEST_BYTES
}

const fn follow_redirects_by_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSnapshot {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<HeaderEntry>,
    pub body: ResponseBody,
    pub timing: ResponseTiming,
    pub received_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseBody {
    pub content: String,
    pub encoding: ResponseBodyEncoding,
    pub content_type: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResponseBodyEncoding {
    Utf8,
    Base64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseTiming {
    pub total_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RequestValidationError {
    #[error("request method cannot be empty")]
    EmptyMethod,
    #[error("invalid request URL: {0}")]
    InvalidUrl(String),
    #[error("unsupported URL scheme '{0}'; only http and https are allowed")]
    UnsupportedScheme(String),
    #[error("credentials cannot be embedded in the URL; use an authentication setting")]
    CredentialsInUrl,
    #[error("request timeout must be greater than zero")]
    ZeroTimeout,
    #[error("maximum request size must be greater than zero")]
    ZeroRequestLimit,
    #[error("maximum response size must be greater than zero")]
    ZeroResponseLimit,
    #[error("bearer token cannot be empty")]
    EmptyBearerToken,
    #[error("API key name cannot be empty")]
    EmptyApiKeyName,
    #[error("inherited authentication must be resolved before execution")]
    UnresolvedInheritedAuth,
    #[error("OAuth 2 authentication must be resolved before execution")]
    UnresolvedOAuth2Auth,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_request() -> RequestDefinition {
        RequestDefinition {
            method: "GET".to_owned(),
            url: "https://example.com/users".to_owned(),
            query_params: Vec::new(),
            headers: Vec::new(),
            auth: RequestAuth::None,
            body: RequestBody::Empty,
            options: RequestOptions::default(),
        }
    }

    #[test]
    fn accepts_http_requests() {
        assert_eq!(valid_request().validate(), Ok(()));
    }

    #[test]
    fn rejects_non_http_schemes() {
        let mut request = valid_request();
        request.url = "file:///private/data".to_owned();

        assert_eq!(
            request.validate(),
            Err(RequestValidationError::UnsupportedScheme("file".to_owned()))
        );
    }

    #[test]
    fn rejects_credentials_embedded_in_urls() {
        let mut request = valid_request();
        request.url = "https://user:secret@example.com".to_owned();

        assert_eq!(
            request.validate(),
            Err(RequestValidationError::CredentialsInUrl)
        );
    }

    #[test]
    fn defaults_are_safe_and_bounded() {
        let options = RequestOptions::default();

        assert_eq!(options.timeout_ms, 30_000);
        assert_eq!(options.max_request_bytes, 100 * 1024 * 1024);
        assert_eq!(options.max_response_bytes, 10 * 1024 * 1024);
        assert!(options.follow_redirects);
    }

    #[test]
    fn request_body_uses_the_frontend_ipc_shape() {
        let body = RequestBody::Text {
            content: "{}".to_owned(),
            content_type: Some("application/json".to_owned()),
        };

        let json = serde_json::to_value(&body).unwrap();

        assert_eq!(json["kind"], "text");
        assert_eq!(json["contentType"], "application/json");
        assert!(json.get("content_type").is_none());

        let form = RequestBody::FormUrlEncoded {
            fields: vec![KeyValueEntry {
                name: "grant_type".to_owned(),
                value: "client_credentials".to_owned(),
                enabled: true,
            }],
        };
        let json = serde_json::to_value(&form).unwrap();
        assert_eq!(json["kind"], "formUrlEncoded");
        assert_eq!(json["fields"][0]["name"], "grant_type");

        let multipart = RequestBody::Multipart {
            parts: vec![MultipartPart {
                name: "document".to_owned(),
                enabled: true,
                value: MultipartValue::File {
                    path: "C:\\files\\report.pdf".to_owned(),
                    file_name: Some("report.pdf".to_owned()),
                    content_type: Some("application/pdf".to_owned()),
                },
            }],
        };
        let json = serde_json::to_value(&multipart).unwrap();
        assert_eq!(json["kind"], "multipart");
        assert_eq!(json["parts"][0]["value"]["kind"], "file");
        assert_eq!(json["parts"][0]["value"]["fileName"], "report.pdf");

        let binary = RequestBody::Binary {
            path: "/tmp/archive.zip".to_owned(),
            content_type: Some("application/zip".to_owned()),
        };
        let json = serde_json::to_value(&binary).unwrap();
        assert_eq!(json["kind"], "binary");
        assert_eq!(json["contentType"], "application/zip");
    }

    #[test]
    fn request_auth_uses_the_frontend_ipc_shape() {
        let auth = RequestAuth::ApiKey {
            key: "X-API-Key".to_owned(),
            value: "{{apiKey}}".to_owned(),
            placement: ApiKeyPlacement::Header,
        };

        let json = serde_json::to_value(&auth).unwrap();

        assert_eq!(json["kind"], "apiKey");
        assert_eq!(json["placement"], "header");
        assert!(json.get("api_key").is_none());
    }

    #[test]
    fn validates_auth_configuration() {
        let mut request = valid_request();
        for token in ["", " ", "\t\r\n"] {
            request.auth = RequestAuth::Bearer {
                token: token.to_owned(),
            };
            assert_eq!(
                request.validate(),
                Err(RequestValidationError::EmptyBearerToken)
            );
        }

        request.auth = RequestAuth::ApiKey {
            key: "  ".to_owned(),
            value: "value".to_owned(),
            placement: ApiKeyPlacement::Query,
        };
        assert_eq!(
            request.validate(),
            Err(RequestValidationError::EmptyApiKeyName)
        );

        request.auth = RequestAuth::Inherit;
        assert_eq!(
            request.validate(),
            Err(RequestValidationError::UnresolvedInheritedAuth)
        );

        request.auth = RequestAuth::OAuth2ClientCredentials {
            issuer_url: String::new(),
            token_url: "https://id.example.com/token".to_owned(),
            client_id: "client".to_owned(),
            client_secret: "{{clientSecret}}".to_owned(),
            scope: "read".to_owned(),
            audience: String::new(),
            client_auth: OAuth2ClientAuth::Basic,
            extra_parameters: Vec::new(),
        };
        assert_eq!(
            request.validate(),
            Err(RequestValidationError::UnresolvedOAuth2Auth)
        );
    }

    #[test]
    fn deserializes_requests_from_older_workspace_schemas() {
        let request: RequestDefinition = serde_json::from_value(serde_json::json!({
            "method": "GET",
            "url": "https://example.com",
            "headers": [],
            "body": { "kind": "empty" },
            "options": {
                "timeoutMs": 30000,
                "maxResponseBytes": 10485760,
                "followRedirects": true
            }
        }))
        .unwrap();

        assert!(request.query_params.is_empty());
        assert_eq!(request.auth, RequestAuth::None);
        assert_eq!(request.options.max_request_bytes, DEFAULT_MAX_REQUEST_BYTES);
    }

    #[test]
    fn resolves_nested_variables_across_request_fields() {
        let mut request = valid_request();
        request.url = "{{baseUrl}}/{{resource}}".to_owned();
        request.headers.push(HeaderEntry {
            name: "X-{{headerName}}".to_owned(),
            value: "{{token}}".to_owned(),
            enabled: true,
        });
        request.auth = RequestAuth::Bearer {
            token: "{{token}}".to_owned(),
        };
        request.body = RequestBody::Text {
            content: "{\"owner\":\"{{owner}}\"}".to_owned(),
            content_type: Some("application/{{format}}".to_owned()),
        };
        let values = BTreeMap::from([
            ("host".to_owned(), "api.example.com".to_owned()),
            ("baseUrl".to_owned(), "https://{{host}}".to_owned()),
            ("resource".to_owned(), "users".to_owned()),
            ("headerName".to_owned(), "Token".to_owned()),
            ("token".to_owned(), "local-secret".to_owned()),
            ("owner".to_owned(), "Ada".to_owned()),
            ("format".to_owned(), "json".to_owned()),
        ]);

        let resolution = resolve_request_variables(&request, &values);
        assert_eq!(resolution.request.url, "https://api.example.com/users");
        assert_eq!(resolution.request.headers[0].name, "X-Token");
        assert_eq!(resolution.request.headers[0].value, "local-secret");
        assert!(matches!(
            resolution.request.auth,
            RequestAuth::Bearer { ref token } if token == "local-secret"
        ));
        assert!(matches!(
            resolution.request.body,
            RequestBody::Text { ref content, ref content_type }
                if content == "{\"owner\":\"Ada\"}"
                    && content_type.as_deref() == Some("application/json")
        ));
        assert!(resolution.unresolved.is_empty());
        assert!(resolution.cycles.is_empty());
    }

    const OPTIONAL_ENTRY_KINDS: [&str; 6] = [
        "header",
        "query",
        "form",
        "multipart-text",
        "multipart-file",
        "oauth-extra",
    ];

    fn request_with_optional_variables(kind: &str, enabled: bool) -> RequestDefinition {
        let mut request = valid_request();
        let template = "{{known}}/{{missing}}/{{first}}";
        let entry = || KeyValueEntry {
            name: template.to_owned(),
            value: template.to_owned(),
            enabled,
        };
        match kind {
            "header" => request.headers.push(HeaderEntry {
                name: template.to_owned(),
                value: template.to_owned(),
                enabled,
            }),
            "query" => request.query_params.push(entry()),
            "form" => {
                request.body = RequestBody::FormUrlEncoded {
                    fields: vec![entry()],
                };
            }
            "multipart-text" | "multipart-file" => {
                let value = if kind == "multipart-text" {
                    MultipartValue::Text {
                        content: template.to_owned(),
                        content_type: Some(template.to_owned()),
                    }
                } else {
                    MultipartValue::File {
                        path: template.to_owned(),
                        file_name: Some(template.to_owned()),
                        content_type: Some(template.to_owned()),
                    }
                };
                request.body = RequestBody::Multipart {
                    parts: vec![MultipartPart {
                        name: template.to_owned(),
                        enabled,
                        value,
                    }],
                };
            }
            "oauth-extra" => {
                request.auth = RequestAuth::OAuth2ClientCredentials {
                    issuer_url: String::new(),
                    token_url: "https://id.example.com/token".to_owned(),
                    client_id: "client".to_owned(),
                    client_secret: "secret".to_owned(),
                    scope: String::new(),
                    audience: String::new(),
                    client_auth: OAuth2ClientAuth::Basic,
                    extra_parameters: vec![entry()],
                };
            }
            _ => panic!("unknown optional entry kind: {kind}"),
        }
        request
    }

    fn optional_entry_values() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("known".to_owned(), "resolved".to_owned()),
            ("first".to_owned(), "{{second}}".to_owned()),
            ("second".to_owned(), "{{first}}".to_owned()),
        ])
    }

    #[test]
    fn disabled_entries_preserve_templates_without_missing_or_cycle_diagnostics() {
        for kind in OPTIONAL_ENTRY_KINDS {
            let request = request_with_optional_variables(kind, false);
            let resolution = resolve_request_variables(&request, &optional_entry_values());

            assert_eq!(resolution.request, request, "disabled {kind} was changed");
            assert!(resolution.unresolved.is_empty(), "disabled {kind}");
            assert!(resolution.cycles.is_empty(), "disabled {kind}");
            assert!(resolution.errors.is_empty(), "disabled {kind}");
        }
    }

    #[test]
    fn enabled_entries_still_resolve_and_report_missing_values_and_cycles() {
        for kind in OPTIONAL_ENTRY_KINDS {
            let request = request_with_optional_variables(kind, true);
            let resolution = resolve_request_variables(&request, &optional_entry_values());

            assert_eq!(resolution.unresolved, vec!["missing"], "enabled {kind}");
            assert_eq!(
                resolution.cycles,
                vec!["first → second → first"],
                "enabled {kind}"
            );
            let serialized = serde_json::to_string(&resolution.request).unwrap();
            assert!(!serialized.contains("{{known}}"), "enabled {kind}");
            assert!(serialized.contains("{{missing}}"), "enabled {kind}");
            assert!(serialized.contains("{{first}}"), "enabled {kind}");
        }
    }

    #[test]
    fn disabled_entries_do_not_hide_enabled_entry_diagnostics() {
        let mut request = request_with_optional_variables("header", false);
        request.query_params.push(KeyValueEntry {
            name: "active".to_owned(),
            value: "{{activeMissing}}".to_owned(),
            enabled: true,
        });

        let resolution = resolve_request_variables(&request, &optional_entry_values());

        assert_eq!(resolution.request.headers, request.headers);
        assert_eq!(resolution.unresolved, vec!["activeMissing"]);
        assert!(resolution.cycles.is_empty());
    }

    #[test]
    fn resolves_oauth_client_credentials_without_losing_configuration() {
        let mut request = valid_request();
        request.auth = RequestAuth::OAuth2ClientCredentials {
            issuer_url: String::new(),
            token_url: "{{issuer}}/token".to_owned(),
            client_id: "{{clientId}}".to_owned(),
            client_secret: "{{clientSecret}}".to_owned(),
            scope: "read {{scopeSuffix}}".to_owned(),
            audience: "{{audience}}".to_owned(),
            client_auth: OAuth2ClientAuth::RequestBody,
            extra_parameters: vec![KeyValueEntry {
                name: "resource".to_owned(),
                value: "{{resource}}".to_owned(),
                enabled: true,
            }],
        };
        let values = BTreeMap::from([
            ("issuer".to_owned(), "https://id.example.com".to_owned()),
            ("clientId".to_owned(), "desktop".to_owned()),
            ("clientSecret".to_owned(), "secret".to_owned()),
            ("scopeSuffix".to_owned(), "write".to_owned()),
            ("audience".to_owned(), "payments".to_owned()),
            ("resource".to_owned(), "ledger".to_owned()),
        ]);

        let resolution = resolve_request_variables(&request, &values);

        assert!(resolution.unresolved.is_empty());
        assert!(matches!(
            resolution.request.auth,
            RequestAuth::OAuth2ClientCredentials {
                ref token_url,
                ref client_id,
                ref client_secret,
                ref scope,
                ref audience,
                client_auth: OAuth2ClientAuth::RequestBody,
                ref extra_parameters,
                ..
            } if token_url == "https://id.example.com/token"
                && client_id == "desktop"
                && client_secret == "secret"
                && scope == "read write"
                && audience == "payments"
                && extra_parameters[0].value == "ledger"
        ));
    }

    #[test]
    fn reports_unresolved_variables_and_cycles_without_erasing_tokens() {
        let mut request = valid_request();
        request.url = "https://example.com/{{missing}}/{{first}}".to_owned();
        let values = BTreeMap::from([
            ("first".to_owned(), "{{second}}".to_owned()),
            ("second".to_owned(), "{{first}}".to_owned()),
        ]);

        let resolution = resolve_request_variables(&request, &values);
        assert_eq!(resolution.unresolved, vec!["missing"]);
        assert_eq!(resolution.cycles, vec!["first → second → first"]);
        assert!(resolution.request.url.contains("{{missing}}"));
        assert!(resolution.request.url.contains("{{first}}"));
        assert!(resolution.errors.is_empty());
    }

    fn nested_variable_values(
        depth: usize,
        duplicate: bool,
        leaf: &str,
    ) -> BTreeMap<String, String> {
        let mut values = BTreeMap::from([("v0".to_owned(), leaf.to_owned())]);
        for index in 1..=depth {
            let previous = format!("{{{{v{}}}}}", index - 1);
            values.insert(
                format!("v{index}"),
                if duplicate {
                    previous.repeat(2)
                } else {
                    previous
                },
            );
        }
        values
    }

    fn assert_expansion_error(input: &str, values: &BTreeMap<String, String>, expected: &str) {
        let text = resolve_text_variables(input, values);
        assert_eq!(text.errors, vec![expected]);
        assert_eq!(text.value, input);
        assert!(text.cycles.is_empty());
        assert!(text.unresolved.is_empty());

        let mut request = valid_request();
        request.body = RequestBody::Text {
            content: input.to_owned(),
            content_type: None,
        };
        let resolved = resolve_request_variables(&request, values);
        assert_eq!(resolved.errors, vec![expected]);
        assert_eq!(resolved.request.body, request.body);
        assert!(resolved.cycles.is_empty());
        assert!(resolved.unresolved.is_empty());
    }

    #[test]
    fn variable_expansion_depth_is_bounded_without_reclassifying_cycles() {
        let allowed = resolve_text_variables("{{v63}}", &nested_variable_values(63, false, "x"));
        assert_eq!(allowed.value, "x");
        assert!(allowed.errors.is_empty());
        assert_expansion_error(
            "{{v64}}",
            &nested_variable_values(64, false, "x"),
            VARIABLE_DEPTH_ERROR,
        );
    }

    #[test]
    fn exponential_variable_expansion_stops_at_the_work_limit() {
        assert_expansion_error(
            "{{v40}}",
            &nested_variable_values(40, true, "x"),
            VARIABLE_WORK_ERROR,
        );
    }

    #[test]
    fn variable_expansion_bounds_cumulative_utf8_bytes_before_allocating_large_results() {
        assert_expansion_error(
            "{{v12}}",
            &nested_variable_values(12, true, &"🔑".repeat(1024)),
            VARIABLE_BYTES_ERROR,
        );
        // A single oversized leaf must also be rejected before copying it into output.
        assert_expansion_error(
            "{{v0}}",
            &nested_variable_values(0, false, &"x".repeat(MAX_VARIABLE_EXPANSION_BYTES + 1)),
            VARIABLE_BYTES_ERROR,
        );
    }

    #[test]
    fn variable_expansion_budgets_are_shared_across_request_fields() {
        let mut request = valid_request();
        request.headers = vec![
            HeaderEntry {
                name: "First".to_owned(),
                value: "{{large}}".to_owned(),
                enabled: true,
            },
            HeaderEntry {
                name: "Second".to_owned(),
                value: "{{large}}".to_owned(),
                enabled: true,
            },
        ];
        request.body = RequestBody::Text {
            content: "{{small}}".to_owned(),
            content_type: None,
        };
        let values = BTreeMap::from([
            (
                "large".to_owned(),
                "x".repeat(MAX_VARIABLE_EXPANSION_BYTES / 2),
            ),
            ("small".to_owned(), "x".to_owned()),
        ]);
        let resolution = resolve_request_variables(&request, &values);
        assert_eq!(resolution.errors, vec![VARIABLE_BYTES_ERROR]);
        assert_eq!(
            resolution.request.headers[0].value.len(),
            MAX_VARIABLE_EXPANSION_BYTES / 2
        );
        assert_eq!(
            resolution.request.headers[1].value.len(),
            MAX_VARIABLE_EXPANSION_BYTES / 2
        );
        assert_eq!(resolution.request.body, request.body);

        for header in &mut request.headers {
            header.value = "{{empty}}".repeat(MAX_VARIABLE_SUBSTITUTIONS / 2);
        }
        request.body = RequestBody::Text {
            content: "{{empty}}".to_owned(),
            content_type: None,
        };
        let resolution = resolve_request_variables(
            &request,
            &BTreeMap::from([("empty".to_owned(), String::new())]),
        );
        assert_eq!(resolution.errors, vec![VARIABLE_WORK_ERROR]);
        assert!(
            resolution
                .request
                .headers
                .iter()
                .all(|header| header.value.is_empty())
        );
        assert_eq!(resolution.request.body, request.body);
    }

    #[test]
    fn large_literal_bodies_do_not_consume_the_variable_expansion_budget() {
        let body = "x".repeat(MAX_VARIABLE_EXPANSION_BYTES + 1);
        let text = resolve_text_variables(&body, &BTreeMap::new());
        assert!(text.errors.is_empty());
        assert_eq!(text.value, body);
        let mut request = valid_request();
        request.body = RequestBody::Text {
            content: body,
            content_type: None,
        };
        let resolution = resolve_request_variables(&request, &BTreeMap::new());
        assert!(resolution.errors.is_empty());
        assert_eq!(resolution.request.body, request.body);
    }

    #[test]
    fn resolves_standalone_text_with_nested_values() {
        let values = BTreeMap::from([
            ("proxyHost".to_owned(), "proxy.internal".to_owned()),
            (
                "proxyUrl".to_owned(),
                "http://{{proxyHost}}:8080".to_owned(),
            ),
        ]);

        let resolution = resolve_text_variables("{{proxyUrl}}", &values);

        assert_eq!(resolution.value, "http://proxy.internal:8080");
        assert!(resolution.unresolved.is_empty());
        assert!(resolution.cycles.is_empty());
    }

    #[test]
    fn standalone_text_reports_missing_values_and_cycles() {
        let values = BTreeMap::from([
            ("first".to_owned(), "{{second}}".to_owned()),
            ("second".to_owned(), "{{first}}".to_owned()),
        ]);

        let resolution = resolve_text_variables("{{missing}}/{{first}}", &values);

        assert_eq!(resolution.unresolved, vec!["missing"]);
        assert_eq!(resolution.cycles, vec!["first → second → first"]);
        assert!(resolution.errors.is_empty());
    }
}
