//! Blocks → a complete Typst document (preamble + body).
//!
//! Every dynamic string is emitted as a Typst string literal (`typst_str`) or
//! a `raw(…)` argument, and `Filename`, `Title`, `Fields`, `Heading`, `Note`
//! and table cells are plain text, so nothing agent-written can execute as
//! Typst code. Only `Prose` and `Steps` go through the Markdown converter,
//! which itself emits nothing but string literals and fixed markup.
//!
//! A `Block::Image` is the one file the document reads: its bytes travel
//! beside the markup in [`TypstDoc::files`] under a fixed virtual path built
//! from its (validated) sha256 and its sniffed type, and the PDF world
//! serves only those.

use crate::blocks::Block;
use crate::prose::{md_to_typst, typst_str};
use crate::text::safe_lang;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A complete Typst document: its markup, and the image files it references
/// by virtual path (`/evidence/<sha256>.<ext>`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypstDoc {
    pub markup: String,
    pub files: BTreeMap<String, Arc<[u8]>>,
}

impl From<String> for TypstDoc {
    /// Markup that reads no files.
    fn from(markup: String) -> Self {
        TypstDoc {
            markup,
            files: BTreeMap::new(),
        }
    }
}

/// The virtual path an image is served under. `mime` is one of the four
/// raster types `Block::Image` carries; Typst picks the decoder by extension.
fn image_path(sha256: &str, mime: &str) -> String {
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        _ => "webp",
    };
    // `sha256` was checked to be 64 lowercase hex before the bytes were read,
    // so the path is a plain file name; filtering again costs nothing.
    let name: String = sha256.chars().filter(char::is_ascii_hexdigit).collect();
    format!("/evidence/{name}.{ext}")
}

const PREAMBLE: &str = "#set page(paper: \"a4\", margin: (x: 2cm, y: 2cm), numbering: \"1\")\n\
#set text(font: \"Libertinus Serif\", size: 10pt)\n\
#set par(justify: false)\n\
#show raw: set text(font: \"DejaVu Sans Mono\", size: 8pt)\n\
#show raw.where(block: true): block.with(fill: luma(245), inset: 6pt, radius: 3pt, width: 100%)\n\
#show heading.where(level: 2): set text(size: 11pt)\n\n";

/// A content block holding `s` as plain text.
fn content(s: &str) -> String {
    format!("[#{}]", typst_str(s))
}

/// Render `blocks` as complete Typst markup (see [`render_doc`] for the
/// files an image block needs).
pub fn render(blocks: &[Block]) -> String {
    render_doc(blocks).markup
}

/// Render `blocks` as a complete Typst document with its image files.
pub fn render_doc(blocks: &[Block]) -> TypstDoc {
    let mut files = BTreeMap::new();
    let mut out = String::from(PREAMBLE);
    for b in blocks {
        match b {
            Block::Filename(f) => out.push_str(&format!(
                "#text(size: 8pt, fill: gray)[#{}]\n\n",
                typst_str(&format!("Filename: {f}"))
            )),
            Block::Title(t) => out.push_str(&format!("#heading(level: 1)[#{}]\n\n", typst_str(t))),
            Block::Fields(rows) => {
                if rows.is_empty() {
                    continue;
                }
                out.push_str("#table(columns: (auto, 1fr), stroke: none, inset: (x: 0pt, y: 2pt), column-gutter: 1em,\n");
                for (k, v) in rows {
                    let v = crate::blocks::field_text(k, v);
                    out.push_str(&format!(
                        "  [#strong[#{}]], {},\n",
                        typst_str(k),
                        content(&v)
                    ));
                }
                out.push_str(")\n\n");
            }
            Block::Heading(h) => {
                out.push_str(&format!("#heading(level: 2)[#{}]\n\n", typst_str(h)))
            }
            Block::Prose(p) => {
                out.push_str(&md_to_typst(p));
                out.push('\n');
            }
            Block::Code { lang, text } => match safe_lang(lang.as_deref()) {
                Some(l) => out.push_str(&format!(
                    "#raw(block: true, lang: {}, {})\n\n",
                    typst_str(&l),
                    typst_str(text)
                )),
                None => out.push_str(&format!("#raw(block: true, {})\n\n", typst_str(text))),
            },
            Block::Steps(steps) => {
                if steps.is_empty() {
                    continue;
                }
                // Typst reads `1 a A i I *` in a numbering pattern as counting
                // symbols, so a relabel containing those letters (e.g.
                // "Finding 1:") would break the numbering, not just the label.
                out.push_str("#enum(numbering: \"Step 1:\",\n");
                for s in steps {
                    out.push_str(&format!("  [{}],\n", md_to_typst(s).trim_end()));
                }
                out.push_str(")\n\n");
            }
            Block::Note(n) => out.push_str(&format!("#emph[#{}]\n\n", typst_str(n))),
            Block::Table { headers, rows } => {
                // A ragged row would shift every later cell out of its column,
                // so size the grid to the widest row and pad the short ones.
                let cols = rows
                    .iter()
                    .map(Vec::len)
                    .chain([headers.len()])
                    .max()
                    .unwrap_or(0);
                if cols == 0 {
                    continue;
                }
                out.push_str(&format!(
                    "#table(columns: {cols}, stroke: 0.5pt + luma(200), inset: 4pt,\n"
                ));
                if !headers.is_empty() {
                    let mut cells: Vec<String> = headers
                        .iter()
                        .map(|h| format!("[#strong[#{}]]", typst_str(h)))
                        .collect();
                    cells.resize(cols, "[]".to_string());
                    out.push_str(&format!("  table.header({}),\n", cells.join(", ")));
                }
                for r in rows {
                    let mut cells: Vec<String> = r.iter().map(|c| content(c)).collect();
                    cells.resize(cols, "[]".to_string());
                    out.push_str(&format!("  {},\n", cells.join(", ")));
                }
                out.push_str(")\n\n");
            }
            Block::PageBreak => out.push_str("#pagebreak()\n\n"),
            Block::Image {
                caption,
                sha256,
                mime,
                bytes,
                ..
            } => {
                let path = image_path(sha256, mime);
                out.push_str(&format!(
                    "#figure(image({}, alt: {}), caption: {})\n\n",
                    typst_str(&path),
                    typst_str(caption),
                    content(caption)
                ));
                files.insert(path, Arc::clone(bytes));
            }
        }
    }
    TypstDoc { markup: out, files }
}
