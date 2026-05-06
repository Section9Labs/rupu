use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::state::{NodeStatus, RunModel};
use crate::view::layout::{layout_canvas, Position};
use crate::view::palette::{color_for, glyph_for};

// Card geometry. The Okesu web canvas uses 240px-wide cards with a
// status-colored top stripe + 2 content rows; we mirror that shape
// in a terminal grid: 22 cols wide × 5 rows tall (top border, stripe,
// row 1, row 2, bottom border). Inter-card gaps leave room for the
// thicker `═══▶` edges.
const CARD_W: u16 = 22;
const CARD_H: u16 = 5;
const COL_GAP: u16 = 4;
const ROW_GAP: u16 = 1;
/// Below this terminal width even one card looks bad; suggest tree.
const MIN_USEFUL_WIDTH: u16 = 50;

pub fn render_canvas(
    frame: &mut Frame,
    area: Rect,
    model: &RunModel,
    edges: &[(String, String)],
    focused: &str,
) {
    paint_dotted_backdrop(frame, area);

    let ids: Vec<&str> = model.nodes.keys().map(|s| s.as_str()).collect();
    let positions = layout_canvas(&ids, edges);

    // Edges first — boxes paint over them where they overlap.
    draw_edges(frame, area, model, edges, &positions);

    let pulse = pulse_phase();
    for (id, pos) in &positions {
        let Some(node) = model.node(id) else { continue };
        draw_card(frame, area, *pos, node, id == focused, pulse);
    }
}

/// Paint `·` into every cell in `area` to give the canvas a
/// dotted backdrop reminiscent of the Okesu authoring/run canvas.
fn paint_dotted_backdrop(frame: &mut Frame, area: Rect) {
    let style = Style::default().fg(Color::DarkGray);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            buf[(x, y)].set_symbol("·").set_style(style);
        }
    }
}

/// Render one DAG node as a multi-row card with a status stripe.
fn draw_card(
    frame: &mut Frame,
    area: Rect,
    pos: Position,
    node: &crate::state::NodeState,
    focused: bool,
    pulse: bool,
) {
    let Some((x, y)) = card_origin(area, pos) else { return };
    let rect = Rect {
        x,
        y,
        width: CARD_W,
        height: CARD_H,
    };
    let glyph = glyph_for(node.status);
    let color = color_for(node.status);

    // Border: rounded corners + status-colored. Bold pulse on
    // running/awaiting (alternates each second via `pulse_phase`).
    let mut border_style = Style::default().fg(color);
    let should_pulse = matches!(
        node.status,
        NodeStatus::Active | NodeStatus::Working | NodeStatus::Awaiting,
    );
    if focused || (should_pulse && pulse) {
        border_style = border_style.add_modifier(Modifier::BOLD);
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);
    frame.render_widget(block, rect);

    // Status stripe — solid block row just inside the top border.
    paint_stripe(frame, x + 1, y + 1, CARD_W.saturating_sub(2), color);

    // Two content rows. Row 1: glyph + step_id. Row 2: secondary
    // info derived from status (agent / tokens / `awaiting…`).
    let title_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    let inner_w = CARD_W.saturating_sub(4) as usize; // 2 borders + 1-col side padding each
    let title_text = format!("{glyph} {}", trim(&node.step_id, inner_w));
    let title = Paragraph::new(title_text).style(title_style);
    frame.render_widget(
        title,
        Rect {
            x: x + 2,
            y: y + 2,
            width: CARD_W.saturating_sub(4),
            height: 1,
        },
    );

    let detail = secondary_line(node);
    let detail_style = Style::default().fg(Color::Gray);
    frame.render_widget(
        Paragraph::new(detail).style(detail_style),
        Rect {
            x: x + 2,
            y: y + 3,
            width: CARD_W.saturating_sub(4),
            height: 1,
        },
    );
}

/// Paint the colored status stripe inside a card's top row.
fn paint_stripe(frame: &mut Frame, x: u16, y: u16, width: u16, color: Color) {
    let style = Style::default().fg(color);
    let buf = frame.buffer_mut();
    let area = buf.area;
    for dx in 0..width {
        let xx = x + dx;
        if xx >= area.x + area.width || y >= area.y + area.height {
            break;
        }
        buf[(xx, y)].set_symbol("█").set_style(style);
    }
}

fn secondary_line(node: &crate::state::NodeState) -> String {
    let total_tokens = node.tokens.input + node.tokens.output;
    let agent_short = trim(&node.agent, 12);
    match node.status {
        NodeStatus::Waiting => "waiting".into(),
        NodeStatus::Active => format!("starting · {agent_short}"),
        NodeStatus::Working => {
            if total_tokens > 0 {
                format!("{agent_short} · {total_tokens}t")
            } else {
                format!("running · {agent_short}")
            }
        }
        NodeStatus::Complete => {
            if total_tokens > 0 {
                format!("done · {total_tokens}t")
            } else {
                "done".into()
            }
        }
        NodeStatus::Failed => "failed".into(),
        NodeStatus::SoftFailed => "failed (cont)".into(),
        NodeStatus::Awaiting => "awaiting approval".into(),
        NodeStatus::Retrying => "retrying…".into(),
        NodeStatus::Skipped => "skipped".into(),
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

fn trim(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

/// Wall-clock-driven pulse for active/awaiting cards. Alternates
/// every ~1s so the App's render loop (30fps) toggles the bold
/// modifier without any state plumbing.
fn pulse_phase() -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.is_multiple_of(2)
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

        // Same-row hop: thick horizontal arrow into the next column.
        for child in &same_row_children {
            let Some(child_pos) = positions.get(**child) else {
                continue;
            };
            let Some((cx, _cy)) = card_origin(area, *child_pos) else {
                continue;
            };
            for x in exit_x..cx {
                if x >= area.x + area.width {
                    break;
                }
                let symbol = if x == cx.saturating_sub(1) { "▶" } else { "═" };
                buf[(x, exit_y)].set_symbol(symbol).set_style(style);
            }
        }

        // Fan-out: drop from the parent's bottom centre, then turn
        // into each child. Only if there's at least one off-row
        // child; otherwise no vertical drop needed.
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
                let from_x = drop_x.min(cx.saturating_sub(1));
                let to_x = drop_x.max(cx.saturating_sub(1));
                for x in (from_x + 1)..to_x {
                    if x >= area.x + area.width || mid_y >= area.y + area.height {
                        break;
                    }
                    buf[(x, mid_y)].set_symbol("═").set_style(style);
                }
                if cx > area.x && mid_y < area.y + area.height {
                    buf[(cx - 1, mid_y)].set_symbol("▶").set_style(style);
                }
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
                    let symbol = if is_last_child { "╚" } else { "╠" };
                    buf[(drop_x, mid_y)].set_symbol(symbol).set_style(style);
                }
            }
            for y in drop_top_y..max_child_y {
                if y >= area.y + area.height {
                    break;
                }
                if drop_x < area.x + area.width {
                    buf[(drop_x, y)].set_symbol("║").set_style(style);
                }
            }
        }
    }
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
        let para = Paragraph::new(text).style(Style::default().fg(Color::Yellow));
        frame.render_widget(para, area);
        return;
    }
    render_canvas(frame, area, model, edges, focused);
}
