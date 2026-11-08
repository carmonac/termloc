use std::any::Any;

use cloud_objects::cloud_object::{GenericServerObject, ServerMetadata};
use cloud_objects::ids::ObjectUid;

use crate::{
    ServerAIExecutionProfile, ServerAIFact, ServerAmbientAgentEnvironment, ServerCloudAgentConfig,
    ServerEnvVarCollection, ServerFolder, ServerMCPServer, ServerNotebook, ServerPreference,
    ServerScheduledAmbientAgent, ServerTemplatableMCPServer, ServerWorkflow, ServerWorkflowEnum,
};

/// A cloud object from the server.
#[derive(Clone, Debug)]
pub enum ServerCloudObject {
    Notebook(ServerNotebook),
    Workflow(Box<ServerWorkflow>),
    Folder(ServerFolder),
    Preference(ServerPreference),
    EnvVarCollection(ServerEnvVarCollection),
    WorkflowEnum(ServerWorkflowEnum),
    AIFact(ServerAIFact),
    MCPServer(ServerMCPServer),
    AIExecutionProfile(ServerAIExecutionProfile),
    TemplatableMCPServer(ServerTemplatableMCPServer),
    AmbientAgentEnvironment(ServerAmbientAgentEnvironment),
    ScheduledAmbientAgent(ServerScheduledAmbientAgent),
    CloudAgentConfig(ServerCloudAgentConfig),
}

impl ServerCloudObject {
    pub fn metadata(&self) -> &ServerMetadata {
        match self {
            ServerCloudObject::Notebook(notebook) => &notebook.metadata,
            ServerCloudObject::Workflow(workflow) => &workflow.metadata,
            ServerCloudObject::Folder(folder) => &folder.metadata,
            ServerCloudObject::Preference(preferences) => &preferences.metadata,
            ServerCloudObject::EnvVarCollection(env_var_collection) => &env_var_collection.metadata,
            ServerCloudObject::WorkflowEnum(workflow_enum) => &workflow_enum.metadata,
            ServerCloudObject::AIFact(aifact) => &aifact.metadata,
            ServerCloudObject::MCPServer(mcp_server) => &mcp_server.metadata,
            ServerCloudObject::TemplatableMCPServer(templatable_mcp_server) => {
                &templatable_mcp_server.metadata
            }
            ServerCloudObject::AIExecutionProfile(ai_execution_profile) => {
                &ai_execution_profile.metadata
            }
            ServerCloudObject::AmbientAgentEnvironment(ambient_agent_environment) => {
                &ambient_agent_environment.metadata
            }
            ServerCloudObject::ScheduledAmbientAgent(scheduled_ambient_agent) => {
                &scheduled_ambient_agent.metadata
            }
            ServerCloudObject::CloudAgentConfig(cloud_agent_config) => &cloud_agent_config.metadata,
        }
    }

    pub fn uid(&self) -> ObjectUid {
        match self {
            ServerCloudObject::Notebook(notebook) => notebook.id.uid(),
            ServerCloudObject::Workflow(workflow) => workflow.id.uid(),
            ServerCloudObject::Folder(folder) => folder.id.uid(),
            ServerCloudObject::Preference(preferences) => preferences.id.uid(),
            ServerCloudObject::EnvVarCollection(env_var_collection) => env_var_collection.id.uid(),
            ServerCloudObject::WorkflowEnum(workflow_enum) => workflow_enum.id.uid(),
            ServerCloudObject::AIFact(aifact) => aifact.id.uid(),
            ServerCloudObject::MCPServer(mcp_server) => mcp_server.id.uid(),
            ServerCloudObject::AIExecutionProfile(ai_execution_profile) => {
                ai_execution_profile.id.uid()
            }
            ServerCloudObject::TemplatableMCPServer(templatable_mcp_server) => {
                templatable_mcp_server.id.uid()
            }
            ServerCloudObject::AmbientAgentEnvironment(ambient_agent_environment) => {
                ambient_agent_environment.id.uid()
            }
            ServerCloudObject::ScheduledAmbientAgent(scheduled_ambient_agent) => {
                scheduled_ambient_agent.id.uid()
            }
            ServerCloudObject::CloudAgentConfig(cloud_agent_config) => cloud_agent_config.id.uid(),
        }
    }
}

impl<K, M> From<&GenericServerObject<K, M>> for ServerCloudObject
where
    K: 'static,
    M: 'static,
{
    fn from(value: &GenericServerObject<K, M>) -> Self {
        let value = value as &dyn Any;
        if let Some(server_notebook) = value.downcast_ref::<ServerNotebook>() {
            ServerCloudObject::Notebook(server_notebook.clone())
        } else if let Some(server_workflow) = value.downcast_ref::<ServerWorkflow>() {
            ServerCloudObject::Workflow(Box::new(server_workflow.clone()))
        } else if let Some(server_folder) = value.downcast_ref::<ServerFolder>() {
            ServerCloudObject::Folder(server_folder.clone())
        } else if let Some(server_preferences) = value.downcast_ref::<ServerPreference>() {
            ServerCloudObject::Preference(server_preferences.clone())
        } else if let Some(server_env_var_collection) =
            value.downcast_ref::<ServerEnvVarCollection>()
        {
            ServerCloudObject::EnvVarCollection(server_env_var_collection.clone())
        } else if let Some(server_workflow_enum) = value.downcast_ref::<ServerWorkflowEnum>() {
            ServerCloudObject::WorkflowEnum(server_workflow_enum.clone())
        } else if let Some(server_aifact) = value.downcast_ref::<ServerAIFact>() {
            ServerCloudObject::AIFact(server_aifact.clone())
        } else if let Some(server_mcp_server) = value.downcast_ref::<ServerMCPServer>() {
            ServerCloudObject::MCPServer(server_mcp_server.clone())
        } else if let Some(server_ai_execution_profile) =
            value.downcast_ref::<ServerAIExecutionProfile>()
        {
            ServerCloudObject::AIExecutionProfile(server_ai_execution_profile.clone())
        } else if let Some(server_templatable_mcp_server) =
            value.downcast_ref::<ServerTemplatableMCPServer>()
        {
            ServerCloudObject::TemplatableMCPServer(server_templatable_mcp_server.clone())
        } else if let Some(server_ambient_agent_environment) =
            value.downcast_ref::<ServerAmbientAgentEnvironment>()
        {
            ServerCloudObject::AmbientAgentEnvironment(server_ambient_agent_environment.clone())
        } else if let Some(server_scheduled_ambient_agent) =
            value.downcast_ref::<ServerScheduledAmbientAgent>()
        {
            ServerCloudObject::ScheduledAmbientAgent(server_scheduled_ambient_agent.clone())
        } else if let Some(server_cloud_agent_config) =
            value.downcast_ref::<ServerCloudAgentConfig>()
        {
            ServerCloudObject::CloudAgentConfig(server_cloud_agent_config.clone())
        } else {
            panic!("Unknown server object type");
        }
    }
}
