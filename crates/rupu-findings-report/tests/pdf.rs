mod common;

use common::*;
use rupu_coverage::{Severity, Surface};
use rupu_findings_report::blocks::{finding_blocks, Block};
use rupu_findings_report::model::{ExportFinding, ReportMeta};
use rupu_findings_report::number::{filename, number_map};
use rupu_findings_report::pdf::render_pdf;
use rupu_findings_report::typst_doc;
use rupu_findings_report::{render_finding, render_project, render_split_zip, ExportError, Format};
use std::collections::HashMap;
use std::io::Read;

fn meta() -> ReportMeta {
    ReportMeta {
        title: "Notebin findings".into(),
        generated_at: ts("2026-09-29T12:00:00Z"),
        scope: "Project notebin".into(),
    }
}

fn two() -> Vec<ExportFinding> {
    numbered(vec![
        input(
            "notebin",
            Some("audit"),
            full_record("fnd_full", Severity::Critical, full_report()),
        ),
        input("notebin", None, summary_record("fnd_sum", Severity::High)),
    ])
}

fn one_numbers(f: &ExportFinding) -> HashMap<String, String> {
    number_map(std::slice::from_ref(f))
}

fn is_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF")
}

fn unzip(bytes: &[u8]) -> Vec<(String, String)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a zip");
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).unwrap();
            let mut body = String::new();
            f.read_to_string(&mut body).unwrap();
            (f.name().to_string(), body)
        })
        .collect()
}

#[test]
fn format_names_and_content_types() {
    assert_eq!(Format::Pdf.content_type(), "application/pdf");
    assert_eq!(
        Format::Markdown.content_type(),
        "text/markdown; charset=utf-8"
    );
    assert_eq!(Format::Html.content_type(), "text/html; charset=utf-8");
    assert_eq!(Format::Markdown.ext(), "md");
    assert_eq!(Format::Html.ext(), "html");
    assert_eq!(Format::Pdf.ext(), "pdf");
    assert_eq!(Format::parse("pdf"), Some(Format::Pdf));
    assert_eq!(Format::parse("markdown"), Some(Format::Markdown));
    assert_eq!(Format::parse("md"), Some(Format::Markdown));
    assert_eq!(Format::parse("html"), Some(Format::Html));
    assert_eq!(Format::parse("docx"), None);
}

#[test]
fn a_full_finding_renders_a_pdf() {
    let f = full_finding();
    let pdf = render_finding(&f, &one_numbers(&f), Format::Pdf).unwrap();
    assert!(is_pdf(&pdf), "not a pdf: {:?}", &pdf[..pdf.len().min(16)]);
}

#[test]
fn a_summary_finding_renders_a_pdf() {
    let f = numbered(vec![input(
        "notebin",
        None,
        summary_record("fnd_sum", Severity::High),
    )])
    .remove(0);
    let pdf = render_finding(&f, &one_numbers(&f), Format::Pdf).unwrap();
    assert!(is_pdf(&pdf));
}

#[test]
fn a_project_pdf_is_bigger_than_one_finding() {
    let all = two();
    let numbers = number_map(&all);
    let single = render_finding(&all[0], &numbers, Format::Pdf).unwrap();
    let project = render_project(&meta(), &all, Format::Pdf).unwrap();
    assert!(is_pdf(&project));
    assert!(
        project.len() > single.len(),
        "project {} <= single {}",
        project.len(),
        single.len()
    );
}

#[test]
fn an_empty_project_still_renders_a_pdf() {
    let pdf = render_project(&meta(), &[], Format::Pdf).unwrap();
    assert!(is_pdf(&pdf));
}

#[test]
fn typst_looking_text_is_data_not_code() {
    let mut r = full_report();
    r.title = "#panic(\"title\")".into();
    r.description = "#panic(\"x\") and #(1/0) and $ x $ and #import \"@preview/a:1\"".into();
    r.remediation = "```\n#panic(\"in a fence\")\n```".into();
    r.references = "[#panic(\"link\")](https://example.com)".into();
    let mut rec = full_record("fnd_x", Severity::High, r);
    rec.summary = "#panic(\"summary\")".into();
    let f = numbered(vec![input("notebin", Some("#panic(\"wf\")"), rec)]).remove(0);
    let pdf = render_finding(&f, &one_numbers(&f), Format::Pdf)
        .expect("escaped Typst-looking text must compile");
    assert!(is_pdf(&pdf));
}

#[test]
fn markdown_split_zip_has_an_index_and_one_file_per_finding() {
    let all = two();
    let zip = render_split_zip(&meta(), &all, Format::Markdown).unwrap();
    let entries = unzip(&zip);
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    let want: Vec<String> = ["index.md".to_string()]
        .into_iter()
        .chain(all.iter().map(|f| filename(f, "md")))
        .collect();
    assert_eq!(names, want.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(names[1].starts_with("SEC-001 - "), "{names:?}");
    assert!(names[2].starts_with("SEC-002 - "), "{names:?}");

    let index = &entries[0].1;
    assert!(index.contains("Notebin findings"), "{index}");
    assert!(
        index.contains("SEC-001") && index.contains("SEC-002"),
        "{index}"
    );
    // The index is the title + table only, not every finding's body.
    assert!(!index.contains("Replication Steps"), "{index}");
    assert!(!index.contains("Provenance"), "{index}");
    assert!(
        entries[1].1.contains("Replication Steps"),
        "{}",
        entries[1].1
    );
}

#[test]
fn pdf_split_zip_holds_real_pdfs() {
    let all = two();
    let zip = render_split_zip(&meta(), &all, Format::Pdf).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    assert_eq!(z.len(), 3);
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().to_string();
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).unwrap();
        if name == "index.md" {
            assert!(!is_pdf(&bytes));
        } else {
            assert!(name.ends_with(".pdf"), "{name}");
            assert!(is_pdf(&bytes), "{name}");
        }
    }
}

#[test]
fn a_typst_compile_error_is_an_err_not_a_panic() {
    let err = render_pdf("#panic(\"boom\")".to_string()).unwrap_err();
    assert!(matches!(err, ExportError::Typst(_)), "{err:?}");
    assert!(err.to_string().contains("boom"), "{err}");
    // File access is refused, so an include cannot read the host.
    let err = render_pdf("#include \"/etc/hosts\"".to_string()).unwrap_err();
    assert!(matches!(err, ExportError::Typst(_)), "{err:?}");
    let err = render_pdf("#read(\"/etc/hosts\")".to_string()).unwrap_err();
    assert!(matches!(err, ExportError::Typst(_)), "{err:?}");
}

// ------------------------------------------------ adversarial compile corpus

/// Markdown built to stress the Markdown -> Typst converter.
fn hostile_markdown() -> String {
    [
        "# A top heading `with code`",
        "",
        "## Second `heading` [link](https://example.com)",
        "",
        "Text with a [relative link](../up/file.md) and a [web link](https://example.com/a?b=c&d=e#f) \
         and *emphasis*[neutral](javascript:alert(1)) straight after, then [x]() empty, \
         then [mail](mailto:a@b.c), and [ftp](ftp://h/f) and [data](data:text/html;base64,AAAA).",
        "",
        "A neutralised link mid-paragraph: before [neutral](javascript:alert(1)) after, and \
         **bold**[also](vbscript:x) and ~~struck~~[more](/relative) tail.",
        "",
        "Every Typst-significant character: # $ * _ = - + / < > @ [ ] ~ ` \\ \" ' { } ( ) ; : & % ^ |",
        "",
        "#panic(\"x\") $ x^2 $ #let a = 1 #import \"@preview/x:0.1.0\" @label <lbl> = not a heading",
        "",
        "- bullet one",
        "- bullet two with `code` and [link](https://example.com)",
        "  - nested bullet",
        "    - nested twice",
        "- bullet three",
        "",
        "1. first",
        "2. second",
        "   1. nested ordered",
        "   2. again",
        "3. third",
        "",
        "> a blockquote with *emphasis* and a [neutral](javascript:x) link",
        ">",
        "> > nested quote",
        "",
        "~~strikethrough~~ and ~~[struck link](https://example.com)~~",
        "",
        "```rust \"quoted\" \\back\\slash",
        "fn main() { println!(\"hi \\\" there\\n\"); } // # $ * _ \\ \"",
        "```",
        "",
        "```\"\\\"",
        "body with \"quotes\" and \\backslashes\\ and ``` inside? no",
        "```",
        "",
        "    indented code block with # and \"",
        "",
        "Inline `code with \"quotes\" \\ and # $` span, and ``double `tick` span``.",
        "",
        "Hard  ",
        "break and soft",
        "break.",
        "",
        "---",
        "",
        "<div>raw html</div> and <span onclick=\"x()\">inline</span>",
        "",
        "![image alt](https://example.com/x.png)",
        "",
        "| a | b |",
        "|---|---|",
        "| 1 | 2 |",
        "",
        "Trailing paragraph.",
    ]
    .join("\n")
}

fn compiles(name: &str, blocks: &[Block]) {
    let markup = typst_doc::render(blocks);
    match render_pdf(markup.clone()) {
        Ok(pdf) => assert!(is_pdf(&pdf), "{name}: not a pdf"),
        Err(e) => panic!("{name} failed to compile: {e}\n--- markup ---\n{markup}"),
    }
}

#[test]
fn hostile_prose_compiles_through_the_real_emitters() {
    compiles("prose", &[Block::Prose(hostile_markdown())]);
    // The same text as a step, where it sits inside a `[ ... ]` content arg.
    compiles(
        "steps",
        &[Block::Steps(vec![hostile_markdown(), "short".into()])],
    );
}

#[test]
fn each_hostile_construct_compiles_on_its_own() {
    // Isolated cases: a failure names the construct instead of the whole soup.
    let cases = [
        "before [neutral](javascript:alert(1)) after",
        "*emphasis*[neutral](javascript:alert(1))",
        "**bold**[neutral](javascript:alert(1)) then more",
        "[x]()",
        "[](https://example.com)",
        "[relative](../a/b) and [abs](/a/b)",
        "[web](https://example.com/a?b=c&d=e#f)",
        "[web](https://example.com/a\"b\\c)",
        "[mail](mailto:a@b.c)",
        "- a\n  - b\n    - c\n- d",
        "1. a\n   1. b\n2. c",
        "- [ ] task\n- [x] done",
        "> quote\n>\n> > nested",
        "> - list in quote\n> - again",
        "- > quote in list",
        "~~s~~",
        "```rust \"q\" \\b\ncode \"q\" \\ # $\n```",
        "```\"\ncode\n```",
        "```\\\ncode\n```",
        "```\n\n```",
        "``",
        "`a \"b\" \\ c`",
        "# # # heading",
        "###### six",
        "#",
        "$",
        "\\",
        "\"",
        "a\\\nb",
        "line one  \nline two",
        "***",
        "* * *",
        "[a](<b c>)",
        "<https://example.com/auto>",
        "<javascript:alert(1)>",
        "[ref][r]\n\n[r]: https://example.com",
        "[ref][r]\n\n[r]: javascript:alert(1)",
        "![alt](https://example.com/i.png)",
        "term\n: definition",
        "| a | b |\n|---|---|\n| 1 | 2 |",
        "1) one\n2) two",
        "- item\n\n  second paragraph in item\n\n  ```\n  code in item\n  ```",
        "",
        "\n\n\n",
    ];
    for md in cases {
        compiles(&format!("prose {md:?}"), &[Block::Prose(md.to_string())]);
        compiles(
            &format!("step {md:?}"),
            &[Block::Steps(vec![md.to_string()])],
        );
    }
}

#[test]
fn every_block_kind_compiles_with_awkward_content() {
    let nasty = "# $ * _ = - + / < > @ [ ] ~ ` \\ \" ' #panic(\"x\") \u{202e} \u{0}";
    compiles(
        "all blocks",
        &[
            Block::Filename(nasty.into()),
            Block::Title(nasty.into()),
            Block::Fields(vec![(nasty.into(), nasty.into()), ("k".into(), "".into())]),
            Block::Fields(vec![]),
            Block::Heading(nasty.into()),
            Block::Prose(nasty.into()),
            Block::Note(nasty.into()),
            Block::Steps(vec![nasty.into(), "".into()]),
            Block::Steps(vec![]),
            Block::Code {
                lang: Some("diff".into()),
                text: "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old \"q\" \\\n+new # $\n".into(),
            },
            Block::Code {
                lang: Some("c++".into()),
                text: nasty.into(),
            },
            Block::Code {
                lang: Some("\"\\ x".into()),
                text: nasty.into(),
            },
            Block::Code {
                lang: None,
                text: String::new(),
            },
            Block::Table {
                headers: vec![nasty.into(), "b".into()],
                rows: vec![
                    vec![nasty.into(), "2".into()],
                    vec!["ragged".into()],
                    vec!["a".into(), "b".into(), "extra".into(), "more".into()],
                    vec![],
                ],
            },
            Block::Table {
                headers: vec![],
                rows: vec![vec!["headerless".into()]],
            },
            Block::Table {
                headers: vec![],
                rows: vec![],
            },
            Block::PageBreak,
            Block::PageBreak,
            Block::Title("after breaks".into()),
        ],
    );
}

#[test]
fn an_empty_document_compiles() {
    compiles("empty", &[]);
}

#[test]
fn hostile_report_fields_compile_end_to_end() {
    let md = hostile_markdown();
    let mut r = full_report();
    r.description = md.clone();
    r.impact = md.clone();
    r.root_cause = md.clone();
    r.remediation = md.clone();
    r.references = md.clone();
    r.replication_steps = vec![md.clone(), "second".into()];
    let mut rec = full_record("fnd_hostile", Severity::Critical, r);
    rec.summary = md.clone();
    rec.declared_by.surface = Surface::Session;
    let mut sum = summary_record("fnd_sum_hostile", Severity::Low);
    sum.evidence.rationale = md.clone();
    sum.evidence.code_excerpt = Some(md.clone());
    sum.evidence.references = vec![md.clone()];
    let all = numbered(vec![
        input("notebin", Some(&md), rec),
        input("notebin", None, sum),
    ]);
    let numbers = number_map(&all);
    for f in &all {
        let blocks = finding_blocks(f, &numbers);
        compiles(&f.number, &blocks);
    }
    let pdf = render_project(&meta(), &all, Format::Pdf).expect("project compiles");
    assert!(is_pdf(&pdf));
}

// ------------------------------------------------------- pathological depth

/// Typst rejects a document whose show rules or parse tree nest too deep, so
/// the converter caps how many quote / emphasis wrappers it emits. The text
/// inside must survive the cap.
#[test]
fn absurd_nesting_compiles_and_keeps_its_words() {
    let cycle = ["*", "_", "**", "__", "~~"];
    let nested = |depth: usize, word: &str| -> String {
        let open: String = (0..depth).map(|i| format!("{}a ", cycle[i % 5])).collect();
        let close: String = (0..depth)
            .rev()
            .map(|i| format!(" a{}", cycle[i % 5]))
            .collect();
        format!("{open}{word}{close}")
    };
    let cases = [
        ("quotes", format!("{} deepquote", ">".repeat(500))),
        ("quotes+list", format!("{} - deepitem", ">".repeat(200))),
        ("mixed inline", nested(3000, "deepinline")),
        (
            "quotes over inline over a link",
            format!(
                "{} {}",
                ">".repeat(100),
                nested(400, "[deeplink](https://example.com)")
            ),
        ),
        (
            "strike only",
            format!("{}deepstrike{}", "~~a ".repeat(300), " a~~".repeat(300)),
        ),
    ];
    for (name, md) in cases {
        let markup = typst_doc::render(&[Block::Prose(md.clone())]);
        let word = md
            .split(|c: char| !c.is_ascii_alphabetic())
            .find(|w| w.starts_with("deep"))
            .unwrap();
        assert!(markup.contains(word), "{name}: lost {word}");
        compiles(name, &[Block::Prose(md.clone())]);
        compiles(name, &[Block::Steps(vec![md])]);
    }
}
