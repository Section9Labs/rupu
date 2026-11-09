//! Bounded, local-artifact observations for one root session turn.
//! No operator router, token, remote connector, catalog scan or write path.
use rupu_tools::{
    ports::{PortError, SessionReadPort, SessionReadRequest},
    Surface, ToolContext,
};
use rustix::{
    fd::OwnedFd,
    fs::{fstat, open, openat, FileType, Mode, OFlags},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINE: usize = 256 * 1024;
const MAX_RECORDS: usize = 1024;
const MAX_LINKS: usize = 1000;
const MAX_OUTPUT: usize = 128 * 1024;
fn error(message: &'static str) -> PortError {
    PortError::new(message)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn text(value: &str) -> bool {
    value.len() <= 256 && !value.chars().any(char::is_control)
}

// serde_json::Value normally accepts duplicate keys. Ownership records must
// not silently choose the last of two conflicting run/workspace identities.
fn parse_record(bytes: &[u8]) -> Result<Value, PortError> {
    use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
    struct Unique(Value);
    impl<'de> Deserialize<'de> for Unique {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Unique;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a JSON value without duplicate keys")
                }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Unique, E> {
                    Ok(Unique(json!(v)))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Unique, E> {
                    Ok(Unique(json!(v)))
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Unique, E> {
                    Ok(Unique(json!(v)))
                }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Unique, E> {
                    Ok(Unique(json!(v)))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Unique, E> {
                    Ok(Unique(json!(v)))
                }
                fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Unique, E> {
                    Ok(Unique(Value::String(v)))
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Unique, E> {
                    Ok(Unique(Value::Null))
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Unique, A::Error> {
                    let mut rows = Vec::new();
                    while let Some(Unique(value)) = seq.next_element::<Unique>()? {
                        rows.push(value);
                    }
                    Ok(Unique(Value::Array(rows)))
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
                    let mut out = serde_json::Map::new();
                    while let Some((key, Unique(value))) = map.next_entry::<String, Unique>()? {
                        if out.insert(key, value).is_some() {
                            return Err(serde::de::Error::custom("duplicate record key"));
                        }
                    }
                    Ok(Unique(Value::Object(out)))
                }
            }
            d.deserialize_any(V)
        }
    }
    serde_json::from_slice::<Unique>(bytes)
        .map(|v| v.0)
        .map_err(|_| error("invalid recorded artifact"))
}

#[derive(Clone, Debug)]
pub struct Owner {
    pub session_id: String,
    pub run_id: String,
    pub workspace_id: String,
    pub workspace_path: PathBuf,
    pub project_root: Option<PathBuf>,
    pub transcript_path: PathBuf,
}
#[derive(Clone, Debug, Serialize)]
pub struct ContextSnapshot {
    /// Exact trusted bindings authorized by the creation version or an
    /// explicit current settings edit; never raw model-supplied patterns.
    pub allowed_reads: Vec<String>,
    pub permission_mode: String,
    pub declared_tools: Option<Vec<String>>,
    pub selected_skills: Vec<String>,
    pub preloaded_skills: Vec<String>,
    pub settings_revision: u64,
    pub orchestration_revision: u64,
    pub builtin_tools_preset: Option<String>,
}
struct Root {
    path: PathBuf,
    fd: OwnedFd,
}
impl Root {
    fn new(path: &Path) -> Result<Self, PortError> {
        let path = path
            .canonicalize()
            .map_err(|_| error("session artifact root unavailable"))?;
        let fd = open(
            &path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| error("session artifact root unavailable"))?;
        Ok(Self { path, fd })
    }
    fn current(&self) -> Result<(), PortError> {
        let now = open(
            &self.path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| error("session artifact root changed"))?;
        let before = fstat(&self.fd).map_err(|_| error("session artifact root unavailable"))?;
        let after = fstat(&now).map_err(|_| error("session artifact root unavailable"))?;
        if (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino) {
            return Err(error("session artifact root changed"));
        }
        Ok(())
    }
    fn open(&self, path: &Path, directory: bool) -> Result<Option<OwnedFd>, PortError> {
        self.current()?;
        let relative = path
            .strip_prefix(&self.path)
            .map_err(|_| error("artifact is outside session roots"))?;
        let components: Vec<_> = relative.components().collect();
        if components.is_empty()
            || components.len() > 24
            || components
                .iter()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(error("invalid artifact path"));
        }
        let mut parent =
            rustix::io::dup(&self.fd).map_err(|_| error("artifact root unavailable"))?;
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(name) = component else {
                unreachable!()
            };
            let is_dir = index + 1 < components.len() || directory;
            let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
            if is_dir {
                flags |= OFlags::DIRECTORY;
            }
            parent = match openat(&parent, *name, flags, Mode::empty()) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::NOENT) => return Ok(None),
                Err(_) => return Err(error("unsafe or unavailable session artifact")),
            };
            let metadata = fstat(&parent).map_err(|_| error("artifact metadata unavailable"))?;
            let kind = FileType::from_raw_mode(metadata.st_mode);
            if (is_dir && kind != FileType::Directory)
                || (!is_dir && (kind != FileType::RegularFile || metadata.st_nlink != 1))
            {
                return Err(error("session artifact is not a private regular file"));
            }
        }
        self.current()?;
        Ok(Some(parent))
    }
}

pub struct SessionReads {
    global: Root,
    project: Option<Root>,
    workspace: Root,
    owner: Owner,
    snapshot: ContextSnapshot,
    netflow_root: PathBuf,
    /// Only aliases supplied by the trusted worker at construction. A
    /// recorded descendant cannot introduce a new filesystem alias.
    path_aliases: Vec<(PathBuf, PathBuf)>,
}
struct Budget {
    bytes: usize,
    records: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            bytes: MAX_BYTES,
            records: MAX_RECORDS,
        }
    }
}
struct Lines {
    values: Vec<Value>,
    partial: bool,
    bytes: usize,
}
impl SessionReads {
    pub fn new(
        global: PathBuf,
        mut owner: Owner,
        snapshot: ContextSnapshot,
    ) -> Result<Self, PortError> {
        if !valid_id(&owner.session_id)
            || !owner.session_id.starts_with("ses_")
            || !valid_id(&owner.run_id)
            || !valid_id(&owner.workspace_id)
        {
            return Err(error("invalid session read owner"));
        }
        let original_global = global.clone();
        let original_workspace = owner.workspace_path.clone();
        let original_project = owner.project_root.clone();
        let global = Root::new(&global)?;
        let workspace = Root::new(&owner.workspace_path)?;
        owner.workspace_path = workspace.path.clone();
        let project = owner.project_root.as_deref().map(Root::new).transpose()?;
        owner.project_root = project.as_ref().map(|r| r.path.clone());
        if !owner.transcript_path.is_absolute() {
            return Err(error("invalid session transcript path"));
        }
        // No transcript existence requirement: construction precedes RunStart.
        let netflow_root =
            rupu_netflow::ledger::netflow_dir(&global.path, owner.project_root.as_deref());
        let mut path_aliases = vec![
            (original_global, global.path.clone()),
            (global.path.clone(), global.path.clone()),
            (original_workspace, workspace.path.clone()),
            (workspace.path.clone(), workspace.path.clone()),
        ];
        if let (Some(original), Some(root)) = (original_project, &project) {
            path_aliases.push((original, root.path.clone()));
            path_aliases.push((root.path.clone(), root.path.clone()));
        }
        path_aliases.sort_by_key(|(from, _)| std::cmp::Reverse(from.components().count()));
        let mut result = Self {
            global,
            project,
            workspace,
            owner,
            snapshot,
            netflow_root,
            path_aliases,
        };
        result.owner.transcript_path =
            result.normalize_recorded_path(&result.owner.transcript_path)?;
        result.root(&result.owner.transcript_path)?;
        let allowed: BTreeSet<_> = result.snapshot.allowed_reads.iter().collect();
        if allowed.len() != result.snapshot.allowed_reads.len()
            || allowed.len() > 4
            || allowed.iter().any(|name| {
                !matches!(
                    name.as_str(),
                    "session.context" | "session.workflows" | "run.inspect" | "run.netflows"
                )
            })
        {
            return Err(error("invalid exact session reader authorization"));
        }
        if result
            .snapshot
            .declared_tools
            .as_ref()
            .is_some_and(|v| v.len() > 512 || v.iter().any(|s| !text(s)))
            || result.snapshot.selected_skills.len() > 256
            || result.snapshot.preloaded_skills.len() > 256
            || result
                .snapshot
                .selected_skills
                .iter()
                .chain(&result.snapshot.preloaded_skills)
                .any(|s| !text(s))
            || !text(&result.snapshot.permission_mode)
            || result
                .snapshot
                .builtin_tools_preset
                .as_ref()
                .is_some_and(|s| !text(s))
        {
            return Err(error("session context exceeds bounded metadata"));
        }
        Ok(result)
    }
    fn normalize_recorded_path(&self, path: &Path) -> Result<PathBuf, PortError> {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(error("invalid recorded artifact path"));
        }
        for (original, pinned) in &self.path_aliases {
            if let Ok(relative) = path.strip_prefix(original) {
                if relative
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
                {
                    return Ok(pinned.join(relative));
                }
                return Err(error("invalid recorded artifact suffix"));
            }
        }
        Err(error("recorded artifact is outside trusted roots"))
    }
    fn root(&self, path: &Path) -> Result<&Root, PortError> {
        // Restrict trusted input paths to artifact subtrees, never arbitrary
        // source files under the project. Every open walks with NOFOLLOW.
        if path.starts_with(self.global.path.join("runs"))
            || path.starts_with(self.global.path.join("transcripts"))
            || path.starts_with(self.global.path.join("netflow"))
            || path.starts_with(
                self.global
                    .path
                    .join("sessions")
                    .join(&self.owner.session_id),
            )
        {
            return Ok(&self.global);
        }
        for root in self.project.iter().chain(std::iter::once(&self.workspace)) {
            if path.starts_with(root.path.join(".rupu/transcripts"))
                || path.starts_with(root.path.join(".rupu/netflow"))
            {
                return Ok(root);
            }
        }
        Err(error("artifact is outside session artifact roots"))
    }
    fn file(&self, path: &Path) -> Result<Option<File>, PortError> {
        Ok(self.root(path)?.open(path, false)?.map(File::from))
    }
    fn lines(
        &self,
        path: &Path,
        budget: &mut Budget,
        first_only: bool,
    ) -> Result<Option<Lines>, PortError> {
        let Some(file) = self.file(path)? else {
            return Ok(None);
        };
        let limit = budget.bytes;
        let mut input = BufReader::new(file.take(limit as u64));
        let mut values = Vec::new();
        let mut bytes = 0;
        let mut partial = false;
        loop {
            if budget.records == 0 || bytes == limit {
                partial = true;
                break;
            }
            let mut line = Vec::new();
            let n = input
                .by_ref()
                .take((MAX_LINE + 1) as u64)
                .read_until(b'\n', &mut line)
                .map_err(|_| error("artifact read failed"))?;
            if n == 0 {
                break;
            }
            bytes += n;
            if line.len() > MAX_LINE || !line.ends_with(b"\n") {
                partial = true;
                break;
            }
            budget.records -= 1;
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let value = parse_record(&line)?;
            values.push(value);
            if first_only {
                break;
            }
        }
        budget.bytes = budget.bytes.saturating_sub(bytes);
        Ok(Some(Lines {
            values,
            partial,
            bytes,
        }))
    }
    fn prologue(&self, path: &Path, run: &str, budget: &mut Budget) -> Result<Value, PortError> {
        let rows = self
            .lines(path, budget, true)?
            .ok_or_else(|| error("recorded transcript is not available yet"))?;
        let value = rows
            .values
            .first()
            .ok_or_else(|| error("bounded transcript prologue is unavailable"))?;
        if value["type"] != "run_start"
            || value["data"]["run_id"] != run
            || value["data"]["workspace_id"] != self.owner.workspace_id
        {
            return Err(error("recorded run owner does not match this session"));
        }
        let d = &value["data"];
        for key in ["agent", "provider", "model", "started_at"] {
            if !d[key].as_str().is_some_and(text) {
                return Err(error("invalid run prologue metadata"));
            }
        }
        Ok(
            json!({"run_id":run,"workspace_id":self.owner.workspace_id,"agent":d["agent"],"provider":d["provider"],"model":d["model"],"started_at":d["started_at"]}),
        )
    }
    fn check(&self, ctx: &ToolContext, budget: &mut Budget) -> Result<(), PortError> {
        if ctx.identity.surface != Surface::Session
            || ctx.identity.parent.is_some()
            || ctx.identity.run_id != self.owner.run_id
            || ctx.workspace.id != self.owner.workspace_id
            // The worker may retain a trusted lexical workspace spelling
            // (for example /var versus /private/var on macOS). Resolve it on
            // every call so an alias retarget cannot keep its old authority.
            || ctx.workspace.path.canonicalize().ok().as_ref() != Some(&self.owner.workspace_path)
        {
            return Err(error("session reader owner mismatch"));
        }
        self.global.current()?;
        self.workspace.current()?;
        if let Some(root) = &self.project {
            root.current()?;
        }
        self.prologue(&self.owner.transcript_path, &self.owner.run_id, budget)?;
        Ok(())
    }
    fn target(
        &self,
        ancestry: &[String],
        budget: &mut Budget,
    ) -> Result<(String, PathBuf), PortError> {
        if ancestry.len() > 8 || ancestry.iter().any(|s| !valid_id(s)) {
            return Err(error("invalid descendant chain"));
        }
        let mut visited = BTreeSet::from([self.owner.run_id.clone()]);
        let mut parent = self.owner.run_id.clone();
        let mut transcript = self.owner.transcript_path.clone();
        for child in ancestry {
            if !visited.insert(child.clone()) {
                return Err(error("cyclic descendant chain"));
            }
            let ledger = self
                .global
                .path
                .join("runs")
                .join(&parent)
                .join("children.jsonl");
            let lines = self
                .lines(&ledger, budget, false)?
                .ok_or_else(|| error("descendant membership unavailable"))?;
            // Prefix-only membership could miss a conflicting later start.
            if lines.partial {
                return Err(error("descendant membership exceeds read bound"));
            }
            let candidates: Vec<_> = lines
                .values
                .iter()
                .filter(|v| v["t"] == "started" && v["run_id"] == *child)
                .collect();
            if candidates.len() != 1 {
                return Err(error("descendant membership is missing or ambiguous"));
            }
            let path = candidates[0]["transcript"]
                .as_str()
                .ok_or_else(|| error("this descendant has no recorded agent transcript"))?;
            if path.len() > 4096 {
                return Err(error("descendant transcript path exceeds bound"));
            }
            transcript = self.normalize_recorded_path(Path::new(path))?;
            self.prologue(&transcript, child, budget)?;
            parent = child.clone();
        }
        Ok((parent, transcript))
    }
    fn workflows(
        &self,
        offset: usize,
        limit: usize,
        budget: &mut Budget,
    ) -> Result<Value, PortError> {
        if offset > MAX_LINKS || !(1..=50).contains(&limit) {
            return Err(error("invalid workflow page"));
        }
        let path = self
            .global
            .path
            .join("sessions")
            .join(&self.owner.session_id)
            .join("workflow-runs");
        let Some(fd) = self.global.open(&path, true)? else {
            return Ok(
                json!({"scope":"this_session","rows":[],"next_offset":null,"partial":false}),
            );
        };
        let before = fstat(&fd).map_err(|_| error("workflow link directory unavailable"))?;
        let mut names = Vec::new();
        let mut partial = false;
        let directory =
            rustix::fs::Dir::read_from(&fd).map_err(|_| error("workflow links unavailable"))?;
        for (index, entry) in directory.enumerate() {
            if index == MAX_LINKS {
                partial = true;
                break;
            }
            let entry = entry.map_err(|_| error("workflow link unavailable"))?;
            if let Ok(name) = entry.file_name().to_str() {
                if name.ends_with(".json") {
                    names.push(name.to_owned());
                }
            }
        }
        let again = self
            .global
            .open(&path, true)?
            .ok_or_else(|| error("workflow links changed"))?;
        let after = fstat(&again).map_err(|_| error("workflow links unavailable"))?;
        if (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino) {
            return Err(error("workflow links changed"));
        }
        // Without a complete name set, sorting/paging would invent stable
        // page order. Refuse continuation and mark a bounded observation.
        names.sort();
        let mut rows = Vec::new();
        let mut omitted = 0usize;
        for name in names.iter().skip(offset).take(limit) {
            let p = path.join(name);
            let Some(file) = self.file(&p)? else {
                omitted += 1;
                continue;
            };
            let cap = budget.bytes.min(32 * 1024);
            let mut bytes = Vec::new();
            file.take(cap as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| error("workflow link read failed"))?;
            budget.bytes = budget.bytes.saturating_sub(bytes.len());
            if bytes.len() > cap {
                return Err(error("workflow link exceeds read bound"));
            }
            let stored: Value = parse_record(&bytes)?;
            let a = &stored["association"];
            let run = a["run_id"]
                .as_str()
                .filter(|s| valid_id(s))
                .ok_or_else(|| error("invalid workflow owner"))?;
            if a["session_id"] != self.owner.session_id
                || a["workspace_id"] != self.owner.workspace_id
                || a["host_id"] != "local"
                || p.file_stem().and_then(|s| s.to_str()) != Some(run)
            {
                return Err(error("workflow link owner mismatch"));
            }
            for key in ["workflow_name", "launched_at", "status"] {
                if !a[key].as_str().is_some_and(text) {
                    return Err(error("invalid workflow metadata"));
                }
            }
            rows.push(json!({"run_id":run,"workflow_name":a["workflow_name"],"launched_at":a["launched_at"],"recorded_status":a["status"]}));
        }
        let next =
            (!partial && offset.saturating_add(limit) < names.len()).then_some(offset + limit);
        Ok(
            json!({"scope":"this_session","rows":rows,"next_offset":next,"partial":partial||omitted>0,"omitted":omitted,"status_evidence":"stored_association_not_live","pagination":"live_directory_not_snapshot"}),
        )
    }
    fn inspect(&self, run: &str, path: &Path, budget: &mut Budget) -> Result<Value, PortError> {
        let prologue = self.prologue(path, run, budget)?;
        let lines = self
            .lines(path, budget, false)?
            .ok_or_else(|| error("transcript unavailable"))?;
        if lines.values.first().is_none_or(|row| {
            row["type"] != "run_start"
                || row["data"]["run_id"] != run
                || row["data"]["workspace_id"] != self.owner.workspace_id
        }) {
            return Err(error("recorded transcript owner changed"));
        }
        let mut counts = BTreeMap::<String, usize>::new();
        let mut terminal = None;
        for row in &lines.values {
            let kind = row["type"]
                .as_str()
                .filter(|s| text(s))
                .ok_or_else(|| error("invalid transcript event"))?;
            *counts.entry(kind.to_owned()).or_default() += 1;
            if kind == "run_start"
                && (row["data"]["run_id"] != run
                    || row["data"]["workspace_id"] != self.owner.workspace_id)
            {
                return Err(error("recorded transcript owner mismatch"));
            }
            if kind == "run_complete" {
                if row["data"]["run_id"] != run {
                    return Err(error("recorded completion owner mismatch"));
                }
                terminal = row["data"]["status"]
                    .as_str()
                    .filter(|s| matches!(*s, "ok" | "error" | "aborted"))
                    .map(str::to_owned);
            }
        }
        let ledger = self
            .global
            .path
            .join("runs")
            .join(run)
            .join("children.jsonl");
        let children = self.lines(&ledger, budget, false)?;
        let mut child_rows = Vec::new();
        let mut seen = BTreeSet::new();
        if let Some(lines) = &children {
            for value in &lines.values {
                if value["t"] == "started" {
                    let id = value["run_id"]
                        .as_str()
                        .filter(|s| valid_id(s))
                        .ok_or_else(|| error("invalid child identity"))?;
                    if seen.insert(id.to_owned()) && child_rows.len() < 50 {
                        child_rows.push(
                            json!({"run_id":id,"has_transcript":value["transcript"].is_string()}),
                        );
                    }
                }
            }
        }
        Ok(
            json!({"prologue":prologue,"event_counts":counts,"observed_terminal_status":terminal,"partial":lines.partial,"bytes_read":lines.bytes,"children":child_rows,"children_partial":children.as_ref().is_some_and(|l|l.partial)||seen.len()>50,"evidence":"bounded_recorded_prefix_not_live_status"}),
        )
    }
    fn netflows(&self, run: &str, budget: &mut Budget) -> Result<Value, PortError> {
        let path = self.netflow_root.join(format!("{run}.jsonl"));
        let Some(lines) = self.lines(&path, budget, false)? else {
            return Ok(json!({"run_id":run,"state":"unavailable","rows":[],"partial":true}));
        };
        let mut groups = BTreeMap::<(String, u64, String, String), usize>::new();
        let mut dropped = 0u64;
        let mut capture = 0usize;
        let mut unknown = 0usize;
        let mut legacy_owner_rows = 0usize;
        let mut annotated_owner_rows = 0usize;
        for row in &lines.values {
            match row["type"].as_str() {
                Some("flow") => {
                    // Flow is a serde internally tagged newtype: fields are
                    // at the envelope root, not under a synthetic `data` key.
                    let c = &row["ctx"];
                    if !c.is_object()
                        || ["run_id", "workspace_id"]
                            .iter()
                            .any(|key| !c[key].is_null() && !c[key].is_string())
                    {
                        return Err(error("invalid netflow owner metadata"));
                    }
                    if c["run_id"].as_str().is_some_and(|s| s != run)
                        || c["workspace_id"]
                            .as_str()
                            .is_some_and(|s| s != self.owner.workspace_id)
                    {
                        return Err(error("netflow record owner mismatch"));
                    }
                    if c["run_id"].is_string() && c["workspace_id"].is_string() {
                        annotated_owner_rows += 1;
                    } else {
                        legacy_owner_rows += 1;
                    }
                    let host = row["host"]
                        .as_str()
                        .filter(|s| text(s) && !s.is_empty() && !s.contains(['/', '?', '@', '#']))
                        .ok_or_else(|| error("invalid netflow endpoint"))?;
                    let port = row["port"]
                        .as_u64()
                        .filter(|p| *p <= 65535)
                        .ok_or_else(|| error("invalid netflow endpoint"))?;
                    let outcome = row["outcome"]
                        .as_str()
                        .filter(|s| {
                            matches!(*s, "ok" | "http_error" | "transport_error" | "timeout")
                        })
                        .ok_or_else(|| error("invalid netflow outcome"))?;
                    let fidelity = row["fidelity"]
                        .as_str()
                        .filter(|s| matches!(*s, "coarse" | "socket" | "http" | "full"))
                        .ok_or_else(|| error("invalid netflow fidelity"))?;
                    *groups
                        .entry((host.into(), port, outcome.into(), fidelity.into()))
                        .or_default() += 1;
                }
                Some("dropped") => {
                    let count = row["count"]
                        .as_u64()
                        .ok_or_else(|| error("invalid netflow dropped count"))?;
                    dropped = dropped.saturating_add(count)
                }
                Some("capture") => capture += 1,
                Some("complete" | "socket_complete" | "recovered") => {}
                _ => unknown += 1,
            }
        }
        let omitted = groups.len().saturating_sub(50);
        let ownership_evidence = match (legacy_owner_rows, annotated_owner_rows) {
            (0, annotated) if annotated > 0 => "record_annotations",
            (legacy, annotated) if legacy > 0 && annotated > 0 => "mixed",
            _ => "artifact_path_only",
        };
        let rows:Vec<_>=groups.into_iter().take(50).map(|((host,port,outcome,fidelity),count)|json!({"host":host,"port":port,"outcome":outcome,"fidelity":fidelity,"count":count})).collect();
        Ok(
            json!({"run_id":run,"rows":rows,"partial":lines.partial||omitted>0||unknown>0||legacy_owner_rows>0,"legacy_owner_rows":legacy_owner_rows,"ownership_evidence":ownership_evidence,"omitted_endpoint_groups":omitted,"unrecognized_records":unknown,"capture_records":capture,"recorded_drop_markers":dropped,"evidence":"bounded_recorded_prefix","coverage":"capture_may_be_incomplete","bytes_read":lines.bytes}),
        )
    }
}
impl SessionReadPort for SessionReads {
    fn permits(&self, name: &str) -> bool {
        self.snapshot
            .allowed_reads
            .iter()
            .any(|allowed| allowed == name)
    }
    fn read(&self, request: SessionReadRequest, ctx: &ToolContext) -> Result<Value, PortError> {
        let name = match &request {
            SessionReadRequest::Context => "session.context",
            SessionReadRequest::Workflows { .. } => "session.workflows",
            SessionReadRequest::Inspect { .. } => "run.inspect",
            SessionReadRequest::Netflows { .. } => "run.netflows",
        };
        if !self.permits(name) {
            return Err(error("session reader binding is not authorized"));
        }
        let mut budget = Budget::default();
        self.check(ctx, &mut budget)?;
        let result = match request {
            SessionReadRequest::Context => {
                json!({"schema_version":1,"session_id":self.owner.session_id,"run_id":self.owner.run_id,"workspace_id":self.owner.workspace_id,"host_id":"local","applies":"frozen_current_turn","selection":self.snapshot,"provider":ctx.identity.provider,"model":ctx.identity.model,"notice":"selection_is_not_execution_evidence"})
            }
            SessionReadRequest::Workflows { offset, limit } => {
                self.workflows(offset, limit, &mut budget)?
            }
            SessionReadRequest::Inspect { ancestry } => {
                let (run, path) = self.target(&ancestry, &mut budget)?;
                self.inspect(&run, &path, &mut budget)?
            }
            SessionReadRequest::Netflows { ancestry } => {
                let (run, _) = self.target(&ancestry, &mut budget)?;
                self.netflows(&run, &mut budget)?
            }
        };
        self.global.current()?;
        self.workspace.current()?;
        if let Some(root) = &self.project {
            root.current()?;
        }
        if serde_json::to_vec(&result)
            .map_err(|_| error("session observation serialization failed"))?
            .len()
            > MAX_OUTPUT
        {
            return Err(error("session observation exceeds output bound"));
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests;
