use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use api_client_core::{
    ApiKeyPlacement, HeaderEntry, KeyValueEntry, MultipartPart, MultipartValue, OAuth2ClientAuth,
    RequestAuth, RequestBody, RequestDefinition, RequestOptions,
};
use api_client_workspace::{
    Collection, Environment, EnvironmentVariable, RequestFolder, SavedRequest,
};
use serde_json::{Map, Value};

use crate::{ApiDocumentImport, ApiDocumentImportError, ApiDocumentKind};

const MAX_OPERATIONS: usize = 10_000;
const MAX_REFERENCE_DEPTH: usize = 32;
const MAX_SCHEMA_DEPTH: usize = 24;
const MAX_WARNINGS: usize = 512;
const HTTP_METHODS: [&str; 8] = [
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

pub(crate) fn import_openapi(
    document: &Value,
    file_name: &str,
) -> Result<ApiDocumentImport, ApiDocumentImportError> {
    let is_swagger = match (
        document.get("openapi").and_then(Value::as_str),
        document.get("swagger").and_then(Value::as_str),
    ) {
        (Some(version), _) if version.starts_with("3.") => false,
        (_, Some("2.0")) => true,
        (Some(version), _) => {
            return Err(ApiDocumentImportError::Invalid(format!(
                "OpenAPI version '{version}' is not supported"
            )));
        }
        (_, Some(version)) => {
            return Err(ApiDocumentImportError::Invalid(format!(
                "Swagger version '{version}' is not supported"
            )));
        }
        _ => {
            return Err(ApiDocumentImportError::Invalid(
                "OpenAPI or Swagger version is missing".to_owned(),
            ));
        }
    };

    let fallback_name = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Imported API");
    let name = clean_name(
        document
            .pointer("/info/title")
            .and_then(Value::as_str)
            .unwrap_or(fallback_name),
    );
    let mut context = ImportContext::new(document);
    context.parse_security_schemes(is_swagger);
    let base_url_variable = context.import_base_url(is_swagger);

    let mut collection = Collection::new(&name);
    collection.auth = context.security_auth(document.get("security"), false, "API");

    if document
        .get("webhooks")
        .and_then(Value::as_object)
        .is_some_and(|webhooks| !webhooks.is_empty())
    {
        context.warn("OpenAPI webhooks are not represented and were omitted.");
    }

    let Some(paths) = document.get("paths").and_then(Value::as_object) else {
        return Err(ApiDocumentImportError::Invalid(
            "OpenAPI document does not contain a paths object".to_owned(),
        ));
    };

    let mut operation_count = 0usize;
    for (raw_path, raw_path_item) in paths {
        let path_item = context.resolve_value(raw_path_item, &format!("path '{raw_path}'"));
        let Some(path_object) = path_item.as_object() else {
            context.warn(format!(
                "Skipped path '{raw_path}' because it is not an object."
            ));
            continue;
        };
        let path_parameters = path_object
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for method in HTTP_METHODS {
            let Some(raw_operation) = path_object.get(method) else {
                continue;
            };
            operation_count += 1;
            if operation_count > MAX_OPERATIONS {
                return Err(ApiDocumentImportError::Invalid(format!(
                    "OpenAPI document contains more than {MAX_OPERATIONS} operations"
                )));
            }
            let operation = context.resolve_value(
                raw_operation,
                &format!("{} {raw_path}", method.to_ascii_uppercase()),
            );
            let Some(operation_object) = operation.as_object() else {
                context.warn(format!(
                    "Skipped {} {raw_path} because the operation is not an object.",
                    method.to_ascii_uppercase()
                ));
                continue;
            };
            let saved = context.import_operation(
                method,
                raw_path,
                path_object,
                operation_object,
                &path_parameters,
                &base_url_variable,
                is_swagger,
            );
            let tag = operation_object
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|tags| tags.first())
                .and_then(Value::as_str)
                .map(clean_name)
                .unwrap_or_else(|| "Untagged".to_owned());
            if let Some(folder) = collection
                .folders
                .iter_mut()
                .find(|folder| folder.name == tag)
            {
                folder.requests.push(saved);
            } else {
                let mut folder = RequestFolder::new(tag);
                folder.requests.push(saved);
                collection.folders.push(folder);
            }
        }
    }

    if operation_count == 0 {
        context.warn("The OpenAPI document contained no importable HTTP operations.");
    }

    let environments = if context.variables.is_empty() {
        Vec::new()
    } else {
        let mut environment = Environment::new(format!("{name} environment"));
        environment.variables = context.variables;
        vec![environment]
    };

    Ok(ApiDocumentImport {
        kind: ApiDocumentKind::OpenApi,
        name,
        collections: vec![collection],
        environments,
        warnings: context.warnings,
    })
}

struct ImportContext<'a> {
    root: &'a Value,
    variables: Vec<EnvironmentVariable>,
    security_schemes: HashMap<String, RequestAuth>,
    warnings: Vec<String>,
    warning_set: HashSet<String>,
}

impl<'a> ImportContext<'a> {
    fn new(root: &'a Value) -> Self {
        Self {
            root,
            variables: Vec::new(),
            security_schemes: HashMap::new(),
            warnings: Vec::new(),
            warning_set: HashSet::new(),
        }
    }

    fn warn(&mut self, warning: impl Into<String>) {
        let warning = warning.into();
        if self.warnings.len() < MAX_WARNINGS && self.warning_set.insert(warning.clone()) {
            self.warnings.push(warning);
        }
    }

    fn resolve_value(&mut self, value: &Value, context: &str) -> Value {
        let mut resolved = value.clone();
        let mut visited = HashSet::new();
        for _ in 0..MAX_REFERENCE_DEPTH {
            let Some(reference) = resolved.get("$ref").and_then(Value::as_str) else {
                return resolved;
            };
            if !reference.starts_with("#/") {
                self.warn(format!(
                    "External reference '{reference}' in {context} was not resolved."
                ));
                return resolved;
            }
            if !visited.insert(reference.to_owned()) {
                self.warn(format!(
                    "Reference cycle in {context} was stopped at '{reference}'."
                ));
                return resolved;
            }
            let pointer = &reference[1..];
            let Some(next) = self.root.pointer(pointer) else {
                self.warn(format!(
                    "Reference '{reference}' in {context} does not exist."
                ));
                return resolved;
            };
            resolved = next.clone();
        }
        self.warn(format!(
            "Reference chain in {context} exceeds {MAX_REFERENCE_DEPTH} levels."
        ));
        resolved
    }

    fn ensure_variable(&mut self, preferred: &str, value: String, secret: bool) -> String {
        let base = variable_name(preferred);
        if let Some(existing) = self.variables.iter().find(|variable| {
            variable.key == base
                && variable.secret == secret
                && (variable.value == value || value.is_empty())
        }) {
            return existing.key.clone();
        }
        let mut key = base.clone();
        let mut suffix = 2usize;
        while self.variables.iter().any(|variable| variable.key == key) {
            key = format!("{base}{suffix}");
            suffix += 1;
        }
        let mut variable = EnvironmentVariable::new(&key, if secret { "" } else { &value });
        variable.secret = secret;
        self.variables.push(variable);
        key
    }

    fn import_base_url(&mut self, is_swagger: bool) -> String {
        let mut value = if is_swagger {
            let scheme = self
                .root
                .get("schemes")
                .and_then(Value::as_array)
                .and_then(|schemes| schemes.first())
                .and_then(Value::as_str)
                .unwrap_or("https");
            let host = self
                .root
                .get("host")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let base_path = self
                .root
                .get("basePath")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if host.is_empty() {
                self.warn(
                    "Swagger host is missing; set the imported baseUrl environment variable before sending requests.",
                );
                String::new()
            } else {
                format!("{scheme}://{host}{base_path}")
            }
        } else {
            self.root
                .get("servers")
                .and_then(Value::as_array)
                .and_then(|servers| servers.first())
                .and_then(|server| server.get("url"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };

        if !is_swagger {
            let server = self
                .root
                .get("servers")
                .and_then(Value::as_array)
                .and_then(|servers| servers.first())
                .cloned();
            if let Some(server_variables) = server
                .as_ref()
                .and_then(|server| server.get("variables"))
                .and_then(Value::as_object)
            {
                for (name, definition) in server_variables {
                    let default = definition
                        .get("default")
                        .map(value_to_string)
                        .unwrap_or_default();
                    let imported_name = self.ensure_variable(name, default, false);
                    value =
                        value.replace(&format!("{{{name}}}"), &format!("{{{{{imported_name}}}}}"));
                }
            }
            if value.is_empty() {
                self.warn(
                    "OpenAPI servers are missing; set the imported baseUrl environment variable before sending requests.",
                );
            } else if !value.contains("://") && !value.starts_with("{{") {
                self.warn(
                    "The first OpenAPI server URL is relative; update baseUrl to an absolute URL before sending requests.",
                );
            }
        }
        while value.ends_with('/') {
            value.pop();
        }
        self.ensure_variable("baseUrl", value, false)
    }

    fn parse_security_schemes(&mut self, is_swagger: bool) {
        let schemes = if is_swagger {
            self.root.get("securityDefinitions")
        } else {
            self.root.pointer("/components/securitySchemes")
        }
        .and_then(Value::as_object)
        .map(|schemes| {
            schemes
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

        for (name, raw_scheme) in schemes {
            let scheme = self.resolve_value(&raw_scheme, &format!("security scheme '{name}'"));
            if let Some(auth) = self.import_security_scheme(&name, &scheme, is_swagger) {
                self.security_schemes.insert(name, auth);
            }
        }
    }

    fn import_security_scheme(
        &mut self,
        name: &str,
        scheme: &Value,
        is_swagger: bool,
    ) -> Option<RequestAuth> {
        let scheme_type = scheme
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match scheme_type {
            "apiKey" => {
                let key = scheme
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("X-API-Key")
                    .to_owned();
                let variable = self.ensure_variable(&format!("{name}ApiKey"), String::new(), true);
                Some(RequestAuth::ApiKey {
                    key,
                    value: format!("{{{{{variable}}}}}"),
                    placement: if scheme.get("in").and_then(Value::as_str) == Some("query") {
                        ApiKeyPlacement::Query
                    } else {
                        ApiKeyPlacement::Header
                    },
                })
            }
            "basic" if is_swagger => Some(self.basic_auth(name)),
            "http" => match scheme
                .get("scheme")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "basic" => Some(self.basic_auth(name)),
                "bearer" => Some(self.bearer_auth(name)),
                unsupported => {
                    self.warn(format!(
                        "HTTP auth scheme '{unsupported}' in security scheme '{name}' is not supported."
                    ));
                    None
                }
            },
            "oauth2" => self.oauth2_auth(name, scheme, is_swagger),
            "openIdConnect" => {
                self.warn(format!(
                    "OpenID Connect security scheme '{name}' needs an explicit client flow and was not mapped automatically."
                ));
                None
            }
            "mutualTLS" => {
                self.warn(format!(
                    "Mutual TLS scheme '{name}' must be configured in workspace Network settings."
                ));
                None
            }
            unsupported => {
                self.warn(format!(
                    "Security scheme '{name}' has unsupported type '{unsupported}'."
                ));
                None
            }
        }
    }

    fn basic_auth(&mut self, name: &str) -> RequestAuth {
        let username = self.ensure_variable(&format!("{name}Username"), String::new(), false);
        let password = self.ensure_variable(&format!("{name}Password"), String::new(), true);
        RequestAuth::Basic {
            username: format!("{{{{{username}}}}}"),
            password: format!("{{{{{password}}}}}"),
        }
    }

    fn bearer_auth(&mut self, name: &str) -> RequestAuth {
        let token = self.ensure_variable(&format!("{name}Token"), String::new(), true);
        RequestAuth::Bearer {
            token: format!("{{{{{token}}}}}"),
        }
    }

    fn oauth2_auth(&mut self, name: &str, scheme: &Value, is_swagger: bool) -> Option<RequestAuth> {
        let flow = if is_swagger {
            (scheme.get("flow").and_then(Value::as_str) == Some("application")).then_some(scheme)
        } else {
            scheme.pointer("/flows/clientCredentials")
        };
        let Some(flow) = flow else {
            self.warn(format!(
                "OAuth 2 scheme '{name}' does not define a client-credentials flow and was not mapped."
            ));
            return None;
        };
        let token_url = flow
            .get("tokenUrl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if token_url.is_empty() {
            self.warn(format!(
                "OAuth 2 client-credentials scheme '{name}' has no token URL."
            ));
        }
        let client_id = self.ensure_variable(&format!("{name}ClientId"), String::new(), false);
        let client_secret =
            self.ensure_variable(&format!("{name}ClientSecret"), String::new(), true);
        Some(RequestAuth::OAuth2ClientCredentials {
            issuer_url: String::new(),
            token_url,
            client_id: format!("{{{{{client_id}}}}}"),
            client_secret: format!("{{{{{client_secret}}}}}"),
            scope: String::new(),
            audience: String::new(),
            client_auth: OAuth2ClientAuth::Basic,
            extra_parameters: Vec::new(),
        })
    }

    fn security_auth(
        &mut self,
        security: Option<&Value>,
        inherit_when_missing: bool,
        context: &str,
    ) -> Option<RequestAuth> {
        let Some(security) = security else {
            return inherit_when_missing.then_some(RequestAuth::Inherit);
        };
        let Some(requirements) = security.as_array() else {
            self.warn(format!(
                "Security requirements for {context} are not an array."
            ));
            return Some(RequestAuth::None);
        };
        if requirements.is_empty() {
            return Some(RequestAuth::None);
        }
        for requirement in requirements {
            let Some(required_schemes) = requirement.as_object() else {
                continue;
            };
            if required_schemes.is_empty() {
                return Some(RequestAuth::None);
            }
            if required_schemes.len() > 1 {
                self.warn(format!(
                    "{context} requires multiple auth schemes simultaneously; only the first supported scheme was imported."
                ));
            }
            for (scheme_name, scopes) in required_schemes {
                if let Some(mut auth) = self.security_schemes.get(scheme_name).cloned() {
                    if let RequestAuth::OAuth2ClientCredentials { scope, .. } = &mut auth {
                        *scope = scopes
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" ");
                    }
                    return Some(auth);
                }
            }
        }
        self.warn(format!(
            "No supported security scheme could be selected for {context}; it was set to No Auth."
        ));
        Some(RequestAuth::None)
    }

    #[allow(clippy::too_many_arguments)]
    fn import_operation(
        &mut self,
        method: &str,
        raw_path: &str,
        path_item: &Map<String, Value>,
        operation: &Map<String, Value>,
        path_parameters: &[Value],
        base_url_variable: &str,
        is_swagger: bool,
    ) -> SavedRequest {
        let operation_label = format!("{} {raw_path}", method.to_ascii_uppercase());
        let parameters = self.merge_parameters(path_parameters, operation.get("parameters"));
        let mut query_params = Vec::new();
        let mut headers = Vec::new();
        let mut path_variables = HashMap::new();
        for parameter in &parameters {
            let Some(parameter_object) = parameter.as_object() else {
                self.warn(format!(
                    "Ignored a malformed parameter on {operation_label}."
                ));
                continue;
            };
            let Some(name) = parameter_object.get("name").and_then(Value::as_str) else {
                self.warn(format!(
                    "Ignored an unnamed parameter on {operation_label}."
                ));
                continue;
            };
            let location = parameter_object
                .get("in")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let required = parameter_object
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let deprecated = parameter_object
                .get("deprecated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let value = self.parameter_example(parameter_object, &operation_label);
            match location {
                "path" => {
                    let imported_name = self.ensure_variable(name, value, false);
                    path_variables.insert(name.to_owned(), imported_name);
                }
                "query" => query_params.push(KeyValueEntry {
                    name: name.to_owned(),
                    value,
                    enabled: required && !deprecated,
                }),
                "header" => headers.push(HeaderEntry {
                    name: name.to_owned(),
                    value,
                    enabled: required && !deprecated,
                }),
                "cookie" => self.warn(format!(
                    "Cookie parameter '{name}' on {operation_label} must be configured in Cookie Manager."
                )),
                "body" | "formData" => {}
                unsupported if !unsupported.is_empty() => self.warn(format!(
                    "Parameter '{name}' on {operation_label} has unsupported location '{unsupported}'."
                )),
                _ => {}
            }
        }

        let rendered_path = self.render_path(raw_path, &mut path_variables, &operation_label);
        let url = format!("{{{{{base_url_variable}}}}}{rendered_path}");
        let body = if is_swagger {
            self.swagger_body(operation, &parameters, &operation_label)
        } else {
            self.openapi_body(operation.get("requestBody"), &operation_label)
        };
        let auth = self
            .security_auth(
                operation.get("security"),
                true,
                &format!("operation {operation_label}"),
            )
            .unwrap_or(RequestAuth::Inherit);
        let name = clean_name(
            operation
                .get("summary")
                .and_then(Value::as_str)
                .or_else(|| operation.get("operationId").and_then(Value::as_str))
                .unwrap_or(&operation_label),
        );

        if operation
            .get("callbacks")
            .and_then(Value::as_object)
            .is_some_and(|callbacks| !callbacks.is_empty())
        {
            self.warn(format!("Callbacks on {operation_label} were not imported."));
        }
        if path_item.get("servers").is_some() || operation.get("servers").is_some() {
            self.warn(format!(
                "Server override on {operation_label} was omitted; the imported environment uses the document's first server."
            ));
        }

        SavedRequest::new(
            name,
            RequestDefinition {
                method: method.to_ascii_uppercase(),
                url,
                query_params,
                headers,
                auth,
                body,
                options: RequestOptions::default(),
            },
        )
    }

    fn merge_parameters(
        &mut self,
        path_parameters: &[Value],
        operation_parameters: Option<&Value>,
    ) -> Vec<Value> {
        let mut merged = Vec::<Value>::new();
        let mut indexes = HashMap::<(String, String), usize>::new();
        for raw in path_parameters.iter().chain(
            operation_parameters
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        ) {
            let parameter = self.resolve_value(raw, "parameter");
            let key = (
                parameter
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                parameter
                    .get("in")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            );
            if let Some(index) = indexes.get(&key).copied() {
                merged[index] = parameter;
            } else {
                indexes.insert(key, merged.len());
                merged.push(parameter);
            }
        }
        merged
    }

    fn parameter_example(&mut self, parameter: &Map<String, Value>, context: &str) -> String {
        if let Some(example) = parameter.get("example") {
            return value_to_string(example);
        }
        let schema = parameter.get("schema").unwrap_or(&Value::Null);
        let example = self.schema_example(schema, 0, context);
        if !example.is_null() {
            return value_to_string(&example);
        }
        parameter
            .get("default")
            .or_else(|| {
                parameter
                    .get("items")
                    .and_then(|items| items.get("default"))
            })
            .map(value_to_string)
            .unwrap_or_default()
    }

    fn render_path(
        &mut self,
        path: &str,
        path_variables: &mut HashMap<String, String>,
        context: &str,
    ) -> String {
        let mut rendered = path.to_owned();
        for placeholder in path_placeholders(path) {
            let imported_name = path_variables.remove(&placeholder).unwrap_or_else(|| {
                self.warn(format!(
                    "Path variable '{placeholder}' on {context} has no parameter definition."
                ));
                self.ensure_variable(&placeholder, String::new(), false)
            });
            rendered = rendered.replace(
                &format!("{{{placeholder}}}"),
                &format!("{{{{{imported_name}}}}}"),
            );
        }
        rendered
    }

    fn openapi_body(&mut self, raw_body: Option<&Value>, context: &str) -> RequestBody {
        let Some(raw_body) = raw_body else {
            return RequestBody::Empty;
        };
        let body = self.resolve_value(raw_body, &format!("request body for {context}"));
        let Some(content) = body.get("content").and_then(Value::as_object) else {
            self.warn(format!("Request body for {context} has no content map."));
            return RequestBody::Empty;
        };
        let Some((media_type, media)) = preferred_media_type(content) else {
            return RequestBody::Empty;
        };
        self.body_from_media(media_type, media, context)
    }

    fn body_from_media(&mut self, media_type: &str, media: &Value, context: &str) -> RequestBody {
        let schema = media.get("schema").unwrap_or(&Value::Null);
        let explicit_example = media.get("example").cloned().or_else(|| {
            media
                .get("examples")
                .and_then(Value::as_object)
                .and_then(|examples| examples.values().next())
                .and_then(|example| example.get("value").cloned())
        });
        if media_type == "application/x-www-form-urlencoded" {
            return RequestBody::FormUrlEncoded {
                fields: self.schema_fields(schema, context),
            };
        }
        if media_type == "multipart/form-data" {
            return RequestBody::Multipart {
                parts: self.schema_multipart_parts(schema, context),
            };
        }
        let example = explicit_example.unwrap_or_else(|| self.schema_example(schema, 0, context));
        let content = if let Some(content) = example.as_str() {
            content.to_owned()
        } else if media_type.contains("json") || example.is_object() || example.is_array() {
            serde_json::to_string_pretty(&example).unwrap_or_else(|_| "{}".to_owned())
        } else {
            value_to_string(&example)
        };
        if media_rank(media_type) >= 10 {
            self.warn(format!(
                "Request body media type '{media_type}' on {context} was imported as raw text."
            ));
        }
        RequestBody::Text {
            content,
            content_type: Some(media_type.to_owned()),
        }
    }

    fn swagger_body(
        &mut self,
        operation: &Map<String, Value>,
        parameters: &[Value],
        context: &str,
    ) -> RequestBody {
        let consumes = operation
            .get("consumes")
            .or_else(|| self.root.get("consumes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let media_type = consumes.first().copied().unwrap_or("application/json");
        if let Some(body_parameter) = parameters
            .iter()
            .find(|parameter| parameter.get("in").and_then(Value::as_str) == Some("body"))
        {
            return self.body_from_media(media_type, body_parameter, context);
        }
        let form_parameters = parameters
            .iter()
            .filter(|parameter| parameter.get("in").and_then(Value::as_str) == Some("formData"))
            .collect::<Vec<_>>();
        if form_parameters.is_empty() {
            return RequestBody::Empty;
        }
        let multipart = media_type == "multipart/form-data"
            || form_parameters
                .iter()
                .any(|parameter| parameter.get("type").and_then(Value::as_str) == Some("file"));
        if multipart {
            RequestBody::Multipart {
                parts: form_parameters
                    .into_iter()
                    .filter_map(|parameter| {
                        let name = parameter.get("name")?.as_str()?.to_owned();
                        let enabled = parameter
                            .get("required")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let value = if parameter.get("type").and_then(Value::as_str) == Some("file")
                        {
                            self.warn(format!(
                                "File field '{name}' on {context} needs a local file selection."
                            ));
                            MultipartValue::File {
                                path: String::new(),
                                file_name: None,
                                content_type: None,
                            }
                        } else {
                            MultipartValue::Text {
                                content: self.parameter_example(
                                    parameter.as_object().expect("parameter is an object"),
                                    context,
                                ),
                                content_type: None,
                            }
                        };
                        Some(MultipartPart {
                            name,
                            enabled,
                            value,
                        })
                    })
                    .collect(),
            }
        } else {
            RequestBody::FormUrlEncoded {
                fields: form_parameters
                    .into_iter()
                    .filter_map(|parameter| {
                        Some(KeyValueEntry {
                            name: parameter.get("name")?.as_str()?.to_owned(),
                            value: self.parameter_example(parameter.as_object()?, context),
                            enabled: parameter
                                .get("required")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        })
                    })
                    .collect(),
            }
        }
    }

    fn schema_fields(&mut self, schema: &Value, context: &str) -> Vec<KeyValueEntry> {
        self.schema_properties(schema, context)
            .into_iter()
            .map(|(name, property, required)| KeyValueEntry {
                name,
                value: value_to_string(&self.schema_example(&property, 0, context)),
                enabled: required,
            })
            .collect()
    }

    fn schema_multipart_parts(&mut self, schema: &Value, context: &str) -> Vec<MultipartPart> {
        self.schema_properties(schema, context)
            .into_iter()
            .map(|(name, property, required)| {
                let property = self.resolve_value(&property, context);
                let is_file = property.get("format").and_then(Value::as_str) == Some("binary")
                    || property.get("type").and_then(Value::as_str) == Some("file");
                let value = if is_file {
                    self.warn(format!(
                        "File field '{name}' on {context} needs a local file selection."
                    ));
                    MultipartValue::File {
                        path: String::new(),
                        file_name: None,
                        content_type: property
                            .get("contentMediaType")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    }
                } else {
                    MultipartValue::Text {
                        content: value_to_string(&self.schema_example(&property, 0, context)),
                        content_type: None,
                    }
                };
                MultipartPart {
                    name,
                    enabled: required,
                    value,
                }
            })
            .collect()
    }

    fn schema_properties(&mut self, schema: &Value, context: &str) -> Vec<(String, Value, bool)> {
        let schema = self.resolve_value(schema, context);
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<HashSet<_>>();
        schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| {
                properties
                    .iter()
                    .filter(|(_, property)| {
                        !property
                            .get("readOnly")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    })
                    .map(|(name, property)| {
                        (
                            name.clone(),
                            property.clone(),
                            required.contains(name.as_str()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn schema_example(&mut self, schema: &Value, depth: usize, context: &str) -> Value {
        if depth > MAX_SCHEMA_DEPTH {
            self.warn(format!(
                "Schema example generation on {context} exceeded {MAX_SCHEMA_DEPTH} levels."
            ));
            return Value::Null;
        }
        let schema = self.resolve_value(schema, context);
        for key in ["example", "default", "const"] {
            if let Some(value) = schema.get(key) {
                return value.clone();
            }
        }
        if let Some(value) = schema
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
        {
            return value.clone();
        }
        for composition in ["oneOf", "anyOf"] {
            if let Some(first) = schema
                .get(composition)
                .and_then(Value::as_array)
                .and_then(|values| values.first())
            {
                return self.schema_example(first, depth + 1, context);
            }
        }
        if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
            let mut object = Map::new();
            for part in parts {
                if let Value::Object(values) = self.schema_example(part, depth + 1, context) {
                    object.extend(values);
                }
            }
            return Value::Object(object);
        }

        let schema_type = schema
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| schema.get("properties").is_some().then_some("object"))
            .or_else(|| schema.get("items").is_some().then_some("array"))
            .unwrap_or("string");
        match schema_type {
            "object" => {
                let mut output = Map::new();
                if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                    for (name, property) in properties {
                        if property
                            .get("readOnly")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                        {
                            continue;
                        }
                        output.insert(
                            name.clone(),
                            self.schema_example(property, depth + 1, context),
                        );
                    }
                }
                Value::Object(output)
            }
            "array" => schema
                .get("items")
                .map(|items| vec![self.schema_example(items, depth + 1, context)])
                .map(Value::Array)
                .unwrap_or_else(|| Value::Array(Vec::new())),
            "integer" => Value::Number(0.into()),
            "number" => serde_json::Number::from_f64(0.0)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            "boolean" => Value::Bool(false),
            "null" => Value::Null,
            _ => Value::String(sample_string(schema.get("format").and_then(Value::as_str))),
        }
    }
}

fn preferred_media_type(content: &Map<String, Value>) -> Option<(&str, &Value)> {
    content
        .iter()
        .min_by_key(|(media_type, _)| media_rank(media_type))
        .map(|(media_type, media)| (media_type.as_str(), media))
}

fn media_rank(media_type: &str) -> u8 {
    match media_type {
        "application/json" => 0,
        value if value.ends_with("+json") => 1,
        "application/x-www-form-urlencoded" => 2,
        "multipart/form-data" => 3,
        "application/graphql" => 4,
        "application/xml" | "text/xml" => 5,
        value if value.starts_with("text/") => 6,
        _ => 10,
    }
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(values) => values
            .iter()
            .map(value_to_string)
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn path_placeholders(path: &str) -> Vec<String> {
    let mut placeholders = Vec::new();
    let mut remaining = path;
    while let Some(start) = remaining.find('{') {
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find('}') else {
            break;
        };
        let name = &after_start[..end];
        if !name.is_empty() && !name.contains(['{', '}']) {
            placeholders.push(name.to_owned());
        }
        remaining = &after_start[end + 1..];
    }
    placeholders
}

fn variable_name(value: &str) -> String {
    let mut output = String::new();
    let mut uppercase_next = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            if uppercase_next {
                output.extend(character.to_uppercase());
                uppercase_next = false;
            } else {
                output.push(character);
            }
        } else {
            uppercase_next = !output.is_empty();
        }
        if output.chars().count() >= 110 {
            break;
        }
    }
    if output.is_empty() {
        "value".to_owned()
    } else {
        output
    }
}

fn sample_string(format: Option<&str>) -> String {
    match format.unwrap_or_default() {
        "date" => "2026-01-01",
        "date-time" => "2026-01-01T00:00:00Z",
        "uuid" => "00000000-0000-4000-8000-000000000000",
        "email" => "developer@example.com",
        "uri" | "url" => "https://example.com",
        "hostname" => "example.com",
        "ipv4" => "192.0.2.1",
        "ipv6" => "2001:db8::1",
        "password" => "",
        "binary" | "byte" => "",
        _ => "string",
    }
    .to_owned()
}

fn clean_name(value: &str) -> String {
    let cleaned = value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(120)
        .collect::<String>();
    if cleaned.trim().is_empty() {
        "Untitled".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use api_client_core::{RequestAuth, RequestBody};

    use super::import_openapi;

    #[test]
    fn imports_openapi_three_paths_bodies_variables_and_security() {
        let source = serde_yaml_ng::from_str::<serde_json::Value>(
            r#"
openapi: 3.1.0
info:
  title: Pet Store
  version: 1.0.0
servers:
  - url: https://{region}.example.com/v1
    variables:
      region:
        default: eu
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
security:
  - bearerAuth: []
paths:
  /pets/{petId}:
    parameters:
      - name: petId
        in: path
        required: true
        schema:
          type: string
          example: pet-42
    post:
      tags: [Pets]
      summary: Update pet
      parameters:
        - name: verbose
          in: query
          schema:
            type: boolean
            default: false
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [name]
              properties:
                name:
                  type: string
                  example: Ada
                age:
                  type: integer
"#,
        )
        .unwrap();

        let imported = import_openapi(&source, "petstore.yaml").unwrap();
        assert_eq!(imported.name, "Pet Store");
        assert_eq!(imported.collections[0].folders[0].name, "Pets");
        let request = &imported.collections[0].folders[0].requests[0].request;
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "{{baseUrl}}/pets/{{petId}}");
        assert_eq!(request.query_params[0].name, "verbose");
        assert!(!request.query_params[0].enabled);
        assert_eq!(request.auth, RequestAuth::Inherit);
        assert!(matches!(
            request.body,
            RequestBody::Text { ref content, ref content_type }
                if content.contains("\"name\": \"Ada\"")
                    && content_type.as_deref() == Some("application/json")
        ));
        assert!(matches!(
            imported.collections[0].auth,
            Some(RequestAuth::Bearer { .. })
        ));
        let environment = &imported.environments[0];
        assert!(
            environment
                .variables
                .iter()
                .any(|variable| variable.key == "baseUrl"
                    && variable.value == "https://{{region}}.example.com/v1")
        );
        assert!(
            environment
                .variables
                .iter()
                .any(|variable| variable.key == "bearerAuthToken" && variable.secret)
        );
    }

    #[test]
    fn imports_swagger_two_form_data_and_basic_auth() {
        let source = serde_json::json!({
            "swagger": "2.0",
            "info": { "title": "Upload API", "version": "1" },
            "host": "api.example.com",
            "basePath": "/v2",
            "schemes": ["https"],
            "consumes": ["multipart/form-data"],
            "securityDefinitions": {
                "login": { "type": "basic" }
            },
            "security": [{ "login": [] }],
            "paths": {
                "/files": {
                    "post": {
                        "parameters": [
                            { "name": "title", "in": "formData", "type": "string", "required": true },
                            { "name": "file", "in": "formData", "type": "file", "required": true }
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        });

        let imported = import_openapi(&source, "upload.json").unwrap();
        let request = &imported.collections[0].folders[0].requests[0].request;
        assert_eq!(request.url, "{{baseUrl}}/files");
        assert!(matches!(request.body, RequestBody::Multipart { ref parts } if parts.len() == 2));
        assert!(matches!(
            imported.collections[0].auth,
            Some(RequestAuth::Basic { .. })
        ));
        assert!(
            imported.environments[0]
                .variables
                .iter()
                .any(|variable| variable.key == "baseUrl"
                    && variable.value == "https://api.example.com/v2")
        );
    }
}
