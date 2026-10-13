# Agent Skills Plan 4: the control plane

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The CP shows and edits everything skills touch:
- installed skills (a Library tab and a detail page);
- an agent's `skills` / `preloadSkills` (a picker in the agent editor, with unresolved names shown as a load error);
- a workflow step's `skills` / `preload_skills`;
- skills on run and session launches;
- the session composer's "invoke a skill" action.

Session skills reach a remote CP host only when it advertises `session.skills`.

**Architecture:**
- **Server.** A new read-only `api/skills.rs` (`GET /api/skills`, `GET /api/skills/:layer/:name`) lists the catalog through `rupu-skills`, the same way `rupu skill list/show` does.
  - `AgentDto` gains the two keys and a names-only skill check in `load_error`.
  - The session start and send API carries skill entries down to `rupu session start/send --skill`, through the argv Plan 3 added.
  - The HTTP connector refuses a peer without `session.skills`.
- **Web.** New API client calls, a Library "skills" tab and a skill page, a Skills card in the agent builder, step fields in the workflow editor, a skills picker in the launcher sheet, and an invoke-skill control in the session composer.

**Tech Stack:** Rust (axum, `rupu-skills`), TypeScript/React (vite, vitest, testing-library).

**Spec:** `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md` §8, §5.2. Plans 1 to 3 must be merged. Plan 3 adds `RunArgv::SessionStart { skills, preload_skills }` and `RunArgv::SessionSend { skills }`, and makes a `session send` prompt optional when `--skill` is given.

## Global Constraints

- **GUI.** The web work changes what the operator sees. Before merge, build the CP (`make cp-web`), run it against a scratch `RUPU_HOME` with installed skills, and screenshot every new or changed screen for matt.
- **No silent drops.** Every CP path that receives skill entries either delivers them or refuses with a 4xx/501 and a message:
  - a local send/start → the `rupu session` argv;
  - a remote host → refused unless the peer advertises `session.skills`.
- **Validation.** The API checks entry syntax only (`rupu_skills::parse_grants`; for operator sends, also no wildcards). Whether a skill is installed is decided by the run or session where it executes. Agent writes refuse malformed entries but accept unknown names, which then show in `load_error`.
- **Read-only listing.** `/api/skills` never loads more than the listing needs. The list view forces each entry's load to show its status, exactly like `rupu skill list`. The detail endpoint loads one.
- Web code follows the existing patterns: `SortableTable` lists, `Composite` tabs, `ChipsInput` pickers, `ErrorBanner`, colocated vitest tests (DOM tests start with `// @vitest-environment jsdom`).
- Rust: workspace deps, one `tests/it` binary, rustfmt only touched files, never package-wide `cargo fmt`. Never bare `git stash`.

## Review Focus

1. **A skill name with an invalid entry in a project layer** lists with its error and a status of `invalid`. The page never crashes on it. Pinned in Task 1 and Task 4.
2. **Saving an agent with `skills: [not-installed]`** succeeds. The agent then shows the "This agent can't be launched" banner naming the skill and the install hint. Pinned in Task 2 and Task 5.
3. **A session send with only a skill and no prompt** works from the composer and from the API. Pinned in Task 3 and Task 7.
4. **A session send to a remote CP that predates `session.skills`** is refused with 501 and an "upgrade rupu there" message. Nothing reaches the peer. Pinned in Task 3.
5. **A workflow saved in the editor** keeps `skills` / `preload_skills` after editing other fields of the step. Pinned in Task 6.

---

### Task 1: `GET /api/skills` and `GET /api/skills/:layer/:name`

**Files:**
- Create: `crates/rupu-cp/src/api/skills.rs`
- Modify: `crates/rupu-cp/src/api/mod.rs` (`pub mod skills;`), `crates/rupu-cp/src/server.rs` (`.merge(crate::api::skills::routes())`)
- Test: in-file tests in `skills.rs`

**Interfaces:**

```rust
#[derive(Serialize)]
pub struct SkillRow {
    pub name: String,
    pub layer: &'static str,          // "global" | "project"
    pub status: &'static str,         // "ok" | "invalid"
    pub description: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub stamp: Option<String>,        // short
    pub source: Option<String>,       // the source record's `source`
    pub both_layers: bool,            // the other layer has the same name
}
#[derive(Serialize)]
pub struct SkillsResponse { pub skills: Vec<SkillRow> }

#[derive(Serialize)]
pub struct SkillDetail {
    #[serde(flatten)] pub row: SkillRow,
    pub path: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Vec<String>,
    pub mapped_tools: Vec<&'static str>,
    pub disable_model_invocation: bool,
    pub metadata: std::collections::BTreeMap<String, String>,
    pub files: Vec<String>,
    pub content_hash: Option<String>, // short
    pub body: Option<String>,
    pub source_record: Option<serde_json::Value>, // the record as written
}
```

- `GET /api/skills`: the global layer only. `GET /api/skills?ws_id=<id>`: global plus that project's layer, through `code::load_workspace` (404 when unknown), then `rupu_workspace::project_layer_dir(&global, Some(Path::new(&w.path)))`, then `LayerDirs::for_layers`. Rows come sorted by name, then layer.
- `GET /api/skills/:layer/:name?ws_id=`: one entry. The response is 404 `{"error":"no installed skill …"}` when the skill is absent, 400 for a bad layer word, and 400 for a project layer requested without `ws_id`.

- [ ] **Step 1: Write failing tests.** Use the `api/tools.rs` pattern: `routes().with_state(test_state(&tmp))` and `oneshot`. Install the fixtures under `<tmp>/skills/…`, plus a registered workspace with `<ws>/.rupu/skills/…`.
  1. The global list returns `pdf` with status `ok`, its description, and `source: None`. Write a `.sources/pdf.json` and the next list shows its source.
  2. `?ws_id=` adds the project rows. A name present in both layers has `both_layers: true` on both rows.
  3. An invalid project skill (name mismatch) lists with status `invalid` and its error.
  4. A description over 1024 characters lists `ok` with a warning.
  5. The detail endpoint returns the body, the files without `SKILL.md`, `mapped_tools`, and the content hash. It returns 404 for an unknown name and 400 for a bad layer.
  6. An unknown `ws_id` returns 404.
- [ ] **Step 2: Run and watch them fail.**
- [ ] **Step 3: Implement.** Build the rows with `SkillCatalog::discover`, forcing `e.skill()` per entry. Read records with `rupu_skills::record::read_record`. Keep the IO off the async thread with `tokio::task::spawn_blocking`.
- [ ] **Step 4:** `cargo test -p rupu-cp --lib api::skills`.
- [ ] **Step 5: Commit.** `feat(cp): /api/skills lists and shows installed skills`

---

### Task 2: Agents carry skills in the API

**Files:**
- Modify: `crates/rupu-cp/src/api/agents.rs` (`AgentDto`, `from_spec`, `parse_agent_for_write`, its callers), `crates/rupu-cp/src/api/projects.rs` (`project_agents`)
- Test: in-file tests in `agents.rs`

**Interfaces:**
- `AgentDto.skills: Vec<String>` and `AgentDto.preload_skills: Vec<String>`. Each carries `#[serde(skip_serializing_if = "Vec::is_empty")]`.
- `AgentDto::from_spec(spec, …, skills_layers: Option<&LayerDirs>)` sets `load_error` from `validate_tools()` first. If that passes and the agent declares skills, it runs `rupu_agent::loader`'s names-only skill check against the given layers and renders the `AgentLoadError::Skill` message (it includes the install hint). Callers pass the layers for the scope the agent was listed from: global for global agents, global plus the project's for project agents. Discover once per layer set, not once per agent.
- `parse_agent_for_write` also runs `rupu_skills::parse_grants` on each key. A malformed entry returns 400 `"skills: entry …"` or `"preloadSkills: entry …"`. Unknown names are not refused.

- [ ] **Step 1: Write failing tests.**
  - `writing_an_agent_with_a_malformed_skill_is_refused` asserts the 400 message.
  - `writing_an_agent_with_an_unknown_skill_is_accepted_and_lists_its_load_error` asserts save succeeds, then `load_error` contains `unknown skill "nope"` and `rupu skill install`.
  - `an_agent_with_installed_skills_lists_them`: no `load_error`, and `skills` / `preload_skills` serialize.
  - `project_agents_check_skills_against_the_project_layer`.
- [ ] **Step 2:** Run and watch them fail.
- [ ] **Step 3:** Implement. Expose the names-only check from rupu-agent if it is private: a `pub fn check_agent_skill_names(spec, &SkillCatalog) -> Result<(), AgentLoadError>`.
- [ ] **Step 4:** `cargo test -p rupu-cp --lib api::agents api::projects`, `cargo test -p rupu-agent --lib`.
- [ ] **Step 5: Commit.** `feat(cp): agents list their skills and unresolved skill names`

---

### Task 3: Session start and send carry skills; remote CPs need `session.skills`

**Files:**
- Modify:
  - `crates/rupu-cp/src/session_starter.rs` (`SessionStartRequest.skills/preload_skills`)
  - `crates/rupu-cp/src/session_sender.rs` (`SendMessageRequest.skills`; prompt may be empty when skills are given)
  - `crates/rupu-cp/src/api/agents.rs` (`SessionStartBody.skills/preload_skills`; remote `start_session` maps `Unsupported` → 501)
  - `crates/rupu-cp/src/api/sessions.rs` (`SendBody { prompt: Option<String> (default), skills: Vec<String> }`; empty prompt allowed only with skills; operator entries refuse wildcards)
  - `crates/rupu-cp/src/host/http.rs` (`start_session` / `send_session_turn`: skills in the body; `require_in(CAP_SESSION_SKILLS)` when non-empty)
  - `crates/rupu-cp/src/node/protocol.rs` (`CAP_SESSION_SKILLS = "session.skills"` in `host_features()` only, because only HTTP hosts run sessions)
  - `crates/rupu-cli/src/cp_session_starter.rs`, `cp_session_sender.rs` (pass skills into the argv)
  - every literal the compiler reports (tunnel and bucket test literals, `StubConnector`)
- Test: in-file tests in `agents.rs` and `sessions.rs`; `crates/rupu-cp/tests/it/host_http.rs`; `crates/rupu-cli` adapter tests

- [ ] **Step 1: Write failing tests.**
  - **Local start:** `start_session_forwards_the_skills` (MockStarter records the request).
  - **Local send:** `send_session_forwards_operator_skills`; `send_with_only_a_skill_and_no_prompt_is_accepted`; `send_with_neither_prompt_nor_skill_is_400`; `send_with_a_wildcard_skill_is_400`.
  - **Remote (`tests/it/host_http.rs`):** `send_session_turn_delivers_skills_to_a_real_remote_cp`; `start_session_delivers_skills_to_a_real_remote_cp`; `session_skills_are_refused_by_a_remote_without_session_skills` (httpmock features list without the capability; the send endpoint is hit 0 times; the CP API answers 501 with "upgrade rupu"); `a_session_send_without_skills_posts_no_skills_key`.
  - **CLI adapters:** the argv contains `--skill pdf` before `--`, and a skill-only send renders no prompt.
- [ ] **Step 2:** Run and watch them fail.
- [ ] **Step 3:** Implement following the `launch_agent` skills pattern in `http.rs` (the peer-info read condition, `require_in`, body keys only when non-empty). The local path relies on the CLI's own resolution: `rupu session send --skill` resolves operator entries (Plan 3, `resolve_operator_skills` with the session's held set) and exits 1 with a message on a typo, wildcard or layer conflict. `cp_session_sender` must turn that non-zero exit into `SendError::Invalid(<the CLI's stderr message>)`, which the API returns as 400. A test feeds a typo through the real adapter path, or a stub that exits 1 with the CLI's text, and asserts the 400 body. `cp_session_starter` does the same for `session start --skill`.
- [ ] **Step 4:** `cargo test -p rupu-cp --lib`, `cargo test -p rupu-cp --test it`, `cargo test -p rupu-cli --lib`.
- [ ] **Step 5: Commit.** `feat(cp): session start and send carry skills; remote hosts need session.skills`

---

### Task 4: Web: API client, Library "skills" tab and skill page

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (`SkillRow`, `SkillDetail` types; `api.getSkills(wsId?)`, `api.getSkill(layer, name, wsId?)`; `AgentSummary.skills?`, `preload_skills?`; `startSession`/`sendSessionMessage`/`launchAgent` accept `skills`/`preload_skills`), `components/v2/pages/LibraryV2.tsx` (a `skills` tab), `App.tsx` (route `/skills/:layer/:name`)
- Create: `pages/Skills.tsx`, `pages/SkillDetail.tsx`, and tests `pages/Skills.test.tsx`, `pages/SkillDetail.test.tsx`, `lib/api.skills.test.ts`

**Behaviour:**
- **Skills list.** A `SortableTable` with columns:
  - name (links to the detail page);
  - layer (`ScopeChip`-style badge);
  - status (an "invalid" badge carries the error as its title);
  - source (`-` when none);
  - description (truncated).

  A project selector, the same control other Library tabs use when they scope by project, passes `ws_id`. Use `EmptyState` with text that says how to install: "Install skills with `rupu skill install <source>`".
- **Skill detail.** A header with name, layer and source, then sections for:
  - description;
  - compatibility;
  - license;
  - allowed-tools with the rupu mapping, noted "informational, never a grant";
  - metadata;
  - files;
  - the body rendered as markdown (the transcript view has a markdown component; reuse it);
  - the content hash.

  An invalid skill shows an `ErrorBanner` with its error. There are no edit actions; install and update stay CLI operations.

- [ ] **Step 1: Write failing tests.**
  - **`api.skills.test.ts`:** `fetch` is called with `/api/skills?ws_id=w1` and the response is parsed.
  - **`Skills.test.tsx`:** rows render; an invalid row shows its badge and error title; the empty state shows the install hint.
  - **`SkillDetail.test.tsx`:** the body renders; `SKILL.md` is absent from the file list; the invalid banner shows.
- [ ] **Step 2:** Run `npm test -- --run` and watch them fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `npm test -- --run`, `npm run build` (the typecheck plus vite build).
- [ ] **Step 5: Commit.** `feat(web): Library skills tab and skill page`

---

### Task 5: Web: Skills card in the agent builder

**Files:**
- Modify: `lib/agentBuilder/agentSpec.ts` (`AgentDraft.skills?: string[]`, `preloadSkills?: string[]`, `KEY_ORDER` in the Rust `Frontmatter` order (`skills` and `preloadSkills` follow `findingsProfile`), parse and serialize), `lib/agentBuilder/fields.ts` (`CARD_REGISTRY` entry `skills`), `components/agentBuilder/AgentBuilder.tsx` (`CARD_FIELDS.skills = ['skills', 'preloadSkills']`, `CANVAS_ORDER`, `renderCardBody`, a new `skillNames` prop), `pages/AgentNew.tsx` and `pages/AgentDetail.tsx` (fetch `api.getSkills(wsId)` and pass the names; for a project-scoped agent pass its `scope_id`)
- Create: `components/agentBuilder/cards/Skills.tsx` (two `ChipsInput`s, "Skills the agent may load" and "Always in its prompt (preload)", with suggestions from `skillNames` plus `*` and `project:*`; helper text: "`*` means every global skill; project skills need their exact name or `project:*`")
- Test: `agentSpec.test.ts` (parse/serialize round-trip with both keys, order preserved), `cards/Skills.test.tsx`

- [ ] **Step 1: Write failing tests.** Round-trip `skills: [pdf, "project:*"]` and `preloadSkills: [house-style]` through `parseAgent` and `serializeAgent`, and check that the keys are no longer in `_passthrough`. The card shows suggestions and adding a chip patches the draft.
- [ ] **Step 2:** Run and watch them fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `npm test -- --run`, `npm run build`.
- [ ] **Step 5: Commit.** `feat(web): skills card in the agent builder`

---

### Task 6: Web: skills on workflow steps

**Files:**
- Modify: `lib/workflowGraph.ts` (`StepNodeData.skills?`, `preload_skills?`; parse; add both to `MODELLED_STEP_KEYS`; emit only when non-empty; `acceptsSkills(d)` mirrors the server rule: not when the step runs no agent and not on `action:` steps), `components/workflow-editor/StepForm.tsx` (a `SkillsField` with the two chip inputs and installed-skill suggestions, mounted where `FindingsProfileField` is, and carried by `switchKind` like `actions`/`findings_profile`), `WorkflowEditor.tsx` (fetch `api.getSkills(wsId)` once and pass the names down)
- Test: `workflowGraph.test.ts`, `StepForm` tests

- [ ] **Step 1: Write failing tests.**
  - `workflowGraph`: both keys survive a parse, an edit of a different step field, and an emit (Review Focus 5). Empty arrays are not emitted.
  - `acceptsSkills` is false for a gate, a branch and an `action:` step.
  - `StepForm` renders the field for an agent step and hides it for a gate.
  - Switching an agent step's kind to `parallel` keeps the skills.
- [ ] **Step 2:** Run and watch them fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `npm test -- --run`, `npm run build`.
- [ ] **Step 5: Commit.** `feat(web): skills on workflow steps in the editor`

---

### Task 7: Web: skills at launch and in the session composer

**Files:**
- Modify: `components/AgentLauncherSheet.tsx` (a collapsed "Skills" section with two `ChipsInput`s, suggestions from `api.getSkills(wsId)`; sent on run and session launches), `pages/SessionDetail.tsx` (an "Invoke skill" control next to the composer's send button: a chip input of installed skill names for the session's project, so `SessionSummary` gains `workspace_id` in `api.ts`, which the server already serializes; send enables when the prompt is non-empty or a skill is chosen; on success, clear both), `lib/api.ts` (the bodies)
- Test: `AgentLauncherSheet` tests, `SessionDetail` tests

- [ ] **Step 1: Write failing tests.**
  - The launcher sends `skills` / `preload_skills` for a run and for a session.
  - The composer is disabled when empty, enabled with only a skill, and calls `api.sendSessionMessage(id, '', host, ['pdf'])`.
  - A 501 from the API shows its message in the composer's error area.
- [ ] **Step 2:** Run and watch them fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `npm test -- --run`, `npm run build`.
- [ ] **Step 5: Commit.** `feat(web): skills at launch and an invoke-skill action in sessions`

---

### Task 8: Visual check, docs

**Files:** `docs/skills.md` (a "In the control plane" section), `docs/cp-api.md` (`/api/skills`, the agent fields, the session start/send fields, `session.skills`), `CLAUDE.md` (`rupu-cp` entry; mark Plan 4 complete), spec §8.

- [ ] **Step 1:** Run `make cp-web`, then `cargo build -p rupu-cli`. Start `rupu cp serve --bind 127.0.0.1:<free port>` with `RUPU_HOME` pointing at a scratch home. That home holds two installed global skills (one invalid), a registered project with a project skill, an agent with `skills: [pdf, not-installed]`, a workflow whose step has `skills`, and a session.
- [ ] **Step 2:** Open each screen in the built-in browser and save a screenshot to the scratchpad:
  - the Library skills tab (global, and with the project selected);
  - a skill page;
  - an invalid skill page;
  - the agent builder Skills card;
  - the agent detail page with the unresolved-skill banner;
  - the workflow step editor with the skills field;
  - the launcher sheet's Skills section;
  - the session composer with a skill chosen.

  Check each against the behaviour above and fix what's wrong. Send the screenshots to matt (`SendUserFile`).
- [ ] **Step 3:** Docs.
- [ ] **Step 4: Commit.** `docs(skills): the control plane`

---

## Done when

- `cargo test -p rupu-cp --lib --test it`, `-p rupu-cli --lib --test it` (stdin `/dev/null`), `-p rupu-agent --lib`; `npm test -- --run` and `npm run build` in `crates/rupu-cp/web`; clippy is clean.
- The screenshots from Task 8 have been sent to matt, and every screen behaves as described.
