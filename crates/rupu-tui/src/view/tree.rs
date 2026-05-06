use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::state::RunModel;
use crate::view::layout::layout_tree;
use crate::view::palette::{color_for, glyph_for};

pub fn render_tree(
    frame: &mut Frame,
    area: Rect,
    model: &RunModel,
    edges: &[(String, String)],
    focused: &str,
) {
    let ids: Vec<&str> = model.nodes.keys().map(|s| s.as_str()).collect();
    let lines_idx = layout_tree(&ids, edges);

    let mut text_lines: Vec<Line> = Vec::with_capacity(lines_idx.len());
    for (id, depth) in &lines_idx {
        let Some(node) = model.node(id) else { continue };
        let prefix = if id == focused { "> " } else { "  " };
        let indent = "  ".repeat(*depth as usize);
        let glyph = glyph_for(node.status);
        let style = Style::default().fg(color_for(node.status))
            .add_modifier(if id == focused { Modifier::BOLD } else { Modifier::empty() });
        text_lines.push(Line::from(vec![
            Span::raw(prefix),
            Span::raw(indent),
            Span::styled(format!("{glyph} {}", node.step_id), style),
        ]));
    }
    frame.render_widget(Paragraph::new(text_lines), area);
}
