mod common;

use common::*;
use rupu_coverage::Severity;
use rupu_findings_report::blocks::{finding_blocks, project_blocks, Block};
use rupu_findings_report::html;
use rupu_findings_report::model::ReportMeta;
use rupu_findings_report::number::number_map;
use rupu_findings_report::typst_doc;

const XSS: &str = "<img src=x onerror=alert(1)>";
const TYPST_INJECT: &str = "#set page(width: 1cm)";

fn full_blocks() -> Vec<Block> {
    let f = full_finding();
    finding_blocks(&f, &number_map(std::slice::from_ref(&f)))
}

fn two_finding_blocks() -> Vec<Block> {
    let all = numbered(vec![
        input(
            "notebin",
            Some("audit"),
            full_record("fnd_full", Severity::Critical, full_report()),
        ),
        input("notebin", None, summary_record("fnd_sum", Severity::High)),
    ]);
    let meta = ReportMeta {
        title: "Notebin findings".into(),
        generated_at: ts("2026-09-29T12:00:00Z"),
        scope: "Project notebin".into(),
    };
    project_blocks(&meta, &all)
}

// ---------------------------------------------------------------- HTML

#[test]
fn html_escapes_field_values_and_every_plain_text_block() {
    let h = html::render(
        XSS,
        &[
            Block::Filename(XSS.into()),
            Block::Title(XSS.into()),
            Block::Fields(vec![(XSS.into(), XSS.into())]),
            Block::Heading(XSS.into()),
            Block::Note(XSS.into()),
            Block::Table {
                headers: vec![XSS.into()],
                rows: vec![vec![XSS.into()]],
            },
            Block::Code {
                lang: None,
                text: XSS.into(),
            },
        ],
    );
    assert!(h.contains("&lt;img src=x onerror=alert(1)&gt;"), "{h}");
    assert!(!h.contains("<img"), "{h}");
    // 1 <title>, then filename, h1, dt, dd, h2, note, th, td, code.
    assert_eq!(h.matches("&lt;img").count(), 10, "{h}");
}

#[test]
fn html_plain_text_blocks_are_not_parsed_as_markdown() {
    let h = html::render(
        "t",
        &[
            Block::Fields(vec![("*k*".into(), "**bold** [x](https://e.com)".into())]),
            Block::Note("_note_".into()),
        ],
    );
    assert!(h.contains("<dd>**bold** [x](https://e.com)</dd>"), "{h}");
    assert!(h.contains("<dt>*k*</dt>"), "{h}");
    assert!(!h.contains("<strong>") && !h.contains("<em>"), "{h}");
    assert!(!h.contains("href"), "{h}");
}

#[test]
fn html_is_self_contained_with_a_locked_down_csp() {
    let mut blocks = two_finding_blocks();
    blocks.push(Block::Prose(
        "[x](javascript:alert(1)) <script>alert(1)</script>\n\n<script>alert(2)</script>".into(),
    ));
    blocks.push(Block::Steps(vec!["<script>alert(3)</script> step".into()]));
    let h = html::render("Notebin findings", &blocks);
    let lower = h.to_ascii_lowercase();
    assert!(lower.starts_with("<!doctype html>"), "{h}");
    assert!(lower.contains("<meta charset=\"utf-8\">"), "{h}");
    assert!(
        h.contains(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'\">"
        ),
        "{h}"
    );
    // No script, no external stylesheet/script/frame, no scheme that runs code.
    for bad in [
        "<script",
        "<link",
        "<iframe",
        "<object",
        "<embed",
        "@import",
        "url(",
        "javascript:",
        " onerror=",
        "src=\"http",
        "src='http",
        "href=\"http://fonts",
    ] {
        assert!(!lower.contains(bad), "found {bad:?} in {h}");
    }
    // The only `http(s)://` in the head is none at all: styles are inline.
    let head = &h[..h.find("</head>").unwrap()];
    assert!(
        !head.contains("http://") && !head.contains("https://"),
        "{head}"
    );
    assert!(h.contains("<style>") && h.contains("</style>"), "{h}");
}

#[test]
fn html_csp_and_referrer_metas_come_before_title_style_and_body() {
    let h = html::render("t", &[Block::Title("x".into())]);
    let csp = h.find("http-equiv=\"Content-Security-Policy\"").unwrap();
    let referrer = h
        .find("<meta name=\"referrer\" content=\"no-referrer\">")
        .unwrap();
    for later in ["<title>", "<style>", "<body>"] {
        let at = h.find(later).unwrap();
        assert!(csp < at, "CSP meta must precede {later}: {h}");
        assert!(referrer < at, "referrer meta must precede {later}: {h}");
    }
    assert!(
        h.contains("base-uri 'none'") && h.contains("form-action 'none'"),
        "{h}"
    );
    // Exactly one of each; nothing else in the head loads or redirects.
    assert_eq!(h.matches("Content-Security-Policy").count(), 1, "{h}");
    assert!(
        !h.to_ascii_lowercase().contains("http-equiv=\"refresh\""),
        "{h}"
    );
}

#[test]
fn html_export_of_a_report_with_an_image_in_its_prose_has_no_img() {
    let mut report = full_report();
    report.root_cause =
        "Shown: ![alt text](https://x.example/y.png?d=1) and ![z](data:image/png;base64,AAAA)"
            .into();
    let f = numbered(vec![input(
        "notebin",
        None,
        full_record("fnd_img", Severity::High, report),
    )])
    .remove(0);
    let blocks = finding_blocks(&f, &number_map(std::slice::from_ref(&f)));
    let h = html::render("t", &blocks);
    assert!(h.contains("alt text"), "{h}");
    assert!(!h.to_ascii_lowercase().contains("<img"), "{h}");
    assert!(!h.contains("x.example"), "{h}");
    assert!(!h.contains("data:image"), "{h}");
}

#[test]
fn html_prose_links_survive_but_scripts_do_not() {
    let h = html::render(
        "t",
        &[Block::Prose("see [docs](https://example.com/a) now".into())],
    );
    assert!(
        h.contains("<a href=\"https://example.com/a\">docs</a>"),
        "{h}"
    );
}

#[test]
fn html_title_is_escaped() {
    let h = html::render("</title><script>alert(1)</script>", &[]);
    assert!(h.contains("<title>&lt;/title&gt;&lt;script&gt;"), "{h}");
    assert!(!h.to_ascii_lowercase().contains("<script"), "{h}");
}

#[test]
fn html_renders_the_patch_as_a_diff_code_block() {
    let h = html::render("t", &full_blocks());
    assert!(h.contains("<pre><code class=\"language-diff\">"), "{h}");
    assert!(h.contains("<h2>Replication Steps</h2>"), "{h}");
    assert!(h.contains("<ol class=\"steps\">"), "{h}");
    assert!(h.contains("<dl>"), "{h}");
}

#[test]
fn html_page_break_sits_between_findings() {
    let h = html::render("t", &two_finding_blocks());
    assert_eq!(h.matches("class=\"pb\"").count(), 2, "{h}");
    let first_pb = h.find("class=\"pb\"").unwrap();
    let second_pb = h.rfind("class=\"pb\"").unwrap();
    assert!(h[..first_pb].contains("<h2>Index</h2>"), "{h}");
    assert!(h[first_pb..second_pb].contains("SEC-001"), "{h}");
    assert!(h[second_pb..].contains("SEC-002"), "{h}");
    assert!(h.contains(".pb{page-break-before:always"), "{h}");
    assert!(h.contains("<div class=\"pb\"></div>"), "{h}");
}

#[test]
fn html_recleans_the_code_language_itself() {
    let h = html::render(
        "t",
        &[
            Block::Code {
                lang: Some("diff\" onmouseover=\"alert(1)".into()),
                text: "x".into(),
            },
            Block::Code {
                lang: Some("<>\"' ".into()),
                text: "y".into(),
            },
            Block::Code {
                lang: Some("c++".into()),
                text: "z".into(),
            },
            Block::Code {
                lang: Some("c#".into()),
                text: "w".into(),
            },
        ],
    );
    assert!(!h.contains("onmouseover=\""), "{h}");
    assert!(
        h.contains("<pre><code class=\"language-diffonmouseoveralert1\">x</code></pre>"),
        "{h}"
    );
    // Nothing left after cleaning: no class attribute at all.
    assert!(h.contains("<pre><code>y</code></pre>"), "{h}");
    assert!(
        h.contains("<pre><code class=\"language-c++\">z</code></pre>"),
        "{h}"
    );
    assert!(
        h.contains("<pre><code class=\"language-c#\">w</code></pre>"),
        "{h}"
    );
}

#[test]
fn html_handles_empty_and_ragged_structures() {
    let h = html::render(
        "t",
        &[
            Block::Fields(vec![]),
            Block::Steps(vec![]),
            Block::Table {
                headers: vec![],
                rows: vec![vec!["a".into()]],
            },
        ],
    );
    assert!(!h.contains("<dl>") && !h.contains("<ol"), "{h}");
    assert!(!h.contains("<thead>"), "{h}");
    assert!(h.contains("<td>a</td>"), "{h}");
}

// --------------------------------------------------------------- Typst

/// The document with every Typst string literal removed: what is left is the
/// markup and code the emitter itself wrote. Hostile text must not be in it.
fn outside_strings(t: &str) -> String {
    let mut out = String::new();
    let mut chars = t.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            loop {
                match chars.next() {
                    Some('\\') => {
                        chars.next();
                    }
                    Some('"') | None => break,
                    Some(_) => {}
                }
            }
            out.push('"');
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn typst_field_values_are_string_literals_never_markup() {
    let t = typst_doc::render(&[Block::Fields(vec![(
        TYPST_INJECT.into(),
        TYPST_INJECT.into(),
    )])]);
    assert!(t.contains(&format!("\"{TYPST_INJECT}\"")), "{t}");
    assert!(
        t.contains(&format!(
            "[#strong[#\"{TYPST_INJECT}\"]], [#\"{TYPST_INJECT}\"],"
        )),
        "{t}"
    );
    // No line of the output *is* the injected directive.
    assert!(!t.lines().any(|l| l.trim() == TYPST_INJECT), "{t}");
    assert!(!outside_strings(&t).contains("width: 1cm"), "{t}");
}

#[test]
fn typst_every_plain_text_block_goes_through_a_string_literal() {
    let payloads = [
        "#set page(width: 1cm)",
        "]#import \"@preview/evil:1.0.0\": *[",
        "#read(\"/etc/passwd\")",
        "\\\"); #panic(\"x\"); (\"",
        "$ x $ @ref <label> = not a heading",
        "line one\n#set text(fill: red)",
    ];
    for p in payloads {
        let s = p.to_string();
        let t = typst_doc::render(&[
            Block::Filename(s.clone()),
            Block::Title(s.clone()),
            Block::Fields(vec![(s.clone(), s.clone())]),
            Block::Heading(s.clone()),
            Block::Note(s.clone()),
            Block::Table {
                headers: vec![s.clone()],
                rows: vec![vec![s.clone()]],
            },
            Block::Code {
                lang: Some(s.clone()),
                text: s.clone(),
            },
            Block::Prose(s.clone()),
            Block::Steps(vec![s.clone()]),
        ]);
        let rest = outside_strings(&t);
        for needle in [
            "width: 1cm",
            "@preview",
            "/etc/passwd",
            "#panic",
            "not a heading",
            "fill: red",
        ] {
            assert!(
                !rest.contains(needle),
                "{needle:?} escaped a literal in:\n{t}"
            );
        }
    }
}

#[test]
fn typst_preamble_and_page_break() {
    let t = typst_doc::render(&[Block::Title("T".into()), Block::PageBreak]);
    assert!(t.contains("#set text(font: \"Libertinus Serif\""), "{t}");
    assert!(
        t.contains("#show raw: set text(font: \"DejaVu Sans Mono\""),
        "{t}"
    );
    assert!(t.contains("#set page(paper: \"a4\""), "{t}");
    assert!(t.contains("#heading(level: 1)[#\"T\"]"), "{t}");
    assert!(t.trim_end().ends_with("#pagebreak()"), "{t}");
}

#[test]
fn typst_page_break_sits_between_findings() {
    let t = typst_doc::render(&two_finding_blocks());
    assert_eq!(t.matches("#pagebreak()").count(), 2, "{t}");
    let first = t.find("#pagebreak()").unwrap();
    let second = t.rfind("#pagebreak()").unwrap();
    assert!(t[first..second].contains("SEC-001"), "{t}");
    assert!(t[second..].contains("SEC-002"), "{t}");
}

#[test]
fn typst_code_uses_raw_with_a_cleaned_lang() {
    let t = typst_doc::render(&[
        Block::Code {
            lang: Some("diff".into()),
            text: "--- a\n+++ b\n".into(),
        },
        Block::Code {
            lang: Some("x\", evil: \"".into()),
            text: "t".into(),
        },
        Block::Code {
            lang: Some("#$ \"\\".into()),
            text: "u".into(),
        },
        Block::Code {
            lang: None,
            text: "v".into(),
        },
        Block::Code {
            lang: Some("$ \"\\".into()),
            text: "w".into(),
        },
        Block::Code {
            lang: Some("c#".into()),
            text: "x".into(),
        },
    ]);
    assert!(
        t.contains("#raw(block: true, lang: \"diff\", \"--- a\\n+++ b\\n\")"),
        "{t}"
    );
    assert!(
        t.contains("#raw(block: true, lang: \"xevil\", \"t\")"),
        "{t}"
    );
    // `#` is a legal tag character (`c#`), so it survives; the rest is dropped.
    assert!(t.contains("#raw(block: true, lang: \"#\", \"u\")"), "{t}");
    assert!(t.contains("#raw(block: true, \"v\")"), "{t}");
    // Nothing left after cleaning: no `lang:` at all.
    assert!(t.contains("#raw(block: true, \"w\")"), "{t}");
    assert!(t.contains("#raw(block: true, lang: \"c#\", \"x\")"), "{t}");
}

#[test]
fn typst_tables_are_padded_to_a_rectangle() {
    let t = typst_doc::render(&[Block::Table {
        headers: vec!["A".into(), "B".into()],
        rows: vec![vec!["1".into()], vec!["1".into(), "2".into(), "3".into()]],
    }]);
    assert!(t.contains("#table(columns: 3,"), "{t}");
    assert!(
        t.contains("table.header([#strong[#\"A\"]], [#strong[#\"B\"]], []),"),
        "{t}"
    );
    assert!(t.contains("[#\"1\"], [], [],"), "{t}");
    assert!(t.contains("[#\"1\"], [#\"2\"], [#\"3\"],"), "{t}");
    // Nothing to draw: no table at all.
    let none = typst_doc::render(&[Block::Table {
        headers: vec![],
        rows: vec![],
    }]);
    assert!(!none.contains("#table("), "{none}");
}

#[test]
fn typst_steps_use_a_numbered_enum_and_skip_when_empty() {
    let t = typst_doc::render(&[Block::Steps(vec!["one".into(), "two".into()])]);
    assert!(t.contains("#enum(numbering: \"Step 1:\","), "{t}");
    assert!(t.contains("  [#\"one\"],\n  [#\"two\"],\n)"), "{t}");
    let none = typst_doc::render(&[Block::Steps(vec![]), Block::Fields(vec![])]);
    assert!(
        !none.contains("#enum(") && !none.contains("#table("),
        "{none}"
    );
}

#[test]
fn typst_full_finding_renders_every_block_kind() {
    let t = typst_doc::render(&full_blocks());
    for want in [
        "#text(size: 8pt, fill: gray)[#\"Filename: SEC-001 - ",
        "#heading(level: 1)[#\"Notes API returns another user's note by id\"]",
        "#heading(level: 2)[#\"Root Cause\"]",
        "lang: \"diff\"",
        "#enum(numbering: \"Step 1:\"",
    ] {
        assert!(t.contains(want), "missing {want:?} in:\n{t}");
    }
}

#[test]
fn both_emitters_accept_an_empty_document() {
    let none: Vec<Block> = vec![];
    assert!(html::render("t", &none).contains("<body>\n</body>"));
    assert!(typst_doc::render(&none).starts_with("#set page("));
}
