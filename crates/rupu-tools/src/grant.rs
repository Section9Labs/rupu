//! What an agent may call: the one grant resolver.
//!
//! [`ToolCatalog::resolve_grant`] turns an agent's `tools:` list, its step's
//! `actions:`, the run's ambient grants and the services the run provides
//! into a [`ResolvedGrant`]: the tools offered to the model, each with the
//! reasons it is there, plus what `actions:` narrowed away and what was named
//! but can't be served. It is the only input to a run's tool registry, it is
//! written to the transcript (`tool_grant`), and every call is audited
//! against it.
//!
//! The `tools:` grammar (D9):
//!
//! | Entry | Means |
//! |---|---|
//! | `bash`, `findings.report`, … | that canonical tool |
//! | `report_finding`, … | an alias, resolved to its canonical tool |
//! | `findings.*`, `scm.*`, … | every tool in that namespace |
//! | `core.*` | the unqualified core tools ([`crate::catalog::CORE_NAMES`]) |
//! | `*` | the whole catalog, except the authority tools (`wildcard: false`: `board.directive`, `board.retract` — D22) |
//! | anything else | a [`GrantError`] with a did-you-mean (D8) |
//!
//! A wildcard adds only tools this run can serve: one whose service is
//! missing is recorded as skipped, silently. A tool named exactly whose
//! service is missing is recorded as unavailable, and the run says so with a
//! `tool_unavailable` notice (P7).
//!
//! Spec: `docs/superpowers/specs/2026-10-07-rupu-tool-and-launch-architecture/W2-tool-grants.md`.

use crate::catalog::ToolCatalog;
use crate::descriptor::{AliasScope, Service, ToolDescriptor};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// What an agent with no `tools:` list gets. This is a compatibility
/// default: it reproduces the registry such an agent had before grants
/// existed — every builtin (the core tools and sub-agent dispatch, now the
/// launch pair) and every connector tool.
pub const DEFAULT_GRANT: &[&str] = &[
    "core.*", "dispatch", "join", "scm.*", "issues.*", "github.*", "gitlab.*",
];

/// The `tools:` entry that grants the core tools.
const CORE_NAMESPACE: &str = "core.*";

/// Why a tool is in a grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantReason {
    /// Named exactly in `tools:` (canonical name or alias).
    Declared,
    /// Matched a wildcard in `tools:` (`*`, `core.*`, `scm.*`, …); carries
    /// the wildcard.
    DeclaredWildcard(String),
    /// The agent has no `tools:` list: [`DEFAULT_GRANT`].
    Default,
    /// Granted by the run's context, not the agent file (`concerns`,
    /// `engagement`).
    Ambient(&'static str),
    /// Granted by how the run was started (`flow_lead`, `flow_unit`).
    Origin(&'static str),
    /// Granted because another granted tool is unusable without it: `join`
    /// with `dispatch`, which returns before its child ends.
    Implied(&'static str),
}

impl fmt::Display for GrantReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrantReason::Declared => f.write_str("declared"),
            GrantReason::DeclaredWildcard(w) => write!(f, "declared:{w}"),
            GrantReason::Default => f.write_str("default"),
            GrantReason::Ambient(r) => write!(f, "ambient:{r}"),
            GrantReason::Origin(r) => write!(f, "origin:{r}"),
            GrantReason::Implied(by) => write!(f, "implied:{by}"),
        }
    }
}

/// A grant the run's context adds on top of the agent's `tools:`. Ambient
/// grants are never narrowed by `actions:` (none of them are connector
/// tools).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmbientGrant {
    /// Tool names (canonical or alias).
    pub tools: Vec<&'static str>,
    pub reason: GrantReason,
}

impl AmbientGrant {
    /// `concerns:` present: the coverage ledger tools and `findings.report`.
    pub fn concerns() -> Self {
        Self {
            tools: vec![
                "coverage.mark",
                "coverage.status",
                "coverage.remaining",
                "coverage.concerns.search",
                "coverage.concerns.detail",
                "findings.report",
            ],
            reason: GrantReason::Ambient("concerns"),
        }
    }

    /// An engagement profile is active: `findings.report` and `assets.mark`.
    pub fn engagement() -> Self {
        Self {
            tools: vec!["findings.report", "assets.mark"],
            reason: GrantReason::Ambient("engagement"),
        }
    }

    /// A run with skills is offered the loader (spec §2.4).
    pub fn skills() -> Self {
        Self {
            tools: vec!["skills.load"],
            reason: GrantReason::Ambient("skills"),
        }
    }

    /// An agentiflow lead's coordination set (`origin:flow_lead`): the whole
    /// board and mailbox, the flow's live status, the agent/workflow roster
    /// `findings.report`, and the launch tools (`dispatch`, `join`,
    /// `workflows.generate`).
    pub fn flow_lead() -> Self {
        Self {
            tools: vec![
                "board.claim",
                "board.release",
                "board.post",
                "board.read",
                "board.directive",
                "board.retract",
                "msg.send",
                "goal.status",
                "goal.coverage",
                "budget.status",
                "agents.list",
                "agents.get",
                "workflows.list",
                "workflows.get",
                "catalog.search",
                "findings.report",
                "dispatch",
                "join",
                "workflows.generate",
            ],
            reason: GrantReason::Origin("flow_lead"),
        }
    }

    /// A unit joining an agentiflow (`origin:flow_unit`): claim, release,
    /// post and read on the board, and `msg.send`.
    pub fn flow_unit() -> Self {
        Self {
            tools: vec![
                "board.claim",
                "board.release",
                "board.post",
                "board.read",
                "msg.send",
            ],
            reason: GrantReason::Origin("flow_unit"),
        }
    }
}

/// The messaging tools every run with a message space gets by default (spec
/// F1d D1), and what a workflow's `messaging: true` grants its steps (spec
/// F1a §7): the board and `msg.send`, without the authority tools.
pub const WORKFLOW_MESSAGING: &[&str] = &[
    "board.claim",
    "board.release",
    "board.post",
    "board.read",
    "msg.send",
];

impl AmbientGrant {
    /// A workflow's `messaging:` key (`ambient:workflow_messaging`): the
    /// tools it names, canonical, from [`ToolCatalog::resolve_messaging`].
    pub fn workflow_messaging(tools: Vec<&'static str>) -> Self {
        Self {
            tools,
            reason: GrantReason::Ambient("workflow_messaging"),
        }
    }

    /// The default (`ambient:messaging_default`, spec F1d D1): what a run in
    /// a message space gets when nothing names or turns off its messaging.
    pub fn messaging_default() -> Self {
        Self {
            tools: WORKFLOW_MESSAGING.to_vec(),
            reason: GrantReason::Ambient("messaging_default"),
        }
    }
}

/// What a workflow's `messaging:` key asks for its agent steps and units
/// (spec F1d D2), carried on the step's origin. Absent is the default set,
/// `true` or `{ tools: [...] }` the tools it names, `false` off for the
/// steps and everything they launch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MessagingSwitch {
    /// No key: the default (`ambient:messaging_default`).
    #[default]
    Default,
    /// `messaging: true` or `{ tools: [...] }`: these, canonical, as
    /// `ambient:workflow_messaging`.
    Tools(Vec<&'static str>),
    /// `messaging: false`.
    Off,
}

/// The services a run provides. A tool is offered only when every service
/// its descriptor `needs` is here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceSet(BTreeSet<Service>);

impl ServiceSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, s: Service) {
        self.0.insert(s);
    }

    pub fn with(mut self, s: Service) -> Self {
        self.insert(s);
        self
    }

    pub fn contains(&self, s: Service) -> bool {
        self.0.contains(&s)
    }

    /// The services `d` needs that this set lacks, in `d.needs` order.
    pub fn missing_for(&self, d: &ToolDescriptor) -> Vec<Service> {
        d.needs
            .iter()
            .copied()
            .filter(|s| !self.contains(*s))
            .collect()
    }
}

impl FromIterator<Service> for ServiceSet {
    fn from_iter<I: IntoIterator<Item = Service>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// The inputs to one grant.
#[derive(Clone, Copy, Debug)]
pub struct GrantInputs<'a> {
    /// The agent's `tools:`; `None` = [`DEFAULT_GRANT`].
    pub declared: Option<&'a [String]>,
    /// The step's `actions:`; empty = no narrowing.
    pub step_actions: &'a [String],
    /// The connector tools a parent run narrowed its children to (W7, L4):
    /// connector tools outside it are narrowed away like `actions:` does.
    /// `None` = no cap; an empty cap allows no connector tool.
    pub connector_cap: Option<&'a [String]>,
    pub ambient: &'a [AmbientGrant],
    /// Which services this run provides.
    pub available: &'a ServiceSet,
    /// Where aliases resolve: [`AliasScope::FlowLead`] inside an agentiflow
    /// lead (where `coverage.status` means `goal.coverage`), else
    /// [`AliasScope::Everywhere`].
    pub alias_scope: AliasScope,
}

/// One tool offered to the model.
#[derive(Clone, Debug)]
pub struct GrantEntry {
    pub canonical: &'static str,
    pub descriptor: &'static ToolDescriptor,
    /// Why it is offered, in the order the reasons were found.
    pub reasons: Vec<GrantReason>,
}

impl GrantEntry {
    /// The reasons as one string (`declared,ambient:concerns`), as the
    /// transcript's `tool_audit.reason` records them.
    pub fn reasons_string(&self) -> String {
        self.reasons
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// A tool the grant names that this run can't serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unavailable {
    pub tool: &'static str,
    /// The services it needs that the run lacks.
    pub missing: Vec<Service>,
}

impl Unavailable {
    /// The `tool_unavailable` notice text.
    pub fn notice_message(&self) -> String {
        let needs = self
            .missing
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "tool \"{}\" is not available in this run: it needs {needs}, which this run does not provide",
            self.tool
        )
    }
}

/// The notice kind written once per [`ResolvedGrant::unavailable`] tool.
pub const UNAVAILABLE_NOTICE_KIND: &str = "tool_unavailable";

/// The result of [`ToolCatalog::resolve_grant`].
#[derive(Clone, Debug, Default)]
pub struct ResolvedGrant {
    /// Offered to the model, by canonical name.
    pub entries: BTreeMap<&'static str, GrantEntry>,
    /// Connector tools the grant held that the step's `actions:` removed.
    pub narrowed: Vec<&'static str>,
    /// Named exactly, but a service is missing: not offered, and the run
    /// writes a `tool_unavailable` notice for each.
    pub unavailable: Vec<Unavailable>,
    /// Matched only by a wildcard (or the default grant, or an ambient
    /// grant), but a service is missing: not offered, no notice.
    pub skipped: Vec<Unavailable>,
    /// The step's `actions:`, resolved to canonical connector names; `None`
    /// when the step has none (the agent is unrestricted).
    pub actions: Option<BTreeSet<&'static str>>,
    /// Connector tools the step's `actions:` names that the agent's grant
    /// does not cover. The step can't add them (no escalation), but it is
    /// very likely an authoring mistake.
    pub actions_not_granted: Vec<&'static str>,
    /// The connector tools this run is narrowed to — its `actions:` within
    /// the cap its parent passed down — which its own children inherit as
    /// their cap (W7). `None` = not narrowed.
    pub connector_cap: Option<BTreeSet<&'static str>>,
}

impl ResolvedGrant {
    /// The entry for canonical name `name`.
    pub fn entry(&self, name: &str) -> Option<&GrantEntry> {
        self.entries.get(name)
    }

    /// Whether `name` (canonical) is offered.
    pub fn offers(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Whether the agent's grant covered `name` (canonical) before `actions:`
    /// narrowing — offered, narrowed away, or named but unservable.
    pub fn granted_before_narrowing(&self, name: &str) -> bool {
        self.entries.contains_key(name)
            || self.narrowed.contains(&name)
            || self.unavailable.iter().any(|u| u.tool == name)
    }

    /// Whether the step's `actions:` names `name` (canonical). `false` when
    /// the step has no `actions:`.
    pub fn declared_by_actions(&self, name: &str) -> bool {
        self.actions.as_ref().is_some_and(|a| a.contains(name))
    }

    /// Offer `d` for `reason`, or record why not.
    fn offer(
        &mut self,
        d: &'static ToolDescriptor,
        reason: GrantReason,
        named: bool,
        available: &ServiceSet,
    ) {
        if let Some(entry) = self.entries.get_mut(d.name) {
            if !entry.reasons.contains(&reason) {
                entry.reasons.push(reason);
            }
            return;
        }
        let missing = available.missing_for(d);
        if missing.is_empty() {
            self.skipped.retain(|u| u.tool != d.name);
            self.entries.insert(
                d.name,
                GrantEntry {
                    canonical: d.name,
                    descriptor: d,
                    reasons: vec![reason],
                },
            );
            return;
        }
        let known = |list: &[Unavailable]| list.iter().any(|u| u.tool == d.name);
        if named {
            self.skipped.retain(|u| u.tool != d.name);
            if !known(&self.unavailable) {
                self.unavailable.push(Unavailable {
                    tool: d.name,
                    missing,
                });
            }
        } else if !known(&self.unavailable) && !known(&self.skipped) {
            self.skipped.push(Unavailable {
                tool: d.name,
                missing,
            });
        }
    }
}

/// Why a `tools:` or `actions:` list can't be resolved.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GrantError {
    /// A name no tool answers to. `field` is where it was written
    /// (`tools:` / `actions:`); `suggestion` is the closest known name,
    /// already formatted (`"report_finding" → findings.report`).
    #[error("unknown tool \"{name}\" in {field}{}", suggestion_suffix(.suggestion))]
    UnknownTool {
        field: &'static str,
        name: String,
        suggestion: Option<String>,
    },
    /// `ns.*` where no tool's name starts with `ns.`.
    #[error("unknown tool namespace \"{name}\" in {field}{}", suggestion_suffix(.suggestion))]
    UnknownNamespace {
        field: &'static str,
        name: String,
        suggestion: Option<String>,
    },
    /// An `actions:` entry that names no connector tool. `actions:` narrows
    /// connector tools only, so the entry would do nothing.
    /// A `messaging: { tools: [...] }` entry that names no message-space
    /// tool.
    #[error(
        "\"{name}\" in messaging: is not a messaging tool; messaging: grants only board.* and msg.* tools"
    )]
    NotMessaging { name: String },
    #[error(
        "\"{name}\" in actions: is not a connector tool; actions: narrows only connector \
         (scm/issues) tools, so it would have no effect"
    )]
    NotConnector { name: String },
    /// A `tools:` entry names a messaging tool in an agent whose
    /// `messaging: false` turns them off (spec F1d D4): the file contradicts
    /// itself.
    #[error("\"{name}\" in tools: is a messaging tool, but the agent sets messaging: false")]
    MessagingOff { name: String },
}

fn suggestion_suffix(s: &Option<String>) -> String {
    match s {
        Some(s) => format!(" (did you mean {s}?)"),
        None => String::new(),
    }
}

/// One `tools:` / `actions:` entry, expanded.
enum Expansion {
    /// An exact name (canonical or alias).
    Exact(&'static ToolDescriptor),
    /// A wildcard: every matching tool.
    Wildcard(Vec<&'static ToolDescriptor>),
}

impl ToolCatalog {
    /// The tool `name` resolves to in `scope`: its canonical name, else one
    /// of its aliases. Inside a flow lead a lead-scoped alias wins over a
    /// canonical name (`coverage.status` → `goal.coverage`); elsewhere a
    /// lead-scoped alias doesn't resolve.
    pub fn resolve_name(&self, name: &str, scope: AliasScope) -> Option<&'static ToolDescriptor> {
        let scoped_alias = |want: AliasScope| {
            self.descriptors()
                .find(|d| d.aliases.iter().any(|a| a.name == name && a.scope == want))
        };
        if scope == AliasScope::FlowLead {
            if let Some(d) = scoped_alias(AliasScope::FlowLead) {
                return Some(d);
            }
        }
        self.descriptor(name)
            .or_else(|| scoped_alias(AliasScope::Everywhere))
    }

    /// Check that every entry of a `tools:` list is valid grammar naming a
    /// known tool or namespace. Names only: services don't matter here. This
    /// is what an agent file is checked with when it loads.
    pub fn validate_names(&self, names: &[String]) -> Result<(), GrantError> {
        for n in names {
            self.expand(n, AliasScope::Everywhere, "tools:")?;
        }
        Ok(())
    }

    /// Resolve a step's `actions:` to the canonical connector tools it
    /// names. Accepts exact names, aliases, `ns.*` and `*`; an entry naming
    /// no connector tool is an error ([`GrantError::NotConnector`]).
    pub fn resolve_actions(
        &self,
        actions: &[String],
        scope: AliasScope,
    ) -> Result<BTreeSet<&'static str>, GrantError> {
        let mut out = BTreeSet::new();
        for a in actions {
            let descs = match self.expand(a, scope, "actions:")? {
                Expansion::Exact(d) => vec![d],
                Expansion::Wildcard(ds) => ds,
            };
            let connectors: Vec<_> = descs.into_iter().filter(|d| d.is_connector()).collect();
            if connectors.is_empty() {
                return Err(GrantError::NotConnector { name: a.clone() });
            }
            out.extend(connectors.into_iter().map(|d| d.name));
        }
        Ok(out)
    }

    /// Resolve a workflow's `messaging: { tools: [...] }` to the canonical
    /// tools it grants. Exact names, aliases and `ns.*` only, each naming a
    /// tool that needs the message bus ([`GrantError::NotMessaging`]).
    pub fn resolve_messaging(&self, names: &[String]) -> Result<Vec<&'static str>, GrantError> {
        let mut out: Vec<&'static str> = Vec::new();
        for n in names {
            if n.trim() == "*" {
                return Err(GrantError::NotMessaging { name: n.clone() });
            }
            let descs = match self.expand(n, AliasScope::Everywhere, "messaging:")? {
                Expansion::Exact(d) => vec![d],
                Expansion::Wildcard(ds) => ds,
            };
            if descs.is_empty()
                || descs
                    .iter()
                    .any(|d| !d.needs.contains(&Service::MessageBus))
            {
                return Err(GrantError::NotMessaging { name: n.clone() });
            }
            for d in descs {
                if !out.contains(&d.name) {
                    out.push(d.name);
                }
            }
        }
        Ok(out)
    }

    /// The first `declared` entry that asks for a tool needing `service` by
    /// name: an exact name, an alias or its `ns.*` — never `*`, which asks
    /// for whatever the run happens to provide. `None` when none does.
    pub fn naming_service<'a>(&self, declared: &'a [String], service: Service) -> Option<&'a str> {
        declared
            .iter()
            .find(|n| {
                if matches!(n.trim(), "*" | "core.*") {
                    return false;
                }
                match self.expand(n, AliasScope::Everywhere, "tools:") {
                    Ok(Expansion::Exact(d)) => d.needs.contains(&service),
                    Ok(Expansion::Wildcard(ds)) => ds.iter().any(|d| d.needs.contains(&service)),
                    Err(_) => false,
                }
            })
            .map(String::as_str)
    }

    /// Compute what an agent may call (§3 of the W2 card).
    ///
    /// 1. The agent's `tools:` (or [`DEFAULT_GRANT`]), expanded by the
    ///    grammar. Only tools whose services the run provides are offered.
    /// 2. The step's `actions:` narrows connector tools only:
    ///    `effective = (G \ Connector) ∪ (G ∩ Connector ∩ actions)`.
    /// 3. Ambient grants are added, un-narrowed.
    pub fn resolve_grant(&self, inputs: GrantInputs<'_>) -> Result<ResolvedGrant, GrantError> {
        let mut g = ResolvedGrant::default();
        let scope = inputs.alias_scope;

        let (list, is_default): (Vec<&str>, bool) = match inputs.declared {
            Some(l) => (l.iter().map(String::as_str).collect(), false),
            None => (DEFAULT_GRANT.to_vec(), true),
        };
        for entry in list {
            let expansion = match self.expand(entry, scope, "tools:") {
                Ok(x) => x,
                // A namespace the default names but this catalog holds no
                // tool of (a catalog without the connector tools): nothing.
                Err(GrantError::UnknownNamespace { .. }) if is_default => continue,
                Err(e) => return Err(e),
            };
            match expansion {
                Expansion::Exact(d) if is_default => {
                    g.offer(d, GrantReason::Default, false, inputs.available)
                }
                Expansion::Exact(d) => g.offer(d, GrantReason::Declared, true, inputs.available),
                Expansion::Wildcard(ds) => {
                    let reason = if is_default {
                        GrantReason::Default
                    } else {
                        GrantReason::DeclaredWildcard(entry.to_string())
                    };
                    for d in ds {
                        g.offer(d, reason.clone(), false, inputs.available);
                    }
                }
            }
        }

        let actions = if inputs.step_actions.is_empty() {
            None
        } else {
            Some(self.resolve_actions(inputs.step_actions, scope)?)
        };
        // A parent's cap: names it resolved itself, so an unknown one is
        // dropped rather than an error here.
        let cap: Option<BTreeSet<&'static str>> = inputs.connector_cap.map(|c| {
            c.iter()
                .filter_map(|n| self.resolve_name(n, scope).map(|d| d.name))
                .collect()
        });
        let effective = match (&actions, cap) {
            (Some(a), Some(c)) => Some(a.intersection(&c).copied().collect()),
            (Some(a), None) => Some(a.clone()),
            (None, c) => c,
        };
        if let Some(allowed) = &effective {
            let keep = |name: &str, connector: bool| !connector || allowed.contains(name);
            let mut narrowed = Vec::new();
            g.entries.retain(|name, e| {
                let k = keep(name, e.descriptor.is_connector());
                if !k {
                    narrowed.push(*name);
                }
                k
            });
            for list in [&mut g.unavailable, &mut g.skipped] {
                list.retain(|u| {
                    let connector = self.descriptor(u.tool).is_some_and(|d| d.is_connector());
                    let k = keep(u.tool, connector);
                    if !k {
                        narrowed.push(u.tool);
                    }
                    k
                });
            }
            narrowed.sort_unstable();
            narrowed.dedup();
            g.narrowed = narrowed;
        }
        if let Some(actions) = actions {
            g.actions_not_granted = actions
                .iter()
                .copied()
                .filter(|a| !g.granted_before_narrowing(a))
                .collect();
            g.actions = Some(actions);
        }
        g.connector_cap = effective;

        for a in inputs.ambient {
            for name in &a.tools {
                let d = self
                    .resolve_name(name, scope)
                    .ok_or_else(|| GrantError::UnknownTool {
                        field: "an ambient grant",
                        name: name.to_string(),
                        suggestion: None,
                    })?;
                g.offer(d, a.reason.clone(), false, inputs.available);
            }
        }
        let (dispatch, join) = (
            crate::launch::dispatch::DESCRIPTOR.name,
            crate::launch::join::DESCRIPTOR.name,
        );
        if g.offers(dispatch) && !g.offers(join) {
            if let Some(d) = self.descriptor(join) {
                g.offer(d, GrantReason::Implied(dispatch), false, inputs.available);
            }
        }
        Ok(g)
    }

    /// Expand one grammar entry. `field` names where it was written, for
    /// the error.
    fn expand(
        &self,
        entry: &str,
        scope: AliasScope,
        field: &'static str,
    ) -> Result<Expansion, GrantError> {
        if entry == "*" {
            // Every capability, but never authority over other agents (D22).
            return Ok(Expansion::Wildcard(
                self.descriptors().filter(|d| d.wildcard).collect(),
            ));
        }
        if entry == CORE_NAMESPACE {
            return Ok(Expansion::Wildcard(
                self.descriptors().filter(|d| d.is_core()).collect(),
            ));
        }
        if let Some(ns) = entry.strip_suffix(".*") {
            if !ns.is_empty() && !ns.contains('*') {
                let prefix = format!("{ns}.");
                let ds: Vec<_> = self
                    .descriptors()
                    .filter(|d| d.name.starts_with(&prefix))
                    .collect();
                if ds.is_empty() {
                    return Err(GrantError::UnknownNamespace {
                        field,
                        name: entry.to_string(),
                        suggestion: self.suggest(entry),
                    });
                }
                return Ok(Expansion::Wildcard(ds));
            }
        }
        if !entry.contains('*') {
            if let Some(d) = self.resolve_name(entry, scope) {
                return Ok(Expansion::Exact(d));
            }
        }
        Err(GrantError::UnknownTool {
            field,
            name: entry.to_string(),
            suggestion: self.suggest(entry),
        })
    }

    /// The closest known spelling to `typo` within edit distance 2, among
    /// canonical names, aliases and namespace wildcards, formatted for an
    /// error (`"report_finding" → findings.report`).
    fn suggest(&self, typo: &str) -> Option<String> {
        let typo = typo.to_ascii_lowercase();
        let mut candidates: Vec<(String, Option<&'static str>)> = Vec::new();
        for d in self.descriptors() {
            candidates.push((d.name.to_string(), None));
            for a in d.aliases {
                candidates.push((a.name.to_string(), Some(d.name)));
            }
        }
        let mut namespaces: BTreeSet<String> = self
            .descriptors()
            .filter_map(|d| d.name.split_once('.').map(|(ns, _)| format!("{ns}.*")))
            .collect();
        namespaces.insert(CORE_NAMESPACE.to_string());
        candidates.extend(namespaces.into_iter().map(|n| (n, None)));

        let mut best: Option<(usize, &(String, Option<&'static str>))> = None;
        for c in &candidates {
            let dist = edit_distance(&typo, &c.0.to_ascii_lowercase());
            if dist <= 2 && best.is_none_or(|(b, _)| dist < b) {
                best = Some((dist, c));
            }
        }
        best.map(|(_, (name, canonical))| match canonical {
            Some(c) => format!("\"{name}\" → {c}"),
            None => format!("\"{name}\""),
        })
    }
}

/// Levenshtein distance over chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::edit_distance;

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("repot_finding", "report_finding"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
        assert_eq!(edit_distance("bahs", "bash"), 2);
    }
}
