use i18n::t;
#[cfg(not(target_family = "wasm"))]
use local_agent::LocalModel;
use pathfinder_color::ColorU;
#[cfg(not(target_family = "wasm"))]
use thousands::Separable as _;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, Expanded, Flex, MainAxisSize,
    ParentElement as _, Percentage, Radius, Rect, Stack, Text,
};
use warpui::prelude::{Align, CrossAxisAlignment};
use warpui::text_layout::ClipConfig;
use warpui::{AppContext, Element, SingletonEntity as _};

#[cfg(not(target_family = "wasm"))]
use crate::ai::llms::LLMId;
use crate::ai::llms::LLMSpec;
#[cfg(not(target_family = "wasm"))]
use crate::ai::local::LocalModels;
use crate::appearance::Appearance;
use crate::terminal::input::inline_menu::styles as inline_styles;

const CORNER_RADIUS: f32 = 4.0;
const ROW_SPACING: f32 = 12.0;

pub const MODEL_SPECS_TITLE: &str = "Model Specs";
pub const MODEL_SPECS_DESCRIPTION: &str = "Termloc's benchmarks for how well a model performs in our harness, the rate at which it consumes credits, and task speed.";

pub const REASONING_LEVEL_TITLE: &str = "Reasoning level";
pub const REASONING_LEVEL_DESCRIPTION: &str = "Increased reasoning levels consume more credits and have higher latency, but higher performance for complicated tasks.";

#[cfg(not(target_family = "wasm"))]
pub const LOCAL_MODEL_DETAILS_TITLE: &str = "Model details";
#[cfg(not(target_family = "wasm"))]
pub const LOCAL_MODEL_DETAILS_DESCRIPTION: &str = "As reported by Ollama on this machine.";

pub struct ModelSpecScoresLayout {
    pub bg_bar_color: ColorU,
}

pub fn render_model_spec_scores(
    spec: Option<&LLMSpec>,
    layout: ModelSpecScoresLayout,
    app: &AppContext,
) -> Box<dyn Element> {
    let rows = vec![
        render_score_row(
            t!("Intelligence"),
            spec.map(|spec| spec.quality),
            layout.bg_bar_color,
            app,
        ),
        render_score_row(
            t!("Speed"),
            spec.map(|spec| spec.speed),
            layout.bg_bar_color,
            app,
        ),
        render_score_row(
            t!("Cost"),
            spec.map(|spec| spec.cost),
            layout.bg_bar_color,
            app,
        ),
    ];

    Flex::column()
        .with_spacing(ROW_SPACING)
        .with_children(rows)
        .finish()
}

fn render_score_row(
    name: &str,
    value: Option<f32>,
    bg_bar_color: ColorU,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();

    // Approximate the required width for the longest label ("Intelligence"), and use this as a
    // consistent width for the labels so the labels and bars are vertically aligned.
    //
    // 8 ems is enough space for Intelligence with some right margin to spare.
    let label_width = app.font_cache().em_width(
        appearance.ui_font_family(),
        appearance.monospace_font_size(),
    ) * 8.;
    let label = ConstrainedBox::new(render_row_label(name, appearance, app))
        .with_width(label_width)
        .finish();

    let bar_height = app.font_cache().line_height(
        appearance.monospace_font_size(),
        appearance.line_height_ratio(),
    );

    let row_content: Box<dyn Element> = match value {
        Some(value) => {
            let background_bar = Rect::new()
                .with_background_color(bg_bar_color)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CORNER_RADIUS)))
                .finish();

            let filled_bar = Rect::new()
                .with_background_color(internal_colors::neutral_6(theme))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CORNER_RADIUS)))
                .finish();

            Expanded::new(
                1.,
                ConstrainedBox::new(
                    Stack::new()
                        .with_child(background_bar)
                        .with_child(Percentage::width(value, filled_bar).finish())
                        .finish(),
                )
                .with_height(bar_height)
                .finish(),
            )
            .finish()
        }
        None => {
            let background_bar = Rect::new()
                .with_background_color(bg_bar_color)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CORNER_RADIUS)))
                .finish();

            Expanded::new(
                1.,
                ConstrainedBox::new(
                    Stack::new()
                        .with_child(background_bar)
                        .with_child(
                            Align::new(
                                Text::new(
                                    "?",
                                    appearance.ui_font_family(),
                                    inline_styles::font_size(appearance),
                                )
                                .with_color(
                                    appearance
                                        .theme()
                                        .disabled_text_color(bg_bar_color.into())
                                        .into_solid(),
                                )
                                .finish(),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .with_height(bar_height)
                .finish(),
            )
            .finish()
        }
    };

    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(label)
        .with_child(Expanded::new(1., row_content).finish())
        .finish()
}

fn render_row_label(label: &str, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
    Text::new(
        label.to_string(),
        appearance.ui_font_family(),
        appearance.monospace_font_size(),
    )
    .with_color(
        inline_styles::primary_text_color(
            appearance.theme(),
            inline_styles::menu_background_color(app).into(),
        )
        .into_solid(),
    )
    .finish()
}

pub fn render_model_spec_header(
    title: &str,
    description: &str,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();

    let title = Text::new(
        title.to_string(),
        appearance.ui_font_family(),
        appearance.monospace_font_size(),
    )
    .with_color(
        inline_styles::primary_text_color(theme, inline_styles::menu_background_color(app).into())
            .into_solid(),
    )
    .with_clip(ClipConfig::ellipsis())
    .finish();

    let description = Text::new(
        description.to_string(),
        appearance.ui_font_family(),
        inline_styles::font_size(appearance),
    )
    .with_color(theme.disabled_ui_text_color().into())
    .finish();

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(title)
            .with_child(Container::new(description).with_margin_top(4.).finish())
            .finish(),
    )
    .with_padding_bottom(12.)
    .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
    .finish()
}

/// The installed Ollama model behind `id`, if it is one.
#[cfg(not(target_family = "wasm"))]
pub fn installed_local_model(id: &LLMId, app: &AppContext) -> Option<LocalModel> {
    if !app.has_singleton_model::<LocalModels>() {
        return None;
    }
    LocalModels::as_ref(app)
        .installed_models()
        .iter()
        .find(|model| model.name == id.as_str())
        .cloned()
}

/// Label and value rows describing `model`, leaving out what Ollama didn't report.
#[cfg(not(target_family = "wasm"))]
pub fn local_model_detail_rows(model: &LocalModel) -> Vec<(&'static str, String)> {
    let capabilities = [
        (model.supports_tools, "tools"),
        (model.supports_vision, "vision"),
        (model.supports_thinking, "thinking"),
    ]
    .into_iter()
    .filter_map(|(supported, name)| supported.then_some(name))
    .collect::<Vec<_>>();

    [
        (t!("Parameters"), model.parameter_size.clone()),
        (t!("Quantization"), model.quantization.clone()),
        (
            t!("Context"),
            model
                .context_length
                .map(|tokens| t!("{count} tokens", count = tokens.separate_with_commas())),
        ),
        (
            t!("Size on disk"),
            (model.size > 0).then(|| format_size(model.size)),
        ),
        (
            t!("Supports"),
            (!capabilities.is_empty()).then(|| capabilities.join(" · ")),
        ),
    ]
    .into_iter()
    .filter_map(|(label, value)| value.map(|value| (label, value)))
    .collect()
}

/// Formats a byte count in decimal units, as Ollama reports model sizes.
#[cfg(not(target_family = "wasm"))]
fn format_size(bytes: u64) -> String {
    const GB: f64 = 1e9;
    const MB: f64 = 1e6;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else {
        format!("{:.0} MB", bytes / MB)
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn render_local_model_details(model: &LocalModel, app: &AppContext) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    // Wide enough for "Size on disk" and "Quantization" plus a gap before the value.
    let label_width = app.font_cache().em_width(
        appearance.ui_font_family(),
        appearance.monospace_font_size(),
    ) * 10.;
    let rows = local_model_detail_rows(model)
        .into_iter()
        .map(|(label, value)| {
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    ConstrainedBox::new(render_row_label(label, appearance, app))
                        .with_width(label_width)
                        .finish(),
                )
                .with_child(Expanded::new(1., render_row_label(&value, appearance, app)).finish())
                .finish()
        })
        .collect::<Vec<_>>();

    Flex::column()
        .with_spacing(ROW_SPACING)
        .with_children(rows)
        .finish()
}

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "model_spec_scores_tests.rs"]
mod tests;
