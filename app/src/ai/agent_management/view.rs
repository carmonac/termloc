use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use fuzzy_match::match_indices_case_insensitive;
use lazy_static::lazy_static;
use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use siphasher::sip::SipHasher;
use warp_cli::agent::Harness;
use warp_core::features::FeatureFlag;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::Fill;
use warp_core::ui::theme::color::internal_colors;
use warpui::clipboard::ClipboardContent;
use warpui::elements::new_scrollable::{
    NewScrollableElement, ScrollableAppearance, SingleAxisConfig,
};
use warpui::elements::{
    Align, Border, ChildAnchor, ChildView, Clipped, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, Element, Empty, Expanded, Flex, Hoverable, List, ListState, MainAxisSize,
    MouseStateHandle, NewScrollable, OffsetPositioning, Padding, ParentAnchor, ParentElement,
    ParentOffsetBounds, Radius, Rect, ScrollStateHandle, ScrollbarWidth, Shrinkable,
    SizeConstraintCondition, SizeConstraintSwitch, Stack, Text, Wrap,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::scene::DropShadow;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Entity, FocusContext, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle, WeakViewHandle,
};

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent_conversations_model::{
    AgentConversationEntry, AgentConversationEntryId, AgentConversationNavigationSubject,
    AgentConversationsModel, AgentConversationsModelEvent, AgentManagementFilters, ArtifactFilter,
    ConversationUpdateKind, CreatedOnFilter, StatusFilter,
};
use crate::ai::agent_management::details_action_buttons::{
    ActionButtonsConfig, AgentDetailsButtonEvent, ConversationActionButtonsRow,
};
use crate::ai::artifacts::{Artifact, ArtifactButtonsRow, ArtifactButtonsRowEvent};
use crate::ai::conversation_details_panel::{
    ConversationDetailsData, ConversationDetailsPanel, ConversationDetailsPanelEvent,
};
use crate::ai::harness_display;
use crate::app_state::PersistedAgentManagementFilters;
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys,
    PropagateHorizontalNavigationKeys, SingleLineEditorOptions, TextOptions,
};
use crate::menu::{MenuItem, MenuItemFields};
use crate::notebooks::NotebookId;
use crate::ui_components::agent_icon::agent_conversation_entry_icon_variant;
use crate::ui_components::avatar::{Avatar, AvatarContent};
use crate::ui_components::icon_with_status::render_icon_with_status;
use crate::util::time_format::format_approx_duration_from_now_utc;
use crate::view_components::DismissibleToast;
use crate::view_components::action_button::{
    ActionButton, ButtonSize, NakedTheme, PrimaryTheme, SecondaryTheme,
};
use crate::view_components::compactible_action_button::{
    CompactibleActionButton, MEDIUM_SIZE_SWITCH_THRESHOLD,
};
use crate::view_components::dropdown::{
    Dropdown, DropdownAction, DropdownItemAction, DropdownStyle,
};
use crate::workspace::{
    ForkedConversationDestination, RestoreConversationLayout, ToastStack, WorkspaceAction,
};

lazy_static! {
    static ref HASHER: SipHasher = SipHasher::new_with_keys(0, 0);
}

const MANAGEMENT_PANEL_WIDTH: f32 = 400.;
// Vertical margin for filter row elements to align with dropdown buttons
const FILTER_ROW_VERTICAL_MARGIN: f32 = 6.;

const CARD_ROW_SPACING: f32 = 8.;
const CARD_CONTENT_PADDING: f32 = 12.;
const CARD_BORDER_RADIUS: f32 = 4.;
const CARD_MARGIN_BOTTOM: f32 = 8.;

const BUTTON_SIZE: f32 = 20.;
/// Total size of the agent icon-with-status component rendered in each card's header row.
const CARD_AGENT_ICON_SIZE: f32 = 24.;
const CREATOR_AVATAR_FONT_SIZE: f32 = 10.;

pub fn init(app: &mut AppContext) {
    use crate::util::bindings::cmd_or_ctrl_shift;

    app.register_fixed_bindings([FixedBinding::new(
        cmd_or_ctrl_shift("f"),
        AgentManagementViewAction::FocusSearch,
        warpui::keymap::macros::id!(AgentManagementView::ui_name()),
    )]);
}

fn should_show_artifacts(artifacts: &[Artifact]) -> bool {
    !artifacts.is_empty() && FeatureFlag::ConversationArtifacts.is_enabled()
}

pub type ManagementCardItemId = AgentConversationEntryId;

/// Store state for a given task row
struct CardState {
    hover_state: MouseStateHandle,
    avatar_hover_state: MouseStateHandle,
    artifact_buttons_view: Option<ViewHandle<ArtifactButtonsRow>>,
    action_buttons_hover_state: MouseStateHandle,
    action_buttons_view: ViewHandle<ConversationActionButtonsRow>,
    /// Use this ID to look up the full data from the model
    item_id: ManagementCardItemId,
}

pub struct AgentManagementView {
    list_state: ListState<()>,
    loading_icon_mouse_state: MouseStateHandle,
    scroll_state: ScrollStateHandle,

    /// Store the most recent requested set of ConversationOrTasks on the view
    items: Vec<CardState>,

    /// Store filters on the data
    filters: AgentManagementFilters,

    /// Search query for filtering by title
    search_query: String,
    search_editor: ViewHandle<EditorView>,

    new_agent_button: CompactibleActionButton,

    status_dropdown: ViewHandle<Dropdown<AgentManagementViewAction>>,
    created_on_dropdown: ViewHandle<Dropdown<AgentManagementViewAction>>,
    artifact_dropdown: ViewHandle<Dropdown<AgentManagementViewAction>>,
    clear_all_filters_button: ViewHandle<ActionButton>,
    no_filter_results_button: ViewHandle<ActionButton>,

    /// Details panel for showing task/conversation metadata
    details_panel: ViewHandle<ConversationDetailsPanel>,
    /// Currently selected item ID (for rendering details)
    selected_item_id: Option<ManagementCardItemId>,
}

/// Enum to track the state of the view, based on what tasks we have visible
enum ViewState {
    /// The model is still loading in data
    Loading,
    /// We have tasks, but currently have a filter applied that matches none of them
    NoFilterMatches,
    /// We have tasks that should be shown to the user
    HasTasks,
}

impl AgentManagementView {
    pub fn new(
        persisted_filters: Option<PersistedAgentManagementFilters>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(
            &AgentConversationsModel::handle(ctx),
            Self::handle_agent_management_model_event,
        );

        let view_handle = ctx.handle();
        let list_state = Self::construct_fresh_list_state(view_handle.clone());

        // Set up dropdowns
        let status_dropdown = ctx.add_typed_action_view(Self::create_status_dropdown);
        let created_on_dropdown = ctx.add_typed_action_view(Self::create_created_on_dropdown);
        let artifact_dropdown = ctx.add_typed_action_view(Self::create_artifact_dropdown);

        let no_filter_results_button = ctx.add_typed_action_view(move |_ctx| {
            ActionButton::new("Clear filters", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(AgentManagementViewAction::ClearFilters)
                })
        });

        let clear_all_filters_button = ctx.add_typed_action_view(move |_ctx| {
            ActionButton::new("Clear all", NakedTheme)
                .with_icon(Icon::X)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(AgentManagementViewAction::ClearFilters)
                })
        });

        let search_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let mut editor = EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions::ui_text(Some(appearance.ui_font_size()), appearance),
                    select_all_on_focus: true,
                    clear_selections_on_blur: true,
                    propagate_and_no_op_vertical_navigation_keys:
                        PropagateAndNoOpNavigationKeys::Always,
                    propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys::Always,
                    ..Default::default()
                },
                ctx,
            );
            editor.set_placeholder_text("Search", ctx);
            editor
        });
        ctx.subscribe_to_view(&search_editor, |me, _handle, event, ctx| {
            me.handle_search_editor_event(event, ctx);
        });

        let new_agent_button = CompactibleActionButton::new(
            "New agent".to_string(),
            None,
            ButtonSize::Small,
            AgentManagementViewAction::NewAgent,
            Icon::Plus,
            Arc::new(PrimaryTheme),
            ctx,
        );

        let filters = persisted_filters.map(|p| p.filters).unwrap_or_default();

        let details_panel: ViewHandle<ConversationDetailsPanel> =
            ctx.add_typed_action_view(|ctx| {
                ConversationDetailsPanel::new(true, MANAGEMENT_PANEL_WIDTH, ctx)
            });

        ctx.subscribe_to_view(&details_panel, Self::handle_details_panel_event);

        let mut view = Self {
            list_state,
            scroll_state: ScrollStateHandle::default(),
            items: Vec::new(),
            filters,
            search_query: String::new(),
            search_editor,
            loading_icon_mouse_state: MouseStateHandle::default(),
            status_dropdown,
            created_on_dropdown,
            artifact_dropdown,
            clear_all_filters_button,
            no_filter_results_button,
            new_agent_button,
            details_panel,
            selected_item_id: None,
        };

        view.sync_with_loaded_filters(ctx);
        view.get_tasks_from_model(ctx);
        view
    }

    fn get_view_state(&self, app: &AppContext) -> ViewState {
        let model = AgentConversationsModel::as_ref(app);
        // If loading with zero items, show skeleton cards
        // If loading with items, show list of interactive conversations (with loading indicator)
        if model.is_loading() && !model.has_items(app) {
            return ViewState::Loading;
        }

        if self.items.is_empty() {
            return ViewState::NoFilterMatches;
        }

        ViewState::HasTasks
    }

    /// Sync the UI to match the loaded filter states.
    fn sync_with_loaded_filters(&mut self, ctx: &mut ViewContext<Self>) {
        self.status_dropdown.update(ctx, |dropdown, ctx| {
            dropdown.set_selected_by_action(
                AgentManagementViewAction::SetStatusFilter(self.filters.status),
                ctx,
            );
        });

        self.created_on_dropdown.update(ctx, |dropdown, ctx| {
            dropdown.set_selected_by_action(
                AgentManagementViewAction::SetCreatedOnFilter(self.filters.created_on),
                ctx,
            );
        });

        self.artifact_dropdown.update(ctx, |dropdown, ctx| {
            dropdown.set_selected_by_action(
                AgentManagementViewAction::SetArtifactFilter(self.filters.artifact),
                ctx,
            );
        });
    }

    /// Get current filters for persistence.
    pub fn get_filters(&self) -> PersistedAgentManagementFilters {
        PersistedAgentManagementFilters {
            filters: self.filters.clone(),
        }
    }

    fn construct_fresh_list_state(handle: WeakViewHandle<Self>) -> ListState<()> {
        ListState::new(move |index, _scroll_offset, app| {
            let Some(view_handle) = handle.upgrade(app) else {
                return Empty::new().finish();
            };
            view_handle.as_ref(app).render_card_at_index(index, app)
        })
    }

    fn create_status_dropdown(
        ctx: &mut ViewContext<Dropdown<AgentManagementViewAction>>,
    ) -> Dropdown<AgentManagementViewAction> {
        let (magenta, green, red) = {
            let theme = Appearance::as_ref(ctx).theme();
            (
                theme.ansi_fg_magenta(),
                theme.ansi_fg_green(),
                theme.ansi_fg_red(),
            )
        };

        let mut dropdown = Dropdown::new(ctx);
        Self::setup_filter_menu(&mut dropdown, "Status", ctx);

        // Use this helper to make dropdown items with status icons
        let make_status_option =
            |label: &str, action: AgentManagementViewAction, icon_data: Option<(Icon, Fill)>| {
                let mut fields = MenuItemFields::new(label)
                    .with_on_select_action(DropdownAction::select_action_and_close(action));
                if let Some((icon, color)) = icon_data {
                    fields = fields.with_icon(icon).with_override_icon_color(color);
                }
                MenuItem::Item(fields)
            };

        let items = vec![
            make_status_option(
                "All",
                AgentManagementViewAction::SetStatusFilter(StatusFilter::All),
                None,
            ),
            make_status_option(
                "Working",
                AgentManagementViewAction::SetStatusFilter(StatusFilter::Working),
                Some((Icon::ClockLoader, Fill::from(magenta))),
            ),
            make_status_option(
                "Done",
                AgentManagementViewAction::SetStatusFilter(StatusFilter::Done),
                Some((Icon::Check, Fill::from(green))),
            ),
            make_status_option(
                "Failed",
                AgentManagementViewAction::SetStatusFilter(StatusFilter::Failed),
                Some((Icon::X, Fill::from(red))),
            ),
        ];

        dropdown.set_rich_items(items, ctx);
        dropdown.set_selected_by_index(0, ctx);
        dropdown
    }

    fn create_created_on_dropdown(
        ctx: &mut ViewContext<Dropdown<AgentManagementViewAction>>,
    ) -> Dropdown<AgentManagementViewAction> {
        let mut dropdown = Dropdown::new(ctx);
        Self::setup_filter_menu(&mut dropdown, "Created on", ctx);

        let items = vec![
            MenuItem::Item(MenuItemFields::new("All").with_on_select_action(
                DropdownAction::select_action_and_close(
                    AgentManagementViewAction::SetCreatedOnFilter(CreatedOnFilter::All),
                ),
            )),
            MenuItem::Item(MenuItemFields::new("Last 24 hours").with_on_select_action(
                DropdownAction::select_action_and_close(
                    AgentManagementViewAction::SetCreatedOnFilter(CreatedOnFilter::Last24Hours),
                ),
            )),
            MenuItem::Item(MenuItemFields::new("Past 3 days").with_on_select_action(
                DropdownAction::select_action_and_close(
                    AgentManagementViewAction::SetCreatedOnFilter(CreatedOnFilter::Past3Days),
                ),
            )),
            MenuItem::Item(MenuItemFields::new("Last week").with_on_select_action(
                DropdownAction::select_action_and_close(
                    AgentManagementViewAction::SetCreatedOnFilter(CreatedOnFilter::LastWeek),
                ),
            )),
        ];

        dropdown.set_rich_items(items, ctx);
        dropdown.set_selected_by_index(0, ctx);
        dropdown
    }

    fn create_artifact_dropdown(
        ctx: &mut ViewContext<Dropdown<AgentManagementViewAction>>,
    ) -> Dropdown<AgentManagementViewAction> {
        let mut dropdown = Dropdown::new(ctx);
        Self::setup_filter_menu(&mut dropdown, "Has artifact", ctx);

        let items = vec![
            MenuItem::Item(MenuItemFields::new("All").with_on_select_action(
                DropdownAction::select_action_and_close(
                    AgentManagementViewAction::SetArtifactFilter(ArtifactFilter::All),
                ),
            )),
            MenuItem::Item(MenuItemFields::new("Pull Request").with_on_select_action(
                DropdownAction::select_action_and_close(
                    AgentManagementViewAction::SetArtifactFilter(ArtifactFilter::PullRequest),
                ),
            )),
            MenuItem::Item(MenuItemFields::new("Plan").with_on_select_action(
                DropdownAction::select_action_and_close(
                    AgentManagementViewAction::SetArtifactFilter(ArtifactFilter::Plan),
                ),
            )),
            MenuItem::Item(MenuItemFields::new("Screenshot").with_on_select_action(
                DropdownAction::select_action_and_close(
                    AgentManagementViewAction::SetArtifactFilter(ArtifactFilter::Screenshot),
                ),
            )),
            MenuItem::Item(MenuItemFields::new("File").with_on_select_action(
                DropdownAction::select_action_and_close(
                    AgentManagementViewAction::SetArtifactFilter(ArtifactFilter::File),
                ),
            )),
        ];

        dropdown.set_rich_items(items, ctx);
        dropdown.set_selected_by_index(0, ctx);
        dropdown
    }

    // Initialize the dropdown menu for the filter dropdowns (status, source)
    fn setup_filter_menu<A: DropdownItemAction>(
        dropdown: &mut Dropdown<A>,
        label_prefix: &'static str,
        ctx: &mut ViewContext<Dropdown<A>>,
    ) {
        dropdown.set_menu_width(160., ctx);
        dropdown.set_main_axis_size(MainAxisSize::Min, ctx);
        dropdown.set_menu_header_text_override(move |text| format!("{}: {}", label_prefix, text));
        dropdown.set_style(DropdownStyle::ActionButtonSecondary, ctx);
    }

    fn handle_search_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        if let EditorEvent::Edited(_) = event {
            let new_query = self.search_editor.as_ref(ctx).buffer_text(ctx);
            if new_query != self.search_query {
                self.search_query = new_query;
                self.get_tasks_from_model(ctx);
            }
        }
    }

    /// Common handler for filter changes: refresh tasks and save.
    fn on_filter_changed(&mut self, ctx: &mut ViewContext<Self>) {
        self.get_tasks_from_model(ctx);
        ctx.dispatch_global_action("workspace:save_app", ());
    }

    /// Sync all tasks from the management model, and update the ListState
    fn get_tasks_from_model(&mut self, ctx: &mut ViewContext<Self>) {
        // Collect all card data we need
        struct CardData {
            item_id: ManagementCardItemId,
            artifacts: Vec<Artifact>,
            action_buttons_config: ActionButtonsConfig,
        }

        // Get sorted tasks and conversations from model
        let model = AgentConversationsModel::as_ref(ctx);
        let search_query = self.search_query.trim().to_lowercase();
        let cards: Vec<CardData> = model
            .get_entries(&self.filters, ctx)
            .into_iter()
            .filter(|entry| {
                if search_query.is_empty() {
                    return true;
                }
                match_indices_case_insensitive(&entry.display.title, &search_query).is_some()
            })
            .map(|entry| {
                let item_id = entry.id;
                let copy_link_url = entry
                    .capabilities
                    .can_copy_link
                    .then(|| {
                        AgentConversationsModel::resolve_copy_link(
                            AgentConversationNavigationSubject::Entry(entry.id),
                            ctx,
                        )
                    })
                    .flatten();
                let mut config = Self::action_buttons_config_for_entry(&entry, None, copy_link_url);
                if FeatureFlag::AgentManagementDetailsView.is_enabled() {
                    config.view_details_item_id = Some(item_id);
                }

                CardData {
                    item_id,
                    artifacts: entry.display.artifacts,
                    action_buttons_config: config,
                }
            })
            .collect();

        // Drain old state cards for reuse
        let mut old_items: HashMap<String, CardState> = self
            .items
            .drain(..)
            .map(|i| (i.item_id.as_key(), i))
            .collect();

        // Reset the list state, but save scroll position
        // We rebuild from scratch because items may have moved around in the view (if updated_at changes, for example)
        let current_scroll_index = self.list_state.get_scroll_index();
        let current_scroll_offset = self.list_state.get_scroll_offset();
        self.list_state = Self::construct_fresh_list_state(ctx.handle());

        // Rebuild the list state
        let mut new_items = Vec::with_capacity(cards.len());
        for card in cards {
            self.list_state.add_item();
            let card_key = card.item_id.as_key();

            match old_items.remove(&card_key) {
                Some(mut existing) => {
                    // Update artifacts view if it exists, or create if needed
                    if should_show_artifacts(&card.artifacts) {
                        if let Some(view) = &existing.artifact_buttons_view {
                            view.update(ctx, |v, ctx| v.update_artifacts(&card.artifacts, ctx));
                        } else {
                            existing.artifact_buttons_view =
                                Some(self.create_artifact_buttons_view(&card.artifacts, ctx));
                        }
                    } else {
                        existing.artifact_buttons_view = None;
                    }

                    existing.action_buttons_view.update(ctx, |row, ctx| {
                        row.set_config(card.action_buttons_config, ctx)
                    });

                    new_items.push(existing);
                }
                _ => {
                    let artifact_buttons_view = if should_show_artifacts(&card.artifacts) {
                        Some(self.create_artifact_buttons_view(&card.artifacts, ctx))
                    } else {
                        None
                    };
                    let action_buttons_view =
                        self.create_action_buttons_view(card.action_buttons_config, ctx);

                    new_items.push(CardState {
                        hover_state: MouseStateHandle::default(),
                        avatar_hover_state: MouseStateHandle::default(),
                        action_buttons_hover_state: MouseStateHandle::default(),
                        artifact_buttons_view,
                        action_buttons_view,
                        item_id: card.item_id,
                    });
                }
            }
        }

        // Restore approximate scroll position for the new list state
        let num_items = new_items.len();
        if num_items != 0 {
            if current_scroll_index > num_items {
                // If we have fewer items than we used to, scroll to the bottom of the list
                self.list_state.scroll_to(num_items - 1);
            } else {
                self.list_state
                    .scroll_to_with_offset(current_scroll_index, current_scroll_offset);
            }
        }

        self.items = new_items;
        ctx.notify();
    }

    fn create_action_buttons_view(
        &self,
        config: ActionButtonsConfig,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<ConversationActionButtonsRow> {
        let view = ctx.add_typed_action_view(ConversationActionButtonsRow::new);
        ctx.subscribe_to_view(&view, move |me, _, event, ctx| {
            me.handle_action_buttons_event(event, ctx);
        });
        view.update(ctx, |row, ctx| row.set_config(config, ctx));
        view
    }

    fn action_buttons_config_for_entry(
        entry: &AgentConversationEntry,
        open_action: Option<WorkspaceAction>,
        copy_link_url: Option<String>,
    ) -> ActionButtonsConfig {
        if let Some(conversation_id) = entry.identity.local_conversation_id {
            ActionButtonsConfig::for_conversation(conversation_id, open_action, copy_link_url)
        } else {
            ActionButtonsConfig {
                open_action,
                copy_link_url,
                ..Default::default()
            }
        }
    }

    fn handle_action_buttons_event(
        &mut self,
        event: &AgentDetailsButtonEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            AgentDetailsButtonEvent::Open => {
                // Open button only shown in details panel, not in management view cards.
                // We open the cards directly via clicking on them.
            }
            AgentDetailsButtonEvent::ForkConversation { conversation_id } => {
                ctx.dispatch_typed_action(&WorkspaceAction::ForkAIConversation {
                    conversation_id: *conversation_id,
                    fork_from_exchange: None,
                    summarize_after_fork: false,
                    summarization_prompt: None,
                    initial_prompt: None,
                    initial_attachments: vec![],
                    destination: ForkedConversationDestination::NewTab,
                });
            }
            AgentDetailsButtonEvent::ViewDetails { item_id } => {
                self.update_details_panel_for_item(item_id, ctx);
                self.selected_item_id = Some(*item_id);
                ctx.notify();
            }
            AgentDetailsButtonEvent::CopyLink { link } => {
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(link.clone()));
            }
        }
    }

    fn create_artifact_buttons_view(
        &self,
        artifacts: &[Artifact],
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<ArtifactButtonsRow> {
        let view = ctx.add_typed_action_view(|ctx| ArtifactButtonsRow::new(artifacts, ctx));
        ctx.subscribe_to_view(&view, Self::handle_artifact_buttons_event);
        view
    }

    fn handle_artifact_buttons_event(
        &mut self,
        _view: ViewHandle<ArtifactButtonsRow>,
        event: &ArtifactButtonsRowEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            ArtifactButtonsRowEvent::OpenPlan { notebook_uid } => {
                ctx.emit(AgentManagementViewEvent::OpenPlanNotebook {
                    notebook_uid: *notebook_uid,
                });
            }
            ArtifactButtonsRowEvent::CopyBranch { branch } => {
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(branch.clone()));

                let window_id = ctx.window_id();
                ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                    let toast = DismissibleToast::default("Copied branch name".to_string());
                    toast_stack.add_ephemeral_toast(toast, window_id, ctx);
                });
            }
            ArtifactButtonsRowEvent::OpenPullRequest { url } => {
                ctx.open_url(url);
            }
        }
    }

    fn handle_agent_management_model_event(
        &mut self,
        _model: ModelHandle<AgentConversationsModel>,
        event: &AgentConversationsModelEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            AgentConversationsModelEvent::ConversationsLoaded => {
                self.refresh_details_panel_if_needed(ctx);
                self.get_tasks_from_model(ctx);
            }
            AgentConversationsModelEvent::ConversationUpdated { kind } => {
                self.handle_conversation_updated(*kind, ctx);
            }
            AgentConversationsModelEvent::ConversationArtifactsUpdated { conversation_id } => {
                self.update_artifacts_for_conversation(*conversation_id, ctx);
                self.refresh_details_panel_if_needed(ctx);
            }
        }
    }

    /// Refresh the details panel if it's currently showing an item
    fn refresh_details_panel_if_needed(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(item_id) = self.selected_item_id {
            self.update_details_panel_for_item(&item_id, ctx);
        }
    }

    /// Decide how much work a `ConversationUpdated` event requires, based on its kind and the
    /// active status filter:
    /// * `Restored`: the underlying status didn't change, so the visible cards don't change
    ///   either. Just refresh the details panel.
    /// * `MetadataChanged`: rebuild the cards so metadata-derived actions update.
    /// * `TitleChanged`: rebuild the cards so filtering and titles update.
    /// * `StatusSet` that crosses the active status filter: rebuild the
    ///   card list via `get_tasks_from_model`.
    /// * `StatusSet` that doesn't cross the active filter (or `All` is active):
    ///   just refresh the details panel re-render so the status icon picks up the new value.
    fn handle_conversation_updated(
        &mut self,
        kind: ConversationUpdateKind,
        ctx: &mut ViewContext<Self>,
    ) {
        match kind {
            ConversationUpdateKind::Restored => {}
            ConversationUpdateKind::MetadataChanged => self.get_tasks_from_model(ctx),
            ConversationUpdateKind::TitleChanged => self.get_tasks_from_model(ctx),
            ConversationUpdateKind::StatusSet {
                prev_filter,
                new_filter,
            } => {
                if self
                    .filters
                    .status
                    .is_membership_crossed(prev_filter, new_filter)
                {
                    self.get_tasks_from_model(ctx);
                } else {
                    ctx.notify();
                }
            }
        }
        self.refresh_details_panel_if_needed(ctx);
    }

    /// Update the details panel with fresh data for the given item.
    fn update_details_panel_for_item(
        &mut self,
        item_id: &ManagementCardItemId,
        ctx: &mut ViewContext<Self>,
    ) {
        let model = AgentConversationsModel::as_ref(ctx);
        let Some(entry) = model.get_entry_by_id(item_id, ctx) else {
            return;
        };
        let open_action = AgentConversationsModel::resolve_open_action(
            AgentConversationNavigationSubject::Entry(*item_id),
            Some(RestoreConversationLayout::NewTab),
            ctx,
        );
        let copy_link_url = AgentConversationsModel::resolve_copy_link(
            AgentConversationNavigationSubject::Entry(*item_id),
            ctx,
        );
        let data = ConversationDetailsData::from_agent_conversation_entry(
            &entry,
            open_action,
            copy_link_url,
        );

        self.details_panel.update(ctx, |p, ctx| {
            p.set_conversation_details(data, ctx);
        });
    }

    /// Update just the artifact buttons for a specific conversation
    fn update_artifacts_for_conversation(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ViewContext<Self>,
    ) {
        let model = AgentConversationsModel::as_ref(ctx);
        let Some((index, entry)) = self.items.iter().enumerate().find_map(|(index, card)| {
            let entry = model.get_entry_by_id(&card.item_id, ctx)?;
            (entry.identity.local_conversation_id == Some(conversation_id))
                .then_some((index, entry))
        }) else {
            return;
        };
        let artifacts = entry.display.artifacts;

        // Update the artifact buttons for this card
        if should_show_artifacts(&artifacts) {
            if let Some(view) = &self.items[index].artifact_buttons_view {
                view.update(ctx, |v, ctx| v.update_artifacts(&artifacts, ctx));
            } else {
                let new_view = self.create_artifact_buttons_view(&artifacts, ctx);
                self.items[index].artifact_buttons_view = Some(new_view);
            }
        } else {
            self.items[index].artifact_buttons_view = None;
        }
        ctx.notify();
    }

    fn handle_details_panel_event(
        &mut self,
        _view: ViewHandle<ConversationDetailsPanel>,
        event: &ConversationDetailsPanelEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            ConversationDetailsPanelEvent::Close => {
                self.selected_item_id = None;
                ctx.notify();
            }
            ConversationDetailsPanelEvent::OpenPlanNotebook { notebook_uid } => {
                ctx.emit(AgentManagementViewEvent::OpenPlanNotebook {
                    notebook_uid: *notebook_uid,
                });
            }
        }
    }

    fn build_avatar(name: &str, appearance: &Appearance) -> Container {
        let theme = appearance.theme();
        // Use a hash function for a range of colors in the IDs
        let mut hasher = *HASHER;
        name.hash(&mut hasher);
        let hash = hasher.finish();

        let color = match hash % 6 {
            0 => theme.ansi_fg_red(),
            1 => theme.ansi_fg_blue(),
            2 => theme.ansi_fg_green(),
            3 => theme.ansi_fg_yellow(),
            4 => theme.ansi_fg_magenta(),
            _ => theme.ansi_fg_cyan(),
        };

        Avatar::new(
            AvatarContent::DisplayName(name.to_string()),
            UiComponentStyles {
                width: Some(BUTTON_SIZE),
                height: Some(BUTTON_SIZE),
                font_size: Some(CREATOR_AVATAR_FONT_SIZE),
                font_family_id: Some(appearance.monospace_font_family()),
                font_weight: Some(Weight::Bold),
                font_color: Some(theme.surface_1().into()),
                background: Some(color.into()),
                border_radius: Some(CornerRadius::with_all(Radius::Pixels(BUTTON_SIZE / 2.0))),
                ..Default::default()
            },
        )
        .build()
    }

    fn render_avatar_with_tooltip(
        creator_name: &str,
        appearance: &Appearance,
        mouse_state: MouseStateHandle,
    ) -> Box<dyn Element> {
        let avatar = Self::build_avatar(creator_name, appearance);
        let tooltip_text = creator_name.to_string();
        let ui_builder = appearance.ui_builder().clone();

        // Add a tooltip displaying the creator's full name
        Hoverable::new(mouse_state, move |state| {
            let mut stack = Stack::new().with_child(avatar.finish());
            if state.is_hovered() {
                let tooltip = ui_builder.tool_tip(tooltip_text.clone()).build().finish();
                stack.add_positioned_overlay_child(
                    tooltip,
                    OffsetPositioning::offset_from_parent(
                        vec2f(0., 4.),
                        ParentOffsetBounds::WindowByPosition,
                        ParentAnchor::BottomMiddle,
                        ChildAnchor::TopMiddle,
                    ),
                );
            }
            stack.finish()
        })
        .finish()
    }

    // Create a skeleton card for the loading screen
    fn render_skeleton_card(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();

        // Gradient values taken from the placeholders in the code review pane
        let base_gradient_color_start = internal_colors::neutral_3(theme);
        let mut base_gradient_color_end = base_gradient_color_start;
        base_gradient_color_end.a = 26;

        // Helper to create a gradient placeholder rect
        let placeholder = |max_width: f32, height: f32| {
            ConstrainedBox::new(
                Rect::new()
                    .with_horizontal_background_gradient(
                        base_gradient_color_start,
                        base_gradient_color_end,
                    )
                    .finish(),
            )
            .with_max_width(max_width)
            .with_height(height)
            .finish()
        };

        let header_row = placeholder(180., 14.);
        let metadata_row = placeholder(300., 12.);

        let content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(8.)
            .with_child(header_row)
            .with_child(metadata_row)
            .finish();

        Container::new(content)
            .with_background(theme.surface_1())
            .with_border(Border::all(1.).with_border_fill(theme.surface_2()))
            .with_uniform_padding(12.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish()
    }

    fn render_loading_state(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let skeleton_cards = (0..5).map(|_| self.render_skeleton_card(appearance));

        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_children(skeleton_cards)
            .with_spacing(8.)
            .finish()
    }

    fn render_card_at_index(&self, index: usize, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let Some(card_state) = self.items.get(index) else {
            return Empty::new().finish();
        };

        let model = AgentConversationsModel::as_ref(app);
        let Some(entry) = model.get_entry_by_id(&card_state.item_id, app) else {
            return Empty::new().finish();
        };

        self.render_card(card_state, &entry, appearance, app)
    }

    fn render_card(
        &self,
        card_state: &CardState,
        entry: &AgentConversationEntry,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();

        let artifact_buttons_element = card_state.artifact_buttons_view.as_ref().and_then(|view| {
            if view.as_ref(app).is_empty() || !FeatureFlag::ConversationArtifacts.is_enabled() {
                None
            } else {
                Some(ChildView::new(view).finish())
            }
        });

        let action_buttons_mouse_over = card_state
            .action_buttons_hover_state
            .lock()
            .expect("action buttons hover state lock poisoned")
            .is_mouse_over_element();
        let action_buttons_is_empty = card_state.action_buttons_view.as_ref(app).is_empty();

        let card_hoverable = Hoverable::new(card_state.hover_state.clone(), move |mouse_state| {
            let mut card_content = Flex::column()
                .with_spacing(CARD_ROW_SPACING)
                .with_child(Self::render_header_row(card_state, entry, appearance))
                .with_child(Self::render_metadata_row(entry, appearance));

            // Add artifacts row if there is a buttons view
            if let Some(buttons_element) = artifact_buttons_element {
                card_content.add_child(buttons_element);
            }

            // Determine whether to show the buttons based on whether we are hovering on the action buttons or the card,
            // to prevent lots of flickering.
            let should_show_action_buttons = mouse_state.is_hovered() || action_buttons_mouse_over;

            let card_background = if should_show_action_buttons {
                internal_colors::fg_overlay_2(theme)
            } else {
                internal_colors::fg_overlay_1(theme)
            };

            let card = Container::new(card_content.finish())
                .with_background(card_background)
                .with_border(Border::all(1.).with_border_fill(internal_colors::fg_overlay_2(theme)))
                .with_uniform_padding(CARD_CONTENT_PADDING)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_BORDER_RADIUS)))
                .with_margin_top(CARD_MARGIN_BOTTOM)
                .finish();

            let mut stack = Stack::new().with_child(card);
            if should_show_action_buttons && !action_buttons_is_empty {
                let action_buttons =
                    Hoverable::new(card_state.action_buttons_hover_state.clone(), |_| {
                        Container::new(ChildView::new(&card_state.action_buttons_view).finish())
                            .with_background(theme.surface_3())
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                            .with_uniform_padding(4.)
                            .with_drop_shadow(DropShadow {
                                color: ColorU::new(0, 0, 0, 77),
                                offset: vec2f(0., 4.),
                                blur_radius: 7.,
                                spread_radius: 0.,
                            })
                            .finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .with_defer_events_to_children();

                // Note: we use an overlay layer so that the hover on the top of the list can extend outside
                // of the list boundaries, rendered unclipped.
                stack.add_positioned_overlay_child(
                    action_buttons.finish(),
                    OffsetPositioning::offset_from_parent(
                        vec2f(-CARD_CONTENT_PADDING, 4.),
                        ParentOffsetBounds::WindowByPosition,
                        ParentAnchor::TopRight,
                        ChildAnchor::MiddleRight,
                    ),
                );
            }

            stack.finish()
        })
        .with_defer_events_to_children();

        let item_id = card_state.item_id;
        let card_hoverable = if entry.capabilities.can_open {
            card_hoverable
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(AgentManagementViewAction::OpenSession { item_id });
                })
        } else {
            card_hoverable
        };

        card_hoverable.finish()
    }

    fn render_header_row(
        card_state: &CardState,
        entry: &AgentConversationEntry,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size();

        let title_text = Text::new_inline(entry.display.title.clone(), font_family, font_size)
            .with_color(theme.active_ui_text_color().into());
        let status_icon = render_icon_with_status(
            agent_conversation_entry_icon_variant(entry),
            CARD_AGENT_ICON_SIZE,
            0.,
            theme,
            internal_colors::fg_overlay_1(theme),
        );
        let time_str = format_approx_duration_from_now_utc(entry.display.last_updated);
        let time_text = Text::new_inline(time_str, font_family, font_size)
            .with_color(theme.nonactive_ui_text_color().into());
        let creator_name = entry
            .display
            .creator
            .name
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        let avatar = Self::render_avatar_with_tooltip(
            &creator_name,
            appearance,
            card_state.avatar_hover_state.clone(),
        );

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(2.)
            .with_child(Container::new(status_icon).with_margin_right(4.).finish())
            .with_child(Expanded::new(1., title_text.finish()).finish());
        let mut time_and_avatar = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.);

        time_and_avatar.add_child(time_text.finish());
        time_and_avatar.add_child(avatar);

        row.add_child(
            Container::new(time_and_avatar.finish())
                .with_margin_right(2.)
                .finish(),
        );

        // We want to make sure the text in the row is always at least the button height
        ConstrainedBox::new(row.finish())
            .with_min_height(BUTTON_SIZE)
            .finish()
    }

    fn render_metadata_row(
        entry: &AgentConversationEntry,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size();
        let mut metadata_parts = Vec::new();

        if let Some(harness) = entry
            .display
            .harness
            .filter(|harness| *harness != Harness::Local)
        {
            metadata_parts.push(format!(
                "Harness: {}",
                harness_display::display_name(harness)
            ));
        }

        Text::new(metadata_parts.join(" • "), font_family, font_size)
            .with_color(theme.nonactive_ui_text_color().into())
            .finish()
    }

    // Render the main page header based on the current view state
    fn render_header(&self, app: &AppContext) -> Box<dyn Element> {
        match self.get_view_state(app) {
            ViewState::Loading => self.render_loading_header(app),
            ViewState::NoFilterMatches | ViewState::HasTasks => self.render_task_list_header(app),
        }
    }

    fn render_task_list_header(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let is_loading = AgentConversationsModel::handle(app)
            .as_ref(app)
            .is_loading();

        let size_switch_threshold = MEDIUM_SIZE_SWITCH_THRESHOLD * appearance.monospace_ui_scalar();

        let build_header = |use_expanded: bool| {
            let title = Text::new_inline(
                "Runs",
                appearance.ui_font_family(),
                appearance.ui_font_size() + 4.,
            )
            .with_style(Properties::default().weight(Weight::Semibold))
            .with_color(theme.active_ui_text_color().into())
            .finish();

            let new_agent_button = if use_expanded {
                self.new_agent_button.expanded_button()
            } else {
                self.new_agent_button.compact_button()
            };

            let mut header_top = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_spacing(4.)
                .with_child(Container::new(title).with_margin_right(12.).finish());

            if is_loading {
                header_top.add_child(self.render_loading_icon(appearance));
            }

            header_top.add_child(Expanded::new(1., Empty::new().finish()).finish());

            if !cfg!(target_family = "wasm") {
                header_top.add_child(ChildView::new(new_agent_button).finish());
            }

            let mut filters_wrap = Wrap::row()
                .with_spacing(4.)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(ChildView::new(&self.status_dropdown).finish())
                .with_child(ChildView::new(&self.created_on_dropdown).finish())
                .with_child(ChildView::new(&self.artifact_dropdown).finish());

            if self.filters.is_filtering() {
                filters_wrap.add_child(ChildView::new(&self.clear_all_filters_button).finish());
            }

            let search_input = self.render_search_input(app);
            let header_bottom = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_main_axis_size(MainAxisSize::Max)
                .with_spacing(8.)
                .with_child(Expanded::new(3., filters_wrap.finish()).finish())
                .with_child(Expanded::new(1., search_input).finish())
                .finish();

            let header_bottom = Container::new(header_bottom).with_margin_top(4.).finish();

            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_child(header_top.finish())
                .with_child(header_bottom)
                .finish()
        };

        SizeConstraintSwitch::new(
            build_header(true),
            vec![(
                SizeConstraintCondition::WidthLessThan(size_switch_threshold),
                build_header(false),
            )],
        )
        .finish()
    }

    fn render_loading_icon(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let ui_builder = appearance.ui_builder().clone();
        let icon_size = appearance.ui_font_size();

        let loading_icon = ConstrainedBox::new(
            Icon::Refresh
                .to_warpui_icon(theme.sub_text_color(theme.surface_1()))
                .finish(),
        )
        .with_height(icon_size)
        .with_width(icon_size)
        .finish();

        Hoverable::new(self.loading_icon_mouse_state.clone(), move |mouse_state| {
            let mut stack = Stack::new().with_child(loading_icon);
            if mouse_state.is_hovered() {
                let tooltip = ui_builder
                    .tool_tip(String::from("Loading conversations"))
                    .build()
                    .finish();
                stack.add_positioned_overlay_child(
                    tooltip,
                    OffsetPositioning::offset_from_parent(
                        vec2f(0., -8.),
                        ParentOffsetBounds::WindowByPosition,
                        ParentAnchor::TopMiddle,
                        ChildAnchor::BottomMiddle,
                    ),
                );
            }
            stack.finish()
        })
        .finish()
    }

    fn render_loading_header(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let title = Text::new_inline(
            "Runs",
            appearance.ui_font_family(),
            appearance.ui_font_size() + 4.,
        )
        .with_style(Properties::default().weight(Weight::Semibold))
        .with_color(theme.active_ui_text_color().into())
        .finish();

        let loading_icon = ConstrainedBox::new(
            Icon::Loading
                .to_warpui_icon(Fill::Solid(internal_colors::neutral_6(theme)))
                .finish(),
        )
        .with_height(appearance.ui_font_size() + 2.)
        .with_width(appearance.ui_font_size() + 2.)
        .finish();

        let loading_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Container::new(loading_icon).with_margin_right(10.).finish())
            .with_child(
                Text::new_inline(
                    "Loading agents...",
                    appearance.ui_font_family(),
                    appearance.ui_font_size() + 2.,
                )
                .with_style(Properties::default().weight(Weight::Semibold))
                .with_color(theme.active_ui_text_color().into())
                .finish(),
            )
            .finish();

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(12.)
            .with_child(title)
            .with_child(loading_row)
            .finish()
    }

    fn render_search_input(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = Appearance::handle(app).as_ref(app).theme();

        let search_row = Clipped::new(ChildView::new(&self.search_editor).finish()).finish();

        Container::new(
            ConstrainedBox::new(
                Container::new(search_row)
                    .with_padding(
                        Padding::uniform(0.)
                            .with_vertical(5.)
                            .with_left(8.)
                            .with_right(8.),
                    )
                    .with_border(Border::all(1.).with_border_fill(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                    .finish(),
            )
            .with_height(30.)
            .finish(),
        )
        .with_margin_top(FILTER_ROW_VERTICAL_MARGIN)
        .with_margin_bottom(FILTER_ROW_VERTICAL_MARGIN)
        .finish()
    }

    fn render_no_results_view(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let icon = ConstrainedBox::new(
            Icon::FilterOff
                .to_warpui_icon(appearance.theme().nonactive_ui_text_color())
                .finish(),
        )
        .with_width(24.)
        .with_height(24.)
        .finish();

        let text = Text::new_inline(
            "No results matched your filters",
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_style(Properties::default().weight(Weight::Semibold))
        .with_color(appearance.theme().nonactive_ui_text_color().into())
        .finish();

        Align::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(12.)
                .with_child(icon)
                .with_child(text)
                .with_child(ChildView::new(&self.no_filter_results_button).finish())
                .finish(),
        )
        .finish()
    }

    fn render_default_scroll_view(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = Appearance::as_ref(app).theme();
        let axis_config = SingleAxisConfig::Manual {
            handle: self.scroll_state.clone(),
            child: NewScrollableElement::finish_scrollable(List::new(self.list_state.clone())),
        };
        NewScrollable::vertical(
            axis_config,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            warpui::elements::Fill::None,
        )
        .with_vertical_scrollbar(ScrollableAppearance::new(ScrollbarWidth::None, false))
        .with_always_handle_events_first(false)
        .finish()
    }
}

impl Entity for AgentManagementView {
    type Event = AgentManagementViewEvent;
}

impl View for AgentManagementView {
    fn ui_name() -> &'static str {
        "AgentManagementView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.search_editor);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let main_content = match self.get_view_state(app) {
            ViewState::Loading => self.render_loading_state(app),
            ViewState::NoFilterMatches => self.render_no_results_view(app),
            ViewState::HasTasks => self.render_default_scroll_view(app),
        };

        let content = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.render_header(app))
            .with_child(
                Shrinkable::new(
                    1.,
                    Container::new(main_content).with_margin_top(12.).finish(),
                )
                .finish(),
            )
            .finish();

        let centered = Align::new(content).top_center().finish();

        let main_view = Container::new(centered).with_uniform_margin(16.).finish();

        // Wrap main view with details panel if we have selected an item

        (if self.selected_item_id.is_some() {
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(Expanded::new(1., main_view).finish())
                .with_child(ChildView::new(&self.details_panel).finish())
                .finish()
        } else {
            main_view
        }) as _
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentManagementViewAction {
    SetStatusFilter(StatusFilter),
    SetCreatedOnFilter(CreatedOnFilter),
    SetArtifactFilter(ArtifactFilter),
    ClearFilters,
    NewAgent,
    OpenSession { item_id: ManagementCardItemId },
    FocusSearch,
}

pub enum AgentManagementViewEvent {
    OpenPlanNotebook { notebook_uid: NotebookId },
}

impl TypedActionView for AgentManagementView {
    type Action = AgentManagementViewAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            AgentManagementViewAction::SetStatusFilter(filter) => {
                self.filters.status = *filter;
                self.on_filter_changed(ctx);
            }
            AgentManagementViewAction::SetCreatedOnFilter(filter) => {
                self.filters.created_on = *filter;
                self.on_filter_changed(ctx);
            }
            AgentManagementViewAction::SetArtifactFilter(filter) => {
                self.filters.artifact = *filter;
                self.get_tasks_from_model(ctx);
                ctx.dispatch_global_action("workspace:save_app", ());
            }
            AgentManagementViewAction::ClearFilters => {
                self.filters = AgentManagementFilters::default();

                // Reset the selected states for the dropdowns
                self.status_dropdown.update(ctx, |dropdown, ctx| {
                    dropdown.set_selected_by_index(0, ctx);
                });
                self.created_on_dropdown.update(ctx, |dropdown, ctx| {
                    dropdown.set_selected_by_index(0, ctx);
                });
                self.artifact_dropdown.update(ctx, |dropdown, ctx| {
                    dropdown.set_selected_by_index(0, ctx);
                });
                self.on_filter_changed(ctx);
            }
            AgentManagementViewAction::NewAgent => {
                ctx.dispatch_typed_action(&WorkspaceAction::NewTabInAgentMode {
                    zero_state_prompt_suggestion_type: None,
                });
            }
            AgentManagementViewAction::OpenSession { item_id } => {
                let Some(action) = AgentConversationsModel::resolve_open_action(
                    AgentConversationNavigationSubject::Entry(*item_id),
                    Some(RestoreConversationLayout::NewTab),
                    ctx,
                ) else {
                    return;
                };

                ctx.dispatch_typed_action(&action);
            }
            AgentManagementViewAction::FocusSearch => {
                ctx.focus(&self.search_editor);
            }
        }
    }
}
