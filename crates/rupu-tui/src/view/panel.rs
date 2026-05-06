use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::state::RunModel;
use crate::view::palette::{color_for, glyph_for};

pub fn render_panel(frame: &mut Frame, area: Rect, model: &RunModel, focused: &str) {
    let Some(node) = model.node(focused) else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    let glyph = glyph_for(node.status);
    let style = ratatui::style::Style::default().fg(color_for(node.status));
    lines.push(Line::from(vec![
        Span::styled(format!("{glyph} {}", node.step_id), style),
        Span::raw("  "),
        Span::raw(node.agent.clone()),
    ]));
    lines.push(Line::raw(format!(
        "tokens: {} in / {} out / {} cached",
        node.tokens.input, node.tokens.output, node.tokens.cached,
    )));
    if !node.tools_used.is_empty() {
        lines.push(Line::raw("tools used:"));
        for (tool, n) in &node.tools_used {
            lines.push(Line::raw(format!("  {tool} · {n}")));
        }
    }
    if !node.transcript_tail.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::raw("transcript:"));
        for tl in &node.transcript_tail {
            let truncated: String = tl.chars().take(36).collect();
            lines.push(Line::raw(format!("  {truncated}")));
        }
    }
    if let Some(la) = &node.last_action {
        lines.push(Line::raw(format!("last: {} {}", la.tool, la.summary)));
    }
    if let Some(prompt) = &node.gate_prompt {
        lines.push(Line::raw(format!("⏸ {prompt}")));
    }
    let block = Block::default().borders(Borders::ALL).title("focus");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
