use super::ConversationDriver;

#[test]
fn native_owns_everything() {
    let driver = ConversationDriver::Native;
    assert!(driver.owns_turn_lifecycle());
    assert!(driver.executes_tool_calls_locally());
    assert!(!driver.reconstructs_inputs_from_messages());
    assert!(!driver.is_read_only_ui());
}

#[test]
fn cli_agent_transcript_is_a_read_only_vehicle() {
    let driver = ConversationDriver::CliAgentTranscript;
    assert!(!driver.owns_turn_lifecycle());
    assert!(!driver.executes_tool_calls_locally());
    assert!(!driver.reconstructs_inputs_from_messages());
    assert!(driver.is_read_only_ui());
}

#[test]
fn external_harness_owns_the_conversation_but_not_the_turn() {
    let driver = ConversationDriver::ExternalHarness;
    assert!(!driver.owns_turn_lifecycle());
    assert!(!driver.executes_tool_calls_locally());
    assert!(driver.reconstructs_inputs_from_messages());
    assert!(!driver.is_read_only_ui());
}

#[test]
fn default_driver_is_native() {
    assert_eq!(ConversationDriver::default(), ConversationDriver::Native);
}
