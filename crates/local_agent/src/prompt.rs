use warp_multi_agent_api::InputContext;

const BASE_PROMPT: &str = "You are a coding agent running inside a terminal on the user's machine. \
Use the provided tools to inspect and change the project, then answer briefly. \
Read a file before editing it. Run one step at a time and check each result. \
When the task is finished, reply with plain text and no tool call.";

pub(crate) const SUMMARY_PROMPT: &str = "Summarize this conversation so that work can continue \
from the summary alone. Keep the user's goal, decisions made, files changed, and open steps. \
Reply with the summary only.";

pub(crate) const CONTINUE_PROMPT: &str =
    "Continue with the task from where the summary leaves off.";

pub(crate) fn system_prompt(context: Option<&InputContext>) -> String {
    let mut prompt = BASE_PROMPT.to_owned();
    let Some(context) = context else {
        return prompt;
    };
    if let Some(directory) = context.directory.as_ref().filter(|dir| !dir.pwd.is_empty()) {
        prompt.push_str(&format!("\nWorking directory: {}", directory.pwd));
    }
    if let Some(shell) = context
        .shell
        .as_ref()
        .filter(|shell| !shell.name.is_empty())
    {
        prompt.push_str(&format!("\nShell: {}", shell.name));
    }
    if let Some(os) = context
        .operating_system
        .as_ref()
        .filter(|os| !os.platform.is_empty())
    {
        prompt.push_str(&format!("\nOperating system: {}", os.platform));
    }
    prompt
}
