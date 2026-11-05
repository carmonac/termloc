//! Settings UI for the Ollama server and the models it offers.
use std::cell::RefCell;
use std::collections::HashMap;

use settings::Setting as _;
use url::Url;
use warp_errors::report_if_error;
use warpui::elements::{Element, MouseStateHandle};
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle};

use super::settings_page::{
    LocalOnlyIconState, MatchData, PageTitle, PageType, SettingsPageMeta, SettingsPageViewHandle,
    SettingsWidget, render_body_item,
};
use super::{SettingsSection, ToggleState};
use crate::ai::local::{LocalModels, LocalModelsState};
use crate::appearance::Appearance;
use crate::editor::{EditorView, Event as EditorEvent, SingleLineEditorOptions, TextOptions};
use crate::settings::{AISettings, LocalContextWindow, OllamaUrl};

const INPUT_WIDTH: f32 = 280.;
const MIN_CONTEXT_WINDOW: u32 = 2_048;

#[derive(Clone, Debug, PartialEq)]
pub enum LocalModelsPageAction {
    Refresh,
}

pub struct LocalModelsPageView {
    page: PageType<Self>,
    local_only_icon_tooltip_states: RefCell<HashMap<String, MouseStateHandle>>,
    url_editor: ViewHandle<EditorView>,
    context_window_editor: ViewHandle<EditorView>,
}

fn is_valid_url(text: &str) -> bool {
    Url::parse(text).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

fn parse_context_window(text: &str) -> Option<u32> {
    text.trim()
        .parse()
        .ok()
        .filter(|tokens| *tokens >= MIN_CONTEXT_WINDOW)
}

impl LocalModelsPageView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let url_editor = Self::add_editor(ctx);
        let context_window_editor = Self::add_editor(ctx);
        let settings = AISettings::as_ref(ctx);
        let url = settings.ollama_url.value().clone();
        let context_window = settings.local_context_window.value().to_string();
        url_editor.update(ctx, |editor, ctx| editor.set_buffer_text(&url, ctx));
        context_window_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&context_window, ctx);
        });

        ctx.subscribe_to_view(&url_editor, |me, _, event, ctx| {
            if matches!(event, EditorEvent::Enter | EditorEvent::Blurred) {
                me.save_url(ctx);
            }
        });
        ctx.subscribe_to_view(&context_window_editor, |me, _, event, ctx| {
            if matches!(event, EditorEvent::Enter | EditorEvent::Blurred) {
                me.save_context_window(ctx);
            }
        });
        ctx.subscribe_to_model(&LocalModels::handle(ctx), |_, _, _, ctx| ctx.notify());

        let widgets: Vec<Box<dyn SettingsWidget<View = Self>>> = vec![
            Box::new(OllamaUrlWidget),
            Box::new(ContextWindowWidget),
            Box::new(InstalledModelsWidget::default()),
        ];
        Self {
            page: PageType::new_uncategorized(widgets, Some(PageTitle::new("Local models"))),
            local_only_icon_tooltip_states: RefCell::new(HashMap::new()),
            url_editor,
            context_window_editor,
        }
    }

    fn add_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        let options = SingleLineEditorOptions {
            text: TextOptions::ui_font_size(Appearance::as_ref(ctx)),
            ..Default::default()
        };
        ctx.add_typed_action_view(|ctx| EditorView::single_line(options, ctx))
    }

    fn save_url(&mut self, ctx: &mut ViewContext<Self>) {
        let text = self.url_editor.as_ref(ctx).buffer_text(ctx);
        let text = text.trim().trim_end_matches('/').to_owned();
        let current = AISettings::as_ref(ctx).ollama_url.value().clone();
        if text == current {
            return;
        }
        if !is_valid_url(&text) {
            self.url_editor
                .update(ctx, |editor, ctx| editor.set_buffer_text(&current, ctx));
            return;
        }
        AISettings::handle(ctx).update(ctx, |settings, ctx| {
            report_if_error!(settings.ollama_url.set_value(text, ctx));
        });
    }

    fn save_context_window(&mut self, ctx: &mut ViewContext<Self>) {
        let text = self.context_window_editor.as_ref(ctx).buffer_text(ctx);
        let current = *AISettings::as_ref(ctx).local_context_window.value();
        match parse_context_window(&text) {
            Some(tokens) if tokens != current => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.local_context_window.set_value(tokens, ctx));
                });
            }
            Some(_) => {}
            None => self.context_window_editor.update(ctx, |editor, ctx| {
                editor.set_buffer_text(&current.to_string(), ctx);
            }),
        }
    }
}

impl Entity for LocalModelsPageView {
    type Event = ();
}

impl TypedActionView for LocalModelsPageView {
    type Action = LocalModelsPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            LocalModelsPageAction::Refresh => {
                LocalModels::handle(ctx).update(ctx, |models, ctx| models.refresh(ctx));
            }
        }
    }
}

impl View for LocalModelsPageView {
    fn ui_name() -> &'static str {
        "LocalModelsSettingsPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

impl SettingsPageMeta for LocalModelsPageView {
    fn section() -> SettingsSection {
        SettingsSection::LocalModels
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        true
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<LocalModelsPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<LocalModelsPageView>) -> Self {
        SettingsPageViewHandle::LocalModels(view_handle)
    }
}

fn render_input(editor: &ViewHandle<EditorView>, appearance: &Appearance) -> Box<dyn Element> {
    appearance
        .ui_builder()
        .text_input(editor.clone())
        .with_style(UiComponentStyles {
            width: Some(INPUT_WIDTH),
            padding: Some(Coords {
                top: 4.,
                bottom: 4.,
                left: 6.,
                right: 6.,
            }),
            background: Some(appearance.theme().surface_2().into()),
            ..Default::default()
        })
        .build()
        .finish()
}

struct OllamaUrlWidget;

impl SettingsWidget for OllamaUrlWidget {
    type View = LocalModelsPageView;

    fn search_terms(&self) -> &str {
        "local models ollama server url address host"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        render_body_item::<LocalModelsPageAction>(
            "Ollama URL".into(),
            None,
            LocalOnlyIconState::for_setting(
                OllamaUrl::storage_key(),
                OllamaUrl::sync_to_cloud(),
                &mut view.local_only_icon_tooltip_states.borrow_mut(),
                app,
            ),
            ToggleState::Enabled,
            appearance,
            render_input(&view.url_editor, appearance),
            Some("Where Ollama is running. Prompts and code are sent to this server.".to_owned()),
        )
    }
}

struct ContextWindowWidget;

impl SettingsWidget for ContextWindowWidget {
    type View = LocalModelsPageView;

    fn search_terms(&self) -> &str {
        "local models context window tokens memory size"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        render_body_item::<LocalModelsPageAction>(
            "Context window".into(),
            None,
            LocalOnlyIconState::for_setting(
                LocalContextWindow::storage_key(),
                LocalContextWindow::sync_to_cloud(),
                &mut view.local_only_icon_tooltip_states.borrow_mut(),
                app,
            ),
            ToggleState::Enabled,
            appearance,
            render_input(&view.context_window_editor, appearance),
            Some(
                "Maximum tokens per request. Larger values use more memory, and each model has \
                 its own limit."
                    .to_owned(),
            ),
        )
    }
}

#[derive(Default)]
struct InstalledModelsWidget {
    refresh_mouse_state: MouseStateHandle,
}

impl InstalledModelsWidget {
    fn description(app: &AppContext) -> String {
        let models = LocalModels::as_ref(app);
        match models.state() {
            LocalModelsState::Loading => "Checking Ollama…".to_owned(),
            LocalModelsState::Unavailable(message) => message.clone(),
            LocalModelsState::Ready => {
                let names: Vec<&str> = models
                    .models()
                    .iter()
                    .map(|model| model.name.as_str())
                    .collect();
                format!(
                    "Installed models with tool support: {}. Choose one in the agent's model \
                     menu.",
                    names.join(", ")
                )
            }
        }
    }
}

impl SettingsWidget for InstalledModelsWidget {
    type View = LocalModelsPageView;

    fn search_terms(&self) -> &str {
        "local models ollama installed refresh status qwen llama"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let button = appearance
            .ui_builder()
            .button(ButtonVariant::Secondary, self.refresh_mouse_state.clone())
            .with_text_label("Refresh".to_owned())
            .build()
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(LocalModelsPageAction::Refresh);
            })
            .finish();
        render_body_item::<LocalModelsPageAction>(
            "Models".into(),
            None,
            // Not rendering a setting, so no need to show local only icon state.
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            button,
            Some(Self::description(app)),
        )
    }
}

#[cfg(test)]
#[path = "local_models_page_tests.rs"]
mod tests;
