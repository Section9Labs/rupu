//! Transcript-event → live-feed-line projection and (Task 2) the bounded
//! multi-transcript firehose that merges those lines (Plan 3, spec
//! 2026-09-30). `project_event` is pure: no I/O, no clock.

use chrono::{DateTime, Utc};
use rupu_transcript::{Event, FileEditKind};

use crate::output::live_view::layout::printable;
use crate::output::live_view::row::{truncate_to, Line};

/// Display-column budget for the body of a feed row (everything after the
/// codename lead), so one event stays one terminal row.
const FEED_BODY_COLS: usize = 60;

/// One projected firehose row. `line` is a Plan 2 [`Line`] so the renderer
/// colors it like every other row; `codename` tags the source unit /
/// sub-agent; `ts` orders the merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedLine {
    /// Per-event wall-clock time. `rupu_transcript::Event` carries no
    /// per-event timestamp (only `RunStart.started_at`), so
    /// [`project_event`] always yields `None`; the mux orders by arrival.
    pub ts: Option<DateTime<Utc>>,
    pub codename: Option<String>,
    pub line: Line,
}

/// Project one transcript event onto at most one feed row. Returns `None`
/// for events with no feed representation (run/turn boundaries, usage,
/// seeds, tool results, unknowns, …) and for text events with nothing to
/// show. Streamed `AssistantDelta`/`ThinkingDelta` chunks are intentionally
/// dropped: the committed `AssistantMessage`/`Thinking` event that follows
/// carries the whole block, so projecting both would duplicate it as
/// fragment rows.
pub fn project_event(ev: &Event, codename: Option<&str>) -> Option<FeedLine> {
    let body = match ev {
        Event::ToolCall { tool, input, .. } => {
            let arg = tool_key_arg(tool, input);
            let line = Line::new().dim("▸ ").strong(tool.clone());
            Some(if arg.is_empty() {
                line
            } else {
                line.dim(format!(" {arg}"))
            })
        }
        Event::Thinking { text, .. } => {
            let shown = text.as_deref().map(squash).unwrap_or_default();
            // `text: None` = redacted / display-omitted reasoning: the block
            // existed but has no human-readable content. Say so.
            let shown = if shown.is_empty() {
                "(hidden)".to_string()
            } else {
                shown
            };
            Some(Line::new().dim(format!("◇ thinking {shown}")))
        }
        Event::AssistantMessage { content, .. } => {
            let shown = squash(content);
            non_empty(&shown).then(|| Line::new().plain(format!("▪ {shown}")))
        }
        Event::FileEdit { path, kind, .. } => Some(
            Line::new()
                .dim("✎ ")
                .strong(file_edit_word(*kind))
                .plain(format!(" {path}")),
        ),
        Event::CommandRun {
            argv, exit_code, ..
        } => {
            let argv0 = argv.first().map(String::as_str).unwrap_or("<empty>");
            let line = Line::new().dim("$ ").plain(argv0.to_string());
            Some(match exit_code {
                0 => line.dim(" (exit 0)"),
                n => line.danger(format!(" (exit {n})")),
            })
        }
        Event::ActionEmitted { kind, payload, .. } if kind.contains("finding") => {
            let sev = payload
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_uppercase();
            let title = payload
                .get("title")
                .and_then(|v| v.as_str())
                .map(squash)
                .filter(|t| non_empty(t))
                .unwrap_or_else(|| kind.clone());
            let text = if sev.is_empty() {
                format!("⚑ {title}")
            } else {
                format!("⚑ {sev} {title}")
            };
            Some(match sev.as_str() {
                "HIGH" | "CRITICAL" => Line::new().danger(text),
                _ => Line::new().meter(text),
            })
        }
        Event::ActionEmitted { kind, .. } => Some(Line::new().dim("◆ ").plain(kind.clone())),
        // A denied catalog call (step `actions:` narrowing or the run's
        // permission mode) is the ONE tool_audit outcome operators must never
        // miss; allowed calls stay silent (the paired `ToolCall` row already
        // surfaced the attempt).
        Event::ToolAudit {
            tool,
            blocked: true,
            ..
        } => Some(Line::new().danger(format!("✕ {tool} blocked"))),
        Event::Notice { message, .. } => Some(Line::new().dim(format!("! {}", squash(message)))),
        _ => None,
    }?;

    let body = truncate_to(body, FEED_BODY_COLS);
    let mut line = Line::new();
    if let Some(name) = codename {
        line = line.role(role_word(name), name.to_string()).plain(" ");
    }
    line.segments.extend(body.segments);
    // Choke point: every field above is untrusted wire text (model output,
    // paths, titles, tool names, the codename). Control characters (ESC
    // sequences, stray newlines) become U+FFFD so they can neither reach the
    // terminal nor break the single-row guarantee.
    for seg in &mut line.segments {
        seg.text = printable(&seg.text);
    }
    Some(FeedLine {
        ts: None,
        codename: codename.map(str::to_string),
        line,
    })
}

fn non_empty(s: &str) -> bool {
    !s.is_empty()
}

fn file_edit_word(kind: FileEditKind) -> &'static str {
    match kind {
        FileEditKind::Create => "create",
        FileEditKind::Modify => "modify",
        FileEditKind::Delete => "delete",
    }
}

/// Collapse every whitespace run (incl. newlines) to one space so multi-line
/// model text stays a single feed row.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Role word keying the codename's hue: the leaf segment's role, with the
/// `#n` instance and `.attempt` suffixes dropped (`crew/heron#4>lynx#3` →
/// `lynx`). A bare or unparseable name falls back to itself.
fn role_word(codename: &str) -> String {
    let leaf = codename.rsplit(['/', '>']).next().unwrap_or(codename);
    let role = leaf.split(['#', '.']).next().unwrap_or(leaf);
    if role.is_empty() {
        codename.to_string()
    } else {
        role.to_string()
    }
}

/// The single most informative argument of a tool call, or `""`.
fn tool_key_arg(tool: &str, input: &serde_json::Value) -> String {
    let by_tool = match tool {
        "read_file" | "write_file" | "edit_file" => input.get("path"),
        "grep" => input.get("pattern"),
        "bash" => input.get("command"),
        _ => None,
    };
    by_tool
        .or_else(|| input.get("path"))
        .or_else(|| input.get("query"))
        .and_then(|v| v.as_str())
        .map(squash)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::live_view::row::{render_plain, Style};
    use rupu_transcript::{Event, FileEditKind};
    use serde_json::json;

    fn plain(ev: &Event, codename: Option<&str>) -> String {
        render_plain(&[project_event(ev, codename).expect("projects").line])
    }

    #[test]
    fn tool_call_shows_glyph_codename_tool_and_key_arg() {
        let ev = Event::ToolCall {
            call_id: "c1".into(),
            tool: "read_file".into(),
            input: json!({"path": "src/main.rs"}),
        };
        assert_eq!(
            plain(&ev, Some("otter#3")),
            "otter#3 ▸ read_file src/main.rs"
        );
        assert_eq!(plain(&ev, None), "▸ read_file src/main.rs");
    }

    #[test]
    fn tool_call_without_a_summarizable_arg_shows_the_tool_alone() {
        let ev = Event::ToolCall {
            call_id: "c1".into(),
            tool: "list_files".into(),
            input: json!({}),
        };
        assert_eq!(plain(&ev, Some("otter#3")), "otter#3 ▸ list_files");
    }

    #[test]
    fn codename_is_a_role_styled_lead_segment() {
        let ev = Event::ToolCall {
            call_id: "c1".into(),
            tool: "grep".into(),
            input: json!({"pattern": "TODO"}),
        };
        let fl = project_event(&ev, Some("cobalt-harbor/heron#412>lynx#3")).unwrap();
        assert_eq!(
            fl.codename.as_deref(),
            Some("cobalt-harbor/heron#412>lynx#3")
        );
        assert_eq!(fl.ts, None);
        assert_eq!(fl.line.segments[0].text, "cobalt-harbor/heron#412>lynx#3");
        // The hue key is the leaf role word, not the whole path.
        assert_eq!(fl.line.segments[0].style, Style::Role("lynx".into()));
    }

    #[test]
    fn committed_thinking_projects_dim_with_text() {
        let think = Event::Thinking {
            text: Some("weighing the two options".into()),
            provider: "anthropic".into(),
            model: "m".into(),
            raw: json!(null),
        };
        assert_eq!(
            plain(&think, Some("otter#3")),
            "otter#3 ◇ thinking weighing the two options"
        );
        let fl = project_event(&think, None).unwrap();
        assert!(fl.line.segments.iter().all(|s| s.style == Style::Dim));
    }

    #[test]
    fn streamed_deltas_are_dropped_because_the_committed_event_carries_the_block() {
        let assistant = Event::AssistantDelta {
            content: "partial".into(),
        };
        let thinking = Event::ThinkingDelta {
            content: "hmm".into(),
        };
        for codename in [None, Some("otter#3")] {
            assert_eq!(project_event(&assistant, codename), None);
            assert_eq!(project_event(&thinking, codename), None);
        }
    }

    #[test]
    fn redacted_thinking_says_so_rather_than_inventing_text() {
        let think = Event::Thinking {
            text: None,
            provider: "anthropic".into(),
            model: "m".into(),
            raw: json!({"type": "redacted_thinking"}),
        };
        assert_eq!(plain(&think, None), "◇ thinking (hidden)");
    }

    #[test]
    fn committed_assistant_message_projects_and_collapses_to_one_row() {
        let msg = Event::AssistantMessage {
            content: "Found the bug.\n\n  It is in   the parser.".into(),
            thinking: None,
        };
        assert_eq!(
            plain(&msg, Some("otter#3")),
            "otter#3 ▪ Found the bug. It is in the parser."
        );
    }

    #[test]
    fn blank_assistant_message_has_no_feed_row() {
        let ev = Event::AssistantMessage {
            content: "  \n ".into(),
            thinking: None,
        };
        assert_eq!(project_event(&ev, Some("otter#3")), None);
    }

    #[test]
    fn control_characters_are_scrubbed_from_every_field() {
        // ESC sequences and stray newlines in untrusted text (paths, model
        // output, notices, even the codename) must never reach the terminal
        // raw nor break the single-row guarantee.
        let edit = Event::FileEdit {
            path: "a\x1b[2Jb\nc.rs".into(),
            kind: FileEditKind::Create,
            diff: String::new(),
        };
        let msg = Event::AssistantMessage {
            content: "ok\x1b[2J\x1b]0;pwned\x07done\nnext".into(),
            thinking: None,
        };
        let notice = Event::Notice {
            kind: "provider_retry".into(),
            message: "retry\x1b[2J\nnow".into(),
        };
        let call = Event::ToolCall {
            call_id: "c1".into(),
            tool: "bash".into(),
            input: json!({"command": "ls\x1b[2J"}),
        };
        for (ev, expected) in [
            (&edit, "✎ create a\u{FFFD}[2Jb\u{FFFD}c.rs"),
            // Whitespace collapses first, so the newline here is a space.
            (&msg, "▪ ok\u{FFFD}[2J\u{FFFD}]0;pwned\u{FFFD}done next"),
            (&notice, "! retry\u{FFFD}[2J now"),
            (&call, "▸ bash ls\u{FFFD}[2J"),
        ] {
            let fl = project_event(ev, Some("ot\x1b[2Jter#3\n")).unwrap();
            let rendered = render_plain(&[fl.line]);
            assert_eq!(
                rendered,
                format!("ot\u{FFFD}[2Jter#3\u{FFFD} {expected}"),
                "{ev:?}"
            );
            assert!(!rendered.chars().any(char::is_control), "{ev:?}");
        }
    }

    #[test]
    fn long_text_is_clipped_to_one_row_with_an_ellipsis() {
        let ev = Event::AssistantMessage {
            content: "word ".repeat(100),
            thinking: None,
        };
        let fl = project_event(&ev, Some("otter#3")).unwrap();
        let body: usize = fl.line.segments[2..]
            .iter()
            .map(|s| s.text.chars().count())
            .sum();
        assert!(body <= 60, "body {body} cols");
        assert!(render_plain(&[fl.line]).ends_with('…'));
    }

    #[test]
    fn file_edit_and_command_run_project() {
        let edit = Event::FileEdit {
            path: "src/lib.rs".into(),
            kind: FileEditKind::Modify,
            diff: "@@".into(),
        };
        assert_eq!(plain(&edit, Some("otter#3")), "otter#3 ✎ modify src/lib.rs");
        let del = Event::FileEdit {
            path: "old.rs".into(),
            kind: FileEditKind::Delete,
            diff: String::new(),
        };
        assert_eq!(plain(&del, None), "✎ delete old.rs");
        let cmd = Event::CommandRun {
            argv: vec!["cargo".into(), "test".into()],
            cwd: "/w".into(),
            exit_code: 101,
            stdout_bytes: 0,
            stderr_bytes: 0,
        };
        assert_eq!(plain(&cmd, Some("otter#3")), "otter#3 $ cargo (exit 101)");
        let fl = project_event(&cmd, None).unwrap();
        assert!(fl.line.segments.iter().any(|s| s.style == Style::Danger));
    }

    #[test]
    fn finding_action_projects_severity_and_title_with_severity_style() {
        let high = Event::ActionEmitted {
            kind: "report_finding".into(),
            payload: json!({"severity": "high", "title": "SQL injection in login"}),
            allowed: true,
            applied: true,
            reason: None,
        };
        assert_eq!(
            plain(&high, Some("otter#3")),
            "otter#3 ⚑ HIGH SQL injection in login"
        );
        let fl = project_event(&high, None).unwrap();
        assert!(fl.line.segments.iter().any(|s| s.style == Style::Danger));

        let low = Event::ActionEmitted {
            kind: "report_finding".into(),
            payload: json!({"severity": "low", "title": "Verbose banner"}),
            allowed: true,
            applied: true,
            reason: None,
        };
        let fl = project_event(&low, None).unwrap();
        assert!(fl.line.segments.iter().any(|s| s.style == Style::Meter));
        assert!(fl.line.segments.iter().all(|s| s.style != Style::Danger));
    }

    #[test]
    fn finding_without_severity_or_title_still_projects() {
        let ev = Event::ActionEmitted {
            kind: "report_finding".into(),
            payload: json!({}),
            allowed: true,
            applied: true,
            reason: None,
        };
        assert_eq!(plain(&ev, None), "⚑ report_finding");
    }

    #[test]
    fn non_finding_action_projects_its_kind() {
        let ev = Event::ActionEmitted {
            kind: "issues.create".into(),
            payload: json!({}),
            allowed: true,
            applied: true,
            reason: None,
        };
        assert_eq!(plain(&ev, Some("otter#3")), "otter#3 ◆ issues.create");
    }

    #[test]
    fn blocked_tool_audit_projects_danger_and_allowed_one_is_silent() {
        let blocked = Event::ToolAudit {
            tool: "issues.create".into(),
            declared: false,
            granted: true,
            blocked: true,
            restricted: true,
        };
        assert_eq!(
            plain(&blocked, Some("otter#3")),
            "otter#3 ✕ issues.create blocked"
        );
        let fl = project_event(&blocked, None).unwrap();
        assert!(fl.line.segments.iter().any(|s| s.style == Style::Danger));

        let allowed = Event::ToolAudit {
            tool: "issues.create".into(),
            declared: true,
            granted: true,
            blocked: false,
            restricted: true,
        };
        assert!(project_event(&allowed, Some("otter#3")).is_none());
    }

    #[test]
    fn notice_projects_dim() {
        let ev = Event::Notice {
            kind: "provider_retry".into(),
            message: "retrying after 429".into(),
        };
        assert_eq!(plain(&ev, Some("otter#3")), "otter#3 ! retrying after 429");
        let fl = project_event(&ev, None).unwrap();
        assert!(fl.line.segments.iter().all(|s| s.style == Style::Dim));
    }

    #[test]
    fn events_without_a_feed_representation_project_to_none() {
        assert!(project_event(&Event::TurnStart { turn_idx: 0 }, None).is_none());
        assert!(project_event(
            &Event::TurnEnd {
                turn_idx: 0,
                tokens_in: None,
                tokens_out: None,
                stop_reason: None,
                response_id: None,
            },
            Some("otter#3")
        )
        .is_none());
        assert!(project_event(
            &Event::Usage {
                provider: "anthropic".into(),
                model: "m".into(),
                served_model: None,
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: 0,
                cache_write_tokens: 0,
                purpose: None,
            },
            None
        )
        .is_none());
        assert!(project_event(
            &Event::UserMessage {
                content: "hi".into()
            },
            None
        )
        .is_none());
        assert!(project_event(&Event::Unknown, None).is_none());
        assert!(project_event(
            &Event::ToolResult {
                call_id: "c1".into(),
                output: "ok".into(),
                error: None,
                duration_ms: 1,
                structured: None,
            },
            None
        )
        .is_none());
    }
}
