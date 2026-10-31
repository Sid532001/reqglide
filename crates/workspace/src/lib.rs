//! Versioned, Git-friendly workspace documents and safe local persistence.

use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use api_client_core::{HeaderEntry, RequestAuth, RequestDefinition};
use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const CURRENT_SCHEMA_VERSION: u32 = 7;
pub const WORKSPACE_FILE_NAME: &str = "workspace.yaml";
const MAX_WORKSPACE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NAME_CHARS: usize = 120;
const MAX_TREE_DEPTH: usize = 32;
const MAX_NODES: usize = 100_000;
const MAX_SCRIPT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceDocument {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub collections: Vec<Collection>,
    #[serde(default)]
    pub environments: Vec<Environment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocol_profiles: Vec<ProtocolProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<RequestAuth>,
    #[serde(default, skip_serializing_if = "NetworkSettings::is_default")]
    pub network: NetworkSettings,
}

impl WorkspaceDocument {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            collections: vec![Collection::new("My Collection")],
            environments: Vec::new(),
            protocol_profiles: Vec::new(),
            auth: None,
            network: NetworkSettings::default(),
        }
    }

    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(WorkspaceError::UnsupportedSchemaVersion {
                found: self.schema_version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }

        validate_name("workspace", &self.name)?;

        let mut ids = HashSet::new();
        validate_id("workspace", &self.id, &mut ids)?;

        let mut node_count = 1usize;
        for collection in &self.collections {
            validate_collection(collection, &mut ids, &mut node_count)?;
        }
        for environment in &self.environments {
            validate_environment(environment, &mut ids, &mut node_count)?;
        }
        for profile in &self.protocol_profiles {
            validate_protocol_profile(profile, &mut ids, &mut node_count)?;
        }
        validate_parent_auth("workspace", self.auth.as_ref())?;
        self.network.validate()?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkSettings {
    #[serde(default = "enabled_by_default")]
    pub cookie_jar_enabled: bool,
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proxy_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proxy_username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proxy_password: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub no_proxy: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ca_certificate_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_certificate_path: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub accept_invalid_certificates: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            cookie_jar_enabled: true,
            proxy_mode: ProxyMode::System,
            proxy_url: String::new(),
            proxy_username: String::new(),
            proxy_password: String::new(),
            no_proxy: String::new(),
            ca_certificate_path: String::new(),
            client_certificate_path: String::new(),
            accept_invalid_certificates: false,
        }
    }
}

impl NetworkSettings {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }

    fn validate(&self) -> Result<(), WorkspaceError> {
        for (label, value, maximum) in [
            ("proxy URL", self.proxy_url.as_str(), 8_192usize),
            ("proxy username", self.proxy_username.as_str(), 4_096),
            ("proxy password", self.proxy_password.as_str(), 65_536),
            ("no-proxy rules", self.no_proxy.as_str(), 65_536),
            (
                "CA certificate path",
                self.ca_certificate_path.as_str(),
                4_096,
            ),
            (
                "client certificate path",
                self.client_certificate_path.as_str(),
                4_096,
            ),
        ] {
            if value.len() > maximum {
                return Err(WorkspaceError::InvalidDocument(format!(
                    "{label} exceeds the {maximum}-byte limit"
                )));
            }
        }
        if self.proxy_mode == ProxyMode::Custom && self.proxy_url.trim().is_empty() {
            return Err(WorkspaceError::InvalidDocument(
                "custom proxy mode requires a proxy URL".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProxyMode {
    #[default]
    System,
    Disabled,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<RequestAuth>,
    #[serde(default)]
    pub folders: Vec<RequestFolder>,
    #[serde(default)]
    pub requests: Vec<SavedRequest>,
}

impl Collection {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            auth: None,
            folders: Vec::new(),
            requests: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestFolder {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<RequestAuth>,
    #[serde(default)]
    pub folders: Vec<RequestFolder>,
    #[serde(default)]
    pub requests: Vec<SavedRequest>,
}

impl RequestFolder {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            auth: None,
            folders: Vec::new(),
            requests: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedRequest {
    pub id: String,
    pub name: String,
    pub request: RequestDefinition,
    #[serde(default)]
    pub scripts: RequestScripts,
}

impl SavedRequest {
    #[must_use]
    pub fn new(name: impl Into<String>, request: RequestDefinition) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            request,
            scripts: RequestScripts::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestScripts {
    #[serde(default)]
    pub pre_request: String,
    #[serde(default)]
    pub post_response: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub variables: Vec<EnvironmentVariable>,
}

impl Environment {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            variables: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentVariable {
    pub id: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolProfile {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<HeaderEntry>,
    #[serde(default)]
    pub auth: RequestAuth,
    pub configuration: ProtocolConfiguration,
}

impl ProtocolProfile {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        url: impl Into<String>,
        configuration: ProtocolConfiguration,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            url: url.into(),
            headers: Vec::new(),
            auth: RequestAuth::None,
            configuration,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProtocolConfiguration {
    GraphQl {
        query: String,
        #[serde(default = "empty_json_object")]
        variables: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        operation_name: String,
    },
    WebSocket {
        #[serde(default)]
        subprotocols: Vec<String>,
    },
    ServerSentEvents {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        last_event_id: String,
    },
    Grpc {
        service: String,
        method: String,
        #[serde(default = "empty_json_object")]
        message: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        descriptor_set_path: String,
        #[serde(default = "enabled_by_default")]
        use_reflection: bool,
    },
}

fn empty_json_object() -> String {
    "{}".to_owned()
}

impl EnvironmentVariable {
    #[must_use]
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            key: key.into(),
            value: value.into(),
            enabled: true,
            secret: false,
            local: false,
        }
    }
}

const fn enabled_by_default() -> bool {
    true
}

const fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSession {
    pub root_path: String,
    pub workspace: WorkspaceDocument,
}

pub struct WorkspaceStore;

impl WorkspaceStore {
    pub fn create(parent: &Path, name: &str) -> Result<WorkspaceSession, WorkspaceError> {
        validate_name("workspace", name)?;
        Self::create_document(parent, WorkspaceDocument::new(name.trim()))
    }

    /// Restore into a new directory and credential namespace. Never overwrite
    /// an existing workspace or attach another workspace's device credentials.
    pub fn restore(
        parent: &Path,
        backup: &WorkspaceDocument,
    ) -> Result<WorkspaceSession, WorkspaceError> {
        let mut document = backup.clone();
        document.id = Uuid::new_v4().to_string();
        Self::create_document(parent, document)
    }

    fn create_document(
        parent: &Path,
        document: WorkspaceDocument,
    ) -> Result<WorkspaceSession, WorkspaceError> {
        document.validate()?;
        let parent = canonical_directory(parent, "workspace parent")?;
        let root = parent.join(workspace_directory_name(&document.name));

        if root.exists() {
            return Err(WorkspaceError::DestinationExists(root));
        }

        fs::create_dir(&root).map_err(|source| WorkspaceError::Io {
            operation: "create workspace directory",
            path: root.clone(),
            source,
        })?;

        if let Err(error) = write_document(&root, &document) {
            let _ = fs::remove_dir(&root);
            return Err(error);
        }

        session(root, document)
    }

    pub fn load(root: &Path) -> Result<WorkspaceSession, WorkspaceError> {
        let root = canonical_directory(root, "workspace")?;
        let document = read_document(&root)?;
        session(root, document)
    }

    pub fn save(
        root: &Path,
        expected: &WorkspaceDocument,
        document: &WorkspaceDocument,
    ) -> Result<WorkspaceSession, WorkspaceError> {
        let root = canonical_directory(root, "workspace")?;
        let existing = read_document(&root)?;

        if existing.id != expected.id || document.id != expected.id {
            return Err(WorkspaceError::WorkspaceIdentityChanged);
        }

        if existing != *expected {
            return Err(WorkspaceError::ConcurrentModification);
        }

        write_document(&root, document)?;
        session(root, document.clone())
    }
}

fn read_document(root: &Path) -> Result<WorkspaceDocument, WorkspaceError> {
    let path = root.join(WORKSPACE_FILE_NAME);
    let metadata = fs::metadata(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            WorkspaceError::ManifestNotFound(path.clone())
        } else {
            WorkspaceError::Io {
                operation: "inspect workspace file",
                path: path.clone(),
                source,
            }
        }
    })?;

    if !metadata.is_file() {
        return Err(WorkspaceError::ManifestNotFound(path));
    }

    if metadata.len() > MAX_WORKSPACE_BYTES {
        return Err(WorkspaceError::WorkspaceTooLarge {
            actual: metadata.len(),
            maximum: MAX_WORKSPACE_BYTES,
        });
    }

    let yaml = fs::read_to_string(&path).map_err(|source| WorkspaceError::Io {
        operation: "read workspace file",
        path: path.clone(),
        source,
    })?;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SchemaHeader {
        schema_version: u32,
    }

    let header: SchemaHeader = serde_yaml_ng::from_str(&yaml)
        .map_err(|source| WorkspaceError::InvalidYaml(source.to_string()))?;

    if !matches!(
        header.schema_version,
        1 | 2 | 3 | 4 | 5 | 6 | CURRENT_SCHEMA_VERSION
    ) {
        return Err(WorkspaceError::UnsupportedSchemaVersion {
            found: header.schema_version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    let mut document: WorkspaceDocument = serde_yaml_ng::from_str(&yaml)
        .map_err(|source| WorkspaceError::InvalidYaml(source.to_string()))?;
    if document.schema_version < CURRENT_SCHEMA_VERSION {
        document.schema_version = CURRENT_SCHEMA_VERSION;
    }
    document.validate()?;
    Ok(document)
}

fn write_document(root: &Path, document: &WorkspaceDocument) -> Result<(), WorkspaceError> {
    document.validate()?;
    let path = root.join(WORKSPACE_FILE_NAME);
    let yaml = serde_yaml_ng::to_string(document)
        .map_err(|source| WorkspaceError::Serialize(source.to_string()))?;

    if yaml.len() as u64 > MAX_WORKSPACE_BYTES {
        return Err(WorkspaceError::WorkspaceTooLarge {
            actual: yaml.len() as u64,
            maximum: MAX_WORKSPACE_BYTES,
        });
    }

    let mut file = AtomicWriteFile::options()
        .open(&path)
        .map_err(|source| WorkspaceError::Io {
            operation: "prepare workspace file",
            path: path.clone(),
            source,
        })?;

    file.write_all(b"# Reqglide workspace. Review before storing in Git.\n")
        .and_then(|()| file.write_all(yaml.as_bytes()))
        .map_err(|source| WorkspaceError::Io {
            operation: "write workspace file",
            path: path.clone(),
            source,
        })?;

    file.commit().map_err(|source| WorkspaceError::Io {
        operation: "commit workspace file",
        path,
        source,
    })?;

    Ok(())
}

fn session(
    root: PathBuf,
    workspace: WorkspaceDocument,
) -> Result<WorkspaceSession, WorkspaceError> {
    Ok(WorkspaceSession {
        root_path: root.to_str().ok_or(WorkspaceError::NonUtf8Path)?.to_owned(),
        workspace,
    })
}

fn canonical_directory(path: &Path, label: &'static str) -> Result<PathBuf, WorkspaceError> {
    let canonical = path.canonicalize().map_err(|source| WorkspaceError::Io {
        operation: "resolve directory",
        path: path.to_path_buf(),
        source,
    })?;

    if !canonical.is_dir() {
        return Err(WorkspaceError::NotDirectory {
            label,
            path: canonical,
        });
    }

    Ok(canonical)
}

fn workspace_directory_name(name: &str) -> String {
    let mut slug = String::new();
    let mut previous_was_separator = false;

    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator && !slug.is_empty() {
            slug.push('-');
            previous_was_separator = true;
        }

        if slug.len() >= 64 {
            break;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        slug.push_str("api-workspace");
    }

    const WINDOWS_RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if WINDOWS_RESERVED.contains(&slug.as_str()) {
        slug.push_str("-workspace");
    }

    slug
}

fn validate_collection(
    collection: &Collection,
    ids: &mut HashSet<String>,
    node_count: &mut usize,
) -> Result<(), WorkspaceError> {
    bump_node_count(node_count)?;
    validate_id("collection", &collection.id, ids)?;
    validate_name("collection", &collection.name)?;
    validate_parent_auth("collection", collection.auth.as_ref())?;

    for request in &collection.requests {
        validate_saved_request(request, ids, node_count)?;
    }
    for folder in &collection.folders {
        validate_folder(folder, ids, node_count, 1)?;
    }
    Ok(())
}

fn validate_folder(
    folder: &RequestFolder,
    ids: &mut HashSet<String>,
    node_count: &mut usize,
    depth: usize,
) -> Result<(), WorkspaceError> {
    if depth > MAX_TREE_DEPTH {
        return Err(WorkspaceError::TreeTooDeep(MAX_TREE_DEPTH));
    }

    bump_node_count(node_count)?;
    validate_id("folder", &folder.id, ids)?;
    validate_name("folder", &folder.name)?;
    validate_parent_auth("folder", folder.auth.as_ref())?;

    for request in &folder.requests {
        validate_saved_request(request, ids, node_count)?;
    }
    for child in &folder.folders {
        validate_folder(child, ids, node_count, depth + 1)?;
    }
    Ok(())
}

fn validate_parent_auth(label: &str, auth: Option<&RequestAuth>) -> Result<(), WorkspaceError> {
    if matches!(auth, Some(RequestAuth::Inherit)) {
        return Err(WorkspaceError::InvalidDocument(format!(
            "{label} authentication must be omitted to inherit from its parent"
        )));
    }
    Ok(())
}

fn validate_saved_request(
    request: &SavedRequest,
    ids: &mut HashSet<String>,
    node_count: &mut usize,
) -> Result<(), WorkspaceError> {
    bump_node_count(node_count)?;
    validate_id("request", &request.id, ids)?;
    validate_name("request", &request.name)?;

    if request.request.method.chars().count() > 32 {
        return Err(WorkspaceError::InvalidDocument(
            "request method is longer than 32 characters".to_owned(),
        ));
    }
    if request.request.url.chars().count() > 16_384 {
        return Err(WorkspaceError::InvalidDocument(
            "request URL is longer than 16,384 characters".to_owned(),
        ));
    }
    if request.request.headers.len() > 4_096 {
        return Err(WorkspaceError::InvalidDocument(
            "request contains more than 4,096 headers".to_owned(),
        ));
    }
    if request.scripts.pre_request.len() > MAX_SCRIPT_BYTES
        || request.scripts.post_response.len() > MAX_SCRIPT_BYTES
    {
        return Err(WorkspaceError::InvalidDocument(
            "a request script is larger than 1 MiB".to_owned(),
        ));
    }
    Ok(())
}

fn validate_environment(
    environment: &Environment,
    ids: &mut HashSet<String>,
    node_count: &mut usize,
) -> Result<(), WorkspaceError> {
    bump_node_count(node_count)?;
    validate_id("environment", &environment.id, ids)?;
    validate_name("environment", &environment.name)?;

    let mut keys = HashSet::new();
    for variable in &environment.variables {
        bump_node_count(node_count)?;
        validate_id("environment variable", &variable.id, ids)?;
        validate_name("environment variable", &variable.key)?;
        if !keys.insert(variable.key.clone()) {
            return Err(WorkspaceError::InvalidDocument(format!(
                "environment '{}' contains duplicate variable '{}'",
                environment.name, variable.key
            )));
        }
        if variable.secret && variable.local {
            return Err(WorkspaceError::InvalidDocument(format!(
                "environment variable '{}' cannot be both secret and local-only",
                variable.key
            )));
        }
        if (variable.secret || variable.local) && !variable.value.is_empty() {
            return Err(WorkspaceError::InvalidDocument(format!(
                "vault-backed environment variable '{}' must not contain a plaintext value",
                variable.key
            )));
        }
    }
    Ok(())
}

fn validate_protocol_profile(
    profile: &ProtocolProfile,
    ids: &mut HashSet<String>,
    node_count: &mut usize,
) -> Result<(), WorkspaceError> {
    bump_node_count(node_count)?;
    validate_id("protocol profile", &profile.id, ids)?;
    validate_name("protocol profile", &profile.name)?;
    if profile.url.chars().count() > 16_384 {
        return Err(WorkspaceError::InvalidDocument(
            "protocol profile URL is longer than 16,384 characters".to_owned(),
        ));
    }
    if profile.headers.len() > 4_096 {
        return Err(WorkspaceError::InvalidDocument(
            "protocol profile contains more than 4,096 headers".to_owned(),
        ));
    }
    const MAX_PROTOCOL_TEXT_BYTES: usize = 4 * 1024 * 1024;
    let check_text = |label: &str, value: &str| {
        if value.len() > MAX_PROTOCOL_TEXT_BYTES {
            Err(WorkspaceError::InvalidDocument(format!(
                "protocol profile {label} exceeds the 4 MiB limit"
            )))
        } else {
            Ok(())
        }
    };
    match &profile.configuration {
        ProtocolConfiguration::GraphQl {
            query,
            variables,
            operation_name,
        } => {
            check_text("query", query)?;
            check_text("variables", variables)?;
            if operation_name.len() > 1_024 {
                return Err(WorkspaceError::InvalidDocument(
                    "GraphQL operation name exceeds the 1,024-byte limit".to_owned(),
                ));
            }
        }
        ProtocolConfiguration::WebSocket { subprotocols } => {
            if subprotocols.len() > 64
                || subprotocols
                    .iter()
                    .any(|protocol| protocol.is_empty() || protocol.len() > 256)
            {
                return Err(WorkspaceError::InvalidDocument(
                    "WebSocket subprotocols are invalid or exceed their limits".to_owned(),
                ));
            }
        }
        ProtocolConfiguration::ServerSentEvents { last_event_id } => {
            if last_event_id.len() > 8_192 {
                return Err(WorkspaceError::InvalidDocument(
                    "SSE last event ID exceeds the 8 KiB limit".to_owned(),
                ));
            }
        }
        ProtocolConfiguration::Grpc {
            service,
            method,
            message,
            descriptor_set_path,
            ..
        } => {
            if service.is_empty() || service.len() > 1_024 {
                return Err(WorkspaceError::InvalidDocument(
                    "gRPC service name is empty or exceeds the 1,024-byte limit".to_owned(),
                ));
            }
            if method.is_empty() || method.len() > 1_024 {
                return Err(WorkspaceError::InvalidDocument(
                    "gRPC method name is empty or exceeds the 1,024-byte limit".to_owned(),
                ));
            }
            check_text("message", message)?;
            if descriptor_set_path.len() > 4_096 {
                return Err(WorkspaceError::InvalidDocument(
                    "gRPC descriptor-set path exceeds the 4,096-byte limit".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn bump_node_count(node_count: &mut usize) -> Result<(), WorkspaceError> {
    *node_count += 1;
    if *node_count > MAX_NODES {
        return Err(WorkspaceError::TooManyNodes(MAX_NODES));
    }
    Ok(())
}

fn validate_name(kind: &'static str, name: &str) -> Result<(), WorkspaceError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(WorkspaceError::InvalidName {
            kind,
            reason: "name cannot be empty",
        });
    }
    if trimmed.chars().count() > MAX_NAME_CHARS {
        return Err(WorkspaceError::InvalidName {
            kind,
            reason: "name is longer than 120 characters",
        });
    }
    if trimmed.chars().any(char::is_control) {
        return Err(WorkspaceError::InvalidName {
            kind,
            reason: "name cannot contain control characters",
        });
    }
    Ok(())
}

fn validate_id(
    kind: &'static str,
    id: &str,
    ids: &mut HashSet<String>,
) -> Result<(), WorkspaceError> {
    Uuid::parse_str(id)
        .map_err(|_| WorkspaceError::InvalidDocument(format!("{kind} id is not a valid UUID")))?;

    if !ids.insert(id.to_owned()) {
        return Err(WorkspaceError::DuplicateId(id.to_owned()));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("invalid {kind} name: {reason}")]
    InvalidName {
        kind: &'static str,
        reason: &'static str,
    },
    #[error("workspace destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("{label} path is not a directory: {path}")]
    NotDirectory { label: &'static str, path: PathBuf },
    #[error("no {WORKSPACE_FILE_NAME} was found in {0}")]
    ManifestNotFound(PathBuf),
    #[error(
        "workspace schema version {found} is not supported; this build supports version {supported}"
    )]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    #[error("workspace file is {actual} bytes; maximum supported size is {maximum} bytes")]
    WorkspaceTooLarge { actual: u64, maximum: u64 },
    #[error("workspace YAML is invalid: {0}")]
    InvalidYaml(String),
    #[error("could not serialize workspace: {0}")]
    Serialize(String),
    #[error("invalid workspace document: {0}")]
    InvalidDocument(String),
    #[error("duplicate workspace node id: {0}")]
    DuplicateId(String),
    #[error("workspace hierarchy exceeds the maximum depth of {0}")]
    TreeTooDeep(usize),
    #[error("workspace contains more than {0} nodes")]
    TooManyNodes(usize),
    #[error("workspace identity changed while it was open")]
    WorkspaceIdentityChanged,
    #[error("workspace changed on disk; reopen it before saving to avoid losing external edits")]
    ConcurrentModification,
    #[error("workspace paths must be valid UTF-8")]
    NonUtf8Path,
    #[error("failed to {operation} at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client_core::{RequestBody, RequestOptions};
    use tempfile::tempdir;

    fn sample_request() -> RequestDefinition {
        RequestDefinition {
            method: "POST".to_owned(),
            url: "https://example.com/users".to_owned(),
            query_params: Vec::new(),
            headers: Vec::new(),
            auth: api_client_core::RequestAuth::None,
            body: RequestBody::Text {
                content: "{\"name\":\"Ada\"}".to_owned(),
                content_type: Some("application/json".to_owned()),
            },
            options: RequestOptions::default(),
        }
    }

    #[test]
    fn creates_saves_and_loads_a_workspace() {
        let parent = tempdir().unwrap();
        let created = WorkspaceStore::create(parent.path(), "Payments API").unwrap();
        let root = PathBuf::from(&created.root_path);

        assert!(root.ends_with("payments-api"));
        assert_eq!(created.workspace.collections.len(), 1);

        let mut changed = created.workspace.clone();
        changed.collections[0]
            .requests
            .push(SavedRequest::new("Create user", sample_request()));

        WorkspaceStore::save(&root, &created.workspace, &changed).unwrap();
        let loaded = WorkspaceStore::load(&root).unwrap();

        assert_eq!(loaded.workspace, changed);
        let yaml = fs::read_to_string(root.join(WORKSPACE_FILE_NAME)).unwrap();
        assert!(yaml.starts_with("# Reqglide workspace"));
        assert!(yaml.contains("schemaVersion: 7"));
        assert!(yaml.contains("name: Create user"));
    }

    #[test]
    fn refuses_to_replace_an_existing_destination() {
        let parent = tempdir().unwrap();
        WorkspaceStore::create(parent.path(), "Team APIs").unwrap();

        let error = WorkspaceStore::create(parent.path(), "Team APIs").unwrap_err();
        assert!(matches!(error, WorkspaceError::DestinationExists(_)));
    }

    #[test]
    fn rejects_duplicate_node_ids_before_writing() {
        let parent = tempdir().unwrap();
        let created = WorkspaceStore::create(parent.path(), "Validation").unwrap();
        let root = PathBuf::from(&created.root_path);
        let original = fs::read_to_string(root.join(WORKSPACE_FILE_NAME)).unwrap();

        let mut invalid = created.workspace.clone();
        let duplicate = invalid.collections[0].id.clone();
        invalid.collections.push(Collection {
            id: duplicate,
            name: "Duplicate".to_owned(),
            auth: None,
            folders: Vec::new(),
            requests: Vec::new(),
        });

        let error = WorkspaceStore::save(&root, &created.workspace, &invalid).unwrap_err();
        assert!(matches!(error, WorkspaceError::DuplicateId(_)));
        assert_eq!(
            fs::read_to_string(root.join(WORKSPACE_FILE_NAME)).unwrap(),
            original
        );
    }

    #[test]
    fn rejects_unknown_schema_versions_before_full_deserialization() {
        let root = tempdir().unwrap();
        fs::write(
            root.path().join(WORKSPACE_FILE_NAME),
            "schemaVersion: 99\nid: ignored\nname: Future\ncollections: []\n",
        )
        .unwrap();

        let error = WorkspaceStore::load(root.path()).unwrap_err();
        assert!(matches!(
            error,
            WorkspaceError::UnsupportedSchemaVersion {
                found: 99,
                supported: 7
            }
        ));
    }

    #[test]
    fn migrates_older_documents_in_memory() {
        let root = tempdir().unwrap();
        fs::write(
            root.path().join(WORKSPACE_FILE_NAME),
            "schemaVersion: 1\nid: 58860d30-7d2b-4887-bc21-2b3e16eb213b\nname: Legacy\ncollections: []\n",
        )
        .unwrap();

        let migrated = WorkspaceStore::load(root.path()).unwrap();
        assert_eq!(migrated.workspace.schema_version, 7);
        assert!(migrated.workspace.environments.is_empty());

        fs::write(
            root.path().join(WORKSPACE_FILE_NAME),
            "schemaVersion: 2\nid: 58860d30-7d2b-4887-bc21-2b3e16eb213b\nname: Legacy\ncollections: []\nenvironments: []\n",
        )
        .unwrap();
        let migrated = WorkspaceStore::load(root.path()).unwrap();
        assert_eq!(migrated.workspace.schema_version, 7);

        fs::write(
            root.path().join(WORKSPACE_FILE_NAME),
            "schemaVersion: 3\nid: 58860d30-7d2b-4887-bc21-2b3e16eb213b\nname: Legacy\ncollections: []\nenvironments: []\n",
        )
        .unwrap();
        let migrated = WorkspaceStore::load(root.path()).unwrap();
        assert_eq!(migrated.workspace.schema_version, 7);

        fs::write(
            root.path().join(WORKSPACE_FILE_NAME),
            "schemaVersion: 4\nid: 58860d30-7d2b-4887-bc21-2b3e16eb213b\nname: Legacy\ncollections: []\nenvironments: []\n",
        )
        .unwrap();
        let migrated = WorkspaceStore::load(root.path()).unwrap();
        assert_eq!(migrated.workspace.schema_version, 7);
        assert_eq!(migrated.workspace.network, NetworkSettings::default());

        fs::write(
            root.path().join(WORKSPACE_FILE_NAME),
            "schemaVersion: 5\nid: 58860d30-7d2b-4887-bc21-2b3e16eb213b\nname: Legacy\ncollections: []\nenvironments: []\n",
        )
        .unwrap();
        let migrated = WorkspaceStore::load(root.path()).unwrap();
        assert_eq!(migrated.workspace.schema_version, 7);
        assert!(migrated.workspace.auth.is_none());

        fs::write(
            root.path().join(WORKSPACE_FILE_NAME),
            "schemaVersion: 6\nid: 58860d30-7d2b-4887-bc21-2b3e16eb213b\nname: Legacy\ncollections: []\nenvironments: []\n",
        )
        .unwrap();
        let migrated = WorkspaceStore::load(root.path()).unwrap();
        assert_eq!(migrated.workspace.schema_version, 7);
        assert!(migrated.workspace.protocol_profiles.is_empty());
    }

    #[test]
    fn secret_environment_values_are_references_only() {
        let mut workspace = WorkspaceDocument::new("Secrets");
        let mut environment = Environment::new("Development");
        let mut variable = EnvironmentVariable::new("accessToken", "");
        variable.secret = true;
        environment.variables.push(variable.clone());
        workspace.environments.push(environment);
        assert!(workspace.validate().is_ok());

        let yaml = serde_yaml_ng::to_string(&workspace).unwrap();
        assert!(yaml.contains("secret: true"));
        assert!(!yaml.contains("value:"));

        workspace.environments[0].variables[0].value = "must-not-be-written".to_owned();
        assert!(matches!(
            workspace.validate(),
            Err(WorkspaceError::InvalidDocument(message))
                if message.contains("must not contain a plaintext value")
        ));

        workspace.environments[0].variables[0].value.clear();
        workspace.environments[0].variables[0].secret = false;
        workspace.environments[0].variables[0].local = true;
        assert!(workspace.validate().is_ok());
        let yaml = serde_yaml_ng::to_string(&workspace).unwrap();
        assert!(yaml.contains("local: true"));
        assert!(!yaml.contains("value:"));

        workspace.environments[0].variables[0].secret = true;
        assert!(matches!(
            workspace.validate(),
            Err(WorkspaceError::InvalidDocument(message))
                if message.contains("cannot be both secret and local-only")
        ));
    }

    #[test]
    fn validates_and_serializes_non_default_network_settings() {
        let mut workspace = WorkspaceDocument::new("Network");
        let default_yaml = serde_yaml_ng::to_string(&workspace).unwrap();
        assert!(!default_yaml.contains("network:"));

        workspace.network.proxy_mode = ProxyMode::Custom;
        assert!(matches!(
            workspace.validate(),
            Err(WorkspaceError::InvalidDocument(message))
                if message.contains("requires a proxy URL")
        ));

        workspace.network.proxy_url = "http://{{proxyHost}}:8080".to_owned();
        workspace.network.proxy_password = "{{proxyPassword}}".to_owned();
        workspace.network.cookie_jar_enabled = false;
        assert!(workspace.validate().is_ok());

        let yaml = serde_yaml_ng::to_string(&workspace).unwrap();
        assert!(yaml.contains("network:"));
        assert!(yaml.contains("proxyMode: custom"));
        assert!(yaml.contains("proxyPassword: '{{proxyPassword}}'"));
        assert!(yaml.contains("cookieJarEnabled: false"));
    }

    #[test]
    fn persists_parent_auth_policies_and_rejects_redundant_inherit_values() {
        let mut workspace = WorkspaceDocument::new("Auth policies");
        workspace.auth = Some(RequestAuth::Bearer {
            token: "{{workspaceToken}}".to_owned(),
        });
        workspace.collections[0].auth = Some(RequestAuth::None);
        let mut folder = RequestFolder::new("Inherited folder");
        folder.auth = Some(RequestAuth::ApiKey {
            key: "X-API-Key".to_owned(),
            value: "{{apiKey}}".to_owned(),
            placement: api_client_core::ApiKeyPlacement::Header,
        });
        workspace.collections[0].folders.push(folder);

        assert!(workspace.validate().is_ok());
        let yaml = serde_yaml_ng::to_string(&workspace).unwrap();
        assert!(yaml.contains("workspaceToken"));
        assert!(yaml.contains("kind: apiKey"));

        workspace.collections[0].auth = Some(RequestAuth::Inherit);
        assert!(matches!(
            workspace.validate(),
            Err(WorkspaceError::InvalidDocument(message))
                if message.contains("must be omitted to inherit")
        ));
    }

    #[test]
    fn refuses_to_overwrite_external_changes() {
        let parent = tempdir().unwrap();
        let created = WorkspaceStore::create(parent.path(), "Concurrent edits").unwrap();
        let root = PathBuf::from(&created.root_path);

        let mut external = created.workspace.clone();
        external.name = "Changed outside the app".to_owned();
        write_document(&root, &external).unwrap();

        let mut local = created.workspace.clone();
        local.name = "Changed inside the app".to_owned();
        let error = WorkspaceStore::save(&root, &created.workspace, &local).unwrap_err();

        assert!(matches!(error, WorkspaceError::ConcurrentModification));
        assert_eq!(WorkspaceStore::load(&root).unwrap().workspace, external);
    }

    #[test]
    fn generates_portable_workspace_directory_names() {
        assert_eq!(
            workspace_directory_name("  Core / Payments API  "),
            "core-payments-api"
        );
        assert_eq!(workspace_directory_name("CON"), "con-workspace");
        assert_eq!(workspace_directory_name("東京"), "api-workspace");
    }
}
