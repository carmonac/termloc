use warp_multi_agent_api as api;
use warp_util::local_or_remote_path::LocalOrRemotePath;

use super::{
    SkillConversionError, SkillPathOrigin, skill_reference_from_api_skill_ref,
    skill_reference_from_read_skill_ref,
};
use crate::skills::{ParsedSkill, SkillReference};

fn api_project_skill(path: &str) -> api::Skill {
    api::Skill {
        descriptor: Some(api::SkillDescriptor {
            skill_reference: Some(api::skill_descriptor::SkillReference::Path(
                path.to_string(),
            )),
            name: "deploy".to_string(),
            description: "Deploy the service".to_string(),
            scope: Some(api::skill_descriptor::Scope {
                r#type: Some(api::skill_descriptor::scope::Type::Project(())),
            }),
            provider: Some(api::skill_descriptor::Provider {
                r#type: Some(api::skill_descriptor::provider::Type::Agents(())),
            }),
        }),
        content: Some(api::FileContent {
            file_path: path.to_string(),
            content: "# Deploy".to_string(),
            line_range: None,
        }),
    }
}

#[test]
fn try_from_api_with_unavailable_origin_rejects_path_based_skills() {
    let error = ParsedSkill::try_from_api_with_origin(
        api_project_skill("/repo/.agents/skills/deploy/SKILL.md"),
        &SkillPathOrigin::Unavailable,
    )
    .expect_err("restored skills without host context should not fabricate local paths");

    assert!(matches!(error, SkillConversionError::PathOriginUnavailable));
}

#[test]
fn skill_ref_with_unavailable_origin_preserves_bundled_skills() {
    let skill_reference = skill_reference_from_api_skill_ref(
        api::SkillRef {
            skill_reference: Some(api::skill_ref::SkillReference::BundledSkillId(
                "review-comments".to_string(),
            )),
        },
        &SkillPathOrigin::Unavailable,
    );

    assert_eq!(
        skill_reference,
        Some(SkillReference::BundledSkillId(
            "review-comments".to_string()
        ))
    );
}

#[test]
fn local_origin_normalizes_double_leading_slash() {
    let result = SkillPathOrigin::Local
        .location_for_path("//workspace/common-skills/.agents/skills/deploy/SKILL.md")
        .expect("double-slash path should be accepted");

    let LocalOrRemotePath::Local(path) = result;
    assert_eq!(
        path.to_str().unwrap(),
        "/workspace/common-skills/.agents/skills/deploy/SKILL.md"
    );
}

#[test]
fn local_origin_normalizes_multiple_slashes() {
    let result = SkillPathOrigin::Local
        .location_for_path("///workspace///skills///SKILL.md")
        .expect("multi-slash path should be accepted");

    let LocalOrRemotePath::Local(path) = result;
    assert_eq!(path.to_str().unwrap(), "/workspace/skills/SKILL.md");
}

#[test]
fn local_origin_preserves_normal_absolute_path() {
    let result = SkillPathOrigin::Local
        .location_for_path("/workspace/.agents/skills/deploy/SKILL.md")
        .expect("normal path should be accepted");

    let LocalOrRemotePath::Local(path) = result;
    assert_eq!(
        path.to_str().unwrap(),
        "/workspace/.agents/skills/deploy/SKILL.md"
    );
}

#[test]
fn restored_display_origin_normalizes_double_leading_slash() {
    let result = SkillPathOrigin::RestoredDisplayOnly
        .location_for_path("//repo/.agents/skills/deploy/SKILL.md")
        .expect("double-slash path should be accepted");

    let LocalOrRemotePath::Local(path) = result;
    assert_eq!(
        path.to_str().unwrap(),
        "/repo/.agents/skills/deploy/SKILL.md"
    );
}

#[test]
fn read_skill_ref_with_local_origin_normalizes_double_slash() {
    let skill_reference = skill_reference_from_read_skill_ref(
        api::message::tool_call::read_skill::SkillReference::SkillPath(
            "//workspace/.agents/skills/deploy/SKILL.md".to_string(),
        ),
        &SkillPathOrigin::Local,
    )
    .expect("double-slash read_skill path should convert");

    let SkillReference::Path(LocalOrRemotePath::Local(path)) = skill_reference else {
        panic!("expected a local skill path");
    };
    assert_eq!(
        path.to_str().unwrap(),
        "/workspace/.agents/skills/deploy/SKILL.md"
    );
}
