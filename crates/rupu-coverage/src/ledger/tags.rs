//! Finding tags (spec `docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md`).
//!
//! A finding's tags are the ones it was declared with (`FindingRecord::tags`)
//! plus the add/remove events in its workspace's `finding_tags.jsonl`,
//! applied in file order.

use crate::ledger::events::{Attribution, FindingRecord};
use crate::ledger::stream::RunStream;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// At most this many effective tags on one finding.
pub const MAX_TAGS_PER_FINDING: usize = 32;

/// A normalized tag. Constructing one is the only validation path, so a
/// `Tag` in hand is always valid.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Tag(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tag `{raw}`: {reason}")]
pub struct TagParseError {
    pub raw: String,
    pub reason: &'static str,
}

impl Tag {
    pub const MAX_LEN: usize = 64;

    /// Trim and lowercase `raw`, then validate it. An invalid tag is an
    /// error naming it; it is never rewritten into a valid one.
    pub fn parse(raw: &str) -> Result<Self, TagParseError> {
        let err = |reason| TagParseError {
            raw: raw.to_string(),
            reason,
        };
        let t = raw.trim().to_lowercase();
        if t.is_empty() {
            return Err(err("empty"));
        }
        let allowed = |c: char| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | ':' | '/' | '-')
        };
        if !t.chars().all(allowed) {
            return Err(err("only a-z, 0-9 and . _ : / - are allowed"));
        }
        if !t.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err(err("must start with a letter or digit"));
        }
        // ASCII only from here on, so bytes == characters.
        if t.len() > Self::MAX_LEN {
            return Err(err("longer than 64 characters"));
        }
        Ok(Tag(t))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Tag {
    type Error = TagParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Tag::parse(&s)
    }
}

impl From<Tag> for String {
    fn from(t: Tag) -> String {
        t.0
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for Tag {
    type Err = TagParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Tag::parse(s)
    }
}

/// Validate every entry; the result is deduped and sorted. The first
/// invalid entry is the error.
pub fn parse_tags<S: AsRef<str>>(raw: &[S]) -> Result<Vec<Tag>, TagParseError> {
    let set: BTreeSet<Tag> = raw
        .iter()
        .map(|r| Tag::parse(r.as_ref()))
        .collect::<Result<_, _>>()?;
    Ok(set.into_iter().collect())
}

/// The `tags` property `report_finding` and `findings.record` advertise.
pub fn tags_schema_property() -> serde_json::Value {
    serde_json::json!({
        "type": "array",
        "items": { "type": "string" },
        "maxItems": MAX_TAGS_PER_FINDING,
        "description": "Free-form tags: lowercase a-z, 0-9 and . _ : / -, starting with a letter or digit, e.g. class:sqli, needs-poc. Reuse tags already in use where they fit (query_findings / findings.query list them)."
    })
}

/// The tag log's file name, at `<workspace>/.rupu/coverage/`.
pub const TAG_LOG_FILE: &str = "finding_tags.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagOp {
    Add,
    Remove,
}

/// Where an operator's change came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperatorSurface {
    Cli,
    Cp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorAttribution {
    pub user: String,
    pub via: OperatorSurface,
}

/// Who changed a tag: an agent (the same attribution `report_finding`
/// stamps) or a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TagActor {
    Agent(Attribution),
    Operator(OperatorAttribution),
}

impl TagActor {
    /// The person running this process (`$USER`, else `unknown`).
    pub fn operator(via: OperatorSurface) -> Self {
        let user = std::env::var("USER")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        TagActor::Operator(OperatorAttribution { user, via })
    }
}

/// One line of `finding_tags.jsonl`. Not `deny_unknown_fields`: a newer
/// writer's extra fields are ignored. A line with an op, actor kind or tag
/// this build cannot read is skipped by [`read_tag_events`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagEvent {
    pub id: String,
    pub finding_id: String,
    pub op: TagOp,
    pub tag: Tag,
    pub by: TagActor,
    pub at: DateTime<Utc>,
}

/// A workspace's tag log: one per workspace, shared by all its coverage
/// targets (a run stream names its writer's scope, never the target of the
/// finding it tags, so a per-target log could not route a remote event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagLog {
    pub workspace: PathBuf,
    pub path: PathBuf,
    /// Sidecar lock serializing every writer of `path`.
    pub lock: PathBuf,
    /// Where [`apply`]'s events are mirrored (`rupu run` only).
    pub run_stream: Option<RunStream>,
}

impl TagLog {
    pub fn for_workspace(workspace: &Path) -> Self {
        let dir = workspace.join(".rupu").join("coverage");
        Self {
            workspace: workspace.to_path_buf(),
            path: dir.join(TAG_LOG_FILE),
            lock: dir.join(format!("{TAG_LOG_FILE}.lock")),
            run_stream: None,
        }
    }

    pub fn with_run_stream(mut self, run_stream: Option<RunStream>) -> Self {
        self.run_stream = run_stream;
        self
    }
}

/// Every readable event, in file order. A missing log is no events; a line
/// that is not an event this build can read (torn, hand-edited, or from a
/// newer writer) is skipped with one warning, never an error.
pub fn read_tag_events(log: &TagLog) -> std::io::Result<Vec<TagEvent>> {
    let raw = match std::fs::read(&log.path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut skipped = 0usize;
    let events: Vec<TagEvent> = raw
        .split(|b| *b == b'\n')
        .filter_map(|l| std::str::from_utf8(l).ok())
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str::<TagEvent>(l) {
            Ok(e) => Some(e),
            Err(_) => {
                skipped += 1;
                None
            }
        })
        .collect();
    if skipped > 0 {
        tracing::warn!(
            path = %log.path.display(),
            skipped,
            "skipped finding-tag log lines this rupu version cannot read"
        );
    }
    Ok(events)
}

/// Fold `events` onto each record's declared tags, in file order: `add`
/// inserts, `remove` deletes, so the last event for a tag wins. An event for
/// a finding not in `records` is ignored here (it applies once that finding
/// is read alongside it).
pub fn fold_tags(records: &mut [FindingRecord], events: &[TagEvent]) {
    if events.is_empty() {
        return;
    }
    let mut sets: Vec<BTreeSet<Tag>> = records
        .iter()
        .map(|r| r.tags.iter().cloned().collect())
        .collect();
    {
        let mut by_id: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, r) in records.iter().enumerate() {
            by_id.entry(r.id.as_str()).or_default().push(i);
        }
        for e in events {
            for &i in by_id.get(e.finding_id.as_str()).into_iter().flatten() {
                match e.op {
                    TagOp::Add => {
                        sets[i].insert(e.tag.clone());
                    }
                    TagOp::Remove => {
                        sets[i].remove(&e.tag);
                    }
                }
            }
        }
    }
    for (r, s) in records.iter_mut().zip(sets) {
        r.tags = s.into_iter().collect();
    }
}

/// One finding's events, in file order.
pub fn tag_history(log: &TagLog, finding_id: &str) -> std::io::Result<Vec<TagEvent>> {
    Ok(read_tag_events(log)?
        .into_iter()
        .filter(|e| e.finding_id == finding_id)
        .collect())
}

/// Every coverage target's findings in `workspace`, as declared (no events
/// folded).
pub fn read_declared_workspace_findings(workspace: &Path) -> std::io::Result<Vec<FindingRecord>> {
    let mut out = Vec::new();
    for t in crate::ledger::discover::discover_targets(workspace)? {
        let paths = crate::ledger::paths::CoveragePaths::new(workspace, &t.target_id);
        out.extend(crate::ledger::views::read_declared_findings(&paths)?);
    }
    Ok(out)
}

/// Every coverage target's findings in `workspace`, with effective tags.
pub fn read_workspace_findings(workspace: &Path) -> std::io::Result<Vec<FindingRecord>> {
    let mut records = read_declared_workspace_findings(workspace)?;
    fold_tags(
        &mut records,
        &read_tag_events(&TagLog::for_workspace(workspace))?,
    );
    Ok(records)
}

/// One requested change: add and remove tags on a set of findings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagChange {
    pub finding_ids: Vec<String>,
    pub add: Vec<Tag>,
    pub remove: Vec<Tag>,
}

/// `tag_findings` / `findings.tag` input. Unknown fields are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagChangeInput {
    pub finding_ids: Vec<String>,
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

impl TagChangeInput {
    pub fn into_change(self) -> Result<TagChange, TagParseError> {
        Ok(TagChange {
            finding_ids: self.finding_ids,
            add: parse_tags(&self.add)?,
            remove: parse_tags(&self.remove)?,
        })
    }
}

/// Input schema shared by `tag_findings` and `findings.tag`.
pub fn tag_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["finding_ids"],
        "additionalProperties": false,
        "properties": {
            "finding_ids": { "type": "array", "items": { "type": "string" }, "minItems": 1, "description": "Finding ids (fnd_…) from query_findings or report_finding." },
            "add": { "type": "array", "items": { "type": "string" }, "description": "Tags to add." },
            "remove": { "type": "array", "items": { "type": "string" }, "description": "Tags to remove." }
        }
    })
}

impl TagChange {
    /// The checks that need no I/O: something to change, some findings,
    /// and no tag both added and removed.
    pub fn check(&self) -> Result<(), TagError> {
        if self.add.is_empty() && self.remove.is_empty() {
            return Err(TagError::EmptyChange);
        }
        if self.finding_ids.iter().all(|id| id.trim().is_empty()) {
            return Err(TagError::NoFindings);
        }
        let add: BTreeSet<&Tag> = self.add.iter().collect();
        let conflict: Vec<Tag> = self
            .remove
            .iter()
            .filter(|t| add.contains(t))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !conflict.is_empty() {
            return Err(TagError::Conflict(conflict));
        }
        Ok(())
    }
}

/// One finding's tags before and after a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagOutcome {
    pub finding_id: String,
    pub before: Vec<Tag>,
    pub after: Vec<Tag>,
}

impl TagOutcome {
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("nothing to change: give at least one tag to add or remove")]
    EmptyChange,
    #[error("no finding ids given")]
    NoFindings,
    #[error("a tag cannot be both added and removed: {}", .0.iter().map(Tag::as_str).collect::<Vec<_>>().join(", "))]
    Conflict(Vec<Tag>),
    #[error("unknown finding id(s): {}", .0.join(", "))]
    UnknownFindings(Vec<String>),
    #[error("{finding_id} would have {count} tags; at most {MAX_TAGS_PER_FINDING} are allowed")]
    TooManyTags { finding_id: String, count: usize },
    #[error("finding-tag log I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("finding-tag log encode: {0}")]
    Encode(#[from] serde_json::Error),
}

/// Apply `change` to the findings of `log`'s workspace. The ONLY writer of
/// tag events (spec "Write path").
///
/// Under the log's sidecar lock: read every target's declared findings and
/// the log, fold, refuse the whole batch if any id is unknown or the change
/// would take a finding past [`MAX_TAGS_PER_FINDING`] (a finding already
/// over it, say after two remote units' events were ingested, can still be
/// changed so long as its tag count does not grow), then append only the
/// events that change something (so a repeated request writes nothing) in
/// one write, fsynced, and mirror them to the log's run stream. Findings are read
/// without their own lock: their ledger is append-only and an import swaps
/// it by atomic rename, so a read always sees a whole file and an id never
/// disappears. Outcomes are sorted by finding id.
pub fn apply(log: &TagLog, change: &TagChange, by: &TagActor) -> Result<Vec<TagOutcome>, TagError> {
    change.check()?;
    let ids: Vec<String> = change
        .finding_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let add: BTreeSet<Tag> = change.add.iter().cloned().collect();
    let remove: BTreeSet<Tag> = change.remove.iter().cloned().collect();
    // No coverage directory: no findings here, and nothing is created.
    if !log.path.parent().is_some_and(Path::is_dir) {
        return Err(TagError::UnknownFindings(ids));
    }
    crate::ledger::stream::locked_at(&log.lock, &log.path, std::fs::File::lock, || {
        let mut records = read_declared_workspace_findings(&log.workspace)?;
        fold_tags(&mut records, &read_tag_events(log)?);
        let current: HashMap<&str, BTreeSet<Tag>> = records
            .iter()
            .map(|r| (r.id.as_str(), r.tags.iter().cloned().collect()))
            .collect();
        let unknown: Vec<String> = ids
            .iter()
            .filter(|id| !current.contains_key(id.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(TagError::UnknownFindings(unknown));
        }
        let at = Utc::now();
        let mut events = Vec::new();
        let mut outcomes = Vec::with_capacity(ids.len());
        for id in &ids {
            let before = current[id.as_str()].clone();
            let mut after = before.clone();
            for t in &remove {
                if after.remove(t) {
                    events.push(new_event(id, TagOp::Remove, t, by, at));
                }
            }
            for t in &add {
                if after.insert(t.clone()) {
                    events.push(new_event(id, TagOp::Add, t, by, at));
                }
            }
            if after.len() > MAX_TAGS_PER_FINDING && after.len() > before.len() {
                return Err(TagError::TooManyTags {
                    finding_id: id.clone(),
                    count: after.len(),
                });
            }
            outcomes.push(TagOutcome {
                finding_id: id.clone(),
                before: before.into_iter().collect(),
                after: after.into_iter().collect(),
            });
        }
        append_events(log, &events)?;
        Ok(outcomes)
    })
}

fn new_event(finding_id: &str, op: TagOp, tag: &Tag, by: &TagActor, at: DateTime<Utc>) -> TagEvent {
    TagEvent {
        id: format!("tge_{}", ulid::Ulid::new()),
        finding_id: finding_id.to_string(),
        op,
        tag: tag.clone(),
        by: by.clone(),
        at,
    }
}

/// Append `events` in ONE write (a concurrent unlocked appender could
/// otherwise land between two of them), fsync, then mirror each to the run
/// stream. The caller holds the log's lock.
fn append_events(log: &TagLog, events: &[TagEvent]) -> Result<(), TagError> {
    use std::io::Write;
    if events.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = events
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<_, _>>()?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&log.path)?;
    // A crash can leave the log ending mid-line. Start on a fresh line so
    // the torn one stays a skipped line instead of swallowing our first event.
    let mut buf = String::new();
    if ends_mid_line(&mut f)? {
        buf.push('\n');
    }
    buf.push_str(&lines.join("\n"));
    buf.push('\n');
    f.write_all(buf.as_bytes())?;
    f.sync_data()?;
    for l in &lines {
        crate::ledger::stream::stream_json_to(
            log.run_stream.as_ref(),
            crate::ledger::stream::Ledger::Tags,
            l,
        );
    }
    Ok(())
}

/// True when the file is non-empty and its last byte is not `\n`.
fn ends_mid_line(f: &mut std::fs::File) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    if f.metadata()?.len() == 0 {
        return Ok(false);
    }
    f.seek(SeekFrom::End(-1))?;
    let mut last = [0u8; 1];
    f.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

/// Append a remote unit's events (`ingest_unit_stream`) that `log` does not
/// already hold, by event id, under the log's lock so two ingests cannot
/// both append one event. Never validated against findings: an orphan is
/// folded away until its finding arrives. Returns (appended, duplicates).
pub fn ingest_tag_events(log: &TagLog, events: Vec<TagEvent>) -> Result<(usize, usize), TagError> {
    if events.is_empty() {
        return Ok((0, 0));
    }
    crate::ledger::stream::locked_at(&log.lock, &log.path, std::fs::File::lock, || {
        let mut seen: std::collections::HashSet<String> =
            read_tag_events(log)?.into_iter().map(|e| e.id).collect();
        let total = events.len();
        let fresh: Vec<TagEvent> = events
            .into_iter()
            .filter(|e| seen.insert(e.id.clone()))
            .collect();
        let appended = fresh.len();
        append_events(log, &fresh)?;
        Ok((appended, total - appended))
    })
}

/// What [`merge_tag_log_copy`] did with another copy's events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TagLogMerge {
    /// Events `log` did not hold, now appended.
    pub appended: usize,
    /// Events `log` already held (by id).
    pub duplicates: usize,
    /// Lines this rupu version could not read; not merged.
    pub unreadable: usize,
}

/// Merge `copy` — the bytes of another copy of a workspace's tag log, such as
/// the one a remote unit's workspace delta carries — into `log` by event id
/// ([`ingest_tag_events`]). The log is append-only and every event has a
/// unique id, so this union is exact, and unlike writing `copy` over the file
/// it keeps the events `log` gained meanwhile.
pub fn merge_tag_log_copy(log: &TagLog, copy: &[u8]) -> Result<TagLogMerge, TagError> {
    let mut unreadable = 0usize;
    let events: Vec<TagEvent> = copy
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        .filter_map(|l| {
            match std::str::from_utf8(l)
                .ok()
                .and_then(|l| serde_json::from_str::<TagEvent>(l).ok())
            {
                Some(e) => Some(e),
                None => {
                    unreadable += 1;
                    None
                }
            }
        })
        .collect();
    let (appended, duplicates) = ingest_tag_events(log, events)?;
    Ok(TagLogMerge {
        appended,
        duplicates,
        unreadable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_is_trimmed_and_lowercased() {
        assert_eq!(Tag::parse("  Needs-POC ").unwrap().as_str(), "needs-poc");
        assert_eq!(Tag::parse("class:SQLi").unwrap().as_str(), "class:sqli");
        assert_eq!(
            Tag::parse("team/payments_v2.1").unwrap().as_str(),
            "team/payments_v2.1"
        );
    }

    #[test]
    fn an_invalid_tag_is_rejected_never_rewritten() {
        for (raw, why) in [
            ("", "empty"),
            ("   ", "empty"),
            ("needs poc", "only a-z"),
            ("na\u{ef}ve", "only a-z"),
            ("-leading", "must start"),
            (":leading", "must start"),
        ] {
            let e = Tag::parse(raw).unwrap_err();
            assert!(e.to_string().contains(why), "{raw:?}: {e}");
        }
        assert!(Tag::parse(&"a".repeat(64)).is_ok());
        assert!(Tag::parse(&"a".repeat(65))
            .unwrap_err()
            .to_string()
            .contains("longer than 64"));
    }

    #[test]
    fn parse_tags_dedupes_and_sorts() {
        let tags = parse_tags(&["b", "A", "a", "b"]).unwrap();
        let got: Vec<&str> = tags.iter().map(Tag::as_str).collect();
        assert_eq!(got, ["a", "b"]);
        assert!(parse_tags(&["ok", "not ok"]).is_err());
    }

    #[test]
    fn a_tag_serializes_as_a_plain_string_and_validates_on_read() {
        let t = Tag::parse("needs-poc").unwrap();
        assert_eq!(serde_json::to_string(&t).unwrap(), "\"needs-poc\"");
        assert!(serde_json::from_str::<Tag>("\"bad tag\"").is_err());
    }

    use crate::ledger::events::{
        Attribution, FindingEvidence, FindingRecord, FindingScope, Surface,
    };
    use crate::ledger::paths::CoveragePaths;
    use crate::ledger::stream::{append_record, Ledger};
    use chrono::Utc;

    pub(crate) fn attribution() -> Attribution {
        Attribution {
            run_id: "run_t".into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: Some("tagger".into()),
            provider: None,
        }
    }

    pub(crate) fn finding(id: &str, tags: &[&str]) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: Some("src/lib.rs".into()),
            line_range: Some([1, 2]),
            target_ref: None,
            scope: FindingScope::Line,
            summary: format!("summary of {id}"),
            severity: crate::catalog::types::Severity::High,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
            profile: crate::report::FindingProfile::Summary,
            report: None,
            tags: parse_tags(tags).unwrap(),
        }
    }

    fn ev(finding_id: &str, op: TagOp, tag: &str) -> TagEvent {
        TagEvent {
            id: format!("tge_{}", ulid::Ulid::new()),
            finding_id: finding_id.into(),
            op,
            tag: Tag::parse(tag).unwrap(),
            by: TagActor::Agent(attribution()),
            at: Utc::now(),
        }
    }

    fn write_events(log: &TagLog, events: &[TagEvent]) {
        std::fs::create_dir_all(log.path.parent().unwrap()).unwrap();
        let text: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&log.path, text).unwrap();
    }

    fn names(r: &FindingRecord) -> Vec<&str> {
        r.tags.iter().map(Tag::as_str).collect()
    }

    #[test]
    fn fold_applies_events_in_file_order_last_one_wins() {
        let mut recs = vec![finding("fnd_a", &["keep", "drop"]), finding("fnd_b", &[])];
        fold_tags(
            &mut recs,
            &[
                ev("fnd_a", TagOp::Remove, "drop"),
                ev("fnd_a", TagOp::Add, "x"),
                ev("fnd_a", TagOp::Remove, "x"),
                ev("fnd_a", TagOp::Add, "x"),
                ev("fnd_b", TagOp::Add, "y"),
                ev("fnd_unknown", TagOp::Add, "z"),
            ],
        );
        assert_eq!(names(&recs[0]), ["keep", "x"]);
        assert_eq!(names(&recs[1]), ["y"]);
    }

    #[test]
    fn an_orphan_event_applies_once_its_finding_arrives() {
        let events = [ev("fnd_late", TagOp::Add, "triaged")];
        let mut none: Vec<FindingRecord> = vec![];
        fold_tags(&mut none, &events);
        let mut later = vec![finding("fnd_late", &[])];
        fold_tags(&mut later, &events);
        assert_eq!(names(&later[0]), ["triaged"]);
    }

    #[test]
    fn actors_serialize_with_a_kind() {
        let agent = serde_json::to_value(TagActor::Agent(attribution())).unwrap();
        assert_eq!(agent["kind"], "agent");
        assert_eq!(agent["run_id"], "run_t");
        let op = serde_json::to_value(TagActor::Operator(OperatorAttribution {
            user: "alice".into(),
            via: OperatorSurface::Cli,
        }))
        .unwrap();
        assert_eq!(
            op,
            serde_json::json!({"kind":"operator","user":"alice","via":"cli"})
        );
    }

    #[test]
    fn unreadable_log_lines_are_skipped_not_fatal() {
        let ws = tempfile::TempDir::new().unwrap();
        let log = TagLog::for_workspace(ws.path());
        let good = ev("fnd_a", TagOp::Add, "ok");
        write_events(&log, std::slice::from_ref(&good));
        let mut text = std::fs::read_to_string(&log.path).unwrap();
        text.push_str("not json\n");
        text.push_str(
            &serde_json::to_string(&good)
                .unwrap()
                .replace("\"add\"", "\"rename\""),
        );
        text.push_str("\n{\"id\":\"tge_x\",\"finding_id\":\"fnd_a\",\"op\":\"add\",\"tag\":\"BAD TAG\",\"by\":{\"kind\":\"operator\",\"user\":\"u\",\"via\":\"cli\"},\"at\":\"2026-10-06T00:00:00Z\"}\n");
        std::fs::write(&log.path, text).unwrap();
        assert_eq!(read_tag_events(&log).unwrap(), vec![good]);
    }

    #[test]
    fn a_missing_log_reads_as_no_events() {
        let ws = tempfile::TempDir::new().unwrap();
        assert!(read_tag_events(&TagLog::for_workspace(ws.path()))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn read_findings_folds_the_workspace_log_across_targets() {
        let ws = tempfile::TempDir::new().unwrap();
        let t1 = CoveragePaths::new(ws.path(), "t1");
        let t2 = CoveragePaths::new(ws.path(), "t2");
        append_record(&t1, Ledger::Findings, &finding("fnd_a", &["declared"])).unwrap();
        append_record(&t2, Ledger::Findings, &finding("fnd_b", &[])).unwrap();
        write_events(
            &TagLog::for_workspace(ws.path()),
            &[ev("fnd_a", TagOp::Add, "x"), ev("fnd_b", TagOp::Add, "y")],
        );

        let a = crate::ledger::views::read_findings(&t1).unwrap();
        assert_eq!(names(&a[0]), ["declared", "x"]);
        let declared = crate::ledger::views::read_declared_findings(&t1).unwrap();
        assert_eq!(names(&declared[0]), ["declared"]);

        let all = read_workspace_findings(ws.path()).unwrap();
        assert_eq!(all.len(), 2);
        let b = all.iter().find(|r| r.id == "fnd_b").unwrap();
        assert_eq!(names(b), ["y"]);
        // The log file sits beside the target dirs and is not one.
        let targets = crate::ledger::discover::discover_targets(ws.path()).unwrap();
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn history_is_one_findings_events_in_file_order() {
        let ws = tempfile::TempDir::new().unwrap();
        let log = TagLog::for_workspace(ws.path());
        let events = [
            ev("fnd_a", TagOp::Add, "x"),
            ev("fnd_b", TagOp::Add, "y"),
            ev("fnd_a", TagOp::Remove, "x"),
        ];
        write_events(&log, &events);
        let h = tag_history(&log, "fnd_a").unwrap();
        assert_eq!(h, vec![events[0].clone(), events[2].clone()]);
    }
}
