use api_client_workspace::{WorkspaceSession, WorkspaceStore};
use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_SETTINGS_BYTES: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartupSettings {
    schema_version: u32,
    last_workspace: Option<WorkspaceLocation>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceLocation {
    root_path: PathBuf,
    workspace_id: String,
}

pub(super) fn remember(
    settings_path: &Path,
    root: &Path,
    workspace_id: &str,
) -> Result<(), String> {
    if !root.is_absolute() || Uuid::parse_str(workspace_id).is_err() {
        return Err("The open workspace location is invalid.".to_owned());
    }
    write(
        settings_path,
        Some(WorkspaceLocation {
            root_path: root.to_path_buf(),
            workspace_id: workspace_id.to_owned(),
        }),
    )
}

pub(super) fn forget(settings_path: &Path) -> Result<(), String> {
    write(settings_path, None)
}

fn write(settings_path: &Path, last_workspace: Option<WorkspaceLocation>) -> Result<(), String> {
    let payload = serde_json::to_vec(&StartupSettings {
        schema_version: 1,
        last_workspace,
    })
    .map_err(|_| "Could not encode the last workspace setting.".to_owned())?;
    if payload.len() as u64 > MAX_SETTINGS_BYTES {
        return Err("The workspace location is too long to remember.".to_owned());
    }
    let parent = settings_path
        .parent()
        .ok_or_else(|| "Local application storage is unavailable.".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|_| "Could not prepare local application storage.".to_owned())?;
    let mut file = AtomicWriteFile::options()
        .open(settings_path)
        .map_err(|_| "Could not save the last workspace setting.".to_owned())?;
    file.write_all(&payload)
        .map_err(|_| "Could not save the last workspace setting.".to_owned())?;
    file.commit()
        .map_err(|_| "Could not save the last workspace setting.".to_owned())
}

pub(super) fn reopen(settings_path: &Path) -> Result<Option<WorkspaceSession>, String> {
    let file = match fs::File::open(settings_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(
                "Could not read the last workspace setting. Use Open to choose a workspace."
                    .to_owned(),
            );
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            "Could not read the last workspace setting. Use Open to choose a workspace.".to_owned()
        })?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(
            "The last workspace setting is invalid. Use Open to choose a workspace.".to_owned(),
        );
    }
    let settings: StartupSettings = serde_json::from_slice(&bytes).map_err(|_| {
        "The last workspace setting is invalid. Use Open to choose a workspace.".to_owned()
    })?;
    if settings.schema_version != 1 {
        return Err(
            "The last workspace setting version is unsupported. Use Open to choose a workspace."
                .to_owned(),
        );
    }
    let Some(location) = settings.last_workspace else {
        return Ok(None);
    };
    if !location.root_path.is_absolute() || Uuid::parse_str(&location.workspace_id).is_err() {
        return Err(
            "The last workspace location is invalid. Use Open to choose a workspace.".to_owned(),
        );
    }
    let session = WorkspaceStore::load(&location.root_path).map_err(|_| {
        "Could not reopen the last workspace. Its folder may have moved or be unavailable. Use Open to choose a workspace.".to_owned()
    })?;
    if session.workspace.id != location.workspace_id {
        return Err(
            "The previous folder contains a different workspace. Use Open to choose it explicitly."
                .to_owned(),
        );
    }
    Ok(Some(session))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_a_location_and_reopens_saved_data_without_serializing_requests() {
        let temp = tempfile::tempdir().unwrap();
        let session = WorkspaceStore::create(temp.path(), "Last project").unwrap();
        let settings = temp.path().join("app/last-workspace.json");
        remember(
            &settings,
            Path::new(&session.root_path),
            &session.workspace.id,
        )
        .unwrap();
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
        assert_eq!(document["lastWorkspace"].as_object().unwrap().len(), 2);
        assert_eq!(
            document["lastWorkspace"]["workspaceId"],
            session.workspace.id
        );
        assert!(document.get("workspace").is_none());
        assert_eq!(reopen(&settings).unwrap().unwrap(), session);
    }

    #[test]
    fn missing_settings_and_explicit_close_leave_no_workspace_to_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("last-workspace.json");
        assert!(reopen(&settings).unwrap().is_none());
        let session = WorkspaceStore::create(temp.path(), "Keep my files").unwrap();
        remember(
            &settings,
            Path::new(&session.root_path),
            &session.workspace.id,
        )
        .unwrap();
        forget(&settings).unwrap();
        assert!(reopen(&settings).unwrap().is_none());
        assert_eq!(
            WorkspaceStore::load(Path::new(&session.root_path)).unwrap(),
            session
        );
    }

    #[test]
    fn later_workspace_replaces_the_previous_location() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("last-workspace.json");
        let first = WorkspaceStore::create(temp.path(), "First").unwrap();
        let second = WorkspaceStore::create(temp.path(), "Second").unwrap();
        remember(&settings, Path::new(&first.root_path), &first.workspace.id).unwrap();
        remember(
            &settings,
            Path::new(&second.root_path),
            &second.workspace.id,
        )
        .unwrap();
        assert_eq!(reopen(&settings).unwrap().unwrap(), second);
    }

    #[test]
    fn moved_and_replaced_workspaces_require_explicit_selection() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("last-workspace.json");
        let missing = temp.path().join("missing");
        remember(&settings, &missing, &Uuid::new_v4().to_string()).unwrap();
        assert!(reopen(&settings).unwrap_err().contains("Use Open"));
        let session = WorkspaceStore::create(temp.path(), "Different identity").unwrap();
        remember(
            &settings,
            Path::new(&session.root_path),
            &Uuid::new_v4().to_string(),
        )
        .unwrap();
        assert!(
            reopen(&settings)
                .unwrap_err()
                .contains("different workspace")
        );
        assert_eq!(
            WorkspaceStore::load(Path::new(&session.root_path)).unwrap(),
            session
        );
    }

    #[test]
    fn corrupt_or_oversized_preferences_do_not_expose_file_contents() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("last-workspace.json");
        fs::write(&settings, b"private-invalid-data").unwrap();
        assert!(
            !reopen(&settings)
                .unwrap_err()
                .contains("private-invalid-data")
        );
        fs::File::create(&settings)
            .unwrap()
            .set_len(MAX_SETTINGS_BYTES + 1)
            .unwrap();
        assert!(reopen(&settings).is_err());
        assert!(
            remember(
                &settings,
                Path::new("relative"),
                &Uuid::new_v4().to_string()
            )
            .is_err()
        );
    }
}
