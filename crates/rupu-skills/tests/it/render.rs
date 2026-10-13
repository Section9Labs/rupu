use rupu_skills::render::{
    render_load_result, render_skills_section, IndexItem, LoadView, PreloadItem,
};
use rupu_skills::tools_map::mapped_tools;
use rupu_skills::{parse_skill_md, ContentHash};
use std::path::Path;

#[test]
fn nothing_renders_nothing() {
    assert_eq!(render_skills_section(&[], &[]), None);
}

#[test]
fn index_is_sorted_and_exact() {
    let got = render_skills_section(
        &[],
        &[
            IndexItem {
                name: "pdf-processing",
                description: "Extract PDF text. Use when handling PDFs.",
            },
            IndexItem {
                name: "code-review",
                description: "Review a diff.",
            },
        ],
    )
    .unwrap();
    assert_eq!(
        got,
        "## Skills\n\n\
Each skill below holds instructions, and sometimes scripts and reference files,\n\
for one kind of task. When a task matches a skill's description, call\n\
`skills.load` with its name before you start, and follow it. Paths inside a\n\
skill are relative to the base directory that `skills.load` returns.\n\n\
- code-review: Review a diff.\n\
- pdf-processing: Extract PDF text. Use when handling PDFs."
    );
}

#[test]
fn preloads_come_first_then_the_index() {
    let got = render_skills_section(
        &[PreloadItem {
            name: "house-style",
            base_dir: Path::new("/h/.rupu/skills/house-style"),
            body: "Write plainly.",
        }],
        &[IndexItem {
            name: "code-review",
            description: "Review a diff.",
        }],
    )
    .unwrap();
    assert!(got.starts_with("## Skill: house-style\n\nBase directory: /h/.rupu/skills/house-style\n\nWrite plainly.\n\n## Skills\n\n"));
    assert!(got.ends_with("- code-review: Review a diff."));
}

#[test]
fn load_result_is_exact() {
    let got = render_load_result(&LoadView {
        name: "pdf-processing",
        version: ContentHash([0xab; 32]),
        base_dir: Path::new("/h/.rupu/skills/pdf-processing"),
        files: &["SKILL.md", "scripts/extract.py"],
        compatibility: Some("Requires Python 3.12+"),
        changed_note: Some(
            "This skill changed on disk since this run started; this is its current version.",
        ),
        body: "Step one.",
    });
    assert_eq!(
        got,
        "This skill changed on disk since this run started; this is its current version.\n\
Skill: pdf-processing (version abababababab)\n\
Base directory: /h/.rupu/skills/pdf-processing\n\
Files: scripts/extract.py\n\
Compatibility: Requires Python 3.12+\n\n\
Step one."
    );
}

#[test]
fn a_multi_line_description_renders_on_one_index_line() {
    let got = render_skills_section(
        &[],
        &[IndexItem {
            name: "pdf-processing",
            description: "Extract PDF text.\n  Use when\thandling PDFs.\n",
        }],
    )
    .unwrap();
    assert!(
        got.ends_with("\n\n- pdf-processing: Extract PDF text. Use when handling PDFs."),
        "{got:?}"
    );
}

#[test]
fn preloads_are_sorted_by_name() {
    let got = render_skills_section(
        &[
            PreloadItem {
                name: "zeta",
                base_dir: Path::new("/z"),
                body: "Z.",
            },
            PreloadItem {
                name: "alpha",
                base_dir: Path::new("/a"),
                body: "A.",
            },
        ],
        &[],
    )
    .unwrap();
    assert_eq!(
        got,
        "## Skill: alpha\n\nBase directory: /a\n\nA.\n\n## Skill: zeta\n\nBase directory: /z\n\nZ."
    );
}

#[test]
fn a_preload_with_an_empty_body_renders_its_header_only() {
    let got = render_skills_section(
        &[PreloadItem {
            name: "empty",
            base_dir: Path::new("/e"),
            body: "",
        }],
        &[IndexItem {
            name: "code-review",
            description: "Review a diff.",
        }],
    )
    .unwrap();
    assert!(
        got.starts_with("## Skill: empty\n\nBase directory: /e\n\n## Skills\n\n"),
        "{got:?}"
    );
    assert_eq!(
        render_skills_section(
            &[PreloadItem {
                name: "empty",
                base_dir: Path::new("/e"),
                body: "",
            }],
            &[],
        )
        .unwrap(),
        "## Skill: empty\n\nBase directory: /e"
    );
}

#[test]
fn a_load_result_with_an_empty_body_ends_after_its_header() {
    let got = render_load_result(&LoadView {
        name: "notes",
        version: ContentHash([0x01; 32]),
        base_dir: Path::new("/h/.rupu/skills/notes"),
        files: &["SKILL.md", "references/A.md"],
        compatibility: Some("Any"),
        changed_note: None,
        body: "",
    });
    assert_eq!(
        got,
        "Skill: notes (version 010101010101)\n\
Base directory: /h/.rupu/skills/notes\n\
Files: references/A.md\n\
Compatibility: Any"
    );
}

#[test]
fn a_load_result_with_only_skill_md_has_no_files_line() {
    let got = render_load_result(&LoadView {
        name: "solo",
        version: ContentHash([0x02; 32]),
        base_dir: Path::new("/s"),
        files: &["SKILL.md"],
        compatibility: None,
        changed_note: None,
        body: "Do it.",
    });
    assert_eq!(
        got,
        "Skill: solo (version 020202020202)\nBase directory: /s\n\nDo it."
    );
}

#[test]
fn allowed_tools_map_to_core_names() {
    let allowed: Vec<String> = [
        "Bash(git status:*)",
        "Read",
        "Grep",
        "Glob",
        "Edit",
        "MultiEdit",
        "Write",
        "WebFetch",
        "mcp__x__y",
        "Bash(jq:*)",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        mapped_tools(&allowed),
        vec![
            "bash",
            "edit_file",
            "glob",
            "grep",
            "read_file",
            "write_file"
        ]
    );
}

#[test]
fn allowed_tools_line_with_a_spaced_pattern_maps_to_two_tools() {
    let (fm, _) = parse_skill_md(
        "---\nname: a\ndescription: d\nallowed-tools: Bash(git status:*) Read\n---\n",
    )
    .unwrap();
    assert_eq!(mapped_tools(&fm.allowed_tools), vec!["bash", "read_file"]);
}

#[test]
fn an_over_limit_index_description_is_cut_at_1024_chars() {
    let long = "é".repeat(1100);
    let exact = "x".repeat(1024);
    let out = render_skills_section(
        &[],
        &[
            IndexItem {
                name: "long",
                description: &long,
            },
            IndexItem {
                name: "exact",
                description: &exact,
            },
        ],
    )
    .unwrap();
    let lines: Vec<&str> = out.lines().rev().take(2).collect();
    assert_eq!(lines[0], format!("- long: {}…", "é".repeat(1024)));
    assert_eq!(lines[1], format!("- exact: {exact}"));
}

#[test]
fn reattach_block_is_exact() {
    use rupu_skills::render::{render_reattach, ReattachItem, REATTACH_MARKER};
    let got = render_reattach(
        &[ReattachItem {
            name: "pdf",
            base_dir: Path::new("/h/skills/pdf"),
            body: "Step one.",
        }],
        &["docx"],
        &["gone"],
    );
    assert_eq!(
        got,
        "<skills-in-use>\n\
You loaded these skills earlier in this run. Their instructions follow at their current version; keep following them.\n\n\
## Skill: pdf\n\nBase directory: /h/skills/pdf\n\nStep one.\n\n\
Not re-attached for space (reload with `skills.load` when you need them): docx\n\
No longer installed: gone\n\
</skills-in-use>"
    );
    assert!(got.starts_with(REATTACH_MARKER));
}
