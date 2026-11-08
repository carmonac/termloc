use local_agent::LocalModel;

use super::local_model_detail_rows;

#[test]
fn local_model_rows_describe_what_ollama_reports() {
    let model = LocalModel {
        name: "qwen3:8b".to_owned(),
        supports_tools: true,
        supports_thinking: true,
        context_length: Some(40_960),
        size: 5_225_388_164,
        parameter_size: Some("8.2B".to_owned()),
        quantization: Some("Q4_K_M".to_owned()),
        ..Default::default()
    };

    assert_eq!(
        local_model_detail_rows(&model),
        vec![
            ("Parameters", "8.2B".to_owned()),
            ("Quantization", "Q4_K_M".to_owned()),
            ("Context", "40,960 tokens".to_owned()),
            ("Size on disk", "5.2 GB".to_owned()),
            ("Supports", "tools · thinking".to_owned()),
        ]
    );
}

#[test]
fn local_model_rows_skip_unreported_values() {
    let model = LocalModel {
        name: "tiny".to_owned(),
        size: 274_302_450,
        ..Default::default()
    };

    assert_eq!(
        local_model_detail_rows(&model),
        vec![("Size on disk", "274 MB".to_owned())]
    );
}
