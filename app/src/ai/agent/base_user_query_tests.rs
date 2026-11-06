use warp_multi_agent_api as api;

use super::BaseUserQuery;

#[test]
fn debug_output_reports_shape_without_content() {
    let query = api::request::input::UserQuery {
        query: "the secret launch codes are 1234".to_string(),
        origin: Some(api::UserQueryOrigin::default()),
        ..Default::default()
    };

    let debug = format!("{:?}", BaseUserQuery::from_proto(query));

    assert!(!debug.contains("secret"), "{debug}");
    assert!(debug.contains("query_len: 32"), "{debug}");
    assert!(debug.contains("has_origin: true"), "{debug}");
    assert!(debug.contains("has_author: false"), "{debug}");
}

fn external_author() -> api::QueryAuthor {
    api::QueryAuthor {
        principal: Some(api::query_author::Principal::User(api::WarpUser {
            uid: "external-author".into(),
            email: "author@example.com".into(),
            team_uid: "author-team".into(),
        })),
        resolution: api::IdentityResolution::ExternalAccountBinding.into(),
    }
}

fn external_source() -> api::ExternalMessage {
    api::ExternalMessage {
        body: "new message".into(),
        platform: Some(api::external_message::Platform::Slack(
            api::external_message::Slack {
                channel_id: "channel".into(),
                thread_ts: "thread".into(),
                thread_history: vec![api::external_message::slack::ThreadMessage {
                    text: "earlier context".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

fn external_origin() -> api::UserQueryOrigin {
    api::UserQueryOrigin {
        variant: Some(api::user_query_origin::Variant::ExternalPlatform(
            api::user_query_origin::ExternalPlatform {},
        )),
    }
}

#[test]
fn from_message_lifts_only_the_attribution_and_skips_unattributed_messages() {
    assert!(BaseUserQuery::from_message(&api::message::UserQuery::default()).is_none());

    let message = api::message::UserQuery {
        query: "formatted follow-up".into(),
        mode: Some(api::UserQueryMode { r#type: None }),
        origin: Some(external_origin()),
        author: Some(external_author()),
        source_message: Some(external_source()),
        ..Default::default()
    };

    let lifted = BaseUserQuery::from_message(&message)
        .expect("an attributed message is lifted")
        .to_proto();

    assert_eq!(lifted.origin, Some(external_origin()));
    assert_eq!(lifted.author, Some(external_author()));
    assert_eq!(lifted.source_message, Some(external_source()));
    assert!(
        lifted.query.is_empty(),
        "text and mode are the live input's, not the message's"
    );
    assert!(lifted.mode.is_none());
}

#[test]
fn from_message_keeps_an_unresolved_sender() {
    let message = api::message::UserQuery {
        author: Some(api::QueryAuthor {
            principal: None,
            resolution: api::IdentityResolution::Unresolved.into(),
        }),
        ..Default::default()
    };

    let lifted = BaseUserQuery::from_message(&message).unwrap().to_proto();

    assert_eq!(
        lifted.author.unwrap().resolution,
        api::IdentityResolution::Unresolved as i32
    );
}
