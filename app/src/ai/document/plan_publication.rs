//! Publishing plan documents to Warp Drive ahead of child-agent launch.
//!
//! Callers first call [`prepare_plan_publications`] to kick off publication of
//! a conversation's plans, then await [`wait_for_plan_publications`] before
//! proceeding with work that requires the plans to be server-backed.
