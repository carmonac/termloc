use chrono::{Local, Utc};
use persistence::model::ChargedUsageTotals;
use warp_cli::agent::Harness;
use warpui::App;

use super::ConversationDetailsData;
use crate::ai::agent::conversation::{AIConversation, AIConversationId};

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

#[test]
fn test_from_conversation_uses_charged_usage_dollar_total() {
    App::test((), |mut app| async move {
        let mut conversation = AIConversation::new(false, false);
        conversation.set_charged_usage_for_test(Some(ChargedUsageTotals {
            input_cost_in_cents: 10.0,
            output_cost_in_cents: 12.0,
            platform_cost_in_cents: 8.0,
            web_search_cost_in_cents: 6.0,
            ..Default::default()
        }));

        app.update(|ctx| {
            let data = ConversationDetailsData::from_conversation(&conversation, ctx);

            assert_eq!(data.cost_in_cents, Some(36.0));
        });
    });
}

/// Without streamed charges, the panel falls back to the server's cumulative billed snapshot;
/// the provider cost is never used.
#[test]
fn test_from_conversation_falls_back_to_the_billed_snapshot() {
    App::test((), |mut app| async move {
        let mut conversation = AIConversation::new(false, false);
        conversation.set_billed_cost_in_cents_for_test(Some(250.0));

        app.update(|ctx| {
            let data = ConversationDetailsData::from_conversation(&conversation, ctx);

            assert_eq!(data.cost_in_cents, Some(250.0));
        });
    });
}
