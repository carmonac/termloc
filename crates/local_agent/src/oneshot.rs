//! Single-request features that don't go through an agent turn.
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use futures::StreamExt as _;
use serde::Deserialize;

use crate::ollama::{ChatBackend, ChatMessage, ChatOptions, ChatRequest};

const KEEP_ALIVE: &str = "30m";

const COMMANDS_PROMPT: &str = "You turn requests into shell commands. Reply with JSON only, in \
the form {\"commands\":[{\"command\":\"...\",\"description\":\"...\"}]}, with up to three \
alternatives. Each command must be complete and ready to run.";

const METADATA_PROMPT: &str = "You describe shell commands so they can be saved and reused. Reply \
with JSON only, in the form {\"title\":\"...\",\"description\":\"...\",\
\"parameterized_command\":\"...\",\"parameters\":[{\"name\":\"...\",\"description\":\"...\",\
\"value\":\"...\"}]}. In parameterized_command, replace values a user would change with \
{{name}}; value is the original text.";

const NEXT_COMMAND_PROMPT: &str = "You predict the next shell command a user will run. Reply with \
exactly one command on one line and nothing else. If there is no likely command, reply with \
nothing.";

const ASSISTANT_PROMPT: &str = "You are a terminal assistant. Answer briefly, and put commands in \
code blocks.";

const TITLE_PROMPT: &str = "Write a title of at most six words for a conversation that starts \
with the user's message. Reply with the title only, in the user's language, without quotes.";

const COMMIT_MESSAGE_PROMPT: &str = "Write a git commit message for the diff. Use a short summary \
line in the imperative mood, then a blank line and a brief explanation if the change needs one. Reply \
with the message only.";

const PR_TITLE_PROMPT: &str = "Write a pull request title of at most ten words for the diff. Reply \
with the title only, without quotes.";

const PR_DESCRIPTION_PROMPT: &str = "Write a pull request description for the diff in Markdown: a \
short summary followed by the main changes as a list. Reply with the description only.";

/// The text to write about a set of changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeText {
    CommitMessage,
    PrTitle,
    PrDescription,
}

/// The changes to write about.
#[derive(Clone, Debug, Default)]
pub struct ChangeContext {
    pub diff: String,
    pub branch_name: String,
    pub commit_messages: Vec<String>,
}

/// The model used for requests that need a quick answer.
#[derive(Clone, Debug)]
pub struct QuickModel {
    pub name: String,
    /// Whether the model reasons by default, which these requests turn off to save time.
    pub thinks: bool,
    pub num_ctx: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct GeneratedCommand {
    pub command: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize)]
struct GeneratedCommands {
    commands: Vec<GeneratedCommand>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct CommandParameter {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct CommandMetadata {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub parameterized_command: String,
    #[serde(default)]
    pub parameters: Vec<CommandParameter>,
}

/// What is known about the terminal when predicting the next command.
#[derive(Clone, Debug, Default)]
pub struct NextCommandContext {
    pub recent_blocks: Vec<String>,
    pub history: String,
    pub system: Option<String>,
    pub rejected: Vec<String>,
    pub prefix: Option<String>,
}

/// A client for the single-request features.
#[derive(Clone)]
pub struct OneShot {
    backend: Arc<dyn ChatBackend>,
    model: QuickModel,
}

impl OneShot {
    pub fn new(backend: Arc<dyn ChatBackend>, model: QuickModel) -> Self {
        Self { backend, model }
    }

    /// Turns a natural language request into commands. Small models often ignore the JSON format,
    /// so a plain answer counts as a single command.
    pub async fn generate_commands(&self, prompt: &str) -> anyhow::Result<Vec<GeneratedCommand>> {
        let reply = self.complete(COMMANDS_PROMPT, prompt).await?;
        let commands = match serde_json::from_str::<GeneratedCommands>(&reply) {
            Ok(parsed) => parsed.commands,
            Err(_) => first_line(&reply)
                .map(|command| {
                    vec![GeneratedCommand {
                        command,
                        description: prompt.to_owned(),
                    }]
                })
                .unwrap_or_default(),
        };
        let commands: Vec<GeneratedCommand> = commands
            .into_iter()
            .filter(|command| !command.command.trim().is_empty())
            .collect();
        if commands.is_empty() {
            return Err(anyhow!("the model returned no command"));
        }
        Ok(commands)
    }

    pub async fn command_metadata(&self, command: &str) -> anyhow::Result<CommandMetadata> {
        let reply = self.complete(METADATA_PROMPT, command).await?;
        serde_json::from_str(&reply).context("the model returned invalid metadata")
    }

    pub async fn next_command(
        &self,
        context: &NextCommandContext,
    ) -> anyhow::Result<Option<String>> {
        let reply = self
            .complete(NEXT_COMMAND_PROMPT, &next_command_prompt(context))
            .await?;
        let command = first_line(&reply)
            .map(|line| line.strip_prefix("$ ").map_or(line.clone(), str::to_owned));
        Ok(command.filter(|command| {
            !context.rejected.contains(command)
                && context
                    .prefix
                    .as_ref()
                    .is_none_or(|prefix| command.starts_with(prefix.as_str()))
        }))
    }

    /// Answers `question` after a conversation given as (question, answer) pairs.
    pub async fn answer(
        &self,
        transcript: &[(String, String)],
        question: &str,
    ) -> anyhow::Result<String> {
        let mut messages = vec![ChatMessage::new("system", ASSISTANT_PROMPT)];
        for (user, assistant) in transcript {
            messages.push(ChatMessage::new("user", user.as_str()));
            messages.push(ChatMessage::new("assistant", assistant.as_str()));
        }
        messages.push(ChatMessage::new("user", question));
        self.send(messages).await
    }

    /// Writes a commit message, pull request title or pull request description for `changes`.
    pub async fn change_text(
        &self,
        kind: ChangeText,
        changes: &ChangeContext,
    ) -> anyhow::Result<String> {
        let system = match kind {
            ChangeText::CommitMessage => COMMIT_MESSAGE_PROMPT,
            ChangeText::PrTitle => PR_TITLE_PROMPT,
            ChangeText::PrDescription => PR_DESCRIPTION_PROMPT,
        };
        let reply = self.complete(system, &change_prompt(changes)).await?;
        match kind {
            ChangeText::PrTitle => first_line(&reply)
                .map(|title| title.trim_matches(['"', '\'']).to_owned())
                .ok_or_else(|| anyhow!("the model returned no title")),
            ChangeText::CommitMessage | ChangeText::PrDescription => Ok(reply),
        }
    }

    pub async fn title(&self, first_message: &str) -> anyhow::Result<String> {
        let reply = self.complete(TITLE_PROMPT, first_message).await?;
        first_line(&reply)
            .map(|title| title.trim_matches(['"', '\'', '.']).to_owned())
            .filter(|title| !title.is_empty())
            .ok_or_else(|| anyhow!("the model returned no title"))
    }

    async fn complete(&self, system: &str, user: &str) -> anyhow::Result<String> {
        self.send(vec![
            ChatMessage::new("system", system),
            ChatMessage::new("user", user),
        ])
        .await
    }

    async fn send(&self, messages: Vec<ChatMessage>) -> anyhow::Result<String> {
        let request = ChatRequest {
            model: self.model.name.clone(),
            messages,
            tools: Vec::new(),
            options: ChatOptions {
                num_ctx: self.model.num_ctx,
            },
            stream: true,
            keep_alive: KEEP_ALIVE,
            think: self.model.thinks.then_some(false),
        };
        let mut stream = self.backend.chat(request).await?;
        let mut reply = String::new();
        while let Some(chunk) = stream.next().await {
            reply.push_str(&chunk?.message.content);
        }
        Ok(clean_reply(&reply))
    }
}

fn next_command_prompt(context: &NextCommandContext) -> String {
    let mut prompt = String::new();
    if let Some(system) = &context.system {
        prompt.push_str(&format!("System: {system}\n"));
    }
    if !context.history.is_empty() {
        prompt.push_str(&format!("Command history:\n{}\n", context.history));
    }
    if !context.recent_blocks.is_empty() {
        prompt.push_str(&format!(
            "Recent commands and output:\n{}\n",
            context.recent_blocks.join("\n")
        ));
    }
    if !context.rejected.is_empty() {
        prompt.push_str(&format!(
            "Don't suggest these: {}\n",
            context.rejected.join(", ")
        ));
    }
    if let Some(prefix) = &context.prefix {
        prompt.push_str(&format!("The command starts with: {prefix}\n"));
    }
    prompt
}

fn change_prompt(changes: &ChangeContext) -> String {
    let mut prompt = String::new();
    if !changes.branch_name.is_empty() {
        prompt.push_str(&format!("Branch: {}\n", changes.branch_name));
    }
    if !changes.commit_messages.is_empty() {
        prompt.push_str(&format!(
            "Commits:\n{}\n",
            changes.commit_messages.join("\n")
        ));
    }
    prompt.push_str(&format!("Diff:\n{}", changes.diff));
    prompt
}

/// Drops the reasoning and code fences models add around an answer.
fn clean_reply(reply: &str) -> String {
    let reply = reply
        .rsplit_once("</think>")
        .map_or(reply, |(_, answer)| answer)
        .trim();
    let Some(fenced) = reply.strip_prefix("```") else {
        return reply.to_owned();
    };
    let body = fenced.split_once('\n').map_or(fenced, |(_, body)| body);
    body.trim_end().trim_end_matches("```").trim().to_owned()
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "oneshot_tests.rs"]
mod tests;
