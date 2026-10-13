# Agent Skills Plan 2a: skills in a run

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An agent that declares `skills:` / `preloadSkills:` gets those skills in every run the assembler builds: an index (or preloaded bodies) in its system prompt, a `skills.load` tool, read access to the skills' files, `skill_index` / `skill_load` transcript events, and its loaded skills re-attached after compaction.

**Architecture:** `rupu-skills` gains the run-level model (`ResolvedSkills`: one `resolve_grants` call over every request, preload marked by membership) and the re-attach renderer. The assembler resolves a run's skills first, fails closed with `AssembleError::Skills`, and puts them on the tool context as `ToolServices.skills` plus `WorkspaceScope.read_roots`. `rupu-tools` adds `Service::Skills`, `AmbientGrant::skills()` and the `skills.load` body; the four fs read tools accept paths under a read root. The runner writes the events, keeps the run's loaded set, and re-attaches it in `compact_context`. Replay rebuilds the loaded set so a continued run keeps it. Workflow-step skills and placed units are Plan 2b; sessions are Plan 3.

**Tech Stack:** Rust 2021, the existing crates; no new third-party dependencies.

**Spec:** `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md` §2.1, §2.3, §2.4, §3.1 to §3.5, §4, §6, §10, §11. Read it, and Plan 1's `crates/rupu-skills`, before starting.

## Global Constraints

- Workspace deps only (`rupu-skills = { workspace = true }` in each crate that needs it). `rupu-skills` stays a leaf (no rupu deps).
- `rupu-cli` stays thin; `AgentRunOpts` and `ToolContext` are built only in `rupu-runtime/src/assembly` (`tests/it/no_hand_built_run_opts.rs`).
- Integration tests: one `tests/it` binary per crate. Tests that set `RUPU_MOCK_PROVIDER_SCRIPT` (`MockScript::set`) are `#[serial]`.
- `skills.load` declares `needs: &[Service::Skills]`. That keeps it out of `action:` steps (`ACTION_SERVICES` is Scm + Findings) and out of `rupu mcp serve`.
- A skill never widens the run's tool grant or permission mode (S5). Its files are read-only to the model; scripts run through `bash` under the run's mode.
- `*` covers the global layer only; a run binds each name once (spec §2.1).
- Model-facing text comes from `rupu_skills::render`, byte-pinned by its tests.
- Format only touched files (`rustfmt --edition 2021 <file>`); never package-wide `cargo fmt`. Never bare `git stash`.
- Fixture skills are invented (public repo).

## Review Focus

1. **A run whose project `.rupu` is the global dir** (`project_root_for` resolved to `$HOME`) sees each global skill once, not twice as "both layers". Pinned in Task 1.
2. **A skill edited between `skills.load` calls** comes back at its new version with a `skill_changed` notice, never a stale copy. Pinned in Task 3 and Task 7.
3. **Two compactions in one run** carry each loaded skill once, not twice. Pinned in Task 8.
4. **A continued run** (`rupu run --continue`) keeps the skills its predecessor loaded and re-attaches them at its next compaction. Pinned in Task 8.
5. **`read_file` on a skill's reference, then `write_file` to the same path**: the read succeeds, the write is refused, and the read writes no coverage touch. Pinned in Task 4.

---

## File Structure

| File | Change |
|---|---|
| `crates/rupu-skills/src/skill.rs` | `Skill.dir` (the directory as found) |
| `crates/rupu-skills/src/catalog.rs` | `LayerDirs::for_layers` drops a project layer that is the global dir (R5) |
| `crates/rupu-skills/src/run.rs` (new) | `SkillReason`, `SkillRequest`, `GrantedSkill`, `ResolvedSkills`, `resolve_run_skills`, `RunSkillsError` |
| `crates/rupu-skills/src/render.rs` | `REATTACH_MARKER`, `ReattachItem`, `render_reattach`, `CHANGED_NOTE` |
| `crates/rupu-agent/src/spec.rs`, `loader.rs` | `skills` / `preloadSkills` keys; `validate_skills`; `AgentLoadError::Skill` |
| `crates/rupu-tools/src/descriptor.rs`, `tool.rs`, `grant.rs`, `bodies.rs`, `catalog/mod.rs` | `Service::Skills`, `ToolServices.skills`, `WorkspaceScope.read_roots`, `AmbientGrant::skills()`, body + catalog entry |
| `crates/rupu-tools/src/skills/mod.rs` (new) | `skills.load` |
| `crates/rupu-tools/src/path_scope.rs`, `read_file.rs`, `grep.rs`, `glob.rs`, `ast_grep.rs` | read roots |
| `crates/rupu-transcript/src/event.rs`, `skills.rs` (new), `lib.rs` | `SkillIndex`, `SkillLoad` events + line renderers |
| `crates/rupu-runtime/src/assembly/{mod,spec}.rs` | `Overrides.skills/preload_skills`, `AssembleError::Skills`, resolution, prompt section, services |
| `crates/rupu-agent/src/grant.rs` | ambient skills grant |
| `crates/rupu-agent/src/runner.rs`, `replay.rs`, `continuation.rs` | events, loaded set, re-attach, replay |
| `rupu-cli` exhaustive `Event` matches | new arms |
| `docs/transcript-schema.md`, `docs/agent-format.md`, `docs/skills.md`, `CLAUDE.md`, the spec | docs |

---

### Task 1: Run-level skills in `rupu-skills`

**Files:**
- Modify: `crates/rupu-skills/src/skill.rs`, `catalog.rs`, `render.rs`, `lib.rs`
- Create: `crates/rupu-skills/src/run.rs`
- Test: `crates/rupu-skills/tests/it/run.rs` (add `mod run;` to `main.rs`), additions to `tests/it/render.rs` and `tests/it/catalog.rs`

**Interfaces:**
- Produces:
  - `Skill.dir: PathBuf`: the directory `load_skill` was given (a symlinked global skill's link path), so a reload validates the name against the right directory. `root` stays the canonical path.
  - `LayerDirs::for_layers(global, project)` returns `project: None` when the project `.rupu` canonicalizes to the global dir.
  - `SkillReason::{Agent, Step, Session, Operator}` (`Copy`, `Serialize` lowercase, `as_str()`).
  - `SkillRequest<'a> { reason: SkillReason, grants: &'a [String], preload: &'a [String] }`.
  - `GrantedSkill { skill: Arc<Skill>, preload: bool, indexed: bool, reason: SkillReason }` (`Clone`, `Debug`), `name(&self) -> &str`.
  - `ResolvedSkills` (`Clone`, `Debug`, `Default`): `entries() -> &[GrantedSkill]`, `is_empty()`, `get(name) -> Option<&GrantedSkill>`, `names() -> Vec<&str>`, `read_roots() -> Vec<PathBuf>`, `prompt_section() -> Option<String>`.
  - `resolve_run_skills(catalog: &SkillCatalog, requests: &[SkillRequest<'_>]) -> Result<ResolvedSkills, RunSkillsError>`.
  - `RunSkillsError::{Parse(GrantParseError), Resolve(Vec<ResolveError>)}`, `Display` joining resolve errors with `"; "`.
  - `render::REATTACH_MARKER: &str = "<skills-in-use>"`, `render::CHANGED_NOTE: &str`, `ReattachItem<'a> { name: &'a str, base_dir: &'a Path, body: &'a str }`, `render_reattach(items: &[ReattachItem<'_>], omitted: &[&str], missing: &[&str]) -> String`.

- [ ] **Step 1: Write the failing tests**

`tests/it/run.rs`:

```rust
use crate::support::write_skill;
use rupu_skills::run::{resolve_run_skills, SkillReason, SkillRequest};
use rupu_skills::{LayerDirs, SkillCatalog};
use std::fs;

fn catalog(g: &std::path::Path, p: Option<&std::path::Path>) -> SkillCatalog {
    SkillCatalog::discover(&LayerDirs::for_layers(g, p)).unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn nothing_requested_is_empty() {
    let g = tempfile::tempdir().unwrap();
    let r = resolve_run_skills(&catalog(g.path(), None), &[]).unwrap();
    assert!(r.is_empty());
    assert_eq!(r.prompt_section(), None);
}

#[test]
fn preload_is_marked_by_membership_and_reason_by_first_request() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "report-style", "report-style", "House style.");
    write_skill(&g.path().join("skills"), "code-review", "code-review", "Review a diff.");
    let cat = catalog(g.path(), None);
    let agent_grants = s(&["code-review"]);
    let step_preload = s(&["report-style"]);
    let r = resolve_run_skills(
        &cat,
        &[
            SkillRequest { reason: SkillReason::Agent, grants: &agent_grants, preload: &[] },
            SkillRequest { reason: SkillReason::Step, grants: &[], preload: &step_preload },
        ],
    )
    .unwrap();
    let rows: Vec<(&str, bool, bool, SkillReason)> =
        r.entries().iter().map(|g| (g.name(), g.preload, g.indexed, g.reason)).collect();
    assert_eq!(
        rows,
        vec![
            ("code-review", false, true, SkillReason::Agent),
            ("report-style", true, false, SkillReason::Step),
        ]
    );
    let section = r.prompt_section().unwrap();
    assert!(section.starts_with("## Skill: report-style\n\nBase directory: "));
    assert!(section.contains("Do the report-style thing."));
    assert!(section.ends_with("- code-review: Review a diff."));
    assert_eq!(r.read_roots().len(), 2);
}

#[test]
fn a_name_bound_from_both_layers_across_requests_is_refused() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "x", "x", "g");
    write_skill(&p.path().join("skills"), "x", "x", "p");
    let cat = catalog(g.path(), Some(p.path()));
    let a = s(&["global:x"]);
    let b = s(&["project:x"]);
    let err = resolve_run_skills(
        &cat,
        &[
            SkillRequest { reason: SkillReason::Agent, grants: &a, preload: &[] },
            SkillRequest { reason: SkillReason::Agent, grants: &[], preload: &b },
        ],
    )
    .unwrap_err();
    assert!(err.to_string().contains("granted from both the global and the project layer"), "{err}");
}

#[test]
fn disable_model_invocation_is_granted_but_not_indexed() {
    let g = tempfile::tempdir().unwrap();
    let dir = write_skill(&g.path().join("skills"), "deploy", "deploy", "Ship it.");
    fs::write(
        dir.join("SKILL.md"),
        "---\nname: deploy\ndescription: Ship it.\ndisable-model-invocation: true\n---\nSteps.\n",
    )
    .unwrap();
    let grants = s(&["deploy"]);
    let r = resolve_run_skills(
        &catalog(g.path(), None),
        &[SkillRequest { reason: SkillReason::Agent, grants: &grants, preload: &[] }],
    )
    .unwrap();
    assert!(!r.entries()[0].indexed);
    assert_eq!(r.prompt_section(), None);
}
```

`tests/it/catalog.rs`, append:

```rust
#[test]
fn a_project_layer_that_is_the_global_dir_is_dropped() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "only", "only", "d");
    let dirs = rupu_skills::LayerDirs::for_layers(g.path(), Some(g.path()));
    assert_eq!(dirs.project, None);
    let cat = rupu_skills::SkillCatalog::discover(&dirs).unwrap();
    assert_eq!(cat.named("only").count(), 1);
}

#[cfg(unix)]
#[test]
fn skill_dir_is_the_path_as_found() {
    let t = tempfile::tempdir().unwrap();
    let real = write_skill(&t.path().join("elsewhere"), "linked", "linked", "d");
    let skills = t.path().join("skills");
    fs::create_dir_all(&skills).unwrap();
    std::os::unix::fs::symlink(&real, skills.join("linked")).unwrap();
    let s = rupu_skills::load_skill(&skills.join("linked"), rupu_skills::Layer::Global).unwrap();
    assert_eq!(s.dir, skills.join("linked"));
    assert_eq!(s.root, real.canonicalize().unwrap());
}
```

`tests/it/render.rs`, append:

```rust
#[test]
fn reattach_block_is_exact() {
    use rupu_skills::render::{render_reattach, ReattachItem, REATTACH_MARKER};
    let got = render_reattach(
        &[ReattachItem { name: "pdf", base_dir: Path::new("/h/skills/pdf"), body: "Step one." }],
        &["docx"],
        &["gone"],
    );
    assert_eq!(
        got,
        "<skills-in-use>\n\
You loaded these skills earlier in this run. Their instructions follow at their current version; keep following them.\n\n\
## Skill: pdf\n\nBase directory: /h/skills/pdf\n\nStep one.\n\n\
Not re-attached for space (reload with `skills.load` when you need them): docx\n\
No longer installed: gone\n\
</skills-in-use>"
    );
    assert!(got.starts_with(REATTACH_MARKER));
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p rupu-skills --test it run:: catalog:: render::`
Expected: compile errors (`run` module, `Skill.dir`, `render_reattach` missing).

- [ ] **Step 3: Implement**

`skill.rs`: add `pub dir: PathBuf,` to `Skill` (after `root`), and set `dir: dir.to_owned(),` in `load_skill`'s constructor.

`catalog.rs`, `LayerDirs::for_layers`:

```rust
    /// The `skills/` directories under the global `.rupu` and, when the run
    /// has one, the project's. A project `.rupu` that IS the global dir
    /// (`project_root_for` resolved to `$HOME`) is no project layer (R5).
    pub fn for_layers(global_rupu_dir: &Path, project_rupu_dir: Option<&Path>) -> Self {
        let same = |p: &Path| match (p.canonicalize(), global_rupu_dir.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => p == global_rupu_dir,
        };
        Self {
            global: Some(global_rupu_dir.join("skills")),
            project: project_rupu_dir.filter(|p| !same(p)).map(|p| p.join("skills")),
        }
    }
```

`run.rs`:

```rust
//! The skills one run is given (spec §2.4). Every request a run carries
//! (the agent's keys, a workflow step's, a session's, an operator's) is
//! resolved in ONE `resolve_grants` call over their union, so a name bound
//! from both layers across requests is caught; preload is then marked by
//! membership and the reason is the first request that names the skill.

use crate::catalog::SkillCatalog;
use crate::grant::{parse_grants, resolve_grants, GrantParseError, ResolveError};
use crate::render::{render_skills_section, IndexItem, PreloadItem};
use crate::skill::Skill;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillReason {
    Agent,
    Step,
    Session,
    Operator,
}

impl SkillReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkillReason::Agent => "agent",
            SkillReason::Step => "step",
            SkillReason::Session => "session",
            SkillReason::Operator => "operator",
        }
    }
}

pub struct SkillRequest<'a> {
    pub reason: SkillReason,
    pub grants: &'a [String],
    pub preload: &'a [String],
}

#[derive(Debug, Clone)]
pub struct GrantedSkill {
    pub skill: Arc<Skill>,
    pub preload: bool,
    pub indexed: bool,
    pub reason: SkillReason,
}

impl GrantedSkill {
    pub fn name(&self) -> &str {
        self.skill.name.as_str()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedSkills {
    entries: Vec<GrantedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunSkillsError {
    #[error(transparent)]
    Parse(#[from] GrantParseError),
    #[error("{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Resolve(Vec<ResolveError>),
}

pub fn resolve_run_skills(
    catalog: &SkillCatalog,
    requests: &[SkillRequest<'_>],
) -> Result<ResolvedSkills, RunSkillsError> {
    let all: Vec<String> = requests
        .iter()
        .flat_map(|r| r.grants.iter().chain(r.preload.iter()))
        .cloned()
        .collect();
    if all.is_empty() {
        return Ok(ResolvedSkills::default());
    }
    let skills = resolve_grants(&parse_grants(&all)?, catalog).map_err(RunSkillsError::Resolve)?;
    let covers = |entries: &[String], skill: &Skill| -> Result<bool, RunSkillsError> {
        if entries.is_empty() {
            return Ok(false);
        }
        let picked = resolve_grants(&parse_grants(entries)?, catalog).map_err(RunSkillsError::Resolve)?;
        Ok(picked.iter().any(|s| s.name == skill.name && s.layer == skill.layer))
    };
    let mut entries = Vec::with_capacity(skills.len());
    for skill in skills {
        let mut reason = None;
        let mut preload = false;
        for r in requests {
            let in_preload = covers(r.preload, &skill)?;
            if reason.is_none() && (in_preload || covers(r.grants, &skill)?) {
                reason = Some(r.reason);
            }
            preload |= in_preload;
        }
        let indexed = !preload && !skill.frontmatter.disable_model_invocation;
        entries.push(GrantedSkill { skill, preload, indexed, reason: reason.unwrap_or(SkillReason::Agent) });
    }
    Ok(ResolvedSkills { entries })
}

impl ResolvedSkills {
    pub fn entries(&self) -> &[GrantedSkill] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&GrantedSkill> {
        self.entries.iter().find(|g| g.name() == name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(GrantedSkill::name).collect()
    }

    /// Every granted skill's canonical root: the directories the fs read
    /// tools may read outside the workspace (spec §3.4).
    pub fn read_roots(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|g| g.skill.root.clone()).collect()
    }

    /// The system prompt's skills section: preloaded bodies, then the index.
    pub fn prompt_section(&self) -> Option<String> {
        let preloads: Vec<PreloadItem<'_>> = self
            .entries
            .iter()
            .filter(|g| g.preload)
            .map(|g| PreloadItem { name: g.name(), base_dir: &g.skill.root, body: &g.skill.body })
            .collect();
        let index: Vec<IndexItem<'_>> = self
            .entries
            .iter()
            .filter(|g| g.indexed)
            .map(|g| IndexItem { name: g.name(), description: &g.skill.frontmatter.description })
            .collect();
        render_skills_section(&preloads, &index)
    }
}
```

`render.rs`, append:

```rust
/// Opens the block compaction re-attaches (spec §6). `compact_messages`
/// skips a text block that starts with it when it rebuilds the task text,
/// so a second compaction never copies the block.
pub const REATTACH_MARKER: &str = "<skills-in-use>";

/// Leads a `skills.load` result for a skill that changed on disk since the
/// run's index was built.
pub const CHANGED_NOTE: &str =
    "This skill changed on disk since this run started; this is its current version.";

pub struct ReattachItem<'a> {
    pub name: &'a str,
    pub base_dir: &'a Path,
    pub body: &'a str,
}

pub fn render_reattach(items: &[ReattachItem<'_>], omitted: &[&str], missing: &[&str]) -> String {
    let mut out = format!(
        "{REATTACH_MARKER}\nYou loaded these skills earlier in this run. Their instructions follow at their current version; keep following them."
    );
    for i in items {
        out.push_str(&format!("\n\n## Skill: {}\n\nBase directory: {}", i.name, i.base_dir.display()));
        if !i.body.is_empty() {
            out.push_str("\n\n");
            out.push_str(i.body);
        }
    }
    if !omitted.is_empty() || !missing.is_empty() {
        out.push('\n');
    }
    if !omitted.is_empty() {
        out.push_str(&format!(
            "\nNot re-attached for space (reload with `skills.load` when you need them): {}",
            omitted.join(", ")
        ));
    }
    if !missing.is_empty() {
        out.push_str(&format!("\nNo longer installed: {}", missing.join(", ")));
    }
    out.push_str("\n</skills-in-use>");
    out
}
```

If Plan 1's `render_load_result` hardcodes its own "changed on disk" text, keep it as is; `CHANGED_NOTE` is what callers pass as `changed_note`.

`lib.rs`: `pub mod run;` and `pub use run::{resolve_run_skills, GrantedSkill, ResolvedSkills, RunSkillsError, SkillReason, SkillRequest};`.

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-skills --test it`
Expected: all pass (Plan 1's 102 + the new ones).

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-skills/src/{run,render,catalog,skill,lib}.rs crates/rupu-skills/tests/it/{run,render,catalog,main}.rs
git add crates/rupu-skills
git commit -m "feat(skills): resolve a run's skills in one pass and render the re-attach block"
```

---

### Task 2: `skills:` / `preloadSkills:` on agents

**Files:**
- Modify: `crates/rupu-agent/Cargo.toml` (`rupu-skills = { workspace = true }`), `src/spec.rs`, `src/loader.rs`, `crates/rupu-cli/src/cmd/session.rs:7750` (`session_agent_spec` literal)
- Test: `crates/rupu-agent/tests/it/agent_skills.rs` (add to `tests/it/main.rs`), `spec.rs` unit tests

**Interfaces:**
- Produces: `AgentSpec.skills: Option<Vec<String>>`, `AgentSpec.preload_skills: Option<Vec<String>>`; `AgentSpec::declares_skills(&self) -> bool`; `AgentSpec::validate_skills(&self, catalog: &rupu_skills::SkillCatalog) -> Result<(), rupu_skills::RunSkillsError>`; `AgentLoadError::Skill { agent: String, key: &'static str, source: SkillCheckError }` (`SkillCheckError::{Discover, Resolve}`, typed so `is_fd_exhaustion` sees an EMFILE behind it; `key` is `preloadSkills` only when that key's entries alone fail). Discovery lists the skill directories only; each `CatalogEntry::skill()` loads on first use and caches for the catalog's life, so a check loads only the skills its grants name.

- [ ] **Step 1: Write the failing tests**

`spec.rs` unit test (next to `parses_findings_profile`):

```rust
#[test]
fn parses_skill_keys() {
    let s = AgentSpec::parse(
        "---\nname: a\nskills: [code-review, \"project:*\"]\npreloadSkills: [report-style]\n---\nbody\n",
    )
    .unwrap();
    assert_eq!(s.skills, Some(vec!["code-review".to_string(), "project:*".to_string()]));
    assert_eq!(s.preload_skills, Some(vec!["report-style".to_string()]));
    assert!(s.declares_skills());
    assert!(!AgentSpec::parse("---\nname: b\n---\nbody\n").unwrap().declares_skills());
}
```

`tests/it/agent_skills.rs`:

```rust
use rupu_agent::{check_agent_files, load_agent, AgentLoadError};
use std::fs;
use std::path::Path;

fn skill(skills_dir: &Path, name: &str) {
    let d = skills_dir.join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\nbody\n")).unwrap();
}

fn agent(global: &Path, body: &str) {
    fs::create_dir_all(global.join("agents")).unwrap();
    fs::write(global.join("agents/a.md"), format!("---\nname: a\n{body}---\nprompt\n")).unwrap();
}

#[test]
fn load_agent_accepts_installed_skills() {
    let g = tempfile::tempdir().unwrap();
    skill(&g.path().join("skills"), "code-review");
    agent(g.path(), "skills: [code-review]\n");
    assert_eq!(load_agent(g.path(), None, "a").unwrap().skills, Some(vec!["code-review".into()]));
}

#[test]
fn load_agent_refuses_an_unknown_skill_with_a_suggestion() {
    let g = tempfile::tempdir().unwrap();
    skill(&g.path().join("skills"), "code-review");
    agent(g.path(), "preloadSkills: [code-reviw]\n");
    match load_agent(g.path(), None, "a") {
        Err(AgentLoadError::Skill { agent, message }) => {
            assert_eq!(agent, "a");
            assert!(message.contains("did you mean code-review"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn project_skills_need_an_exact_grant() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    skill(&p.path().join("skills"), "local-only");
    agent(g.path(), "skills: [\"*\", local-only]\n");
    assert!(load_agent(g.path(), Some(p.path()), "a").is_ok());
}

#[test]
fn check_agent_files_reports_skill_errors() {
    let g = tempfile::tempdir().unwrap();
    agent(g.path(), "skills: [missing]\n");
    let checks = check_agent_files(g.path(), None);
    assert!(matches!(&checks[0].error, Some(AgentLoadError::Skill { .. })));
}
```

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-agent --test it agent_skills::` and `cargo test -p rupu-agent --lib parses_skill_keys`
Expected: compile errors.

- [ ] **Step 3: Implement**

`spec.rs`, `Frontmatter` (after `findings_profile`):

```rust
    /// Skills this agent may load (`skills.load`): exact names, `global:x` /
    /// `project:x`, `*` (every global skill) or `project:*`.
    #[serde(default)]
    skills: Option<Vec<String>>,
    /// Skills whose bodies are always in this agent's system prompt.
    #[serde(default, rename = "preloadSkills")]
    preload_skills: Option<Vec<String>>,
```

`AgentSpec` gains `pub skills: Option<Vec<String>>, pub preload_skills: Option<Vec<String>>,`; `parse` maps `skills: fm.skills, preload_skills: fm.preload_skills,`. Methods:

```rust
    pub fn declares_skills(&self) -> bool {
        self.skills.as_ref().is_some_and(|v| !v.is_empty())
            || self.preload_skills.as_ref().is_some_and(|v| !v.is_empty())
    }

    /// Resolve the agent's own skill keys against `catalog` (spec §2.1).
    pub fn validate_skills(&self, catalog: &rupu_skills::SkillCatalog) -> Result<(), rupu_skills::RunSkillsError> {
        let grants = self.skills.clone().unwrap_or_default();
        let preload = self.preload_skills.clone().unwrap_or_default();
        rupu_skills::resolve_run_skills(
            catalog,
            &[rupu_skills::SkillRequest { reason: rupu_skills::SkillReason::Agent, grants: &grants, preload: &preload }],
        )
        .map(|_| ())
    }
```

`loader.rs`: add the variant

```rust
    #[error("agent `{agent}`: skills: {message}")]
    Skill { agent: String, message: String },
```

and a helper used by `load_agent` and `check_agent_files` (discover once per call, only when an agent declares skills):

```rust
fn check_skills(
    spec: &AgentSpec,
    global: &Path,
    project: Option<&Path>,
    catalog: &mut Option<rupu_skills::SkillCatalog>,
) -> Result<(), AgentLoadError> {
    if !spec.declares_skills() {
        return Ok(());
    }
    let err = |message: String| AgentLoadError::Skill { agent: spec.name.clone(), message };
    if catalog.is_none() {
        let dirs = rupu_skills::LayerDirs::for_layers(global, project);
        *catalog = Some(rupu_skills::SkillCatalog::discover(&dirs).map_err(|e| err(e.to_string()))?);
    }
    spec.validate_skills(catalog.as_ref().expect("discovered above")).map_err(|e| err(e.to_string()))
}
```

`load_agent` calls `check_skills(&spec, global, project, &mut None)?` after `validate_tools`. `check_agent_files` keeps one `let mut catalog = None;` across files and, for a file whose tools validated, sets its error from `check_skills(...)`. `fd_budget::load_agent_admitted` goes through `load_agent`, so it needs nothing.

`session.rs:7750` `session_agent_spec`: add `skills: None, preload_skills: None,` (sessions get skills in Plan 3; the record carries none yet).

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-agent --test it agent_skills::`, `cargo test -p rupu-agent --lib parses_skill_keys`, `cargo build -p rupu-cli`
Expected: pass; the CLI builds.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-agent/src/spec.rs crates/rupu-agent/src/loader.rs crates/rupu-agent/tests/it/agent_skills.rs crates/rupu-agent/tests/it/main.rs
git add crates/rupu-agent crates/rupu-cli/src/cmd/session.rs Cargo.lock
git commit -m "feat(agent): skills and preloadSkills frontmatter, checked at load"
```

(For `session.rs`, add only the two-field change; do not reformat the file.)

---

### Task 3: `Service::Skills`, the ambient grant and `skills.load`

**Files:**
- Modify: `crates/rupu-tools/Cargo.toml` (`rupu-skills = { workspace = true }`), `src/descriptor.rs`, `src/tool.rs`, `src/grant.rs`, `src/bodies.rs`, `src/catalog/mod.rs`, `src/lib.rs`
- Create: `crates/rupu-tools/src/skills/mod.rs`
- Modify: `crates/rupu-agent/src/grant.rs` (`resolve_run_grant`)
- Test: `crates/rupu-tools/tests/it/skills_load.rs` (add to `main.rs`), `tests/it/descriptors.rs`, `tests/it/support.rs`; `crates/rupu-agent/tests/it/tool_grant.rs`

**Interfaces:**
- Consumes: `rupu_skills::{ResolvedSkills, load_skill, render::{render_load_result, LoadView, CHANGED_NOTE}, tools_map::mapped_tools}`.
- Produces:
  - `Service::Skills` (`as_str` → `"skills"`).
  - `ToolServices.skills: Option<Arc<rupu_skills::ResolvedSkills>>`; `provided()` inserts `Service::Skills` when it is `Some`.
  - `AmbientGrant::skills()` → `{ tools: vec!["skills.load"], reason: GrantReason::Ambient("skills"), self_served: false }`.
  - `skills::LOAD: ToolDescriptor` (`skills.load`) and `skills::SkillsLoadTool`. Its successful output's `structured` is `{"skill", "layer", "content_hash", "base_dir", "changed", "allowed_tools"}`; the runner (Task 7) reads exactly these keys.

- [ ] **Step 1: Write the failing tests**

`tests/it/support.rs`, add a helper:

```rust
/// A `ResolvedSkills` holding the global skills named in `grants`, all
/// installed under `<global>/skills/` by the caller.
pub fn resolved(global: &std::path::Path, grants: &[&str]) -> std::sync::Arc<rupu_skills::ResolvedSkills> {
    let cat = rupu_skills::SkillCatalog::discover(&rupu_skills::LayerDirs::for_layers(global, None)).unwrap();
    let g: Vec<String> = grants.iter().map(|s| s.to_string()).collect();
    std::sync::Arc::new(
        rupu_skills::resolve_run_skills(
            &cat,
            &[rupu_skills::SkillRequest { reason: rupu_skills::SkillReason::Agent, grants: &g, preload: &[] }],
        )
        .unwrap(),
    )
}
```

`tests/it/skills_load.rs`:

```rust
use crate::support::resolved;
use rupu_tools::{Tool, ToolContext};
use serde_json::json;
use std::fs;

fn install(global: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let d = global.join("skills").join(name);
    fs::create_dir_all(d.join("references")).unwrap();
    fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\nallowed-tools: Bash(jq:*) Read\n---\n{body}\n"),
    )
    .unwrap();
    fs::write(d.join("references/REF.md"), "ref\n").unwrap();
    d
}

fn ctx(global: &std::path::Path, grants: &[&str]) -> ToolContext {
    let mut c = ToolContext::in_workspace(global);
    c.services.skills = Some(resolved(global, grants));
    c
}

#[tokio::test]
async fn loads_a_granted_skill() {
    let g = tempfile::tempdir().unwrap();
    let dir = install(g.path(), "pdf", "Step one.");
    let out = rupu_tools::skills::SkillsLoadTool.invoke(json!({"name": "pdf"}), &ctx(g.path(), &["pdf"])).await.unwrap();
    assert_eq!(out.error, None);
    assert!(out.stdout.starts_with("Skill: pdf (version "), "{}", out.stdout);
    assert!(out.stdout.contains(&format!("Base directory: {}", dir.canonicalize().unwrap().display())));
    assert!(out.stdout.contains("Files: references/REF.md"));
    assert!(out.stdout.ends_with("Step one."));
    let s = out.structured.unwrap();
    assert_eq!(s["skill"], "pdf");
    assert_eq!(s["layer"], "global");
    assert_eq!(s["changed"], false);
    assert_eq!(s["allowed_tools"], json!(["bash", "read_file"]));
    assert_eq!(s["content_hash"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn an_ungranted_skill_is_refused_with_the_granted_names() {
    let g = tempfile::tempdir().unwrap();
    install(g.path(), "pdf", "x");
    install(g.path(), "docx", "x");
    let out = rupu_tools::skills::SkillsLoadTool.invoke(json!({"name": "docx"}), &ctx(g.path(), &["pdf"])).await.unwrap();
    let err = out.error.unwrap();
    assert!(err.contains("docx") && err.contains("pdf"), "{err}");
}

#[tokio::test]
async fn an_edited_skill_comes_back_current_with_a_note() {
    let g = tempfile::tempdir().unwrap();
    let dir = install(g.path(), "pdf", "Old.");
    let c = ctx(g.path(), &["pdf"]);
    fs::write(dir.join("SKILL.md"), "---\nname: pdf\ndescription: d\n---\nNew text that is longer.\n").unwrap();
    let out = rupu_tools::skills::SkillsLoadTool.invoke(json!({"name": "pdf"}), &c).await.unwrap();
    assert!(out.stdout.starts_with(rupu_skills::render::CHANGED_NOTE), "{}", out.stdout);
    assert!(out.stdout.ends_with("New text that is longer."));
    assert_eq!(out.structured.unwrap()["changed"], true);
}
```

`tests/it/descriptors.rs`:
- Append `include_str!("../../src/skills/mod.rs"),` to `production_impls()`.
- In `full_context()`, add `s.skills = Some(std::sync::Arc::new(rupu_skills::ResolvedSkills::default()));`.

`crates/rupu-agent/tests/it/tool_grant.rs`, add a test modelled on `ambient_is_visible`: build the opts with `tool_context.services.skills = Some(...)` (one installed skill), resolve via `with_grant`, and assert the grant offers `skills.load` with reason `"ambient:skills"`; and that without `services.skills` it is not offered even under `tools: ["*"]`.

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-tools --test it skills_load:: descriptors::`, `cargo test -p rupu-agent --test it tool_grant::`
Expected: compile errors.

- [ ] **Step 3: Implement**

`descriptor.rs`: add `Skills` to `Service` and `Service::Skills => "skills",` to `as_str`.

`tool.rs`: add to `ToolServices` (after `catalog`):

```rust
    /// The skills this run was granted (spec §2.4); `skills.load` reads it.
    pub skills: Option<Arc<rupu_skills::ResolvedSkills>>,
```

its `Debug` line `.field("skills", &self.skills.as_ref().map(|s| s.names()))`, and in `provided()`: `if self.skills.is_some() { s.insert(Service::Skills); }`.

`grant.rs` (rupu-tools), next to `engagement()`:

```rust
    /// A run with skills is offered the loader (spec §2.4).
    pub fn skills() -> Self {
        Self { tools: vec!["skills.load"], reason: GrantReason::Ambient("skills"), self_served: false }
    }
```

`skills/mod.rs`:

```rust
//! `skills.load` (spec §3.3): the body of one of the run's granted skills,
//! re-read from disk so an edit since the run started is never hidden.

use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::output::{failed, ok, req_str};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_skills::render::{render_load_result, LoadView, CHANGED_NOTE};
use serde_json::{json, Value};
use std::time::Instant;

pub static LOAD: ToolDescriptor = ToolDescriptor {
    name: "skills.load",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Skills],
    uses: &[],
    wildcard: true,
    description: "Load one of your skills by name. Returns its instructions, its base \
                  directory and its files. Paths inside a skill are relative to its base \
                  directory; read them with read_file and run its scripts with bash. Load a \
                  skill before you start a task its description matches.",
    input_schema: schema,
};

fn schema() -> Value {
    json!({
        "type": "object",
        "required": ["name"],
        "properties": { "name": { "type": "string", "description": "The skill's name, as listed under ## Skills." } }
    })
}

pub struct SkillsLoadTool;

#[async_trait]
impl Tool for SkillsLoadTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &LOAD
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let name = req_str(&input, "name")?;
        let skills = ctx
            .services
            .skills
            .as_ref()
            .ok_or_else(|| ToolError::Execution("this run has no skills".into()))?;
        let Some(granted) = skills.get(name) else {
            return Ok(failed(format!(
                "no skill named {name:?} is available to this run; available: {}",
                skills.names().join(", ")
            ))
            .timed(started));
        };
        let fresh = match rupu_skills::load_skill(&granted.skill.dir, granted.skill.layer) {
            Ok(s) => s,
            Err(e) => return Ok(failed(format!("skill {name:?} can no longer be loaded: {e}")).timed(started)),
        };
        let hash = match fresh.content_hash() {
            Ok(h) => h,
            Err(e) => return Ok(failed(format!("skill {name:?} can no longer be read: {e}")).timed(started)),
        };
        let changed = fresh.stamp != granted.skill.stamp;
        let files = fresh.files();
        let text = render_load_result(&LoadView {
            name,
            version: hash,
            base_dir: &fresh.root,
            files: &files,
            compatibility: fresh.frontmatter.compatibility.as_deref(),
            changed_note: changed.then_some(CHANGED_NOTE),
            body: &fresh.body,
        });
        let mut out = ok(text);
        out.structured = Some(json!({
            "skill": name,
            "layer": fresh.layer.as_str(),
            "content_hash": hash.hex(),
            "base_dir": fresh.root,
            "changed": changed,
            "allowed_tools": rupu_skills::tools_map::mapped_tools(&fresh.frontmatter.allowed_tools),
        }));
        Ok(out.timed(started))
    }
}
```

`catalog/mod.rs` `ALL`: add `// a run's skills` then `&crate::skills::LOAD,` before the agentiflow launch tools. `lib.rs`: `pub mod skills;` (alphabetical). `bodies.rs`: `let skills = services.skills.is_some();` and `"skills.load" if skills => Arc::new(skills::SkillsLoadTool),` (add `skills` to the `use crate::{…}` list).

`crates/rupu-agent/src/grant.rs` `resolve_run_grant`, after the engagement push:

```rust
    if ctx.services.skills.is_some() {
        ambient.push(AmbientGrant::skills());
    }
```

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-tools --test it`, `cargo test -p rupu-agent --test it tool_grant::`, `cargo test -p rupu-mcp --test it`, `cargo test -p rupu-cp --lib api::tools`
Expected: pass. `action_eligibility_is_connectors_and_findings` still counts 22 (Skills is not an action service).

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-tools/src/skills/mod.rs crates/rupu-tools/tests/it/skills_load.rs
git add crates/rupu-tools crates/rupu-agent/src/grant.rs crates/rupu-agent/tests/it/tool_grant.rs Cargo.lock
git commit -m "feat(tools): skills.load, Service::Skills and the ambient skills grant"
```

(Run rustfmt only on new files; for edited files with drift, check `git diff` shows only your lines.)

---

### Task 4: Read roots for the fs read tools

**Files:**
- Modify: `crates/rupu-tools/src/tool.rs` (`WorkspaceScope.read_roots`), `src/path_scope.rs`, `src/read_file.rs`, `src/grep.rs`, `src/glob.rs`, `src/ast_grep.rs`
- Modify: `crates/rupu-runtime/src/assembly/mod.rs` (the two `WorkspaceScope { .. }` literals: add `read_roots: Vec::new(),` for now; Task 6 fills the run's)
- Test: `crates/rupu-tools/tests/it/read_roots.rs` (add to `main.rs`); snapshot updates in `crates/rupu-agentiflow/tests/it/snapshots/`

**Interfaces:**
- Produces: `WorkspaceScope.read_roots: Vec<PathBuf>` (`#[serde(default)]`); `path_scope::is_readable(ws: &WorkspaceScope, candidate: &Path) -> bool`.

- [ ] **Step 1: Write the failing test** (`tests/it/read_roots.rs`)

```rust
use rupu_tools::{Tool, ToolContext};
use serde_json::json;
use std::fs;

fn setup() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf, ToolContext) {
    let ws = tempfile::tempdir().unwrap();
    let skills = tempfile::tempdir().unwrap();
    let root = skills.path().join("pdf");
    fs::create_dir_all(root.join("references")).unwrap();
    fs::write(root.join("references/REF.md"), "needle in a reference\n").unwrap();
    let root = root.canonicalize().unwrap();
    let mut ctx = ToolContext::in_workspace(ws.path());
    ctx.workspace.read_roots = vec![root.clone()];
    (ws, skills, root, ctx)
}

#[tokio::test]
async fn read_file_reads_under_a_read_root_but_write_file_refuses() {
    let (_ws, _s, root, ctx) = setup();
    let path = root.join("references/REF.md");
    let out = rupu_tools::ReadFileTool.invoke(json!({"path": path}), &ctx).await.unwrap();
    assert_eq!(out.error, None, "{:?}", out.error);
    assert!(out.stdout.contains("needle in a reference"));
    let w = rupu_tools::WriteFileTool.invoke(json!({"path": path, "content": "x"}), &ctx).await.unwrap();
    assert!(w.error.unwrap().contains("escapes workspace"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "needle in a reference\n");
}

#[tokio::test]
async fn paths_outside_every_root_stay_refused() {
    let (_ws, _s, _root, ctx) = setup();
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("secret"), "s").unwrap();
    let out = rupu_tools::ReadFileTool.invoke(json!({"path": other.path().join("secret")}), &ctx).await.unwrap();
    assert!(out.error.is_some());
}

#[tokio::test]
async fn grep_and_glob_search_a_read_root() {
    let (_ws, _s, root, ctx) = setup();
    let g = rupu_tools::GrepTool.invoke(json!({"pattern": "needle", "path": root}), &ctx).await.unwrap();
    assert_eq!(g.error, None);
    assert!(g.stdout.contains("REF.md"), "{}", g.stdout);
    let pattern = format!("{}/**/*.md", root.display());
    let gl = rupu_tools::GlobTool.invoke(json!({"pattern": pattern}), &ctx).await.unwrap();
    assert!(gl.stdout.contains(&root.join("references/REF.md").display().to_string()), "{}", gl.stdout);
}

#[tokio::test]
async fn reads_under_a_read_root_write_no_coverage_touch() {
    let (ws, _s, root, mut ctx) = setup();
    let (writer, ledger) = crate::support::coverage_writer(ws.path());
    ctx.services.coverage_writer = Some(writer);
    rupu_tools::ReadFileTool.invoke(json!({"path": root.join("references/REF.md")}), &ctx).await.unwrap();
    assert_eq!(crate::support::touches(&ledger), 0);
}
```

If `tests/it/support.rs` has no `coverage_writer` / `touches` helpers, build them from the pattern the existing `read_file` coverage tests use (search `coverage_writer` in `crates/rupu-tools/tests/it/`), returning the writer and a way to count `files` ledger lines.

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-tools --test it read_roots::`
Expected: compile error (`read_roots` missing), then failures.

- [ ] **Step 3: Implement**

`tool.rs` `WorkspaceScope`: add

```rust
    /// Directories outside the workspace the fs read tools may read: the
    /// run's granted skills (spec §3.4). Never writable.
    #[serde(default)]
    pub read_roots: Vec<PathBuf>,
```

and `read_roots: Vec::new(),` in its `Default`.

`path_scope.rs`:

```rust
/// Inside the workspace or one of the run's read roots (spec §3.4). The
/// read tools use this; `write_file` and `edit_file` keep `is_inside`.
pub(crate) fn is_readable(ws: &crate::tool::WorkspaceScope, candidate: &Path) -> bool {
    is_inside(&ws.path, candidate) || ws.read_roots.iter().any(|r| is_inside(r, candidate))
}
```

`read_file.rs`: replace the check with `if !is_readable(&ctx.workspace, &abs)`, keep the message, and guard the coverage `emit` with `if is_inside(&ctx.workspace.path, &abs)`. `ctx.workspace.path.join(&i.path)` already yields `i.path` when it is absolute.

`grep.rs` / `ast_grep.rs`: use `is_readable` for the `search_path` check; emit each per-file coverage event only when that file's path is inside the workspace (`strip_prefix(&ctx.workspace.path).is_ok()`), so a read-root search emits nothing.

`glob.rs`: choose the walk base before building the walker:

```rust
    let (base, pattern) = read_root_pattern(&ctx.workspace, &i.pattern)
        .unwrap_or_else(|| (ctx.workspace.path.clone(), i.pattern.clone()));
```

with

```rust
/// An absolute pattern under one of the run's read roots walks from that
/// root; anything else walks the workspace.
fn read_root_pattern(ws: &crate::tool::WorkspaceScope, pattern: &str) -> Option<(PathBuf, String)> {
    ws.read_roots.iter().find_map(|root| {
        let prefix = format!("{}/", root.display());
        pattern.strip_prefix(&prefix).map(|rest| (root.clone(), rest.to_owned()))
    })
}
```

Matches under a read root are reported as absolute paths (`base.join(rel)`), workspace matches stay relative, and only workspace matches emit coverage. Keep the `is_inside(&base, entry.path())` filter.

Descriptions: in each of the four tools, change the sentence about absolute or escaping paths to say that paths are relative to the workspace and that absolute paths are allowed only inside a skill's base directory (`skills.load` returns it). Then update the agentiflow toolset snapshots: `INSTA_UPDATE=always cargo test -p rupu-agentiflow --test it flow_toolset_parity::`, and review the `.snap` diffs: only those four descriptions may change.

`crates/rupu-runtime/src/assembly/mod.rs`: add `read_roots: Vec::new(),` to both `WorkspaceScope { id, path, bash }` literals (around `:347` and `:614`).

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-tools --test it`, `cargo test -p rupu-agentiflow --test it flow_toolset_parity::`, `cargo build -p rupu-runtime`
Expected: pass; the existing escape tests (`rejects_path_outside_workspace`, `glob_stays_in_workspace`, ast_grep's) still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-tools crates/rupu-runtime/src/assembly/mod.rs crates/rupu-agentiflow/tests/it/snapshots
git commit -m "feat(tools): read roots let the read tools reach a run's skill directories"
```

---

### Task 5: `skill_index` and `skill_load` transcript events

**Files:**
- Modify: `crates/rupu-transcript/src/event.rs`, `src/lib.rs`
- Create: `crates/rupu-transcript/src/skills.rs`
- Modify (new match arms): `crates/rupu-agent/src/replay.rs` (`:346` group), `crates/rupu-cli/src/cmd/session.rs` (`:2952`, `:5536`), `cmd/autoflow.rs:4695`, `cmd/transcript.rs` (`:1232`, `:1733`), `output/workflow_printer.rs` (`:2071`, `:3555`)
- Modify: `docs/transcript-schema.md`
- Test: `event.rs` roundtrip table; `crates/rupu-transcript/tests/it/` (or unit tests in `skills.rs`)

**Interfaces:**
- Produces:
  - `SkillIndexEntry { name: String, layer: String, stamp: String, preload: bool, indexed: bool, reason: String }` (`Debug, Clone, PartialEq, Eq, Serialize, Deserialize`).
  - `Event::SkillIndex { skills: Vec<SkillIndexEntry> }` (tag `skill_index`).
  - `Event::SkillLoad { name: String, layer: String, content_hash: String, base_dir: String, via: String }` (tag `skill_load`). `via` is one of `model`, `preload`, `operator`, `reattach`, `continuation`.
  - `skills::{skill_index_line(&[SkillIndexEntry]) -> String, skill_load_line(name: &str, via: &str, content_hash: &str) -> String}`.

- [ ] **Step 1: Write the failing tests**

In `event.rs`'s roundtrip table add:

```rust
(
    Event::SkillIndex { skills: vec![SkillIndexEntry { name: "pdf".into(), layer: "global".into(), stamp: "ab".repeat(32), preload: false, indexed: true, reason: "agent".into() }] },
    "skill_index",
),
(
    Event::SkillLoad { name: "pdf".into(), layer: "global".into(), content_hash: "cd".repeat(32), base_dir: "/h/skills/pdf".into(), via: "model".into() },
    "skill_load",
),
```

`skills.rs` unit tests:

```rust
#[test]
fn lines() {
    let e = |name: &str, preload, indexed| SkillIndexEntry {
        name: name.into(), layer: "global".into(), stamp: String::new(), preload, indexed, reason: "agent".into(),
    };
    assert_eq!(
        skill_index_line(&[e("a", true, false), e("b", false, true), e("c", false, false)]),
        "3 skills: a (preloaded), b, c (not indexed)"
    );
    assert_eq!(skill_load_line("pdf", "model", &"ab".repeat(32)), "loaded skill pdf (abababababab) via model");
}
```

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-transcript`
Expected: compile errors.

- [ ] **Step 3: Implement**

Add the struct (next to `ToolGrantEntry`), the two variants (after `ToolGrant`), `"skill_index", "skill_load"` to `KNOWN_EVENT_TAGS`, and extend the `Notice` doc comment's kinds with `skill_not_indexed`, `skill_changed`, `skill_missing`, `skill_tools_missing`, `skill_reattach_truncated`. `skills.rs`:

```rust
//! One-line renderers for the skills events, shared by every view.

use crate::event::SkillIndexEntry;

pub fn skill_index_line(entries: &[SkillIndexEntry]) -> String {
    let names: Vec<String> = entries
        .iter()
        .map(|e| match (e.preload, e.indexed) {
            (true, _) => format!("{} (preloaded)", e.name),
            (false, true) => e.name.clone(),
            (false, false) => format!("{} (not indexed)", e.name),
        })
        .collect();
    let noun = if entries.len() == 1 { "skill" } else { "skills" };
    format!("{} {noun}: {}", entries.len(), names.join(", "))
}

pub fn skill_load_line(name: &str, via: &str, content_hash: &str) -> String {
    let short: String = content_hash.chars().take(12).collect();
    format!("loaded skill {name} ({short}) via {via}")
}
```

`lib.rs`: `pub mod skills;` and re-export `SkillIndexEntry`.

Match arms: in `replay.rs`'s no-op group add `Event::SkillIndex { .. } | Event::SkillLoad { .. }` (Task 8 gives `SkillLoad` a real fold in a separate function, not here). In each `rupu-cli` view that renders `ToolGrant` with `tool_grant_line`, render `SkillIndex` the same way under the label `"skills"` with `skill_index_line`, and `SkillLoad` with `skill_load_line`; in the other exhaustive matches, add both to the no-op group. Follow each file's existing `ToolGrant` arm exactly.

`docs/transcript-schema.md`: add `### \`skill_index\`` and `### \`skill_load\`` sections after `tool_grant` (field tables + one JSON example each), add the five notice kinds to the `notice` kind cell, add `skill_index` / `skill_load` to the "added within version 2" sentence, and in "Event order guarantees" put `skill_index? / skill_load*  # preload` after `notice*  # tool_unavailable` and `skill_load?` after `tool_result` inside the turn. In "Replay", list both as not affecting the conversation (the loaded set is folded separately, Task 8).

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-transcript`, `cargo build -p rupu-cli`, `cargo test -p rupu-agent --test it replay`
Expected: pass; the CLI builds (every exhaustive match has the new arms).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-transcript crates/rupu-agent/src/replay.rs crates/rupu-cli/src docs/transcript-schema.md
git commit -m "feat(transcript): skill_index and skill_load events"
```

---

### Task 6: The assembler resolves a run's skills

**Files:**
- Modify: `crates/rupu-runtime/Cargo.toml` (`rupu-skills = { workspace = true }`), `src/assembly/spec.rs` (`Overrides`), `src/assembly/mod.rs` (`AssembleError`, `into_run_error`, `record_unstarted`, `assemble`, `system_prompt_for`)
- Modify: every other `system_prompt_for` caller (`grep -rn "system_prompt_for(" crates/`) to pass `None`
- Test: `crates/rupu-runtime/tests/it/assembly_skills.rs` (add to `main.rs`), `crates/rupu-runtime/tests/it/support.rs` (a `skill` helper)

**Interfaces:**
- Consumes: Task 1 (`resolve_run_skills`), Task 3 (`ToolServices.skills`), Task 4 (`read_roots`).
- Produces:
  - `Overrides.skills: Vec<String>`, `Overrides.preload_skills: Vec<String>`: a launch's own skill entries (a workflow step's in Plan 2b, `rupu run --skill`), resolved with `SkillReason::Step`.
  - `AssembleError::Skills(String)` (`#[error("skills: {0}")]`); `into_run_error` → `RunError::Preflight(e.to_string())`; `record_unstarted` maps it to `ProviderError::Preflight(e.to_string())` like `AgentLoad`.
  - `system_prompt_for(agent, overrides, skills: Option<&ResolvedSkills>) -> String`: agent body, `## Run target`, then the skills section.

- [ ] **Step 1: Write the failing tests**

`support.rs`:

```rust
impl Home {
    /// Install `<global>/skills/<name>` with a reference and a script.
    pub fn skill(&self, name: &str, description: &str, body: &str) -> std::path::PathBuf {
        let d = self.global().join("skills").join(name);
        std::fs::create_dir_all(d.join("references")).unwrap();
        std::fs::create_dir_all(d.join("scripts")).unwrap();
        std::fs::write(d.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n")).unwrap();
        std::fs::write(d.join("references/REF.md"), "reference text\n").unwrap();
        std::fs::write(d.join("scripts/hello.sh"), "#!/bin/sh\necho hello-from-skill\n").unwrap();
        d
    }
}
```

`assembly_skills.rs` (each test `#[tokio::test] #[serial]`):

```rust
use crate::support::{events, Home, MockScript, DONE};
use rupu_runtime::assembly::{AssembleError, Origin};
use rupu_transcript::Event;
use serial_test::serial;

fn run_start_prompt(evs: &[Event]) -> String {
    evs.iter()
        .find_map(|e| match e {
            Event::RunStart { system_prompt, .. } => system_prompt.clone(),
            _ => None,
        })
        .expect("run_start with a system prompt")
}

#[tokio::test]
#[serial]
async fn agent_skills_reach_the_prompt_the_grant_and_the_transcript() {
    let home = Home::new();
    home.skill("report-style", "House style for reports.", "Write plainly.");
    home.skill("code-review", "Review a diff.", "Look for bugs.");
    let assembler = home.assembler(crate::support::config());
    let spec = crate::assembly::agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [code-review]\npreloadSkills: [report-style]");
    let l = crate::assembly::launch(&home, spec, Origin::Standalone, "run_sk");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l).await.unwrap().result.unwrap();
    let evs = events(&path);
    let prompt = run_start_prompt(&evs);
    assert!(prompt.contains("## Skill: report-style"), "{prompt}");
    assert!(prompt.contains("Write plainly."));
    assert!(prompt.contains("- code-review: Review a diff."));
    let offered = evs.iter().find_map(|e| match e {
        Event::ToolGrant { entries, .. } => Some(entries.clone()),
        _ => None,
    }).unwrap();
    let load = offered.iter().find(|e| e.name == "skills.load").expect("skills.load offered");
    assert!(load.reasons.iter().any(|r| r == "ambient:skills"), "{:?}", load.reasons);
    assert!(evs.iter().any(|e| matches!(e, Event::SkillIndex { skills } if skills.len() == 2)));
    assert!(evs.iter().any(|e| matches!(e, Event::SkillLoad { name, via, .. } if name == "report-style" && via == "preload")));
}

#[tokio::test]
#[serial]
async fn an_agent_without_skills_gets_no_loader() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    let assembler = home.assembler(crate::support::config());
    let spec = crate::assembly::agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [\"*\"]");
    let l = crate::assembly::launch(&home, spec, Origin::Standalone, "run_none");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l).await.unwrap().result.unwrap();
    let evs = events(&path);
    assert!(!run_start_prompt(&evs).contains("## Skills"));
    assert!(!evs.iter().any(|e| matches!(e, Event::SkillIndex { .. })));
    let offered = evs.iter().find_map(|e| match e { Event::ToolGrant { entries, .. } => Some(entries.clone()), _ => None }).unwrap();
    assert!(offered.iter().all(|e| e.name != "skills.load"));
}

#[tokio::test]
#[serial]
async fn an_unknown_skill_fails_assembly() {
    let home = Home::new();
    let assembler = home.assembler(crate::support::config());
    let spec = rupu_agent::AgentSpec::parse("---\nname: probe\nprovider: anthropic\nmodel: claude-sonnet-4-6\nskills: [missing]\n---\nx\n").unwrap();
    let l = crate::assembly::launch(&home, spec, Origin::Standalone, "run_bad");
    match assembler.assemble(l).await {
        Err(AssembleError::Skills(m)) => assert!(m.contains("unknown skill \"missing\""), "{m}"),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("assembled a run with an unknown skill"),
    }
}

#[tokio::test]
#[serial]
async fn launch_overrides_add_skills_with_reason_step() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    let assembler = home.assembler(crate::support::config());
    let spec = crate::assembly::agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6");
    let mut l = crate::assembly::launch(&home, spec, Origin::Standalone, "run_step");
    l.overrides.skills = vec!["code-review".into()];
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l).await.unwrap().result.unwrap();
    let evs = events(&path);
    assert!(evs.iter().any(|e| matches!(e, Event::SkillIndex { skills } if skills[0].reason == "step")));
}
```

`agent()` and `launch()` live in `tests/it/assembly.rs`; make them `pub(crate)` if they are private. If `ToolGrantEntry`'s reasons field has a different name, use the real one (see `rupu-transcript/src/event.rs:48`).

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-runtime --test it assembly_skills::`
Expected: compile errors, then failures.

- [ ] **Step 3: Implement**

`spec.rs` `Overrides`: add

```rust
    /// The launch's own `skills:` entries (a workflow step's, `rupu run --skill`).
    pub skills: Vec<String>,
    /// The launch's own `preloadSkills:` entries.
    pub preload_skills: Vec<String>,
```

`mod.rs`: the variant `#[error("skills: {0}")] Skills(String),`; `into_run_error` gains `AssembleError::Skills(m) => rupu_agent::RunError::Preflight(format!("skills: {m}")),`; `record_unstarted` maps `Skills` exactly like `AgentLoad` (a `Preflight` failure with the message).

In `assemble`, right after `let d = defaults_for(&spec.origin);` and before the netflow sink:

```rust
        let skills = self.resolve_skills(&spec)?;
```

with

```rust
    /// The run's skills (spec §2.4): the agent's keys and the launch's own,
    /// resolved in one pass against the global and project layers. Fails
    /// closed: a run never starts without a skill it was given.
    fn resolve_skills(&self, spec: &LaunchSpec) -> Result<Option<Arc<rupu_skills::ResolvedSkills>>, AssembleError> {
        let agent_grants = spec.agent.skills.clone().unwrap_or_default();
        let agent_preload = spec.agent.preload_skills.clone().unwrap_or_default();
        let o = &spec.overrides;
        if agent_grants.is_empty() && agent_preload.is_empty() && o.skills.is_empty() && o.preload_skills.is_empty() {
            return Ok(None);
        }
        let project = self.ctx.project_root.as_ref().map(|p| p.join(".rupu"));
        let dirs = rupu_skills::LayerDirs::for_layers(&self.ctx.global, project.as_deref());
        let catalog = rupu_skills::SkillCatalog::discover(&dirs).map_err(|e| AssembleError::Skills(e.to_string()))?;
        let resolved = rupu_skills::resolve_run_skills(
            &catalog,
            &[
                rupu_skills::SkillRequest { reason: rupu_skills::SkillReason::Agent, grants: &agent_grants, preload: &agent_preload },
                rupu_skills::SkillRequest { reason: rupu_skills::SkillReason::Step, grants: &o.skills, preload: &o.preload_skills },
            ],
        )
        .map_err(|e| AssembleError::Skills(e.to_string()))?;
        Ok((!resolved.is_empty()).then(|| Arc::new(resolved)))
    }
```

In the `ToolContext` literal: `workspace: WorkspaceScope { …, read_roots: skills.as_ref().map(|s| s.read_roots()).unwrap_or_default() }` and `services: ToolServices { …, skills: skills.clone() }`. Replace `system_prompt_for(agent, &spec.overrides)` with `system_prompt_for(agent, &spec.overrides, skills.as_deref())`:

```rust
pub fn system_prompt_for(
    agent: &rupu_agent::AgentSpec,
    overrides: &Overrides,
    skills: Option<&rupu_skills::ResolvedSkills>,
) -> String {
    let mut prompt = match overrides.system_prompt_suffix.as_deref() {
        Some(suffix) => format!("{}\n\n## Run target\n\n{suffix}", agent.system_prompt),
        None => agent.system_prompt.clone(),
    };
    if let Some(section) = skills.and_then(|s| s.prompt_section()) {
        prompt.push_str("\n\n");
        prompt.push_str(&section);
    }
    prompt
}
```

Other callers of `system_prompt_for` pass `None`.

`crates/rupu-cli/tests/it/netflow_capture_wiring.rs` scans the `ToolContext {` literals; it only checks netflow fields, so it is unaffected. `no_hand_built_run_opts` is unaffected (no new site).

The `skill_index` / preload `skill_load` events these tests assert are written by the runner in Task 7; until then only the prompt and grant assertions can pass. Implement Task 6 and Task 7 back to back, and commit Task 6 with the event assertions temporarily `#[ignore]`d only if you must commit between them. Remove the ignore in Task 7.

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-runtime --test it assembly_skills::` (event assertions after Task 7), `cargo test -p rupu-runtime --test it assembly::`, `cargo build -p rupu-cli -p rupu-orchestrator -p rupu-agentiflow`
Expected: pass; `origin_matrix` and `star_parity_per_origin` snapshots are unchanged (the probe agent declares no skills).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-runtime crates/rupu-cli crates/rupu-orchestrator crates/rupu-agentiflow Cargo.lock
git commit -m "feat(runtime): the assembler resolves a run's skills and renders them into the prompt"
```

---

### Task 7: Runner events, the loaded set and notices

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` (`Run` struct + `Run::new`, after `write_tool_grant` at `:2050`, the tool success branch at `:3051`)
- Test: `crates/rupu-runtime/tests/it/assembly_skills.rs` (end to end through the assembler)

**Interfaces:**
- Consumes: `ToolServices.skills`, `skills.load`'s `structured` keys (Task 3), the events (Task 5), `ResolvedGrant::offers(name)`.
- Produces: `pub struct LoadedSkill { pub name: String, pub content_hash: String }` (`Clone, Debug, PartialEq, Eq`, exported from `rupu-agent`); `Run.loaded_skills: Vec<LoadedSkill>`, most recently loaded last, one entry per name.

- [ ] **Step 1: Write the failing test** (append to `assembly_skills.rs`)

```rust
#[tokio::test]
#[serial]
async fn a_model_loads_a_skill_reads_its_reference_and_runs_its_script() {
    let home = Home::new();
    let dir = home.skill("pdf", "PDF things.", "Run scripts/hello.sh.");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("scripts/hello.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let root = dir.canonicalize().unwrap();
    let assembler = home.assembler(crate::support::config());
    let spec = crate::assembly::agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [read_file, bash]\nskills: [pdf]");
    let l = crate::assembly::launch(&home, spec, Origin::Standalone, "run_use");
    let path = l.transcript_path.clone();
    let script = serde_json::json!([
        { "AssistantToolUse": { "text": null, "tool_id": "c1", "tool_name": "skills.load", "tool_input": { "name": "pdf" }, "stop": "tool_use" } },
        { "AssistantToolUse": { "text": null, "tool_id": "c2", "tool_name": "read_file", "tool_input": { "path": root.join("references/REF.md") }, "stop": "tool_use" } },
        { "AssistantToolUse": { "text": null, "tool_id": "c3", "tool_name": "bash", "tool_input": { "command": format!("sh {}", root.join("scripts/hello.sh").display()) }, "stop": "tool_use" } },
        { "AssistantText": { "text": "done", "stop": "end_turn" } }
    ]);
    let _m = MockScript::set(&script.to_string());
    rupu_runtime::run_agent(&assembler, l).await.unwrap().result.unwrap();
    let evs = events(&path);
    assert!(evs.iter().any(|e| matches!(e, Event::SkillLoad { name, via, .. } if name == "pdf" && via == "model")));
    let outputs: Vec<String> = evs.iter().filter_map(|e| match e { Event::ToolResult { output, .. } => Some(output.clone()), _ => None }).collect();
    assert!(outputs.iter().any(|o| o.contains("reference text")), "{outputs:?}");
    assert!(outputs.iter().any(|o| o.contains("hello-from-skill")), "{outputs:?}");
}
```

The launch must run with a permission mode that allows `bash` (the `launch` helper's default; if it is readonly, set `l.mode` to bypass for this test).

Add a second test: a skill whose `SKILL.md` declares `allowed-tools: Bash(jq:*)`, loaded by an agent granted only `read_file`, writes a `Notice { kind: "skill_tools_missing", .. }` naming `bash`. And a third: a granted `disable-model-invocation` skill writes `skill_not_indexed` at run start.

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-runtime --test it assembly_skills::`
Expected: the event assertions fail (no `skill_load` written yet).

- [ ] **Step 3: Implement**

`runner.rs`:
- Add `LoadedSkill` (above) and `loaded_skills: Vec<LoadedSkill>` to `Run`, initialised to `Vec::new()` in `Run::new` (Task 8 seeds it).
- After `write_tool_grant(&mut writer, &grant)?;`:

```rust
        if let Some(skills) = opts.tool_context.services.skills.clone() {
            write_skill_index(&mut writer, &skills)?;
        }
```

with

```rust
/// `skill_index`, a `skill_load { via: preload }` per preloaded skill (its
/// body is in the system prompt), and a `skill_not_indexed` notice per
/// granted skill the model is never offered.
fn write_skill_index(writer: &mut JsonlWriter, skills: &rupu_skills::ResolvedSkills) -> std::io::Result<()> {
    let rows = skills
        .entries()
        .iter()
        .map(|g| rupu_transcript::SkillIndexEntry {
            name: g.name().to_owned(),
            layer: g.skill.layer.as_str().to_owned(),
            stamp: g.skill.stamp.hex(),
            preload: g.preload,
            indexed: g.indexed,
            reason: g.reason.as_str().to_owned(),
        })
        .collect();
    writer.write(&Event::SkillIndex { skills: rows })?;
    for g in skills.entries() {
        if g.preload {
            let hash = g.skill.content_hash().map(|h| h.hex()).unwrap_or_default();
            writer.write(&Event::SkillLoad {
                name: g.name().to_owned(),
                layer: g.skill.layer.as_str().to_owned(),
                content_hash: hash,
                base_dir: g.skill.root.display().to_string(),
                via: "preload".into(),
            })?;
        } else if !g.indexed {
            writer.write(&Event::Notice {
                kind: "skill_not_indexed".into(),
                message: format!(
                    "skill {} is granted but marked disable-model-invocation, so the model is never offered it; preload it or invoke it from a session",
                    g.name()
                ),
            })?;
        }
    }
    Ok(())
}
```

(Match the writer's real error type; `write_tool_grant` shows it.)

- In the tool success branch, after the `ToolResult` write, when the resolved descriptor's name is `"skills.load"` and `out.error.is_none()`:

```rust
if descriptor.name == rupu_tools::skills::LOAD.name && out.error.is_none() {
    if let Some(s) = out.structured.as_ref() {
        record_skill_load(&mut writer, opts, &grant, s)?;
    }
}
```

with

```rust
fn record_skill_load(
    writer: &mut JsonlWriter,
    run: &mut Run,
    grant: &rupu_tools::ResolvedGrant,
    s: &serde_json::Value,
) -> std::io::Result<()> {
    let text = |k: &str| s[k].as_str().unwrap_or_default().to_owned();
    let name = text("skill");
    let content_hash = text("content_hash");
    writer.write(&Event::SkillLoad {
        name: name.clone(),
        layer: text("layer"),
        content_hash: content_hash.clone(),
        base_dir: text("base_dir"),
        via: "model".into(),
    })?;
    if s["changed"].as_bool() == Some(true) {
        writer.write(&Event::Notice {
            kind: "skill_changed".into(),
            message: format!("skill {name} changed on disk since this run started; the model was given its current version"),
        })?;
    }
    let missing: Vec<&str> = s["allowed_tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str())
        .filter(|t| !grant.offers(t))
        .collect();
    if !missing.is_empty() {
        writer.write(&Event::Notice {
            kind: "skill_tools_missing".into(),
            message: format!("skill {name} expects tools this run was not granted: {}", missing.join(", ")),
        })?;
    }
    run.loaded_skills.retain(|l| l.name != name);
    run.loaded_skills.push(LoadedSkill { name, content_hash });
    Ok(())
}
```

Use the real variable names at the call site (the run state, the grant, the descriptor). If `grant` is not in scope in the tool loop, keep a clone of the offered names on `Run`.

- Remove any `#[ignore]` left in Task 6.

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-runtime --test it assembly_skills::`, `cargo test -p rupu-agent --test it`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-agent crates/rupu-runtime/tests
git commit -m "feat(agent): record skill loads, keep the loaded set, notice what a skill lacks"
```

---

### Task 8: Re-attach after compaction; replay and continuation keep the loaded set

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` (`compact_messages` task-text extraction at `:496`, `compact_context` before `:579`), `src/replay.rs`, `src/continuation.rs`, `UserTurn` (`runner.rs:1436`), `src/lib.rs` (exports)
- Test: `crates/rupu-agent/tests/it/skills_compaction.rs` (add to `main.rs`); a `replay` unit test

**Interfaces:**
- Consumes: `REATTACH_MARKER`, `render_reattach`, `ReattachItem` (Task 1); `Run.loaded_skills` (Task 7).
- Produces:
  - `replay::loaded_skills(events: &[Event]) -> Vec<LoadedSkill>`: every `skill_load` whose `via` is not `preload`, latest per name, in load order.
  - `UserTurn.loaded_skills: Vec<LoadedSkill>` (`Default` empty); `Continuation::Resume` gains `loaded_skills: Vec<LoadedSkill>`; `apply_continuation` sets `UserTurn.loaded_skills`.
  - A continued run writes `skill_load { via: "continuation" }` for each seeded skill at start, so every transcript carries its own loaded set.

- [ ] **Step 1: Write the failing tests**

`tests/it/skills_compaction.rs`, modelled on `runner_model_limits.rs`'s compaction tests (`dense_seed()`, `ModelLimits::fixed`, a scripted summary turn) and `tool_grant.rs`'s opts:
- `a_loaded_skill_survives_compaction`: tool context with `services.skills` holding skill `pdf` (body `"PDF STEPS"`); script: `skills.load {name: pdf}`, then enough dense turns to cross the fixed limit twice with summary turns. Assert: the first `Event::Compaction`'s `messages[0]` has a text block starting with `"<skills-in-use>"` containing `"PDF STEPS"`; the second `Compaction`'s `messages[0]` contains `"<skills-in-use>"` exactly once and `"PDF STEPS"` exactly once; a `skill_load { via: "reattach" }` follows each compaction.
- `an_uninstalled_skill_is_listed_missing`: delete the skill dir after the load; the re-attach block says `No longer installed: pdf` and a `skill_missing` notice is written.
- `the_cap_omits_older_skills`: `ModelLimits::fixed` with a small input limit and two loaded skills with large bodies; the most recently loaded is attached, the other is listed under "Not re-attached for space", and a `skill_reattach_truncated` notice is written.

`replay.rs` unit test:

```rust
#[test]
fn loaded_skills_fold_latest_per_name_skipping_preload() {
    let load = |name: &str, hash: &str, via: &str| Event::SkillLoad {
        name: name.into(), layer: "global".into(), content_hash: hash.into(), base_dir: String::new(), via: via.into(),
    };
    let got = loaded_skills(&[load("a", "1", "model"), load("p", "9", "preload"), load("b", "2", "model"), load("a", "3", "reattach")]);
    assert_eq!(
        got,
        vec![
            LoadedSkill { name: "b".into(), content_hash: "2".into() },
            LoadedSkill { name: "a".into(), content_hash: "3".into() },
        ]
    );
}
```

A continuation test in `tests/it/run_continue.rs`'s style (or `continuation.rs` tests): a transcript with a `skill_load { via: model }` and an interrupted turn; `prepare_continuation` returns `Resume { loaded_skills: [pdf] }`; `apply_continuation` puts it on the `UserTurn`.

- [ ] **Step 2: Run and fail**

Run: `cargo test -p rupu-agent --test it skills_compaction::`, `cargo test -p rupu-agent --lib replay`
Expected: failures / compile errors.

- [ ] **Step 3: Implement**

`compact_messages` task text: skip a text block that starts with the marker:

```rust
            if let ContentBlock::Text { text } = b {
                (!text.starts_with(rupu_skills::render::REATTACH_MARKER)).then_some(text.as_str())
            } else {
                None
            }
```

`compact_context`, after `*messages = outcome.messages;` and before `let post = serde_json::to_value(&*messages)`:

```rust
            reattach_skills(messages, opts, writer);
```

with

```rust
/// Spec §6: put the run's loaded skills back after compaction summarised
/// them away, at their current version, most recently loaded first, capped
/// at a quarter of the input limit (32 KiB of text when it is unknown).
fn reattach_skills(messages: &mut [Message], run: &mut Run, writer: &mut JsonlWriter) {
    let Some(skills) = run.tool_context.services.skills.clone() else { return };
    if run.loaded_skills.is_empty() || messages.is_empty() {
        return;
    }
    let cap = run.limits.input_tokens().map(|t| t as usize).unwrap_or(32 * 1024);
    let mut fresh = Vec::new();
    let mut omitted = Vec::new();
    let mut missing = Vec::new();
    let mut used = 0usize;
    for loaded in run.loaded_skills.iter().rev() {
        let skill = skills
            .get(&loaded.name)
            .and_then(|g| rupu_skills::load_skill(&g.skill.dir, g.skill.layer).ok());
        let Some(skill) = skill else {
            missing.push(loaded.name.clone());
            continue;
        };
        if used + skill.body.len() > cap {
            omitted.push(loaded.name.clone());
            continue;
        }
        used += skill.body.len();
        fresh.push(skill);
    }
    let items: Vec<rupu_skills::render::ReattachItem<'_>> = fresh
        .iter()
        .map(|s| rupu_skills::render::ReattachItem { name: s.name.as_str(), base_dir: &s.root, body: &s.body })
        .collect();
    let omitted_ref: Vec<&str> = omitted.iter().map(String::as_str).collect();
    let missing_ref: Vec<&str> = missing.iter().map(String::as_str).collect();
    let block = rupu_skills::render::render_reattach(&items, &omitted_ref, &missing_ref);
    messages[0].content.push(ContentBlock::Text { text: block });
    for s in &fresh {
        let hash = s.content_hash().map(|h| h.hex()).unwrap_or_default();
        if run.loaded_skills.iter().any(|l| l.name == s.name.as_str() && l.content_hash != hash) {
            let _ = writer.write(&Event::Notice {
                kind: "skill_changed".into(),
                message: format!("skill {} changed on disk since it was loaded; its current version was re-attached", s.name),
            });
        }
        let _ = writer.write(&Event::SkillLoad {
            name: s.name.to_string(),
            layer: s.layer.as_str().into(),
            content_hash: hash,
            base_dir: s.root.display().to_string(),
            via: "reattach".into(),
        });
    }
    for m in &missing {
        let _ = writer.write(&Event::Notice { kind: "skill_missing".into(), message: format!("skill {m} is no longer installed and was not re-attached") });
    }
    if !omitted.is_empty() {
        let _ = writer.write(&Event::Notice {
            kind: "skill_reattach_truncated".into(),
            message: format!("not re-attached for space: {}", omitted.join(", ")),
        });
    }
}
```

`run.limits.input_tokens()` stands for however `ModelLimits` exposes the input limit (the value `compact_threshold` reads); use the real accessor. The cap is in characters: a quarter of the input tokens at ~4 characters a token is the token count itself. If `messages[0]` can hold non-text content, pushing a text block is still valid; the rebuilt first message from `rebuild_compacted` is a user message.

`replay.rs`:

```rust
/// The run's loaded skills (spec §6): the latest `skill_load` per name,
/// preloads excluded (their bodies live in the system prompt), in load order.
pub fn loaded_skills(events: &[Event]) -> Vec<crate::LoadedSkill> {
    let mut out: Vec<crate::LoadedSkill> = Vec::new();
    for e in events {
        if let Event::SkillLoad { name, content_hash, via, .. } = e {
            if via == "preload" {
                continue;
            }
            out.retain(|l| &l.name != name);
            out.push(crate::LoadedSkill { name: name.clone(), content_hash: content_hash.clone() });
        }
    }
    out
}
```

`continuation.rs`: `prepare` already reads the transcript's events; add `loaded_skills: replay::loaded_skills(&events)` to `Continuation::Resume`, and have `apply_continuation` / the `ContinuationTarget` impl for `UserTurn` set `loaded_skills`. Fix every `Continuation::Resume { .. }` pattern the compiler reports.

`runner.rs`: `UserTurn` gains `pub loaded_skills: Vec<LoadedSkill>` (in its constructors default empty). `Run::new` seeds `loaded_skills` from `opts.prompt.loaded_skills`. After `write_skill_index` at run start, for each seeded skill still granted, write `Event::SkillLoad { via: "continuation", .. }` with its recorded hash; drop seeded names the run no longer grants (with a `skill_missing` notice).

`lib.rs`: export `LoadedSkill` and `replay::loaded_skills`.

- [ ] **Step 4: Run and pass**

Run: `cargo test -p rupu-agent --test it`, `cargo test -p rupu-agent --lib`, `cargo test -p rupu-cli --test it run_continue::`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-agent crates/rupu-cli
git commit -m "feat(agent): re-attach loaded skills after compaction and keep them across continuation"
```

---

### Task 9: Docs and the spec

**Files:**
- Modify: `docs/agent-format.md` (the two keys, the grant grammar, an example), `docs/skills.md` (replace its "arrives with Plan 2" line with how agents use skills: index vs preload, `skills.load`, reading and running a skill's files, compaction), `CLAUDE.md` (`rupu-tools`: `skills.load` + `Service::Skills` + read roots; `rupu-agent`: the keys, `AgentLoadError::Skill`, the loaded set, re-attach; `rupu-runtime`: `Overrides.skills`, `AssembleError::Skills`, `system_prompt_for`'s skills section; `rupu-transcript`: the two events; `rupu-skills`: `run.rs`), the spec.

- [ ] **Step 1:** Spec edits:
  - §2.2: step skills travel on `Overrides.skills` / `Overrides.preload_skills` (resolved with reason `step`), not on `Origin::WorkflowStep`. `rupu run --skill` uses the same fields, which is why they live on `Overrides`.
  - §12: Plan 2 is split. **2a** is runs, this plan; **2b** is workflow steps and placed units.
- [ ] **Step 2:** `cargo test -p rupu-cli --test it test_layout::` (docs-only changes don't affect it; run it as the final sanity check).
- [ ] **Step 3: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs(skills): agents use skills; plan 2 split into runs and steps"
```

---

## Done when

- `cargo test -p rupu-skills --test it`, `-p rupu-tools --test it`, `-p rupu-agent --test it` and `--lib`, `-p rupu-runtime --test it`, `-p rupu-transcript`, `-p rupu-agentiflow --test it flow_toolset_parity::`, `-p rupu-mcp --test it` and `-p rupu-cli --test it` pass. `cargo clippy` on each touched crate is clean with `-D warnings`.
- By hand, with a scratch `RUPU_HOME` that has `pdf` installed from `anthropics/skills` and an agent with `skills: [pdf]`: `rupu run <agent> "extract the text of <a real pdf>"` on a real provider. The transcript (`rupu transcript show`) shows `skills: 1 skill: pdf`, a `skill_load` via model, a `read_file` of a file under the skill's base directory, and a `bash` run of one of its scripts.
- Nothing in workflow parsing, placed units or sessions changed. Those are Plan 2b and Plan 3.
