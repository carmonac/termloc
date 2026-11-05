use std::fs;

use async_io::Async;
use base64::Engine as _;
use futures::executor::block_on;
use futures::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};
use serde_json::json;
use tempfile::TempDir;
use url::Url;
use warpui::r#async::FutureExt as _;
use warpui::{App, SingletonEntity as _};

use super::attachments::ResolvedFile;
use super::protocol::{ContentBlock, PromptCapabilities};
use super::*;
use crate::terminal::History;
use crate::terminal::model::block::BlockMetadata;
use crate::terminal::model::session::SessionInfo;
use crate::terminal::shell::ShellType;
use crate::terminal::view::Event;
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

/// Plays the agent's side of the handshake, answering `initialize` with `initialize_result`,
/// and returns the content of the first `session/prompt`.
async fn first_acp_prompt(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    token: &str,
    initialize_result: Value,
) -> Value {
    let mut stream = BufReader::new(stream);
    stream
        .get_mut()
        .write_all(format!("{token}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    assert_eq!(line.trim_end(), "ok");
    for (method, result) in [
        ("initialize", initialize_result),
        ("session/new", json!({"sessionId": "test-session"})),
    ] {
        line.clear();
        stream.read_line(&mut line).await.unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], method);
        let response = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
        stream
            .get_mut()
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
    }
    line.clear();
    stream.read_line(&mut line).await.unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(request["method"], "session/prompt");
    assert_eq!(request["params"]["sessionId"], "test-session");
    request["params"]["prompt"].clone()
}

#[test]
fn initial_prompt_links_files_after_its_text() {
    let dir = std::env::temp_dir().join("acp-initial-attachments");
    let screenshot = dir.join("image #雪%.png");
    let notes = dir.join("notes.txt");
    let files = [
        ResolvedFile {
            name: "image #雪%.png".to_owned(),
            path: screenshot.clone(),
            mime_type: "image/png".to_owned(),
            contents: None,
        },
        ResolvedFile {
            name: "notes.txt".to_owned(),
            path: notes.clone(),
            mime_type: "text/plain".to_owned(),
            contents: None,
        },
    ];

    let blocks = attachments::prompt_content(
        PromptCapabilities::default(),
        "inspect these".to_owned(),
        Vec::new(),
        &files,
    );

    assert_eq!(blocks.len(), 3);
    let ContentBlock::ResourceLink { uri: image_uri, .. } = &blocks[1] else {
        panic!("expected an image resource link");
    };
    let ContentBlock::ResourceLink { uri: notes_uri, .. } = &blocks[2] else {
        panic!("expected a non-image resource link");
    };
    assert_eq!(
        serde_json::to_value(&blocks).unwrap(),
        json!([
            {"type": "text", "text": "inspect these"},
            {"type": "resource_link", "uri": image_uri, "name": "image #雪%.png"},
            {"type": "resource_link", "uri": notes_uri, "name": "notes.txt"}
        ])
    );
    assert_eq!(
        Url::parse(image_uri).unwrap().to_file_path().unwrap(),
        screenshot
    );
    assert_eq!(
        Url::parse(notes_uri).unwrap().to_file_path().unwrap(),
        notes
    );
    assert!(image_uri.contains("image%20%23%E9%9B%AA%25.png"));
}

#[test]
fn a_restarted_phase_gets_the_full_budget_again() {
    let budget = Duration::from_millis(400);
    let clock = PhaseClock::start("first");
    block_on(async {
        let expire = clock.expire(budget).fuse();
        // 500ms of work in total, which outruns one budget but not two phases of it.
        let work = async {
            Timer::after(Duration::from_millis(200)).await;
            clock.restart("second");
            Timer::after(Duration::from_millis(300)).await;
        }
        .fuse();
        pin_mut!(expire, work);
        select! {
            () = expire => panic!("the deadline ignored the restart"),
            () = work => {}
        }
    });
    assert_eq!(clock.phase(), "second");
}

#[test]
fn a_phase_that_is_not_restarted_expires() {
    let clock = PhaseClock::start("only");
    block_on(async {
        let expire = clock.expire(Duration::from_millis(100)).fuse();
        let work = Timer::after(Duration::from_millis(400)).fuse();
        pin_mut!(expire, work);
        select! {
            () = expire => {}
            _ = work => panic!("the deadline never fired"),
        }
    });
    assert_eq!(clock.phase(), "only");
}

#[test]
#[serial_test::serial]
fn initial_files_reach_the_agent_shaped_by_its_capabilities() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("image.png");
        fs::write(&file_path, b"image bytes").unwrap();

        let file_uri = Url::from_file_path(&file_path).unwrap();
        let link_block = json!({
            "type": "resource_link",
            "uri": file_uri.as_str(),
            "name": "image.png",
        });
        let image_block = json!({
            "type": "image",
            "data": base64::engine::general_purpose::STANDARD.encode(b"image bytes"),
            "mimeType": "image/png",
        });
        let baseline_agent = json!({"protocolVersion": PROTOCOL_VERSION});
        let image_agent = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "agentCapabilities": {"promptCapabilities": {"image": true}},
        });
        let cases = [(baseline_agent, link_block), (image_agent, image_block)];
        for (initialize_result, expected_file_block) in cases {
            let terminal = add_window_with_terminal(&mut app, None);
            let session_id = terminal.read(&app, |terminal, _| {
                terminal
                    .model
                    .lock()
                    .block_list()
                    .active_block()
                    .session_id()
                    .unwrap()
            });
            terminal.update(&mut app, |terminal, ctx| {
                terminal
                    .model_event_dispatcher()
                    .update(ctx, |dispatcher, _| {
                        dispatcher.set_active_session_id(session_id);
                    });
                terminal.sessions_model().update(ctx, |sessions, ctx| {
                    sessions.initialize_bootstrapped_session(
                        SessionInfo::new_for_test()
                            .with_id(session_id)
                            .with_shell_type(ShellType::Zsh),
                        "test command".to_owned(),
                        Vec::new(),
                        None,
                        ctx,
                    );
                });
                terminal.input().update(ctx, |input, ctx| {
                    input.set_active_block_metadata(
                        BlockMetadata::new(Some(session_id), None),
                        false,
                        ctx,
                    );
                });
            });
            History::initialized_sessions(&mut History::handle(&app), &mut app, vec![session_id])
                .await;
            let (commands, command_rx) = async_channel::unbounded();
            app.update(|ctx| {
                ctx.subscribe_to_view(&terminal, move |_, event, _| {
                    if let Event::ExecuteCommand(event) = event {
                        commands.try_send(event.command.clone()).unwrap();
                    }
                });
            });
            let acp_terminal =
                app.update(|ctx| AcpTerminal::create_from_existing_view(terminal, ctx));
            let foreground = acp_terminal.update(&mut app, |_, ctx| ctx.spawner());
            let runner = AcpRunner::new(AcpRunConfig {
                harness: Harness::Codex,
                launch: AcpLaunchSpec {
                    program: "codex-acp".to_owned(),
                    args: Vec::new(),
                },
                prompt: "inspect these".to_owned(),
                initial_files: vec![file_path.clone()],
                working_dir: temp.path().to_path_buf(),
                mcp_servers: Vec::new(),
                model_config: None,
            });
            let observe_prompt = async {
                let command = command_rx.recv().await.unwrap();
                let launch_args =
                    shell_words::split(command.split_once("--launch-file ").unwrap().1).unwrap();
                let launch: bridge::BridgeLaunch =
                    serde_json::from_slice(&fs::read(&launch_args[0]).unwrap()).unwrap();
                let prompt = match launch.endpoint {
                    #[cfg(unix)]
                    bridge::BridgeEndpoint::Unix(path) => {
                        let stream = Async::<std::os::unix::net::UnixStream>::connect(path)
                            .await
                            .unwrap();
                        first_acp_prompt(stream, &launch.token, initialize_result).await
                    }
                    #[cfg(not(unix))]
                    bridge::BridgeEndpoint::Unix(_) => panic!("expected a TCP endpoint"),
                    bridge::BridgeEndpoint::Tcp(address) => {
                        let stream = Async::<std::net::TcpStream>::connect(address)
                            .await
                            .unwrap();
                        first_acp_prompt(stream, &launch.token, initialize_result).await
                    }
                };
                assert_eq!(
                    prompt,
                    json!([
                        {"type": "text", "text": "inspect these"},
                        expected_file_block,
                    ])
                );
            };
            async {
                let (started, ()) = futures::join!(runner.start(&foreground), observe_prompt);
                started.unwrap();
            }
            .with_timeout(Duration::from_secs(10))
            .await
            .unwrap();
        }
    });
}
