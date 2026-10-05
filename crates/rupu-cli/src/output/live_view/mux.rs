//! Transcript-event → live-feed-line projection and the bounded
//! multi-transcript firehose that merges those lines (Plan 3, spec
//! 2026-09-30). `project_event` is pure: no I/O, no clock; `TranscriptMux`
//! owns the (bounded) transcript I/O.

use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rupu_transcript::{Event, FileEditKind};

use crate::output::jsonl_reader::TranscriptTailer;
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
        // A classified non-normal reply (refusal, cut-off, malformed call, …)
        // and what the run did about it. Errors are the rows operators must
        // not miss; warnings share the finding-meter tone; recovery is dim.
        Event::Outcome { outcome, .. } => {
            let text = squash(&rupu_transcript::outcome::outcome_line(outcome));
            Some(match outcome.severity {
                rupu_transcript::Severity::Error => Line::new().danger(text),
                rupu_transcript::Severity::Warning => Line::new().meter(text),
                rupu_transcript::Severity::Info => Line::new().dim(text),
            })
        }
        // The feed draws no status glyph of its own, so it takes the
        // glyph-bearing one-line forms.
        Event::Recovery {
            rung,
            action,
            attempt,
            budget,
            provider,
            model,
            reason,
            ..
        } => Some(
            Line::new().dim(squash(&rupu_transcript::outcome::recovery_line(
                *action,
                *rung,
                provider.as_deref(),
                model.as_deref(),
                *attempt,
                *budget,
                reason.as_deref(),
            ))),
        ),
        // A reply block with no event of its own: a server-side fallback
        // boundary, an unrecognized block, or one abandoned at a mid-output
        // fallback.
        Event::AssistantBlock { block, abandoned } => Some(Line::new().dim(format!(
            "· {}",
            squash(&rupu_transcript::outcome::assistant_block_line(
                block, *abandoned
            ))
        ))),
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

/// Floor / ceiling on how many transcripts are tailed at once, whatever the
/// caller derived from the fd budget.
const MIN_TAILS: usize = 4;
const MAX_TAILS: usize = 64;
/// Merged feed ring capacity (oldest rows drop first).
const RING_CAP: usize = 500;
/// Pinned-stream buffer capacity (oldest rows drop first).
const PINNED_CAP: usize = 500;
/// Descriptors left untouched when deciding whether another tail may open.
const FD_RESERVE: u64 = 16;

/// Per-transcript bookkeeping, kept for every observed path (open or not).
struct Observed {
    /// Grouping / lookup key handed to [`project_event`]; never display text
    /// of its own.
    codename: Option<String>,
    /// Registration order: the stable drain order within one tick.
    order: u64,
    /// LRU stamp (larger = more recently active). Eviction drops the
    /// smallest unpinned one.
    last_active: u64,
    /// Stamp of the last time this path was drained. A closed path that has
    /// changed on disk is re-ranked least-recently-served first, so a busy
    /// fan-out larger than the budget rotates instead of starving the
    /// early-registered units.
    served: u64,
    /// Raw transcript events already consumed. A re-opened tailer restarts at
    /// byte 0 and skips this many events, so an evict/re-open cycle never
    /// re-projects rows already in the ring.
    consumed: usize,
    /// File length when last drained: a closed path whose length differs has
    /// new output and competes for a slot again.
    seen_len: u64,
}

struct OpenTail {
    tailer: TranscriptTailer,
    /// Events still to skip after a (re-)open from offset 0.
    skip: usize,
}

/// Bounded multi-transcript tail: merges the unit / sub-agent transcripts a
/// run produces into one arrival-ordered firehose without ever touching more
/// than `max_tails` files per tick, and without opening any more when the
/// process is short of file descriptors. The pinned (drilled) transcript is
/// always tailed and never evicted, and its own stream is kept in a dedicated
/// buffer so it stays complete however chatty the siblings are. Display-only:
/// token / usage totals live elsewhere.
pub struct TranscriptMux {
    /// Concurrently tailed transcripts, pinned included (clamped 4..=64).
    max_tails: usize,
    pinned: Option<PathBuf>,
    observed: HashMap<PathBuf, Observed>,
    open: HashMap<PathBuf, OpenTail>,
    /// Merged cross-unit firehose, arrival order.
    ring: VecDeque<FeedLine>,
    /// The pinned transcript's OWN stream (history from offset 0, then live),
    /// independent of the shared ring so sibling chatter can't scroll it out.
    pinned_buf: VecDeque<Line>,
    /// Monotonic stamp source for `order` / `last_active` / `served`.
    seq: u64,
    /// `(open descriptors, limit)`; injectable so tests don't depend on the
    /// test process's real descriptor table.
    fd_probe: fn() -> Option<(u64, u64)>,
}

impl TranscriptMux {
    /// `max_tails` is clamped to `4..=64`; callers derive it from the fd
    /// budget and leave headroom.
    pub fn new(max_tails: usize) -> Self {
        Self {
            max_tails: max_tails.clamp(MIN_TAILS, MAX_TAILS),
            pinned: None,
            observed: HashMap::new(),
            open: HashMap::new(),
            ring: VecDeque::new(),
            pinned_buf: VecDeque::new(),
            seq: 0,
            fd_probe: rupu_agent::fd_budget::fd_usage,
        }
    }

    /// Register a transcript the run produced. Idempotent: a repeat keeps the
    /// existing state and only upgrades the codename when a new one is given.
    /// Registering counts as activity (a brand-new unit is the most recently
    /// active), but does not open anything by itself; [`drain`](Self::drain)
    /// decides who gets a slot.
    pub fn observe(&mut self, path: PathBuf, codename: Option<String>) {
        self.register(path, codename);
    }

    /// Pin the drilled unit's transcript (or clear the pin). A pinned path is
    /// always tailed and exempt from eviction. Re-pinning the SAME path is a
    /// no-op (callers may pin every tick); moving or clearing the pin resets
    /// the stream buffer, and a newly pinned path is re-tailed from offset 0
    /// on the next [`drain`](Self::drain) so its whole history fills the buffer.
    pub fn pin(&mut self, path: Option<PathBuf>) {
        if self.pinned == path {
            return;
        }
        self.pinned_buf.clear();
        if let Some(p) = &path {
            self.register(p.clone(), None);
            self.open.remove(p);
        }
        self.pinned = path;
    }

    /// Advance the open tails: the pinned one plus the most-recently-active
    /// observed paths that fit the budget. New events are projected, tagged
    /// with their path's codename, and appended to the capped ring in arrival
    /// order (the pinned path's rows also go to its dedicated stream buffer).
    /// A transcript that cannot be read (not created yet, fd
    /// exhaustion) is swallowed and simply contributes nothing this tick.
    pub fn drain(&mut self) {
        self.bump_changed_closed();
        let wanted = self.wanted();
        self.open.retain(|p, _| wanted.contains(p));

        let probe = self.fd_probe;
        let mut headroom: Option<bool> = None;
        for path in &wanted {
            if !self.open.contains_key(path) {
                let pinned = self.pinned.as_ref() == Some(path);
                // The pinned stream is what the operator is looking at: it
                // always tries. Everything else waits for descriptor headroom.
                if !pinned && !*headroom.get_or_insert_with(|| fd_headroom(probe)) {
                    continue;
                }
            }
            self.drain_one(path);
        }
    }

    /// The merged cross-unit ring, oldest to newest.
    pub fn firehose_lines(&self) -> Vec<Line> {
        self.ring.iter().map(|f| f.line.clone()).collect()
    }

    /// The pinned transcript's own stream, oldest to newest: its history from
    /// offset 0 plus live rows, kept in a dedicated buffer so it is complete
    /// regardless of sibling activity. Empty when nothing is pinned.
    pub fn pinned_lines(&self) -> Vec<Line> {
        self.pinned_buf.iter().cloned().collect()
    }

    fn register(&mut self, path: PathBuf, codename: Option<String>) {
        if let Some(entry) = self.observed.get_mut(&path) {
            if codename.is_some() {
                entry.codename = codename;
            }
            return;
        }
        self.seq += 1;
        self.observed.insert(
            path,
            Observed {
                codename,
                order: self.seq,
                last_active: self.seq,
                served: 0,
                consumed: 0,
                seen_len: 0,
            },
        );
    }

    /// A closed path whose file grew since it was last drained is "newly
    /// active": re-stamp it so it can displace the least-recently-active open
    /// tail. Among several, the least recently served gets the newest stamp.
    fn bump_changed_closed(&mut self) {
        let mut changed: Vec<(PathBuf, u64, u64)> = self
            .observed
            .iter()
            .filter(|(p, o)| {
                !self.open.contains_key(*p)
                    && self.pinned.as_ref() != Some(*p)
                    && file_len(p) != o.seen_len
            })
            .map(|(p, o)| (p.clone(), o.served, o.order))
            .collect();
        changed.sort_by_key(|&(_, served, order)| (Reverse(served), order));
        for (path, _, _) in changed {
            self.seq += 1;
            if let Some(o) = self.observed.get_mut(&path) {
                o.last_active = self.seq;
            }
        }
    }

    /// The paths to tail this tick: the pinned one, then the most recently
    /// active others filling the remaining slots, in registration order.
    fn wanted(&self) -> Vec<PathBuf> {
        let slots = self.max_tails - usize::from(self.pinned.is_some());
        let mut rest: Vec<(&PathBuf, &Observed)> = self
            .observed
            .iter()
            .filter(|(p, _)| self.pinned.as_ref() != Some(*p))
            .collect();
        rest.sort_by_key(|(_, o)| Reverse(o.last_active));
        rest.truncate(slots);
        rest.sort_by_key(|(_, o)| o.order);
        let mut wanted = Vec::with_capacity(slots + 1);
        wanted.extend(self.pinned.clone());
        wanted.extend(rest.into_iter().map(|(p, _)| p.clone()));
        wanted
    }

    /// Open (if needed) and drain one transcript, projecting its new events
    /// into the ring (and, for the pinned path, its stream buffer).
    fn drain_one(&mut self, path: &Path) {
        let is_pinned = self.pinned.as_deref() == Some(path);
        let Some(entry) = self.observed.get_mut(path) else {
            return;
        };
        let tail = self
            .open
            .entry(path.to_path_buf())
            .or_insert_with(|| OpenTail {
                tailer: TranscriptTailer::new(path),
                skip: entry.consumed,
            });
        // Measured BEFORE the read so a write racing the drain still shows
        // up as a change on the next probe.
        entry.seen_len = file_len(path);
        self.seq += 1;
        entry.served = self.seq;

        let mut fresh = false;
        for ev in tail.tailer.drain() {
            // Replayed prefix after a re-open from offset 0: already in the
            // ring, so it never re-enters it. The pinned stream wants it
            // anyway (that is its history); other paths skip it outright.
            let replay = tail.skip > 0;
            if replay {
                tail.skip -= 1;
                if !is_pinned {
                    continue;
                }
            } else {
                fresh = true;
                entry.consumed += 1;
            }
            if let Some(feed) = project_event(&ev, entry.codename.as_deref()) {
                if is_pinned {
                    if self.pinned_buf.len() >= PINNED_CAP {
                        self.pinned_buf.pop_front();
                    }
                    self.pinned_buf.push_back(feed.line.clone());
                }
                if !replay {
                    if self.ring.len() >= RING_CAP {
                        self.ring.pop_front();
                    }
                    self.ring.push_back(feed);
                }
            }
        }
        if fresh {
            self.seq += 1;
            entry.last_active = self.seq;
        }
    }
}

/// Current length of `path`, `0` when it can't be stat'ed (missing file).
fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Whether the process has descriptors to spare for another tail. An
/// unknowable budget (`None`) is treated as fine.
fn fd_headroom(probe: fn() -> Option<(u64, u64)>) -> bool {
    match probe() {
        Some((used, limit)) => used.saturating_add(FD_RESERVE) < limit,
        None => true,
    }
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
                discarded: false,
                stop: None,
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
        assert!(project_event(
            &Event::Unknown {
                tag: "future_event".into(),
                data: serde_json::Value::Null
            },
            None
        )
        .is_none());
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

    #[test]
    fn project_event_surfaces_outcome_and_recovery() {
        let outcome = Event::Outcome {
            turn_idx: 1,
            outcome: rupu_transcript::OutcomeRecord {
                id: "oc_1".into(),
                class: "refusal".into(),
                severity: rupu_transcript::Severity::Error,
                title: "refused · cyber".into(),
                detail: None,
                error_class: None,
                wire: serde_json::Value::Null,
            },
        };
        let row = project_event(&outcome, Some("otter#3")).expect("outcome row");
        assert!(render_plain(std::slice::from_ref(&row.line)).contains("refused · cyber"));
        let recovery = Event::Recovery {
            outcome_id: "oc_1".into(),
            rung: 1,
            action: rupu_transcript::RecoveryAction::Retried,
            attempt: None,
            budget: None,
            provider: None,
            model: None,
            reason: None,
            merge_into_previous: false,
            continues_output: false,
        };
        let row = project_event(&recovery, None).expect("recovery row");
        assert!(render_plain(std::slice::from_ref(&row.line)).contains("↺ rung 1 · retried"));
        let block = Event::AssistantBlock {
            block: serde_json::json!({
                "type": "fallback", "from_model": "model-a", "to_model": "model-b"
            }),
            abandoned: false,
        };
        let row = project_event(&block, None).expect("assistant_block row");
        assert!(render_plain(std::slice::from_ref(&row.line))
            .contains("served by fallback · model-a → model-b"));
    }

    // ---- TranscriptMux -------------------------------------------------

    use std::io::Write;

    fn msg(text: &str) -> Event {
        Event::AssistantMessage {
            content: text.into(),
            thinking: None,
        }
    }

    fn write_events(path: &Path, events: &[Event]) {
        let mut f = std::fs::File::create(path).unwrap();
        for ev in events {
            writeln!(f, "{}", serde_json::to_string(ev).unwrap()).unwrap();
        }
    }

    fn append_event(path: &Path, ev: &Event) {
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(f, "{}", serde_json::to_string(ev).unwrap()).unwrap();
    }

    /// A mux that never trips the fd guard (the real probe reads this test
    /// process's descriptor table, which parallel tests make unpredictable).
    fn mux(max_tails: usize) -> TranscriptMux {
        let mut m = TranscriptMux::new(max_tails);
        m.fd_probe = || None;
        m
    }

    fn plain_lines(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| render_plain(std::slice::from_ref(l)))
            .collect()
    }

    /// `n` transcripts under `dir`, `names[i]` with two uniquely-worded
    /// messages each: `"{name} first"` / `"{name} second"`.
    fn seed(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| {
                let path = dir.join(format!("{name}.jsonl"));
                write_events(
                    &path,
                    &[
                        msg(&format!("{name} first")),
                        msg(&format!("{name} second")),
                    ],
                );
                path
            })
            .collect()
    }

    #[test]
    fn new_clamps_the_tail_budget_to_a_sane_range() {
        assert_eq!(TranscriptMux::new(0).max_tails, 4);
        assert_eq!(TranscriptMux::new(1).max_tails, 4);
        assert_eq!(TranscriptMux::new(16).max_tails, 16);
        assert_eq!(TranscriptMux::new(10_000).max_tails, 64);
    }

    #[test]
    fn merges_three_transcripts_into_one_firehose_tagged_by_codename() {
        let dir = tempfile::tempdir().unwrap();
        let paths = seed(dir.path(), &["a", "b", "c"]);
        let mut m = mux(8);
        for (p, name) in paths.iter().zip(["otter#1", "heron#2", "lynx#3"]) {
            m.observe(p.clone(), Some(name.to_string()));
        }
        m.drain();
        let lines = plain_lines(&m.firehose_lines());
        assert_eq!(lines.len(), 6, "{lines:?}");
        for (name, word) in [("otter#1", "a"), ("heron#2", "b"), ("lynx#3", "c")] {
            assert!(
                lines.contains(&format!("{name} ▪ {word} first")),
                "{lines:?}"
            );
            assert!(
                lines.contains(&format!("{name} ▪ {word} second")),
                "{lines:?}"
            );
        }
        // Within one transcript, order is preserved.
        let first = lines.iter().position(|l| l == "otter#1 ▪ a first").unwrap();
        let second = lines
            .iter()
            .position(|l| l == "otter#1 ▪ a second")
            .unwrap();
        assert!(first < second);
    }

    #[test]
    fn observe_is_idempotent_and_keeps_the_codename_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let paths = seed(dir.path(), &["a"]);
        let mut m = mux(8);
        m.observe(paths[0].clone(), Some("otter#1".into()));
        m.observe(paths[0].clone(), None);
        m.observe(paths[0].clone(), Some("otter#1".into()));
        m.drain();
        m.drain();
        let lines = plain_lines(&m.firehose_lines());
        assert_eq!(
            lines,
            vec!["otter#1 ▪ a first".to_string(), "otter#1 ▪ a second".into()]
        );
        assert_eq!(m.open.len(), 1);
    }

    #[test]
    fn drain_only_projects_new_events_each_tick() {
        let dir = tempfile::tempdir().unwrap();
        let paths = seed(dir.path(), &["a"]);
        let mut m = mux(8);
        m.observe(paths[0].clone(), Some("otter#1".into()));
        m.drain();
        assert_eq!(m.firehose_lines().len(), 2);
        m.drain();
        assert_eq!(m.firehose_lines().len(), 2, "nothing new, nothing added");
        append_event(&paths[0], &msg("a third"));
        m.drain();
        let lines = plain_lines(&m.firehose_lines());
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2], "otter#1 ▪ a third");
    }

    #[test]
    fn open_set_stays_within_budget_and_every_unit_is_still_served() {
        let dir = tempfile::tempdir().unwrap();
        // new(1) clamps to the floor of 4; six units overflow it.
        let names = ["a", "b", "c", "d", "e", "f"];
        let paths = seed(dir.path(), &names);
        let mut m = mux(1);
        assert_eq!(m.max_tails, 4);
        for p in &paths {
            m.observe(p.clone(), None);
        }
        for _ in 0..6 {
            m.drain();
            assert!(m.open.len() <= 4, "open {}", m.open.len());
        }
        // Rotation reached every unit exactly once: 12 rows, no duplicates
        // from the evict -> re-open cycle.
        let lines = plain_lines(&m.firehose_lines());
        assert_eq!(lines.len(), 12, "{lines:?}");
        for name in names {
            for ord in ["first", "second"] {
                let want = format!("▪ {name} {ord}");
                assert_eq!(
                    lines.iter().filter(|l| **l == want).count(),
                    1,
                    "{want} in {lines:?}"
                );
            }
        }
    }

    #[test]
    fn least_recently_active_tail_is_evicted_first() {
        let dir = tempfile::tempdir().unwrap();
        let paths = seed(dir.path(), &["p0", "p1", "p2", "p3", "p4"]);
        let mut m = mux(4);
        for p in &paths[..4] {
            m.observe(p.clone(), None);
        }
        m.drain();
        assert_eq!(m.open.len(), 4);
        // p0 is written to; p1..p3 go quiet. A fifth unit then appears.
        append_event(&paths[0], &msg("p0 third"));
        m.drain();
        m.observe(paths[4].clone(), None);
        m.drain();
        assert_eq!(m.open.len(), 4);
        assert!(m.open.contains_key(&paths[0]), "active p0 survives");
        assert!(m.open.contains_key(&paths[4]), "new p4 is opened");
        // p1 went quiet first -> it is the one dropped.
        assert!(!m.open.contains_key(&paths[1]), "LRU p1 evicted");
        assert!(m.open.contains_key(&paths[2]));
        assert!(m.open.contains_key(&paths[3]));
        // The new unit's rows made it into the firehose.
        let lines = plain_lines(&m.firehose_lines());
        assert!(lines.contains(&"▪ p4 first".to_string()), "{lines:?}");
    }

    #[test]
    fn evicted_tail_reopens_when_it_gets_new_output_without_duplicating() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["a", "b", "c", "d", "e", "f"];
        let paths = seed(dir.path(), &names);
        let mut m = mux(1);
        for p in &paths {
            m.observe(p.clone(), None);
        }
        for _ in 0..6 {
            m.drain();
        }
        let before = m.firehose_lines().len();
        assert_eq!(before, 12);
        // Every unit speaks once more; the budget forces rotation again.
        for (p, name) in paths.iter().zip(names) {
            append_event(p, &msg(&format!("{name} third")));
        }
        for _ in 0..6 {
            m.drain();
            assert!(m.open.len() <= 4);
        }
        let lines = plain_lines(&m.firehose_lines());
        assert_eq!(lines.len(), 18, "{lines:?}");
        for name in names {
            let want = format!("▪ {name} third");
            assert_eq!(lines.iter().filter(|l| **l == want).count(), 1, "{want}");
            let old = format!("▪ {name} first");
            assert_eq!(lines.iter().filter(|l| **l == old).count(), 1, "{old}");
        }
    }

    #[test]
    fn pinned_tail_is_never_evicted_and_pinned_lines_are_only_its_stream() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["a", "b", "c", "d", "e", "f"];
        let paths = seed(dir.path(), &names);
        let mut m = mux(1);
        for (p, name) in paths.iter().zip(names) {
            m.observe(p.clone(), Some(format!("{name}#1")));
        }
        assert!(m.pinned_lines().is_empty(), "nothing pinned yet");
        // Pin the oldest-registered (so LRU would normally drop it first).
        m.pin(Some(paths[0].clone()));
        for _ in 0..6 {
            for p in &paths[1..] {
                append_event(p, &msg("noise"));
            }
            m.drain();
            assert!(m.open.len() <= 4, "open {}", m.open.len());
            assert!(m.open.contains_key(&paths[0]), "pinned stays open");
        }
        assert_eq!(
            plain_lines(&m.pinned_lines()),
            vec!["a#1 ▪ a first".to_string(), "a#1 ▪ a second".into()]
        );
        // The firehose still carries the other units.
        let all = plain_lines(&m.firehose_lines());
        assert!(all.iter().any(|l| l.starts_with("b#1")), "{all:?}");

        m.pin(None);
        assert!(m.pinned_lines().is_empty());
    }

    #[test]
    fn pinning_an_unobserved_path_still_tails_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = seed(dir.path(), &["a"]);
        let mut m = mux(8);
        m.pin(Some(paths[0].clone()));
        m.drain();
        assert_eq!(
            plain_lines(&m.pinned_lines()),
            vec!["▪ a first".to_string(), "▪ a second".into()]
        );
    }

    #[test]
    fn missing_transcripts_are_swallowed_and_picked_up_once_they_appear() {
        let dir = tempfile::tempdir().unwrap();
        let ghost = dir.path().join("not-yet.jsonl");
        let ghost_pinned = dir.path().join("pinned-not-yet.jsonl");
        let mut m = mux(8);
        m.observe(ghost.clone(), Some("otter#1".into()));
        m.pin(Some(ghost_pinned.clone()));
        m.drain();
        m.drain();
        assert!(m.firehose_lines().is_empty());
        assert!(m.pinned_lines().is_empty());

        write_events(&ghost, &[msg("hello")]);
        write_events(&ghost_pinned, &[msg("pinned hello")]);
        m.drain();
        // Both land in the firehose (order within one tick is unspecified).
        let mut all = plain_lines(&m.firehose_lines());
        all.sort();
        assert_eq!(
            all,
            vec!["otter#1 ▪ hello".to_string(), "▪ pinned hello".into()]
        );
        assert_eq!(
            plain_lines(&m.pinned_lines()),
            vec!["▪ pinned hello".to_string()]
        );
    }

    #[test]
    fn fd_pressure_skips_opening_unpinned_tails_but_never_the_pinned_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = seed(dir.path(), &["a", "b"]);
        let mut m = TranscriptMux::new(8);
        m.fd_probe = || Some((250, 256));
        m.observe(paths[0].clone(), None);
        m.pin(Some(paths[1].clone()));
        m.drain();
        assert!(m.open.contains_key(&paths[1]), "pinned always opens");
        assert!(!m.open.contains_key(&paths[0]), "no headroom for others");
        assert_eq!(
            plain_lines(&m.firehose_lines()),
            vec!["▪ b first".to_string(), "▪ b second".into()]
        );

        // Pressure eases: the held-back unit opens on the next tick.
        m.fd_probe = || Some((10, 256));
        m.drain();
        assert!(m.open.contains_key(&paths[0]));
        assert_eq!(m.firehose_lines().len(), 4);
    }

    #[test]
    fn ring_is_capped_oldest_dropped_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.jsonl");
        let events: Vec<Event> = (0..600).map(|i| msg(&format!("m{i}"))).collect();
        write_events(&path, &events);
        let mut m = mux(8);
        m.pin(Some(path.clone()));
        m.drain();
        let lines = plain_lines(&m.firehose_lines());
        assert_eq!(lines.len(), 500);
        assert_eq!(lines[0], "▪ m100");
        assert_eq!(lines[499], "▪ m599");
        assert_eq!(m.pinned_lines().len(), 500);
    }

    #[test]
    fn pinned_stream_stays_complete_while_chatty_siblings_flood_the_ring() {
        let dir = tempfile::tempdir().unwrap();
        let quiet = dir.path().join("quiet.jsonl");
        write_events(&quiet, &[msg("q one"), msg("q two"), msg("q three")]);
        let chatty = dir.path().join("chatty.jsonl");
        let flood: Vec<Event> = (0..650).map(|i| msg(&format!("s{i}"))).collect();
        write_events(&chatty, &flood);

        let mut m = mux(8);
        m.observe(quiet.clone(), Some("q#1".into()));
        m.observe(chatty.clone(), Some("s#1".into()));
        m.pin(Some(quiet.clone()));
        m.drain();
        // More sibling chatter over later ticks; the pinned unit stays quiet.
        for tick in 0..3 {
            for i in 0..100 {
                append_event(&chatty, &msg(&format!("late{tick}-{i}")));
            }
            m.pin(Some(quiet.clone())); // re-pinning each tick must be harmless
            m.drain();
        }

        // The pinned stream still shows the quiet unit's whole history...
        assert_eq!(
            plain_lines(&m.pinned_lines()),
            vec![
                "q#1 ▪ q one".to_string(),
                "q#1 ▪ q two".into(),
                "q#1 ▪ q three".into()
            ]
        );
        // ...even though the shared firehose has long scrolled it out and is
        // full of the siblings' chatter.
        let all = plain_lines(&m.firehose_lines());
        assert_eq!(all.len(), 500);
        assert!(all.iter().all(|l| !l.contains("q#1")), "{all:?}");
        assert_eq!(all[499], "s#1 ▪ late2-99");
    }

    #[test]
    fn re_pinning_the_same_path_does_not_reset_or_duplicate_the_stream() {
        let dir = tempfile::tempdir().unwrap();
        let paths = seed(dir.path(), &["a"]);
        let mut m = mux(8);
        m.pin(Some(paths[0].clone()));
        m.drain();
        for n in 0..3 {
            m.pin(Some(paths[0].clone()));
            append_event(&paths[0], &msg(&format!("a more{n}")));
            m.drain();
        }
        assert_eq!(
            plain_lines(&m.pinned_lines()),
            vec![
                "▪ a first".to_string(),
                "▪ a second".into(),
                "▪ a more0".into(),
                "▪ a more1".into(),
                "▪ a more2".into(),
            ]
        );
    }

    #[test]
    fn pinning_an_already_drained_unit_backfills_its_history_without_touching_the_ring() {
        let dir = tempfile::tempdir().unwrap();
        let paths = seed(dir.path(), &["a", "b"]);
        let mut m = mux(8);
        m.observe(paths[0].clone(), Some("a#1".into()));
        m.observe(paths[1].clone(), Some("b#1".into()));
        m.drain();
        let ring_before = plain_lines(&m.firehose_lines());
        assert_eq!(ring_before.len(), 4);
        assert!(m.pinned_lines().is_empty());

        // Drill into `a`: its history (already consumed into the ring) fills
        // the stream buffer; the ring must not see it a second time.
        m.pin(Some(paths[0].clone()));
        m.drain();
        assert_eq!(
            plain_lines(&m.pinned_lines()),
            vec!["a#1 ▪ a first".to_string(), "a#1 ▪ a second".into()]
        );
        assert_eq!(plain_lines(&m.firehose_lines()), ring_before);

        // Live output after the replay reaches both.
        append_event(&paths[0], &msg("a third"));
        m.drain();
        assert_eq!(m.pinned_lines().len(), 3);
        assert_eq!(m.firehose_lines().len(), 5);

        // Moving the pin swaps the stream; clearing it empties it.
        m.pin(Some(paths[1].clone()));
        m.drain();
        assert_eq!(
            plain_lines(&m.pinned_lines()),
            vec!["b#1 ▪ b first".to_string(), "b#1 ▪ b second".into()]
        );
        assert_eq!(m.firehose_lines().len(), 5, "no ring duplicates");
        m.pin(None);
        assert!(m.pinned_lines().is_empty());
    }
}
