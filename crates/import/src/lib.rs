//! Import adapters for external request formats.

mod openapi;
mod postman;

use api_client_core::{
    ApiKeyPlacement, HeaderEntry, MultipartPart, MultipartValue, RequestAuth, RequestBody,
    RequestDefinition, RequestOptions,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use api_client_workspace::{Collection, Environment};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiDocumentKind {
    PostmanCollection,
    PostmanEnvironment,
    OpenApi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiDocumentImport {
    pub kind: ApiDocumentKind,
    pub name: String,
    pub collections: Vec<Collection>,
    pub environments: Vec<Environment>,
    pub warnings: Vec<String>,
}

pub fn import_api_document(
    contents: &[u8],
    file_name: &str,
) -> Result<ApiDocumentImport, ApiDocumentImportError> {
    let text = std::str::from_utf8(contents).map_err(|_| ApiDocumentImportError::NotUtf8)?;
    let document = serde_json::from_str::<serde_json::Value>(text)
        .or_else(|_| serde_yaml_ng::from_str::<serde_json::Value>(text))
        .map_err(|_| ApiDocumentImportError::InvalidDocument)?;
    let object = document
        .as_object()
        .ok_or(ApiDocumentImportError::InvalidDocument)?;

    if object.contains_key("openapi") || object.contains_key("swagger") {
        return openapi::import_openapi(&document, file_name);
    }
    if object.contains_key("item") && object.contains_key("info") {
        return postman::import_postman_collection(&document);
    }
    if object.contains_key("values")
        && (object.contains_key("_postman_variable_scope") || object.contains_key("name"))
    {
        return postman::import_postman_environment(&document);
    }
    Err(ApiDocumentImportError::UnsupportedFormat)
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApiDocumentImportError {
    #[error("API document must be UTF-8 text")]
    NotUtf8,
    #[error("API document is not valid JSON or YAML")]
    InvalidDocument,
    #[error("file is not a supported Postman, OpenAPI, or Swagger document")]
    UnsupportedFormat,
    #[error("API document is invalid: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurlImport {
    pub request: RequestDefinition,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CurlShell {
    Posix,
    PowerShell,
}

pub fn export_curl(
    request: &RequestDefinition,
    shell: CurlShell,
) -> Result<String, CurlExportError> {
    request
        .validate()
        .map_err(|error| CurlExportError::InvalidRequest(error.to_string()))?;

    let mut url = Url::parse(request.url.trim())
        .map_err(|error| CurlExportError::InvalidRequest(error.to_string()))?;
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

    let mut arguments = Vec::new();
    if request.options.follow_redirects {
        arguments.push("--location".to_owned());
    }
    arguments.push("--max-time".to_owned());
    arguments.push(format_duration_seconds(request.options.timeout_ms));
    arguments.push("--request".to_owned());
    arguments.push(request.method.trim().to_owned());

    let is_multipart = matches!(request.body, RequestBody::Multipart { .. });
    let has_body = !matches!(request.body, RequestBody::Empty);
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

    for header in request.headers.iter().filter(|header| {
        header.enabled && !(header.name.trim().is_empty() && header.value.is_empty())
    }) {
        if (is_multipart && header.name.trim().eq_ignore_ascii_case("content-type"))
            || (has_body && header.name.trim().eq_ignore_ascii_case("content-length"))
            || auth_header_name.is_some_and(|name| header.name.trim().eq_ignore_ascii_case(name))
        {
            continue;
        }
        push_header_argument(&mut arguments, &header.name, &header.value);
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
            arguments.push("--user".to_owned());
            arguments.push(format!("{username}:{password}"));
        }
        RequestAuth::Bearer { token } => {
            push_header_argument(
                &mut arguments,
                "Authorization",
                &format!("Bearer {}", token.trim()),
            );
        }
        RequestAuth::ApiKey {
            key,
            value,
            placement: ApiKeyPlacement::Header,
        } => push_header_argument(&mut arguments, key.trim(), value),
    }

    match &request.body {
        RequestBody::Empty => {}
        RequestBody::Text {
            content,
            content_type,
        } => {
            if !has_content_type
                && let Some(content_type) =
                    content_type.as_deref().filter(|value| !value.is_empty())
            {
                push_header_argument(&mut arguments, "Content-Type", content_type);
            }
            arguments.push("--data-raw".to_owned());
            arguments.push(content.clone());
        }
        RequestBody::FormUrlEncoded { fields } => {
            if !has_content_type {
                push_header_argument(
                    &mut arguments,
                    "Content-Type",
                    "application/x-www-form-urlencoded",
                );
            }
            for field in fields
                .iter()
                .filter(|field| field.enabled)
                .filter(|field| !(field.name.is_empty() && field.value.is_empty()))
            {
                arguments.push("--data-urlencode".to_owned());
                arguments.push(format!("{}={}", field.name, field.value));
            }
        }
        RequestBody::Multipart { parts } => {
            for part in parts.iter().filter(|part| part.enabled) {
                match &part.value {
                    MultipartValue::Text {
                        content,
                        content_type,
                    } => {
                        if let Some(content_type) =
                            content_type.as_deref().filter(|value| !value.is_empty())
                        {
                            arguments.push("--form".to_owned());
                            arguments
                                .push(format!("{}={};type={content_type}", part.name, content));
                        } else {
                            arguments.push("--form-string".to_owned());
                            arguments.push(format!("{}={}", part.name, content));
                        }
                    }
                    MultipartValue::File {
                        path,
                        file_name,
                        content_type,
                    } => {
                        let mut specification = format!("{}=@{path}", part.name);
                        if let Some(file_name) =
                            file_name.as_deref().filter(|value| !value.is_empty())
                        {
                            specification.push_str(";filename=");
                            specification.push_str(file_name);
                        }
                        if let Some(content_type) =
                            content_type.as_deref().filter(|value| !value.is_empty())
                        {
                            specification.push_str(";type=");
                            specification.push_str(content_type);
                        }
                        arguments.push("--form".to_owned());
                        arguments.push(specification);
                    }
                }
            }
        }
        RequestBody::Binary { path, content_type } => {
            if !has_content_type
                && let Some(content_type) =
                    content_type.as_deref().filter(|value| !value.is_empty())
            {
                push_header_argument(&mut arguments, "Content-Type", content_type);
            }
            arguments.push("--data-binary".to_owned());
            arguments.push(format!("@{path}"));
        }
    }

    arguments.push(url.into());
    render_command(arguments, shell)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodeLanguage {
    JavaScriptFetch,
    NodeAxios,
    PythonRequests,
}

struct ResolvedExport {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    basic_auth: Option<(String, String)>,
    body: ExportBody,
}

enum ExportBody {
    None,
    Raw(String),
    FormUrlEncoded(Vec<(String, String)>),
    Unsupported(&'static str),
}

/// Generates a runnable code snippet for the resolved request. Values are embedded as literals,
/// same as cURL export, so the snippet may contain credentials the caller must review before sharing.
pub fn export_code(
    request: &RequestDefinition,
    language: CodeLanguage,
) -> Result<String, CurlExportError> {
    let resolved = build_resolved_export(request)?;
    Ok(match language {
        CodeLanguage::JavaScriptFetch => render_javascript_fetch(&resolved),
        CodeLanguage::NodeAxios => render_node_axios(&resolved),
        CodeLanguage::PythonRequests => render_python_requests(&resolved),
    })
}

fn build_resolved_export(request: &RequestDefinition) -> Result<ResolvedExport, CurlExportError> {
    request
        .validate()
        .map_err(|error| CurlExportError::InvalidRequest(error.to_string()))?;

    let mut url = Url::parse(request.url.trim())
        .map_err(|error| CurlExportError::InvalidRequest(error.to_string()))?;
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

    let is_multipart = matches!(request.body, RequestBody::Multipart { .. });
    let has_body = !matches!(request.body, RequestBody::Empty);
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

    let mut headers = Vec::new();
    for header in request.headers.iter().filter(|header| {
        header.enabled && !(header.name.trim().is_empty() && header.value.is_empty())
    }) {
        if (is_multipart && header.name.trim().eq_ignore_ascii_case("content-type"))
            || (has_body && header.name.trim().eq_ignore_ascii_case("content-length"))
            || auth_header_name.is_some_and(|name| header.name.trim().eq_ignore_ascii_case(name))
        {
            continue;
        }
        headers.push((header.name.trim().to_owned(), header.value.clone()));
    }

    let mut basic_auth = None;
    match &request.auth {
        RequestAuth::None
        | RequestAuth::Inherit
        | RequestAuth::OAuth2ClientCredentials { .. }
        | RequestAuth::ApiKey {
            placement: ApiKeyPlacement::Query,
            ..
        } => {}
        RequestAuth::Basic { username, password } => {
            basic_auth = Some((username.clone(), password.clone()));
        }
        RequestAuth::Bearer { token } => {
            headers.push((
                "Authorization".to_owned(),
                format!("Bearer {}", token.trim()),
            ));
        }
        RequestAuth::ApiKey {
            key,
            value,
            placement: ApiKeyPlacement::Header,
        } => headers.push((key.trim().to_owned(), value.clone())),
    }

    let body = match &request.body {
        RequestBody::Empty => ExportBody::None,
        RequestBody::Text {
            content,
            content_type,
        } => {
            if !has_content_type
                && let Some(content_type) =
                    content_type.as_deref().filter(|value| !value.is_empty())
            {
                headers.push(("Content-Type".to_owned(), content_type.to_owned()));
            }
            ExportBody::Raw(content.clone())
        }
        RequestBody::FormUrlEncoded { fields } => {
            if !has_content_type {
                headers.push((
                    "Content-Type".to_owned(),
                    "application/x-www-form-urlencoded".to_owned(),
                ));
            }
            ExportBody::FormUrlEncoded(
                fields
                    .iter()
                    .filter(|field| field.enabled)
                    .filter(|field| !(field.name.is_empty() && field.value.is_empty()))
                    .map(|field| (field.name.clone(), field.value.clone()))
                    .collect(),
            )
        }
        RequestBody::Multipart { .. } => {
            ExportBody::Unsupported("multipart form bodies are not yet supported by code export")
        }
        RequestBody::Binary { .. } => {
            ExportBody::Unsupported("binary file bodies are not yet supported by code export")
        }
    };

    Ok(ResolvedExport {
        method: request.method.trim().to_owned(),
        url: url.into(),
        headers,
        basic_auth,
        body,
    })
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
}

fn render_javascript_fetch(export: &ResolvedExport) -> String {
    let mut prelude = Vec::new();
    let mut header_lines = Vec::new();
    for (name, value) in &export.headers {
        header_lines.push(format!(
            "    {}: {},",
            json_string(name),
            json_string(value)
        ));
    }
    if let Some((username, password)) = &export.basic_auth {
        prelude.push(format!(
            "const basicAuth = \"Basic \" + btoa({} + \":\" + {});",
            json_string(username),
            json_string(password)
        ));
        prelude.push(String::new());
        header_lines.push("    Authorization: basicAuth,".to_owned());
    }

    let mut options = vec![format!("  method: {},", json_string(&export.method))];
    if !header_lines.is_empty() {
        options.push("  headers: {".to_owned());
        options.extend(header_lines);
        options.push("  },".to_owned());
    }
    match &export.body {
        ExportBody::None => {}
        ExportBody::Raw(content) => options.push(format!("  body: {},", json_string(content))),
        ExportBody::FormUrlEncoded(fields) => {
            let pairs = fields
                .iter()
                .map(|(name, value)| format!("[{}, {}]", json_string(name), json_string(value)))
                .collect::<Vec<_>>()
                .join(", ");
            options.push(format!(
                "  body: new URLSearchParams([{pairs}]).toString(),"
            ));
        }
        ExportBody::Unsupported(reason) => {
            options.push(format!("  // {reason}; add the body manually."))
        }
    }

    let mut output = prelude;
    output.push(format!(
        "const response = await fetch({}, {{",
        json_string(&export.url)
    ));
    output.extend(options);
    output.push("});".to_owned());
    output.push(String::new());
    output.push("const data = await response.text();".to_owned());
    output.push("console.log(data);".to_owned());
    output.join("\n")
}

fn render_node_axios(export: &ResolvedExport) -> String {
    let mut header_lines = Vec::new();
    for (name, value) in &export.headers {
        header_lines.push(format!(
            "    {}: {},",
            json_string(name),
            json_string(value)
        ));
    }

    let mut lines = vec![
        "const axios = require(\"axios\");".to_owned(),
        String::new(),
        "const response = await axios({".to_owned(),
        format!("  method: {},", json_string(&export.method)),
        format!("  url: {},", json_string(&export.url)),
    ];
    if !header_lines.is_empty() {
        lines.push("  headers: {".to_owned());
        lines.extend(header_lines);
        lines.push("  },".to_owned());
    }
    if let Some((username, password)) = &export.basic_auth {
        lines.push("  auth: {".to_owned());
        lines.push(format!("    username: {},", json_string(username)));
        lines.push(format!("    password: {},", json_string(password)));
        lines.push("  },".to_owned());
    }
    match &export.body {
        ExportBody::None => {}
        ExportBody::Raw(content) => lines.push(format!("  data: {},", json_string(content))),
        ExportBody::FormUrlEncoded(fields) => {
            let pairs = fields
                .iter()
                .map(|(name, value)| format!("[{}, {}]", json_string(name), json_string(value)))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!(
                "  data: new URLSearchParams([{pairs}]).toString(),"
            ));
        }
        ExportBody::Unsupported(reason) => {
            lines.push(format!("  // {reason}; add the body manually."))
        }
    }
    lines.push("});".to_owned());
    lines.push(String::new());
    lines.push("console.log(response.data);".to_owned());
    lines.join("\n")
}

fn render_python_requests(export: &ResolvedExport) -> String {
    let mut lines = vec!["import requests".to_owned(), String::new()];
    lines.push(format!("url = {}", json_string(&export.url)));
    if !export.headers.is_empty() {
        lines.push("headers = {".to_owned());
        for (name, value) in &export.headers {
            lines.push(format!(
                "    {}: {},",
                json_string(name),
                json_string(value)
            ));
        }
        lines.push("}".to_owned());
    }

    let mut call_args = vec![json_string(&export.method), "url".to_owned()];
    if !export.headers.is_empty() {
        call_args.push("headers=headers".to_owned());
    }
    if let Some((username, password)) = &export.basic_auth {
        call_args.push(format!(
            "auth=({}, {})",
            json_string(username),
            json_string(password)
        ));
    }
    match &export.body {
        ExportBody::None => {}
        ExportBody::Raw(content) => call_args.push(format!("data={}", json_string(content))),
        ExportBody::FormUrlEncoded(fields) => {
            let pairs = fields
                .iter()
                .map(|(name, value)| format!("{}: {}", json_string(name), json_string(value)))
                .collect::<Vec<_>>()
                .join(", ");
            call_args.push(format!("data={{{pairs}}}"));
        }
        ExportBody::Unsupported(reason) => {
            lines.push(format!("# {reason}; add the body manually."))
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "response = requests.request({})",
        call_args.join(", ")
    ));
    lines.push("print(response.status_code)".to_owned());
    lines.push("print(response.text)".to_owned());
    lines.join("\n")
}

fn push_header_argument(arguments: &mut Vec<String>, name: &str, value: &str) {
    arguments.push("--header".to_owned());
    arguments.push(if value.is_empty() {
        format!("{};", name.trim())
    } else {
        format!("{}: {value}", name.trim())
    });
}

fn format_duration_seconds(milliseconds: u64) -> String {
    if milliseconds % 1_000 == 0 {
        return (milliseconds / 1_000).to_string();
    }
    let mut value = format!("{:.3}", milliseconds as f64 / 1_000.0);
    while value.ends_with('0') {
        value.pop();
    }
    value
}

fn render_command(arguments: Vec<String>, shell: CurlShell) -> Result<String, CurlExportError> {
    let (executable, continuation) = match shell {
        CurlShell::Posix => ("curl", " \\\n  "),
        CurlShell::PowerShell => ("curl.exe", " `\r\n  "),
    };
    let rendered = arguments
        .iter()
        .map(|argument| quote_shell_argument(argument, shell))
        .collect::<Result<Vec<_>, _>>()?;
    if rendered.is_empty() {
        Ok(executable.to_owned())
    } else {
        Ok(format!(
            "{executable}{continuation}{}",
            rendered.join(continuation)
        ))
    }
}

fn quote_shell_argument(value: &str, shell: CurlShell) -> Result<String, CurlExportError> {
    if value.contains('\0') {
        return Err(CurlExportError::NulByte);
    }
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        return Ok(value.to_owned());
    }
    Ok(match shell {
        CurlShell::Posix => format!("'{}'", value.replace('\'', "'\"'\"'")),
        CurlShell::PowerShell => format!("'{}'", value.replace('\'', "''")),
    })
}

pub fn import_curl(command: &str) -> Result<CurlImport, CurlImportError> {
    let normalized = normalize_command(command);
    let arguments = shell_words::split(&normalized)
        .map_err(|error| CurlImportError::InvalidSyntax(error.to_string()))?
        .into_iter()
        .map(normalize_argument)
        .collect::<Vec<_>>();

    let Some(executable) = arguments.first() else {
        return Err(CurlImportError::EmptyCommand);
    };
    if !is_curl_executable(executable) {
        return Err(CurlImportError::NotCurlCommand);
    }

    let mut parser = CurlParser::default();
    parser.parse(&arguments[1..])?;
    parser.finish()
}

#[derive(Default)]
struct CurlParser {
    explicit_method: Option<String>,
    url: Option<String>,
    headers: Vec<HeaderEntry>,
    auth: RequestAuth,
    data: Vec<String>,
    multipart_parts: Vec<MultipartPart>,
    binary_file: Option<String>,
    upload_file: bool,
    has_data: bool,
    head: bool,
    options: RequestOptions,
    warnings: Vec<String>,
}

impl CurlParser {
    fn parse(&mut self, arguments: &[String]) -> Result<(), CurlImportError> {
        self.options = RequestOptions::default();
        let mut index = 0usize;

        while index < arguments.len() {
            let argument = &arguments[index];
            match argument.as_str() {
                "-X" | "--request" => {
                    self.explicit_method = Some(required_value(arguments, &mut index, argument)?);
                }
                "-H" | "--header" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    self.push_header(&value)?;
                }
                "--url" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    self.set_url(value)?;
                }
                "-d" | "--data" | "--data-ascii" | "--data-raw" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    if argument != "--data-raw" && value.starts_with('@') {
                        return Err(CurlImportError::FileInputUnsupported(argument.clone()));
                    }
                    self.data.push(value);
                    self.has_data = true;
                }
                "--data-binary" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    self.push_binary_data(value)?;
                }
                "--data-urlencode" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    self.push_urlencoded_data(&value)?;
                }
                "-F" | "--form" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    self.push_form(&value, false)?;
                }
                "--form-string" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    self.push_form(&value, true)?;
                }
                "-T" | "--upload-file" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    self.push_upload_file(value)?;
                }
                "-u" | "--user" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    let (username, password) = value.split_once(':').unwrap_or((&value, ""));
                    self.auth = RequestAuth::Basic {
                        username: username.to_owned(),
                        password: password.to_owned(),
                    };
                    self.warnings.push(
                        "The imported Basic authentication contains credentials; replace them with environment variables before saving."
                            .to_owned(),
                    );
                }
                "--oauth2-bearer" => {
                    let token = required_value(arguments, &mut index, argument)?;
                    self.auth = RequestAuth::Bearer { token };
                    self.warnings.push(
                        "The imported bearer token may be sensitive; replace it with an environment variable before saving."
                            .to_owned(),
                    );
                }
                "-b" | "--cookie" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    if value.starts_with('@') {
                        return Err(CurlImportError::FileInputUnsupported(argument.clone()));
                    }
                    self.headers.push(HeaderEntry {
                        name: "Cookie".to_owned(),
                        value,
                        enabled: true,
                    });
                    self.warnings.push(
                        "The imported Cookie header may contain session credentials; review it before saving."
                            .to_owned(),
                    );
                }
                "-A" | "--user-agent" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    self.upsert_header("User-Agent", value);
                }
                "-e" | "--referer" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    self.upsert_header("Referer", value);
                }
                "-I" | "--head" => self.head = true,
                "-L" | "--location" => self.options.follow_redirects = true,
                "--max-time" => {
                    let value = required_value(arguments, &mut index, argument)?;
                    let seconds = value
                        .parse::<f64>()
                        .map_err(|_| CurlImportError::InvalidOptionValue {
                            option: argument.clone(),
                            value: value.clone(),
                        })?;
                    if !seconds.is_finite() || seconds <= 0.0 {
                        return Err(CurlImportError::InvalidOptionValue {
                            option: argument.clone(),
                            value,
                        });
                    }
                    let milliseconds = seconds * 1000.0;
                    if milliseconds > u64::MAX as f64 {
                        return Err(CurlImportError::InvalidOptionValue {
                            option: argument.clone(),
                            value,
                        });
                    }
                    self.options.timeout_ms = milliseconds.ceil() as u64;
                }
                "-k" | "--insecure" => self.warnings.push(
                    "cURL TLS verification bypass was ignored; certificate validation remains enabled."
                        .to_owned(),
                ),
                "--compressed" | "-s" | "--silent" | "-S" | "--show-error" | "-v"
                | "--verbose" | "--globoff" => {}
                "-o" | "--output" | "-D" | "--dump-header" => {
                    let _ = required_value(arguments, &mut index, argument)?;
                    self.warnings
                        .push(format!("Ignored cURL output option {argument}."));
                }
                "-G" | "--get" => {
                    return Err(CurlImportError::UnsupportedOption(argument.clone()));
                }
                "--" => {
                    index += 1;
                    for value in &arguments[index..] {
                        self.set_url(value.clone())?;
                    }
                    break;
                }
                _ if argument.starts_with("-X") && argument.len() > 2 => {
                    self.explicit_method = Some(argument[2..].to_owned());
                }
                _ if argument.starts_with("-H") && argument.len() > 2 => {
                    self.push_header(&argument[2..])?;
                }
                _ if argument.starts_with("-d") && argument.len() > 2 => {
                    let value = argument[2..].to_owned();
                    if value.starts_with('@') {
                        return Err(CurlImportError::FileInputUnsupported("-d".to_owned()));
                    }
                    self.data.push(value);
                    self.has_data = true;
                }
                _ if argument.starts_with("-F") && argument.len() > 2 => {
                    self.push_form(&argument[2..], false)?;
                }
                _ if argument.starts_with("-T") && argument.len() > 2 => {
                    self.push_upload_file(argument[2..].to_owned())?;
                }
                _ if let Some(value) = argument.strip_prefix("--data-urlencode=") => {
                    self.push_urlencoded_data(value)?;
                }
                _ if let Some(value) = argument.strip_prefix("--data-binary=") => {
                    self.push_binary_data(value.to_owned())?;
                }
                _ if let Some(value) = argument.strip_prefix("--form=") => {
                    self.push_form(value, false)?;
                }
                _ if let Some(value) = argument.strip_prefix("--form-string=") => {
                    self.push_form(value, true)?;
                }
                _ if let Some(value) = argument.strip_prefix("--upload-file=") => {
                    self.push_upload_file(value.to_owned())?;
                }
                _ if argument.starts_with('-') => {
                    return Err(CurlImportError::UnsupportedOption(argument.clone()));
                }
                _ => self.set_url(argument.clone())?,
            }
            index += 1;
        }

        Ok(())
    }

    fn finish(mut self) -> Result<CurlImport, CurlImportError> {
        let mut url = self.url.ok_or(CurlImportError::MissingUrl)?;
        if !url.contains("://") {
            url = format!("http://{url}");
            self.warnings
                .push("The URL had no scheme, so http:// was added.".to_owned());
        }

        let parsed =
            Url::parse(&url).map_err(|error| CurlImportError::InvalidUrl(error.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(CurlImportError::UnsupportedProtocol(
                parsed.scheme().to_owned(),
            ));
        }

        let body_mode_count = usize::from(!self.data.is_empty())
            + usize::from(!self.multipart_parts.is_empty())
            + usize::from(self.binary_file.is_some());
        if body_mode_count > 1 {
            return Err(CurlImportError::IncompatibleBodyOptions);
        }

        let default_upload_method = self.upload_file;
        let method = self
            .explicit_method
            .map(|method| method.to_uppercase())
            .unwrap_or_else(|| {
                if self.head {
                    "HEAD".to_owned()
                } else if default_upload_method {
                    "PUT".to_owned()
                } else if self.has_data {
                    "POST".to_owned()
                } else {
                    "GET".to_owned()
                }
            });
        let body = if !self.multipart_parts.is_empty() {
            RequestBody::Multipart {
                parts: self.multipart_parts,
            }
        } else if let Some(path) = self.binary_file {
            RequestBody::Binary {
                path,
                content_type: None,
            }
        } else if self.has_data {
            let content_type = self
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case("content-type"))
                .map(|header| header.value.clone())
                .or_else(|| Some("application/x-www-form-urlencoded".to_owned()));
            RequestBody::Text {
                content: self.data.join("&"),
                content_type,
            }
        } else {
            RequestBody::Empty
        };

        let request = RequestDefinition {
            method,
            url,
            query_params: Vec::new(),
            headers: self.headers,
            auth: self.auth,
            body,
            options: self.options,
        };
        if let RequestBody::Text {
            content,
            content_type: Some(content_type),
        } = &request.body
            && content_type
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
            && serde_json::from_str::<serde_json::Value>(content)
                .is_ok_and(|value| value.is_object() || value.is_array())
        {
            self.warnings.push(
                "The body is JSON but Content-Type is application/x-www-form-urlencoded. The imported request was preserved; choose application/json in Body if this endpoint expects JSON.".to_owned()
            );
        }
        request
            .validate()
            .map_err(|error| CurlImportError::InvalidRequest(error.to_string()))?;

        Ok(CurlImport {
            request,
            warnings: self.warnings,
        })
    }

    fn set_url(&mut self, value: String) -> Result<(), CurlImportError> {
        if self.url.is_some() {
            return Err(CurlImportError::MultipleUrlsUnsupported);
        }
        let (value, removed_markdown_link) = normalize_url(value);
        if removed_markdown_link {
            self.warnings
                .push("Markdown link formatting around the cURL URL was removed.".to_owned());
        }
        self.url = Some(value);
        Ok(())
    }

    fn push_header(&mut self, value: &str) -> Result<(), CurlImportError> {
        if value.starts_with('@') {
            return Err(CurlImportError::FileInputUnsupported("--header".to_owned()));
        }
        let (name, header_value) = if let Some((name, header_value)) = value.split_once(':') {
            (name, header_value)
        } else if let Some(name) = value.strip_suffix(';') {
            // cURL uses a trailing semicolon to force a header with an empty value.
            (name, "")
        } else {
            return Err(CurlImportError::InvalidHeader(value.to_owned()));
        };
        if name.trim().is_empty() {
            return Err(CurlImportError::InvalidHeader(value.to_owned()));
        }
        self.headers.push(HeaderEntry {
            name: name.trim().to_owned(),
            value: header_value.trim_start().to_owned(),
            enabled: true,
        });
        Ok(())
    }

    fn push_urlencoded_data(&mut self, value: &str) -> Result<(), CurlImportError> {
        let value = unescape_markdown(value);
        if value.starts_with('@')
            || value
                .split_once('@')
                .is_some_and(|(name, _)| !name.contains('='))
        {
            return Err(CurlImportError::FileInputUnsupported(
                "--data-urlencode".to_owned(),
            ));
        }

        let encoded = if let Some(content) = value.strip_prefix('=') {
            percent_encode(content)
        } else if let Some((name, content)) = value.split_once('=') {
            format!("{name}={}", percent_encode(content))
        } else {
            percent_encode(&value)
        };
        self.data.push(encoded);
        self.has_data = true;
        Ok(())
    }

    fn push_binary_data(&mut self, value: String) -> Result<(), CurlImportError> {
        if let Some(path) = value.strip_prefix('@') {
            self.set_binary_file(path.to_owned(), false)
        } else {
            self.data.push(value);
            self.has_data = true;
            Ok(())
        }
    }

    fn push_upload_file(&mut self, path: String) -> Result<(), CurlImportError> {
        self.set_binary_file(path, true)
    }

    fn set_binary_file(
        &mut self,
        path: String,
        is_upload_file: bool,
    ) -> Result<(), CurlImportError> {
        if path.is_empty() {
            return Err(CurlImportError::InvalidFileReference);
        }
        if path == "-" {
            return Err(CurlImportError::StdinUnsupported);
        }
        if self.binary_file.is_some() {
            return Err(CurlImportError::MultipleFileBodiesUnsupported);
        }
        self.binary_file = Some(unescape_markdown(&path));
        self.upload_file = is_upload_file;
        self.has_data = true;
        self.add_file_reference_warning();
        Ok(())
    }

    fn push_form(&mut self, value: &str, force_string: bool) -> Result<(), CurlImportError> {
        let value = unescape_markdown(value);
        let (name, specification) = value.split_once('=').ok_or(CurlImportError::InvalidForm)?;
        if name.trim().is_empty() {
            return Err(CurlImportError::InvalidForm);
        }

        if force_string {
            self.multipart_parts.push(MultipartPart {
                name: name.to_owned(),
                enabled: true,
                value: MultipartValue::Text {
                    content: specification.to_owned(),
                    content_type: None,
                },
            });
            self.has_data = true;
            return Ok(());
        }

        let mut segments = specification.split(';');
        let mut payload = segments.next().unwrap_or_default().to_owned();
        let mut content_type = None;
        let mut file_name = None;
        for segment in segments {
            if let Some(value) = segment.strip_prefix("type=") {
                content_type = Some(value.to_owned());
            } else if let Some(value) = segment.strip_prefix("filename=") {
                file_name = Some(value.to_owned());
            } else {
                payload.push(';');
                payload.push_str(segment);
            }
        }

        let part_value = if let Some(path) = payload
            .strip_prefix('@')
            .or_else(|| payload.strip_prefix('<'))
        {
            if path.is_empty() || path == "-" {
                return Err(if path == "-" {
                    CurlImportError::StdinUnsupported
                } else {
                    CurlImportError::InvalidFileReference
                });
            }
            self.add_file_reference_warning();
            MultipartValue::File {
                path: path.to_owned(),
                file_name,
                content_type,
            }
        } else {
            MultipartValue::Text {
                content: payload,
                content_type,
            }
        };

        self.multipart_parts.push(MultipartPart {
            name: name.to_owned(),
            enabled: true,
            value: part_value,
        });
        self.has_data = true;
        Ok(())
    }

    fn add_file_reference_warning(&mut self) {
        const WARNING: &str = "Local file references were imported but not read. They are machine-specific and will only be accessed when this request is sent.";
        if !self.warnings.iter().any(|warning| warning == WARNING) {
            self.warnings.push(WARNING.to_owned());
        }
    }

    fn upsert_header(&mut self, name: &str, value: String) {
        if let Some(header) = self
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(name))
        {
            header.value = value;
            header.enabled = true;
        } else {
            self.headers.push(HeaderEntry {
                name: name.to_owned(),
                value,
                enabled: true,
            });
        }
    }
}

fn required_value(
    arguments: &[String],
    index: &mut usize,
    option: &str,
) -> Result<String, CurlImportError> {
    *index += 1;
    arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| CurlImportError::MissingOptionValue(option.to_owned()))
}

fn normalize_command(command: &str) -> String {
    let trimmed = command.trim().trim_start_matches("$ ").trim();
    trimmed
        .replace("^\r\n", "")
        .replace("^\n", "")
        .replace("`\r\n", "")
        .replace("`\n", "")
        .replace("^\"", "\"")
}

fn normalize_argument(value: String) -> String {
    if let Some(option) = value.strip_prefix("\\--") {
        format!("--{option}")
    } else {
        value
    }
}

fn normalize_url(value: String) -> (String, bool) {
    let value = unescape_markdown(&value);
    if let Some(markdown_body) = value.strip_prefix('[') {
        if let Some((_, target)) = markdown_body.split_once("](") {
            if let Some(target) = target.strip_suffix(')') {
                if !target.trim().is_empty() {
                    return (unescape_markdown(target.trim()), true);
                }
            }
        }
    }

    if let Some(value) = value
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
    {
        return (value.trim().to_owned(), true);
    }

    (value, false)
}

fn unescape_markdown(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\' {
            if let Some(next) = characters.peek().copied() {
                if next.is_ascii_punctuation() {
                    normalized.push(next);
                    characters.next();
                    continue;
                }
            }
        }
        normalized.push(character);
    }
    normalized
}

fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

fn is_curl_executable(value: &str) -> bool {
    let executable = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(value)
        .to_ascii_lowercase();
    matches!(executable.as_str(), "curl" | "curl.exe")
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CurlExportError {
    #[error("request cannot be exported: {0}")]
    InvalidRequest(String),
    #[error("cURL arguments cannot contain a NUL byte")]
    NulByte,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CurlImportError {
    #[error("paste a cURL command to import it")]
    EmptyCommand,
    #[error("the pasted text is not a cURL command")]
    NotCurlCommand,
    #[error("cURL syntax is invalid: {0}")]
    InvalidSyntax(String),
    #[error("cURL option {0} requires a value")]
    MissingOptionValue(String),
    #[error("cURL option {option} has an invalid value: {value}")]
    InvalidOptionValue { option: String, value: String },
    #[error("cURL option {0} is not supported by this importer yet")]
    UnsupportedOption(String),
    #[error("cURL file input for {0} is not imported for security; paste the content directly")]
    FileInputUnsupported(String),
    #[error("cURL form fields must use the form name=value or name=@file")]
    InvalidForm,
    #[error("cURL body options select incompatible body types")]
    IncompatibleBodyOptions,
    #[error("the cURL command contains an invalid local file reference")]
    InvalidFileReference,
    #[error("reading request bodies from standard input is not supported")]
    StdinUnsupported,
    #[error("importing multiple standalone upload files is not supported")]
    MultipleFileBodiesUnsupported,
    #[error("cURL header is invalid: {0}")]
    InvalidHeader(String),
    #[error("the cURL command does not contain a URL")]
    MissingUrl,
    #[error("importing multiple URLs in one cURL command is not supported")]
    MultipleUrlsUnsupported,
    #[error("cURL URL is invalid: {0}")]
    InvalidUrl(String),
    #[error("cURL protocol {0} is not supported; use HTTP or HTTPS")]
    UnsupportedProtocol(String),
    #[error("imported request is invalid: {0}")]
    InvalidRequest(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_a_browser_style_json_request() {
        let imported = import_curl(
            r#"curl 'https://api.example.com/users?active=true' \
              -X POST \
              -H 'Accept: application/json' \
              -H 'Content-Type: application/json' \
              --data-raw '{"name":"Ada"}' \
              --compressed"#,
        )
        .unwrap();

        assert_eq!(imported.request.method, "POST");
        assert_eq!(
            imported.request.url,
            "https://api.example.com/users?active=true"
        );
        assert_eq!(imported.request.headers.len(), 2);
        assert!(matches!(
            imported.request.body,
            RequestBody::Text { ref content, .. } if content == "{\"name\":\"Ada\"}"
        ));
    }

    #[test]
    fn warns_about_raw_json_with_a_form_content_type_without_rewriting_it() {
        let command = r#"curl.exe `
          --location `
          --max-time `
          30 `
          --request `
          POST `
          --header `
          'Authorization: Bearer {{access_token}}' `
          --header `
          'Content-Type: application/x-www-form-urlencoded' `
          --data-raw `
          '{
            "filters": [{"field":"id","operator":"eq","value":"example-device"}]
          }
          ' `
          https://api.example.test/v1/events/search"#;
        let imported = import_curl(command).unwrap();
        assert_eq!(imported.request.method, "POST");
        assert_eq!(
            imported.request.url,
            "https://api.example.test/v1/events/search"
        );
        assert_eq!(imported.request.options.timeout_ms, 30_000);
        assert!(imported.request.options.follow_redirects);
        assert!(
            imported
                .request
                .headers
                .iter()
                .any(|header| header.name == "Authorization"
                    && header.value == "Bearer {{access_token}}")
        );
        assert!(matches!(&imported.request.body,
            RequestBody::Text { content, content_type } if content.contains("example-device")
                && content_type.as_deref() == Some("application/x-www-form-urlencoded")
        ));
        assert!(
            imported
                .warnings
                .iter()
                .any(|warning| warning.contains("body is JSON"))
        );
        let corrected =
            import_curl(&command.replace("application/x-www-form-urlencoded", "application/json"))
                .unwrap();
        assert!(
            !corrected
                .warnings
                .iter()
                .any(|warning| warning.contains("body is JSON"))
        );
        assert_eq!(
            corrected.request.body,
            match imported.request.body {
                RequestBody::Text { content, .. } => RequestBody::Text {
                    content,
                    content_type: Some("application/json".to_owned())
                },
                _ => unreachable!(),
            }
        );
    }

    #[test]
    fn infers_post_for_data_and_default_content_type() {
        let imported = import_curl("curl https://example.com -d 'a=1' -d 'b=2'").unwrap();
        assert_eq!(imported.request.method, "POST");
        assert!(matches!(
            imported.request.body,
            RequestBody::Text { ref content, ref content_type }
                if content == "a=1&b=2"
                    && content_type.as_deref() == Some("application/x-www-form-urlencoded")
        ));
    }

    #[test]
    fn imports_basic_auth_with_a_security_warning() {
        let imported = import_curl("curl -u user:secret https://example.com").unwrap();
        assert_eq!(
            imported.request.auth,
            RequestAuth::Basic {
                username: "user".to_owned(),
                password: "secret".to_owned(),
            }
        );
        assert_eq!(imported.warnings.len(), 1);
    }

    #[test]
    fn adds_http_when_curl_would_infer_a_scheme() {
        let imported = import_curl("curl example.com/health").unwrap();
        assert_eq!(imported.request.url, "http://example.com/health");
        assert_eq!(imported.warnings.len(), 1);
    }

    #[test]
    fn refuses_implicit_data_file_reads_but_imports_multipart_file_references() {
        assert!(matches!(
            import_curl("curl https://example.com --data @secret.txt"),
            Err(CurlImportError::FileInputUnsupported(_))
        ));
        let imported = import_curl(
            "curl https://example.com -F 'description=report' -F 'file=@secret.txt;type=text/plain;filename=report.txt'",
        )
        .unwrap();
        assert_eq!(imported.request.method, "POST");
        assert!(matches!(
            imported.request.body,
            RequestBody::Multipart { ref parts }
                if parts.len() == 2
                    && matches!(
                        &parts[1].value,
                        MultipartValue::File { path, file_name, content_type }
                            if path == "secret.txt"
                                && file_name.as_deref() == Some("report.txt")
                                && content_type.as_deref() == Some("text/plain")
                    )
        ));
        assert_eq!(imported.warnings.len(), 1);
    }

    #[test]
    fn imports_binary_file_and_upload_file_references() {
        let imported =
            import_curl("curl https://example.com/upload --data-binary @payload.bin").unwrap();
        assert_eq!(imported.request.method, "POST");
        assert!(matches!(
            imported.request.body,
            RequestBody::Binary { ref path, .. } if path == "payload.bin"
        ));

        let imported = import_curl("curl -T payload.bin https://example.com/upload").unwrap();
        assert_eq!(imported.request.method, "PUT");
        assert!(matches!(
            imported.request.body,
            RequestBody::Binary { ref path, .. } if path == "payload.bin"
        ));
    }

    #[test]
    fn rejects_incompatible_curl_body_modes() {
        assert_eq!(
            import_curl("curl https://example.com -d a=1 -F b=2").unwrap_err(),
            CurlImportError::IncompatibleBodyOptions
        );
    }

    #[test]
    fn rejects_non_curl_text() {
        assert_eq!(
            import_curl("https://example.com").unwrap_err(),
            CurlImportError::NotCurlCommand
        );
    }

    #[test]
    fn imports_markdown_escaped_oauth_command() {
        let imported = import_curl(
            r#"curl --location '[https://auth.example.test/oauth2/v1/token](https://auth.example.test/oauth2/v1/token)' \
\--header 'Content-Type: application/x-www-form-urlencoded' \
\--header 'Authorization;' \
\--data-urlencode 'grant\_type=client\_credentials'"#,
        )
        .unwrap();

        assert_eq!(imported.request.method, "POST");
        assert_eq!(
            imported.request.url,
            "https://auth.example.test/oauth2/v1/token"
        );
        assert!(imported.request.headers.iter().any(|header| {
            header.name == "Authorization" && header.value.is_empty() && header.enabled
        }));
        assert!(matches!(
            imported.request.body,
            RequestBody::Text { ref content, ref content_type }
                if content == "grant_type=client_credentials"
                    && content_type.as_deref()
                        == Some("application/x-www-form-urlencoded")
        ));
        assert_eq!(imported.warnings.len(), 1);
    }

    #[test]
    fn urlencodes_data_values_and_refuses_file_inputs() {
        let imported =
            import_curl("curl https://example.com --data-urlencode 'scope=read write&admin'")
                .unwrap();
        assert!(matches!(
            imported.request.body,
            RequestBody::Text { ref content, .. }
                if content == "scope=read%20write%26admin"
        ));

        assert!(matches!(
            import_curl("curl https://example.com --data-urlencode token@secret.txt"),
            Err(CurlImportError::FileInputUnsupported(_))
        ));
    }

    #[test]
    fn exports_a_shell_safe_request_and_round_trips_common_fields() {
        let request = RequestDefinition {
            method: "POST".to_owned(),
            url: "https://example.com/token?existing=1".to_owned(),
            query_params: vec![api_client_core::KeyValueEntry {
                name: "scope".to_owned(),
                value: "read write".to_owned(),
                enabled: true,
            }],
            headers: vec![HeaderEntry {
                name: "X-Label".to_owned(),
                value: "Ada's client".to_owned(),
                enabled: true,
            }],
            auth: RequestAuth::Basic {
                username: "local user".to_owned(),
                password: "p@ss'word".to_owned(),
            },
            body: RequestBody::FormUrlEncoded {
                fields: vec![api_client_core::KeyValueEntry {
                    name: "grant_type".to_owned(),
                    value: "client_credentials".to_owned(),
                    enabled: true,
                }],
            },
            options: RequestOptions {
                timeout_ms: 1_250,
                ..RequestOptions::default()
            },
        };

        let command = export_curl(&request, CurlShell::Posix).unwrap();
        assert!(command.starts_with("curl \\\n"));
        assert!(command.contains("--max-time \\\n  1.25"));
        assert!(command.contains("'X-Label: Ada'\"'\"'s client'"));
        assert!(command.contains("scope=read+write"));

        let imported = import_curl(&command).unwrap();
        assert_eq!(imported.request.method, "POST");
        assert_eq!(imported.request.options.timeout_ms, 1_250);
        assert_eq!(
            imported.request.auth,
            RequestAuth::Basic {
                username: "local user".to_owned(),
                password: "p@ss'word".to_owned(),
            }
        );
        assert!(imported.request.url.contains("scope=read+write"));
        assert!(matches!(
            imported.request.body,
            RequestBody::Text { ref content, .. } if content == "grant_type=client_credentials"
        ));
    }

    fn code_export_request() -> RequestDefinition {
        RequestDefinition {
            method: "POST".to_owned(),
            url: "https://example.com/token?existing=1".to_owned(),
            query_params: vec![api_client_core::KeyValueEntry {
                name: "scope".to_owned(),
                value: "read write".to_owned(),
                enabled: true,
            }],
            headers: vec![HeaderEntry {
                name: "X-Label".to_owned(),
                value: "Ada's client".to_owned(),
                enabled: true,
            }],
            auth: RequestAuth::Basic {
                username: "local user".to_owned(),
                password: "p@ss'word".to_owned(),
            },
            body: RequestBody::FormUrlEncoded {
                fields: vec![api_client_core::KeyValueEntry {
                    name: "grant_type".to_owned(),
                    value: "client_credentials".to_owned(),
                    enabled: true,
                }],
            },
            options: RequestOptions::default(),
        }
    }

    #[test]
    fn exports_javascript_fetch_with_basic_auth_and_form_body() {
        let code = export_code(&code_export_request(), CodeLanguage::JavaScriptFetch).unwrap();
        assert!(code.contains(
            "const basicAuth = \"Basic \" + btoa(\"local user\" + \":\" + \"p@ss'word\");"
        ));
        assert!(
            code.contains("await fetch(\"https://example.com/token?existing=1&scope=read+write\"")
        );
        assert!(code.contains("Authorization: basicAuth,"));
        assert!(code.contains(
            "body: new URLSearchParams([[\"grant_type\", \"client_credentials\"]]).toString(),"
        ));
    }

    #[test]
    fn exports_node_axios_with_basic_auth_object() {
        let code = export_code(&code_export_request(), CodeLanguage::NodeAxios).unwrap();
        assert!(code.contains("const axios = require(\"axios\");"));
        assert!(code.contains("auth: {"));
        assert!(code.contains("username: \"local user\","));
        assert!(code.contains("password: \"p@ss'word\","));
    }

    #[test]
    fn exports_python_requests_with_auth_tuple_and_data_dict() {
        let code = export_code(&code_export_request(), CodeLanguage::PythonRequests).unwrap();
        assert!(code.contains("import requests"));
        assert!(code.contains("auth=(\"local user\", \"p@ss'word\")"));
        assert!(code.contains("data={\"grant_type\": \"client_credentials\"}"));
    }

    #[test]
    fn exports_bearer_tokens_trimmed_in_curl_and_code() {
        let mut request = code_export_request();
        request.auth = RequestAuth::Bearer {
            token: "  eyJhbGciOiJIUzI1NiJ9.payload.sig \n".to_owned(),
        };

        let command = export_curl(&request, CurlShell::Posix).unwrap();
        assert!(command.contains("'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig'"));

        let code = export_code(&request, CodeLanguage::PythonRequests).unwrap();
        assert!(code.contains("\"Authorization\": \"Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig\","));
    }

    #[test]
    fn code_export_notes_unsupported_multipart_bodies() {
        let mut request = code_export_request();
        request.body = RequestBody::Multipart { parts: Vec::new() };
        let code = export_code(&request, CodeLanguage::PythonRequests).unwrap();
        assert!(code.contains("multipart form bodies are not yet supported"));
    }

    #[test]
    fn exports_multipart_and_binary_file_references_without_reading_them() {
        let mut request = RequestDefinition {
            method: "POST".to_owned(),
            url: "https://example.com/upload".to_owned(),
            query_params: Vec::new(),
            headers: Vec::new(),
            auth: RequestAuth::None,
            body: RequestBody::Multipart {
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
                            path: "C:\\Reports\\report.pdf".to_owned(),
                            file_name: Some("sent.pdf".to_owned()),
                            content_type: Some("application/pdf".to_owned()),
                        },
                    },
                ],
            },
            options: RequestOptions::default(),
        };

        let multipart = export_curl(&request, CurlShell::PowerShell).unwrap();
        assert!(multipart.starts_with("curl.exe `\r\n"));
        assert!(multipart.contains("--form-string"));
        assert!(multipart.contains("document=@C:\\Reports\\report.pdf"));

        request.method = "PUT".to_owned();
        request.body = RequestBody::Binary {
            path: "C:\\Files\\payload.bin".to_owned(),
            content_type: Some("application/octet-stream".to_owned()),
        };
        let binary = export_curl(&request, CurlShell::PowerShell).unwrap();
        assert!(binary.contains("--data-binary"));
        assert!(binary.contains("'@C:\\Files\\payload.bin'"));
        assert!(binary.contains("'Content-Type: application/octet-stream'"));
    }

    #[test]
    fn uses_native_single_quote_escaping_for_each_shell() {
        assert_eq!(
            quote_shell_argument("it's $5", CurlShell::Posix).unwrap(),
            "'it'\"'\"'s $5'"
        );
        assert_eq!(
            quote_shell_argument("it's $5", CurlShell::PowerShell).unwrap(),
            "'it''s $5'"
        );
        assert_eq!(
            quote_shell_argument("", CurlShell::PowerShell).unwrap(),
            "''"
        );
    }
}
