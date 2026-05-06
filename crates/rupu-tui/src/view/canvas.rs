use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::state::{NodeStatus, RunModel};
use crate::view::layout::{layout_canvas, Position};
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

    // Edges first — boxes paint over them where they overlap.
    draw_edges(frame, area, model, edges, &positions);

    for (id, pos) in &positions {
        let Some(node) = model.node(id) else { continue };
        let Some((x, y)) = card_origin(area, *pos) else { continue };
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

/// Compute the top-left origin of a card given its grid position.
/// Returns `None` if the card would clip outside `area`.
fn card_origin(area: Rect, pos: Position) -> Option<(u16, u16)> {
    let x = area.x + pos.col * (CARD_W + COL_GAP);
    let y = area.y + pos.row * (CARD_H + ROW_GAP);
    if x + CARD_W > area.x + area.width || y + CARD_H > area.y + area.height {
        return None;
    }
    Some((x, y))
}

/// Paint connector lines between adjacent cards. Edges are drawn
/// before the cards so the box borders overpaint where they meet.
/// Edge colour follows the upstream node's status — green when the
/// parent completed, blue when running, grey when waiting, red on
/// failure (mirrors the Okesu canvas idiom).
fn draw_edges(
    frame: &mut Frame,
    area: Rect,
    model: &RunModel,
    edges: &[(String, String)],
    positions: &std::collections::BTreeMap<String, Position>,
) {
    use std::collections::BTreeMap;

    // Group edges by parent so we can detect fan-out and pick the
    // right Unicode tee.
    let mut by_parent: BTreeMap<&String, Vec<&String>> = BTreeMap::new();
    for (p, c) in edges {
        by_parent.entry(p).or_default().push(c);
    }

    let buf = frame.buffer_mut();
    for (parent_id, children) in by_parent {
        let Some(parent_pos) = positions.get(parent_id) else { continue };
        let Some((px, py)) = card_origin(area, *parent_pos) else { continue };
        let parent_status = model
            .node(parent_id)
            .map(|n| n.status)
            .unwrap_or(NodeStatus::Waiting);
        let style = Style::default().fg(color_for(parent_status));

        // Right-edge midpoint of the parent card, where the line exits.
        let exit_x = px + CARD_W;
        let exit_y = py + CARD_H / 2;

        let same_row_children: Vec<&&String> = children
            .iter()
            .filter(|c| {
                positions
                    .get(**c)
                    .is_some_and(|cp| cp.row == parent_pos.row)
            })
            .collect();
        let other_row_children: Vec<&&String> = children
            .iter()
            .filter(|c| {
                positions
                    .get(**c)
                    .is_some_and(|cp| cp.row != parent_pos.row)
            })
            .collect();

        // Same-row hop: straight horizontal arrow into the next column.
        for child in &same_row_children {
            let Some(child_pos) = positions.get(**child) else {
                continue;
            };
            let Some((cx, _cy)) = card_origin(area, *child_pos) else {
                continue;
            };
            // Span runs from exit_x..cx exclusive; arrow tip at cx-1.
            for x in exit_x..cx {
                if x >= area.x + area.width {
                    break;
                }
                let symbol = if x == cx.saturating_sub(1) { "▶" } else { "─" };
                buf[(x, exit_y)].set_symbol(symbol).set_style(style);
            }
        }

        // Fan-out: drop down from the parent's bottom centre, then
        // turn into each child. Only if there's at least one
        // off-row child; otherwise no vertical drop needed.
        if !other_row_children.is_empty() {
            let drop_x = px + CARD_W / 2;
            let drop_top_y = py + CARD_H; // first cell BELOW the card
            let mut max_child_y = drop_top_y;
            for child in &other_row_children {
                let Some(child_pos) = positions.get(**child) else {
                    continue;
                };
                let Some((cx, cy)) = card_origin(area, *child_pos) else {
                    continue;
                };
                let mid_y = cy + CARD_H / 2;
                if mid_y > max_child_y {
                    max_child_y = mid_y;
                }
                // Horizontal stub from drop_x to child's left edge,
                // at the child's row midline.
                let from_x = drop_x.min(cx.saturating_sub(1));
                let to_x = drop_x.max(cx.saturating_sub(1));
                for x in (from_x + 1)..to_x {
                    if x >= area.x + area.width || mid_y >= area.y + area.height {
                        break;
                    }
                    buf[(x, mid_y)].set_symbol("─").set_style(style);
                }
                // Arrow tip at the child's left edge.
                if cx > area.x && mid_y < area.y + area.height {
                    buf[(cx - 1, mid_y)].set_symbol("▶").set_style(style);
                }
                // Corner / tee at the drop column.
                if drop_x < area.x + area.width && mid_y < area.y + area.height {
                    let is_last_child = other_row_children
                        .iter()
                        .all(|other| {
                            let other_y = positions
                                .get(**other)
                                .map(|p| p.row)
                                .unwrap_or(0);
                            let this_y = child_pos.row;
                            other_y <= this_y
                        });
                    let symbol = if is_last_child { "└" } else { "├" };
                    buf[(drop_x, mid_y)].set_symbol(symbol).set_style(style);
                }
            }
            // Vertical line from the parent's bottom-centre to the
            // last child's row.
            for y in drop_top_y..max_child_y {
                if y >= area.y + area.height {
                    break;
                }
                if drop_x < area.x + area.width {
                    buf[(drop_x, y)].set_symbol("│").set_style(style);
                }
            }
        }
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
