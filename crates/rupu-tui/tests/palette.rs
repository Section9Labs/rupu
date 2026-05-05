use rupu_tui::state::NodeStatus;
use rupu_tui::view::palette::{glyph_for, color_for};

#[test]
fn waiting_is_dim_circle() {
    assert_eq!(glyph_for(NodeStatus::Waiting), '○');
}

#[test]
fn awaiting_is_pause_glyph() {
    assert_eq!(glyph_for(NodeStatus::Awaiting), '⏸');
}

#[test]
fn complete_is_check() {
    assert_eq!(glyph_for(NodeStatus::Complete), '✓');
}

#[test]
fn no_color_returns_reset_for_all_statuses() {
    std::env::set_var("NO_COLOR", "1");
    assert!(matches!(color_for(NodeStatus::Working), ratatui::style::Color::Reset));
    std::env::remove_var("NO_COLOR");
}
