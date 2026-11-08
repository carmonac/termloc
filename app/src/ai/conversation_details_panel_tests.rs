use chrono::{Local, Utc};
use warp_cli::agent::Harness;

use super::ConversationDetailsData;
use crate::ai::agent::conversation::AIConversationId;

#[test]
fn test_from_conversation_metadata_passes_harness_through() {
    for harness in [
        None,
        Some(Harness::Local),
        Some(Harness::Claude),
        Some(Harness::Gemini),
        Some(Harness::Unknown),
    ] {
        let data = ConversationDetailsData::from_conversation_metadata(
            AIConversationId::new(),
            "Title".to_string(),
            None,
            Utc::now().with_timezone(&Local),
            None,
            None,
            vec![],
            None,
            None,
            None,
            None,
            harness,
        );
        assert_eq!(
            data.harness, harness,
            "harness {harness:?} should pass through"
        );
    }
}
