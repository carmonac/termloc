use std::collections::HashSet;

use futures::executor::block_on;

use super::*;
use crate::auth::UserUid;
use crate::server::ids::ClientId;

fn owner() -> Owner {
    Owner::User {
        user_uid: UserUid::new("local-user"),
    }
}

fn request(client_id: ClientId) -> CreateObjectRequest {
    CreateObjectRequest {
        serialized_model: None,
        title: Some("title".to_owned()),
        owner: owner(),
        client_id,
        initial_folder_id: None,
        entrypoint: crate::cloud_object::CloudObjectEventEntrypoint::Unknown,
    }
}

fn created(result: CreateCloudObjectResult) -> CreatedCloudObject {
    match result {
        CreateCloudObjectResult::Success {
            created_cloud_object,
        } => created_cloud_object,
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn created_objects_get_unique_ids_and_keep_their_owner() {
    let client = LocalObjectClient;
    let client_id = ClientId::default();

    let ids: HashSet<_> = (0..100)
        .map(|_| {
            let object = created(block_on(client.create_notebook(request(client_id))).unwrap());
            assert_eq!(object.client_id, client_id);
            assert_eq!(object.permissions.space, owner());
            assert_eq!(object.server_id_and_type.id_type, ObjectIdType::Notebook);
            object.server_id_and_type.id
        })
        .collect();

    assert_eq!(ids.len(), 100);
}

#[test]
fn changed_objects_response_never_deletes_local_objects() {
    let response =
        block_on(LocalObjectClient.fetch_changed_objects(ObjectsToUpdate::default(), true))
            .unwrap();

    assert!(response.deleted_notebooks.is_empty());
    assert!(response.deleted_workflows.is_empty());
    assert!(response.deleted_folders.is_empty());
    assert!(response.deleted_generic_string_objects.is_empty());
}

#[test]
fn sharing_is_rejected() {
    let id = ServerId::from(1);
    assert!(matches!(
        block_on(LocalObjectClient.leave_object(id)).unwrap(),
        ObjectDeleteResult::Failure
    ));
    assert!(
        block_on(LocalObjectClient.add_object_guests(
            id,
            vec!["someone@example.com".to_owned()],
            AccessLevel::Viewer
        ))
        .is_err()
    );
}
