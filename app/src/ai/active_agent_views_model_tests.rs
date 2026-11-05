use warpui::{App, EntityId, WindowId};

use super::*;

fn setup_model(app: &mut App) -> ModelHandle<ActiveAgentViewsModel> {
    app.add_singleton_model(|_| ActiveAgentViewsModel::new())
}

#[test]
fn focus_change_without_task_id_has_no_conversation() {
    App::test((), |mut app| async move {
        let model = setup_model(&mut app);
        let window = WindowId::new();
        let terminal = EntityId::new();

        // No agent view handles registered, no task_id → active_conversation_id should be None.
        model.update(&mut app, |model, ctx| {
            model.handle_pane_focus_change(window, Some(terminal), ctx);
        });

        model.read(&app, |model, _| {
            assert_eq!(model.get_focused_conversation(window), None);
        });
    });
}
