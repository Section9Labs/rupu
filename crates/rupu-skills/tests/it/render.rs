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
Files: SKILL.md, scripts/extract.py\n\
Compatibility: Requires Python 3.12+\n\n\
Step one."
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
