//! `allowed-tools` names from other harnesses, mapped to rupu's core
//! tools. Used only to warn when a run lacks a tool a skill expects
//! (spec §3.5); it never grants anything.

const MAP: &[(&str, &str)] = &[
    ("Bash", "bash"),
    ("Read", "read_file"),
    ("Write", "write_file"),
    ("Edit", "edit_file"),
    ("MultiEdit", "edit_file"),
    ("Grep", "grep"),
    ("Glob", "glob"),
];

pub fn mapped_tools(allowed: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = allowed
        .iter()
        .filter_map(|entry| {
            let base = entry.split('(').next().unwrap_or(entry).trim();
            MAP.iter()
                .find(|(from, _)| *from == base)
                .map(|(_, to)| *to)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}
