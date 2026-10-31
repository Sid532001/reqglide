use api_client_workspace::{ProxyMode, WorkspaceDocument};
use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const MAX_BACKUP_BYTES: usize = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceBackup {
    schema_version: u32,
    application_version: String,
    workspace: WorkspaceDocument,
}

pub(super) fn encode_backup(workspace: &WorkspaceDocument) -> Result<Vec<u8>, String> {
    workspace.validate().map_err(|error| error.to_string())?;
    let payload = serde_json::to_vec_pretty(&WorkspaceBackup {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_owned(),
        workspace: workspace.clone(),
    })
    .map_err(|_| "Could not encode the workspace backup.".to_owned())?;
    if payload.len() > MAX_BACKUP_BYTES {
        return Err("Workspace backup exceeds the 32 MiB safety limit.".to_owned());
    }
    Ok(payload)
}

pub(super) fn read_backup(path: &Path) -> Result<WorkspaceDocument, String> {
    let file =
        fs::File::open(path).map_err(|_| "Could not open the selected backup.".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "Could not inspect the selected backup.".to_owned())?;
    if !metadata.is_file() || metadata.len() > MAX_BACKUP_BYTES as u64 {
        return Err("Choose a regular workspace backup file no larger than 32 MiB.".to_owned());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BACKUP_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read the selected backup.".to_owned())?;
    if bytes.len() > MAX_BACKUP_BYTES {
        return Err("Workspace backup exceeds the 32 MiB safety limit.".to_owned());
    }
    let backup: WorkspaceBackup = serde_json::from_slice(&bytes)
        .map_err(|_| "This file is not a valid workspace backup.".to_owned())?;
    if backup.schema_version != 1 {
        return Err("This workspace backup version is not supported.".to_owned());
    }
    backup
        .workspace
        .validate()
        .map_err(|_| "The backup contains invalid workspace data.".to_owned())?;
    Ok(backup.workspace)
}

pub(super) fn write_json_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Err("Choose a .json filename for this export.".to_owned());
    }
    let mut file = AtomicWriteFile::options()
        .open(path)
        .map_err(|_| "Could not prepare the export file.".to_owned())?;
    file.write_all(bytes)
        .map_err(|_| "Could not write the export file.".to_owned())?;
    file.commit()
        .map_err(|_| "Could not commit the export file.".to_owned())
}

// This allowlist deliberately has no request strings, names, paths, environment
// values, headers, script text, response data, errors, history, cookies, or vault access.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostics {
    schema_version: u32,
    application_version: &'static str,
    operating_system: &'static str,
    architecture: &'static str,
    debug_build: bool,
    workspace: Option<WorkspaceDiagnostics>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceDiagnostics {
    collections: usize,
    environments: usize,
    protected_variables: usize,
    protocol_profiles: usize,
    proxy_mode: ProxyMode,
    custom_ca_configured: bool,
    client_identity_configured: bool,
    tls_verification_enabled: bool,
}

pub(super) fn encode_diagnostics(workspace: Option<&WorkspaceDocument>) -> Result<Vec<u8>, String> {
    let data = Diagnostics {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION"),
        operating_system: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        debug_build: cfg!(debug_assertions),
        workspace: workspace.map(|workspace| WorkspaceDiagnostics {
            collections: workspace.collections.len(),
            environments: workspace.environments.len(),
            protected_variables: workspace
                .environments
                .iter()
                .flat_map(|environment| &environment.variables)
                .filter(|variable| variable.secret || variable.local)
                .count(),
            protocol_profiles: workspace.protocol_profiles.len(),
            proxy_mode: workspace.network.proxy_mode,
            custom_ca_configured: !workspace.network.ca_certificate_path.trim().is_empty(),
            client_identity_configured: !workspace
                .network
                .client_certificate_path
                .trim()
                .is_empty(),
            tls_verification_enabled: !workspace.network.accept_invalid_certificates,
        }),
    };
    serde_json::to_vec_pretty(&data).map_err(|_| "Could not encode support diagnostics.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client_core::{RequestAuth, RequestBody, RequestDefinition, RequestOptions};
    use api_client_workspace::{
        Environment, EnvironmentVariable, RequestScripts, SavedRequest, WorkspaceStore,
    };

    fn workspace() -> WorkspaceDocument {
        let mut workspace = WorkspaceDocument::new("Private workspace name");
        let mut environment = Environment::new("Private environment name");
        let mut secret = EnvironmentVariable::new("access_token", "");
        secret.secret = true;
        environment.variables.push(secret);
        environment
            .variables
            .push(EnvironmentVariable::new("shared", "private-shared-value"));
        workspace.environments.push(environment);
        workspace.collections[0].requests.push(SavedRequest {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Private request name".to_owned(),
            request: RequestDefinition {
                method: "POST".to_owned(),
                url: "https://private.example.test/path".to_owned(),
                query_params: Vec::new(),
                headers: Vec::new(),
                auth: RequestAuth::Bearer {
                    token: "literal-private-token".to_owned(),
                },
                body: RequestBody::Text {
                    content: "private-body-data".to_owned(),
                    content_type: Some("text/plain".to_owned()),
                },
                options: RequestOptions::default(),
            },
            scripts: RequestScripts {
                pre_request: "console.log('private-script');".to_owned(),
                post_response: String::new(),
            },
        });
        workspace.network.proxy_password = "private-proxy-password".to_owned();
        workspace.network.ca_certificate_path = "C:/private/path/certificate.pem".to_owned();
        workspace
    }

    #[test]
    fn backup_round_trip_preserves_saved_data_and_restores_into_a_new_identity() {
        let source = workspace();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace.apic-backup.json");
        let payload = encode_backup(&source).unwrap();
        write_json_atomically(&path, &payload).unwrap();
        let decoded = read_backup(&path).unwrap();
        assert_eq!(decoded, source);
        let restored = WorkspaceStore::restore(temp.path(), &decoded).unwrap();
        assert_ne!(restored.workspace.id, source.id);
        assert_eq!(restored.workspace.collections, source.collections);
        assert_eq!(restored.workspace.environments, source.environments);
        assert!(
            restored.workspace.environments[0].variables[0]
                .value
                .is_empty()
        );
        assert_eq!(
            WorkspaceStore::load(Path::new(&restored.root_path))
                .unwrap()
                .workspace,
            restored.workspace
        );
        assert!(WorkspaceStore::restore(temp.path(), &decoded).is_err());
        assert_eq!(
            WorkspaceStore::load(Path::new(&restored.root_path))
                .unwrap()
                .workspace,
            restored.workspace
        );
    }

    #[test]
    fn invalid_and_oversized_backups_are_rejected_before_restore() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("invalid.json");
        let mut unsupported: serde_json::Value =
            serde_json::from_slice(&encode_backup(&workspace()).unwrap()).unwrap();
        unsupported["schemaVersion"] = 2.into();
        fs::write(&path, serde_json::to_vec(&unsupported).unwrap()).unwrap();
        assert_eq!(
            read_backup(&path).unwrap_err(),
            "This workspace backup version is not supported."
        );
        unsupported["schemaVersion"] = 1.into();
        unsupported["workspace"]["environments"][0]["variables"][0]["value"] =
            "embedded-secret-value".into();
        fs::write(&path, serde_json::to_vec(&unsupported).unwrap()).unwrap();
        assert_eq!(
            read_backup(&path).unwrap_err(),
            "The backup contains invalid workspace data."
        );
        fs::write(&path, b"invalid backup data").unwrap();
        assert_eq!(
            read_backup(&path).unwrap_err(),
            "This file is not a valid workspace backup."
        );
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_BACKUP_BYTES as u64 + 1).unwrap();
        assert!(read_backup(&path).is_err());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn diagnostics_omit_workspace_and_credential_strings_by_construction() {
        let data = String::from_utf8(encode_diagnostics(Some(&workspace())).unwrap()).unwrap();
        for private in [
            "Private workspace",
            "Private environment",
            "Private request",
            "private.example",
            "private-shared-value",
            "literal-private-token",
            "private-body-data",
            "private-script",
            "private-proxy-password",
            "C:/private",
        ] {
            assert!(!data.contains(private), "diagnostics leaked {private}");
        }
        let value: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(value["workspace"]["protectedVariables"], 1);
        assert_eq!(value["workspace"]["customCaConfigured"], true);
        assert!(
            serde_json::from_slice::<serde_json::Value>(&encode_diagnostics(None).unwrap())
                .unwrap()["workspace"]
                .is_null()
        );
    }
}
