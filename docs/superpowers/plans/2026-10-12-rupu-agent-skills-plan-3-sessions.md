# Agent Skills Plan 3: skills in sessions

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rupu session` stops refusing agents that declare skills. A session fixes its skill set and the rendered skills section when it starts, so every turn's system prompt is byte-identical. `session start --skill` adds to that set, and `session send --skill` lets the operator invoke any installed skill in a turn. A session's loaded skills survive turns and every kind of compaction.

**Architecture:**
- `rupu-skills` gains a frozen form of a resolved set (`FrozenSkill`, serde) and `resolve_frozen`. That function rebuilds a `ResolvedSkills` from exact `layer:name` entries without re-expanding `*` and without recomputing preload or index flags.
- `GrantedSkill` gains a `baseline_stamp`, the stamp the session's index recorded. `skills.load` and `skill_index` judge change against it.
- The run assembler's prompt takes an optional frozen skills section (`Overrides.skill_section`).
- The runner records operator loads (`UserTurn.operator_loads` writes `skill_load { via: operator }` and joins the loaded set). Its re-attach logic becomes one public helper, which the session's own compaction paths also call.
- `rupu-cli` session code stores the frozen set, prompt and loaded set on `SessionRecord`, and the operator's invocations on the turn request.
- The CP session API and remote session hosts are Plan 4.

**Tech Stack:** Rust 2021, existing crates.

**Spec:** `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md` §5, §6, §4. Plans 2a and 2b must be merged. On main, `SkillReason` is `Agent | Launch | Session | Operator`.

## Global Constraints

- **Byte stability.** A session's system prompt is the stored `agent_system_prompt` plus `"\n\n"` plus the stored `skill_prompt`, identical on every turn. Nothing re-renders preloaded bodies after `session start`.
- **Frozen membership.** A skill installed after the session started is not offered, even under `*`. A frozen skill that is no longer installed is dropped from the turn's set with a `skill_missing` notice, and the session keeps running.
- **Operator invocation (S6).**
  - Any valid installed skill in the session's global or project layer can be invoked, granted or not.
  - The `global:` and `project:` prefixes work as in grants. A bare name in both layers is refused. `*` is refused.
  - An invoked skill joins `operator_skills`: loadable by the model from the next turn, a read root, and never in the index.
- **Old workers.** `ensure_session_worker` already restarts a worker onto a newer binary at enqueue. New fields on `SessionRecord` and `SessionTurnRequest` are `#[serde(default)]` and skipped when empty, so old records still read.
- **Thin CLI.** Resolution, freezing and rendering live in `rupu-skills`, `rupu-runtime` and `rupu-agent`. `cmd/session.rs` stores, passes and prints.
- **Tests.** Session turns are driven in-process through `run_turn` with the `RUPU_MOCK_PROVIDER_SCRIPT` seam under `crate::test_support::ENV_LOCK`, as the existing session tests do.
- Format only touched files. Never run package-wide `cargo fmt`. Never use bare `git stash`.

## Review Focus

1. **Turn 2's `RunStart.system_prompt` equals turn 1's** after the preloaded skill's `SKILL.md` was edited between turns. Pinned in Task 4.
2. **A skill the model loaded in turn 1 is re-attached** when the conversation is compacted in turn 3, by the runner's in-turn compaction, by `rupu session compact`, and by the worker's compact pseudo-turn. Pinned in Tasks 4 and 6.
3. **`session send --skill typo`** is refused with a did-you-mean before anything is enqueued, and the session stays usable. Pinned in Task 5.
4. **A skill edited after session start** loads at its current version with a `skill_changed` notice: the comparison is against the session's baseline, not the turn's. Pinned in Task 1 and Task 4.
5. **A session record written before this plan** (no skill fields) still runs turns unchanged. Pinned in Task 4.

---

### Task 1: Frozen sets and baselines in `rupu-skills`

**Files:**
- Modify: `crates/rupu-skills/src/run.rs` (`GrantedSkill.baseline_stamp`, `FrozenSkill`, `ResolvedSkills::freeze`, `resolve_frozen`, `resolve_operator`), `src/render.rs` (`render_operator_injection`), `src/lib.rs`
- Modify: `crates/rupu-tools/src/skills/mod.rs` (`changed` compares to `baseline_stamp`)
- Test: `crates/rupu-skills/tests/it/run.rs`, `tests/it/render.rs`, `crates/rupu-tools/tests/it/skills_load.rs`

**Interfaces:**
- `GrantedSkill.baseline_stamp: Stamp`. `resolve_run_skills` sets it to `skill.stamp`. `resolve_frozen` sets it to the frozen stamp.
- `SkillReason` gains `Deserialize` (lowercase), plus `FrozenSkill`:

  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct FrozenSkill {
      pub name: String,
      pub layer: Layer,                 // Layer gains Deserialize (lowercase)
      pub preload: bool,
      pub indexed: bool,
      pub reason: SkillReason,
      pub stamp: String,                // hex, the baseline
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub content_hash: Option<String>, // hex, Some exactly for preloads
  }
  ```
- `ResolvedSkills::freeze(&self) -> Vec<FrozenSkill>`.
- `resolve_frozen(catalog: &SkillCatalog, frozen: &[FrozenSkill]) -> (ResolvedSkills, Vec<String> /* missing names */)`.
  - It loads exactly `catalog.get(f.layer, &f.name)`.
  - It keeps `preload`, `indexed` and `reason` from the record.
  - It sets `baseline_stamp` from `f.stamp` and `content_hash` from `f.content_hash`.
  - An entry that is missing or no longer valid goes into the missing list.
  - A stamp that won't parse as hex counts as missing, with a reason the caller can log.
- `resolve_operator(catalog: &SkillCatalog, entry: &str) -> Result<FrozenSkill, RunSkillsError>`.
  - It accepts a bare name or `layer:name`.
  - It refuses `*`, `project:*` and ambiguous names with the existing grant error texts.
  - It returns `reason: Operator, preload: false, indexed: false` and the current stamp.
- `ResolvedSkills::with_frozen(mut self, catalog, extra: &[FrozenSkill]) -> (Self, Vec<String>)`. This appends operator entries the same way and skips names already present.
- `render::render_operator_injection(name: &str, base_dir: &Path, version: &ContentHash, body: &str) -> String`. It is byte-pinned:

  ```text
  <skill name="pdf" base="/h/.rupu/skills/pdf" version="abababababab">
  Step one.
  </skill>
  ```

  `version` is the short form. The body is followed by `"\n</skill>"`.

- [ ] **Step 1: Write the failing tests**

`tests/it/run.rs`:

```rust
#[test]
fn freeze_then_resolve_frozen_keeps_flags_and_baselines() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "report-style", "report-style", "House style.");
    write_skill(&g.path().join("skills"), "code-review", "code-review", "Review a diff.");
    let cat = catalog(g.path(), None);
    let grants = s(&["*"]);
    let preload = s(&["report-style"]);
    let r = resolve_run_skills(&cat, &[SkillRequest { reason: SkillReason::Agent, grants: &grants, preload: &preload }]).unwrap();
    let frozen = r.freeze();
    // A skill installed after the freeze is not offered.
    write_skill(&g.path().join("skills"), "late", "late", "Installed later.");
    // The preloaded skill is edited after the freeze.
    std::fs::write(g.path().join("skills/report-style/SKILL.md"), "---\nname: report-style\ndescription: House style.\n---\nEdited.\n").unwrap();
    let (again, missing) = rupu_skills::run::resolve_frozen(&catalog(g.path(), None), &frozen);
    assert!(missing.is_empty());
    assert_eq!(again.names(), vec!["code-review", "report-style"]);
    let rs = again.get("report-style").unwrap();
    assert!(rs.preload && !rs.indexed);
    assert_eq!(rs.baseline_stamp.hex(), frozen.iter().find(|f| f.name == "report-style").unwrap().stamp);
    assert_ne!(rs.baseline_stamp, rs.skill.stamp, "the edit moved the live stamp, not the baseline");
}

#[test]
fn a_frozen_skill_that_was_removed_is_reported_missing() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "gone", "gone", "d");
    let grants = s(&["gone"]);
    let r = resolve_run_skills(&catalog(g.path(), None), &[SkillRequest { reason: SkillReason::Agent, grants: &grants, preload: &[] }]).unwrap();
    let frozen = r.freeze();
    std::fs::remove_dir_all(g.path().join("skills/gone")).unwrap();
    let (again, missing) = rupu_skills::run::resolve_frozen(&catalog(g.path(), None), &frozen);
    assert!(again.is_empty());
    assert_eq!(missing, vec!["gone".to_string()]);
}

#[test]
fn operator_entries_resolve_any_installed_skill_but_not_wildcards() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "pdf", "pdf", "d");
    write_skill(&p.path().join("skills"), "notes", "notes", "d");
    write_skill(&g.path().join("skills"), "both", "both", "d");
    write_skill(&p.path().join("skills"), "both", "both", "d");
    let cat = catalog(g.path(), Some(p.path()));
    let f = rupu_skills::run::resolve_operator(&cat, "notes").unwrap();
    assert_eq!((f.layer, f.reason, f.preload, f.indexed), (rupu_skills::Layer::Project, SkillReason::Operator, false, false));
    assert!(rupu_skills::run::resolve_operator(&cat, "global:pdf").is_ok());
    assert!(rupu_skills::run::resolve_operator(&cat, "*").is_err());
    assert!(rupu_skills::run::resolve_operator(&cat, "project:*").is_err());
    let amb = rupu_skills::run::resolve_operator(&cat, "both").unwrap_err().to_string();
    assert!(amb.contains("both the global and the project layer"), "{amb}");
    let typo = rupu_skills::run::resolve_operator(&cat, "pdff").unwrap_err().to_string();
    assert!(typo.contains("did you mean"), "{typo}");
}

#[test]
fn frozen_skills_round_trip_through_json() {
    let g = tempfile::tempdir().unwrap();
    write_skill(&g.path().join("skills"), "pdf", "pdf", "d");
    let grants = s(&["pdf"]);
    let frozen = resolve_run_skills(&catalog(g.path(), None), &[SkillRequest { reason: SkillReason::Launch, grants: &grants, preload: &[] }]).unwrap().freeze();
    let json = serde_json::to_string(&frozen).unwrap();
    assert!(json.contains("\"reason\":\"launch\"") && json.contains("\"layer\":\"global\""), "{json}");
    assert_eq!(serde_json::from_str::<Vec<rupu_skills::run::FrozenSkill>>(&json).unwrap(), frozen);
}
```

`tests/it/render.rs`:

```rust
#[test]
fn operator_injection_is_exact() {
    let got = rupu_skills::render::render_operator_injection("pdf", Path::new("/h/.rupu/skills/pdf"), &ContentHash([0xab; 32]), "Step one.");
    assert_eq!(got, "<skill name=\"pdf\" base=\"/h/.rupu/skills/pdf\" version=\"abababababab\">\nStep one.\n</skill>");
}
```

`crates/rupu-tools/tests/it/skills_load.rs`: build a `ResolvedSkills` with `resolve_frozen` from a freeze taken before an edit, then call `skills.load` without editing again. `changed` must be `true` and the output must start with `CHANGED_NOTE`. The comparison is against the baseline.

- [ ] **Step 2: Run them and confirm they fail**

`cargo test -p rupu-skills --test it run:: render::` and `cargo test -p rupu-tools --test it skills_load::`. Expect compile errors.

- [ ] **Step 3: Implement**

`Layer` and `SkillReason` derive `Deserialize` (lowercase). Add `baseline_stamp` to `GrantedSkill` and set it everywhere a `GrantedSkill` is built. `freeze` maps entries to `FrozenSkill`, with `content_hash: g.content_hash.map(|h| h.hex())`.

`resolve_frozen`:

```rust
pub fn resolve_frozen(catalog: &SkillCatalog, frozen: &[FrozenSkill]) -> (ResolvedSkills, Vec<String>) {
    let mut entries = Vec::new();
    let mut missing = Vec::new();
    for f in frozen {
        let skill = catalog.get(f.layer, &f.name).and_then(|e| e.skill().ok().cloned());
        let (Some(skill), Some(baseline)) = (skill, Stamp::from_hex(&f.stamp)) else {
            missing.push(f.name.clone());
            continue;
        };
        entries.push(GrantedSkill {
            skill,
            preload: f.preload,
            indexed: f.indexed,
            reason: f.reason,
            baseline_stamp: baseline,
            content_hash: f.content_hash.as_deref().and_then(ContentHash::from_hex),
        });
    }
    (ResolvedSkills { entries }, missing)
}
```

Add `Stamp::from_hex` and `ContentHash::from_hex` to the fingerprint macro in `tree.rs`, returning `None` on bad input.

`resolve_operator`:
1. Parse the entry with `SkillGrant::from_str`.
2. Refuse `AllGlobal` and `AllProject` with `RunSkillsError::Parse` and a message saying wildcards can't be invoked.
3. Otherwise call `resolve_grants(&[grant], catalog)`, which already gives the ambiguity, unknown and did-you-mean errors.
4. Take its single skill and build the `FrozenSkill` with reason `Operator` and its current stamp.

`skills.load`: `let changed = fresh.stamp != granted.baseline_stamp;`.

- [ ] **Step 4: Run them and confirm they pass**

`cargo test -p rupu-skills --test it`, `cargo test -p rupu-tools --test it`.

- [ ] **Step 5: Commit**

`feat(skills): frozen skill sets, baselines and operator entries`

---

### Task 2: A frozen skills section in the assembler; a session reason

**Files:**
- Modify: `crates/rupu-runtime/src/assembly/spec.rs` (`Overrides.skill_section`), `src/assembly/mod.rs` (`system_prompt_for`, `resolve_skills` takes the launch request's reason)
- Test: `crates/rupu-runtime/tests/it/assembly_skills.rs`

**Interfaces:**
- `Overrides.skill_section: Option<String>`. When it is `Some`, `system_prompt_for` appends it, after `"\n\n"`, instead of `skills.prompt_section()`. When it is `Some("")`, it appends nothing.
- `RunAssembler::resolve_skills_as(&self, agent, overrides, launch_reason: SkillReason)`. The existing `resolve_skills` becomes `resolve_skills_as(.., SkillReason::Launch)`. Sessions call it with `SkillReason::Session`.

- [ ] **Step 1: Write the failing tests**

- **`a_frozen_section_replaces_the_rendered_one`.** Launch with `LaunchServices.skills` set (one indexed skill) and `overrides.skill_section = Some("## Skills\n\nFROZEN".into())`. Assert that the `RunStart.system_prompt` ends with `"\n\n## Skills\n\nFROZEN"` and does not contain the live index line. Assert that `skills.load` is still offered.
- **`resolve_skills_as_session_records_reason_session`.** Assert that the `skill_index` row's reason is `"session"`.

- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.** Keep `system_prompt_for`'s other callers working by passing their overrides as before. The field defaults to `None`.
- [ ] **Step 4:** `cargo test -p rupu-runtime --test it`.
- [ ] **Step 5: Commit**

`feat(runtime): a frozen skills section and a session reason for launch skills`

---

### Task 3: Operator loads in the runner; a shared re-attach helper

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` (`UserTurn.operator_loads`, `write_skill_index` rows use `baseline_stamp`, operator `skill_load` + loaded set, extract `reattach_skills` into a pub helper), `src/lib.rs` (exports)
- Create: `crates/rupu-agent/src/skills_reattach.rs` (or put it in an existing module if the runner's layout prefers that)
- Test: `crates/rupu-agent/tests/it/skills_compaction.rs`, `crates/rupu-agent/tests/it/` (operator loads)

**Interfaces:**
- `pub struct OperatorLoad { pub name: String, pub layer: String, pub content_hash: String, pub base_dir: String }`, with `Clone`, `Debug`, `PartialEq`, `Eq`, `Serialize` and `Deserialize`.
- `UserTurn.operator_loads: Vec<OperatorLoad>`, empty by default.
- At run start, after `write_skill_index` and `seed_loaded_skills`, the runner writes one `skill_load { via: "operator" }` per operator load and puts it in the loaded set, replacing any entry of the same name.
- `skill_index` rows write `baseline_stamp.hex()`.
- `pub fn reattach_block(loaded: &[LoadedSkill], skills: &ResolvedSkills, cap_chars: usize) -> Option<Reattach>`, where `Reattach` is `{ block: String, reattached: Vec<LoadedSkill /* current hashes */>, changed: Vec<String>, missing: Vec<String>, omitted: Vec<String> }`. It is pure apart from reading the skills from disk, and it runs off the async thread.
- The runner's compaction calls the helper and then writes the same events and notices as today, with no behaviour change. Tasks 4 and 6 call the helper from the session code.

- [ ] **Step 1: Write the failing tests**

- **Operator loads.** A run whose `UserTurn.operator_loads` holds `pdf` writes `skill_load { name: "pdf", via: "operator" }` after `skill_index`. A later compaction in that run re-attaches `pdf`. Model this on `a_loaded_skill_survives_compaction`.
- **The helper.** Feed `reattach_block` two loaded skills and a cap that fits one. The block holds the most recent one, `omitted` names the other, and the block starts with `REATTACH_MARKER`.
- **Baseline stamps.** With frozen baselines, the `skill_index` row's stamp equals the baseline, not the live stamp.

- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.** Move the body of `reattach_skills` into the helper without changing its behaviour. The existing compaction tests must pass unchanged.
- [ ] **Step 4:** `cargo test -p rupu-agent --test it`, `cargo test -p rupu-agent --lib`, `cargo test -p rupu-runtime --test it`.
- [ ] **Step 5: Commit**

`feat(agent): operator skill loads and a shared re-attach helper`

---

### Task 4: Sessions carry skills

**Files:**
- Modify: `crates/rupu-cli/src/cmd/session.rs`:
  - `SessionRecord`
  - `StartArgs`
  - `start`
  - `session_agent_spec`
  - `run_turn`
  - the history persistence after a turn
  - remove `sessions_refuse_skills` and both refusal sites
  - `test_session_record`
- Modify: `crates/rupu-cli/tests/it/cli_session.rs`. Replace the two refusal tests with acceptance tests.
- Test: in-crate `session.rs` tests driving `run_turn`

**Interfaces:**
- New `SessionRecord` fields. Every one is `#[serde(default, skip_serializing_if = …)]`.
  - `skill_index: Vec<rupu_skills::run::FrozenSkill>`: the frozen set (agent, `--skill` and operator entries; operator entries are appended later).
  - `skill_prompt: Option<String>`: the rendered section at start. `None` means the session has no skills section.
  - `loaded_skills: Vec<rupu_agent::LoadedSkill>`.
- New `StartArgs` flags:
  - `--skill <s>`, repeatable, into `skills: Vec<String>`.
  - `--preload-skill <s>`, repeatable, into `preload_skills: Vec<String>`.
  - Both are resolved with reason `session`.

- [ ] **Step 1: Write the failing tests**

In-crate tests, using `with_mock_home` and `RUPU_MOCK_PROVIDER_SCRIPT` under `ENV_LOCK`:
1. **`a_session_freezes_its_skills_at_start`.** Install `report-style` and `code-review`. Use an agent with `skills: ["*"]` and `preloadSkills: [report-style]`. Run the `start` path, or build the record the way `start` does through the extracted helper, and `run_turn` twice. Between the turns, edit `report-style/SKILL.md` and install a new skill `late`. Assertions:
   - Both turns' `RunStart.system_prompt` are byte-identical and contain `## Skill: report-style`.
   - Neither contains `late`.
   - Each turn writes `skill_index` with reason `agent`.
   - In turn 2 the scripted `skills.load {name: report-style}` produces `skill_changed`.
2. **`start_flags_add_skills_with_reason_session`.** `--skill code-review` on an agent without skills gives a `skill_index` reason of `session`.
3. **`loaded_skills_persist_across_turns_and_reattach`.** Turn 1 scripts `skills.load {name: code-review}`. Turn 2 has dense history and `ModelLimits::fixed`, which forces an in-turn compaction. Turn 2's `Compaction` event's first message contains the re-attach block with code-review's body. `SessionRecord.loaded_skills` after turn 1 holds `code-review`.
4. **`a_frozen_skill_removed_mid_session_is_dropped_with_a_notice`.** Delete the skill directory between turns. Turn 2 writes `skill_missing` and runs. `skills.load` of that name fails with the "no skill named" message.
5. **`a_record_without_skill_fields_runs_unchanged`.** Use the old `test_session_record()` JSON with no skill keys. The turn runs, there is no `skill_index`, and the prompt is unchanged.

In `cli_session.rs`, replace `session_start_refuses_an_agent_that_declares_skills` and `a_session_turn_refuses_an_agent_that_declares_skills`. The new `session_start_accepts_an_agent_that_declares_skills` runs `session start --detach` with the mock provider. The session dir and `session.json` exist, `skill_index` and `skill_prompt` are populated, and a missing skill (`skills: [not-installed]`) is refused with the looked-in and install message.

- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement**

`start`:
- Load the agent as today, with the `Skill` error arm removed. The loader's names-only check still refuses an unknown name with its message.
- Resolve the session's skills: build a `RunAssembler` the way `run_turn` does, and call `resolve_skills_as(&agent, &Overrides { skills: args.skills, preload_skills: args.preload_skills, .. }, SkillReason::Session)`. On error, print it and abort before writing anything.
- Store `skill_index = resolved.freeze()` and `skill_prompt = resolved.prompt_section()`. Both are empty or `None` when the session has no skills.

`run_turn`:
1. Discover the catalog with `LayerDirs::for_layers(&global, session.project_root.map(join(".rupu")))`.
2. Call `rupu_skills::run::resolve_frozen(&catalog, &session.skill_index)`.
3. Write one `skill_missing` notice per missing name. The runner can't write it, because the skills are gone before assembly. Pass these as a notice list the runner writes at start, by extending `UserTurn` with `start_notices: Vec<(String, String)>`, or reuse an existing seam if there is one.
4. Set `LaunchServices.skills = Some(Arc::new(resolved))` when it is not empty, and `Overrides.skill_section = session.skill_prompt.clone()`.
5. Set `UserTurn.loaded_skills = session.loaded_skills.clone()`.
6. After the turn, on both `Ok` and `Err`, set `session.loaded_skills = exit.loaded_skills` (the `RunExit` field currently discarded by `..`) when the run started. Keep the previous set when the assembly failed.

`session_agent_spec`: keep `skills: None, preload_skills: None`. The agent's skills now arrive frozen through `LaunchServices.skills`, so the assembler must not resolve them again. Remove the refusal.

- [ ] **Step 4:** `cargo test -p rupu-cli --lib session`, `cargo test -p rupu-cli --test it cli_session::` (`< /dev/null`).
- [ ] **Step 5: Commit**

`feat(session): sessions freeze their skills at start and carry them across turns`

---

### Task 5: `session send --skill`: operator invocation

**Files:**
- Modify: `crates/rupu-cli/src/cmd/session.rs` (`SendArgs.skills`, `SessionTurnRequest.operator_skills`, the send-time check, `run_turn` injection)
- Modify: `crates/rupu-runtime/src/argv.rs` (`SessionStart.skills/preload_skills`, `SessionSend.skills`; flags before `--`), `crates/rupu-cli/tests/it/argv_roundtrip.rs`
- Test: in-crate `session.rs` tests; `cli_session.rs`

**Interfaces:**
- `rupu session send --skill <s> <id> [prompt]`, with `--skill` repeatable. The prompt becomes optional when at least one `--skill` is given, so the operator can invoke a skill with no extra text.
- `SessionTurnRequest.operator_skills: Vec<rupu_skills::run::FrozenSkill>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`). These are resolved at send time.
- `RunArgv::SessionStart { .., skills, preload_skills }` and `RunArgv::SessionSend { .., skills }` render `--skill`, `--preload-skill` and `--skill` respectively, each before `--`.

- [ ] **Step 1: Write the failing tests**

1. **`send_skill_injects_records_and_grants`.** A session whose agent has no skills. Send `--skill pdf` with the prompt "summarise x.pdf". Assertions:
   - The turn's `UserMessage` starts with `<skill name="pdf" base="…" version="…">` followed by pdf's body and `</skill>`, then a blank line and the prompt.
   - The transcript has `skill_load { name: "pdf", via: "operator" }`.
   - `SessionRecord.skill_index` gains `pdf` with reason `operator` and `indexed: false`.
   - In the next turn, a scripted `skills.load {name: pdf}` succeeds, and `read_file` of a file under pdf's base directory succeeds.
   - The system prompt is unchanged: still byte-identical to turn 1's, and pdf is not in the index.
2. **`send_skill_typo_is_refused_before_enqueue`.** `--skill pdff` exits non-zero with "did you mean pdf". Nothing is written to the session queue, and the session status is unchanged.
3. **`send_skill_accepts_project_and_disable_model_invocation_skills`.** A project skill invoked with no prefix works. So does a global skill marked `disable-model-invocation`, which is exactly what the flag exists for.
4. **`send_skill_refuses_wildcards_and_ambiguous_names`.**
5. **argv.** `SessionSend` with skills renders `session send --detach --skill pdf -- <id> <prompt>`. `argv_roundtrip` covers both variants. A `--skill`-only send with no prompt round-trips too.

- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement**

`send`, before `launch_turn`:
1. Resolve each `--skill` with `resolve_operator` against the session's catalog. On error, print it and exit 1 without enqueueing.
2. Pass the frozen entries into the request. This means extending `launch_turn` and `enqueue_turn_request` to carry them, and keeping their other callers passing an empty list.

`run_turn` with `request.operator_skills` non-empty (the worker passes them through `RunTurnArgs`, so add the field; `_run-turn` gets a hidden `--operator-skills-json` flag only if the test harness needs it, otherwise the worker's in-process call carries them):
1. Load each skill from disk at its current version.
2. Build the message: the injection blocks joined by `"\n\n"`, then `"\n\n"`, then the prompt (omitted when empty).
3. Set `UserTurn.operator_loads`.
4. Append the entries to `session.skill_index`, skipping names already present, and write the record before the turn. `resolve_frozen` for this turn then includes them, so `skills.load` and the read roots cover them in the same turn.
5. Never touch `skill_prompt`.

- [ ] **Step 4:** `cargo test -p rupu-cli --lib session`, `cargo test -p rupu-runtime --lib argv`, `cargo test -p rupu-cli --test it argv_roundtrip:: cli_session::` (`< /dev/null`).
- [ ] **Step 5: Commit**

`feat(session): session send --skill invokes any installed skill`

---

### Task 6: Session compaction re-attaches loaded skills

**Files:**
- Modify: `crates/rupu-cli/src/cmd/session.rs`: `compact` (the offline `rupu session compact`, around `:7106`) and `run_compact_request` (the worker's compact pseudo-turn, around `:7633`)
- Test: in-crate `session.rs` compaction tests (`SUMMARY_SCRIPT`)

- [ ] **Step 1: Write the failing tests.** Use a session with `loaded_skills: [code-review]` and its `skill_index`.
  - Run `compact` with `SUMMARY_SCRIPT`. The new `message_history[0]` contains the re-attach block with code-review's body, exactly once.
  - Compact a second time. The block still appears exactly once, because the marker skip works through `compact_messages`.
  - Run the same checks for `run_compact_request`.
  - A loaded skill uninstalled before compaction is listed under "No longer installed".
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.**
  1. After `compact_messages` returns, resolve the session's frozen set with `resolve_frozen` and call `rupu_agent::reattach_block(&session.loaded_skills, &resolved, cap)`. The cap is a quarter of the session's model input limit (`session.model_limits`), or 32 KiB.
  2. Push the block as a new text block on `messages[0]`, then set `session.loaded_skills = reattach.reattached`.
  3. The compact pseudo-turn writes the same notices the runner writes (`skill_changed`, `skill_missing`, `skill_reattach_truncated`) to its own transcript if it has one. Otherwise it logs them with `tracing::info`.
- [ ] **Step 4:** `cargo test -p rupu-cli --lib session`.
- [ ] **Step 5: Commit**

`feat(session): session compaction re-attaches loaded skills`

---

### Task 7: Docs and the spec

**Files:** `docs/skills.md`, `docs/transcript-schema.md`, `CLAUDE.md`, the spec, and this plan's line in CLAUDE.md's read-first list.

- [ ] **Step 1: `docs/skills.md`** gains a "Skills in sessions" section covering:
  - the set and prompt are fixed at start, and later installs aren't offered;
  - `session start --skill` and `--preload-skill`;
  - `session send --skill`, which can invoke any installed skill, plus the `<skill …>` block it injects;
  - loaded skills survive turns and compaction.
- [ ] **Step 2: `docs/transcript-schema.md`.**
  - `skill_load` `via: operator` now occurs.
  - `skill_index.stamp` is the session baseline for session turns.
  - The reason `session` now occurs.
- [ ] **Step 3: Spec §5.** It now matches the implementation:
  - frozen `FrozenSkill` entries and `skill_prompt`;
  - `operator_skills` live inside `skill_index` with reason `operator`;
  - the injection format;
  - the CP composer and remote session hosts are Plan 4.
- [ ] **Step 4: `CLAUDE.md`.** Update the `rupu-skills` entry (frozen sets, `resolve_operator`, the injection renderer), `rupu-agent` (`OperatorLoad`, `reattach_block`), `rupu-runtime` (`Overrides.skill_section`, `resolve_skills_as`) and `rupu-cli` (session skills). Mark Plan 3 in the read-first line.
- [ ] **Step 5: Commit**

`docs(skills): skills in sessions`

---

## Done when

- `cargo test` passes for `-p rupu-skills --test it`, `-p rupu-tools --test it`, `-p rupu-agent --test it --lib`, `-p rupu-runtime --test it --lib` and `-p rupu-cli --lib --test it`, with the CLI suites run with stdin from `/dev/null`. Clippy is clean on the touched crates.
- **By hand with a real provider:**
  1. `rupu session start <agent with skills: [word-count]> "How many words are in notes.txt?"` answers using the skill.
  2. `rupu session send <id> --skill word-count "and in README?"` shows the operator `skill_load`.
  3. `rupu session compact <id>`, then another `send`, still shows the skill re-attached.
  4. `rupu session show` lists the turns.
- Nothing in the CP API or remote session hosts changed. That is Plan 4.
