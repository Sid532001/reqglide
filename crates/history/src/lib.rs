//! Bounded, local SQLite request history with credential-aware snapshots.

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use api_client_core::{
    MultipartValue, RequestAuth, RequestBody, RequestDefinition, ResponseSnapshot,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

const DATABASE_VERSION: i64 = 1;
const MAX_HISTORY_ENTRIES: usize = 2_000;
const MAX_HISTORY_REQUEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_QUERY_CHARS: usize = 256;
const MAX_PAGE_SIZE: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRecordInput {
    pub workspace_id: String,
    pub request_id: Option<String>,
    pub request_name: String,
    pub request: RequestDefinition,
    pub response: Option<HistoryResponseSummary>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryResponseSummary {
    pub status: u16,
    pub status_text: String,
    pub duration_ms: u64,
    pub received_bytes: u64,
    pub content_type: Option<String>,
}

impl From<&ResponseSnapshot> for HistoryResponseSummary {
    fn from(response: &ResponseSnapshot) -> Self {
        Self {
            status: response.status,
            status_text: response.status_text.clone(),
            duration_ms: response.timing.total_ms,
            received_bytes: response.received_bytes,
            content_type: response.body.content_type.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub workspace_id: String,
    pub request_id: Option<String>,
    pub request_name: String,
    pub method: String,
    pub url: String,
    pub request: RequestDefinition,
    pub request_body_omitted: bool,
    pub status: Option<u16>,
    pub status_text: Option<String>,
    pub duration_ms: Option<u64>,
    pub received_bytes: Option<u64>,
    pub content_type: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryQuery {
    pub workspace_id: String,
    #[serde(default)]
    pub search: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

const fn default_limit() -> usize {
    50
}

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("history database operation failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("history storage operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("history data is invalid: {0}")]
    Invalid(String),
    #[error("history request snapshot could not be encoded: {0}")]
    Serialize(String),
}

pub struct HistoryStore {
    path: PathBuf,
}

impl HistoryStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, HistoryError> {
        let store = Self { path: path.into() };
        store.with_connection(|_| Ok(()))?;
        Ok(store)
    }

    pub fn record(&self, input: HistoryRecordInput) -> Result<HistoryEntry, HistoryError> {
        validate_workspace_id(&input.workspace_id)?;
        if let Some(request_id) = &input.request_id {
            validate_uuid(request_id, "request ID")?;
        }
        if input.request_name.trim().is_empty() || input.request_name.chars().count() > 200 {
            return Err(HistoryError::Invalid(
                "request name must contain between 1 and 200 characters".to_owned(),
            ));
        }
        input
            .request
            .validate()
            .map_err(|error| HistoryError::Invalid(error.to_string()))?;

        let id = Uuid::new_v4().to_string();
        let created_at_ms = now_ms()?;
        let (request, request_json, request_body_omitted) =
            prepare_request_snapshot(&input.request)?;
        let response = input.response;
        let status = response.as_ref().map(|value| value.status);
        let status_text = response.as_ref().map(|value| value.status_text.clone());
        let duration_ms = response.as_ref().map(|value| value.duration_ms);
        let received_bytes = response.as_ref().map(|value| value.received_bytes);
        let duration_ms_database = duration_ms
            .map(i64::try_from)
            .transpose()
            .map_err(|_| HistoryError::Invalid("response duration is too large".to_owned()))?;
        let received_bytes_database = received_bytes
            .map(i64::try_from)
            .transpose()
            .map_err(|_| HistoryError::Invalid("response byte count is too large".to_owned()))?;
        let content_type = response.and_then(|value| value.content_type);
        let request_name = input.request_name.trim().to_owned();
        let method = request.method.clone();
        let url = request.url.clone();

        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO history_entries (
                    id, workspace_id, request_id, request_name, method, url,
                    request_json, request_body_omitted, status, status_text,
                    duration_ms, received_bytes, content_type, error, created_at_ms, pinned
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0)",
                params![
                    id,
                    input.workspace_id,
                    input.request_id,
                    request_name,
                    method,
                    url,
                    request_json,
                    request_body_omitted,
                    status,
                    status_text,
                    duration_ms_database,
                    received_bytes_database,
                    content_type,
                    input.error,
                    created_at_ms,
                ],
            )?;
            transaction.execute(
                "INSERT INTO history_fts (
                    entry_id, workspace_id, request_name, method, url, status_text, error
                 ) VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?6, ''), COALESCE(?7, ''))",
                params![
                    id,
                    input.workspace_id,
                    request_name,
                    method,
                    url,
                    status_text,
                    input.error,
                ],
            )?;
            prune_history(&transaction, &input.workspace_id)?;
            transaction.commit()?;
            Ok(())
        })?;

        Ok(HistoryEntry {
            id,
            workspace_id: input.workspace_id,
            request_id: input.request_id,
            request_name,
            method,
            url,
            request,
            request_body_omitted,
            status,
            status_text,
            duration_ms,
            received_bytes,
            content_type,
            error: input.error,
            created_at_ms,
            pinned: false,
        })
    }

    pub fn list(&self, query: &HistoryQuery) -> Result<Vec<HistoryEntry>, HistoryError> {
        validate_workspace_id(&query.workspace_id)?;
        if query.search.chars().count() > MAX_QUERY_CHARS {
            return Err(HistoryError::Invalid(format!(
                "history search is limited to {MAX_QUERY_CHARS} characters"
            )));
        }
        if query.limit == 0 || query.limit > MAX_PAGE_SIZE {
            return Err(HistoryError::Invalid(format!(
                "history page size must be between 1 and {MAX_PAGE_SIZE}"
            )));
        }
        let search = fts_query(&query.search);
        self.with_connection(|connection| {
            let sql = if search.is_some() {
                "SELECT e.id, e.workspace_id, e.request_id, e.request_name, e.method, e.url,
                        e.request_json, e.request_body_omitted, e.status, e.status_text,
                        e.duration_ms, e.received_bytes, e.content_type, e.error,
                        e.created_at_ms, e.pinned
                   FROM history_entries e
                   JOIN history_fts f ON f.entry_id = e.id
                  WHERE e.workspace_id = ?1 AND history_fts MATCH ?2
                  ORDER BY e.pinned DESC, e.created_at_ms DESC
                  LIMIT ?3 OFFSET ?4"
            } else {
                "SELECT id, workspace_id, request_id, request_name, method, url,
                        request_json, request_body_omitted, status, status_text,
                        duration_ms, received_bytes, content_type, error,
                        created_at_ms, pinned
                   FROM history_entries
                  WHERE workspace_id = ?1
                  ORDER BY pinned DESC, created_at_ms DESC
                  LIMIT ?3 OFFSET ?4"
            };
            let mut statement = connection.prepare(sql)?;
            let rows = statement.query_map(
                params![
                    query.workspace_id,
                    search.unwrap_or_default(),
                    query.limit as i64,
                    query.offset as i64
                ],
                history_entry_from_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(HistoryError::from)
        })
    }

    pub fn set_pinned(
        &self,
        workspace_id: &str,
        entry_id: &str,
        pinned: bool,
    ) -> Result<(), HistoryError> {
        validate_workspace_id(workspace_id)?;
        validate_uuid(entry_id, "history entry ID")?;
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE history_entries SET pinned = ?1 WHERE id = ?2 AND workspace_id = ?3",
                params![pinned, entry_id, workspace_id],
            )?;
            if changed == 0 {
                return Err(HistoryError::Invalid(
                    "history entry was not found".to_owned(),
                ));
            }
            Ok(())
        })
    }

    pub fn delete(&self, workspace_id: &str, entry_id: &str) -> Result<(), HistoryError> {
        validate_workspace_id(workspace_id)?;
        validate_uuid(entry_id, "history entry ID")?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "DELETE FROM history_fts WHERE entry_id = ?1 AND workspace_id = ?2",
                params![entry_id, workspace_id],
            )?;
            transaction.execute(
                "DELETE FROM history_entries WHERE id = ?1 AND workspace_id = ?2",
                params![entry_id, workspace_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn clear(&self, workspace_id: &str, include_pinned: bool) -> Result<usize, HistoryError> {
        validate_workspace_id(workspace_id)?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let predicate = if include_pinned {
                "workspace_id = ?1"
            } else {
                "workspace_id = ?1 AND entry_id IN (
                    SELECT id FROM history_entries WHERE workspace_id = ?1 AND pinned = 0
                )"
            };
            transaction.execute(
                &format!("DELETE FROM history_fts WHERE {predicate}"),
                [workspace_id],
            )?;
            let changed = if include_pinned {
                transaction.execute(
                    "DELETE FROM history_entries WHERE workspace_id = ?1",
                    [workspace_id],
                )?
            } else {
                transaction.execute(
                    "DELETE FROM history_entries WHERE workspace_id = ?1 AND pinned = 0",
                    [workspace_id],
                )?
            };
            transaction.commit()?;
            Ok(changed)
        })
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, HistoryError>,
    ) -> Result<T, HistoryError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| HistoryError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut connection = Connection::open(&self.path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        initialize(&connection)?;
        operation(&mut connection)
    }
}

fn initialize(connection: &Connection) -> Result<(), HistoryError> {
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA trusted_schema = OFF;
         CREATE TABLE IF NOT EXISTS history_entries (
             id TEXT PRIMARY KEY NOT NULL,
             workspace_id TEXT NOT NULL,
             request_id TEXT,
             request_name TEXT NOT NULL,
             method TEXT NOT NULL,
             url TEXT NOT NULL,
             request_json TEXT NOT NULL,
             request_body_omitted INTEGER NOT NULL DEFAULT 0,
             status INTEGER,
             status_text TEXT,
             duration_ms INTEGER,
             received_bytes INTEGER,
             content_type TEXT,
             error TEXT,
             created_at_ms INTEGER NOT NULL,
             pinned INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS history_workspace_time
             ON history_entries(workspace_id, pinned DESC, created_at_ms DESC);
         CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(
             entry_id UNINDEXED,
             workspace_id UNINDEXED,
             request_name,
             method,
             url,
             status_text,
             error
         );",
    )?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > DATABASE_VERSION {
        return Err(HistoryError::Invalid(format!(
            "history database version {version} is newer than supported version {DATABASE_VERSION}"
        )));
    }
    connection.pragma_update(None, "user_version", DATABASE_VERSION)?;
    Ok(())
}

fn prune_history(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: &str,
) -> Result<(), rusqlite::Error> {
    transaction.execute(
        "DELETE FROM history_fts
          WHERE entry_id IN (
              SELECT id FROM history_entries
               WHERE workspace_id = ?1 AND pinned = 0
               ORDER BY created_at_ms DESC
               LIMIT -1 OFFSET ?2
          )",
        params![workspace_id, MAX_HISTORY_ENTRIES as i64],
    )?;
    transaction.execute(
        "DELETE FROM history_entries
          WHERE id IN (
              SELECT id FROM history_entries
               WHERE workspace_id = ?1 AND pinned = 0
               ORDER BY created_at_ms DESC
               LIMIT -1 OFFSET ?2
          )",
        params![workspace_id, MAX_HISTORY_ENTRIES as i64],
    )?;
    Ok(())
}

fn history_entry_from_row(row: &rusqlite::Row<'_>) -> Result<HistoryEntry, rusqlite::Error> {
    let request_json: String = row.get(6)?;
    let request = serde_json::from_str(&request_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            request_json.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;
    Ok(HistoryEntry {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        request_id: row.get(2)?,
        request_name: row.get(3)?,
        method: row.get(4)?,
        url: row.get(5)?,
        request,
        request_body_omitted: row.get(7)?,
        status: row.get(8)?,
        status_text: row.get(9)?,
        duration_ms: optional_u64(row, 10)?,
        received_bytes: optional_u64(row, 11)?,
        content_type: row.get(12)?,
        error: row.get(13)?,
        created_at_ms: row.get(14)?,
        pinned: row.get(15)?,
    })
}

fn optional_u64(row: &rusqlite::Row<'_>, index: usize) -> Result<Option<u64>, rusqlite::Error> {
    let value: Option<i64> = row.get(index)?;
    value
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Integer,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "history contains a negative unsigned value",
                    )),
                )
            })
        })
        .transpose()
}

fn prepare_request_snapshot(
    request: &RequestDefinition,
) -> Result<(RequestDefinition, String, bool), HistoryError> {
    let mut sanitized = sanitize_request(request);
    let mut json = serde_json::to_string(&sanitized)
        .map_err(|error| HistoryError::Serialize(error.to_string()))?;
    let mut body_omitted = false;
    if json.len() > MAX_HISTORY_REQUEST_BYTES {
        sanitized.body = RequestBody::Empty;
        body_omitted = true;
        json = serde_json::to_string(&sanitized)
            .map_err(|error| HistoryError::Serialize(error.to_string()))?;
    }
    if json.len() > MAX_HISTORY_REQUEST_BYTES {
        return Err(HistoryError::Invalid(format!(
            "history request metadata exceeds the {} MiB safety limit",
            MAX_HISTORY_REQUEST_BYTES / 1024 / 1024
        )));
    }
    Ok((sanitized, json, body_omitted))
}

fn sanitize_request(request: &RequestDefinition) -> RequestDefinition {
    let mut request = request.clone();
    request.url = redact_url_query(&request.url);
    for parameter in &mut request.query_params {
        if is_sensitive_name(&parameter.name) {
            parameter.value = redact_literal(&parameter.value);
        }
    }
    for header in &mut request.headers {
        if is_sensitive_name(&header.name) {
            header.value = redact_literal(&header.value);
        }
    }
    request.auth = match request.auth {
        RequestAuth::Basic { username, password } => RequestAuth::Basic {
            username,
            password: redact_literal(&password),
        },
        RequestAuth::Bearer { token } => RequestAuth::Bearer {
            token: redact_literal(&token),
        },
        RequestAuth::ApiKey {
            key,
            value,
            placement,
        } => RequestAuth::ApiKey {
            key,
            value: redact_literal(&value),
            placement,
        },
        RequestAuth::OAuth2ClientCredentials {
            issuer_url,
            token_url,
            client_id,
            client_secret,
            scope,
            audience,
            client_auth,
            extra_parameters,
        } => RequestAuth::OAuth2ClientCredentials {
            issuer_url,
            token_url,
            client_id,
            client_secret: redact_literal(&client_secret),
            scope,
            audience,
            client_auth,
            extra_parameters: extra_parameters
                .into_iter()
                .map(|mut parameter| {
                    if is_sensitive_name(&parameter.name) {
                        parameter.value = redact_literal(&parameter.value);
                    }
                    parameter
                })
                .collect(),
        },
        RequestAuth::Inherit => RequestAuth::Inherit,
        RequestAuth::None => RequestAuth::None,
    };
    match &mut request.body {
        RequestBody::FormUrlEncoded { fields } => {
            for field in fields {
                if is_sensitive_name(&field.name) {
                    field.value = redact_literal(&field.value);
                }
            }
        }
        RequestBody::Multipart { parts } => {
            for part in parts {
                if is_sensitive_name(&part.name)
                    && let MultipartValue::Text { content, .. } = &mut part.value
                {
                    *content = redact_literal(content);
                }
            }
        }
        RequestBody::Empty | RequestBody::Text { .. } | RequestBody::Binary { .. } => {}
    }
    request
}

fn redact_url_query(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return value.to_owned();
    };
    let pairs = url
        .query_pairs()
        .map(|(name, value)| {
            let value = if is_sensitive_name(&name) {
                redact_literal(&value)
            } else {
                value.into_owned()
            };
            (name.into_owned(), value)
        })
        .collect::<Vec<_>>();
    if pairs.is_empty() {
        return value.to_owned();
    }
    url.query_pairs_mut().clear().extend_pairs(pairs);
    url.to_string()
}

fn redact_literal(value: &str) -> String {
    if value.contains("{{") && value.contains("}}") {
        value.to_owned()
    } else {
        "[REDACTED]".to_owned()
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
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "clientsecret",
    ]
    .iter()
    .any(|sensitive| normalized == *sensitive || normalized.ends_with(sensitive))
}

fn fts_query(value: &str) -> Option<String> {
    let terms = value
        .split_whitespace()
        .filter_map(|term| {
            let safe = term.replace('"', "\"\"");
            (!safe.is_empty()).then(|| format!("\"{safe}\"*"))
        })
        .collect::<Vec<_>>();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn validate_workspace_id(value: &str) -> Result<(), HistoryError> {
    validate_uuid(value, "workspace ID")
}

fn validate_uuid(value: &str, label: &str) -> Result<(), HistoryError> {
    Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| HistoryError::Invalid(format!("{label} is not a valid UUID")))
}

fn now_ms() -> Result<i64, HistoryError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| HistoryError::Invalid("system clock is before Unix epoch".to_owned()))?;
    i64::try_from(duration.as_millis())
        .map_err(|_| HistoryError::Invalid("system clock is outside supported range".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client_core::{HeaderEntry, RequestOptions};
    use tempfile::tempdir;

    fn request() -> RequestDefinition {
        RequestDefinition {
            method: "POST".to_owned(),
            url: "https://example.com/token?api_key=literal&visible=yes".to_owned(),
            query_params: vec![],
            headers: vec![HeaderEntry {
                name: "Authorization".to_owned(),
                value: "Bearer literal".to_owned(),
                enabled: true,
            }],
            auth: RequestAuth::Bearer {
                token: "{{accessToken}}".to_owned(),
            },
            body: RequestBody::FormUrlEncoded {
                fields: vec![api_client_core::KeyValueEntry {
                    name: "client_secret".to_owned(),
                    value: "literal-secret".to_owned(),
                    enabled: true,
                }],
            },
            options: RequestOptions::default(),
        }
    }

    fn record(workspace_id: &str, name: &str) -> HistoryRecordInput {
        HistoryRecordInput {
            workspace_id: workspace_id.to_owned(),
            request_id: None,
            request_name: name.to_owned(),
            request: request(),
            response: Some(HistoryResponseSummary {
                status: 200,
                status_text: "OK".to_owned(),
                duration_ms: 12,
                received_bytes: 3,
                content_type: Some("application/json".to_owned()),
            }),
            error: None,
        }
    }

    #[test]
    fn records_searches_pins_and_deletes_history() {
        let directory = tempdir().unwrap();
        let store = HistoryStore::open(directory.path().join("history.sqlite3")).unwrap();
        let workspace_id = Uuid::new_v4().to_string();
        let entry = store.record(record(&workspace_id, "Create token")).unwrap();

        let listed = store
            .list(&HistoryQuery {
                workspace_id: workspace_id.clone(),
                search: "token".to_owned(),
                limit: 20,
                offset: 0,
            })
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, entry.id);

        store.set_pinned(&workspace_id, &entry.id, true).unwrap();
        assert!(
            store
                .list(&HistoryQuery {
                    workspace_id: workspace_id.clone(),
                    search: String::new(),
                    limit: 20,
                    offset: 0,
                })
                .unwrap()[0]
                .pinned
        );

        store.delete(&workspace_id, &entry.id).unwrap();
        assert!(
            store
                .list(&HistoryQuery {
                    workspace_id,
                    search: String::new(),
                    limit: 20,
                    offset: 0,
                })
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn redacts_known_credentials_but_preserves_variable_references() {
        let sanitized = sanitize_request(&request());
        assert!(sanitized.url.contains("api_key=%5BREDACTED%5D"));
        assert_eq!(sanitized.headers[0].value, "[REDACTED]");
        assert_eq!(
            sanitized.auth,
            RequestAuth::Bearer {
                token: "{{accessToken}}".to_owned()
            }
        );
        let RequestBody::FormUrlEncoded { fields } = sanitized.body else {
            panic!("expected form body")
        };
        assert_eq!(fields[0].value, "[REDACTED]");
    }

    #[test]
    fn omits_oversized_text_bodies() {
        let mut oversized = request();
        oversized.body = RequestBody::Text {
            content: "x".repeat(MAX_HISTORY_REQUEST_BYTES + 1),
            content_type: Some("text/plain".to_owned()),
        };
        let (snapshot, _, omitted) = prepare_request_snapshot(&oversized).unwrap();
        assert!(omitted);
        assert_eq!(snapshot.body, RequestBody::Empty);
    }
}
