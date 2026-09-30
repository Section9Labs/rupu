//! Agent-written Markdown → safe HTML / safe Typst. One parser
//! (pulldown-cmark), two emitters.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

fn parser(md: &str) -> Parser<'_> {
    Parser::new_ext(md, Options::ENABLE_STRIKETHROUGH)
}

fn unsafe_url(url: &str) -> bool {
    // Browsers ignore embedded tabs/newlines in a URL's scheme, so strip all
    // whitespace and control characters before matching.
    let u: String = url
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    u.starts_with("javascript:") || u.starts_with("data:") || u.starts_with("vbscript:")
}

/// Markdown → HTML with raw HTML shown as text and script-ish links removed.
pub fn md_to_html(md: &str) -> String {
    let events = parser(md).map(|e| match e {
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) if unsafe_url(&dest_url) => Event::Start(Tag::Link {
            link_type,
            dest_url: "#".into(),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) if unsafe_url(&dest_url) => Event::Start(Tag::Image {
            link_type,
            dest_url: "#".into(),
            title,
            id,
        }),
        other => other,
    });
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events);
    out
}

/// A Typst string literal (with quotes) for arbitrary text.
pub fn typst_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Markdown → Typst markup. All text becomes string literals, so nothing in
/// agent-written prose can execute as Typst code.
pub fn md_to_typst(md: &str) -> String {
    let mut out = String::new();
    let mut code: Option<(String, String)> = None; // (lang, text) while inside a fenced block
    let mut lists: Vec<bool> = Vec::new(); // true = ordered
    for e in parser(md) {
        if let Some((_, buf)) = code.as_mut() {
            match e {
                Event::Text(t) => {
                    buf.push_str(&t);
                    continue;
                }
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, text) = code.take().expect("inside code block");
                    if lang.is_empty() {
                        out.push_str(&format!("#raw(block: true, {})\n", typst_str(&text)));
                    } else {
                        out.push_str(&format!(
                            "#raw(block: true, lang: {}, {})\n",
                            typst_str(&lang),
                            typst_str(&text)
                        ));
                    }
                    continue;
                }
                _ => continue,
            }
        }
        match e {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                out.push_str(if lists.is_empty() { "\n\n" } else { "" })
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let lvl = match level {
                    HeadingLevel::H1 => 3,
                    HeadingLevel::H2 => 4,
                    _ => 5,
                };
                out.push_str(&format!("#heading(level: {lvl}, outlined: false)["));
            }
            Event::End(TagEnd::Heading(_)) => out.push_str("]\n"),
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        l.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::Start(Tag::List(start)) => lists.push(start.is_some()),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                out.push_str("\n\n");
            }
            Event::Start(Tag::Item) => out.push_str(if *lists.last().unwrap_or(&false) {
                "\n+ "
            } else {
                "\n- "
            }),
            Event::End(TagEnd::Item) => {}
            Event::Start(Tag::Emphasis) => out.push_str("#emph["),
            Event::Start(Tag::Strong) => out.push_str("#strong["),
            Event::Start(Tag::Strikethrough) => out.push_str("#strike["),
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => out.push(']'),
            Event::Start(Tag::BlockQuote(_)) => out.push_str("#quote(block: true)["),
            Event::End(TagEnd::BlockQuote(_)) => out.push_str("]\n"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                if unsafe_url(&dest_url) {
                    out.push('[');
                } else {
                    out.push_str(&format!("#link({})[", typst_str(&dest_url)));
                }
            }
            Event::End(TagEnd::Link) => out.push(']'),
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => {
                out.push('#');
                out.push_str(&typst_str(&t));
            }
            Event::Code(t) => out.push_str(&format!("#raw({})", typst_str(&t))),
            Event::SoftBreak => out.push_str("#\" \""),
            Event::HardBreak => out.push_str("#linebreak()"),
            Event::Rule => out.push_str("#line(length: 100%)\n"),
            _ => {}
        }
    }
    out.trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escapes_raw_html_and_drops_js_links() {
        // Leading prose keeps the `<script>` inline; a line that *starts* with
        // it is a CommonMark HTML block, covered by the next test.
        let h = md_to_html("Hi <script>alert(1)</script> and [x](javascript:alert(1)) and `a<b`");
        assert!(!h.contains("<script>"), "{h}");
        assert!(h.contains("&lt;script&gt;"), "{h}");
        assert!(!h.contains("javascript:"), "{h}");
        assert!(h.contains("<code>a&lt;b</code>"), "{h}");
    }

    #[test]
    fn html_drops_unsafe_urls_in_every_link_form() {
        for md in [
            "[x](javascript:alert(1))",
            "[x](JaVaScRiPt:alert(1))",
            "[x](data:text/html;base64,AAAA)",
            "[x](vbscript:msgbox(1))",
            "[x](java&#9;script:alert(1))",
            "![x](javascript:alert(1))",
            "<javascript:alert(1)>",
            "[x][r]\n\n[r]: javascript:alert(1)",
        ] {
            // The visible text of an autolink may still read `javascript:…`;
            // what must never survive is a live href/src carrying the scheme.
            let h = md_to_html(md).to_ascii_lowercase();
            for attr in ["href=\"", "src=\""] {
                for part in h.split(attr).skip(1) {
                    let url = part.split('"').next().unwrap_or("");
                    for bad in ["javascript:", "vbscript:", "data:"] {
                        assert!(!url.contains(bad), "{md} -> {h}");
                    }
                }
            }
        }
        let ok = md_to_html("[x](https://example.com/a?b=1)");
        assert!(ok.contains(r#"href="https://example.com/a?b=1""#), "{ok}");
    }

    #[test]
    fn typst_drops_unsafe_link_targets() {
        let t = md_to_typst("[x](javascript:alert(1)) [y](https://e.com)");
        assert!(!t.contains("javascript"), "{t}");
        assert!(t.contains(r#"#link("https://e.com")["#), "{t}");
    }

    #[test]
    fn html_neutralises_block_level_raw_html() {
        // A line starting with `<script>` is an HTML *block*: the whole line
        // is opaque to the Markdown parser and must come out as escaped text.
        let h = md_to_html("<script>alert(1)</script> and [x](javascript:alert(1))\n\n<div onclick=\"x()\">hi</div>");
        assert!(!h.contains("<script>"), "{h}");
        assert!(!h.contains("<div"), "{h}");
        assert!(!h.contains("href"), "{h}");
        assert!(h.contains("&lt;script&gt;"), "{h}");
        assert!(h.contains("&lt;div"), "{h}");
    }

    #[test]
    fn typst_string_escapes() {
        assert_eq!(typst_str(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(typst_str("x\ny"), r#""x\ny""#);
    }

    #[test]
    fn typst_never_emits_raw_markup_from_text() {
        let t = md_to_typst(
            "Use #set and *stars* and $math$ and <label> and @ref\n\n```sh\nrm -rf /\n```",
        );
        // every prose run is a string literal; the fenced block is raw()
        assert!(t.contains(r##"#"Use #set and ""##), "{t}");
        assert!(t.contains("#emph["), "{t}");
        assert!(
            t.contains(r#"#raw(block: true, lang: "sh", "rm -rf /\n")"#),
            "{t}"
        );
        assert!(!t.contains("\n#set"), "{t}");
    }

    #[test]
    fn typst_lists_and_code_spans() {
        let t = md_to_typst("- one `x`\n- two");
        assert!(t.contains("\n- #\"one \"#raw(\"x\")"), "{t}");
        assert!(t.contains("\n- #\"two\""), "{t}");
    }
}
