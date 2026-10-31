use super::*;
use api_client_vault::VaultError;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct MemoryStore {
    values: Arc<Mutex<BTreeMap<String, String>>>,
    fail_next_write: Arc<AtomicBool>,
    read_count: Arc<AtomicUsize>,
    fail_read_at: Arc<AtomicUsize>,
}

impl SecretStore for MemoryStore {
    fn get(&self, _: &str, variable_id: &str) -> Result<Option<String>, VaultError> {
        if self.read_count.fetch_add(1, Ordering::SeqCst) + 1
            == self.fail_read_at.load(Ordering::SeqCst)
        {
            return Err(VaultError::Backend("test read failure".to_owned()));
        }
        Ok(self.values.lock().unwrap().get(variable_id).cloned())
    }

    fn set(&self, _: &str, variable_id: &str, value: &str) -> Result<(), VaultError> {
        if self.fail_next_write.swap(false, Ordering::SeqCst) {
            return Err(VaultError::Backend("test write failure".to_owned()));
        }
        self.values
            .lock()
            .unwrap()
            .insert(variable_id.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, _: &str, variable_id: &str) -> Result<(), VaultError> {
        self.values.lock().unwrap().remove(variable_id);
        Ok(())
    }
}

fn environment(local: bool) -> (String, Environment) {
    let mut environment = Environment::new("Stage");
    let mut variable = EnvironmentVariable::new("access_token", "");
    variable.secret = !local;
    variable.local = local;
    environment.variables.push(variable);
    ("test-workspace".to_owned(), environment)
}

fn input(url: String, auth: RequestAuth, post_response: &str) -> RequestLifecycleInput {
    RequestLifecycleInput {
        request_id: None,
        environment_id: None,
        request: RequestDefinition {
            method: "POST".to_owned(),
            url,
            query_params: Vec::new(),
            headers: Vec::new(),
            auth,
            body: RequestBody::Empty,
            options: RequestOptions::default(),
        },
        scripts: RequestScripts {
            pre_request: String::new(),
            post_response: post_response.to_owned(),
        },
    }
}

async fn fixture(bodies: Vec<&'static str>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        for body in bodies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0u8; 2048];
                let read = stream.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0);
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|chunk| chunk == b"\r\n\r\n") {
                    break;
                }
            }
            captured.push(String::from_utf8(request).unwrap());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        captured
    });
    (url, server)
}

#[tokio::test]
async fn script_generated_protected_token_is_used_by_a_separate_send() {
    for local in [false, true] {
        let context = environment(local);
        let variable_id = &context.1.variables[0].id;
        let store = MemoryStore::default();
        store.set(&context.0, variable_id, "old-token").unwrap();
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let network = NetworkSettings {
            proxy_mode: ProxyMode::Disabled,
            ..Default::default()
        };
        let (url, server) = fixture(vec![r#"{"access_token":"fresh-token"}"#, "{}", "{}"]).await;
        let generated = run_request_lifecycle(
            input(
                format!("{url}/token"),
                RequestAuth::None,
                r#"
                pm.environment.set("access_token", pm.response.json().access_token);
                console.log(pm.environment.get("access_token"));
            "#,
            ),
            Some(context.clone()),
            store.clone(),
            None,
            network.clone(),
            None,
            context.0.clone(),
            Arc::clone(&cache),
            Arc::clone(&lock),
        )
        .await
        .unwrap();
        assert!(generated.error.is_none(), "{:?}", generated.error);
        assert_eq!(generated.response.as_ref().unwrap().status, 200);
        assert_eq!(
            store.get(&context.0, variable_id).unwrap().as_deref(),
            Some("fresh-token")
        );
        assert!(generated.environment_updates.is_empty());
        assert!(
            !serde_json::to_string(&generated.script_runs)
                .unwrap()
                .contains("fresh-token")
        );
        assert!(
            !serde_json::to_string(&context.1)
                .unwrap()
                .contains("fresh-token")
        );

        // Both ways of supplying the token must produce the same single header.
        for token in ["{{access_token}}", "fresh-token"] {
            let authenticated = run_request_lifecycle(
                input(
                    format!("{url}/resource"),
                    RequestAuth::Bearer {
                        token: token.to_owned(),
                    },
                    "",
                ),
                Some(context.clone()),
                store.clone(),
                None,
                network.clone(),
                None,
                context.0.clone(),
                Arc::clone(&cache),
                Arc::clone(&lock),
            )
            .await
            .unwrap();
            assert!(authenticated.error.is_none(), "{:?}", authenticated.error);
            assert_eq!(authenticated.response.unwrap().status, 200);
        }
        let requests = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        for request in &requests[1..] {
            let request = request.to_ascii_lowercase();
            assert_eq!(request.matches("authorization:").count(), 1);
            assert!(request.contains("authorization: bearer fresh-token\r\n"));
            assert!(!request.contains("old-token") && !request.contains("{{"));
        }
    }
}

#[tokio::test]
async fn script_token_write_failure_is_visible_and_keeps_the_http_response() {
    let context = environment(false);
    let store = MemoryStore::default();
    let variable_id = &context.1.variables[0].id;
    store.set(&context.0, variable_id, "old-token").unwrap();
    store.fail_next_write.store(true, Ordering::SeqCst);
    let (url, server) = fixture(vec![r#"{"access_token":"fresh-token"}"#]).await;
    let result = run_request_lifecycle(
        input(
            url,
            RequestAuth::None,
            r#"pm.environment.set("access_token", pm.response.json().access_token);"#,
        ),
        Some(context.clone()),
        store.clone(),
        None,
        NetworkSettings {
            proxy_mode: ProxyMode::Disabled,
            ..Default::default()
        },
        None,
        context.0.clone(),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(tokio::sync::Mutex::new(())),
    )
    .await
    .unwrap();
    assert_eq!(result.response.unwrap().status, 200);
    assert!(
        result
            .error
            .unwrap()
            .contains("Could not save script-updated")
    );
    assert!(result.environment_updates.is_empty());
    assert_eq!(
        store.get(&context.0, variable_id).unwrap().as_deref(),
        Some("old-token")
    );
    server.await.unwrap();
}

#[tokio::test]
async fn collection_generated_token_remains_available_after_the_run() {
    let context = environment(false);
    let variable_id = &context.1.variables[0].id;
    let store = MemoryStore::default();
    store.set(&context.0, variable_id, "old-token").unwrap();
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    let cache = Arc::new(Mutex::new(HashMap::new()));
    let network = NetworkSettings {
        proxy_mode: ProxyMode::Disabled,
        ..Default::default()
    };
    let (url, server) = fixture(vec![r#"{"access_token":"fresh-token"}"#, "{}", "{}"]).await;
    let requests = [
        input(
            format!("{url}/token"),
            RequestAuth::None,
            r#"pm.environment.set("access_token", pm.response.json().access_token);"#,
        ),
        input(
            format!("{url}/resource"),
            RequestAuth::Bearer {
                token: "{{access_token}}".to_owned(),
            },
            "",
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, input)| RunnableRequest {
        id: format!("request-{index}"),
        name: format!("Request {index}"),
        request: input.request,
        scripts: input.scripts,
        inherited_auth: RequestAuth::None,
    })
    .collect();
    let report = run_collection_internal(
        CollectionRunInput {
            target: CollectionRunTarget::Collection {
                collection_id: "test-collection".to_owned(),
            },
            environment_id: Some(context.1.id.clone()),
            iterations: 1,
            delay_ms: 0,
            stop_on_failure: true,
            data_rows: Vec::new(),
        },
        requests,
        Some(context.clone()),
        Some(context.1.name.clone()),
        store.clone(),
        None,
        network.clone(),
        context.0.clone(),
        "Workspace".to_owned(),
        "Collection".to_owned(),
        context.0.clone(),
        Arc::clone(&cache),
        Arc::clone(&lock),
    )
    .await
    .unwrap();
    assert_eq!(report.summary.requests_executed, 2);
    assert_eq!(report.summary.requests_failed, 0);
    assert_eq!(
        store.get(&context.0, variable_id).unwrap().as_deref(),
        Some("fresh-token")
    );
    let separate = run_request_lifecycle(
        input(
            format!("{url}/resource"),
            RequestAuth::Bearer {
                token: "{{access_token}}".to_owned(),
            },
            "",
        ),
        Some(context.clone()),
        store,
        None,
        network,
        None,
        context.0,
        cache,
        lock,
    )
    .await
    .unwrap();
    assert!(separate.error.is_none());
    assert_eq!(separate.response.unwrap().status, 200);
    let requests = server.await.unwrap();
    for request in &requests[1..] {
        assert!(request.contains("authorization: Bearer fresh-token\r\n"));
    }
}

#[tokio::test]
async fn script_token_updates_preserve_a_newer_manual_edit() {
    let context = environment(false);
    let store = MemoryStore::default();
    let variable_id = &context.1.variables[0].id;
    store
        .set(&context.0, variable_id, "manually-updated")
        .unwrap();
    let error = persist_script_environment(
        Some(context.clone()),
        &BTreeMap::from([("access_token".to_owned(), "old-token".to_owned())]),
        &BTreeMap::from([("access_token".to_owned(), "fresh-token".to_owned())]),
        store.clone(),
        Arc::new(tokio::sync::Mutex::new(())),
    )
    .await
    .unwrap_err();
    assert!(error.contains("{{access_token}}") && error.contains("newer value was preserved"));
    assert!(!error.contains("manually-updated"));
    assert_eq!(
        store.get(&context.0, variable_id).unwrap().as_deref(),
        Some("manually-updated")
    );
}

fn workspace_with_secret(store: &MemoryStore) -> (tempfile::TempDir, ActiveWorkspace, String) {
    let directory = tempfile::tempdir().unwrap();
    let created = WorkspaceStore::create(directory.path(), "Transaction checks").unwrap();
    let mut workspace = created.workspace.clone();
    let (_, environment) = environment(false);
    let id = environment.variables[0].id.clone();
    store
        .set(&workspace.id, &id, "original-credential")
        .unwrap();
    workspace.environments = vec![environment];
    let root = PathBuf::from(&created.root_path);
    let saved = WorkspaceStore::save(&root, &created.workspace, &workspace).unwrap();
    (
        directory,
        ActiveWorkspace {
            root_path: root,
            workspace: saved.workspace,
        },
        id,
    )
}

#[test]
fn failed_post_write_vault_read_restores_credentials_and_leaves_disk_unchanged() {
    let store = MemoryStore::default();
    let (_directory, active, id) = workspace_with_secret(&store);
    store.fail_read_at.store(2, Ordering::SeqCst);
    let mut updated = active.workspace.clone();
    updated.name = "Changed name".to_owned();
    let error = save_workspace_secret_transaction(
        &store,
        &active,
        updated,
        vec![SecretMutationInput {
            variable_id: id.clone(),
            value: Some("replacement-credential".to_owned()),
        }],
    )
    .unwrap_err();
    assert!(error.contains("test read failure"));
    assert_eq!(
        store.get(&active.workspace.id, &id).unwrap().as_deref(),
        Some("original-credential")
    );
    assert_eq!(
        WorkspaceStore::load(&active.root_path).unwrap().workspace,
        active.workspace
    );
}

#[test]
fn concurrent_manifest_edit_rolls_back_vault_changes_and_preserves_the_external_edit() {
    let store = MemoryStore::default();
    let (_directory, active, id) = workspace_with_secret(&store);
    let mut external = active.workspace.clone();
    external.name = "External edit".to_owned();
    WorkspaceStore::save(&active.root_path, &active.workspace, &external).unwrap();
    assert!(
        save_workspace_secret_transaction(
            &store,
            &active,
            active.workspace.clone(),
            vec![SecretMutationInput {
                variable_id: id.clone(),
                value: Some("replacement-credential".to_owned()),
            }]
        )
        .is_err()
    );
    assert_eq!(
        store.get(&active.workspace.id, &id).unwrap().as_deref(),
        Some("original-credential")
    );
    assert_eq!(
        WorkspaceStore::load(&active.root_path).unwrap().workspace,
        external
    );
}

#[test]
fn credential_transaction_commits_metadata_without_putting_values_in_the_manifest() {
    let store = MemoryStore::default();
    let (_directory, active, id) = workspace_with_secret(&store);
    let mut updated = active.workspace.clone();
    updated.name = "Updated workspace".to_owned();
    let saved = save_workspace_secret_transaction(
        &store,
        &active,
        updated,
        vec![SecretMutationInput {
            variable_id: id.clone(),
            value: Some("replacement-credential".to_owned()),
        }],
    )
    .unwrap();
    assert_eq!(saved.workspace.name, "Updated workspace");
    assert_eq!(
        store.get(&active.workspace.id, &id).unwrap().as_deref(),
        Some("replacement-credential")
    );
    let manifest = fs::read_to_string(active.root_path.join("workspace.yaml")).unwrap();
    assert!(!manifest.contains("replacement-credential"));
    assert!(!manifest.contains("original-credential"));
}

#[test]
fn credential_deletion_cannot_target_an_identifier_outside_the_workspace_metadata() {
    let store = MemoryStore::default();
    let (_directory, active, _) = workspace_with_secret(&store);
    let unrelated_id = Uuid::new_v4().to_string();
    store
        .set(&active.workspace.id, &unrelated_id, "unrelated-value")
        .unwrap();
    assert!(
        save_workspace_secret_transaction(
            &store,
            &active,
            active.workspace.clone(),
            vec![SecretMutationInput {
                variable_id: unrelated_id.clone(),
                value: None,
            }]
        )
        .is_err()
    );
    assert_eq!(
        store
            .get(&active.workspace.id, &unrelated_id)
            .unwrap()
            .as_deref(),
        Some("unrelated-value")
    );
}

#[tokio::test]
async fn variable_expansion_failure_is_rejected_before_any_http_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mut environment = Environment::new("Expansion budget");
    for index in 0..70 {
        environment.variables.push(EnvironmentVariable::new(
            format!("v{index}"),
            format!("{{{{v{}}}}}", index + 1),
        ));
    }
    environment
        .variables
        .push(EnvironmentVariable::new("v70", "resolved"));
    let mut request = input(url, RequestAuth::None, "");
    request.request.body = RequestBody::Text {
        content: "{{v0}}".to_owned(),
        content_type: Some("text/plain".to_owned()),
    };
    let result = run_request_lifecycle(
        request,
        Some(("workspace".to_owned(), environment)),
        MemoryStore::default(),
        None,
        NetworkSettings {
            proxy_mode: ProxyMode::Disabled,
            ..Default::default()
        },
        None,
        "workspace".to_owned(),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(tokio::sync::Mutex::new(())),
    )
    .await
    .unwrap();
    assert!(result.response.is_none());
    assert!(result.error.unwrap().contains("nesting depth limit"));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[test]
fn network_variable_resolution_rejects_expansion_budget_errors() {
    let mut values = BTreeMap::new();
    for index in 0..70 {
        values.insert(format!("v{index}"), format!("{{{{v{}}}}}", index + 1));
    }
    values.insert("v70".to_owned(), "private-proxy-password".to_owned());
    let error = resolve_network_value("proxy password", "{{v0}}", &values).unwrap_err();
    assert!(error.contains("nesting depth limit"));
    assert!(!error.contains("private-proxy-password"));
}

#[tokio::test]
async fn an_unset_secret_can_be_populated_by_the_first_token_response() {
    let context = environment(false);
    let id = &context.1.variables[0].id;
    let store = MemoryStore::default();
    let (url, server) = fixture(vec![r#"{"access_token":"first-token"}"#]).await;
    let result = run_request_lifecycle(
        input(url, RequestAuth::None, r#"pm.environment.set("access_token", pm.response.json().access_token); console.log(pm.environment.get("access_token"));"#),
        Some(context.clone()), store.clone(), None,
        NetworkSettings { proxy_mode: ProxyMode::Disabled, ..Default::default() }, None,
        context.0.clone(), Arc::new(Mutex::new(HashMap::new())), Arc::new(tokio::sync::Mutex::new(())),
    ).await.unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.response.unwrap().status, 200);
    assert_eq!(
        store.get(&context.0, id).unwrap().as_deref(),
        Some("first-token")
    );
    assert!(result.environment_updates.is_empty());
    assert!(
        !serde_json::to_string(&result.script_runs)
            .unwrap()
            .contains("first-token")
    );
    server.await.unwrap();
}

#[tokio::test]
async fn an_unset_secret_reference_still_blocks_the_request_before_network_io() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let result = run_request_lifecycle(
        input(
            format!("http://{}", listener.local_addr().unwrap()),
            RequestAuth::Bearer {
                token: "{{access_token}}".to_owned(),
            },
            "",
        ),
        Some(environment(false)),
        MemoryStore::default(),
        None,
        NetworkSettings {
            proxy_mode: ProxyMode::Disabled,
            ..Default::default()
        },
        None,
        "workspace".to_owned(),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(tokio::sync::Mutex::new(())),
    )
    .await
    .unwrap();
    assert!(result.response.is_none());
    assert!(
        result
            .error
            .unwrap()
            .contains("Unresolved variables: {{access_token}}")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}
