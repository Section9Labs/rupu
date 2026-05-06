// This test mutates the NO_COLOR env var globally. It lives in its own
// test binary so cargo cannot race it against other tests that read
// color_for in parallel.
use rupu_tui::state::NodeStatus;
use rupu_tui::view::palette::color_for;

#[test]
fn no_color_returns_reset_for_all_statuses() {
    std::env::set_var("NO_COLOR", "1");
    assert!(matches!(
        color_for(NodeStatus::Working),
        ratatui::style::Color::Reset
    ));
    std::env::remove_var("NO_COLOR");
}
