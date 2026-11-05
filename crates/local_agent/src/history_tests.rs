use super::*;

fn tool_result(size: usize) -> ChatMessage {
    ChatMessage::tool_result("read_file", "x".repeat(size))
}

#[test]
fn old_results_are_cut_before_recent_ones() {
    let mut messages = vec![tool_result(10_000)];
    messages.extend((0..RECENT_MESSAGES).map(|_| tool_result(1_000)));

    trim_to_budget(&mut messages, 2_500);

    assert!(messages[0].content.len() < 2_000);
    assert_eq!(messages.last().unwrap().content.len(), 1_000);
}

#[test]
fn messages_within_budget_are_untouched() {
    let mut messages = vec![tool_result(10_000)];

    trim_to_budget(&mut messages, 10_000);

    assert_eq!(messages[0].content.len(), 10_000);
}

#[test]
fn only_tool_results_are_cut() {
    let mut messages = vec![ChatMessage::new("user", "x".repeat(10_000))];

    trim_to_budget(&mut messages, 10);

    assert_eq!(messages[0].content.len(), 10_000);
}
