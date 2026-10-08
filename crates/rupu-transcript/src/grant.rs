//! Shared one-line renderers for the tool-grant events (`tool_grant`,
//! `tool_audit`), so every transcript view says the same thing.

use crate::event::{ToolGrantEntry, ToolGrantMissing};

/// How many tool names a `tool_grant` line lists before `+N more`.
const NAMES_SHOWN: usize = 12;

/// One line for a `tool_grant`: the offered tools, then anything narrowed
/// away or unavailable. (`skipped` tools are left out: nobody named them.)
pub fn tool_grant_line(
    entries: &[ToolGrantEntry],
    narrowed: &[String],
    unavailable: &[ToolGrantMissing],
) -> String {
    let mut names: Vec<&str> = entries.iter().map(|e| e.tool.as_str()).collect();
    let more = names.len().saturating_sub(NAMES_SHOWN);
    names.truncate(NAMES_SHOWN);
    let mut line = format!(
        "{} tool{}",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" }
    );
    if !names.is_empty() {
        line.push_str(": ");
        line.push_str(&names.join(", "));
        if more > 0 {
            line.push_str(&format!(" (+{more} more)"));
        }
    }
    if !narrowed.is_empty() {
        line.push_str(&format!(
            "  ·  narrowed by actions: {}",
            narrowed.join(", ")
        ));
    }
    if !unavailable.is_empty() {
        let u: Vec<String> = unavailable
            .iter()
            .map(|m| format!("{} (needs {})", m.tool, m.missing.join(", ")))
            .collect();
        line.push_str(&format!("  ·  unavailable: {}", u.join(", ")));
    }
    line
}

/// Whether a `tool_audit` line is worth its own row in a view that already
/// shows the call: a denial, or a step `actions:` naming a tool the agent
/// was never granted. Every call is audited; an allowed call's audit only
/// repeats its `tool_call` row.
pub fn tool_audit_notable(blocked: bool, declared: bool, granted: bool) -> bool {
    blocked || (declared && !granted)
}

/// The detail text of a `tool_audit` row.
pub fn tool_audit_detail(
    tool: &str,
    declared: bool,
    granted: bool,
    blocked: bool,
    decision: Option<&str>,
) -> String {
    match decision {
        Some(d) => format!("{tool}  ·  {d}  ·  declared={declared} granted={granted}"),
        None => format!("{tool}  ·  declared={declared} granted={granted} blocked={blocked}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_line_lists_tools_and_what_was_left_out() {
        let e = |t: &str| ToolGrantEntry {
            tool: t.into(),
            reasons: vec!["declared".into()],
        };
        let line = tool_grant_line(
            &[e("bash"), e("read_file")],
            &["scm.prs.get".into()],
            &[ToolGrantMissing {
                tool: "board.post".into(),
                missing: vec!["message_bus".into()],
            }],
        );
        assert_eq!(
            line,
            "2 tools: bash, read_file  ·  narrowed by actions: scm.prs.get  ·  unavailable: board.post (needs message_bus)"
        );
        let many: Vec<_> = (0..14).map(|i| e(&format!("t{i}"))).collect();
        assert!(tool_grant_line(&many, &[], &[]).ends_with("(+2 more)"));
        assert_eq!(tool_grant_line(&[], &[], &[]), "0 tools");
    }

    #[test]
    fn only_denials_and_ungranted_declarations_are_notable() {
        assert!(tool_audit_notable(true, false, true));
        assert!(tool_audit_notable(false, true, false));
        assert!(!tool_audit_notable(false, false, true));
        assert!(!tool_audit_notable(false, true, true));
    }
}
