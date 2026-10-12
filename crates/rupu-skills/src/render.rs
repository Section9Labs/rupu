//! The text a model sees (spec §3.1 to §3.3). Changing a string here
//! changes every run's system prompt; the tests pin each byte.

use crate::tree::ContentHash;
use std::path::Path;

pub struct IndexItem<'a> {
    pub name: &'a str,
    pub description: &'a str,
}

pub struct PreloadItem<'a> {
    pub name: &'a str,
    pub base_dir: &'a Path,
    pub body: &'a str,
}

const INDEX_HEADER: &str = "## Skills\n\n\
Each skill below holds instructions, and sometimes scripts and reference files,\n\
for one kind of task. When a task matches a skill's description, call\n\
`skills.load` with its name before you start, and follow it. Paths inside a\n\
skill are relative to the base directory that `skills.load` returns.";

pub fn render_skills_section(
    preloads: &[PreloadItem<'_>],
    index: &[IndexItem<'_>],
) -> Option<String> {
    let mut parts = Vec::new();
    let mut preloads: Vec<&PreloadItem<'_>> = preloads.iter().collect();
    preloads.sort_by_key(|p| p.name);
    for p in preloads {
        let mut part = format!(
            "## Skill: {}\n\nBase directory: {}",
            p.name,
            p.base_dir.display()
        );
        if !p.body.is_empty() {
            part.push_str("\n\n");
            part.push_str(p.body);
        }
        parts.push(part);
    }
    if !index.is_empty() {
        let mut index: Vec<&IndexItem<'_>> = index.iter().collect();
        index.sort_by_key(|i| i.name);
        let lines: Vec<String> = index
            .iter()
            .map(|i| {
                let description: Vec<&str> = i.description.split_whitespace().collect();
                format!("- {}: {}", i.name, description.join(" "))
            })
            .collect();
        parts.push(format!("{INDEX_HEADER}\n\n{}", lines.join("\n")));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

pub struct LoadView<'a> {
    pub name: &'a str,
    pub version: ContentHash,
    pub base_dir: &'a Path,
    pub files: &'a [&'a str],
    pub compatibility: Option<&'a str>,
    pub changed_note: Option<&'a str>,
    pub body: &'a str,
}

/// `files` may include `SKILL.md`; it is left off the `Files:` line because
/// its body is the result's own body.
pub fn render_load_result(v: &LoadView<'_>) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.extend(v.changed_note.map(str::to_owned));
    lines.push(format!("Skill: {} (version {})", v.name, v.version.short()));
    lines.push(format!("Base directory: {}", v.base_dir.display()));
    let files: Vec<&str> = v
        .files
        .iter()
        .copied()
        .filter(|f| *f != "SKILL.md")
        .collect();
    if !files.is_empty() {
        lines.push(format!("Files: {}", files.join(", ")));
    }
    lines.extend(v.compatibility.map(|c| format!("Compatibility: {c}")));
    let mut out = lines.join("\n");
    if !v.body.is_empty() {
        out.push_str("\n\n");
        out.push_str(v.body);
    }
    out
}
