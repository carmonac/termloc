//! The "Warp Agent" settings page, shown under the Agents umbrella.
//!
//! Covers Warp's own AI: the global toggle, Active AI suggestions, agent
//! input behavior, voice input and the miscellaneous agent display settings.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;

use markdown_parser::{FormattedText, FormattedTextFragment, FormattedTextLine};
use settings::{Setting, ToggleableSetting};
use strum::IntoEnumIterator;
use warp_core::context_flag::ContextFlag;
use warp_core::features::FeatureFlag;
use warp_errors::report_if_error;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Flex, FormattedTextElement,
    HighlightedHyperlink, HyperlinkUrl, MouseStateHandle, ParentElement, Text,
};
use warpui::keymap::ContextPredicate;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::switch::{SwitchStateHandle, TooltipConfig};
use warpui::{
    Action, AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle, WeakViewHandle, id,
};

use super::ai_shared::{
    render_ai_feature_switch, render_ai_setting_description, render_ai_setting_label,
    render_ai_setting_toggle, render_toolbar_layout_editor, styles,
    update_editor_interaction_state,
};
use super::settings_page::{
    CONTENT_FONT_SIZE, Category, CategoryHeader, LocalOnlyIconState, MatchData, PageTitle,
    PageType, SettingsPageMeta, SettingsPageViewHandle, SettingsWidget,
    TOGGLE_BUTTON_RIGHT_PADDING, ToggleState, build_toggle_element, render_body_item_label,
    render_dropdown_item, render_filterable_dropdown_item,
};
use super::{
    SettingActionPairContexts, SettingActionPairDescriptions, SettingsAction, SettingsSection,
    ToggleSettingActionPair, flags,
};
use crate::UserWorkspaces;
use crate::ai::blocklist::agent_view::agent_input_footer::editor::{
    AgentToolbarEditorMode, AgentToolbarInlineEditor,
};
use crate::appearance::Appearance;
use crate::editor::{EditorOptions, EditorView, Event as EditorEvent, TextColors, TextOptions};
use crate::settings::{
    AIAutoDetectionEnabled, AICommandDenylist, AISettings, AISettingsChangedEvent,
    AgentModeQuerySuggestionsEnabled, AutoApproveBypassesCommandDenylist,
    EnableAiCommandSearchHashTrigger, GitOperationsAutogenEnabled, IncludeAgentCommandsInHistory,
    InputSettings, IntelligentAutosuggestionsEnabled, LongRunningCommandSubmissionMode,
    NLDInTerminalEnabled, NaturalLanguageAutosuggestionsEnabled, PromptSubmissionMode,
    SharedBlockTitleGenerationEnabled, ShouldRenderUseAgentToolbarForUserCommands, ShowAgentTips,
    ShowConversationHistory, ShowHintText, ThinkingDisplayMode, VOICE_INPUT_LANGUAGES,
    VoiceInputEnabled, VoiceInputLanguage, VoiceInputToggleKey,
};
use crate::util::bindings;
use crate::view_components::{Dropdown, DropdownItem, FilterableDropdown};
use crate::workspaces::workspace::AdminEnablementSetting;

const AI_SETTINGS_DROPDOWN_WIDTH: f32 = 250.;
const AI_SETTINGS_DROPDOWN_MAX_HEIGHT: f32 = 250.;

const NEXT_COMMAND_DESCRIPTION: &str = "Let AI suggest the next command to run based on your command history, outputs, and common workflows.";
const PROMPT_SUGGESTIONS_DESCRIPTION: &str = "Let AI suggest natural language prompts, as inline banners in the input, based on recent commands and their outputs.";
const SUGGESTED_CODE_BANNERS_DESCRIPTION: &str = "Let AI suggest code diffs and queries as inline banners in the blocklist, based on recent commands and their outputs.";
const NATURAL_LANGUAGE_AUTOSUGGESTIONS: &str =
    "Let AI suggest natural language autosuggestions, based on recent commands and their outputs.";
const SHARED_BLOCK_TITLE_GENERATION_DESCRIPTION: &str =
    "Let AI generate a title for your shared block based on the command and output.";
const GIT_OPERATIONS_AUTOGEN_DESCRIPTION: &str =
    "Let AI generate commit messages and pull request titles and descriptions.";
const WISPR_FLOW_URL: &str = "https://wisprflow.ai/";

pub fn init_actions_from_parent_view<T: Action + Clone>(
    app: &mut AppContext,
    context: &ContextPredicate,
    builder: fn(SettingsAction) -> T,
) {
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "AI",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleGlobalAI,
                )),
                context,
                flags::IS_ANY_AI_ENABLED,
            )
            .with_group(bindings::BindingGroup::WarpAi),
        ],
        app,
    );

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "Active AI",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleActiveAI,
                )),
                &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
                flags::IS_ACTIVE_AI_ENABLED,
            )
            .with_group(bindings::BindingGroup::WarpAi),
        ],
        app,
    );

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                if FeatureFlag::AgentView.is_enabled() {
                    "terminal command autodetection in agent input"
                } else {
                    "natural language detection"
                },
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleAIInputAutoDetection,
                )),
                &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
                flags::AI_INPUT_AUTODETECTION_FLAG,
            )
            .with_group(bindings::BindingGroup::WarpAi)
            .with_enabled(|| FeatureFlag::AgentMode.is_enabled()),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "agent prompt autodetection in terminal input",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleNLDInTerminal,
                )),
                &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
                flags::NLD_IN_TERMINAL_FLAG,
            )
            .with_group(bindings::BindingGroup::WarpAi)
            .with_enabled(|| FeatureFlag::AgentView.is_enabled()),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "'#' trigger for AI command search",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleAiCommandSearchHashTrigger,
                )),
                &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
                flags::AI_COMMAND_SEARCH_HASH_TRIGGER_FLAG,
            )
            .with_group(bindings::BindingGroup::WarpAi),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "Next Command",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleIntelligentAutosuggestions,
                )),
                &(context.clone() & id!(flags::IS_ACTIVE_AI_ENABLED)),
                flags::INTELLIGENT_AUTOSUGGESTIONS_FLAG,
            )
            .with_group(bindings::BindingGroup::WarpAi),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "prompt suggestions",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::TogglePromptSuggestions,
                )),
                &(context.clone() & id!(flags::IS_ACTIVE_AI_ENABLED)),
                flags::PROMPT_SUGGESTIONS_FLAG,
            )
            .with_group(bindings::BindingGroup::WarpAi),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "code suggestions",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleCodeSuggestions,
                )),
                &(context.clone()
                    & id!(flags::IS_ACTIVE_AI_ENABLED)
                    & id!(flags::PROMPT_SUGGESTIONS_FLAG)),
                flags::CODE_SUGGESTIONS_FLAG,
            )
            .with_group(bindings::BindingGroup::WarpAi),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::custom(
                SettingActionPairDescriptions::new("Show agent tips", "Hide agent tips"),
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleShowAgentTips,
                )),
                SettingActionPairContexts::new(
                    context.clone()
                        & id!(flags::IS_ANY_AI_ENABLED)
                        & !id!(flags::SHOW_AGENT_TIPS_FLAG),
                    context.clone()
                        & id!(flags::IS_ANY_AI_ENABLED)
                        & id!(flags::SHOW_AGENT_TIPS_FLAG),
                ),
                None,
            )
            .with_group(bindings::BindingGroup::WarpAi)
            .with_enabled(|| FeatureFlag::AgentTips.is_enabled()),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "voice input",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleVoiceInput,
                )),
                &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
                flags::IS_VOICE_INPUT_ENABLED,
            )
            .with_group(bindings::BindingGroup::WarpAi)
            .with_enabled(|| cfg!(feature = "voice_input")),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::custom(
                SettingActionPairDescriptions::new(
                    "Show \"Use Agent\" footer",
                    "Hide \"Use Agent\" footer",
                ),
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleUseAgentToolbar,
                )),
                SettingActionPairContexts::new(
                    context.clone()
                        & id!(flags::IS_ANY_AI_ENABLED)
                        & !id!(flags::USE_AGENT_FOOTER_FLAG),
                    context.clone()
                        & id!(flags::IS_ANY_AI_ENABLED)
                        & id!(flags::USE_AGENT_FOOTER_FLAG),
                ),
                None,
            )
            .with_group(bindings::BindingGroup::WarpAi),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "include agent-executed commands in history",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleIncludeAgentCommandsInHistory,
                )),
                &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
                flags::INCLUDE_AGENT_COMMANDS_IN_HISTORY_FLAG,
            )
            .with_group(bindings::BindingGroup::WarpAi),
            ToggleSettingActionPair::custom(
                SettingActionPairDescriptions::new(
                    "Allow auto-approve to bypass command denylist",
                    "Require approval for denylisted commands in auto-approve",
                ),
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleAutoApproveBypassesCommandDenylist,
                )),
                SettingActionPairContexts::new(
                    context.clone()
                        & id!(flags::IS_ANY_AI_ENABLED)
                        & !id!(flags::AUTO_APPROVE_BYPASSES_COMMAND_DENYLIST_FLAG),
                    context.clone()
                        & id!(flags::IS_ANY_AI_ENABLED)
                        & id!(flags::AUTO_APPROVE_BYPASSES_COMMAND_DENYLIST_FLAG),
                ),
                None,
            )
            .with_group(bindings::BindingGroup::WarpAi),
            ToggleSettingActionPair::new(
                "conversation history in tools panel",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleShowConversationHistory,
                )),
                &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
                flags::SHOW_CONVERSATION_HISTORY,
            )
            .with_group(bindings::BindingGroup::WarpAi),
        ],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![
            ToggleSettingActionPair::new(
                "Auto-spawn servers from third-party agents",
                builder(SettingsAction::WarpAgent(
                    WarpAgentPageAction::ToggleFileBasedMcp,
                )),
                &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
                flags::FILE_BASED_MCP_FLAG,
            )
            .with_group(bindings::BindingGroup::WarpAi)
            .with_enabled(|| {
                FeatureFlag::McpServer.is_enabled()
                    && FeatureFlag::FileBasedMcp.is_enabled()
                    && ContextFlag::ShowMCPServers.is_enabled()
            }),
        ],
        app,
    );
}

pub struct WarpAgentPageView {
    page: PageType<Self>,
    self_handle: WeakViewHandle<Self>,
    voice_input_toggle_key_dropdown: ViewHandle<Dropdown<WarpAgentPageAction>>,
    voice_input_language_dropdown: ViewHandle<FilterableDropdown<WarpAgentPageAction>>,
    local_only_icon_tooltip_states: RefCell<HashMap<String, MouseStateHandle>>,
    autodetection_denylist_editor: ViewHandle<EditorView>,
    agent_toolbar_inline_editor: ViewHandle<AgentToolbarInlineEditor>,

    thinking_display_mode_dropdown: ViewHandle<Dropdown<WarpAgentPageAction>>,
    default_prompt_submission_mode_dropdown: ViewHandle<Dropdown<WarpAgentPageAction>>,
    lrc_submission_mode_dropdown: ViewHandle<Dropdown<WarpAgentPageAction>>,
    #[cfg(feature = "local_fs")]
    conversation_layout_dropdown: ViewHandle<Dropdown<WarpAgentPageAction>>,
}

impl WarpAgentPageView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let self_handle = ctx.handle();
        let is_any_ai_enabled = AISettings::as_ref(ctx).is_any_ai_enabled(ctx);

        let voice_input_toggle_key_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            if !AISettings::as_ref(ctx).is_voice_input_enabled(ctx) {
                dropdown.set_disabled(ctx);
            }

            let values = VoiceInputToggleKey::all_possible_values();
            let current_value = AISettings::as_ref(ctx).voice_input_toggle_key.value();
            let selected_index = values
                .iter()
                .position(|val| val == current_value)
                .unwrap_or_else(|| {
                    log::warn!(
                        "Could not find current VoiceInputToggleKey value in dropdown option list"
                    );
                    0
                });

            dropdown.add_items(
                values
                    .into_iter()
                    .map(|val| {
                        DropdownItem::new(
                            val.display_name(),
                            WarpAgentPageAction::SetVoiceInputToggleKey(val),
                        )
                    })
                    .collect(),
                ctx,
            );
            dropdown.set_selected_by_index(selected_index, ctx);

            dropdown
        });

        let voice_input_language_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = FilterableDropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            if !AISettings::as_ref(ctx).is_voice_input_enabled(ctx) {
                dropdown.set_disabled(ctx);
            }

            dropdown.add_items(
                VOICE_INPUT_LANGUAGES
                    .iter()
                    .map(|&(code, name)| {
                        DropdownItem::new(
                            name,
                            WarpAgentPageAction::SetVoiceInputLanguage(code.to_string()),
                        )
                    })
                    .collect(),
                ctx,
            );
            let current_code = AISettings::as_ref(ctx)
                .voice_input_language_code()
                .unwrap_or("")
                .to_string();
            dropdown.set_selected_by_action(
                WarpAgentPageAction::SetVoiceInputLanguage(current_code),
                ctx,
            );

            dropdown
        });

        let thinking_display_mode_dropdown =
            OtherAIWidget::create_thinking_display_mode_dropdown(ctx);
        // Set initial selection based on current setting value.
        {
            let current_mode = AISettings::as_ref(ctx).thinking_display_mode;
            thinking_display_mode_dropdown.update(ctx, |dropdown, ctx| {
                dropdown.set_selected_by_action(
                    WarpAgentPageAction::SetThinkingDisplayMode(current_mode),
                    ctx,
                );
            });
        }

        let default_prompt_submission_mode_dropdown =
            OtherAIWidget::create_default_prompt_submission_mode_dropdown(ctx);
        {
            let current_mode = AISettings::as_ref(ctx).default_prompt_submission_mode;
            default_prompt_submission_mode_dropdown.update(ctx, |dropdown, ctx| {
                dropdown.set_selected_by_action(
                    WarpAgentPageAction::SetPromptSubmissionMode(current_mode),
                    ctx,
                );
            });
        }

        let lrc_submission_mode_dropdown = OtherAIWidget::create_lrc_submission_mode_dropdown(ctx);
        {
            let current_mode = AISettings::as_ref(ctx).long_running_command_submission_mode;
            lrc_submission_mode_dropdown.update(ctx, |dropdown, ctx| {
                dropdown.set_selected_by_action(
                    WarpAgentPageAction::SetLongRunningCommandSubmissionMode(current_mode),
                    ctx,
                );
            });
        }

        let autodetection_denylist_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = EditorOptions {
                autogrow: true,
                soft_wrap: true,
                text: TextOptions {
                    font_size_override: Some(appearance.ui_font_size()),
                    font_family_override: Some(appearance.monospace_font_family()),
                    text_colors_override: Some(TextColors {
                        default_color: appearance.theme().active_ui_text_color(),
                        disabled_color: appearance.theme().disabled_ui_text_color(),
                        hint_color: appearance.theme().disabled_ui_text_color(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut editor = EditorView::new(options, ctx);

            editor.set_placeholder_text("Commands, comma separated", ctx);

            let current_value = AISettings::as_ref(ctx)
                .autodetection_command_denylist
                .value()
                .clone();
            editor.set_buffer_text(current_value.as_str(), ctx);
            editor
        });
        update_editor_interaction_state(
            autodetection_denylist_editor.clone(),
            is_any_ai_enabled,
            ctx,
        );

        ctx.subscribe_to_view(&autodetection_denylist_editor, move |me, _, event, ctx| {
            me.handle_detection_denylist_editor_event(event, ctx);
        });

        ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), |_, _handle, _event, ctx| {
            // Re-render if teams-related data changed that may affect whether features such as voice input are enabled.
            ctx.notify();
        });

        ctx.subscribe_to_model(&AISettings::handle(ctx), |me, _, event, ctx| {
            match event {
                AISettingsChangedEvent::AICommandDenylist { .. } => {
                    me.autodetection_denylist_editor.update(ctx, |editor, ctx| {
                        let denylist_value = &AISettings::as_ref(ctx)
                            .autodetection_command_denylist
                            .value()
                            .clone();
                        editor.set_buffer_text(denylist_value, ctx);
                    });
                }
                AISettingsChangedEvent::IsAnyAIEnabled { .. } => {
                    let is_enabled = AISettings::as_ref(ctx).is_any_ai_enabled(ctx);

                    update_editor_interaction_state(
                        me.autodetection_denylist_editor.clone(),
                        is_enabled,
                        ctx,
                    );

                    me.update_voice_input_dropdown_enablement(ctx);
                }
                AISettingsChangedEvent::VoiceInputEnabled { .. } => {
                    me.update_voice_input_dropdown_enablement(ctx);
                }
                AISettingsChangedEvent::VoiceInputToggleKey { .. } => {
                    let current_value = AISettings::as_ref(ctx)
                        .voice_input_toggle_key
                        .value()
                        .display_name();
                    me.voice_input_toggle_key_dropdown
                        .update(ctx, |dropdown, ctx| {
                            dropdown.set_selected_by_name(current_value, ctx)
                        });
                }
                AISettingsChangedEvent::VoiceInputLanguage { .. } => {
                    let current_code = AISettings::as_ref(ctx)
                        .voice_input_language_code()
                        .unwrap_or("")
                        .to_string();
                    me.voice_input_language_dropdown
                        .update(ctx, |dropdown, ctx| {
                            dropdown.set_selected_by_action(
                                WarpAgentPageAction::SetVoiceInputLanguage(current_code),
                                ctx,
                            )
                        });
                }
                AISettingsChangedEvent::ThinkingDisplayMode { .. } => {
                    let current_mode = *AISettings::as_ref(ctx).thinking_display_mode.value();
                    me.thinking_display_mode_dropdown
                        .update(ctx, |dropdown, ctx| {
                            dropdown.set_selected_by_action(
                                WarpAgentPageAction::SetThinkingDisplayMode(current_mode),
                                ctx,
                            );
                        });
                }
                AISettingsChangedEvent::PromptSubmissionMode { .. } => {
                    let current_mode = AISettings::as_ref(ctx).default_prompt_submission_mode;
                    me.default_prompt_submission_mode_dropdown
                        .update(ctx, |dropdown, ctx| {
                            dropdown.set_selected_by_action(
                                WarpAgentPageAction::SetPromptSubmissionMode(current_mode),
                                ctx,
                            );
                        });
                }
                AISettingsChangedEvent::LongRunningCommandSubmissionMode { .. } => {
                    let current_mode = AISettings::as_ref(ctx).long_running_command_submission_mode;
                    me.lrc_submission_mode_dropdown
                        .update(ctx, |dropdown, ctx| {
                            dropdown.set_selected_by_action(
                                WarpAgentPageAction::SetLongRunningCommandSubmissionMode(
                                    current_mode,
                                ),
                                ctx,
                            );
                        });
                }
                _ => (),
            }
            ctx.notify();
        });

        ctx.subscribe_to_model(&InputSettings::handle(ctx), |_, _, _, ctx| {
            ctx.notify();
        });

        let agent_toolbar_inline_editor = ctx.add_typed_action_view(|ctx| {
            AgentToolbarInlineEditor::new(AgentToolbarEditorMode::AgentView, ctx)
        });

        #[cfg(feature = "local_fs")]
        let conversation_layout_dropdown = ctx.add_typed_action_view(|ctx| {
            use crate::util::file::external_editor::settings::OpenConversationPreference;

            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);

            let items = vec![
                DropdownItem::new(
                    "New Tab",
                    WarpAgentPageAction::SetConversationLayout(OpenConversationPreference::NewTab),
                ),
                DropdownItem::new(
                    "Split Pane",
                    WarpAgentPageAction::SetConversationLayout(
                        OpenConversationPreference::SplitPane,
                    ),
                ),
            ];
            dropdown.set_items(items, ctx);

            let current = *crate::util::file::external_editor::EditorSettings::as_ref(ctx)
                .open_conversation_layout_preference;
            match current {
                OpenConversationPreference::NewTab => dropdown.set_selected_by_name("New Tab", ctx),
                OpenConversationPreference::SplitPane => {
                    dropdown.set_selected_by_name("Split Pane", ctx)
                }
            };
            dropdown
        });

        Self {
            page: Self::build_page(ctx),
            self_handle,
            voice_input_toggle_key_dropdown,
            voice_input_language_dropdown,
            autodetection_denylist_editor,
            local_only_icon_tooltip_states: Default::default(),
            agent_toolbar_inline_editor,
            thinking_display_mode_dropdown,
            default_prompt_submission_mode_dropdown,
            lrc_submission_mode_dropdown,
            #[cfg(feature = "local_fs")]
            conversation_layout_dropdown,
        }
    }

    fn update_voice_input_dropdown_enablement(&mut self, ctx: &mut ViewContext<Self>) {
        let is_voice_enabled = AISettings::as_ref(ctx).is_voice_input_enabled(ctx);
        self.voice_input_toggle_key_dropdown
            .update(ctx, |dropdown, ctx| {
                if is_voice_enabled {
                    dropdown.set_enabled(ctx);
                } else {
                    dropdown.set_disabled(ctx);
                }
            });
        self.voice_input_language_dropdown
            .update(ctx, |dropdown, ctx| {
                if is_voice_enabled {
                    dropdown.set_enabled(ctx);
                } else {
                    dropdown.set_disabled(ctx);
                }
            });
        ctx.notify();
    }

    fn build_page(ctx: &mut ViewContext<Self>) -> PageType<Self> {
        let ai_settings = AISettings::as_ref(ctx);

        let mut categories: Vec<Category<Self>> = Vec::new();

        if ai_settings
            .intelligent_autosuggestions_enabled_internal
            .is_supported_on_current_platform()
            || ai_settings
                .prompt_suggestions_enabled_internal
                .is_supported_on_current_platform()
            || (FeatureFlag::PredictAMQueries.is_enabled()
                && ai_settings
                    .natural_language_autosuggestions_enabled_internal
                    .is_supported_on_current_platform())
            || (FeatureFlag::SharedBlockTitleGeneration.is_enabled()
                && ai_settings
                    .shared_block_title_generation_enabled_internal
                    .is_supported_on_current_platform())
            || (FeatureFlag::GitOperationsInCodeReview.is_enabled()
                && ai_settings
                    .git_operations_autogen_enabled_internal
                    .is_supported_on_current_platform())
        {
            let active_ai_widgets: Vec<Box<dyn SettingsWidget<View = Self>>> = vec![
                Box::new(NextCommandWidget::default()),
                Box::new(PromptSuggestionsWidget::default()),
                Box::new(SuggestedCodeBannersWidget::default()),
                Box::new(NaturalLanguageAutosuggestionsWidget::default()),
                Box::new(SharedBlockTitleGenerationWidget::default()),
                Box::new(GitOperationsAutogenWidget::default()),
            ];
            let active_ai_toggle = SwitchStateHandle::default();
            categories.push(Category::with_header(
                CategoryHeader::new("Active AI").with_trailing_element(
                    move |_view, _appearance, app| render_active_ai_toggle(&active_ai_toggle, app),
                ),
                active_ai_widgets,
            ));
        }

        categories.push(Category::new(
            "Input",
            vec![
                Box::new(NaturalLanguageDetectionWidget::default()),
                Box::new(ShowInputHintTextWidget::default()),
                Box::new(AiCommandSearchHashTriggerWidget::default()),
                Box::new(ShowAgentTipsWidget::default()),
                Box::new(IncludeAgentCommandsInHistoryWidget::default()),
                Box::new(AutoApproveBypassesCommandDenylistWidget::default()),
                Box::new(PromptSubmissionModeWidget),
            ],
        ));

        let voice_supported = cfg!(feature = "voice_input")
            && ai_settings
                .voice_input_enabled_internal
                .is_supported_on_current_platform();
        if voice_supported {
            categories.push(Category::new(
                "Voice",
                vec![Box::new(VoiceWidget::default())],
            ));
        }

        categories.push(Category::new(
            "Agent Attribution",
            vec![Box::new(AgentAttributionWidget::default())],
        ));

        #[cfg_attr(not(feature = "local_fs"), allow(unused_mut))]
        let mut other_widgets: Vec<Box<dyn SettingsWidget<View = Self>>> = vec![
            Box::new(UseAgentFooterWidget::default()),
            Box::new(AgentToolbarLayoutEditorWidget),
            Box::new(ShowConversationHistoryWidget::default()),
            Box::new(ThinkingDisplayModeWidget),
        ];
        #[cfg(feature = "local_fs")]
        other_widgets.push(Box::new(ConversationLayoutPreferenceWidget));
        categories.push(Category::new("Other", other_widgets));

        let global_ai_switch_state = SwitchStateHandle::default();
        PageType::new_categorized(
            categories,
            Some(PageTitle::new("Local Agent").with_trailing_element(
                move |_view, appearance, app| {
                    render_global_ai_toggle(&global_ai_switch_state, appearance, app)
                },
            )),
        )
    }

    fn handle_detection_denylist_editor_event(
        &mut self,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorEvent::Blurred | EditorEvent::Enter => {
                let buffer_text = self
                    .autodetection_denylist_editor
                    .as_ref(ctx)
                    .buffer_text(ctx);
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    if let Err(e) = settings
                        .autodetection_command_denylist
                        .set_value(buffer_text, ctx)
                    {
                        log::warn!("Failed to set AI autodetection blacklist commands: {e:?}");
                    }
                })
            }
            EditorEvent::Escape => ctx.emit(WarpAgentPageEvent::FocusModal),
            _ => {}
        }
    }
}

impl View for WarpAgentPageView {
    fn ui_name() -> &'static str {
        "WarpAgentPage"
    }

    fn render(&self, app: &warpui::AppContext) -> Box<dyn warpui::Element> {
        self.page.render(self, app)
    }
}

pub enum WarpAgentPageEvent {
    FocusModal,
}

impl Entity for WarpAgentPageView {
    type Event = WarpAgentPageEvent;
}

#[derive(Debug, Clone, PartialEq)]
pub enum WarpAgentPageAction {
    OpenUrl(String),
    SetVoiceInputToggleKey(VoiceInputToggleKey),
    SetVoiceInputLanguage(String),
    ToggleGlobalAI,
    ToggleActiveAI,
    ToggleIntelligentAutosuggestions,
    TogglePromptSuggestions,
    ToggleCodeSuggestions,
    ToggleNaturalLanguageAutosuggestions,
    ToggleSharedTitleGeneration,
    ToggleGitOperationsAutogen,
    ToggleAIInputAutoDetection,
    ToggleNLDInTerminal,
    ToggleUseAgentToolbar,
    ToggleVoiceInput,
    HyperlinkClick(HyperlinkUrl),
    ToggleShowInputHintText,
    ToggleAiCommandSearchHashTrigger,
    ToggleShowAgentTips,
    SetThinkingDisplayMode(ThinkingDisplayMode),
    SetPromptSubmissionMode(PromptSubmissionMode),
    SetLongRunningCommandSubmissionMode(LongRunningCommandSubmissionMode),
    ToggleFileBasedMcp,
    ToggleIncludeAgentCommandsInHistory,
    ToggleAutoApproveBypassesCommandDenylist,
    ToggleAgentAttribution,

    #[cfg(feature = "local_fs")]
    SetConversationLayout(crate::util::file::external_editor::settings::OpenConversationPreference),
    ToggleShowConversationHistory,
}

impl TypedActionView for WarpAgentPageView {
    type Action = WarpAgentPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            WarpAgentPageAction::OpenUrl(url) => {
                ctx.open_url(url.as_str());
            }
            WarpAgentPageAction::SetVoiceInputToggleKey(key) => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.voice_input_toggle_key.set_value(*key, ctx));
                    report_if_error!(
                        settings
                            .explicitly_interacted_with_voice
                            .set_value(true, ctx)
                    );
                });
                ctx.notify();
            }
            WarpAgentPageAction::SetVoiceInputLanguage(language) => {
                let language = language.clone();
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.voice_input_language.set_value(language, ctx));
                });
                ctx.notify();
            }
            WarpAgentPageAction::ToggleGlobalAI => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings.is_any_ai_enabled.toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Global AI setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleActiveAI => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .is_active_ai_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Active AI setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleIntelligentAutosuggestions => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .intelligent_autosuggestions_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Next Command setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::TogglePromptSuggestions => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .prompt_suggestions_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Prompt Suggestions setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleCodeSuggestions => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .code_suggestions_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Code Suggestions setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleNaturalLanguageAutosuggestions => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .natural_language_autosuggestions_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!(
                            "Failed to set value for Natural Language Autosuggestions setting: {e:?}"
                        );
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleSharedTitleGeneration => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .shared_block_title_generation_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_new_value) => {}
                    Err(e) => {
                        log::warn!(
                            "Failed to set value for Shared Block Title Generation setting: {e:?}"
                        );
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleGitOperationsAutogen => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .git_operations_autogen_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Git Operations Autogen setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleAIInputAutoDetection => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .ai_autodetection_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Input Auto-detection: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleNLDInTerminal => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .nld_in_terminal_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_new_value) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for NLD in Terminal: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleUseAgentToolbar => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .should_render_use_agent_footer_for_user_commands
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Use Agent Footer setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::ToggleVoiceInput => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .voice_input_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Voice Input: {e:?}");
                    }
                }
                ctx.notify();
            }
            WarpAgentPageAction::HyperlinkClick(hyperlink) => {
                ctx.notify();
                ctx.open_url(&hyperlink.url);
            }
            WarpAgentPageAction::ToggleShowInputHintText => {
                InputSettings::handle(ctx).update(ctx, |input_settings, ctx| {
                    report_if_error!(input_settings.show_hint_text.toggle_and_save_value(ctx));
                });
            }
            WarpAgentPageAction::ToggleAiCommandSearchHashTrigger => {
                InputSettings::handle(ctx).update(ctx, |input_settings, ctx| {
                    report_if_error!(
                        input_settings
                            .enable_ai_command_search_hash_trigger
                            .toggle_and_save_value(ctx)
                    );
                });
            }
            WarpAgentPageAction::ToggleShowAgentTips => {
                InputSettings::handle(ctx).update(ctx, |input_settings, ctx| match input_settings
                    .show_agent_tips
                    .toggle_and_save_value(ctx)
                {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for Show Agent Tips setting: {e:?}");
                    }
                });
                ctx.notify();
            }
            WarpAgentPageAction::SetThinkingDisplayMode(mode) => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.thinking_display_mode.set_value(*mode, ctx));
                });
                ctx.notify();
            }
            WarpAgentPageAction::SetPromptSubmissionMode(mode) => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(
                        settings
                            .default_prompt_submission_mode
                            .set_value(*mode, ctx)
                    );
                });
                ctx.notify();
            }
            WarpAgentPageAction::SetLongRunningCommandSubmissionMode(mode) => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(
                        settings
                            .long_running_command_submission_mode
                            .set_value(*mode, ctx)
                    );
                });
                ctx.notify();
            }
            WarpAgentPageAction::ToggleFileBasedMcp => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.file_based_mcp_enabled.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            WarpAgentPageAction::ToggleIncludeAgentCommandsInHistory => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(
                        settings
                            .include_agent_commands_in_history
                            .toggle_and_save_value(ctx)
                    );
                });
                ctx.notify();
            }
            WarpAgentPageAction::ToggleAutoApproveBypassesCommandDenylist => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(
                        settings
                            .auto_approve_bypasses_command_denylist
                            .toggle_and_save_value(ctx)
                    );
                });
                ctx.notify();
            }
            #[cfg(feature = "local_fs")]
            WarpAgentPageAction::SetConversationLayout(layout) => {
                crate::util::file::external_editor::EditorSettings::handle(ctx).update(
                    ctx,
                    |settings, ctx| {
                        report_if_error!(
                            settings
                                .open_conversation_layout_preference
                                .set_value(*layout, ctx)
                        );
                    },
                );
                ctx.notify();
            }
            WarpAgentPageAction::ToggleShowConversationHistory => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(
                        settings
                            .show_conversation_history
                            .toggle_and_save_value(ctx)
                    );
                });
                ctx.notify();
            }
            WarpAgentPageAction::ToggleAgentAttribution => {
                // The updated value syncs to warp-server automatically via
                // `CloudPreferencesSyncer` as a `JsonPreference` GSO keyed
                // `Global_AgentAttributionEnabled`; no bespoke server call needed.
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(
                        settings
                            .agent_attribution_enabled
                            .toggle_and_save_value(ctx)
                    );
                });
                ctx.notify();
            }
        }
    }
}

impl SettingsPageMeta for WarpAgentPageView {
    fn section() -> SettingsSection {
        SettingsSection::WarpAgent
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        FeatureFlag::AgentMode.is_enabled()
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

impl From<ViewHandle<WarpAgentPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<WarpAgentPageView>) -> Self {
        SettingsPageViewHandle::WarpAgent(view_handle)
    }
}

/// The page title's trailing widget: the global master switch for all AI features.
fn render_global_ai_toggle(
    switch_state: &SwitchStateHandle,
    appearance: &Appearance,
    app: &AppContext,
) -> Box<dyn Element> {
    let ui_builder = appearance.ui_builder();
    let is_ai_disabled_due_to_remote_session_org_policy =
        AISettings::as_ref(app).is_ai_disabled_due_to_remote_session_org_policy(app);

    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);

    if is_ai_disabled_due_to_remote_session_org_policy {
        row.add_child(
            Container::new(
                ConstrainedBox::new(
                    Container::new(
                        Text::new("Your organization disallows AI when the active pane contains content from a remote session", appearance.ui_font_family(), 12.)
                            .with_color(appearance.theme().ui_warning_color())
                            .finish()
                    )
                    .with_padding_left(8.)
                    .with_padding_right(8.)
                    .finish()
                )
                .with_max_width(400.)
                .finish()
            )
            .with_margin_right(16.)
            .finish()
        );
    }

    row.add_child(
        Container::new(
            ui_builder
                .switch(switch_state.clone())
                .check(AISettings::as_ref(app).is_any_ai_enabled(app))
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(WarpAgentPageAction::ToggleGlobalAI);
                })
                .finish(),
        )
        .with_padding_right(TOGGLE_BUTTON_RIGHT_PADDING)
        .finish(),
    );

    row.finish()
}

fn is_next_command_toggleable(app: &AppContext) -> bool {
    AISettings::as_ref(app)
        .intelligent_autosuggestions_enabled_internal
        .is_supported_on_current_platform()
}

fn is_prompt_suggestions_toggleable(app: &AppContext) -> bool {
    AISettings::as_ref(app)
        .prompt_suggestions_enabled_internal
        .is_supported_on_current_platform()
}

fn is_suggested_code_banners_toggleable(app: &AppContext) -> bool {
    AISettings::as_ref(app)
        .code_suggestions_enabled_internal
        .is_supported_on_current_platform()
}

fn is_natural_language_autosuggestions_toggleable(app: &AppContext) -> bool {
    FeatureFlag::PredictAMQueries.is_enabled()
        && AISettings::as_ref(app)
            .natural_language_autosuggestions_enabled_internal
            .is_supported_on_current_platform()
}

fn is_shared_block_title_generation_toggleable(app: &AppContext) -> bool {
    FeatureFlag::SharedBlockTitleGeneration.is_enabled()
        && AISettings::as_ref(app)
            .shared_block_title_generation_enabled_internal
            .is_supported_on_current_platform()
}

fn is_git_operations_autogen_toggleable(app: &AppContext) -> bool {
    FeatureFlag::GitOperationsInCodeReview.is_enabled()
        && AISettings::as_ref(app)
            .git_operations_autogen_enabled_internal
            .is_supported_on_current_platform()
}

/// The "Active AI" category's header trailing widget: the master switch for all of
/// the category's child settings.
fn render_active_ai_toggle(toggle: &SwitchStateHandle, app: &AppContext) -> Box<dyn Element> {
    let ai_settings = AISettings::as_ref(app);
    let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
    Container::new(render_ai_feature_switch(
        toggle.clone(),
        *ai_settings.is_active_ai_enabled_internal,
        is_any_ai_enabled,
        WarpAgentPageAction::ToggleActiveAI,
        app,
    ))
    .with_padding_right(TOGGLE_BUTTON_RIGHT_PADDING)
    .finish()
}

#[derive(Default)]
struct NextCommandWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for NextCommandWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "active ai a.i. next command suggestions"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        is_next_command_toggleable(app)
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);

        Flex::column()
            .with_child(
                render_ai_setting_toggle::<IntelligentAutosuggestionsEnabled>(
                    "Next Command",
                    WarpAgentPageAction::ToggleIntelligentAutosuggestions,
                    *ai_settings.intelligent_autosuggestions_enabled_internal,
                    is_toggleable,
                    self.toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
            )
            .with_child(render_ai_setting_description(
                NEXT_COMMAND_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }
}

#[derive(Default)]
struct PromptSuggestionsWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for PromptSuggestionsWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "active ai a.i. prompt suggestions"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        is_prompt_suggestions_toggleable(app)
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(
                render_ai_setting_toggle::<AgentModeQuerySuggestionsEnabled>(
                    "Prompt Suggestions",
                    WarpAgentPageAction::TogglePromptSuggestions,
                    *ai_settings.prompt_suggestions_enabled_internal,
                    is_toggleable,
                    self.toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
            )
            .with_child(render_ai_setting_description(
                PROMPT_SUGGESTIONS_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }
}

#[derive(Default)]
struct SuggestedCodeBannersWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for SuggestedCodeBannersWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "active ai a.i. code diffs suggested banners"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        is_suggested_code_banners_toggleable(app)
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(
                render_ai_setting_toggle::<AgentModeQuerySuggestionsEnabled>(
                    "Suggested Code Banners",
                    WarpAgentPageAction::ToggleCodeSuggestions,
                    *ai_settings.code_suggestions_enabled_internal,
                    is_toggleable,
                    self.toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
            )
            .with_child(render_ai_setting_description(
                SUGGESTED_CODE_BANNERS_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }
}

#[derive(Default)]
struct NaturalLanguageAutosuggestionsWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for NaturalLanguageAutosuggestionsWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "active ai a.i. natural language autosuggestions passive"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        is_natural_language_autosuggestions_toggleable(app)
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(render_ai_setting_toggle::<
                NaturalLanguageAutosuggestionsEnabled,
            >(
                "Natural Language Autosuggestions",
                WarpAgentPageAction::ToggleNaturalLanguageAutosuggestions,
                *ai_settings.natural_language_autosuggestions_enabled_internal,
                is_toggleable,
                self.toggle.clone(),
                &view.local_only_icon_tooltip_states,
                app,
            ))
            .with_child(render_ai_setting_description(
                NATURAL_LANGUAGE_AUTOSUGGESTIONS,
                is_toggleable,
                app,
            ))
            .finish()
    }
}

#[derive(Default)]
struct SharedBlockTitleGenerationWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for SharedBlockTitleGenerationWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "active ai a.i. shared block title generation"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        is_shared_block_title_generation_toggleable(app)
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(
                render_ai_setting_toggle::<SharedBlockTitleGenerationEnabled>(
                    "Shared Block Title Generation",
                    WarpAgentPageAction::ToggleSharedTitleGeneration,
                    *ai_settings.shared_block_title_generation_enabled_internal,
                    is_toggleable,
                    self.toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
            )
            .with_child(render_ai_setting_description(
                SHARED_BLOCK_TITLE_GENERATION_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }
}

#[derive(Default)]
struct GitOperationsAutogenWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for GitOperationsAutogenWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "active ai a.i. unit tests commit pull request pr git code review autogen generate"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        is_git_operations_autogen_toggleable(app)
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(render_ai_setting_toggle::<GitOperationsAutogenEnabled>(
                "Commit & Pull Request Generation",
                WarpAgentPageAction::ToggleGitOperationsAutogen,
                *ai_settings.git_operations_autogen_enabled_internal,
                is_toggleable,
                self.toggle.clone(),
                &view.local_only_icon_tooltip_states,
                app,
            ))
            .with_child(render_ai_setting_description(
                GIT_OPERATIONS_AUTOGEN_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }
}

#[derive(Default)]
struct NaturalLanguageDetectionWidget {
    incorrect_autodetection_highlight_index: HighlightedHyperlink,
    autodetection_toggle: SwitchStateHandle,
    nld_in_terminal_toggle: SwitchStateHandle,
}

impl SettingsWidget for NaturalLanguageDetectionWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "agent ai natural language detection autodetection prompt terminal command denylist permissions"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        Self::render_natural_language_detection_section(
            self.incorrect_autodetection_highlight_index.clone(),
            self.autodetection_toggle.clone(),
            self.nld_in_terminal_toggle.clone(),
            view,
            ai_settings,
            appearance,
            app,
        )
    }
}

#[derive(Default)]
struct ShowInputHintTextWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for ShowInputHintTextWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "agent ai input show hint text"
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        render_ai_setting_toggle::<ShowHintText>(
            "Show input hint text",
            WarpAgentPageAction::ToggleShowInputHintText,
            *InputSettings::as_ref(app).show_hint_text,
            is_any_ai_enabled,
            self.toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        )
    }
}

#[derive(Default)]
struct AiCommandSearchHashTriggerWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for AiCommandSearchHashTriggerWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "# hash pound trigger ai command search shorthand shell comment"
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        render_ai_setting_toggle::<EnableAiCommandSearchHashTrigger>(
            "Enable '#' trigger for AI Command Search",
            WarpAgentPageAction::ToggleAiCommandSearchHashTrigger,
            *InputSettings::as_ref(app).enable_ai_command_search_hash_trigger,
            is_any_ai_enabled,
            self.toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        )
    }
}

#[derive(Default)]
struct ShowAgentTipsWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for ShowAgentTipsWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "agent ai show agent tips"
    }

    fn should_render(&self, _app: &AppContext) -> bool {
        FeatureFlag::AgentTips.is_enabled()
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        render_ai_setting_toggle::<ShowAgentTips>(
            "Show agent tips",
            WarpAgentPageAction::ToggleShowAgentTips,
            *InputSettings::as_ref(app).show_agent_tips,
            is_any_ai_enabled,
            self.toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        )
    }
}

#[derive(Default)]
struct IncludeAgentCommandsInHistoryWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for IncludeAgentCommandsInHistoryWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "agent ai include agent-executed commands in history shell"
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
        render_ai_setting_toggle::<IncludeAgentCommandsInHistory>(
            "Include agent-executed commands in history",
            WarpAgentPageAction::ToggleIncludeAgentCommandsInHistory,
            *ai_settings.include_agent_commands_in_history,
            is_any_ai_enabled,
            self.toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        )
    }
}

#[derive(Default)]
struct AutoApproveBypassesCommandDenylistWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for AutoApproveBypassesCommandDenylistWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "agent ai auto-approve fast forward bypass denylist permissions"
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
        Flex::column()
            .with_child(render_ai_setting_toggle::<AutoApproveBypassesCommandDenylist>(
                "Allow auto-approve to bypass command denylist",
                WarpAgentPageAction::ToggleAutoApproveBypassesCommandDenylist,
                *ai_settings.auto_approve_bypasses_command_denylist,
                is_any_ai_enabled,
                self.toggle.clone(),
                &view.local_only_icon_tooltip_states,
                app,
            ))
            .with_child(render_ai_setting_description(
                "When enabled, fast forward and auto-approve run denylisted commands without asking for confirmation.",
                is_any_ai_enabled,
                app,
            ))
            .finish()
    }
}

#[derive(Default)]
struct PromptSubmissionModeWidget;

impl SettingsWidget for PromptSubmissionModeWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "agent ai default prompt submission mode queue interrupt steer next turn auto-queue long-running long running lrc"
    }

    fn should_render(&self, _app: &AppContext) -> bool {
        FeatureFlag::QueueSlashCommand.is_enabled()
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);

        let mut column = Flex::column().with_child(render_dropdown_item(
            appearance,
            "Default prompt submission mode",
            Some(
                "What happens when you submit a new prompt while the agent is still \
                 responding. You can override this per conversation using the auto-queue \
                 toggle.",
            ),
            None,
            LocalOnlyIconState::for_setting(
                PromptSubmissionMode::storage_key(),
                PromptSubmissionMode::sync_to_cloud(),
                &mut view.local_only_icon_tooltip_states.borrow_mut(),
                app,
            ),
            (!is_any_ai_enabled).then(|| appearance.theme().disabled_ui_text_color()),
            &view.default_prompt_submission_mode_dropdown,
        ));

        // With Queue selected, prompts already queue until the end of the full response, so
        // the LRC mode is hidden.
        if ai_settings.default_prompt_submission_mode != PromptSubmissionMode::Queue {
            column.add_child(
                Container::new(render_dropdown_item(
                    appearance,
                    "Default long-running command submission mode",
                    Some(
                        "What happens when you submit a prompt while an agent is driving an \
                         agent-requested long-running command. Queued prompts are sent to the \
                         agent when the command finishes.",
                    ),
                    None,
                    LocalOnlyIconState::for_setting(
                        LongRunningCommandSubmissionMode::storage_key(),
                        LongRunningCommandSubmissionMode::sync_to_cloud(),
                        &mut view.local_only_icon_tooltip_states.borrow_mut(),
                        app,
                    ),
                    (!is_any_ai_enabled).then(|| appearance.theme().disabled_ui_text_color()),
                    &view.lrc_submission_mode_dropdown,
                ))
                .with_margin_top(styles::DESCRIPTION_MARGIN_BOTTOM)
                .finish(),
            );
        }

        column.finish()
    }
}

impl NaturalLanguageDetectionWidget {
    fn render_natural_language_detection_section(
        incorrect_autodetection_highlight_index: HighlightedHyperlink,
        autodetection_toggle: SwitchStateHandle,
        nld_in_terminal_toggle: SwitchStateHandle,
        view: &WarpAgentPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let is_toggleable = ai_settings.is_any_ai_enabled(app);
        let is_nld_enabled = *ai_settings.ai_autodetection_enabled_internal.value();

        let autodetection_denylist_input_field = appearance
            .ui_builder()
            .text_input(view.autodetection_denylist_editor.clone())
            .with_style(UiComponentStyles {
                width: Some(280.),
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
            .finish();

        let mut section = Flex::column();

        if FeatureFlag::AgentView.is_enabled() {
            section.add_children([
                render_ai_setting_toggle::<NLDInTerminalEnabled>(
                    "Autodetect agent prompts in terminal input",
                    WarpAgentPageAction::ToggleNLDInTerminal,
                    ai_settings.is_nld_in_terminal_enabled(app),
                    is_toggleable,
                    nld_in_terminal_toggle,
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
                render_ai_setting_toggle::<AIAutoDetectionEnabled>(
                    "Autodetect terminal commands in agent input",
                    WarpAgentPageAction::ToggleAIInputAutoDetection,
                    is_nld_enabled,
                    is_toggleable,
                    autodetection_toggle,
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
            ])
        } else {
            static NATURAL_LANGUAGE_DETECTION_DESCRIPTION_FRAGMENTS: LazyLock<
                Vec<FormattedTextFragment>,
            > = LazyLock::new(|| {
                vec![FormattedTextFragment::plain_text(
                    "Enabling natural language detection will detect when natural language is written in the terminal input, and then automatically switch to Agent Mode for AI queries.",
                )]
            });

            section.add_children([
                render_ai_setting_toggle::<AIAutoDetectionEnabled>(
                    "Natural language detection",
                    WarpAgentPageAction::ToggleAIInputAutoDetection,
                    is_nld_enabled,
                    is_toggleable,
                    autodetection_toggle,
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
                Container::new(
                    FormattedTextElement::new(
                        FormattedText::new([FormattedTextLine::Line(
                            (*NATURAL_LANGUAGE_DETECTION_DESCRIPTION_FRAGMENTS).clone(),
                        )]),
                        CONTENT_FONT_SIZE,
                        appearance.ui_font_family(),
                        appearance.ui_font_family(),
                        styles::description_font_color(is_toggleable, app).into(),
                        incorrect_autodetection_highlight_index,
                    )
                    .with_hyperlink_font_color(appearance.theme().accent().into_solid())
                    .register_default_click_handlers(|url, ctx, _| {
                        ctx.dispatch_typed_action(WarpAgentPageAction::HyperlinkClick(url));
                    })
                    .finish(),
                )
                .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
                .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
                .finish(),
            ]);
        }

        section
            .with_child(render_ai_setting_label::<AICommandDenylist>(
                "Natural language denylist".to_owned(),
                is_toggleable,
                &view.local_only_icon_tooltip_states,
                app,
            ))
            .with_child(render_ai_setting_description(
                "Commands listed here will never trigger natural language detection.",
                is_toggleable,
                app,
            ))
            .with_child(
                Container::new(autodetection_denylist_input_field)
                    .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                    .finish(),
            )
            .finish()
    }
}

#[derive(Default)]
struct VoiceWidget {
    voice_input_toggle: SwitchStateHandle,
    wispr_highlight_index: HighlightedHyperlink,
}

impl VoiceWidget {
    fn render_voice_section(
        &self,
        view: &WarpAgentPageView,
        appearance: &Appearance,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_any_ai_enabled(app);
        let mut column = Flex::column().with_child(render_ai_setting_toggle::<VoiceInputEnabled>(
            "Voice Input",
            WarpAgentPageAction::ToggleVoiceInput,
            *ai_settings.voice_input_enabled_internal,
            is_toggleable,
            self.voice_input_toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        ));

        let voice_input_description_text_fragments = vec![
            FormattedTextFragment::plain_text(
                "Voice input allows you to control Termloc by speaking directly to your terminal (powered by ",
            ),
            FormattedTextFragment::hyperlink("Wispr Flow", WISPR_FLOW_URL),
            FormattedTextFragment::plain_text(")."),
        ];

        let voice_input_description = FormattedTextElement::new(
            FormattedText::new([FormattedTextLine::Line(
                voice_input_description_text_fragments,
            )]),
            appearance.ui_font_size(),
            appearance.ui_font_family(),
            appearance.ui_font_family(),
            styles::description_font_color(is_toggleable, app).into(),
            self.wispr_highlight_index.clone(),
        )
        .with_hyperlink_font_color(appearance.theme().accent().into_solid())
        .register_default_click_handlers(|url, ctx, _| {
            ctx.dispatch_typed_action(WarpAgentPageAction::HyperlinkClick(url));
        });

        column.add_child(
            Container::new(voice_input_description.finish())
                .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
                .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
                .finish(),
        );

        if ai_settings.is_voice_input_enabled(app) {
            column.add_child(render_dropdown_item(
                appearance,
                "Key for Activating Voice Input",
                Some("Press and hold to activate."),
                None,
                LocalOnlyIconState::for_setting(
                    VoiceInputToggleKey::storage_key(),
                    VoiceInputToggleKey::sync_to_cloud(),
                    &mut view.local_only_icon_tooltip_states.borrow_mut(),
                    app,
                ),
                None,
                &view.voice_input_toggle_key_dropdown,
            ));
            column.add_child(render_filterable_dropdown_item(
                appearance,
                "Speech Language",
                Some("Language used when transcribing voice input."),
                None,
                LocalOnlyIconState::for_setting(
                    VoiceInputLanguage::storage_key(),
                    VoiceInputLanguage::sync_to_cloud(),
                    &mut view.local_only_icon_tooltip_states.borrow_mut(),
                    app,
                ),
                None,
                &view.voice_input_language_dropdown,
            ));
        }

        column.finish()
    }
}

impl SettingsWidget for VoiceWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "voice agent ai a.i. speech input natural language talk english spanish french german estonian finnish"
    }

    fn should_render(&self, _app: &AppContext) -> bool {
        cfg!(feature = "voice_input")
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        self.render_voice_section(view, appearance, app)
    }
}
struct OtherAIWidget;

impl OtherAIWidget {
    fn create_thinking_display_mode_dropdown(
        ctx: &mut ViewContext<WarpAgentPageView>,
    ) -> ViewHandle<Dropdown<WarpAgentPageAction>> {
        let items: Vec<DropdownItem<WarpAgentPageAction>> = ThinkingDisplayMode::iter()
            .map(|mode| {
                DropdownItem::new(
                    mode.display_name(),
                    WarpAgentPageAction::SetThinkingDisplayMode(mode),
                )
            })
            .collect();

        ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_menu_max_height(AI_SETTINGS_DROPDOWN_MAX_HEIGHT, ctx);
            dropdown.add_items(items, ctx);
            dropdown
        })
    }

    fn create_default_prompt_submission_mode_dropdown(
        ctx: &mut ViewContext<WarpAgentPageView>,
    ) -> ViewHandle<Dropdown<WarpAgentPageAction>> {
        let items: Vec<DropdownItem<WarpAgentPageAction>> = PromptSubmissionMode::iter()
            .filter(PromptSubmissionMode::is_available)
            .map(|mode| {
                DropdownItem::new(
                    mode.display_name(),
                    WarpAgentPageAction::SetPromptSubmissionMode(mode),
                )
            })
            .collect();

        ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_menu_max_height(AI_SETTINGS_DROPDOWN_MAX_HEIGHT, ctx);
            dropdown.add_items(items, ctx);
            dropdown
        })
    }

    fn create_lrc_submission_mode_dropdown(
        ctx: &mut ViewContext<WarpAgentPageView>,
    ) -> ViewHandle<Dropdown<WarpAgentPageAction>> {
        let items: Vec<DropdownItem<WarpAgentPageAction>> =
            LongRunningCommandSubmissionMode::iter()
                .map(|mode| {
                    DropdownItem::new(
                        mode.display_name(),
                        WarpAgentPageAction::SetLongRunningCommandSubmissionMode(mode),
                    )
                })
                .collect();

        ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_menu_max_height(AI_SETTINGS_DROPDOWN_MAX_HEIGHT, ctx);
            dropdown.add_items(items, ctx);
            dropdown
        })
    }
}

#[derive(Default)]
struct UseAgentFooterWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for UseAgentFooterWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "other use agent footer full terminal use long running commands"
    }

    fn should_render(&self, _app: &AppContext) -> bool {
        FeatureFlag::AgentView.is_enabled()
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_any_ai_enabled(app);

        Flex::column()
            .with_child(render_ai_setting_toggle::<
                ShouldRenderUseAgentToolbarForUserCommands,
            >(
                "Show \"Use Agent\" footer",
                WarpAgentPageAction::ToggleUseAgentToolbar,
                *ai_settings.should_render_use_agent_footer_for_user_commands,
                is_toggleable,
                self.toggle.clone(),
                &view.local_only_icon_tooltip_states,
                app,
            ))
            .with_child(render_ai_setting_description(
                "Shows hint to use the \"Full Terminal Use\"-enabled agent in long running commands.",
                is_toggleable,
                app,
            ))
            .finish()
    }
}

#[derive(Default)]
struct AgentToolbarLayoutEditorWidget;

impl SettingsWidget for AgentToolbarLayoutEditorWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "other agent toolbar layout chip chips rearrange re-arrange"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        FeatureFlag::AgentView.is_enabled()
            && FeatureFlag::AgentToolbarEditor.is_enabled()
            && AISettings::as_ref(app).is_any_ai_enabled(app)
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        render_toolbar_layout_editor(&view.agent_toolbar_inline_editor, appearance)
    }
}

#[derive(Default)]
struct ShowConversationHistoryWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for ShowConversationHistoryWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "other conversation history tools panel collapse expand hide"
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_any_ai_enabled(app);
        render_ai_setting_toggle::<ShowConversationHistory>(
            "Show conversation history in tools panel",
            WarpAgentPageAction::ToggleShowConversationHistory,
            *ai_settings.show_conversation_history,
            is_toggleable,
            self.toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        )
    }
}

#[derive(Default)]
struct ThinkingDisplayModeWidget;

impl SettingsWidget for ThinkingDisplayModeWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "other agent thinking display reasoning collapse never show expanded"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        render_dropdown_item(
            appearance,
            "Agent thinking display",
            Some("Controls how reasoning/thinking traces are displayed."),
            None,
            LocalOnlyIconState::for_setting(
                ThinkingDisplayMode::storage_key(),
                ThinkingDisplayMode::sync_to_cloud(),
                &mut view.local_only_icon_tooltip_states.borrow_mut(),
                app,
            ),
            (!is_any_ai_enabled).then(|| appearance.theme().disabled_ui_text_color()),
            &view.thinking_display_mode_dropdown,
        )
    }
}

// TODO: OpenConversationLayoutPreference should not depend on local_fs, but it lives under the
// external editor settings which does require local_fs. It was a mistake to put it there, but now
// we keep it there for backward compatibility.
#[cfg(feature = "local_fs")]
#[derive(Default)]
struct ConversationLayoutPreferenceWidget;

#[cfg(feature = "local_fs")]
impl SettingsWidget for ConversationLayoutPreferenceWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "other preferred layout opening existing agent conversations new tab split pane"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        use crate::util::file::external_editor::settings::OpenConversationLayoutPreference;

        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        render_dropdown_item(
            appearance,
            "Preferred layout when opening existing agent conversations",
            None,
            None,
            LocalOnlyIconState::for_setting(
                OpenConversationLayoutPreference::storage_key(),
                OpenConversationLayoutPreference::sync_to_cloud(),
                &mut view.local_only_icon_tooltip_states.borrow_mut(),
                app,
            ),
            (!is_any_ai_enabled).then(|| appearance.theme().disabled_ui_text_color()),
            &view.conversation_layout_dropdown,
        )
    }
}

/// The presentation state of the agent attribution toggle, derived from the
/// org-level [`AdminEnablementSetting`], the user's stored preference, and
/// whether AI is globally enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentAttributionToggleState {
    /// Whether the toggle is rendered in the checked state.
    pub(crate) is_enabled: bool,
    /// Whether the org has forced the value (locking the toggle with a tooltip).
    pub(crate) is_forced_by_org: bool,
    /// Whether the toggle should be rendered as non-interactive overall
    /// (forced by the org, or AI globally disabled).
    pub(crate) is_disabled: bool,
}

/// Derive the toggle state from its three inputs.
pub(crate) fn derive_agent_attribution_toggle_state(
    org_setting: &AdminEnablementSetting,
    user_pref: bool,
    is_any_ai_enabled: bool,
) -> AgentAttributionToggleState {
    let is_forced_by_org = match org_setting {
        AdminEnablementSetting::Enable | AdminEnablementSetting::Disable => true,
        AdminEnablementSetting::RespectUserSetting => false,
    };
    let is_enabled = match org_setting {
        AdminEnablementSetting::Enable => true,
        AdminEnablementSetting::Disable => false,
        AdminEnablementSetting::RespectUserSetting => user_pref,
    };
    AgentAttributionToggleState {
        is_enabled,
        is_forced_by_org,
        is_disabled: is_forced_by_org || !is_any_ai_enabled,
    }
}

#[derive(Default)]
struct AgentAttributionWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for AgentAttributionWidget {
    type View = WarpAgentPageView;

    fn search_terms(&self) -> &str {
        "agent attribution commit pull request co-author author credit warp"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);

        let workspaces = UserWorkspaces::as_ref(app);
        let scope = workspaces.team_context(&view.self_handle, app);
        let org_setting = workspaces.get_agent_attribution_setting(&scope);
        let state = derive_agent_attribution_toggle_state(
            &org_setting,
            *ai_settings.agent_attribution_enabled,
            is_any_ai_enabled,
        );

        let ui_builder = appearance.ui_builder();
        let toggle = if state.is_forced_by_org {
            ui_builder
                .switch(self.toggle.clone())
                .check(state.is_enabled)
                .with_tooltip(TooltipConfig {
                    text: "This option is enforced by your organization's settings and cannot be customized.".to_string(),
                    styles: ui_builder.default_tool_tip_styles(),
                })
                .disable()
                .build()
                .finish()
        } else if !is_any_ai_enabled {
            ui_builder
                .switch(self.toggle.clone())
                .check(state.is_enabled)
                .with_disabled(true)
                .build()
                .finish()
        } else {
            ui_builder
                .switch(self.toggle.clone())
                .check(state.is_enabled)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(WarpAgentPageAction::ToggleAgentAttribution);
                })
                .finish()
        };

        let toggle_row = build_toggle_element(
            render_body_item_label::<WarpAgentPageAction>(
                "Enable agent attribution".to_string(),
                Some(styles::header_font_color(!state.is_disabled, app)),
                None,
                LocalOnlyIconState::Hidden,
                ToggleState::Enabled,
                appearance,
            ),
            toggle,
            appearance,
            None,
        );

        Flex::column()
            .with_child(toggle_row)
            .with_child(render_ai_setting_description(
                "Local Agent can add attribution to commit messages and pull requests it creates",
                !state.is_disabled,
                app,
            ))
            .finish()
    }
}

#[cfg(test)]
#[path = "warp_agent_page_tests.rs"]
mod tests;
