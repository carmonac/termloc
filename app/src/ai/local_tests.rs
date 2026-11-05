use super::*;

fn model(name: &str, supports_tools: bool, context_length: Option<u32>) -> LocalModel {
    LocalModel {
        name: name.to_owned(),
        supports_tools,
        supports_vision: false,
        context_length,
        size: 0,
    }
}

#[test]
fn only_models_with_tools_can_run_the_agent() {
    let models = agent_models(vec![model("a", false, None), model("b", true, None)]);

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].name, "b");
}

#[test]
fn largest_model_comes_first() {
    let sized = |name, size| LocalModel {
        size,
        ..model(name, true, None)
    };

    let models = agent_models(vec![sized("small", 1), sized("large", 9)]);

    assert_eq!(models[0].name, "large");
}

#[test]
fn requested_model_wins_and_unknown_ids_fall_back() {
    let models = vec![model("a", true, None), model("b", true, None)];

    assert_eq!(select_model(&models, "b").unwrap().name, "b");
    assert_eq!(select_model(&models, "auto").unwrap().name, "a");
    assert!(select_model(&[], "b").is_none());
}

#[test]
fn context_window_never_exceeds_what_the_model_supports() {
    assert_eq!(
        context_window(16_384, &model("a", true, Some(8_192))),
        8_192
    );
    assert_eq!(
        context_window(16_384, &model("a", true, Some(40_960))),
        16_384
    );
    assert_eq!(context_window(16_384, &model("a", true, None)), 16_384);
}
