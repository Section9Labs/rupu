use crate::cmd::transcript::truncate_single_line;
use crate::cmd::ui::{highlight_diff, highlight_json, highlight_markdown, highlight_yaml, UiPrefs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    Empty,
    Json,
    Jsonl { records: usize },
    Yaml,
    Diff,
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
            .map(|command| command.trim_end().to_string()),
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

fn looks_like_diff(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    trimmed.starts_with("diff --git ")
        || (trimmed.contains("\n@@ ") && trimmed.contains("\n--- ") && trimmed.contains("\n+++ "))
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
