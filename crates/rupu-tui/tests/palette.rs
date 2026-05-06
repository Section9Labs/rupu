use rupu_tui::state::NodeStatus;
use rupu_tui::view::palette::glyph_for;

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
