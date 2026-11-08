use warp_core::features::FeatureFlag;
use warp_multi_agent_api as api;

use super::get_supported_tools;
use crate::ai::agent::api::RequestParams;
use crate::ai::blocklist::SessionContext;
use crate::ai::llms::LLMId;

fn request_params_with_ask_user_question_enabled(ask_user_question_enabled: bool) -> RequestParams {
    let model = LLMId::from("test-model");

    RequestParams {
        input: vec![],
        conversation_token: None,
        forked_from_conversation_token: None,
        tasks: vec![],
        existing_suggestions: None,
        metadata: None,
        session_context: SessionContext::new_for_test(),
        model: model.clone(),
        coding_model: model.clone(),
        cli_agent_model: model.clone(),
        computer_use_model: model,
        is_memory_enabled: false,
        warp_drive_context_enabled: false,
        context_window_limit: None,
        mcp_context: None,
        planning_enabled: true,
        should_redact_secrets: false,
        autonomy_level: api::AutonomyLevel::Supervised,
        isolation_level: api::IsolationLevel::None,
        web_search_enabled: false,
        computer_use_enabled: false,
        ask_user_question_enabled,
        research_agent_enabled: false,
        supported_tools_override: None,
    }
}

#[test]
fn supported_tools_omits_ask_user_question_when_disabled() {
    let params = request_params_with_ask_user_question_enabled(false);
    let supported_tools = get_supported_tools(&params);

    assert!(!supported_tools.contains(&api::ToolType::AskUserQuestion));
}

#[test]
fn supported_tools_includes_ask_user_question_when_enabled_and_feature_flag_is_enabled() {
    if !FeatureFlag::AskUserQuestion.is_enabled() {
        return;
    }

    let params = request_params_with_ask_user_question_enabled(true);
    let supported_tools = get_supported_tools(&params);

    assert!(supported_tools.contains(&api::ToolType::AskUserQuestion));
}
