# rupu: Agent Skills for agent runs, workflows, flows and sessions

- **Date:** 2026-10-12
- **Status:** draft, for matt's review
- **Builds on:** the tool and launch architecture (`docs/superpowers/specs/2026-10-07-rupu-tool-and-launch-architecture/`): W1 catalog, W2 grants, W3 run assembler.

## Context

An agent's only instructions today are its markdown body. `system_prompt_for` (`crates/rupu-runtime/src/assembly/mod.rs:732`) appends a `## Run target` section and nothing else. Reusable know-how (a review playbook, a report style, a tool's quirks, a script that parses a format) gets copied into every agent that needs it, or it is missing.

The industry answer is the Agent Skills format (agentskills.io). A skill is a directory with a `SKILL.md` (name, description, instructions) plus optional scripts, references and assets. Claude Code, Codex CLI, Gemini CLI and the Anthropic API all read it. The model sees each skill's name and description, loads a skill's body when a task matches, and reads or runs the skill's files only when the body says to. This is progressive disclosure: a skill costs about 100 tokens until it is used.

rupu has one precedent. A concern catalog in index mode puts a summary table in the prompt and serves bodies through `coverage.concerns.detail` (`crates/rupu-coverage/src/catalog/render.rs:9`, `crates/rupu-tools/src/coverage/mod.rs:289`). It is specific to security concerns and the coverage ledger. Skills are the general mechanism.

## Goals

1. **Full format compatibility.** A skill published for Claude Code or Codex installs and works in rupu unchanged, scripts included.
2. **Every launch kind.** `rupu run`, workflow steps (fan-out units and panel members included), sub-agents, agentiflow leads and units, and session turns all get skills through the one run assembler.
3. **Explicit grants.** An agent sees only the skills it is granted. No `skills:` key means no skills.
4. **Sessions have an operator.** The operator can invoke any installed skill in a session turn.
5. **Skills survive long runs.** A loaded skill's body is not lost to context compaction.
6. **Auditable.** The transcript says which skills a run was offered, which it loaded, by whom, and which version.

## Non-goals

- **Runtime acquisition.** An agent never installs, fetches or adopts a skill during a run. If it reads a `SKILL.md` it found in a repo, that is just a file it read. Installing is an operator act.
- **Discovering other harnesses' directories.** rupu does not scan `.claude/skills`, `.agents/skills` or `.codex/skills`. Doing so would let an assessed repo plant instructions and scripts that our agents then run. The operator copies a skill in with `rupu skill install`.
- **The customer layer.** Agents and workflows are found in the global and project layers only (`crates/rupu-agent/src/loader.rs:48`), and the customer store holds only `customer.toml` and `config.toml`. Skills follow agents.
- **Shipping skills to remote hosts.** A placed unit loads its agent on the host (below). Skills follow agents there too.
- **Provider-native skills.** Anthropic's server-side Skills API (`container.skills`) needs Anthropic's code-execution container. rupu serves skills itself, so they work on every provider.
- **Engagement-profile bundles.** `[bundle]` is a launcher prefill that never grants anything (`crates/rupu-coverage/src/profile/package.rs:73`). Stock skills shipped by `rupu fleet install` can follow later.
- **`rupu mcp serve` and `action:` steps.** Neither provides the skills service, so neither offers `skills.load`.
- **Merging concerns into skills.** Concerns carry severity, globs and ledger semantics. They stay as they are.

## Decisions (from the design conversation)

| # | Decision |
|---|---|
| S1 | Adopt the agentskills.io format as it is, scripts included. |
| S2 | Skills are granted explicitly. `skills:` absent means none. `skills: ["*"]` means every installed skill. |
| S3 | Within its grant the model chooses: it sees an index and loads a body when it judges a task matches. |
| S4 | Installed means present in a config layer. Agents never acquire skills at runtime. |
| S5 | A skill never widens the run's tool grant or permission mode. Scripts run through `bash` under both. |
| S6 | In a session, the operator may invoke any installed skill. Naming it is the consent. |
| S7 | Sessions fix their skill index at start. A skill that changes on disk mid-session is used at its new version, with a notice. |
| S8 | Loaded skill bodies are re-attached after compaction. |
| S9 | The project layer is untrusted by default: `*` covers the global layer only, a project skill needs an exact grant, and a name in both layers is an error until the grant says which (§2.1). Added after review: an assessed repo can plant `.rupu/skills/` as easily as `.claude/skills/`. |

## 1. The skill model

### 1.1 Format

A skill is a directory whose name is the skill's name and which contains `SKILL.md`:

```
<layer>/skills/pdf-processing/
├── SKILL.md
├── scripts/extract.py
├── references/REFERENCE.md
└── assets/template.docx
```

`SKILL.md` frontmatter, per the agentskills.io specification:

| Field | Rule rupu enforces |
|---|---|
| `name` | Required. 1 to 64 chars, `a-z`, `0-9`, `-`, no leading, trailing or doubled `-`. Must equal the directory name. |
| `description` | Required. 1 to 1024 chars. |
| `license` | Optional, free text. |
| `compatibility` | Optional, 1 to 500 chars. Shown by `rupu skill show` and in the load result. |
| `metadata` | Optional string-to-string map. Kept, not interpreted. |
| `allowed-tools` | Optional, experimental. A space-separated string per the specification; a YAML list is also accepted, since some harnesses write one. Read, never enforced as a grant (§3.5). |
| `disable-model-invocation` | Claude Code extension. When `true` the skill is left out of the index (§3.2). |
| anything else | Accepted and ignored. Unlike agent frontmatter, `SKILL.md` is not `deny_unknown_fields`: harnesses add their own keys, and rejecting them would break S1. |

The body is the instructions. Paths in it are relative to the skill's directory.

A skill is **invalid** when its frontmatter fails a rule above, or when a symlink inside its directory resolves outside it. In the global layer the skill directory itself may be a symlink (people link `~/.claude/skills/x` in). In the project layer it may not, or a repo could point a read root (§3.4) anywhere on the machine. An invalid skill is listed with its error, the same way an unparseable agent is, and granting it by name fails the run.

### 1.2 Layers and discovery

Skills live in `skills/` under the global dir (`~/.rupu/skills/`) and the project's `.rupu/skills/`. Discovery takes the project layer from `rupu_workspace::config_paths`, so the R5 rule applies: when `project_root_for` resolves to `$HOME`, `~/.rupu` is the global layer only, never both.

Unlike agents, a project skill does not shadow a global one. The catalog keeps both, and §2.1 decides which a grant binds.

### 1.3 Version identity

Two fingerprints, so that a run start never reads a skill's whole tree:

Both walk the same entries: the tree's regular files and symlinks, skipping `.git` and `.DS_Store` at any depth, ordered by their `/`-joined relative path compared as bytes. Anything else in the tree (a FIFO, a socket, a device) and any symlink that leaves the tree or resolves into a skipped name refuses the skill. A file's mode is normalized git-style: `755` when any exec bit is set, else `644`, so a umask or `chmod g+w` changes neither fingerprint. Empty directories don't count.

- **Stamp**: sha256 over each entry as `relative path, NUL`, then for a file `f, NUL, byte length, NUL, mtime in nanoseconds since the Unix epoch, NUL, mode in octal, LF` and for a symlink `l, NUL, target path, LF`. It costs one `stat` per file. The index records it (§4), and a changed stamp is how a load knows the skill changed. Because it reads no bytes, it misses a same-size edit whose mtime was put back (`cp -p`, `touch -r`), so `skill_changed` can be missed in that case.
- **Content hash**: sha256 over each entry as `relative path, NUL`, then for a file `f, NUL, mode in octal, NUL, byte length, NUL, bytes` and for a symlink `l, NUL, target path`, each entry followed by an LF. It is computed only when a skill is loaded or preloaded, and `skill_load` records it as the version.

There is no cross-run cache. Computing the content hash only on load keeps `skills: ["*"]` on a fan-out of 50 units from hashing every installed `assets/` 50 times.

### 1.4 Where the code lives

A new leaf crate, **`rupu-skills`**, with no rupu dependencies. It holds the `Skill` model, `SKILL.md` parsing and validation, discovery over a `(global, project)` pair, the stamp and content hash, the `allowed-tools` name map (§3.5), and the index and preload renderers. `rupu-tools` (the `skills.load` body), `rupu-agent` (load-time validation), `rupu-runtime` (assembly), `rupu-cli` and `rupu-cp` all depend on it. It can't live in `rupu-agent`, because `rupu-tools` needs it and `rupu-agent` depends on `rupu-tools`.

```rust
pub struct Skill {
    pub name: SkillName,          // validated per §1.1
    pub description: String,
    pub root: PathBuf,            // canonical directory
    pub layer: Layer,             // Global | Project
    pub frontmatter: Frontmatter, // license, compatibility, metadata, allowed_tools, disable_model_invocation
    pub body: String,
}

pub struct SkillCatalog { /* name → Result<Skill, SkillError> */ }
```

## 2. Granting skills

### 2.1 Agent frontmatter

Two new keys on `Frontmatter` and `AgentSpec` (`crates/rupu-agent/src/spec.rs:28`, `:174`):

```yaml
skills: [pdf-processing, code-review]   # the model may load these
preloadSkills: [house-style]            # always in the system prompt; implies granted
```

Grant grammar (S9):

| Entry | Binds |
|---|---|
| `code-review` | the skill of that name in whichever layer has it. If both layers have it, the grant is an error naming both, until it is qualified. |
| `global:code-review`, `project:code-review` | that layer's skill |
| `*` | every valid **global** skill |
| `project:*` | every valid project skill, an explicit opt-in |

Skill names contain no `.`, so W2's `ns.*` form has no meaning here. Absent or empty means none (S2).

A run binds each name once: a grant set that picks the same name from both layers (`["*", "project:*"]` over a name installed in both, or `global:x` with `project:x`) is an error, because the index would list the name twice and `skills.load` takes only a name.

The collision rule is deliberate. If a bare name preferred the project copy, a repo could replace a global skill an agent relies on, scripts included. If it preferred the global copy, a project author could never override one. Refusing the ambiguity makes the author choose, and the transcript records the layer either way.

Validation joins `validate_tools` in `load_agent` (`crates/rupu-agent/src/loader.rs:88`). A malformed entry, an unknown or invalid name, or an unqualified collision is `AgentLoadError::Skill`, with a did-you-mean. `check_agent_files` (`rupu agent validate`) checks the same. `load_agents` and `find_agent` still only parse. Every `AgentSpec` struct literal, `session_agent_spec` (`crates/rupu-cli/src/cmd/session.rs:7750`) among them, gains the fields.

Load-time validation is a convenience. The assembler resolves `ResolvedSkills` itself and fails closed (`AssembleError::Skill`, recorded through `record_unstarted`), because some specs never pass through `load_agent`: a session turn's spec is rebuilt from its record.

### 2.2 Workflow steps

`Step` (`crates/rupu-orchestrator/src/workflow.rs:1049`) gains `skills: Vec<String>` and `preload_skills: Vec<String>`. They add to the grant of every agent the step launches: the step's agent, its `for_each` units, its parallel and panel members, a panel gate's `fix_with` agent, and loop members. They never remove a skill.

Workflow parsing stays pure: it checks only entry syntax, so a workflow's validity never depends on what a host has installed. Existence is checked when the step launches, in `DefaultStepFactory::launch_for_step`, the way a missing agent is (`AssembleError::AgentLoad`). The skills travel on `Origin::WorkflowStep` next to `step_actions` (`crates/rupu-orchestrator/src/step_factory.rs:101`).

There is no workflow-level `skills:`. A workflow that wants a skill on every step names it per step, or on the agents. When yokoito replaces workflow definitions, its integration maps its own step key onto the same `Origin::WorkflowStep` fields.

### 2.3 Other launch kinds

| Origin | Skills come from |
|---|---|
| `Standalone` (`rupu run`) | the agent |
| `WorkflowStep` | the agent + the step (§2.2) |
| `SubAgent` | the child agent only. A child does not inherit its parent's skills, the same as tools. |
| `FlowLead`, `FlowUnit` | the agent |
| `SessionTurn` | the session record (§5) |

### 2.4 The resolved set

The assembler resolves one `ResolvedSkills` per run, next to `ResolvedGrant`:

```rust
pub struct ResolvedSkills {
    pub entries: Vec<GrantedSkill>,
}
pub struct GrantedSkill {
    pub skill: Arc<Skill>,
    pub stamp: Stamp,
    pub preload: bool,
    pub indexed: bool,        // false for disable-model-invocation and for preloaded skills
    pub reason: SkillReason,  // Agent | Step | Session | Operator
}
```

The skills service (`ToolServices.skills: Option<Arc<ResolvedSkills>>`, `Service::Skills`, with its arm in `ToolServices::provided`) is present when `entries` is non-empty. `AmbientGrant::skills()` then offers `skills.load` as `Ambient("skills")`. An agent with no skills gets no loader tool and no index. `actions:` never narrows it, since it is not a connector tool.

## 3. What the model sees

### 3.1 The index

`system_prompt_for` builds the assembler's part of the system prompt: the agent body, the `## Run target` section, then the **skills section** (preloaded skills, then the index). The runner appends its own sections after that, as today: the concern catalog, findings guidance and the call-sign line (`crates/rupu-agent/src/runner.rs:1954`, `:2005`, `:2012`). Sessions store the body and run target pre-joined (`crates/rupu-cli/src/cmd/session.rs:1650`), so the same order holds there with the skills section appended to the stored prompt. Indexed skills render as:

```
## Skills

Each skill below holds instructions, and sometimes scripts and reference files,
for one kind of task. When a task matches a skill's description, call
`skills.load` with its name before you start, and follow it. Paths inside a
skill are relative to the base directory that `skills.load` returns.

- pdf-processing: Extract PDF text, fill forms, merge files. Use when handling PDFs.
- code-review: ...
```

The order is by name, so the prompt is byte-stable for a given set. Concerns keep rendering in the runner (`crates/rupu-agent/src/runner.rs:1943`). Moving them is out of scope.

### 3.2 Preloaded skills

A preloaded skill renders its whole body into the system prompt, under `## Skill: <name>` with its base directory. It is not listed in the index. The system prompt is never compacted, so preloaded skills need no re-attach.

A skill marked `disable-model-invocation: true` is reachable only by preload or by an operator invocation. Granting it in `skills:` without preloading is allowed. It writes a `skill_not_indexed` notice at run start, so the author sees why the model never loads it.

### 3.3 `skills.load`

A catalog tool in `rupu-tools` (`skills/`):

- **Descriptor:** `skills.load`, `Effect::Read`, `needs: [Service::Skills]`, not action-eligible, `wildcard: true`.
- **Input:** `{ "name": string }`.
- **Output (text):**

  ```
  Skill: pdf-processing (version 3f2a9c…)
  Base directory: /Users/x/.rupu/skills/pdf-processing
  Files: scripts/extract.py, references/REFERENCE.md, assets/template.docx
  Compatibility: Requires Python 3.12+ and pypdf

  <body>
  ```

- A name outside the run's grant fails with the granted names listed. The model cannot load a skill it was not given, even one installed.
- A skill whose current stamp differs from the stamp in the run's `skill_index` loads at its current version, the result begins with a line that says so, and a `skill_changed` notice is written (S7). A granted skill that has been deleted since the index fails with that reason. Runs other than sessions resolve their index at start, so this matters mostly for long runs and sessions.

### 3.4 Reading and running a skill's files

Skill bodies tell the model to read `references/X.md` or run `scripts/y.py`. Two existing tools do that once they can reach the skill's directory:

- **Reading.** `read_file`, `grep`, `glob` and `ast_grep` refuse paths outside the workspace (`crates/rupu-tools/src/path_scope.rs:9`). `WorkspaceScope` gains `read_roots: Vec<PathBuf>`, set by the assembler to the canonical roots of every granted skill, project skills included (a run that targets a clone has its workspace elsewhere). `path_scope` gains `is_readable(path)`: inside the workspace or inside a read root, after canonicalization. The four read tools use it. `write_file` and `edit_file` keep `is_inside`, so a skill's files are read-only to the model.
  - `glob` walks from `ctx.workspace.path` (`crates/rupu-tools/src/glob.rs:65`). An absolute pattern under a read root walks from that root instead. `grep` and `ast_grep` take their search path the same way.
  - A read under a read root emits no `FileTouchEvent`. Skill files are not target files, and counting them would inflate coverage.
- **Running.** `bash` has no path check today: it runs `/bin/sh -c` in the workspace with a cleared environment (`crates/rupu-tools/src/bash.rs:214`). `python3 /Users/x/.rupu/skills/pdf-processing/scripts/extract.py in.pdf` already works. Nothing changes. Scripts are governed by the run's permission mode (`bash` is `Effect::Write`: readonly denies it, ask prompts) and `[bash]` timeout and env allowlist.

`skills.read` and a separate script-runner tool were considered and dropped. Every published skill is written for a model that uses its harness's ordinary read and shell tools. A second path would be one more thing to learn and one more thing to permission.

### 3.5 `allowed-tools`

In Claude Code, `allowed-tools` pre-approves tools while a skill is active. In rupu the run's resolved grant and permission mode are the ceiling (S5). `rupu-skills` maps the common harness names (`Bash` → `bash`, `Read` → `read_file`, `Write` → `write_file`, `Edit` → `edit_file`, `Grep` → `grep`, `Glob` → `glob`; argument patterns like `Bash(git:*)` reduce to the tool) and ignores the rest. When a skill loads, the runner writes a `skill_tools_missing` notice naming mapped tools the run was not granted. The load still succeeds. The model then knows why a step the skill describes will be refused.

## 4. Transcript and audit

Two new events in `rupu-transcript` (the `Event` enum at `crates/rupu-transcript/src/event.rs:73`; both tags added to `KNOWN_EVENT_TAGS` at `:17`; both documented in `docs/transcript-schema.md`):

- **`skill_index`**, written once at run start after `tool_grant`: one row per granted skill with name, layer, stamp, `preload`, `indexed` and reason.
- **`skill_load`**: name, layer, content hash, base directory, and `via`: `model` (a `skills.load` call), `preload`, `operator` (§5.2) or `reattach` (§6). Written for every load, whichever path made it.

`skills.load` calls also get the usual `tool_audit` line. New notice kinds: `skill_not_indexed`, `skill_changed`, `skill_missing`, `skill_tools_missing`, `skill_reattach_truncated`. `rupu transcript` and the CP transcript view render both events with the shared one-line renderers, as `tool_grant` does.

## 5. Sessions

### 5.1 Fixed at start

`SessionRecord` (`crates/rupu-cli/src/cmd/session.rs:376`) already snapshots the agent at start. It gains:

- `agent_skills`, `agent_preload_skills`: the agent's keys.
- `skill_index`: the resolved set, with names, layers, stamps and flags.
- `skill_prompt`: the rendered skills section, preloaded bodies included, exactly as the first turn sent it.
- `operator_skills`: skills the operator added later (§5.2).
- `loaded_skills`: the run's loaded set (§6) as of the last turn, since each turn writes its own transcript (`crates/rupu-cli/src/cmd/session.rs:1848`).

`rupu session start --skill <name>` (repeatable, like `--engagement-profile`) adds to the grant at start. Every turn appends the stored `skill_prompt` instead of rendering one, so the system prompt is byte-identical turn to turn and the prompt cache holds. That freezes preloaded bodies for the session's life; a session that wants a newer version of a preloaded skill is a new session. A skill installed after the session started is not offered, even under `*`. A skill the model loads that changed on disk loads at its new version with a `skill_changed` notice (S7).

### 5.2 Operator invocation

`rupu session send --skill <name> [prompt]` (repeatable) invokes a skill in that turn:

1. Look the name up in the session's catalog: the global layer plus the project layer of the session's `project_root`. Any valid installed skill qualifies (S6), granted or not, project skills included: the operator naming one is the opt-in that S9 otherwise requires of the grant. The `global:` and `project:` prefixes work as in §2.1, and an unqualified name in both layers refuses the send. An unknown name refuses it with a did-you-mean.
2. Prepend the skill to the turn's user message, before the operator's prompt:

   ```
   <skill name="pdf-processing" base="/Users/x/.rupu/skills/pdf-processing" version="3f2a9c…">
   ...body...
   </skill>
   ```

3. Write `skill_load { via: operator }`.
4. Add the skill to `operator_skills`. From the next turn on, it is in the run's `ResolvedSkills` with reason `Operator`: the model may `skills.load` it again, and its root is a read root. It is not added to the index, so the system prompt does not change.

A skill with `disable-model-invocation: true` is invocable this way. That flag exists for exactly this case.

The CP session composer gets the same action in the CP plan (§8).

## 6. Compaction

Compaction keeps the task text, a summary and the recent messages (`crates/rupu-agent/src/runner.rs:318`). A skill loaded early is summarised away. The agent keeps going without the instructions it was following.

The runner keeps the run's **loaded set**: the skills whose latest `skill_load` is not `preload`. Replay rebuilds it from `skill_load` events, so a continued run has it too. A session seeds it from `loaded_skills` in its record (§5.1), because its earlier turns' events are in earlier transcripts.

`rebuild_compacted` (`crates/rupu-agent/src/runner.rs:318`) puts the re-attached skills in their own text block of the rebuilt first message, after the task-and-summary block. The block opens with a fixed `<skills-in-use>` marker. `compact_messages` builds its task text from `messages[0]`'s text blocks (`:486`), and it skips a block that opens with the marker, so a second compaction neither copies nor doubles the skills. The rebuilt message is in `Event::Compaction.messages`, so replay stays exact.

Each re-attached skill is read at its **current** version, so a skill edited mid-run comes back as it is now. A changed one gets a `skill_changed` notice, and a `skill_load { via: reattach }` records the version. A skill that is no longer installed is listed by name as missing, with a `skill_missing` notice.

Re-attached bodies are capped at a quarter of the model's input limit, or at 32 KiB of text when the limit is unknown. Without a cap, the first message would never shrink, which defeats compaction, and `trim_oldest_exchange` never trims `messages[0]`. Most recently loaded skills come first. Skills that don't fit are listed by name with "reload with `skills.load`", and a `skill_reattach_truncated` notice is written. The cap limits only what is re-attached up front: the model can still load any skill in full.

## 7. CLI

`rupu skill`, registered next to `rupu fleet` (`crates/rupu-cli/src/lib.rs:91`), thin over `rupu-skills`:

| Command | Does |
|---|---|
| `rupu skill list [--project]` | name, layer, description, source, and the error for invalid skills |
| `rupu skill show <name>` | frontmatter, files, layer, stamp and content hash, source record, and whether the other layer has the same name |
| `rupu skill validate [<name>\|--all]` | §1.1 rules; exit 1 on any failure |
| `rupu skill install <source> [--ref <r>] [--skill <n>]... [--all] [--list] [--project] [--force]` | find the skills in a source and copy the chosen ones into the layer |
| `rupu skill update [<name>...\|--all] [--project] [--force]` | re-fetch installed skills from their recorded source |

There is no `remove`: deleting the directory is enough, and an orphaned source record is ignored.

### 7.1 Sources

Skills are published the way the rest of the ecosystem installs them (`npx skills add`, Claude Code plugin marketplaces): a git repository holding one or many skill directories, often under `skills/<name>/`. `install` accepts what people copy:

| Source | Resolves to |
|---|---|
| an existing path (`./x`, `/abs/x`, `~/x`) | that local directory |
| `owner/repo` | `https://github.com/owner/repo` |
| `https://github.com/o/r/tree/<ref>/<path>` (or `/blob/<ref>/<path>/SKILL.md`) | that repo, ref and path. The ref is the first segment after `tree/`, so a branch with `/` in its name needs `--ref` and `#<path>` instead. |
| any `https://` git URL, with optional `#<path>` | that repo and path |

`--ref <branch|tag|commit>` picks a ref; it conflicts with a different ref in a tree URL. Only `https://` is fetched (`ssh`, `git@` and `file://` are refused: a local checkout is a path). The fetch is libgit2: a shallow clone of the default branch, or a full clone checked out at `--ref`. libgit2 runs no hooks and does not touch submodules.

A tree URL that names a path takes no `#<path>` as well. The scheme is matched case-insensitively. A URL carrying credentials (`https://user:token@…`) is refused without echoing it, since the source record would store the token; private repositories are not supported yet (an authentication failure says so), and the operator clones one and installs the directory. A `#<path>`, tree-URL path or recorded path is relative, has no `..`, root or drive prefix, and must resolve inside the clone after symlinks: a repository can commit `skills -> /anywhere`.

### 7.2 Finding and choosing skills

Within the source (the repo root, or `<path>` inside it), every directory holding a `SKILL.md` is a skill. The search descends through hidden directories (`.claude/skills/`, `.agents/skills/`, plugin folders), skips `.git` and `node_modules`, stops at each skill it finds, and goes at most 6 levels deep. Two skills with the same directory name in one source is an error naming both paths; the operator narrows with `#<path>`.

- One skill found: it is installed.
- Several found: `--skill <name>` (repeatable) picks, `--all` takes every valid one (invalid ones are reported and skipped), and without either the command lists them and installs nothing. `--list` lists without installing in any case.
- An explicitly named skill that is invalid fails the install before anything is written.
- An installed skill is named by its frontmatter `name`, whatever its directory is called in the source.

Each install validates first, refuses to overwrite without `--force`, refuses a tree containing a symlink that leaves it or an absolute symlink, preserves file modes so scripts stay executable, and swaps by rename, so a failed install never damages an installed skill. A replace moves the installed skill to `.replaced-<name>-<pid>` before renaming the new copy in; if a crash lands between the two renames, the next install or update of that name renames the backup back first. A skill named twice is installed once. Each skill's source record (§7.3) is written as soon as that skill lands, so a failure part way through a batch leaves the earlier skills installed and tracked.

### 7.3 Source records and `update`

Each install writes `<layer>/skills/.sources/<name>.json`, outside the skill's own directory, so it never moves the skill's stamp or content hash and is never readable through a read root:

```json
{ "version": 1, "source": "anthropics/skills", "url": "https://github.com/anthropics/skills",
  "ref": null, "path": "skills/pdf", "commit": "<sha>", "content_hash": "<sha256>", "installed_at": 1792000000 }
```

A local install records `"local": "<canonical dir>"` instead of `url`/`ref`/`commit`.

`update` trusts no record field. A recorded `path` must pass the same rules as `#<path>` and must resolve, through any symlink, inside the clone. A project-layer record's `local` must resolve inside the project: project records are files a repository can carry, and one naming `~/anything` would copy that directory into the project. A failed check fails that skill's update.

`update` re-fetches each named skill (`--all`: every skill in the layer with a record), clones each `(url, ref)` once, finds the skill at its recorded `path` (falling back to a search by name if the repo moved it) and replaces it. A skill installed at a pinned `--ref` stays on that ref; moving it is a new `install --ref`. Results per skill: `updated <old>→<new>`, `up to date`, `no source record`, or `locally modified`. The last is a skill whose current content hash differs from its record's, meaning someone edited the installed copy; `update` refuses it without `--force`.

## 8. Control plane

- `GET /api/skills` (global + `?ws_id=` project): name, layer, description, stamp, error.
- `AgentDto` (`crates/rupu-cp/src/api/agents.rs:363`) gains `skills` and `preload_skills`. `parse_agent_for_write` refuses malformed entries but not unknown names: an agent may be saved before its skill is installed, and the unresolved name shows in the agent's `load_error` as it does in `rupu agent validate`.
- The web Library gets a Skills tab and the agent editor a skill picker. Transcript views render `skill_index` and `skill_load`. The session composer gets the operator-invoke action.

The web work is GUI: it stops for matt's visual check before merge.

## 9. Remote and placed units

The coordinator sends a placed unit its agent's name, never its content (`crates/rupu-orchestrator/src/runner.rs:124`). The host runs `rupu run <agent>` and loads the agent itself, from a synced project `.rupu/` (workspace sync packs `.rupu/` unless gitignored, `crates/rupu-workspace/src/workspace_sync.rs:114`) or its own global layer. Skills resolve the same way:

- A project skill rides workspace sync with the agents next to it, with two exceptions. `pack_tar` keeps only regular files (`crates/rupu-workspace/src/workspace_sync.rs:128`), so symlinks inside a skill don't arrive. A repo that gitignores `.rupu/` sends no project skills at all. Either way the unit fails at resolution as below.
- A global skill must be installed on the host. If a granted skill is missing there, the unit is recorded unstarted through `AssembleError::record_unstarted`, and the message names the host and the `rupu skill install` fix. The unit fails loudly rather than running without its instructions.
- `skills: ["*"]` resolves to what the host has. The unit's `skill_index` event records which set that was.

A step's own skills (§2.2) are not in the agent file, so they must travel with the dispatch. `UnitDispatch` (`crates/rupu-orchestrator/src/runner.rs:124`) gains `skills` and `preload_skills`. `RunArgv::Agent` gains a repeatable `--skill` and `--preload-skill`, held by `argv_roundtrip`. Peers advertise an `agent.skills` capability, and a connector refuses a unit with step skills when its peer did not advertise it. This is the contract `findings_profile` and `engagement_profiles` already follow: never a silent drop.

An older host whose rupu predates this spec cannot parse a synced agent file with `skills:`, because agent frontmatter is `deny_unknown_fields`. The unit fails with that parse error. This is true of every frontmatter key added before, and updating the host is the fix.

Shipping skill content with a dispatch is a later option if this proves painful. It would follow the customer layer's remote-shipping design.

## 10. Errors

| Situation | Result |
|---|---|
| `skills:` or `preloadSkills:` names an unknown or invalid skill, or an unqualified name in both layers | agent load error with did-you-mean (`rupu agent validate` catches it); the assembler refuses the same at launch |
| step `skills:` entry malformed | workflow parse error |
| step `skills:` names a skill not installed | step recorded unstarted, like a missing agent |
| skill missing on a remote host | unit recorded unstarted, message names the host and the fix |
| step skills on a unit whose peer lacks `agent.skills` | connector refuses the unit |
| granted skill deleted mid-run | `skills.load` fails with that reason; re-attach lists it as missing |
| `skills.load` of an ungranted name | tool error listing the granted names |
| skill changed since the index | loads current version, `skill_changed` notice |
| skill names tools the run lacks | loads, `skill_tools_missing` notice |
| granted, `disable-model-invocation`, not preloaded | runs, `skill_not_indexed` notice |
| `session send --skill` unknown name | send refused, did-you-mean |
| re-attach over the cap | partial re-attach, `skill_reattach_truncated` notice |

## 11. Testing

- **Format conformance (`rupu-skills`).** Fixtures for every valid and invalid example in the agentskills.io specification, plus a real published skill with scripts, parse and validate as the spec says. Unknown frontmatter keys are accepted.
- **Discovery.** Both layers kept; an invalid skill lists with its error; an escaping symlink invalidates; a symlinked skill directory is fine globally and invalid in the project layer; `project_root_for` = `$HOME` yields no project layer.
- **Fingerprints.** The stamp and content hash are stable across runs and change on any edit, add, rename or `chmod +x`; building the index reads no file contents.
- **Grant (`rupu-runtime` `tests/it/assembly.rs`).** Absent means none and no `skills.load`; `*` means all valid global skills and no project skill; `project:*` and exact names reach project skills; a name in both layers fails until qualified; step skills add to the agent's; a sub-agent does not inherit; `origin_matrix` snapshot updated.
- **Prompt stability.** Two assemblies of the same agent and skill set render byte-identical system prompts.
- **Catalog lockstep (`rupu-tools` `tests/it/descriptors.rs`).** `skills.load` registered, has a body when `Service::Skills` is provided, is not action-eligible.
- **Read roots.** `read_file`, `glob` and `grep` reach a granted global skill's files; `write_file` to the same path is refused; a non-granted skill's path is refused; reading a skill file writes no coverage touch.
- **End to end with `MockProvider`.** A scripted model loads a skill, reads a reference and runs its script through `bash`; the transcript holds `skill_index`, `skill_load` and the tool calls. Under readonly the script is denied and audited.
- **Compaction.** Force a compaction after a load; the rebuilt message carries the body; a second compaction carries it once, not twice; replay reproduces both; over-cap and deleted skills are listed and noticed.
- **Sessions (`rupu-cli` `tests/serial/`).** The system prompt is byte-identical across turns even after a preloaded skill is edited; `send --skill` injects, records and grants; a model load after a disk change gives `skill_changed`; a compaction in turn 3 re-attaches a skill loaded in turn 1.
- **Remote.** A placed unit whose host lacks a granted global skill records an unstarted run with the host-named message; step skills reach the host through `--skill`; a peer without `agent.skills` gets a refusal; `argv_roundtrip` covers the new flags.

## 12. Plans

1. **Model and CLI.** `rupu-skills` (model, validation, discovery, fingerprints, renderers, sources, source records) and `rupu skill list|show|validate|install|update`.
2. **Runs.** Agent and step keys, load-time validation, `ResolvedSkills` in the assembler, the index and preload rendering, `Service::Skills` and `skills.load`, read roots, transcript events and notices, compaction re-attach and replay, and the placed-unit `--skill` flags and `agent.skills` capability. Covers every origin but sessions.
3. **Sessions.** Record fields, `session start --skill`, `session send --skill`.
4. **CP.** API fields and endpoints, then the web Library tab, agent picker, transcript rendering and session composer action (stops for the visual check).
