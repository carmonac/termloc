use warp_core::ui::Icon;

pub fn icon_for_context_window_usage(context_window_usage: f32) -> Icon {
    // The circle's solid (white) marks represent the context *remaining*, not
    // the amount used: an empty conversation shows an all-white circle (100%
    // remaining) and counts down to an all-grey circle as the context window
    // fills up (0% remaining). So match the *remaining* fraction
    // (`1 - usage`) to the nearest 10% icon, where `ContextRemainingN`
    // brightens N% of the ring.
    let context_window_remaining = 1.0 - context_window_usage;
    if context_window_remaining >= 0.95 {
        Icon::ContextRemaining100
    } else if context_window_remaining >= 0.85 {
        Icon::ContextRemaining90
    } else if context_window_remaining >= 0.75 {
        Icon::ContextRemaining80
    } else if context_window_remaining >= 0.65 {
        Icon::ContextRemaining70
    } else if context_window_remaining >= 0.55 {
        Icon::ContextRemaining60
    } else if context_window_remaining >= 0.45 {
        Icon::ContextRemaining50
    } else if context_window_remaining >= 0.35 {
        Icon::ContextRemaining40
    } else if context_window_remaining >= 0.25 {
        Icon::ContextRemaining30
    } else if context_window_remaining >= 0.15 {
        Icon::ContextRemaining20
    } else if context_window_remaining >= 0.05 {
        Icon::ContextRemaining10
    } else {
        Icon::ContextRemaining0
    }
}

#[cfg(test)]
#[path = "context_window_icon_tests.rs"]
mod tests;
