use super::{
    MarkerPersistenceAction, TuiOnboardingMarker, TuiOnboardingMarkers, TuiOnboardingMarkersState,
    marker_persistence_action,
};

#[test]
fn markers_are_unavailable_until_initial_load_resolves() {
    let mut markers = TuiOnboardingMarkers {
        state: TuiOnboardingMarkersState::Loading,
        persist_markers: false,
    };

    assert!(!markers.is_ready());
    assert!(!markers.take_available(TuiOnboardingMarker::FirstZeroState));
}

#[test]
fn absent_or_false_cloud_markers_are_available_and_true_markers_are_consumed() {
    let marker_is_available = |value: Option<bool>| value != Some(true);

    assert!(marker_is_available(None));
    assert!(marker_is_available(Some(false)));
    assert!(!marker_is_available(Some(true)));
}

#[test]
fn marker_consumption_is_monotonic() {
    let mut markers = TuiOnboardingMarkers::new_ready_for_test(true);

    assert!(markers.take_available(TuiOnboardingMarker::FirstZeroState));
    assert!(!markers.take_available(TuiOnboardingMarker::FirstZeroState));
}

#[test]
fn marker_persistence_creates_updates_or_skips_without_writing_false() {
    assert_eq!(
        marker_persistence_action(None),
        MarkerPersistenceAction::Create
    );
    assert_eq!(
        marker_persistence_action(Some(false)),
        MarkerPersistenceAction::Update
    );
    assert_eq!(
        marker_persistence_action(Some(true)),
        MarkerPersistenceAction::None
    );
}
