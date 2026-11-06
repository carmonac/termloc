//! The `Request.Input.UserQuery` an agent prompt starts from.
//!
//! [`BaseUserQuery`] keeps the attribution of a persisted or streamed query. `api::convert_to`
//! writes the fields this client models (text, mode, intended agent) over it when the request is
//! sent. Every other field, including ones this client does not model (origin, author, source
//! message, ...), travels through untouched.

use std::fmt;

use warp_multi_agent_api as api;

/// Boxed for size only: the proto is ~1 KiB inline, which would bloat every `AIAgentInput` and
/// `QueuedQueryKind` variant (clippy `large_enum_variant`). Nothing shares it across threads.
#[derive(Clone, PartialEq)]
pub struct BaseUserQuery(Box<api::request::input::UserQuery>);

impl BaseUserQuery {
    pub(crate) fn from_proto(query: api::request::input::UserQuery) -> Self {
        Self(Box::new(query))
    }

    /// Lifts a persisted or streamed `Message.UserQuery`'s attribution (origin, author, source
    /// message, and nothing else) so a restored or live input keeps who was behind it when it
    /// is sent again. `None` when the message carries no attribution: historical queries stay
    /// unattributed rather than being re-stamped as the current user's.
    pub(crate) fn from_message(query: &api::message::UserQuery) -> Option<Self> {
        if query.origin.is_none() && query.author.is_none() && query.source_message.is_none() {
            return None;
        }
        Some(Self::from_proto(api::request::input::UserQuery {
            origin: query.origin.clone(),
            author: query.author.clone(),
            source_message: query.source_message.clone(),
            ..Default::default()
        }))
    }

    /// The decoded query, cloned so the outgoing request's user query can be written over it.
    pub(crate) fn to_proto(&self) -> api::request::input::UserQuery {
        (*self.0).clone()
    }
}

impl fmt::Debug for BaseUserQuery {
    /// Prints shape only: the query text, attachments, and attribution carry user content and
    /// identities that must not reach logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BaseUserQuery")
            .field("query_len", &self.0.query.len())
            .field("attachment_count", &self.0.referenced_attachments.len())
            .field("has_mode", &self.0.mode.is_some())
            .field("has_origin", &self.0.origin.is_some())
            .field("has_author", &self.0.author.is_some())
            .field("has_source_message", &self.0.source_message.is_some())
            .finish()
    }
}

/// The origin of a query a person typed in a Warp client.
pub(crate) fn warp_client_origin() -> api::UserQueryOrigin {
    api::UserQueryOrigin {
        variant: Some(api::user_query_origin::Variant::WarpClient(
            api::user_query_origin::WarpClient {},
        )),
    }
}

#[cfg(test)]
#[path = "base_user_query_tests.rs"]
mod tests;
