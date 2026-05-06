use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::state::RunModel;
use crate::view::layout::layout_canvas;
use crate::view::palette::{color_for, glyph_for};

const CARD_W: u16 = 16;
const CARD_H: u16 = 3;
const COL_GAP: u16 = 4;
const ROW_GAP: u16 = 1;
const MIN_USEFUL_WIDTH: u16 = 40;

pub fn render_canvas(
    frame: &mut Frame,
    area: Rect,
    model: &RunModel,
    edges: &[(String, String)],
    focused: &str,
) {
    let ids: Vec<&str> = model.nodes.keys().map(|s| s.as_str()).collect();
    let positions = layout_canvas(&ids, edges);

    for (id, pos) in &positions {
        let Some(node) = model.node(id) else { continue };
        let x = area.x + pos.col * (CARD_W + COL_GAP);
        let y = area.y + pos.row * (CARD_H + ROW_GAP);
        if x + CARD_W > area.x + area.width || y + CARD_H > area.y + area.height {
            continue;
        }
        let rect = Rect { x, y, width: CARD_W, height: CARD_H };
        let glyph = glyph_for(node.status);
        let color = color_for(node.status);
        let mut style = Style::default().fg(color);
        if id == focused {
            style = style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default().borders(Borders::ALL).border_style(style);
        let label = format!("{glyph} {}", trim(&node.step_id, (CARD_W as usize).saturating_sub(4)));
        let para = Paragraph::new(label).style(style).block(block);
        frame.render_widget(para, rect);
    }
}

fn trim(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

pub fn render_canvas_with_warning(
    frame: &mut Frame,
    area: Rect,
    model: &RunModel,
    edges: &[(String, String)],
    focused: &str,
) {
    if area.width < MIN_USEFUL_WIDTH {
        let text = "(canvas truncated — press v for tree view)";
        let para = Paragraph::new(text).style(Style::default().fg(ratatui::style::Color::Yellow));
        frame.render_widget(para, area);
        return;
    }
    render_canvas(frame, area, model, edges, focused);
}
