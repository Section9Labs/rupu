//! Blocks → Markdown. This is the reference layout the other emitters
//! follow: what it prints for a block is what the block means.

use crate::blocks::Block;
use crate::text::{longest_backtick_run, one_line};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
use std::ops::Range;

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

/// The byte ranges of every code block (fenced or indented, at any depth) in
/// `md`, in document order. Code blocks never nest, so they do not overlap.
fn code_block_ranges(md: &str) -> Vec<Range<usize>> {
    Parser::new_ext(md, Options::empty())
        .into_offset_iter()
        .filter_map(|(e, r)| matches!(e, Event::Start(Tag::CodeBlock(_))).then_some(r))
        .collect()
}

/// Whether `rest`, the text after a line's leading `<`, begins an autolink
/// (`<https://…>`, `<user@host>`). Such a line never opens an HTML block:
/// every HTML block start has a tag name (letters, digits, `-`) followed by
/// whitespace, `/`, `>` or the end of the line, and here it is followed by
/// `+ . _ % : @` instead. So it is left alone and stays a link.
fn starts_autolink(rest: &str) -> bool {
    let run = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-' | b'_' | b'%'))
        .count();
    rest.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric())
        && matches!(rest.as_bytes().get(run), Some(b':' | b'@'))
}

/// Agent Markdown with a backslash before the `<` that starts any line
/// (after at most three spaces) outside a code block. Every CommonMark HTML
/// block (types 1-7) starts that way, and one of types 1-5 (`<script`,
/// `<pre`, `<style`, `<textarea`, `<!--`, `<?`, `<!X`, `<![CDATA[`) left
/// unterminated runs to the end of the document, turning every later
/// section and finding into raw HTML that a sanitising viewer then drops.
/// Escaped, the line is text, as the HTML and PDF exports already show it.
/// Code blocks (found by parsing, so a fence inside a list item counts) are
/// copied byte for byte, and so is a line opening an autolink.
fn escape_html_block_starts(md: &str) -> String {
    let code = code_block_ranges(md);
    let mut next_code = 0;
    let mut out = String::with_capacity(md.len() + 16);
    let mut start = 0;
    for line in md.split_inclusive('\n') {
        let end = start + line.len();
        while code.get(next_code).is_some_and(|r| r.end <= start) {
            next_code += 1;
        }
        let in_code = code.get(next_code).is_some_and(|r| r.start < end);
        let indent = line.bytes().take_while(|b| *b == b' ').count();
        let rest = &line[indent..];
        if !in_code && indent <= 3 && rest.starts_with('<') && !starts_autolink(&rest[1..]) {
            out.push_str(&line[..indent]);
            out.push('\\');
            out.push_str(rest);
        } else {
            out.push_str(line);
        }
        start = end;
    }
    out
}

/// Agent Markdown with its headings moved below the document's own (`#` is
/// the title, `##` a section): an ATX heading goes two levels down, capped at
/// six (`#` → `###`, `##` → `####`, `####` → `######`). A setext heading
/// (`===` / `---` underline) at the top level becomes an ATX heading at the
/// shifted level, its lines joined; one inside a list item or quote keeps its
/// text but loses its heading-ness (the underline's first character is
/// escaped). The HTML and Typst emitters shift headings the same way.
/// Headings are found by parsing, so nothing inside a code block is touched.
fn shift_headings(md: &str) -> String {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut depth = 0usize;
    for (event, range) in Parser::new_ext(md, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let from = level as usize;
                let to = (from + 2).min(6);
                let src = &md[range.clone()];
                let hashes = src.bytes().take_while(|b| *b == b'#').count();
                let atx = hashes == from
                    && matches!(
                        src.as_bytes().get(hashes),
                        None | Some(b' ' | b'\t' | b'\n' | b'\r')
                    );
                let body = src.trim_end_matches(['\n', '\r']);
                let underline_line = body.rfind('\n').map_or(0, |i| i + 1);
                if atx {
                    edits.push((range.start..range.start, "#".repeat(to - from)));
                } else if depth == 0 {
                    let text: Vec<&str> = body[..underline_line]
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .collect();
                    let tail = &src[body.len()..];
                    let atx_line = format!("{} {}{tail}", "#".repeat(to), text.join(" "));
                    edits.push((range, atx_line));
                } else if let Some(i) = body[underline_line..].find(['=', '-']) {
                    let at = range.start + underline_line + i;
                    edits.push((at..at, "\\".to_string()));
                }
                depth += 1;
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let mut out = String::with_capacity(md.len() + 2 * edits.len());
    let mut done = 0;
    for (range, text) in edits {
        out.push_str(&md[done..range.start]);
        out.push_str(&text);
        done = range.end;
    }
    out.push_str(&md[done..]);
    out
}

/// Agent Markdown made safe to splice into the report at a line start: HTML
/// block starts escaped, headings shifted below the document's own, and an
/// unclosed top-level fence closed (last, since escaping can expose a fence
/// that an HTML block used to hide).
fn agent_markdown(md: &str) -> String {
    close_open_fence(&shift_headings(&escape_html_block_starts(md)))
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
                    out.push_str(&format!("{}\n\n", agent_markdown(p.trim_end())));
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
                    // A blank line after each step: a step ending in a list
                    // (or a quote) would otherwise absorb the next step as a
                    // lazy continuation line.
                    let step = agent_markdown(&format!("Step {}: {}", i + 1, s.trim()));
                    out.push_str(&format!("{step}\n\n"));
                }
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
