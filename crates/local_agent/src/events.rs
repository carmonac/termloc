use std::time::SystemTime;

use prost_types::FieldMask;
use uuid::Uuid;
use warp_multi_agent_api::client_action::{
    Action, AddMessagesToTask, AppendToMessageContent, CreateTask, UpdateTaskDescription,
};
use warp_multi_agent_api::message::Message as MessageKind;
use warp_multi_agent_api::response_event::{
    ClientActions, StreamFinished, StreamInit, Type, stream_finished,
};
use warp_multi_agent_api::{ClientAction, Message, ResponseEvent, Task};

pub(crate) const AGENT_OUTPUT_TEXT_PATH: &str = "agent_output.text";
pub(crate) const AGENT_REASONING_TEXT_PATH: &str = "agent_reasoning.reasoning";

pub(crate) fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub(crate) fn init(conversation_id: String, request_id: String) -> ResponseEvent {
    event(Type::Init(StreamInit {
        conversation_id,
        request_id,
        run_id: String::new(),
    }))
}

pub(crate) fn create_task(task: Task) -> ResponseEvent {
    actions(vec![Action::CreateTask(CreateTask { task: Some(task) })])
}

pub(crate) fn finished(reason: stream_finished::Reason) -> ResponseEvent {
    event(Type::Finished(StreamFinished {
        reason: Some(reason),
        ..Default::default()
    }))
}

fn event(event_type: Type) -> ResponseEvent {
    ResponseEvent {
        r#type: Some(event_type),
    }
}

fn actions(actions: Vec<Action>) -> ResponseEvent {
    event(Type::ClientActions(ClientActions {
        actions: actions
            .into_iter()
            .map(|action| ClientAction {
                action: Some(action),
            })
            .collect(),
    }))
}

/// Builds the events of one turn, which all target the same task and request.
#[derive(Clone)]
pub(crate) struct TurnEvents {
    pub(crate) task_id: String,
    pub(crate) request_id: String,
}

impl TurnEvents {
    pub(crate) fn update_description(&self, description: String) -> ResponseEvent {
        actions(vec![Action::UpdateTaskDescription(UpdateTaskDescription {
            task_id: self.task_id.clone(),
            description,
        })])
    }

    pub(crate) fn message(&self, kind: MessageKind) -> Message {
        Message {
            id: new_id(),
            task_id: self.task_id.clone(),
            request_id: self.request_id.clone(),
            timestamp: Some(prost_types::Timestamp::from(SystemTime::now())),
            message: Some(kind),
            ..Default::default()
        }
    }

    pub(crate) fn add_messages(&self, messages: Vec<Message>) -> ResponseEvent {
        actions(vec![Action::AddMessagesToTask(AddMessagesToTask {
            task_id: self.task_id.clone(),
            messages,
        })])
    }

    pub(crate) fn append(
        &self,
        message_id: String,
        delta: MessageKind,
        path: &str,
    ) -> ResponseEvent {
        actions(vec![Action::AppendToMessageContent(
            AppendToMessageContent {
                task_id: self.task_id.clone(),
                message: Some(Message {
                    id: message_id,
                    task_id: self.task_id.clone(),
                    request_id: self.request_id.clone(),
                    message: Some(delta),
                    ..Default::default()
                }),
                mask: Some(FieldMask {
                    paths: vec![path.to_owned()],
                }),
            },
        )])
    }
}
