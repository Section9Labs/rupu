use ratatui::style::Color;

use crate::state::NodeStatus;

pub fn glyph_for(s: NodeStatus) -> char {
    match s {
        NodeStatus::Waiting => '○',
        NodeStatus::Active => '●',
        NodeStatus::Working => '◐',
        NodeStatus::Complete => '✓',
        NodeStatus::Failed => '✗',
        NodeStatus::SoftFailed => '!',
        NodeStatus::Awaiting => '⏸',
        NodeStatus::Retrying => '↺',
        NodeStatus::Skipped => '⊘',
    }
}

pub fn color_for(s: NodeStatus) -> Color {
    if std::env::var_os("NO_COLOR").is_some() {
        return Color::Reset;
    }
    match s {
        NodeStatus::Waiting => Color::DarkGray,
        NodeStatus::Active => Color::LightBlue,
        NodeStatus::Working => Color::Blue,
        NodeStatus::Complete => Color::Green,
        NodeStatus::Failed => Color::Red,
        NodeStatus::SoftFailed => Color::Yellow,
        NodeStatus::Awaiting => Color::LightYellow,
        NodeStatus::Retrying => Color::Magenta,
        NodeStatus::Skipped => Color::DarkGray,
    }
}
