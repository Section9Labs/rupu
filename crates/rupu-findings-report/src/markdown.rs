//! Blocks → Markdown. This is the reference layout the other emitters
//! follow: what it prints for a block is what the block means.

use crate::blocks::Block;

/// Collapse line breaks so a value can never open a new Markdown block
/// (heading, rule, list) from inside a title, field or table cell.
fn one_line(s: &str) -> String {
    s.split(['\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A fence longer than any backtick run in the text, and at least three.
fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

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
                    out.push_str(&format!("{}\n\n", p.trim_end()));
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
                    out.push_str(&format!("Step {}: {}  \n", i + 1, s.trim()));
                }
                out.push('\n');
            }
            Block::Note(n) => out.push_str(&format!("_{}_\n\n", one_line(n))),
            Block::Table { headers, rows } => {
                let esc = |c: &String| one_line(c).replace('|', "\\|");
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
