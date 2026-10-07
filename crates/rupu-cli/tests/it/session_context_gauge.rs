//! The session status header's context gauge renders the full
//! `ctx <used>/<limit> <pct>%` form, not a bare percentage.

use rupu_cli::output::fmt::context_gauge_text;

#[test]
fn gauge_shows_used_limit_and_percent() {
    assert_eq!(
        context_gauge_text(52_000, 200_000).as_deref(),
        Some("ctx 52K/200K 26%")
    );
    assert_eq!(
        context_gauge_text(1_500, 1_000_000).as_deref(),
        Some("ctx 1.5K/1.0M 0%")
    );
}

#[test]
fn gauge_is_absent_without_a_turn_or_a_limit() {
    assert_eq!(context_gauge_text(0, 200_000), None);
    assert_eq!(context_gauge_text(52_000, 0), None);
}
