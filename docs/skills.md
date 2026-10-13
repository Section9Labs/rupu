# Agent Skills

A skill is a directory of instructions, scripts and reference files that an
agent can load when a task calls for it. rupu reads the open
[Agent Skills](https://agentskills.io) format, so a skill written for another
harness installs and validates here unchanged.

This page covers the format rupu enforces, where skills live, and the
`rupu skill` commands. The design is in
[`superpowers/specs/2026-10-12-rupu-agent-skills-design.md`](superpowers/specs/2026-10-12-rupu-agent-skills-design.md).

## Layers

Skills live in two places:

| Layer | Directory |
|---|---|
| global | `~/.rupu/skills/<name>/` (or `$RUPU_HOME/skills/`) |
| project | `<repo>/.rupu/skills/<name>/` |

A project skill does not shadow a global one of the same name. Both are kept,
and the grant decides which one an agent gets. When the repo has no `.rupu/`
of its own, its project root resolves to `$HOME`, and `~/.rupu` is then the
global layer only, never both.

rupu does not read `.claude/skills`, `.agents/skills` or `.codex/skills`.
Scanning them would let a repo under assessment plant instructions and scripts
that rupu's agents then run. Copy a skill in on purpose with
`rupu skill install`.

## The `SKILL.md` format

A skill is a directory whose name is the skill's name and which contains
`SKILL.md`:

```
~/.rupu/skills/report-style/
├── SKILL.md
├── scripts/render.sh
└── references/STYLE.md
```

`SKILL.md` starts with YAML frontmatter. rupu enforces these rules:

| Field | Rule |
|---|---|
| `name` | Required. 1 to 64 chars, `a-z`, `0-9`, `-`, no leading, trailing or doubled `-`. Must equal the directory name. |
| `description` | Required, not empty. Over 1024 chars is a warning; the index the model sees cuts it at 1024 chars. |
| `license` | Optional, free text. |
| `compatibility` | Optional, not empty when present. Over 500 chars is a warning. Shown by `rupu skill show`. |
| `metadata` | Optional string-to-string map. Kept, not interpreted. A number such as `version: 1.0` is read as the string `"1.0"`. |
| `allowed-tools` | Optional. A space-separated string, or a YAML list. Informational only: it is never a tool grant. |
| `disable-model-invocation` | When `true` the skill is left out of the index the model sees. |
| anything else | Accepted and ignored, since other harnesses add their own keys. |

The 1024 and 500 limits come from the agentskills.io specification. Published
skills go over them (Anthropic's `claude-api` skill has a 1068-character
description), so rupu warns rather than rejecting the skill.

The body after the frontmatter is the instructions. Paths in it are relative
to the skill's directory. A UTF-8 BOM and CRLF line endings are accepted.

A skill is **invalid** when its frontmatter breaks a rule above, or when a
symlink inside it does not resolve or resolves outside the skill directory.
In the global layer the skill directory itself may be a symlink. In
the project layer it may not, so a repo cannot point a skill at an arbitrary
directory on the machine. Invalid skills are listed with their error.

### Versions

rupu fingerprints a skill two ways. The **stamp** hashes each file's path,
size, modification time and mode, and costs one `stat` per file. The
**content hash** reads every byte and is computed only when a skill is loaded.
Both skip `.git` and `.DS_Store` at any depth, and both count a file's mode
only as executable or not. Because the stamp reads no bytes, it can miss a
same-size edit whose modification time was put back (for example by `cp -p`
or `touch -r`).

## Installing

```sh
rupu skill install <source> [--ref <r>] [--skill <name>]... [--all] [--list] [--project] [--force]
```

The source is whatever you would copy from a README or a browser:

| Source | Resolves to |
|---|---|
| an existing path (`./x`, `/abs/x`, `~/x`) | that local directory |
| `owner/repo` | `https://github.com/owner/repo` |
| `https://github.com/o/r/tree/<ref>/<path>` (or `/blob/<ref>/<path>/SKILL.md`) | that repo, ref and path |
| any `https://` git URL, with an optional `#<path>` | that repo, and the folder at that path |

In a GitHub tree URL the ref is the first segment after `tree/`, so a branch
with `/` in its name needs `--ref` and `#<path>` instead. Only `https://` is
fetched: `ssh://`, `git@` and `file://` are refused, and a local checkout is
installed as a path. The fetch is a shallow clone of the default branch, or a
full clone checked out at `--ref`. No git hooks run and submodules are not
fetched.

A tree URL that already names a path takes no `#<path>`. A path must stay
inside the repository, symlinks included. Private repositories are not
supported yet, and a URL with credentials in it (`https://user:token@…`) is
refused so the token never lands in the source record: clone the repository
yourself and install from the directory.

rupu searches the source for every directory holding a `SKILL.md`, through
hidden folders such as `.claude/skills/`, up to 6 levels deep, skipping `.git`
and `node_modules`. Then:

- One skill found: it is installed.
- Several found: `--skill <name>` (repeatable) picks some, `--all` takes every
  valid one and reports the invalid ones it skipped, and with neither the
  command lists them and installs nothing.
- `--list` lists the skills in the source and installs nothing. An invalid
  skill's row reads `INVALID: <error>` in place of its description.

```sh
rupu skill install anthropics/skills --list
rupu skill install anthropics/skills --skill pdf
```

The other flags:

- `--ref <branch|tag|commit>` installs from that ref. It conflicts with a
  different ref in a tree URL.
- `--project` installs into the current project's `.rupu/skills/` instead of
  `~/.rupu/skills/`. Outside a rupu project it is refused.
- `--force` replaces an installed skill of the same name. Without it the
  install is refused.

Every chosen skill is validated before anything is written, and a skill
containing an absolute symlink is refused: an absolute target would stop
resolving to the copy. Each copy keeps
its file modes, so scripts stay executable, and is swapped in by rename, so a
failed install, `--force` included, never damages the installed skill.
If a crash interrupts a replace, the next install or update of that skill puts
the old copy back from `.replaced-<name>-<pid>` first. When an install of
several skills fails part way, the skills installed before the failure stay
installed, with their source records.

A skill's scripts need their own tools on the machine: a skill that runs
`python3 scripts/render.py` needs `python3` wherever the agent runs, including
any remote host a placed unit runs on.

## Updating

Each install writes a source record to `<layer>/skills/.sources/<name>.json`,
outside the skill's directory so it never changes the skill's fingerprints.
It holds what you typed, the resolved URL, ref and path (or the local
directory), the commit, and the content hash of what was installed.

```sh
rupu skill update report-style
rupu skill update --all [--project] [--force]
```

`update` re-fetches each skill from its record, cloning each URL and ref
once, and replaces the installed copy. It prints one line per skill:

| Result | Meaning |
|---|---|
| `updated <old> → <new>` | the source changed and the new copy is installed |
| `up to date` | the source matches what is installed |
| `no source record` | the skill was copied in by hand; reinstall it to track a source |
| `locally modified` | the installed copy no longer matches its record, so someone edited it; `--force` replaces the edits |

If the repository moved the skill, `update` finds it by name and the record
follows it. A recorded path that leaves the repository, directly or through a
symlink, fails the update. In the project layer a record's local directory
must be inside the project; one that points elsewhere fails, so a cloned
repository cannot copy a directory from elsewhere on your machine into itself.

A skill installed at a pinned `--ref` stays on that ref. Moving it is a new
`rupu skill install --ref`. `update` exits 1 when any skill was locally
modified, missing or failed to update.

There is no `remove`. Delete the skill's directory; an orphaned source record
is ignored.

## Listing, showing and validating

```sh
rupu skill list [--project]
rupu skill show report-style
rupu skill show project:report-style
rupu skill validate report-style
rupu skill validate --all
```

`list` shows every skill in both layers with its layer, status, source and
description, or its error when it is invalid. It supports
`--format json|csv`.

`show` prints the frontmatter, the source record, the stamp and content hash,
and the skill's files. A bare name shows every layer that has it; prefix
`global:` or `project:` to pick one.

`validate` checks skills against the rules above and exits 1 when any is
invalid. It prints a `warn` line for each warning, such as an over-limit
description; warnings alone exit 0. `show` prints them too.

## Granting skills to agents

Installing a skill makes it available; it does not give it to any agent. An
agent names the skills it gets in its frontmatter:

```yaml
skills: [pdf, code-review]     # the model may load these
preloadSkills: [house-style]   # always in the system prompt
```

`*` grants every valid global skill and never a project one; a project skill
needs its exact name or `project:*`. A bare name installed in both layers is an
error until it is written `global:<name>` or `project:<name>`. The full grammar
is in [agent-format.md](agent-format.md#skills-and-preloadskills). An unknown
or invalid skill fails the agent's load, and `rupu agent validate` catches it.

## Skills on workflow steps

A workflow step can give skills to the agents it launches, on top of each
agent's own:

```yaml
steps:
  - id: review
    agent: reviewer
    prompt: Review the PR.
    skills: [pdf, "project:*"]      # the model may load these
    preload_skills: [house-style]   # always in the system prompt
```

The entries use the same grammar as an agent's `skills:` (see
[Granting skills to agents](#granting-skills-to-agents)). A step's skills
only add: they never remove one the agent declares. They reach every agent
the step launches: its own agent, each `for_each` unit, the members of a
`parallel:` block, and a panel's panelists and its gate's `fix_with` fixer.
A sub-step has no `skills:` of its own; it takes its parent step's.

The keys are allowed only on a step that runs an agent. A gate, `action:`,
`run:`, `branch:`, `split:` or `join:` step that names them fails to parse.
Parsing checks each entry's syntax only. Whether the skill is installed is
checked when the step launches, so a missing skill fails that step before
its agent runs, the way a missing agent does. The run's `skill_index`
records step skills with reason `launch`, the same as `rupu run --skill`.

A placed step (`host:` or `distribute:`) sends its skills to each unit as
`rupu run --skill` / `--preload-skill`, and the unit resolves them against
the host's own layers. The host must run a rupu that advertises
`agent.skills`. An HTTP, tunnel or bucket host that doesn't is refused before
anything runs. Over SSH, the launch command itself checks the remote's
`rupu __features` and refuses when `agent.skills` is missing. A step without
skills launches on any host.

## How an agent uses its skills

Every run the agent starts (`rupu run`, a workflow step, an agentiflow lead or
unit) gets its skills the same way:

- **Index or preload.** A skill in `skills:` is listed under `## Skills` at the
  end of the system prompt: its name and description, about a line each. A
  skill in `preloadSkills:` has its whole body in the system prompt instead.
  The section is rendered the same way for the same skills, so the prompt
  cache holds across runs.
- **`skills.load`.** When a task matches a listed skill, the model calls
  `skills.load` with its name and gets its instructions, base directory and
  file list. The skill is read from disk at that moment: one edited since the
  run started loads at its new version, with a `skill_changed` notice. The
  model can load only skills the agent was granted.
- **Reading and running its files.** `read_file`, `grep`, `glob` and
  `ast_grep` reach a granted skill's directory, even outside the workspace. A
  skill's files are read-only to the model: `write_file` and `edit_file` refuse
  them. Its scripts run through `bash`, under the run's permission mode
  (`readonly` denies them, `ask` prompts) and `[bash]` settings. A skill never
  adds a tool or raises the permission mode.
- **Compaction.** Compaction summarises older turns, which would drop a skill's
  instructions. The runner keeps the skills the model loaded and puts them back
  at their current version, in the rebuilt first message, after every
  compaction. They are capped at a quarter of the model's input limit, most
  recently loaded first; a skill that doesn't fit is named, for the model to
  load again. A skill uninstalled mid-run is named as missing.
- **Continuing a run.** `rupu run --continue` and a resumed workflow step start
  with the skills the interrupted run had loaded, and re-attach them at their
  next compaction. An agentiflow lead carries them from round to round.

`rupu run --skill <name>` and `--preload-skill <name>` (repeat the flag for
several) give one run skills on top of its agent's; its `skill_index` records
them with reason `launch`. `rupu run --continue` without either flag keeps the
earlier run's launch skills; any skill flag replaces them all. Passing a flag
is the only way to change them: a continued run can't drop them entirely.

A child started with `dispatch` (any kind, or a legacy name like
`dispatch_agent`) gets its own agent's skills, not its parent's. Sessions
are covered in [Skills in sessions](#skills-in-sessions).

The transcript records what the run had and used. `skill_index` lists the
granted skills at run start, and a `skill_load` is written for every load, with
`via` set to `model`, `preload`, `reattach` or `continuation`. See
[transcript-schema.md](transcript-schema.md#skill_index). Notices name what
needs attention: `skill_not_indexed`, `skill_changed`, `skill_missing`,
`skill_tools_missing` and `skill_reattach_truncated`.

## Skills in sessions

A session fixes its skills when it starts. `rupu session start` resolves the
agent's `skills:` and `preloadSkills:` once, against the global layer and the
session's project layer, and stores the set and the rendered skills section on
the session. Every turn sends that same section, so the system prompt is
byte-identical from turn to turn and the prompt cache holds. A preloaded
skill's body is the one it had at start; a session that wants its newer
version is a new session. A skill installed after the session started is not
offered, even under `*`. A skill the model loads is read at its current
version, and one edited since the session started loads with a
`skill_changed` notice. A skill uninstalled mid-session is left out of each
turn with a `skill_missing` notice, and the session keeps running.

`rupu session start --skill <name>` and `--preload-skill <name>` (repeat the
flag for several) add skills to the session on top of its agent's. Its
`skill_index` records them with reason `session`.

`rupu session send --skill <name> <id> [prompt]` (repeatable) invokes a skill
in that turn. It takes any valid skill installed in the session's global or
project layer, granted or not, including one marked
`disable-model-invocation`, which exists for exactly this. The `global:` and
`project:` prefixes work as in a grant. A bare name installed in both layers,
a wildcard, an unknown name (with a did-you-mean) and a name the session holds
from the other layer are refused before anything is sent. The prompt is
optional when a skill is given. The skill's body goes ahead of the prompt in
the turn's message:

```
<skill name="pdf" base="/Users/me/.rupu/skills/pdf" version="3f2a9c0d41b7">
...the skill's instructions...
</skill>

summarise x.pdf
```

`version` is the first 12 hex digits of the skill's content hash. The turn's
transcript records a `skill_load` with `via: operator`. From then on the skill
is part of the session with reason `operator`: the model may load it again
with `skills.load` and read its files. It is never added to the index, so the
system prompt does not change.

Sends can queue behind a running turn, so a turn checks its skills again when
it runs. A skill removed since it was sent gets one `skill_missing` notice in
that turn and is not added to the session. A turn fails, with a transcript
that says why, when an earlier queued turn gave the session the same name
from the other layer, or when it has no prompt and none of its skills can be
given.

The skills the model loaded survive turns and compaction. The session keeps
its loaded set from turn to turn, and every compaction puts those skills back
at their current version: the runner's own compaction during a turn,
`rupu session compact`, and the compaction a running session's worker does on
request. A loaded skill uninstalled for a while is named as missing and stays
in the set, so it comes back once it is installed again.

A session record that carries skills is written as version 2; one without
skills stays version 1. A rupu older than session skills doesn't check the
version, so it continues a version 2 session but drops its skills the next
time it writes the record: the session runs on without them. A rupu that
knows session skills refuses a record from a newer rupu ("this session was
written by a newer rupu; upgrade rupu to continue it") rather than write it
back without the fields it doesn't know.
