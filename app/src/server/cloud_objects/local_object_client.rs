//! An [`ObjectClient`] for builds without a server. Cloud objects are already written to SQLite
//! before they reach this client, so it only answers the way the server would.

use std::collections::HashMap;

use anyhow::{Result, bail};
use async_channel::Sender;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cloud_object_client::{
    GetCloudObjectResponse, InitialLoadResponse, ObjectPermissionUpdateResult,
};
use uuid::Uuid;
use warp_graphql::object_permissions::AccessLevel;
use warp_graphql::scalars::time::ServerTimestamp;

use crate::cloud_object::model::actions::{
    ObjectAction, ObjectActionHistory, ObjectActionSubtype, ObjectActionType,
};
use crate::cloud_object::model::generic_string_model::GenericStringObjectId;
use crate::cloud_object::{
    BulkCreateCloudObjectResult, BulkCreateGenericStringObjectsRequest, CreateCloudObjectResult,
    CreateObjectRequest, CreatedCloudObject, GenericStringObjectFormat,
    GenericStringObjectUniqueKey, ObjectDeleteResult, ObjectIdType, ObjectMetadataUpdateResult,
    ObjectPermissionsUpdateData, ObjectType, ObjectsToUpdate, Owner, Revision,
    RevisionAndLastEditor, ServerFolder, ServerMetadata, ServerNotebook, ServerObject,
    ServerPermissions, ServerWorkflow, UpdateCloudObjectResult,
};
use crate::drive::folders::FolderId;
use crate::drive::sharing::SharingAccessLevel;
use crate::notebooks::NotebookId;
use crate::server::cloud_objects::listener::ObjectUpdateMessage;
use crate::server::ids::{ServerId, ServerIdAndType, SyncId};
use crate::server::server_api::object::{GuestIdentifier, ObjectClient};
use crate::server::sync_queue::SerializedModel;
use crate::workflows::WorkflowId;

/// The length of a server ID, which is shorter than a UUID in hexadecimal.
const SERVER_ID_LENGTH: usize = 22;
const SHARING_UNAVAILABLE: &str = "Sharing is not available in a local-only build";

#[derive(Default)]
pub struct LocalObjectClient;

fn new_server_id() -> ServerId {
    let id = Uuid::new_v4().simple().to_string();
    ServerId::from_string_lossy(&id[..SERVER_ID_LENGTH])
}

fn created_object(
    client_id: crate::server::ids::ClientId,
    id_type: ObjectIdType,
    owner: Owner,
) -> CreatedCloudObject {
    let now = Utc::now();
    CreatedCloudObject {
        client_id,
        revision_and_editor: new_revision(),
        metadata_ts: now.into(),
        server_id_and_type: ServerIdAndType {
            id: new_server_id(),
            id_type,
        },
        creator_uid: None,
        permissions: ServerPermissions {
            space: owner,
            guests: Vec::new(),
            anyone_link_sharing: None,
            permissions_last_updated_ts: now.into(),
        },
    }
}

fn created_result(request: CreateObjectRequest, id_type: ObjectIdType) -> CreateCloudObjectResult {
    CreateCloudObjectResult::Success {
        created_cloud_object: created_object(request.client_id, id_type, request.owner),
    }
}

fn now_revision() -> Revision {
    ServerTimestamp::new(Utc::now()).into()
}

fn new_revision() -> RevisionAndLastEditor {
    RevisionAndLastEditor {
        revision: now_revision(),
        last_editor_uid: None,
    }
}

fn updated<T>() -> Result<UpdateCloudObjectResult<T>> {
    Ok(UpdateCloudObjectResult::Success {
        revision_and_editor: new_revision(),
    })
}

/// Metadata for an update that only clears server-side state. [`UpdateManager`] keeps the fields
/// that the object already has locally.
///
/// [`UpdateManager`]: crate::server::cloud_objects::update_manager::UpdateManager
fn metadata_for(id: ServerId) -> ServerMetadata {
    ServerMetadata {
        uid: id,
        revision: now_revision(),
        metadata_last_updated_ts: Utc::now().into(),
        trashed_ts: None,
        folder_id: None,
        is_welcome_object: false,
        creator_uid: None,
        last_editor_uid: None,
        current_editor_uid: None,
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl ObjectClient for LocalObjectClient {
    async fn create_workflow(
        &self,
        request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult> {
        Ok(created_result(request, ObjectIdType::Workflow))
    }

    async fn update_workflow(
        &self,
        _workflow_id: WorkflowId,
        _data: SerializedModel,
        _revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<ServerWorkflow>> {
        updated()
    }

    async fn bulk_create_generic_string_objects(
        &self,
        owner: Owner,
        objects: &[BulkCreateGenericStringObjectsRequest],
    ) -> Result<BulkCreateCloudObjectResult> {
        Ok(BulkCreateCloudObjectResult::Success {
            created_cloud_objects: objects
                .iter()
                .map(|request| created_object(request.id, ObjectIdType::GenericStringObject, owner))
                .collect(),
        })
    }

    async fn create_generic_string_object(
        &self,
        _format: GenericStringObjectFormat,
        _uniqueness_key: Option<GenericStringObjectUniqueKey>,
        request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult> {
        Ok(created_result(request, ObjectIdType::GenericStringObject))
    }

    async fn create_notebook(
        &self,
        request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult> {
        Ok(created_result(request, ObjectIdType::Notebook))
    }

    async fn update_notebook(
        &self,
        _notebook_id: NotebookId,
        _title: Option<String>,
        _data: Option<SerializedModel>,
        _revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<ServerNotebook>> {
        updated()
    }

    async fn create_folder(&self, request: CreateObjectRequest) -> Result<CreateCloudObjectResult> {
        Ok(created_result(request, ObjectIdType::Folder))
    }

    async fn update_folder(
        &self,
        _folder_id: FolderId,
        _name: SerializedModel,
    ) -> Result<UpdateCloudObjectResult<ServerFolder>> {
        updated()
    }

    async fn update_generic_string_object(
        &self,
        _object_id: GenericStringObjectId,
        _model: SerializedModel,
        _revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<Box<dyn ServerObject>>> {
        updated()
    }

    async fn grab_notebook_edit_access(&self, notebook_id: NotebookId) -> Result<ServerMetadata> {
        Ok(metadata_for(notebook_id.into()))
    }

    async fn give_up_notebook_edit_access(
        &self,
        notebook_id: NotebookId,
    ) -> Result<ServerMetadata> {
        Ok(metadata_for(notebook_id.into()))
    }

    /// Stays pending for the life of the app: nothing else can change these objects.
    async fn get_warp_drive_updates(
        &self,
        _message_sender: Sender<ObjectUpdateMessage>,
        _stream_ready_sender: Sender<()>,
    ) -> Result<()> {
        futures::future::pending().await
    }

    async fn fetch_changed_objects(
        &self,
        _objects_to_update: ObjectsToUpdate,
        _force_refresh: bool,
    ) -> Result<InitialLoadResponse> {
        Ok(InitialLoadResponse::default())
    }

    async fn fetch_single_cloud_object(&self, _id: ServerId) -> Result<GetCloudObjectResponse> {
        bail!("Cloud object not found on this machine")
    }

    async fn transfer_notebook_owner(
        &self,
        _notebook_id: NotebookId,
        _owner: Owner,
    ) -> Result<bool> {
        Ok(false)
    }

    async fn transfer_workflow_owner(
        &self,
        _workflow_id: WorkflowId,
        _owner: Owner,
    ) -> Result<bool> {
        Ok(false)
    }

    async fn transfer_generic_string_object_owner(
        &self,
        _workflow_id: GenericStringObjectId,
        _owner: Owner,
    ) -> Result<bool> {
        Ok(false)
    }

    async fn trash_object(&self, _id: ServerId) -> Result<bool> {
        Ok(true)
    }

    async fn untrash_object(&self, id: ServerId) -> Result<ObjectMetadataUpdateResult> {
        Ok(ObjectMetadataUpdateResult::Success {
            metadata: Box::new(metadata_for(id)),
        })
    }

    async fn delete_object(&self, id: ServerId) -> Result<ObjectDeleteResult> {
        Ok(ObjectDeleteResult::Success {
            deleted_ids: vec![SyncId::ServerId(id)],
        })
    }

    /// [`UpdateManager`] works out what the trash holds, since only it can see the objects.
    ///
    /// [`UpdateManager`]: crate::server::cloud_objects::update_manager::UpdateManager
    async fn empty_trash(&self, _owner: Owner) -> Result<ObjectDeleteResult> {
        Ok(ObjectDeleteResult::Success {
            deleted_ids: Vec::new(),
        })
    }

    async fn move_object(
        &self,
        _id: ServerId,
        _folder_id: Option<FolderId>,
        _owner: Owner,
        _object_type: ObjectType,
    ) -> Result<bool> {
        Ok(true)
    }

    async fn record_object_action(
        &self,
        id: ServerId,
        action_type: ObjectActionType,
        timestamp: DateTime<Utc>,
        data: Option<String>,
    ) -> Result<ObjectActionHistory> {
        let now = Utc::now();
        let hashed_sqlite_id = id.to_string();
        Ok(ObjectActionHistory {
            uid: id.to_string(),
            hashed_sqlite_id: hashed_sqlite_id.clone(),
            latest_processed_at_timestamp: now,
            actions: vec![ObjectAction {
                action_type,
                uid: id.to_string(),
                hashed_sqlite_id,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp,
                    processed_at_timestamp: Some(now),
                    data,
                    pending: false,
                },
            }],
        })
    }

    async fn leave_object(&self, _id: ServerId) -> Result<ObjectDeleteResult> {
        Ok(ObjectDeleteResult::Failure)
    }

    async fn set_object_link_permissions(
        &self,
        _object_id: ServerId,
        _access_level: SharingAccessLevel,
    ) -> Result<ObjectPermissionUpdateResult> {
        Ok(ObjectPermissionUpdateResult::Failure)
    }

    async fn remove_object_link_permissions(
        &self,
        _object_id: ServerId,
    ) -> Result<ObjectPermissionUpdateResult> {
        Ok(ObjectPermissionUpdateResult::Failure)
    }

    async fn add_object_guests(
        &self,
        _object_id: ServerId,
        _guest_emails: Vec<String>,
        _access_level: AccessLevel,
    ) -> Result<ObjectPermissionsUpdateData> {
        bail!(SHARING_UNAVAILABLE)
    }

    async fn update_object_guests(
        &self,
        _object_id: ServerId,
        _guest_emails: Vec<String>,
        _access_level: AccessLevel,
    ) -> Result<ServerPermissions> {
        bail!(SHARING_UNAVAILABLE)
    }

    async fn remove_object_guest(
        &self,
        _object_id: ServerId,
        _guest: GuestIdentifier,
    ) -> Result<ServerPermissions> {
        bail!(SHARING_UNAVAILABLE)
    }

    async fn fetch_environment_last_task_run_timestamps(
        &self,
    ) -> Result<HashMap<String, DateTime<Utc>>> {
        Ok(HashMap::new())
    }
}

#[cfg(test)]
#[path = "local_object_client_tests.rs"]
mod tests;
