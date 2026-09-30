//! Blocks → one self-contained HTML document (inline CSS, no external loads).
//!
//! `Filename`, `Title`, `Fields`, `Heading`, `Note` and table cells are plain
//! text and are escaped, never parsed as Markdown. Only `Prose` and `Steps`
//! go through the Markdown → HTML converter, which neutralises raw HTML and
//! script-ish URLs and turns images into alt text, so the markup itself loads
//! nothing. A Content-Security-Policy `<meta>` (first in `<head>`, ahead of
//! the title and styles) is defence in depth against an escaping bug: it
//! forbids scripts, network loads, `<base>` and form posts outright.

use crate::blocks::Block;
use crate::prose::md_to_html;
use crate::text::safe_lang;

/// HTML-escape text for element content or a quoted attribute value.
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

const CSS: &str = "body{font:14px/1.55 -apple-system,system-ui,sans-serif;color:#1b1924;max-width:860px;margin:2rem auto;padding:0 1rem}\
h1{font-size:22px;margin:.2em 0 .6em}h2{font-size:15px;text-transform:uppercase;letter-spacing:.06em;color:#4a4659;margin:1.6em 0 .5em}\
.fn{font:12px ui-monospace,Menlo,monospace;color:#807b92}dl{display:grid;grid-template-columns:12rem 1fr;gap:.2rem 1rem;margin:.5rem 0}\
dt{color:#807b92}dd{margin:0}dd,td{overflow-wrap:anywhere}pre{background:#f4f3f8;padding:.6rem .8rem;border-radius:4px;overflow-x:auto;font:12px/1.5 ui-monospace,Menlo,monospace}\
code{font-family:ui-monospace,Menlo,monospace}ol.steps li{margin:.25rem 0}.note{color:#807b92;font-style:italic}\
table{border-collapse:collapse;width:100%;font-size:13px}th,td{border-bottom:1px solid #dedbe7;text-align:left;padding:.3rem .5rem}\
.pb{page-break-before:always;border-top:1px dashed #dedbe7;margin:2rem 0}@media print{.pb{border:0;margin:0}}";

/// Render `blocks` as a complete HTML document titled `title`.
pub fn render(title: &str, blocks: &[Block]) -> String {
    let mut body = String::new();
    for b in blocks {
        match b {
            Block::Filename(f) => {
                body.push_str(&format!("<div class=\"fn\">Filename: {}</div>\n", esc(f)))
            }
            Block::Title(t) => body.push_str(&format!("<h1>{}</h1>\n", esc(t))),
            Block::Fields(rows) => {
                if rows.is_empty() {
                    continue;
                }
                body.push_str("<dl>");
                for (k, v) in rows {
                    body.push_str(&format!("<dt>{}</dt><dd>{}</dd>", esc(k), esc(v)));
                }
                body.push_str("</dl>\n");
            }
            Block::Heading(h) => body.push_str(&format!("<h2>{}</h2>\n", esc(h))),
            Block::Prose(p) => body.push_str(&md_to_html(p)),
            Block::Code { lang, text } => body.push_str(&format!(
                "<pre><code{}>{}</code></pre>\n",
                safe_lang(lang.as_deref())
                    .map(|l| format!(" class=\"language-{}\"", esc(&l)))
                    .unwrap_or_default(),
                esc(text)
            )),
            Block::Steps(steps) => {
                if steps.is_empty() {
                    continue;
                }
                body.push_str("<ol class=\"steps\">");
                for s in steps {
                    body.push_str(&format!("<li>{}</li>", md_to_html(s)));
                }
                body.push_str("</ol>\n");
            }
            Block::Note(n) => body.push_str(&format!("<p class=\"note\">{}</p>\n", esc(n))),
            Block::Table { headers, rows } => {
                body.push_str("<table>");
                if !headers.is_empty() {
                    body.push_str("<thead><tr>");
                    for h in headers {
                        body.push_str(&format!("<th>{}</th>", esc(h)));
                    }
                    body.push_str("</tr></thead>");
                }
                body.push_str("<tbody>");
                for r in rows {
                    body.push_str("<tr>");
                    for c in r {
                        body.push_str(&format!("<td>{}</td>", esc(c)));
                    }
                    body.push_str("</tr>");
                }
                body.push_str("</tbody></table>\n");
            }
            Block::PageBreak => body.push_str("<div class=\"pb\"></div>\n"),
        }
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'\">\
<meta name=\"referrer\" content=\"no-referrer\">\
<title>{}</title><style>{CSS}</style></head><body>\n{body}</body></html>\n",
        esc(title)
    )
}
