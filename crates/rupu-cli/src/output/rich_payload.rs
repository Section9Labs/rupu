use crate::cmd::transcript::truncate_single_line;
use crate::cmd::ui::{
    highlight_diff, highlight_json, highlight_markdown, highlight_shell, highlight_yaml, UiPrefs,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    Empty,
    Json,
    Jsonl { records: usize },
    Yaml,
    Diff,
    Shell,
    Markdown,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPayload {
    pub kind: PayloadKind,
    pub headline: String,
    pub rendered: String,
}

pub fn render_payload(raw: &str, prefs: &UiPrefs) -> RenderedPayload {
    if raw.trim().is_empty() {
        return RenderedPayload {
            kind: PayloadKind::Empty,
            headline: "empty payload".into(),
            rendered: String::new(),
        };
    }

    if let Some(records) = try_pretty_jsonl(raw) {
        let rendered = highlight_json(&records.join("\n\n"), prefs);
        return RenderedPayload {
            kind: PayloadKind::Jsonl {
                records: records.len(),
            },
            headline: format!("jsonl payload  ·  {} record(s)", records.len()),
            rendered,
        };
    }

    if let Some(pretty) = try_pretty_json(raw) {
        return RenderedPayload {
            kind: PayloadKind::Json,
            headline: "json payload".into(),
            rendered: highlight_json(&pretty, prefs),
        };
    }

    if let Some(pretty) = try_pretty_yaml(raw) {
        return RenderedPayload {
            kind: PayloadKind::Yaml,
            headline: "yaml payload".into(),
            rendered: highlight_yaml(&pretty, prefs),
        };
    }

    if looks_like_diff(raw) {
        return RenderedPayload {
            kind: PayloadKind::Diff,
            headline: "diff payload".into(),
            rendered: highlight_diff(raw.trim_end(), prefs),
        };
    }

    if looks_like_shell(raw) {
        return RenderedPayload {
            kind: PayloadKind::Shell,
            headline: "shell payload".into(),
            rendered: highlight_shell(raw.trim_end(), prefs),
        };
    }

    if looks_like_markdown(raw) {
        return RenderedPayload {
            kind: PayloadKind::Markdown,
            headline: "markdown payload".into(),
            rendered: highlight_markdown(raw.trim_end(), prefs),
        };
    }

    let line_count = raw.lines().count();
    let headline = if line_count > 1 {
        format!("{line_count} lines")
    } else {
        truncate_single_line(raw.trim(), 90)
    };
    RenderedPayload {
        kind: PayloadKind::Plain,
        headline,
        rendered: raw.trim_end().to_string(),
    }
}

pub fn render_tool_input(tool: &str, input: &serde_json::Value, prefs: &UiPrefs) -> Option<String> {
    match tool {
        "bash" => input
            .get("command")
            .and_then(|value| value.as_str())
            .map(|command| highlight_shell(command.trim_end(), prefs)),
        "read_file" | "write_file" | "edit_file" => render_labeled_fields(
            &[("path", input.get("path").and_then(|value| value.as_str()))],
            prefs,
        ),
        "glob" => render_labeled_fields(
            &[(
                "pattern",
                input.get("pattern").and_then(|value| value.as_str()),
            )],
            prefs,
        ),
        "grep" => render_labeled_fields(
            &[
                (
                    "pattern",
                    input.get("pattern").and_then(|value| value.as_str()),
                ),
                ("path", input.get("path").and_then(|value| value.as_str())),
            ],
            prefs,
        ),
        "dispatch_agent" => render_labeled_fields(
            &[
                ("agent", input.get("agent").and_then(|value| value.as_str())),
                (
                    "prompt",
                    input.get("prompt").and_then(|value| value.as_str()),
                ),
            ],
            prefs,
        ),
        "dispatch_agents_parallel" => {
            if let Some(agents) = input.get("agents").and_then(|value| value.as_array()) {
                let listed = agents
                    .iter()
                    .map(|agent| {
                        if let Some(id) = agent.get("id").and_then(|value| value.as_str()) {
                            if let Some(name) = agent.get("agent").and_then(|value| value.as_str())
                            {
                                format!("- {id}: {name}")
                            } else {
                                format!("- {id}")
                            }
                        } else {
                            serde_json::to_string(agent).unwrap_or_default()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut text = String::new();
                if let Some(limit) = input.get("max_parallel").and_then(|value| value.as_u64()) {
                    text.push_str(&format!("max_parallel: {limit}\n"));
                }
                text.push_str("agents:\n");
                for line in listed.lines() {
                    text.push_str("  ");
                    text.push_str(line);
                    text.push('\n');
                }
                Some(highlight_yaml(text.trim_end(), prefs))
            } else {
                serde_json::to_string_pretty(input)
                    .ok()
                    .map(|pretty| highlight_json(&pretty, prefs))
            }
        }
        _ => serde_json::to_string_pretty(input)
            .ok()
            .map(|pretty| highlight_json(&pretty, prefs)),
    }
}

fn try_pretty_json(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

fn try_pretty_jsonl(raw: &str) -> Option<Vec<String>> {
    let mut rows = Vec::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        rows.push(serde_json::to_string_pretty(&value).ok()?);
    }
    if rows.len() > 1 {
        Some(rows)
    } else {
        None
    }
}

fn try_pretty_yaml(raw: &str) -> Option<String> {
    let value: serde_yaml::Value = serde_yaml::from_str(raw.trim()).ok()?;
    serde_yaml::to_string(&value).ok()
}

fn looks_like_markdown(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    trimmed.starts_with('#')
        || trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("```")
        || trimmed.contains("\n#")
        || trimmed.contains("\n- ")
        || trimmed.contains("\n* ")
}

fn looks_like_shell(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    trimmed.starts_with("#!/bin/")
        || trimmed.starts_with("$ ")
        || trimmed.contains("\n$ ")
        || trimmed.starts_with("git ")
        || trimmed.starts_with("cargo ")
        || trimmed.starts_with("npm ")
        || trimmed.starts_with("pnpm ")
}

fn looks_like_diff(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    trimmed.starts_with("diff --git ")
        || (trimmed.contains("\n@@ ") && trimmed.contains("\n--- ") && trimmed.contains("\n+++ "))
}

fn render_labeled_fields(fields: &[(&str, Option<&str>)], prefs: &UiPrefs) -> Option<String> {
    let mut text = String::new();
    let mut wrote = false;
    for (label, value) in fields {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            continue;
        };
        if value.contains('\n') {
            text.push_str(label);
            text.push_str(": |\n");
            for line in value.lines() {
                text.push_str("  ");
                text.push_str(line);
                text.push('\n');
            }
        } else {
            text.push_str(label);
            text.push_str(": ");
            text.push_str(value);
            text.push('\n');
        }
        wrote = true;
    }
    wrote.then(|| highlight_yaml(text.trim_end(), prefs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::ui::LiveViewMode;

    fn prefs() -> UiPrefs {
        UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Full),
        )
    }

    #[test]
    fn render_payload_detects_yaml() {
        let payload = render_payload("name: demo\ncount: 2\n", &prefs());
        assert_eq!(payload.kind, PayloadKind::Yaml);
        assert!(payload.headline.contains("yaml payload"));
    }

    #[test]
    fn render_payload_detects_diff() {
        let raw = "diff --git a/foo b/foo\n--- a/foo\n+++ b/foo\n@@ -1 +1 @@\n-old\n+new\n";
        let payload = render_payload(raw, &prefs());
        assert_eq!(payload.kind, PayloadKind::Diff);
        assert!(payload.headline.contains("diff payload"));
    }
}
