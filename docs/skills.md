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

## Fleet skills

The [stock fleet](engagement-profiles.md#the-stock-fleet) is more than agents and
workflows: the built-in engagement profiles' `[bundle]` also names skills, and
each is pinned to a commit of `Section9Labs/rupu-skills`. `rupu fleet install`
installs them into the same layer as the fleet's agents (global by default,
`--project` for the project) and writes the usual source record for each, with
the pinned commit as its `ref`. `rupu fleet list` shows the skills and their pins.

```sh
rupu fleet install                       # agents, workflows and skills
rupu fleet install --force               # replace what is already there
rupu fleet install --skills-from ./rupu-skills   # offline: see below
```

It prints one line per skill:

| Result | Meaning |
|---|---|
| `CREATED` | the skill was not installed and is now, at its pin |
| `SKIPPED` | the installed copy is already at the pin |
| `UPDATED <old> → <new>` | the installed copy was an older pin of the same source and is now at the new one |
| `OVERWROTE` | `--force` replaced an existing copy |
| `KEPT` | a skill of that name is there that the install will not touch: either you edited it (`locally modified`), or it was installed from somewhere else (`installed from elsewhere`). `--force` replaces it |
| `FAILED` | the skill could not be fetched or installed; the reason follows |

`rupu skill update` keeps a fleet skill at its pin. A newer version of a fleet
skill arrives with a newer rupu: take the release, then run `rupu fleet install`
again.

**Offline boxes.** On a connected machine, clone the repository and check out the
pinned commit (`rupu fleet list` shows the first 12 characters of each pin, which `git checkout` accepts):

```sh
git clone https://github.com/Section9Labs/rupu-skills
git -C rupu-skills checkout <commit>
```

Carry the directory to the offline box and run
`rupu fleet install --skills-from rupu-skills`. The checkout must contain the
pinned commit. Its working tree is ignored, and the skill's content comes from
the commit itself, so untracked or edited files in the copy never reach an
install.

If any skill fails to install, the command still installs the others, then exits
1 and names the ones that are missing. An agent that grants a missing skill does
not launch until it is installed, so re-run `rupu fleet install` when the network
is back, or use `--skills-from`.

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

A placed step (`host:` or `distribute:`) sends its skills with the unit. See
[Skills on placed units](#skills-on-placed-units).

## Skills on placed units

A placed unit runs `rupu run <agent>` on another host. The coordinator ships
the skills the unit needs, and the host runs the unit with exactly those
copies. The host does not need them installed. Design:
[the placed-units spec](superpowers/specs/2026-10-13-rupu-skills-on-placed-units-design.md).

**What ships.** The coordinator resolves the unit's skills the way a local run
does: the `skills:` / `preloadSkills:` of its own copy of the agent, plus the
step's `skills:` / `preload_skills:`. `*` means the coordinator's global layer.
If a step skill is not installed on the coordinator, the unit fails before it
is dispatched, as a local run would. If the coordinator has no copy of the
agent, or its file does not load, only the step's skills ship. The second case
adds a warning naming the file.

**What the host does.** Each skill is packed as a tar and named by its content
hash. The coordinator asks the host which hashes it lacks and sends only those.
The host checks every tar before keeping it: no absolute or `..` paths, no
symlink that leaves the skill, no entry that is not a file, directory or
symlink, and the unpacked tree must hash to the name it was sent under. It
keeps the copy in `<RUPU_HOME>/cache/skills/<hash>/<name>/` (mode 0700). The
cache has no eviction.

**Overlay.** A shipped skill replaces the host's installed skill with the same
layer and name, so the coordinator's version wins. The host still loads the
agent itself and resolves its grants, now over its global layer, its staged
project layer and the shipped skills. The rules do not change: `*` is the
global layer only, and a bare name bound in both layers is ambiguous. A grant
nobody shipped still resolves from what the host has installed, and fails
closed if the host has nothing. The step's own `skills:` and
`preload_skills:` go to the host as the `layer:name` entries they resolved to
on the coordinator, so `*` or a bare name means the same skills there even
when the host's layers would read it differently. The unit's `skill_index`
marks shipped skills `(shipped)`.

**Limits.** A skill tree may be at most 32 MiB and one unit's bundle at most
128 MiB. The coordinator refuses a unit over either limit before dispatch and
names the skill and the limit.

**Transfers.** Within one coordinator process, a host that confirmed a hash is
not asked again, and the units of a fan-out bound for one host share one
transfer. Over SSH a transfer is two commands per host, `rupu __skills missing`
and `rupu __skills receive`, not one per skill. Over HTTP the peer serves
`POST /api/skills/bundle/missing` and `PUT /api/skills/bundle/:hash`.
`host: local` ships nothing.

**Tunnel nodes.** The coordinator sends `SkillsOffer` with the skills' refs
(layer, name, hash). The node answers `SkillsNeed` with the refs its cache
lacks, the coordinator streams each archive as base64 `SkillChunk`s, and the
node verifies and stores them and replies `SkillsStored`. The unit's `RunSpec`
then carries the manifest. A node accepts at most 4 live offers per connection,
256 refs per offer and offer ids of at most 128 bytes. An offer that gets no
frame for 120 seconds fails, and a connection has a byte budget for archives it
holds before they are stored. The first problem in an offer fails that offer
with an error and stores nothing. The connection stays up. The node cache is
never evicted.

**Bucket workers.** The coordinator puts each missing skill as
`skills/<hash>.tar` in the bucket and skips one that is already there. Before
a worker starts the job it downloads every skill its cache lacks and verifies
it. A missing or corrupt object fails the job, and the message names
`layer:name`. A manifest of more than 256 skills fails the job too. A
transient bucket read error leaves the job pending for the next tick. A skill
over the size limit is refused with a typed `TooLarge` error.
When a job fails, the worker writes `failure.txt` before the job's marker, and
the coordinator copies its text into the run record's `error_message`.

**Older hosts.** A host that does not advertise `agent.skill_bundle` is sent
the unit by name, as before, and the run gets a warning that names the host
and says the unit will use the host's installed copies. Over SSH and HTTP the coordinator
remembers the downgrade for the life of its process. Over SSH the launch
command itself checks the remote's `rupu __features`. A tunnel node
advertises it in `Hello.capabilities` and a bucket worker in its marker. For
tunnel and bucket the capability is re-checked on every unit and the downgrade
is not remembered, so a node or worker that is upgraded starts receiving skills
without a coordinator restart.

**Errors.** Every coordinator-side failure for a placed unit's skills (resolve,
pack, limit, transfer) and a skill missing on the host starts with
`host <id>: `.

**Known limits.** Only the placed unit's own skills ship: its coordinator copy
of the agent plus the step's. These still resolve from the host's installed
layers:

- children the unit launches on the host (`dispatch` of `kind: agent` or
  `workflow`, in-process sub-agents, a `rupu run --continue` of a `sub_...`
  run);
- a `rupu coverage` rerun;
- an agent whose own grants the coordinator did not ship.

On the host, `rupu run --skill-bundle <json>` (hidden) carries the manifest.
The run copies it into its run directory, so `rupu run --continue` finds the
same shipped skills.

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
back without the fields it doesn't know. `rupu session list` shows such a
session with status `newer rupu` and a warning, and `rupu session prune`
leaves it alone.

A queued turn that invokes skills is written as turn request version 2; any
other stays version 1, so an older worker still runs it. A worker that knows
`--skill` fails a turn queued by a newer rupu, with a transcript ("this turn
was queued by a newer rupu (turn request version <n>); restart the session
worker: rupu session stop <id>"), rather than run it without what it asked
for. A worker older than `--skill` can't refuse what it predates: it would run
the turn without its skills. `session send` restarts a worker whose rupu
binary is older than its own, judged by the binary's modification time, but
it keeps the worker when that can't be read. After upgrading rupu, run
`rupu session stop <id>` before using `--skill` on a running session.

## In the control plane

The control plane (`rupu cp serve`) shows installed skills and edits where
they are granted. Installing and updating skills stay CLI operations. The
routes are in [cp-api.md](cp-api.md#skills).

The Library's skills tab lists the global layer, and the project picker adds
that project's layer, like `rupu skill list`. Each row shows its layer,
status, source and description; an invalid skill shows its error. A layer
that can't be read (a project `.rupu/skills` that is a file, say) is left out
with a warning, and the other layer still lists. A skill's page shows its
description, compatibility, license, `allowed-tools` with the rupu tools they
map to (informational, never a grant), metadata, files, body, content hash and
install record. An install record that can't be read is shown as a warning
rather than hidden. A record is read only when it is a regular file of at
most 64 KiB, and never through a symlinked project `skills/`.

An agent's `skills:` and `preloadSkills:` are edited in the agent builder's
Skills card, with the installed names as suggestions. The control plane checks
an agent's skills the way `rupu agent validate` does: against the global
layer for a global agent and the global plus project layer for a project
agent, every granted skill must be installed, load, and bind once. An agent
that fails shows "This agent can't be launched" with the reason, and an
unknown name comes with the install hint. Saving such an agent is allowed, so
an agent can be written before its skills are installed; only a malformed
entry is refused.

A workflow step's `skills` and `preload_skills` are fields of the step form
for any step that runs an agent. The launcher's Skills section adds skills to
a run (`rupu run --skill` / `--preload-skill`) or a session
(`rupu session start --skill` / `--preload-skill`). A session's composer has
an "Invoke skill" control: a send with a skill and no prompt is allowed, as
`rupu session send --skill` allows it. A skill name the CLI refuses (unknown,
a wildcard, ambiguous) comes back as the CLI's message.

Session skills on a remote host need a rupu there that advertises
`session.skills`; an older one is refused before anything is sent, with an
"upgrade rupu there" message. Run skills on a remote host need
`agent.skills`, as a placed workflow step does. Shipping a placed unit's
skills is described in [Skills on placed units](#skills-on-placed-units).
