use futures::executor::block_on;
use mockito::{Matcher, Server};
use serde_json::json;

use super::super::ServerApi;
use super::{
    AIClient, ArtifactDownloadResponse, ForkConversationResponse, build_fork_conversation_url,
};
use crate::ai::artifacts::Artifact;
use crate::notebooks::NotebookId;

#[test]
fn block_snapshot_resolves_url_before_deserializing_storage_bytes() {
    let block =
        super::SerializedBlock::new_for_test(b"command".to_vec(), b"\x1b[32moutput".to_vec());
    let mut storage = Server::new();
    let storage_request = storage
        .mock("GET", "/block_snapshot.json")
        .match_query(Matcher::Exact("X-Amz-Signature=test%2Bsignature".into()))
        .match_header("x-warp-os-category", Matcher::Missing)
        .with_status(200)
        .with_body(block.to_json().unwrap())
        .create();
    let api_request = {
        let mut server = warp_core::channel::ChannelState::mock_server();
        server.mock("GET", "/api/v1/agent/conversations/test-block-download/block-snapshot")
            .match_query(Matcher::UrlEncoded("redirect".into(), "false".into()))
            .with_status(200)
            .with_body(json!({"url": format!("{}/block_snapshot.json?X-Amz-Signature=test%2Bsignature", storage.url())}).to_string())
            .create()
    };
    let result = block_on(ServerApi::new_for_test().get_block_snapshot(
        super::ServerConversationToken::new("test-block-download".into()),
    ))
    .unwrap();
    assert_eq!(result.stylized_command, block.stylized_command);
    assert_eq!(result.stylized_output, block.stylized_output);
    api_request.assert();
    storage_request.assert();
}

#[test]
fn test_deserialize_file_artifact_download_response() {
    let json = r#"{
        "artifact_uid": "artifact-123",
        "artifact_type": "FILE",
        "created_at": "2024-01-15T10:30:00Z",
        "data": {
            "download_url": "https://storage.example.com/report.txt",
            "expires_at": "2024-01-15T11:30:00Z",
            "content_type": "text/plain",
            "filepath": "outputs/report.txt",
            "filename": "report.txt",
            "description": "daily summary",
            "size_bytes": 42
        }
    }"#;

    let artifact: ArtifactDownloadResponse = serde_json::from_str(json).unwrap();

    let ArtifactDownloadResponse::File { common, data } = artifact else {
        panic!("expected File artifact download response");
    };
    assert_eq!(common.artifact_uid, "artifact-123");
    assert_eq!(common.created_at.to_rfc3339(), "2024-01-15T10:30:00+00:00");
    assert_eq!(data.download_url, "https://storage.example.com/report.txt");
    assert_eq!(data.expires_at.to_rfc3339(), "2024-01-15T11:30:00+00:00");
    assert_eq!(data.content_type, "text/plain");
    assert_eq!(data.filepath, "outputs/report.txt");
    assert_eq!(data.filename, "report.txt");
    assert_eq!(data.description.as_deref(), Some("daily summary"));
    assert_eq!(data.size_bytes, Some(42));
}

#[test]
fn test_deserialize_screenshot_artifact_download_response() {
    let json = r#"{
        "artifact_uid": "screenshot-123",
        "artifact_type": "SCREENSHOT",
        "created_at": "2024-01-15T10:30:00Z",
        "data": {
            "download_url": "https://storage.example.com/screenshot.png",
            "expires_at": "2024-01-15T11:30:00Z",
            "content_type": "image/png",
            "description": "dashboard screenshot"
        }
    }"#;

    let artifact: ArtifactDownloadResponse = serde_json::from_str(json).unwrap();

    let ArtifactDownloadResponse::Screenshot { common, data } = artifact else {
        panic!("expected Screenshot artifact download response");
    };
    assert_eq!(common.artifact_uid, "screenshot-123");
    assert_eq!(common.created_at.to_rfc3339(), "2024-01-15T10:30:00+00:00");
    assert_eq!(
        data.download_url,
        "https://storage.example.com/screenshot.png"
    );
    assert_eq!(data.expires_at.to_rfc3339(), "2024-01-15T11:30:00+00:00");
    assert_eq!(data.content_type, "image/png");
    assert_eq!(data.description.as_deref(), Some("dashboard screenshot"));
}

#[test]
fn test_deserialize_plan_artifact() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PLAN",
        "data": {
            "document_uid": "doc-uid-123",
            "notebook_uid": "1234567890123456789012",
            "title": "My Plan"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::Plan {
        document_uid,
        notebook_uid,
        title,
    } = &artifact
    else {
        panic!("expected Plan artifact");
    };
    assert_eq!(document_uid, "doc-uid-123");
    assert_eq!(
        notebook_uid.as_ref().map(|n| n.to_string()),
        Some("1234567890123456789012".to_string())
    );
    assert_eq!(*title, Some("My Plan".to_string()));
}

#[test]
fn test_deserialize_pull_request_artifact() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PULL_REQUEST",
        "data": {
            "url": "https://github.com/org/repo/pull/42",
            "branch": "feature-branch"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::PullRequest {
        url,
        branch,
        repo,
        number,
    } = &artifact
    else {
        panic!("expected PullRequest artifact");
    };
    assert_eq!(url, "https://github.com/org/repo/pull/42");
    assert_eq!(branch, "feature-branch");
    assert_eq!(*repo, Some("repo".to_string()));
    assert_eq!(*number, Some(42));
}

#[test]
fn test_deserialize_pull_request_non_github_url() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PULL_REQUEST",
        "data": {
            "url": "https://gitlab.com/org/repo/merge_requests/42",
            "branch": "feature-branch"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::PullRequest { repo, number, .. } = &artifact else {
        panic!("expected PullRequest artifact");
    };
    assert_eq!(*repo, None);
    assert_eq!(*number, None);
}

#[test]
fn test_deserialize_plan_artifact_with_optional_fields_missing() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PLAN",
        "data": {
            "document_uid": "doc-uid-123",
            "notebook_uid": "abcdefghijklmnopqrstuv"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::Plan {
        document_uid,
        notebook_uid,
        title,
    } = &artifact
    else {
        panic!("expected Plan artifact");
    };
    assert_eq!(document_uid, "doc-uid-123");
    assert_eq!(
        notebook_uid.as_ref().map(|n| n.to_string()),
        Some("abcdefghijklmnopqrstuv".to_string())
    );
    assert!(title.is_none());
}

#[test]
fn test_deserialize_artifact_missing_data_field() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PLAN"
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing field"));
}

#[test]
fn test_deserialize_artifact_invalid_plan_data() {
    // Missing required `document_uid` field should fail deserialization
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PLAN",
        "data": {
            "title": "Only title, no document_uid"
        }
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing field"));
}

#[test]
fn test_deserialize_artifact_invalid_pr_data() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PULL_REQUEST",
        "data": {
            "url": "https://github.com/org/repo/pull/1"
        }
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing field"));
}

#[test]
fn test_deserialize_artifact_unknown_variant() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "UNKNOWN_TYPE",
        "data": {
            "some_field": "value"
        }
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("unknown variant"));
}

// ---------------------------------------------------------------------------------------------------------------------
//  We test roundtripping serialize and deserialize since we use this for persisting artifacts for local conversations.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn test_artifact_plan_serialize_deserialize_roundtrip() {
    let original = Artifact::Plan {
        document_uid: "doc-123".to_string(),
        notebook_uid: Some(NotebookId::from("notebook12345678901234".to_string())),
        title: Some("My Plan".to_string()),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}

#[test]
fn test_artifact_plan_serialize_deserialize_roundtrip_no_notebook_uid() {
    let original = Artifact::Plan {
        document_uid: "doc-123".to_string(),
        notebook_uid: None,
        title: Some("My Plan".to_string()),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}

#[test]
fn test_artifact_pr_serialize_deserialize_roundtrip() {
    let original = Artifact::PullRequest {
        url: "https://github.com/org/repo/pull/42".to_string(),
        branch: "feature-branch".to_string(),
        repo: Some("repo".to_string()),
        number: Some(42),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    // repo/number are re-derived from URL on deserialize, so should match
    assert_eq!(original, deserialized);
}

#[test]
fn test_artifact_file_serialize_deserialize_roundtrip() {
    let original = Artifact::File {
        artifact_uid: "artifact-file-1".to_string(),
        filepath: "outputs/report.txt".to_string(),
        filename: "report.txt".to_string(),
        mime_type: "text/plain".to_string(),
        description: Some("Daily summary".to_string()),
        size_bytes: Some(42),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}

#[test]
fn test_artifact_vec_serialize_deserialize_roundtrip() {
    let original = vec![
        Artifact::Plan {
            document_uid: "doc-1".to_string(),
            notebook_uid: None,
            title: Some("Plan 1".to_string()),
        },
        Artifact::PullRequest {
            url: "https://github.com/org/repo/pull/1".to_string(),
            branch: "main".to_string(),
            repo: Some("repo".to_string()),
            number: Some(1),
        },
        Artifact::File {
            artifact_uid: "artifact-file-1".to_string(),
            filepath: "outputs/report.txt".to_string(),
            filename: "report.txt".to_string(),
            mime_type: "text/plain".to_string(),
            description: Some("Daily summary".to_string()),
            size_bytes: Some(42),
        },
    ];

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Vec<Artifact> = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}

#[test]
fn build_fork_conversation_url_routes_to_conversation_fork() {
    assert_eq!(
        build_fork_conversation_url("550e8400-e29b-41d4-a716-446655440000"),
        "agent/conversations/550e8400-e29b-41d4-a716-446655440000/fork"
    );
}

#[test]
fn build_fork_conversation_url_escapes_path_param() {
    assert_eq!(
        build_fork_conversation_url("conversation/with spaces"),
        "agent/conversations/conversation%2Fwith%20spaces/fork"
    );
}

#[test]
fn deserialize_fork_conversation_response() {
    let response: ForkConversationResponse = serde_json::from_value(serde_json::json!({
        "forked_conversation_id": "abcdef01-2345-6789-abcd-ef0123456789",
    }))
    .unwrap();
    assert_eq!(
        response.forked_conversation_id,
        "abcdef01-2345-6789-abcd-ef0123456789"
    );
}
