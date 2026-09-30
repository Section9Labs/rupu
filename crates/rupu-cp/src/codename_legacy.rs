//! Derive-on-read codenames BELOW the run level for records written before
//! codenames existed (spec §6): linear steps, fan-out units, parallel
//! sub-steps, panelists, fixers, `on_reject` cleanups, dispatched sub-agents,
//! and the step/unit/dispatch events that announce them.
//!
//! The derivation replays exactly what the runner would have minted: each run
//! dir stores its `workflow.yaml`, and [`RunNaming::open`] (in memory — never
//! `codenames.json`, a legacy run dir is never written to) walks the same
//! static slots the runner walks, so a derived name equals the name a new run
//! of that workflow gets. Sub-agents replay the dispatcher's
//! `<parent>><canonical role>#n` rule with `n` ordered by sub-run ULID. With no
//! parseable snapshot the fallback is [`rupu_codename::derive_legacy`] (plus
//! `#index+1` for units).
//!
//! Every name filled here is flagged `codename_derived: true`; a record that
//! already carries a codename is never touched. This is the ONE place the
//! run-detail, run-graph and event read paths derive from.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rupu_codename::{crew_for, role_word, Codename, CrewNamer};
use rupu_orchestrator::{
    codenames::RunNaming,
    executor::Event,
    runs::{RunRecord, RunStore},
    Step, Workflow,
};
use serde_json::{Map, Value};

/// JSON key flagging a derived (not stored) codename.
pub const DERIVED_KEY: &str = "codename_derived";

/// Depth backstop for the sub-run walk (mirrors `RunStore`'s own guard).
const MAX_SUB_DEPTH: u32 = 64;

/// Identity of a dispatched sub-agent, folded from its transcript's first-line
/// `RunStart` plus the derived (or stored) codename.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubIdentity {
    pub codename: Option<String>,
    /// True when `codename` was derived here rather than read from the
    /// transcript's `RunStart`.
    pub derived: bool,
    pub agent: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// First-line `RunStart` of a transcript: `(agent, provider, model, codename)`.
pub fn transcript_run_start(path: &Path) -> Option<(String, String, String, Option<String>)> {
    let mut events = rupu_transcript::JsonlReader::iter(path).ok()?;
    match events.next() {
        Some(Ok(rupu_transcript::Event::RunStart {
            agent,
            provider,
            model,
            codename,
            ..
        })) => Some((agent, provider, model, codename)),
        _ => None,
    }
}

/// Per-run derivation state. Open once per run per request / stream (it
/// parses `workflow.yaml` once) and reuse it for every record and event.
pub struct LegacyNamer {
    run_id: String,
    wf: Option<Workflow>,
    naming: Option<RunNaming>,
    /// `agent:<name>` standalone runs: the root agent.
    agent_run: Option<String>,
    /// Panel units seen per `(step, iteration, panelist)`: the rank of a
    /// unit's view index among them is its occurrence (see [`Self::unit_event`]).
    panel_seen: HashMap<(String, u32, String), BTreeSet<usize>>,
    /// Sub-run identities, built lazily on first need.
    subs: Option<BTreeMap<String, SubIdentity>>,
    /// When `subs` was last (re)built: a live stream's cache miss rebuilds at
    /// most once per [`SUB_REFRESH`], never once per new dispatch.
    subs_built_at: Option<Instant>,
}

/// Minimum interval between two sub-run tree rebuilds for one run.
const SUB_REFRESH: Duration = Duration::from_secs(2);

/// Is `record` a run written before codenames existed? Decided ONCE per run
/// from the run record — the only reliable signal: a codename-era runner
/// still writes codename-less records/events on purpose (skipped / pruned /
/// cancelled steps, gate / action / branch / split / join steps), and those
/// must never get derived names.
pub fn run_is_legacy(record: &RunRecord) -> bool {
    record.codename.as_deref().is_none_or(str::is_empty)
}

fn has_codename(obj: &Map<String, Value>) -> bool {
    obj.get("codename")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
}

fn set_derived(obj: &mut Map<String, Value>, name: String) {
    obj.insert("codename".into(), Value::String(name));
    obj.insert(DERIVED_KEY.into(), Value::Bool(true));
}

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// `iter{N}:{rest}` → `(N, rest)`; a key without the prefix is iteration 0.
fn split_panel_key(unit_key: &str) -> (u32, &str) {
    if let Some(tail) = unit_key.strip_prefix("iter") {
        if let Some((n, rest)) = tail.split_once(':') {
            if let Ok(n) = n.parse() {
                return (n, rest);
            }
        }
    }
    (0, unit_key)
}

fn is_fan_or_group(step: &Step) -> bool {
    step.for_each.is_some() || step.parallel.is_some() || step.panel.is_some()
}

/// A transcript path's file stem is the agent run id (`run_<ULID>.jsonl`).
fn run_id_of_transcript(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl LegacyNamer {
    /// Open the namer for `run_id` in `store`: reads `run.json`'s workflow
    /// name and the `workflow.yaml` snapshot. Never writes.
    pub fn open(store: &RunStore, run_id: &str) -> Self {
        let workflow_name = store.load(run_id).ok().map(|r| r.workflow_name);
        let yaml = store.read_workflow_snapshot(run_id).ok();
        Self::from_snapshot(run_id, workflow_name.as_deref(), yaml.as_deref())
    }

    /// Build from an already-read snapshot (`yaml` absent/unparseable ⇒ the
    /// `derive_legacy` fallback).
    pub fn from_snapshot(run_id: &str, workflow_name: Option<&str>, yaml: Option<&str>) -> Self {
        let agent_run = workflow_name
            .and_then(|n| n.strip_prefix("agent:"))
            .filter(|a| !a.is_empty())
            .map(str::to_string);
        let wf = if agent_run.is_some() {
            None
        } else {
            yaml.filter(|y| !y.trim().is_empty())
                .and_then(|y| Workflow::parse(y).ok())
        };
        let naming = wf.as_ref().map(|wf| RunNaming::open(wf, run_id, None));
        Self {
            run_id: run_id.to_string(),
            wf,
            naming,
            agent_run,
            panel_seen: HashMap::new(),
            subs: None,
            subs_built_at: None,
        }
    }

    fn crew(&self) -> Codename {
        Codename::crew_only(crew_for(&self.run_id))
    }

    /// No-snapshot fallback: `crew/<role_word(agent)>[#n]`.
    fn fallback(&self, agent: Option<&str>, n: Option<u32>) -> Option<String> {
        let agent = agent.filter(|a| !a.is_empty())?;
        Some(self.crew().child(role_word(agent), n).to_string())
    }

    fn top_step(&self, step_id: &str) -> Option<&Step> {
        self.wf.as_ref()?.steps.iter().find(|s| s.id == step_id)
    }

    /// `(gate step id, cleanup step)` for an `on_reject` cleanup step id.
    fn on_reject_step(&self, step_id: &str) -> Option<(&str, &Step)> {
        for s in &self.wf.as_ref()?.steps {
            if let Some(ap) = &s.approval {
                if let Some(r) = ap.on_reject.iter().find(|r| r.id == step_id) {
                    return Some((s.id.as_str(), r));
                }
            }
        }
        None
    }

    /// Codename of the single member that ran a linear agent step (or an
    /// `on_reject` cleanup). `None` for fan-out / parallel / panel steps
    /// (their instances are named per unit) and for agent-less steps.
    pub fn step(&self, step_id: &str, agent_hint: Option<&str>) -> Option<String> {
        let Some(naming) = &self.naming else {
            return self.fallback(agent_hint.or(self.agent_run.as_deref()), None);
        };
        if let Some(step) = self.top_step(step_id) {
            if is_fan_or_group(step) {
                return None;
            }
            let agent = step.agent.as_deref()?;
            return Some(naming.step(step_id, agent).to_string());
        }
        if let Some((gate, cleanup)) = self.on_reject_step(step_id) {
            let agent = cleanup.agent.as_deref()?;
            return Some(naming.sub(gate, step_id, agent).to_string());
        }
        // A step id the snapshot doesn't know: never allocate a new role
        // (that would make names depend on read order) — plain fallback.
        self.fallback(agent_hint, None)
    }

    /// The agent def a linear step (or `on_reject` cleanup) runs, per the
    /// snapshot.
    pub fn step_agent(&self, step_id: &str) -> Option<String> {
        if let Some(step) = self.top_step(step_id) {
            return step.agent.clone();
        }
        if let Some((_, cleanup)) = self.on_reject_step(step_id) {
            return cleanup.agent.clone();
        }
        self.agent_run.clone()
    }

    /// Codename of one persisted unit record (`StepResultRecord.items[]`).
    pub fn item(
        &self,
        step_id: &str,
        index: usize,
        sub_id: &str,
        is_fixer: bool,
        agent_hint: Option<&str>,
    ) -> Option<String> {
        let n1 = u32::try_from(index + 1).ok();
        let (Some(naming), Some(step)) = (&self.naming, self.top_step(step_id)) else {
            let agent = agent_hint.or(Some(sub_id).filter(|s| !s.starts_with("fixer:")));
            return self.fallback(agent, n1);
        };
        if step.for_each.is_some() {
            let agent = step.agent.as_deref()?;
            return Some(naming.unit(step_id, agent, index).to_string());
        }
        if let Some(subs) = &step.parallel {
            let sub = subs.iter().find(|s| s.id == sub_id)?;
            return Some(naming.sub(step_id, sub_id, &sub.agent).to_string());
        }
        if let Some(panel) = &step.panel {
            if is_fixer || sub_id.starts_with("fixer:") {
                let gate = panel.gate.as_ref()?;
                return Some(naming.fixer(step_id, &gate.fix_with).to_string());
            }
            // Panel items: `index` is the panelist's position in
            // `panel.panelists`, `sub_id` its agent.
            if panel.panelists.get(index).map(String::as_str) != Some(sub_id) {
                return None;
            }
            let occurrence = panelist_occurrence(&panel.panelists, index);
            return Some(naming.panelist(step_id, sub_id, occurrence).to_string());
        }
        if step.join.is_some() {
            // A join's items are its winning source steps.
            return self.step(sub_id, None);
        }
        None
    }

    /// Codename for a `unit_started` event (or an events-only graph unit).
    /// Panel units carry a monotonic view index, so a repeated panelist's
    /// occurrence is the rank of its index among the same
    /// `(step, iteration, panelist)` units seen so far — exact when units are
    /// fed in index order, which is the runner's start order.
    pub fn unit_event(
        &mut self,
        step_id: &str,
        index: usize,
        unit_key: &str,
        agent: Option<&str>,
    ) -> Option<String> {
        let n1 = u32::try_from(index + 1).ok();
        let (Some(naming), Some(step)) = (&self.naming, self.top_step(step_id)) else {
            return self.fallback(agent, n1);
        };
        if step.for_each.is_some() {
            let agent = step.agent.as_deref()?;
            return Some(naming.unit(step_id, agent, index).to_string());
        }
        let panel = step.panel.as_ref()?;
        let (iteration, rest) = split_panel_key(unit_key);
        if let Some(fixer) = rest.strip_prefix("fix:") {
            let fix_with = panel
                .gate
                .as_ref()
                .map(|g| g.fix_with.as_str())
                .unwrap_or(fixer);
            return Some(naming.fixer(step_id, fix_with).to_string());
        }
        let panelist = agent.filter(|a| !a.is_empty()).unwrap_or(rest).to_string();
        let count = panel.panelists.iter().filter(|p| **p == panelist).count();
        if count == 0 {
            return None;
        }
        let occurrence = if count > 1 {
            let seen = self
                .panel_seen
                .entry((step_id.to_string(), iteration, panelist.clone()))
                .or_default();
            seen.insert(index);
            let rank = seen.range(..index).count() + 1;
            Some(u32::try_from(rank.min(count)).unwrap_or(u32::MAX))
        } else {
            None
        };
        let naming = self.naming.as_ref()?;
        Some(naming.panelist(step_id, &panelist, occurrence).to_string())
    }

    /// Fill every missing codename in a serialized `step_results` array
    /// (run detail `steps[]` / graph `step_results[]`): linear step records,
    /// their `items[]`, and panel `findings[]`. Returns how many names were
    /// derived.
    pub fn fill_step_records(&self, steps: &mut [Value]) -> usize {
        let mut derived = 0;
        for rec in steps.iter_mut() {
            let Some(obj) = rec.as_object_mut() else {
                continue;
            };
            let Some(step_id) = str_field(obj, "step_id").map(str::to_string) else {
                continue;
            };
            let kind = str_field(obj, "kind").unwrap_or("linear");
            if kind == "linear" && !has_codename(obj) {
                if let Some(name) = self.step(&step_id, None) {
                    set_derived(obj, name);
                    derived += 1;
                }
            }
            // (sub_id, codename, output) of each named item, for
            // attributing findings of a repeated panelist.
            let mut panel_items: Vec<(String, String, String)> = Vec::new();
            if let Some(items) = obj.get_mut("items").and_then(Value::as_array_mut) {
                for item in items.iter_mut() {
                    let Some(it) = item.as_object_mut() else {
                        continue;
                    };
                    let index = it.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let sub_id = str_field(it, "sub_id").unwrap_or("").to_string();
                    let is_fixer = it.get("is_fixer").and_then(Value::as_bool).unwrap_or(false);
                    if !has_codename(it) {
                        if let Some(name) = self.item(&step_id, index, &sub_id, is_fixer, None) {
                            set_derived(it, name);
                            derived += 1;
                        }
                    }
                    if let Some(c) = str_field(it, "codename") {
                        panel_items.push((
                            sub_id.clone(),
                            c.to_string(),
                            str_field(it, "output").unwrap_or("").to_string(),
                        ));
                    }
                }
            }
            if let Some(findings) = obj.get_mut("findings").and_then(Value::as_array_mut) {
                for f in findings.iter_mut() {
                    let Some(fo) = f.as_object_mut() else {
                        continue;
                    };
                    if has_codename(fo) {
                        continue;
                    }
                    let Some(source) = str_field(fo, "source").map(str::to_string) else {
                        continue;
                    };
                    let title = str_field(fo, "title").unwrap_or("").to_string();
                    if let Some(name) = self.finding(&step_id, &source, &title, &panel_items) {
                        set_derived(fo, name);
                        derived += 1;
                    }
                }
            }
        }
        derived
    }

    /// A panel finding's panelist. A singleton panelist is unambiguous; a
    /// repeated one is attributed to the single item of that panelist whose
    /// final output contains the finding's title, else left unnamed.
    fn finding(
        &self,
        step_id: &str,
        source: &str,
        title: &str,
        items: &[(String, String, String)],
    ) -> Option<String> {
        let (Some(naming), Some(step)) = (&self.naming, self.top_step(step_id)) else {
            return self.fallback(Some(source), None);
        };
        let panel = step.panel.as_ref()?;
        let count = panel.panelists.iter().filter(|p| *p == source).count();
        match count {
            0 => None,
            1 => Some(naming.panelist(step_id, source, None).to_string()),
            _ => {
                let mut hits = items.iter().filter(|(sub, _, out)| {
                    sub == source && !title.is_empty() && out.contains(title)
                });
                let first = hits.next()?;
                hits.next().is_none().then(|| first.1.clone())
            }
        }
    }

    /// Fill missing codenames on a graph `units[]` array (checkpoints +
    /// events-only units). Units are named in index order per step so a
    /// repeated panelist's occurrence rank is exact. Returns how many names
    /// were derived.
    pub fn fill_units(&mut self, units: &mut [Value]) -> usize {
        let mut derived = 0;
        let mut order: Vec<usize> = (0..units.len()).collect();
        order.sort_by_key(|&i| {
            let o = units[i].as_object();
            (
                o.and_then(|o| str_field(o, "step_id"))
                    .unwrap_or("")
                    .to_string(),
                o.and_then(|o| o.get("index"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            )
        });
        for i in order {
            let Some(obj) = units[i].as_object_mut() else {
                continue;
            };
            if has_codename(obj) {
                continue;
            }
            let Some(step_id) = str_field(obj, "step_id").map(str::to_string) else {
                continue;
            };
            let index = obj.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let key = match obj.get("item") {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            let agent = str_field(obj, "agent").map(str::to_string);
            if let Some(name) = self.unit_event(&step_id, index, &key, agent.as_deref()) {
                set_derived(obj, name);
                derived += 1;
            }
        }
        derived
    }

    // ── Sub-agents ────────────────────────────────────────────────────────

    /// Codename per agent run id for every static-slot instance of this run
    /// (step records + items, stored or derived) plus events-only units —
    /// the parents a dispatched sub-agent can hang off.
    fn instance_parents(&mut self, store: &RunStore) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let mut steps: Vec<Value> = store
            .read_step_results(&self.run_id)
            .unwrap_or_default()
            .iter()
            .filter_map(|r| serde_json::to_value(r).ok())
            .collect();
        self.fill_step_records(&mut steps);
        for rec in &steps {
            let Some(obj) = rec.as_object() else { continue };
            if let (Some(id), Some(c)) = (str_field(obj, "run_id"), str_field(obj, "codename")) {
                out.insert(id.to_string(), c.to_string());
            }
            for it in obj
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(it) = it.as_object() else { continue };
                if let (Some(id), Some(c)) = (str_field(it, "run_id"), str_field(it, "codename")) {
                    out.insert(id.to_string(), c.to_string());
                }
            }
        }
        // Events: running linear steps (step_working transcript) and units
        // (unit_started) that have no persisted record yet.
        let mut step_agent: HashMap<String, Option<String>> = HashMap::new();
        for ev in read_events(store, &self.run_id) {
            match ev {
                Event::StepStarted { step_id, agent, .. } => {
                    step_agent.insert(step_id, agent);
                }
                Event::StepWorking {
                    step_id,
                    transcript_path: Some(p),
                    ..
                } => {
                    let agent = step_agent.get(&step_id).cloned().flatten();
                    if let (Some(id), Some(c)) = (
                        run_id_of_transcript(&p.to_string_lossy()),
                        self.step(&step_id, agent.as_deref()),
                    ) {
                        out.entry(id).or_insert(c);
                    }
                }
                Event::UnitStarted {
                    step_id,
                    index,
                    unit_key,
                    agent,
                    transcript_path,
                    codename,
                    ..
                } => {
                    let c = codename
                        .or_else(|| self.unit_event(&step_id, index, &unit_key, agent.as_deref()));
                    if let (Some(id), Some(c)) =
                        (run_id_of_transcript(&transcript_path.to_string_lossy()), c)
                    {
                        out.entry(id).or_insert(c);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The codename a sub-agent dispatched directly by the run itself hangs
    /// off: the standalone agent (`crew/role`) for an `agent:` run, else the
    /// bare crew.
    fn root_codename(&self) -> Codename {
        match &self.agent_run {
            Some(a) => self.crew().child(role_word(a), None),
            None => self.crew(),
        }
    }

    /// Build `{sub_run_id: identity}` for every sub-run in this run's tree.
    /// `n` in `>{role}#n` is the order of that `(parent, role)` by sub-run
    /// ULID (chronological) — the dispatcher's own counter order. Caveat: two
    /// sub-runs minted in the same millisecond share their ULID timestamp and
    /// order by the random suffix, which may not be dispatch order; such a
    /// pair of same-(parent, role) siblings can have their `#n` swapped.
    fn build_subs(&mut self, store: &RunStore) -> BTreeMap<String, SubIdentity> {
        let mut parents = self.instance_parents(store);
        let root = self.root_codename();
        parents.insert(self.run_id.clone(), root.to_string());

        // Walk the tree: `<root>/<parent>/sub/<sub_id>/`.
        let mut edges: Vec<(String, String)> = Vec::new(); // (sub, parent)
        let mut visited: HashSet<String> = parents.keys().cloned().collect();
        let mut frontier: Vec<(String, u32)> = parents.keys().map(|k| (k.clone(), 0)).collect();
        while let Some((parent, depth)) = frontier.pop() {
            if depth >= MAX_SUB_DEPTH {
                continue;
            }
            for child in store.sub_run_ids(&parent) {
                if visited.insert(child.clone()) {
                    edges.push((child.clone(), parent.clone()));
                    frontier.push((child, depth + 1));
                }
            }
        }
        if edges.is_empty() {
            return BTreeMap::new();
        }
        // Chronological: a child is always dispatched after its parent, so
        // ULID order also resolves every parent before its children.
        edges.sort();

        // The run's namer state after the static walk (dynamic dispatch
        // allocates canonical roles from here), cloned so a rebuild never
        // double-counts instances.
        let mut namer: CrewNamer = match &self.naming {
            Some(n) => n.namer().with(|n| n.clone()),
            None => {
                let mut n = CrewNamer::new(crew_for(&self.run_id));
                if let Some(a) = &self.agent_run {
                    n.seed_role(a, role_word(a));
                }
                n
            }
        };
        let mut out = BTreeMap::new();
        let mut names: HashMap<String, String> = parents.clone().into_iter().collect();
        for (sub, parent) in edges {
            let path = sub_transcript(store, &parent, &sub);
            let start = transcript_run_start(&path);
            let mut ident = SubIdentity::default();
            if let Some((agent, provider, model, stored)) = start {
                ident.agent = Some(agent).filter(|a| !a.is_empty());
                ident.provider = Some(provider).filter(|a| !a.is_empty());
                ident.model = Some(model).filter(|a| !a.is_empty());
                if let Some(c) = stored.filter(|c| !c.is_empty()) {
                    ident.codename = Some(c);
                }
            }
            if ident.codename.is_none() {
                let parent_name = names.get(&parent).and_then(|p| p.parse::<Codename>().ok());
                if let (Some(parent_name), Some(agent)) = (parent_name, ident.agent.as_deref()) {
                    let role = namer.canonical_role(agent);
                    let k = namer.next_instance(&parent_name, &role);
                    ident.codename = Some(parent_name.child(&role, Some(k)).to_string());
                    ident.derived = true;
                }
            }
            if let Some(c) = &ident.codename {
                names.insert(sub.clone(), c.clone());
            }
            out.insert(sub, ident);
        }
        out
    }

    /// Every sub-run identity in this run's tree (built once, cached).
    pub fn subs(&mut self, store: &RunStore) -> &BTreeMap<String, SubIdentity> {
        if self.subs.is_none() {
            let built = self.build_subs(store);
            self.subs = Some(built);
            self.subs_built_at = Some(Instant::now());
        }
        self.subs.as_ref().expect("just built")
    }

    /// One sub-run's identity. A miss (a live run dispatched it after the
    /// cache was built) rebuilds the tree at most once per [`SUB_REFRESH`];
    /// a miss inside the window stays unnamed (a later graph read names it).
    pub fn sub(&mut self, store: &RunStore, sub_run_id: &str) -> Option<SubIdentity> {
        if let Some(hit) = self.subs(store).get(sub_run_id) {
            return Some(hit.clone());
        }
        let stale = self
            .subs_built_at
            .is_none_or(|t| t.elapsed() >= SUB_REFRESH);
        if stale {
            self.subs = None;
            return self.subs(store).get(sub_run_id).cloned();
        }
        None
    }

    // ── Events ────────────────────────────────────────────────────────────

    /// Fill a missing codename on one serialized event row (`step_started` /
    /// `unit_started` / `dispatch_started` / `agent_started`). Rows that
    /// already carry a codename, and every other type, are left untouched.
    /// Returns whether a name was filled.
    pub fn fill_event(&mut self, store: &RunStore, row: &mut Value) -> bool {
        let Some(obj) = row.as_object_mut() else {
            return false;
        };
        if has_codename(obj) {
            return false;
        }
        let ty = str_field(obj, "type").unwrap_or("").to_string();
        let step_id = str_field(obj, "step_id").unwrap_or("").to_string();
        let agent = str_field(obj, "agent").map(str::to_string);
        let name = match ty.as_str() {
            "step_started" => self.step(&step_id, agent.as_deref()),
            "unit_started" => {
                let index = obj.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let key = str_field(obj, "unit_key").unwrap_or("").to_string();
                self.unit_event(&step_id, index, &key, agent.as_deref())
            }
            "agent_started" => match obj.get("unit_index").and_then(Value::as_u64) {
                None => self.step(&step_id, agent.as_deref()),
                Some(i) => self.agent_unit(&step_id, i as usize, agent.as_deref()),
            },
            "dispatch_started" => {
                let sub = str_field(obj, "sub_run_id").unwrap_or("").to_string();
                self.sub(store, &sub).and_then(|s| s.codename)
            }
            _ => None,
        };
        match name {
            Some(n) => {
                set_derived(obj, n);
                true
            }
            None => false,
        }
    }

    /// `agent_started` with a `unit_index`: a fan-out unit (index = item
    /// index) or a parallel sub-step (index = declared position). Panel
    /// panelists are named on their `unit_started` instead.
    fn agent_unit(&self, step_id: &str, index: usize, agent: Option<&str>) -> Option<String> {
        let (Some(naming), Some(step)) = (&self.naming, self.top_step(step_id)) else {
            return self.fallback(agent, u32::try_from(index + 1).ok());
        };
        if step.for_each.is_some() {
            return Some(
                naming
                    .unit(step_id, step.agent.as_deref()?, index)
                    .to_string(),
            );
        }
        let sub = step.parallel.as_ref()?.get(index)?;
        Some(naming.sub(step_id, &sub.id, &sub.agent).to_string())
    }

    /// The codename of a parallel sub-step at its declared position (graph
    /// `unit_identities`).
    pub fn parallel_subs(&self, step_id: &str) -> Vec<(usize, String, String)> {
        let (Some(naming), Some(step)) = (&self.naming, self.top_step(step_id)) else {
            return Vec::new();
        };
        step.parallel
            .iter()
            .flatten()
            .enumerate()
            .map(|(i, s)| {
                (
                    i,
                    naming.sub(step_id, &s.id, &s.agent).to_string(),
                    s.agent.clone(),
                )
            })
            .collect()
    }
}

/// Occurrence of the panelist at `index`: `Some(k)` only when its def repeats.
fn panelist_occurrence(panelists: &[String], index: usize) -> Option<u32> {
    let agent = panelists.get(index)?;
    let count = panelists.iter().filter(|p| *p == agent).count();
    (count > 1).then(|| {
        let k = panelists[..=index].iter().filter(|p| *p == agent).count();
        u32::try_from(k).unwrap_or(u32::MAX)
    })
}

fn sub_transcript(store: &RunStore, parent: &str, sub: &str) -> PathBuf {
    store
        .root
        .join(parent)
        .join("sub")
        .join(sub)
        .join("transcript.jsonl")
}

/// Every parseable event in a run's `events.jsonl` (missing file ⇒ empty).
fn read_events(store: &RunStore, run_id: &str) -> Vec<Event> {
    use std::io::{BufRead, BufReader};
    let Ok(file) = std::fs::File::open(store.events_path(run_id)) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Event>(&l).ok())
        .collect()
}

/// Does this event announce an agent instance without a codename? Cheap,
/// typed check so the streams only re-serialize the events they change.
pub fn event_needs_name(ev: &Event) -> bool {
    matches!(
        ev,
        Event::StepStarted { codename: None, .. }
            | Event::UnitStarted { codename: None, .. }
            | Event::DispatchStarted { codename: None, .. }
            | Event::AgentStarted { codename: None, .. }
    )
}

/// What an [`EventNamers`] knows about one run.
enum RunNames {
    /// A codename-era run (or one whose record can't be read): nothing is
    /// ever derived, no snapshot is ever parsed. One byte of cache.
    Modern,
    Legacy(Box<LegacyNamer>),
}

/// Per-stream / per-request cache keyed by run id. Each run is classified
/// ONCE ([`run_is_legacy`]); only a legacy run opens a [`LegacyNamer`]
/// (parsing `workflow.yaml` once). Owns its store handle so it can live in a
/// `'static` SSE stream; long-lived holders [`Self::evict`] finished runs.
pub struct EventNamers {
    store: Arc<RunStore>,
    by_run: HashMap<String, RunNames>,
}

/// An [`EventNamers`] shared between an SSE stream and the task feeding it.
pub type SharedEventNamers = Arc<Mutex<EventNamers>>;

impl EventNamers {
    pub fn new(store: Arc<RunStore>) -> Self {
        Self {
            store,
            by_run: HashMap::new(),
        }
    }

    pub fn shared(store: Arc<RunStore>) -> SharedEventNamers {
        Arc::new(Mutex::new(Self::new(store)))
    }

    /// Classify a run from a record the caller already holds (no IO). A
    /// legacy run's namer is still opened lazily, on its first unnamed event.
    pub fn note_record(&mut self, record: &RunRecord) {
        if !run_is_legacy(record) {
            self.by_run.insert(record.id.clone(), RunNames::Modern);
        }
    }

    /// True when this run is already known to need no derivation — the
    /// no-IO fast path the streams check before going to the blocking pool.
    pub fn known_modern(&self, run_id: &str) -> bool {
        matches!(self.by_run.get(run_id), Some(RunNames::Modern))
    }

    /// Forget a run (its stream tail was pruned).
    pub fn evict(&mut self, run_id: &str) {
        self.by_run.remove(run_id);
    }

    /// Number of runs cached (tests / diagnostics).
    pub fn cached_runs(&self) -> usize {
        self.by_run.len()
    }

    /// Number of cached runs that opened a [`LegacyNamer`].
    pub fn legacy_runs(&self) -> usize {
        self.by_run
            .values()
            .filter(|r| matches!(r, RunNames::Legacy(_)))
            .count()
    }

    /// Fill a missing codename on a serialized event row of run `run_id`.
    /// Blocking (may read run.json / workflow.yaml / the sub-run tree).
    pub fn fill_row(&mut self, run_id: &str, row: &mut Value) -> bool {
        let store = Arc::clone(&self.store);
        let names =
            self.by_run
                .entry(run_id.to_string())
                .or_insert_with(|| match store.load(run_id) {
                    Ok(rec) if run_is_legacy(&rec) => {
                        RunNames::Legacy(Box::new(LegacyNamer::open(&store, run_id)))
                    }
                    _ => RunNames::Modern,
                });
        match names {
            RunNames::Modern => false,
            RunNames::Legacy(namer) => namer.fill_event(&store, row),
        }
    }

    /// Typed variant: `Some(row)` (the event with a derived codename) only
    /// when a name was filled; `None` means "emit the event exactly as
    /// before". Blocking — async callers use [`name_event`].
    pub fn fill_typed(&mut self, ev: &Event) -> Option<Value> {
        if !event_needs_name(ev) {
            return None;
        }
        let mut row = serde_json::to_value(ev).ok()?;
        self.fill_row(ev.run_id(), &mut row).then_some(row)
    }
}

fn lock(namers: &SharedEventNamers) -> std::sync::MutexGuard<'_, EventNamers> {
    namers.lock().unwrap_or_else(|p| p.into_inner())
}

/// Async, executor-safe [`EventNamers::fill_typed`]: events that need no
/// name, and events of runs already known to be codename-era, return with no
/// IO; anything else (classifying a new run, opening a legacy namer, walking
/// its sub-run tree) runs on the blocking pool.
pub async fn name_event(namers: &SharedEventNamers, ev: &Event) -> Option<Value> {
    if !event_needs_name(ev) || lock(namers).known_modern(ev.run_id()) {
        return None;
    }
    let namers = Arc::clone(namers);
    let ev = ev.clone();
    tokio::task::spawn_blocking(move || lock(&namers).fill_typed(&ev))
        .await
        .ok()
        .flatten()
}

/// Classify one run for a per-run event stream, off the executor. `Some`
/// (with the run's legacy namer already opened) only for a legacy run; a
/// codename-era run — or an unreadable record — gets `None` and its stream
/// never touches the derivation.
pub async fn namers_for_run(store: Arc<RunStore>, run_id: &str) -> Option<SharedEventNamers> {
    let id = run_id.to_string();
    tokio::task::spawn_blocking(move || {
        let record = store.load(&id).ok()?;
        if !run_is_legacy(&record) {
            return None;
        }
        let namer = LegacyNamer::open(&store, &id);
        let mut namers = EventNamers::new(store);
        namers.by_run.insert(id, RunNames::Legacy(Box::new(namer)));
        Some(Arc::new(Mutex::new(namers)))
    })
    .await
    .ok()
    .flatten()
}

/// Fill derived codenames on a run-detail `{run, steps, usage}` object's
/// `steps[]` (linear step records, `items[]`, panel `findings[]`) — for a
/// legacy run only ([`run_is_legacy`]); a codename-era run is returned as is
/// and its snapshot never parsed.
pub fn fill_detail_steps(store: &RunStore, record: &RunRecord, detail: &mut Value) {
    if !run_is_legacy(record) {
        return;
    }
    let Some(steps) = detail.get_mut("steps").and_then(Value::as_array_mut) else {
        return;
    };
    if steps.iter().any(record_lacks_name) {
        LegacyNamer::open(store, &record.id).fill_step_records(steps);
    }
}

/// Does a serialized step record (or any of its items / findings) lack a
/// codename it could carry?
pub fn record_lacks_name(rec: &Value) -> bool {
    let Some(obj) = rec.as_object() else {
        return false;
    };
    let linear = str_field(obj, "kind").unwrap_or("linear") == "linear";
    let nested = |key: &str| {
        obj.get(key).and_then(Value::as_array).is_some_and(|a| {
            a.iter()
                .any(|v| v.as_object().is_some_and(|o| !has_codename(o)))
        })
    };
    (linear && !has_codename(obj)) || nested("items") || nested("findings")
}

#[cfg(test)]
mod tests;
