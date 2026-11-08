use std::sync::Mutex;

use futures::executor::block_on;
use futures::future::BoxFuture;
use futures::{FutureExt as _, stream};

use super::*;
use crate::ollama::{ChatChunk, ChatStream};

#[derive(Default)]
struct ReplyBackend {
    reply: String,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ChatBackend for ReplyBackend {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'static, anyhow::Result<ChatStream>> {
        self.requests.lock().unwrap().push(request);
        let chunk = ChatChunk {
            message: ChatMessage::new("assistant", self.reply.as_str()),
            done: true,
            ..Default::default()
        };
        let stream: ChatStream = stream::iter([Ok(chunk)]).boxed();
        async move { Ok(stream) }.boxed()
    }
}

fn one_shot(reply: &str, thinks: bool) -> (OneShot, Arc<ReplyBackend>) {
    let backend = Arc::new(ReplyBackend {
        reply: reply.to_owned(),
        ..Default::default()
    });
    let model = QuickModel {
        name: "test".to_owned(),
        thinks,
        num_ctx: 4_096,
    };
    (OneShot::new(backend.clone(), model), backend)
}

#[test]
fn commands_are_read_from_json() {
    let (client, _) = one_shot(
        r#"{"commands":[{"command":"ls -a","description":"List all"}]}"#,
        false,
    );

    let commands = block_on(client.generate_commands("list")).unwrap();

    assert_eq!(commands[0].command, "ls -a");
    assert_eq!(commands[0].description, "List all");
}

#[test]
fn a_plain_answer_counts_as_one_command() {
    let (client, _) = one_shot("```bash\nfind . -size +100M\n```", false);

    let commands = block_on(client.generate_commands("big files")).unwrap();

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].command, "find . -size +100M");
    assert_eq!(commands[0].description, "big files");
}

#[test]
fn an_empty_answer_is_an_error() {
    let (client, _) = one_shot("  ", false);

    assert!(block_on(client.generate_commands("x")).is_err());
}

#[test]
fn reasoning_is_turned_off_only_for_models_that_reason() {
    let (client, backend) = one_shot("ok", true);
    block_on(client.title("hello")).unwrap();
    assert_eq!(backend.requests.lock().unwrap()[0].think, Some(false));

    let (client, backend) = one_shot("ok", false);
    block_on(client.title("hello")).unwrap();
    assert_eq!(backend.requests.lock().unwrap()[0].think, None);
}

#[test]
fn reasoning_text_is_dropped_from_the_reply() {
    assert_eq!(clean_reply("<think>hmm</think>\n\nls"), "ls");
}

#[test]
fn next_command_respects_the_prefix_and_rejections() {
    let context = NextCommandContext {
        prefix: Some("git".to_owned()),
        ..Default::default()
    };
    let (client, _) = one_shot("git status", false);
    assert_eq!(
        block_on(client.next_command(&context)).unwrap().as_deref(),
        Some("git status")
    );

    let (client, _) = one_shot("ls", false);
    assert!(block_on(client.next_command(&context)).unwrap().is_none());

    let context = NextCommandContext {
        rejected: vec!["ls".to_owned()],
        ..Default::default()
    };
    assert!(block_on(client.next_command(&context)).unwrap().is_none());
}

#[test]
fn next_command_drops_a_prompt_sign() {
    let (client, _) = one_shot("$ git push", false);

    let command = block_on(client.next_command(&NextCommandContext::default())).unwrap();

    assert_eq!(command.as_deref(), Some("git push"));
}

#[test]
fn titles_lose_their_quotes() {
    let (client, _) = one_shot("\"Fix the build.\"", false);

    assert_eq!(block_on(client.title("x")).unwrap(), "Fix the build");
}

#[test]
fn answers_include_the_transcript() {
    let (client, backend) = one_shot("sure", false);

    block_on(client.answer(&[("a".to_owned(), "b".to_owned())], "c")).unwrap();

    let requests = backend.requests.lock().unwrap();
    let roles: Vec<&str> = requests[0]
        .messages
        .iter()
        .map(|m| m.role.as_str())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"]);
}

/// Needs a running Ollama. Run with `LOCAL_AGENT_TEST_MODEL=<model> cargo test -p local_agent
/// live_quick_requests -- --ignored --nocapture`.
#[test]
#[ignore]
fn live_quick_requests() {
    let name = std::env::var("LOCAL_AGENT_TEST_MODEL").expect("LOCAL_AGENT_TEST_MODEL");
    let thinks = std::env::var("LOCAL_AGENT_TEST_THINKS").is_ok();
    let client = OneShot::new(
        Arc::new(crate::OllamaClient::new("http://127.0.0.1:11434")),
        QuickModel {
            name,
            thinks,
            num_ctx: 4_096,
        },
    );
    let context = NextCommandContext {
        recent_blocks: vec!["$ git add .\n".to_owned()],
        ..Default::default()
    };

    println!(
        "{:?}",
        block_on(client.generate_commands("show the 5 largest files here"))
    );
    println!(
        "{:?}",
        block_on(client.command_metadata("git log -n 5 --oneline"))
    );
    println!("{:?}", block_on(client.next_command(&context)));
    println!(
        "{:?}",
        block_on(client.title("Help me fix the failing login test"))
    );
}
