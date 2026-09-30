//! Blocks → Markdown. This is the reference layout the other emitters
//! follow: what it prints for a block is what the block means.

use crate::blocks::Block;
use crate::text::{longest_backtick_run, one_line};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

/// A fence longer than any backtick run in the text, and at least three.
fn fence_for(text: &str) -> String {
    "`".repeat(longest_backtick_run(text).max(2) + 1)
}

/// A table cell: one line, with `|` escaped. A backslash run directly before
/// a `|` is doubled first, so `\|` in the text cannot turn the escape into
/// an escaped backslash followed by a live delimiter.
fn escape_cell(s: &str) -> String {
    let s = one_line(s);
    let mut out = String::with_capacity(s.len());
    let mut backslashes = 0;
    for c in s.chars() {
        match c {
            '\\' => {
                out.push('\\');
                backslashes += 1;
                continue;
            }
            '|' => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                out.push_str("\\|");
            }
            other => out.push(other),
        }
        backslashes = 0;
    }
    out
}

/// Agent Markdown with any top-level fenced code block it opens but never
/// closes, closed. An unclosed fence runs to the end of the document, so it
/// would swallow every later heading, step and finding into a code block.
/// (A fence inside a list item or quote ends with its container, so only the
/// top level needs the help.)
fn close_open_fence(md: &str) -> String {
    let mut depth = 0usize;
    let mut unclosed: Option<(char, usize)> = None;
    for (event, range) in Parser::new_ext(md, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 && matches!(tag, Tag::CodeBlock(CodeBlockKind::Fenced(_))) {
                    let raw = md[range].trim_end_matches(['\n', '\r']);
                    let mut lines = raw.lines();
                    let opener = lines.next().unwrap_or("").trim_start();
                    let ch = opener.chars().next().unwrap_or('`');
                    let width = opener.chars().take_while(|c| *c == ch).count();
                    let closed = lines.next_back().is_some_and(|last| {
                        let last = last.trim();
                        last.len() >= width && last.chars().all(|c| c == ch)
                    });
                    if !closed {
                        unclosed = Some((ch, width));
                    }
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    match unclosed {
        Some((ch, width)) => format!("{}\n{}", md.trim_end(), ch.to_string().repeat(width)),
        None => md.to_string(),
    }
}

/// Render blocks as CommonMark (with GFM tables), the reference layout the
/// other emitters follow.
///
/// `Fields` labels and values are emitted as Markdown inline text (line
/// breaks collapsed, nothing escaped), so a value like `*x*` renders as
/// emphasis here. The HTML and Typst emitters must treat `Fields` values as
/// plain text instead.
pub fn render(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        match b {
            Block::Filename(f) => out.push_str(&format!("Filename: {}\n\n", one_line(f))),
            Block::Title(t) => out.push_str(&format!("# {}\n\n", one_line(t))),
            Block::Fields(rows) => {
                if rows.is_empty() {
                    continue;
                }
                for (k, v) in rows {
                    out.push_str(&format!("**{}:** {}  \n", one_line(k), one_line(v)));
                }
                out.push('\n');
            }
            Block::Heading(h) => out.push_str(&format!("## {}\n\n", one_line(h))),
            Block::Prose(p) => {
                if !p.trim().is_empty() {
                    out.push_str(&format!("{}\n\n", close_open_fence(p.trim_end())));
                }
            }
            Block::Code { lang, text } => {
                let fence = fence_for(text);
                out.push_str(&format!(
                    "{fence}{}\n{}\n{fence}\n\n",
                    lang.as_deref().unwrap_or(""),
                    text.trim_end_matches('\n')
                ));
            }
            Block::Steps(steps) => {
                if steps.is_empty() {
                    continue;
                }
                for (i, s) in steps.iter().enumerate() {
                    // Two trailing spaces: a hard break, so consecutive steps
                    // stay on their own lines instead of merging into one
                    // paragraph.
                    let step = close_open_fence(&format!("Step {}: {}", i + 1, s.trim()));
                    out.push_str(&format!("{step}  \n"));
                }
                out.push('\n');
            }
            Block::Note(n) => out.push_str(&format!("_{}_\n\n", one_line(n))),
            Block::Table { headers, rows } => {
                let esc = |c: &String| escape_cell(c);
                out.push_str(&format!(
                    "| {} |\n",
                    headers.iter().map(esc).collect::<Vec<_>>().join(" | ")
                ));
                out.push_str(&format!("|{}\n", " --- |".repeat(headers.len())));
                for r in rows {
                    out.push_str(&format!(
                        "| {} |\n",
                        r.iter().map(esc).collect::<Vec<_>>().join(" | ")
                    ));
                }
                out.push('\n');
            }
            Block::PageBreak => out.push_str("---\n\n"),
        }
    }
    out.trim_end().to_string() + "\n"
}
