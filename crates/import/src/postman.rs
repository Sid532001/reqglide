use std::collections::HashMap;

use api_client_core::{
    ApiKeyPlacement, HeaderEntry, KeyValueEntry, MultipartPart, MultipartValue, OAuth2ClientAuth,
    RequestAuth, RequestBody, RequestDefinition, RequestOptions,
};
use api_client_workspace::{
    Collection, Environment, EnvironmentVariable, RequestFolder, RequestScripts, SavedRequest,
};
use serde_json::Value;

use crate::{ApiDocumentImport, ApiDocumentImportError, ApiDocumentKind};

const MAX_IMPORT_ITEMS: usize = 10_000;
const MAX_IMPORT_DEPTH: usize = 32;

pub(crate) fn import_postman_collection(
    document: &Value,
) -> Result<ApiDocumentImport, ApiDocumentImportError> {
    let name = document
        .pointer("/info/name")
        .and_then(Value::as_str)
        .map(clean_name)
        .unwrap_or_else(|| "Imported Postman collection".to_owned());
    let mut warnings = Vec::new();
    let mut variables = parse_variables(document.get("variable"), &mut warnings);
    let collection_scripts = parse_events(document.get("event"));
    let mut collection = Collection::new(&name);
    collection.auth = parse_parent_auth(
        document.get("auth"),
        &mut variables,
        &mut warnings,
        &format!("{name} collection"),
    );
    let mut item_count = 0usize;
    let (folders, requests) = parse_items(
        document.get("item").and_then(Value::as_array),
        0,
        &collection_scripts,
        &mut variables,
        &mut warnings,
        &mut item_count,
    )?;
    collection.folders = folders;
    collection.requests = requests;

    if collection.folders.is_empty() && collection.requests.is_empty() {
        warnings.push("The Postman collection contained no importable requests.".to_owned());
    }
    if document.get("protocolProfileBehavior").is_some() {
        warnings.push(
            "Postman protocol profile behavior is not represented and was not imported.".to_owned(),
        );
    }

    let environments = (!variables.is_empty())
        .then(|| {
            let mut environment = Environment::new(format!("{name} variables"));
            environment.variables = variables;
            environment
        })
        .into_iter()
        .collect();
    Ok(ApiDocumentImport {
        kind: ApiDocumentKind::PostmanCollection,
        name,
        collections: vec![collection],
        environments,
        warnings,
    })
}

pub(crate) fn import_postman_environment(
    document: &Value,
) -> Result<ApiDocumentImport, ApiDocumentImportError> {
    let name = document
        .get("name")
        .and_then(Value::as_str)
        .map(clean_name)
        .unwrap_or_else(|| "Imported Postman environment".to_owned());
    let mut warnings = Vec::new();
    let mut environment = Environment::new(&name);
    environment.variables = parse_variables(document.get("values"), &mut warnings);
    Ok(ApiDocumentImport {
        kind: ApiDocumentKind::PostmanEnvironment,
        name,
        collections: Vec::new(),
        environments: vec![environment],
        warnings,
    })
}

fn parse_items(
    items: Option<&Vec<Value>>,
    depth: usize,
    inherited_scripts: &RequestScripts,
    variables: &mut Vec<EnvironmentVariable>,
    warnings: &mut Vec<String>,
    item_count: &mut usize,
) -> Result<(Vec<RequestFolder>, Vec<SavedRequest>), ApiDocumentImportError> {
    if depth > MAX_IMPORT_DEPTH {
        return Err(ApiDocumentImportError::Invalid(format!(
            "Postman collection nesting exceeds {MAX_IMPORT_DEPTH} levels"
        )));
    }
    let mut folders = Vec::new();
    let mut requests = Vec::new();
    for item in items.into_iter().flatten() {
        *item_count += 1;
        if *item_count > MAX_IMPORT_ITEMS {
            return Err(ApiDocumentImportError::Invalid(format!(
                "Postman collection contains more than {MAX_IMPORT_ITEMS} items"
            )));
        }
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(clean_name)
            .unwrap_or_else(|| "Untitled".to_owned());
        if item.get("request").is_some() {
            match parse_request(item, &name, inherited_scripts, variables, warnings) {
                Ok(request) => requests.push(request),
                Err(error) => warnings.push(format!("Skipped request '{name}': {error}")),
            }
            continue;
        }
        if let Some(children) = item.get("item").and_then(Value::as_array) {
            let scripts = merge_scripts(inherited_scripts, &parse_events(item.get("event")));
            let mut folder = RequestFolder::new(&name);
            folder.auth = parse_parent_auth(
                item.get("auth"),
                variables,
                warnings,
                &format!("{name} folder"),
            );
            let (nested_folders, nested_requests) = parse_items(
                Some(children),
                depth + 1,
                &scripts,
                variables,
                warnings,
                item_count,
            )?;
            folder.folders = nested_folders;
            folder.requests = nested_requests;
            folders.push(folder);
        } else {
            warnings.push(format!(
                "Skipped Postman item '{name}' because it has no request."
            ));
        }
    }
    Ok((folders, requests))
}

fn parse_request(
    item: &Value,
    name: &str,
    inherited_scripts: &RequestScripts,
    variables: &mut Vec<EnvironmentVariable>,
    warnings: &mut Vec<String>,
) -> Result<SavedRequest, ApiDocumentImportError> {
    let request_value = &item["request"];
    let (method, url, query_params, headers, auth, body) = if let Some(url) = request_value.as_str()
    {
        (
            "GET".to_owned(),
            url.to_owned(),
            Vec::new(),
            Vec::new(),
            RequestAuth::Inherit,
            RequestBody::Empty,
        )
    } else {
        let request = request_value.as_object().ok_or_else(|| {
            ApiDocumentImportError::Invalid("request is not an object or URL".to_owned())
        })?;
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase();
        let (url, query_params) = parse_url(request.get("url"), variables, warnings)?;
        let mut headers = Vec::new();
        for header in request
            .get("header")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(header_name) = header.get("key").and_then(Value::as_str) else {
                continue;
            };
            let raw_value = scalar_string(header.get("value"));
            let value = if is_sensitive_name(header_name) {
                protect_secret_value(
                    variables,
                    &format!("{name} {header_name}"),
                    &raw_value,
                    warnings,
                )
            } else {
                raw_value
            };
            headers.push(HeaderEntry {
                name: header_name.to_owned(),
                value,
                enabled: !header
                    .get("disabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
        let auth = parse_request_auth(request.get("auth"), variables, warnings, name);
        let body = parse_body(request.get("body"), warnings);
        (method, url, query_params, headers, auth, body)
    };
    if url.trim().is_empty() {
        return Err(ApiDocumentImportError::Invalid(
            "request URL is empty".to_owned(),
        ));
    }

    let own_scripts = parse_events(item.get("event"));
    let mut saved = SavedRequest::new(
        name,
        RequestDefinition {
            method,
            url,
            query_params,
            headers,
            auth,
            body,
            options: RequestOptions::default(),
        },
    );
    saved.scripts = merge_scripts(inherited_scripts, &own_scripts);
    if item
        .get("response")
        .and_then(Value::as_array)
        .is_some_and(|responses| !responses.is_empty())
    {
        warnings.push(format!(
            "Saved Postman examples for '{name}' are not yet represented and were omitted."
        ));
    }
    Ok(saved)
}

fn parse_url(
    value: Option<&Value>,
    variables: &mut Vec<EnvironmentVariable>,
    warnings: &mut Vec<String>,
) -> Result<(String, Vec<KeyValueEntry>), ApiDocumentImportError> {
    let Some(value) = value else {
        return Err(ApiDocumentImportError::Invalid(
            "request URL is missing".to_owned(),
        ));
    };
    if let Some(url) = value.as_str() {
        return Ok((url.to_owned(), Vec::new()));
    }
    let object = value.as_object().ok_or_else(|| {
        ApiDocumentImportError::Invalid("request URL has an unsupported shape".to_owned())
    })?;
    let mut raw = object
        .get("raw")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if raw.is_empty() {
        let protocol = object
            .get("protocol")
            .and_then(Value::as_str)
            .unwrap_or("https");
        let host = string_or_array(object.get("host"), ".");
        let path = string_or_array(object.get("path"), "/");
        raw = format!("{protocol}://{host}");
        if !path.is_empty() {
            raw.push('/');
            raw.push_str(path.trim_start_matches('/'));
        }
    }
    let query_params = object
        .get("query")
        .and_then(Value::as_array)
        .map(|query| {
            query
                .iter()
                .filter_map(|parameter| {
                    let name = parameter.get("key")?.as_str()?.to_owned();
                    Some(KeyValueEntry {
                        name,
                        value: scalar_string(parameter.get("value")),
                        enabled: !parameter
                            .get("disabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !query_params.is_empty() {
        raw = strip_query(&raw);
    }
    if let Some(url_variables) = object.get("variable").and_then(Value::as_array) {
        for variable in url_variables {
            add_variable(
                variables,
                variable.get("key").and_then(Value::as_str),
                variable.get("value"),
                false,
                warnings,
            );
        }
    }
    Ok((raw, query_params))
}

fn parse_body(value: Option<&Value>, warnings: &mut Vec<String>) -> RequestBody {
    let Some(body) = value.and_then(Value::as_object) else {
        return RequestBody::Empty;
    };
    if body
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return RequestBody::Empty;
    }
    match body.get("mode").and_then(Value::as_str).unwrap_or_default() {
        "raw" => {
            let content = body
                .get("raw")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let language = body
                .get("options")
                .and_then(|options| options.get("raw"))
                .and_then(|raw| raw.get("language"))
                .and_then(Value::as_str);
            RequestBody::Text {
                content,
                content_type: language.and_then(content_type_for_language).map(str::to_owned),
            }
        }
        "urlencoded" => RequestBody::FormUrlEncoded {
            fields: body
                .get("urlencoded")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|field| {
                    Some(KeyValueEntry {
                        name: field.get("key")?.as_str()?.to_owned(),
                        value: scalar_string(field.get("value")),
                        enabled: !field
                            .get("disabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect(),
        },
        "formdata" => RequestBody::Multipart {
            parts: body
                .get("formdata")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|part| {
                    let name = part.get("key")?.as_str()?.to_owned();
                    let enabled = !part
                        .get("disabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let content_type = part
                        .get("contentType")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let value = if part.get("type").and_then(Value::as_str) == Some("file") {
                        let path = part
                            .get("src")
                            .and_then(|value| {
                                value.as_str().or_else(|| {
                                    value
                                        .as_array()
                                        .and_then(|values| values.first())
                                        .and_then(Value::as_str)
                                })
                            })
                            .unwrap_or_default();
                        if path.is_empty() {
                            warnings.push(format!(
                                "Multipart file field '{name}' has no local path and must be selected again."
                            ));
                        }
                        MultipartValue::File {
                            path: path.to_owned(),
                            file_name: None,
                            content_type,
                        }
                    } else {
                        MultipartValue::Text {
                            content: scalar_string(part.get("value")),
                            content_type,
                        }
                    };
                    Some(MultipartPart {
                        name,
                        enabled,
                        value,
                    })
                })
                .collect(),
        },
        "file" => RequestBody::Binary {
            path: body
                .get("file")
                .and_then(|file| file.get("src"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            content_type: None,
        },
        "graphql" => {
            let query = body
                .get("graphql")
                .and_then(|graphql| graphql.get("query"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let variables = body
                .get("graphql")
                .and_then(|graphql| graphql.get("variables"))
                .and_then(Value::as_str)
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            RequestBody::Text {
                content: serde_json::to_string_pretty(&serde_json::json!({
                    "query": query,
                    "variables": variables,
                }))
                .unwrap_or_default(),
                content_type: Some("application/json".to_owned()),
            }
        }
        mode if !mode.is_empty() => {
            warnings.push(format!(
                "Postman body mode '{mode}' is not supported and was imported as empty."
            ));
            RequestBody::Empty
        }
        _ => RequestBody::Empty,
    }
}

fn parse_parent_auth(
    value: Option<&Value>,
    variables: &mut Vec<EnvironmentVariable>,
    warnings: &mut Vec<String>,
    context: &str,
) -> Option<RequestAuth> {
    value
        .filter(|value| !value.is_null())
        .map(|value| parse_auth(value, variables, warnings, context))
}

fn parse_request_auth(
    value: Option<&Value>,
    variables: &mut Vec<EnvironmentVariable>,
    warnings: &mut Vec<String>,
    context: &str,
) -> RequestAuth {
    value
        .filter(|value| !value.is_null())
        .map_or(RequestAuth::Inherit, |value| {
            parse_auth(value, variables, warnings, context)
        })
}

fn parse_auth(
    value: &Value,
    variables: &mut Vec<EnvironmentVariable>,
    warnings: &mut Vec<String>,
    context: &str,
) -> RequestAuth {
    let auth_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("noauth");
    let values = value
        .get(auth_type)
        .and_then(Value::as_array)
        .map(|values| auth_value_map(values))
        .unwrap_or_default();
    match auth_type {
        "noauth" => RequestAuth::None,
        "basic" => RequestAuth::Basic {
            username: values.get("username").cloned().unwrap_or_default(),
            password: protect_secret_value(
                variables,
                &format!("{context} password"),
                values
                    .get("password")
                    .map(String::as_str)
                    .unwrap_or_default(),
                warnings,
            ),
        },
        "bearer" => RequestAuth::Bearer {
            token: protect_secret_value(
                variables,
                &format!("{context} token"),
                values.get("token").map(String::as_str).unwrap_or_default(),
                warnings,
            ),
        },
        "apikey" => RequestAuth::ApiKey {
            key: values
                .get("key")
                .cloned()
                .unwrap_or_else(|| "X-API-Key".to_owned()),
            value: protect_secret_value(
                variables,
                &format!("{context} API key"),
                values.get("value").map(String::as_str).unwrap_or_default(),
                warnings,
            ),
            placement: if values.get("in").is_some_and(|value| value == "query") {
                ApiKeyPlacement::Query
            } else {
                ApiKeyPlacement::Header
            },
        },
        "oauth2" => parse_oauth2_auth(&values, variables, warnings, context),
        unsupported => {
            warnings.push(format!(
                "Postman {context} auth type '{unsupported}' is not supported and was set to No Auth."
            ));
            RequestAuth::None
        }
    }
}

fn parse_oauth2_auth(
    values: &HashMap<String, String>,
    variables: &mut Vec<EnvironmentVariable>,
    warnings: &mut Vec<String>,
    context: &str,
) -> RequestAuth {
    let grant_type = values
        .get("grant_type")
        .or_else(|| values.get("grantType"))
        .map(String::as_str)
        .unwrap_or_default();
    let token_url = values
        .get("accessTokenUrl")
        .or_else(|| values.get("token_url"))
        .cloned()
        .unwrap_or_default();
    let client_id = values
        .get("clientId")
        .or_else(|| values.get("client_id"))
        .cloned()
        .unwrap_or_default();
    if grant_type.contains("client_credentials") || (!token_url.is_empty() && !client_id.is_empty())
    {
        return RequestAuth::OAuth2ClientCredentials {
            issuer_url: String::new(),
            token_url,
            client_id,
            client_secret: protect_secret_value(
                variables,
                &format!("{context} OAuth client secret"),
                values
                    .get("clientSecret")
                    .or_else(|| values.get("client_secret"))
                    .map(String::as_str)
                    .unwrap_or_default(),
                warnings,
            ),
            scope: values.get("scope").cloned().unwrap_or_default(),
            audience: values.get("audience").cloned().unwrap_or_default(),
            client_auth: if values
                .get("client_authentication")
                .is_some_and(|value| value.contains("body"))
            {
                OAuth2ClientAuth::RequestBody
            } else {
                OAuth2ClientAuth::Basic
            },
            extra_parameters: Vec::new(),
        };
    }
    if let Some(token) = values.get("accessToken").or_else(|| values.get("token")) {
        warnings.push(format!(
            "Postman {context} OAuth flow could not be reproduced; its current access token was imported as Bearer auth."
        ));
        return RequestAuth::Bearer {
            token: protect_secret_value(
                variables,
                &format!("{context} OAuth token"),
                token,
                warnings,
            ),
        };
    }
    warnings.push(format!(
        "Postman {context} OAuth configuration could not be mapped and was set to No Auth."
    ));
    RequestAuth::None
}

fn auth_value_map(values: &[Value]) -> HashMap<String, String> {
    values
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("key")?.as_str()?.to_owned(),
                scalar_string(entry.get("value")),
            ))
        })
        .collect()
}

fn parse_events(value: Option<&Value>) -> RequestScripts {
    let mut scripts = RequestScripts::default();
    for event in value.and_then(Value::as_array).into_iter().flatten() {
        let source = event
            .pointer("/script/exec")
            .map(|value| match value {
                Value::Array(lines) => lines
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n"),
                Value::String(source) => source.clone(),
                _ => String::new(),
            })
            .unwrap_or_default();
        match event.get("listen").and_then(Value::as_str) {
            Some("prerequest") => append_script(&mut scripts.pre_request, &source),
            Some("test") => append_script(&mut scripts.post_response, &source),
            _ => {}
        }
    }
    scripts
}

fn merge_scripts(parent: &RequestScripts, own: &RequestScripts) -> RequestScripts {
    let mut merged = parent.clone();
    append_script(&mut merged.pre_request, &own.pre_request);
    append_script(&mut merged.post_response, &own.post_response);
    merged
}

fn append_script(destination: &mut String, source: &str) {
    if source.trim().is_empty() {
        return;
    }
    if !destination.is_empty() {
        destination.push_str("\n\n");
    }
    destination.push_str(source);
}

fn parse_variables(value: Option<&Value>, warnings: &mut Vec<String>) -> Vec<EnvironmentVariable> {
    let mut variables = Vec::new();
    for variable in value.and_then(Value::as_array).into_iter().flatten() {
        let key = variable.get("key").and_then(Value::as_str);
        let secret = variable.get("type").and_then(Value::as_str) == Some("secret")
            || key.is_some_and(is_sensitive_name);
        let added_index =
            add_variable(&mut variables, key, variable.get("value"), secret, warnings);
        if let Some(imported) = added_index.and_then(|index| variables.get_mut(index)) {
            imported.enabled = variable
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
        }
    }
    variables
}

fn add_variable(
    variables: &mut Vec<EnvironmentVariable>,
    key: Option<&str>,
    value: Option<&Value>,
    secret: bool,
    warnings: &mut Vec<String>,
) -> Option<usize> {
    let key = key.map(str::trim).filter(|key| !key.is_empty())?;
    if key.chars().any(char::is_control) || key.chars().count() > 120 {
        warnings.push(format!(
            "Skipped variable '{key}' because its name is not valid in a local workspace."
        ));
        return None;
    }
    if variables.iter().any(|variable| variable.key == key) {
        warnings.push(format!(
            "Ignored duplicate variable '{key}'; the first imported value was kept."
        ));
        return None;
    }
    let imported_value = scalar_string(value);
    let mut variable = EnvironmentVariable::new(key, if secret { "" } else { &imported_value });
    if secret {
        variable.secret = true;
        warnings.push(format!(
            "Secret variable '{key}' was imported without its plaintext value; set it in the OS-backed secret editor."
        ));
    }
    variables.push(variable);
    Some(variables.len() - 1)
}

fn scalar_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn protect_secret_value(
    variables: &mut Vec<EnvironmentVariable>,
    context: &str,
    value: &str,
    warnings: &mut Vec<String>,
) -> String {
    if value.is_empty() || value.contains("{{") {
        return value.to_owned();
    }
    let base = variable_name(context);
    let mut key = base.clone();
    let mut suffix = 2usize;
    while variables.iter().any(|variable| variable.key == key) {
        key = format!("{base}{suffix}");
        suffix += 1;
    }
    let mut variable = EnvironmentVariable::new(&key, "");
    variable.secret = true;
    variables.push(variable);
    warnings.push(format!(
        "A credential in {context} was not persisted; set '{key}' in the OS-backed secret editor."
    ));
    format!("{{{{{key}}}}}")
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
        if output.len() >= 110 {
            break;
        }
    }
    if output.is_empty() {
        "importedSecret".to_owned()
    } else {
        output
    }
}

fn is_sensitive_name(name: &str) -> bool {
    let normalized = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "authorization",
        "proxyauthorization",
        "cookie",
        "setcookie",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "clientsecret",
        "password",
        "passwd",
        "secret",
        "token",
    ]
    .iter()
    .any(|sensitive| normalized == *sensitive || normalized.ends_with(sensitive))
}

fn string_or_array(value: Option<&Value>, separator: &str) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(separator),
        _ => String::new(),
    }
}

fn strip_query(url: &str) -> String {
    let (base, fragment) = url
        .split_once('#')
        .map_or((url, None), |(base, fragment)| (base, Some(fragment)));
    let base = base.split_once('?').map_or(base, |(base, _)| base);
    fragment.map_or_else(|| base.to_owned(), |fragment| format!("{base}#{fragment}"))
}

fn content_type_for_language(language: &str) -> Option<&'static str> {
    match language.to_ascii_lowercase().as_str() {
        "json" => Some("application/json"),
        "xml" => Some("application/xml"),
        "html" => Some("text/html"),
        "javascript" | "js" => Some("application/javascript"),
        "text" => Some("text/plain"),
        _ => None,
    }
}

fn clean_name(value: &str) -> String {
    let value = value.trim();
    let cleaned = value
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
    use api_client_workspace::WorkspaceDocument;

    use super::{import_postman_collection, import_postman_environment};

    #[test]
    fn imports_collection_tree_scripts_and_scrubs_credentials() {
        let source = serde_json::json!({
            "info": {
                "name": "Demo",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "variable": [
                { "key": "client_secret", "value": "plaintext", "enabled": true },
                { "key": "client_secret", "value": "replacement", "enabled": false }
            ],
            "auth": {
                "type": "bearer",
                "bearer": [{ "key": "token", "value": "literal-bearer" }]
            },
            "event": [{
                "listen": "prerequest",
                "script": { "exec": ["pm.variables.set('fromCollection', 'yes');"] }
            }],
            "item": [{
                "name": "Pets",
                "event": [{
                    "listen": "test",
                    "script": { "exec": ["pm.test('folder test', () => true);"] }
                }],
                "item": [{
                    "name": "Create pet",
                    "request": {
                        "method": "POST",
                        "url": {
                            "raw": "https://api.example.com/pets?verbose=true",
                            "query": [{ "key": "verbose", "value": "true" }]
                        },
                        "header": [
                            { "key": "Authorization", "value": "Basic dXNlcjpwYXNz" },
                            { "key": "Content-Type", "value": "application/json" }
                        ],
                        "body": {
                            "mode": "raw",
                            "raw": "{\"name\":\"Ada\"}",
                            "options": { "raw": { "language": "json" } }
                        }
                    },
                    "event": [{
                        "listen": "test",
                        "script": { "exec": ["pm.test('request test', () => true);"] }
                    }]
                }]
            }]
        });

        let imported = import_postman_collection(&source).unwrap();
        let collection = &imported.collections[0];
        assert!(matches!(
            collection.auth,
            Some(RequestAuth::Bearer { ref token }) if token == "{{DemoCollectionToken}}"
        ));
        let request = &collection.folders[0].requests[0];
        assert_eq!(request.request.url, "https://api.example.com/pets");
        assert_eq!(request.request.query_params[0].name, "verbose");
        assert_eq!(
            request.request.headers[0].value,
            "{{CreatePetAuthorization}}"
        );
        assert!(matches!(
            request.request.body,
            RequestBody::Text { ref content_type, .. }
                if content_type.as_deref() == Some("application/json")
        ));
        assert!(request.scripts.pre_request.contains("fromCollection"));
        assert!(request.scripts.post_response.contains("folder test"));
        assert!(request.scripts.post_response.contains("request test"));

        let environment = &imported.environments[0];
        let original_secret = environment
            .variables
            .iter()
            .find(|variable| variable.key == "client_secret")
            .unwrap();
        assert!(original_secret.secret);
        assert!(original_secret.enabled);
        assert!(original_secret.value.is_empty());
        assert!(environment.variables.iter().any(|variable| {
            variable.key == "DemoCollectionToken" && variable.secret && variable.value.is_empty()
        }));
        assert!(environment.variables.iter().any(|variable| {
            variable.key == "CreatePetAuthorization" && variable.secret && variable.value.is_empty()
        }));
        assert!(
            imported
                .warnings
                .iter()
                .any(|warning| warning.contains("duplicate variable"))
        );

        let mut workspace = WorkspaceDocument::new("Imported");
        workspace.collections = imported.collections;
        workspace.environments = imported.environments;
        workspace.validate().unwrap();
    }

    #[test]
    fn imports_postman_environment_without_plaintext_secrets() {
        let source = serde_json::json!({
            "name": "Staging",
            "_postman_variable_scope": "environment",
            "values": [
                { "key": "baseUrl", "value": "https://staging.example.com" },
                { "key": "apiToken", "value": "do-not-persist", "type": "secret" }
            ]
        });

        let imported = import_postman_environment(&source).unwrap();
        assert!(imported.collections.is_empty());
        assert_eq!(imported.environments[0].name, "Staging");
        assert_eq!(
            imported.environments[0].variables[0].value,
            "https://staging.example.com"
        );
        assert!(imported.environments[0].variables[1].secret);
        assert!(imported.environments[0].variables[1].value.is_empty());
    }
}
