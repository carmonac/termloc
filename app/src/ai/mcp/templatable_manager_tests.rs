use futures_util::stream::AbortHandle;
use mcp::oauth::CallbackResult;
use url::Url;
use uuid::Uuid;
use warpui::App;

use super::{SpawnedServerInfo, TemplatableMCPServerManager};

#[test]
fn prewarmed_mcp_spawn_uses_current_key_without_persisting_it() {
    use crate::ai::mcp::{MCPServer, MCPServerExt as _, TransportType};
    let json = r#"{"local":{"command":"test-mcp","env":{"TERMLOC_API_KEY":"standby-key","CUSTOM":"configured"}}}"#;
    let mut server = MCPServer::from_user_json(json).unwrap().pop().unwrap();
    let mut manager = TemplatableMCPServerManager::default();
    for key in ["assigned-key", "next-assigned-key"] {
        manager
            .runtime_environment
            .insert("TERMLOC_API_KEY".into(), key.into());
        manager
            .runtime_environment
            .insert("WARP_RUN_ID".into(), "assigned-run".into());
        manager.apply_runtime_environment(&mut server.transport_type);
        let TransportType::CLIServer(server) = &server.transport_type else {
            panic!("stdio server expected");
        };
        let keys: Vec<_> = server
            .static_env_vars
            .iter()
            .filter(|var| var.name == "TERMLOC_API_KEY")
            .collect();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].value, key);
        assert!(
            server
                .static_env_vars
                .iter()
                .any(|var| var.name == "CUSTOM" && var.value == "configured")
        );
    }
    let stored = MCPServer::from_user_json(json).unwrap().pop().unwrap();
    let TransportType::CLIServer(stored) = stored.transport_type else {
        panic!("stdio server expected");
    };
    assert!(
        stored
            .static_env_vars
            .iter()
            .any(|var| var.name == "TERMLOC_API_KEY" && var.value == "standby-key")
    );
}

#[test]
fn oauth_callback_preserves_decoded_issuer() {
    let mut manager = TemplatableMCPServerManager::default();
    let installation_uuid = Uuid::new_v4();
    let (abort_handle, _) = AbortHandle::new_pair();
    let (oauth_result_tx, oauth_result_rx) = async_channel::unbounded();
    manager.spawned_servers.insert(
        installation_uuid,
        SpawnedServerInfo {
            abort_handle,
            oauth_result_tx,
        },
    );
    manager
        .pending_oauth_csrf
        .insert("test-state".to_string(), installation_uuid);
    let callback_url = Url::parse(
        "warpdev://mcp/oauth2callback?code=test-code&state=test-state&iss=https%3A%2F%2Fmcp.linear.app",
    )
    .unwrap();

    manager.handle_oauth_callback(&callback_url).unwrap();

    match oauth_result_rx.try_recv().unwrap() {
        CallbackResult::Success {
            code,
            csrf_token,
            issuer,
        } => {
            assert_eq!(code, "test-code");
            assert_eq!(csrf_token, "test-state");
            assert_eq!(issuer.as_deref(), Some("https://mcp.linear.app"));
        }
        CallbackResult::Error { error } => panic!("unexpected callback error: {error:?}"),
    }
}

#[test]
fn reconnect_waiter_joins_pending_spawn() {
    let mut manager = TemplatableMCPServerManager::default();
    let installation_uuid = Uuid::new_v4();
    let (abort_handle, _) = AbortHandle::new_pair();
    let (oauth_result_tx, _) = async_channel::unbounded();
    manager.spawned_servers.insert(
        installation_uuid,
        SpawnedServerInfo {
            abort_handle,
            oauth_result_tx,
        },
    );
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();

    assert!(!manager.register_reconnect_waiter(installation_uuid, result_tx));
    assert!(manager.spawned_servers.contains_key(&installation_uuid));

    manager.notify_reconnect_waiters(installation_uuid, Err("spawn failed".to_string()));
    let error = result_rx
        .blocking_recv()
        .expect("waiter should be notified")
        .expect_err("spawn should fail");
    assert_eq!(error, "spawn failed");
}

#[test]
fn reconnect_completion_notifies_all_waiters() {
    let mut manager = TemplatableMCPServerManager::default();
    let installation_uuid = Uuid::new_v4();
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let (second_tx, second_rx) = tokio::sync::oneshot::channel();

    assert!(manager.register_reconnect_waiter(installation_uuid, first_tx));
    assert!(!manager.register_reconnect_waiter(installation_uuid, second_tx));

    manager.notify_reconnect_waiters(installation_uuid, Err("connection failed".to_string()));

    let first_error = first_rx
        .blocking_recv()
        .expect("first waiter should be notified")
        .expect_err("connection should fail");
    let second_error = second_rx
        .blocking_recv()
        .expect("second waiter should be notified")
        .expect_err("connection should fail");
    assert_eq!(first_error, "connection failed");
    assert_eq!(second_error, "connection failed");
    assert!(
        !manager
            .pending_reconnections
            .contains_key(&installation_uuid)
    );
}

#[test]
fn credential_work_waits_for_stored_credentials() {
    App::test((), |mut app| async move {
        app.update(|ctx| warpui_extras::secure_storage::register_noop("test", ctx));
        let manager = app.add_model(|_| TemplatableMCPServerManager {
            credentials_loading: true,
            ..Default::default()
        });

        manager.update(&mut app, |manager, ctx| {
            manager.purge_file_based_server_credentials(&vec![1], ctx);
            assert_eq!(manager.after_credentials_load.len(), 1);

            manager.after_credentials_load.push(Box::new(|manager, _| {
                assert!(!manager.credentials_loading);
                manager
                    .runtime_environment
                    .insert("RAN_AFTER_LOAD".into(), "true".into());
            }));
            manager.finish_loading_credentials(Default::default(), ctx);

            assert!(manager.after_credentials_load.is_empty());
            assert!(manager.runtime_environment.contains_key("RAN_AFTER_LOAD"));
        });
    });
}
