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
        parts.push(format!(
            "## Skill: {}\n\nBase directory: {}\n\n{}",
            p.name,
            p.base_dir.display(),
            p.body
        ));
    }
    if !index.is_empty() {
        let mut index: Vec<&IndexItem<'_>> = index.iter().collect();
        index.sort_by_key(|i| i.name);
        let lines: Vec<String> = index
            .iter()
            .map(|i| format!("- {}: {}", i.name, i.description))
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

pub fn render_load_result(v: &LoadView<'_>) -> String {
    let mut out = String::new();
    if let Some(note) = v.changed_note {
        out.push_str(note);
        out.push('\n');
    }
    out.push_str(&format!(
        "Skill: {} (version {})\n",
        v.name,
        v.version.short()
    ));
    out.push_str(&format!("Base directory: {}\n", v.base_dir.display()));
    out.push_str(&format!("Files: {}\n", v.files.join(", ")));
    if let Some(c) = v.compatibility {
        out.push_str(&format!("Compatibility: {c}\n"));
    }
    out.push('\n');
    out.push_str(v.body);
    out
}
