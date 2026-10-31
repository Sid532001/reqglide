//! Narrow access to the operating system credential store.
//!
//! Workspace manifests contain only stable workspace/variable identifiers.
//! Secret values are stored under those identifiers in the native keychain.

use keyring::{Entry, Error as KeyringError};
use thiserror::Error;

const SERVICE_NAME: &str = "dev.localfirst.apiclient.environment";
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_SECRET_BYTES: usize = 1024 * 1024;

pub trait SecretStore: Send + Sync {
    fn get(&self, workspace_id: &str, variable_id: &str) -> Result<Option<String>, VaultError>;
    fn set(&self, workspace_id: &str, variable_id: &str, value: &str) -> Result<(), VaultError>;
    fn delete(&self, workspace_id: &str, variable_id: &str) -> Result<(), VaultError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NativeSecretStore;

impl SecretStore for NativeSecretStore {
    fn get(&self, workspace_id: &str, variable_id: &str) -> Result<Option<String>, VaultError> {
        let entry = native_entry(workspace_id, variable_id)?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(error) => Err(VaultError::Backend(error.to_string())),
        }
    }

    fn set(&self, workspace_id: &str, variable_id: &str, value: &str) -> Result<(), VaultError> {
        if value.len() > MAX_SECRET_BYTES {
            return Err(VaultError::SecretTooLarge {
                actual: value.len(),
                maximum: MAX_SECRET_BYTES,
            });
        }
        native_entry(workspace_id, variable_id)?
            .set_password(value)
            .map_err(|error| VaultError::Backend(error.to_string()))
    }

    fn delete(&self, workspace_id: &str, variable_id: &str) -> Result<(), VaultError> {
        match native_entry(workspace_id, variable_id)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => Err(VaultError::Backend(error.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretChange {
    pub variable_id: String,
    pub value: Option<String>,
}

pub struct AppliedSecretChanges<'a> {
    store: &'a dyn SecretStore,
    workspace_id: String,
    previous: Vec<SecretChange>,
}

impl AppliedSecretChanges<'_> {
    pub fn rollback(self) -> Result<(), VaultError> {
        restore_changes(self.store, &self.workspace_id, self.previous)
    }
}

/// Validates and snapshots the entire batch before changing the store.
///
/// Failed writes trigger restoration of all attempted changes. On success, the
/// returned handle can restore the batch explicitly; dropping it keeps changes.
pub fn apply_changes<'a>(
    store: &'a dyn SecretStore,
    workspace_id: &str,
    changes: &[SecretChange],
) -> Result<AppliedSecretChanges<'a>, VaultError> {
    validate_identifier("workspace", workspace_id)?;
    let mut previous = Vec::with_capacity(changes.len());

    for change in changes {
        validate_identifier("variable", &change.variable_id)?;
        previous.push(SecretChange {
            variable_id: change.variable_id.clone(),
            value: store.get(workspace_id, &change.variable_id)?,
        });
    }

    for (index, change) in changes.iter().enumerate() {
        let result = match &change.value {
            Some(value) => store.set(workspace_id, &change.variable_id, value),
            None => store.delete(workspace_id, &change.variable_id),
        };
        if let Err(operation_error) = result {
            // A backend can return an error after mutating the attempted entry.
            previous.truncate(index + 1);
            let rollback_error = restore_changes(store, workspace_id, previous).err();
            return Err(VaultError::Transaction {
                operation: operation_error.to_string(),
                rollback: rollback_error.map(|error| error.to_string()),
            });
        }
    }

    Ok(AppliedSecretChanges {
        store,
        workspace_id: workspace_id.to_owned(),
        previous,
    })
}

/// Applies secret changes, then runs fallible validation and persistence.
///
/// The closure observes the updated secrets. Returning `Ok` keeps the changes
/// and returns its value; returning `Err` restores the batch in reverse order.
/// Both the closure error and any rollback errors are retained as messages in
/// [`VaultError::Transaction`]. The closure is not called if applying changes
/// fails. Include every fallible validation read and the final save in it.
///
/// Callers must serialize access to the affected entries and handle any partial
/// side effects of their closure. Panics and process termination are not covered.
pub fn transact_changes<T, E>(
    store: &dyn SecretStore,
    workspace_id: &str,
    changes: &[SecretChange],
    commit: impl FnOnce() -> Result<T, E>,
) -> Result<T, VaultError>
where
    E: std::fmt::Display,
{
    let applied = apply_changes(store, workspace_id, changes)?;
    match commit() {
        Ok(value) => Ok(value),
        Err(error) => Err(VaultError::Transaction {
            operation: error.to_string(),
            rollback: applied.rollback().err().map(|error| error.to_string()),
        }),
    }
}

fn restore_changes(
    store: &dyn SecretStore,
    workspace_id: &str,
    previous: Vec<SecretChange>,
) -> Result<(), VaultError> {
    let mut failures = Vec::new();
    for change in previous.into_iter().rev() {
        let result = match change.value {
            Some(value) => store.set(workspace_id, &change.variable_id, &value),
            None => store.delete(workspace_id, &change.variable_id),
        };
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(VaultError::Rollback(failures.join("; ")))
    }
}

fn native_entry(workspace_id: &str, variable_id: &str) -> Result<Entry, VaultError> {
    validate_identifier("workspace", workspace_id)?;
    validate_identifier("variable", variable_id)?;
    Entry::new(SERVICE_NAME, &format!("{workspace_id}:{variable_id}"))
        .map_err(|error| VaultError::Backend(error.to_string()))
}

fn validate_identifier(kind: &'static str, value: &str) -> Result<(), VaultError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(VaultError::InvalidIdentifier(kind));
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VaultError {
    #[error("invalid {0} identifier for the secret vault")]
    InvalidIdentifier(&'static str),
    #[error("secret is {actual} bytes; maximum supported size is {maximum} bytes")]
    SecretTooLarge { actual: usize, maximum: usize },
    #[error("the operating system credential store is unavailable: {0}")]
    Backend(String),
    #[error("secret update failed: {operation}; rollback: {rollback:?}")]
    Transaction {
        operation: String,
        rollback: Option<String>,
    },
    #[error("secret rollback failed: {0}")]
    Rollback(String),
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use super::*;

    #[derive(Default, Clone)]
    struct MemoryStore {
        values: Arc<Mutex<BTreeMap<String, String>>>,
        fail_on_get: Arc<Mutex<Option<String>>>,
        fail_on_set: Arc<Mutex<Option<String>>>,
        fail_after_set: Arc<Mutex<Option<String>>>,
        fail_on_delete: Arc<Mutex<Option<String>>>,
    }

    impl MemoryStore {
        fn key(workspace_id: &str, variable_id: &str) -> String {
            format!("{workspace_id}:{variable_id}")
        }
    }

    impl SecretStore for MemoryStore {
        fn get(&self, workspace_id: &str, variable_id: &str) -> Result<Option<String>, VaultError> {
            if self.fail_on_get.lock().unwrap().as_deref() == Some(variable_id) {
                return Err(VaultError::Backend(format!(
                    "injected read failure for {variable_id}"
                )));
            }
            Ok(self
                .values
                .lock()
                .unwrap()
                .get(&Self::key(workspace_id, variable_id))
                .cloned())
        }

        fn set(
            &self,
            workspace_id: &str,
            variable_id: &str,
            value: &str,
        ) -> Result<(), VaultError> {
            if self.fail_on_set.lock().unwrap().as_deref() == Some(variable_id) {
                return Err(VaultError::Backend(format!(
                    "injected set failure for {variable_id}"
                )));
            }
            self.values
                .lock()
                .unwrap()
                .insert(Self::key(workspace_id, variable_id), value.to_owned());
            let mut fail_after_set = self.fail_after_set.lock().unwrap();
            if fail_after_set.as_deref() == Some(variable_id) {
                *fail_after_set = None;
                return Err(VaultError::Backend(format!(
                    "injected failure after setting {variable_id}"
                )));
            }
            Ok(())
        }

        fn delete(&self, workspace_id: &str, variable_id: &str) -> Result<(), VaultError> {
            if self.fail_on_delete.lock().unwrap().as_deref() == Some(variable_id) {
                return Err(VaultError::Backend(format!(
                    "injected delete failure for {variable_id}"
                )));
            }
            self.values
                .lock()
                .unwrap()
                .remove(&Self::key(workspace_id, variable_id));
            Ok(())
        }
    }

    fn secret_changes(entries: &[(&str, Option<&str>)]) -> Vec<SecretChange> {
        entries
            .iter()
            .map(|(variable_id, value)| SecretChange {
                variable_id: (*variable_id).to_owned(),
                value: value.map(str::to_owned),
            })
            .collect()
    }

    #[test]
    fn applies_and_rolls_back_changes_without_exposing_values() {
        let store = MemoryStore::default();
        store.set("workspace", "existing", "before").unwrap();
        let changes = vec![
            SecretChange {
                variable_id: "existing".to_owned(),
                value: Some("after".to_owned()),
            },
            SecretChange {
                variable_id: "new".to_owned(),
                value: Some("created".to_owned()),
            },
        ];

        let applied = apply_changes(&store, "workspace", &changes).unwrap();
        assert_eq!(
            store.get("workspace", "existing").unwrap().as_deref(),
            Some("after")
        );
        assert_eq!(
            store.get("workspace", "new").unwrap().as_deref(),
            Some("created")
        );

        applied.rollback().unwrap();
        assert_eq!(
            store.get("workspace", "existing").unwrap().as_deref(),
            Some("before")
        );
        assert_eq!(store.get("workspace", "new").unwrap(), None);
    }

    #[test]
    fn restores_prior_updates_when_a_later_change_fails() {
        let store = MemoryStore::default();
        store.set("workspace", "first", "before").unwrap();
        *store.fail_on_set.lock().unwrap() = Some("second".to_owned());

        let error = match apply_changes(
            &store,
            "workspace",
            &[
                SecretChange {
                    variable_id: "first".to_owned(),
                    value: Some("after".to_owned()),
                },
                SecretChange {
                    variable_id: "second".to_owned(),
                    value: Some("fails".to_owned()),
                },
            ],
        ) {
            Ok(_) => panic!("the injected store failure should be returned"),
            Err(error) => error,
        };

        assert!(matches!(error, VaultError::Transaction { .. }));
        assert_eq!(
            store.get("workspace", "first").unwrap().as_deref(),
            Some("before")
        );
    }

    #[test]
    fn commits_changes_and_returns_the_closure_result() {
        let store = MemoryStore::default();
        store.set("workspace", "existing", "before").unwrap();
        store.set("workspace", "removed", "old").unwrap();
        let changes = secret_changes(&[
            ("existing", Some("after")),
            ("created", Some("new")),
            ("removed", None),
        ]);
        let mut commit_count = 0;

        let result = transact_changes(&store, "workspace", &changes, || {
            assert_eq!(
                store.get("workspace", "existing")?.as_deref(),
                Some("after")
            );
            assert_eq!(store.get("workspace", "created")?.as_deref(), Some("new"));
            assert_eq!(store.get("workspace", "removed")?, None);
            commit_count += 1;
            Ok::<_, VaultError>("saved workspace".to_owned())
        })
        .unwrap();

        assert_eq!(result, "saved workspace");
        assert_eq!(commit_count, 1);
        assert_eq!(
            store.get("workspace", "existing").unwrap().as_deref(),
            Some("after")
        );
        assert_eq!(
            store.get("workspace", "created").unwrap().as_deref(),
            Some("new")
        );
        assert_eq!(store.get("workspace", "removed").unwrap(), None);
    }

    #[test]
    fn restores_the_batch_when_a_validation_read_returns_early() {
        let store = MemoryStore::default();
        store.set("workspace", "existing", "before").unwrap();
        store.set("workspace", "removed", "old").unwrap();
        let before = store.values.lock().unwrap().clone();
        *store.fail_on_get.lock().unwrap() = Some("unreadable".to_owned());
        let changes = secret_changes(&[
            ("existing", Some("after")),
            ("created", Some("new")),
            ("removed", None),
        ]);
        let mut committed = false;

        let error = transact_changes(&store, "workspace", &changes, || {
            assert_eq!(
                store.get("workspace", "existing")?.as_deref(),
                Some("after")
            );
            store.get("workspace", "unreadable")?;
            committed = true;
            Ok::<_, VaultError>(())
        })
        .unwrap_err();

        assert_eq!(
            error,
            VaultError::Transaction {
                operation: VaultError::Backend("injected read failure for unreadable".to_owned())
                    .to_string(),
                rollback: None,
            }
        );
        assert!(!committed);
        assert_eq!(*store.values.lock().unwrap(), before);
    }

    #[test]
    fn restores_the_batch_when_persistence_fails() {
        let store = MemoryStore::default();
        store.set("workspace", "existing", "before").unwrap();
        store.set("workspace", "removed", "old").unwrap();
        let before = store.values.lock().unwrap().clone();
        let changes = secret_changes(&[
            ("existing", Some("after")),
            ("created", Some("new")),
            ("removed", None),
        ]);
        let commit_error = "workspace file could not be committed".to_owned();

        let error = transact_changes(&store, "workspace", &changes, move || {
            Err::<(), String>(commit_error)
        })
        .unwrap_err();

        assert_eq!(
            error,
            VaultError::Transaction {
                operation: "workspace file could not be committed".to_owned(),
                rollback: None,
            }
        );
        assert_eq!(*store.values.lock().unwrap(), before);
    }

    #[test]
    fn retains_the_commit_error_and_all_rollback_failures_and_keeps_restoring() {
        let store = MemoryStore::default();
        store.set("workspace", "first", "before").unwrap();
        store.set("workspace", "second", "before").unwrap();
        store.set("workspace", "removed", "old").unwrap();
        let changes = secret_changes(&[
            ("first", Some("after")),
            ("second", Some("after")),
            ("created", Some("new")),
            ("removed", None),
        ]);

        let error = transact_changes(&store, "workspace", &changes, || {
            *store.fail_on_set.lock().unwrap() = Some("first".to_owned());
            *store.fail_on_delete.lock().unwrap() = Some("created".to_owned());
            Err::<(), _>("workspace file could not be committed")
        })
        .unwrap_err();

        let VaultError::Transaction {
            operation,
            rollback: Some(rollback),
        } = error
        else {
            panic!("both the commit and rollback errors must be returned");
        };
        assert_eq!(operation, "workspace file could not be committed");
        assert!(rollback.contains("injected delete failure for created"));
        assert!(rollback.contains("injected set failure for first"));
        assert_eq!(
            store.get("workspace", "second").unwrap().as_deref(),
            Some("before")
        );
        assert_eq!(
            store.get("workspace", "removed").unwrap().as_deref(),
            Some("old")
        );
        assert_eq!(
            store.get("workspace", "first").unwrap().as_deref(),
            Some("after")
        );
        assert_eq!(
            store.get("workspace", "created").unwrap().as_deref(),
            Some("new")
        );
    }

    #[test]
    fn snapshot_read_failure_leaves_the_store_untouched_and_skips_commit() {
        let store = MemoryStore::default();
        store.set("workspace", "first", "before").unwrap();
        let before = store.values.lock().unwrap().clone();
        *store.fail_on_get.lock().unwrap() = Some("second".to_owned());
        let changes = secret_changes(&[("first", Some("after")), ("second", Some("new"))]);

        let error = transact_changes(&store, "workspace", &changes, || -> Result<(), String> {
            panic!("commit must not run after a snapshot failure")
        })
        .unwrap_err();

        assert_eq!(
            error,
            VaultError::Backend("injected read failure for second".to_owned())
        );
        assert_eq!(*store.values.lock().unwrap(), before);
    }

    #[test]
    fn invalid_later_identifier_leaves_the_store_untouched_and_skips_commit() {
        let store = MemoryStore::default();
        store.set("workspace", "first", "before").unwrap();
        let before = store.values.lock().unwrap().clone();
        let changes = secret_changes(&[("first", Some("after")), ("", Some("new"))]);

        let error = transact_changes(&store, "workspace", &changes, || -> Result<(), String> {
            panic!("commit must not run after invalid input")
        })
        .unwrap_err();

        assert_eq!(error, VaultError::InvalidIdentifier("variable"));
        assert_eq!(*store.values.lock().unwrap(), before);
    }

    #[test]
    fn restores_even_the_write_that_mutated_the_store_before_failing() {
        let store = MemoryStore::default();
        store.set("workspace", "first", "before").unwrap();
        store.set("workspace", "second", "old").unwrap();
        let before = store.values.lock().unwrap().clone();
        *store.fail_after_set.lock().unwrap() = Some("second".to_owned());
        let changes = secret_changes(&[("first", Some("after")), ("second", Some("new"))]);

        let error = transact_changes(&store, "workspace", &changes, || -> Result<(), String> {
            panic!("commit must not run after a write failure")
        })
        .unwrap_err();

        assert_eq!(
            error,
            VaultError::Transaction {
                operation: VaultError::Backend("injected failure after setting second".to_owned())
                    .to_string(),
                rollback: None,
            }
        );
        assert_eq!(*store.values.lock().unwrap(), before);
    }

    #[test]
    fn restores_the_original_value_when_a_batch_changes_an_entry_more_than_once() {
        let store = MemoryStore::default();
        store.set("workspace", "existing", "before").unwrap();
        let changes = secret_changes(&[
            ("existing", Some("intermediate")),
            ("existing", None),
            ("existing", Some("after")),
        ]);

        let error = transact_changes(&store, "workspace", &changes, || {
            assert_eq!(
                store.get("workspace", "existing").unwrap().as_deref(),
                Some("after")
            );
            Err::<(), _>("validation failed")
        })
        .unwrap_err();

        assert_eq!(
            error,
            VaultError::Transaction {
                operation: "validation failed".to_owned(),
                rollback: None,
            }
        );
        assert_eq!(
            store.get("workspace", "existing").unwrap().as_deref(),
            Some("before")
        );
    }

    #[test]
    fn rejects_oversized_secrets_before_reaching_the_native_store() {
        let error = NativeSecretStore
            .set("workspace", "variable", &"x".repeat(MAX_SECRET_BYTES + 1))
            .unwrap_err();
        assert!(matches!(error, VaultError::SecretTooLarge { .. }));
    }
}
