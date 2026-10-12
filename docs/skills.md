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
| `description` | Required. 1 to 1024 chars. |
| `license` | Optional, free text. |
| `compatibility` | Optional, 1 to 500 chars. Shown by `rupu skill show`. |
| `metadata` | Optional string-to-string map. Kept, not interpreted. A number such as `version: 1.0` is read as the string `"1.0"`. |
| `allowed-tools` | Optional. A space-separated string, or a YAML list. Informational only: it is never a tool grant. |
| `disable-model-invocation` | When `true` the skill is left out of the index the model sees. |
| anything else | Accepted and ignored, since other harnesses add their own keys. |

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
- `--list` lists the skills in the source and installs nothing.

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
invalid.

## Granting skills to agents

Installing a skill makes it available; it does not give it to any agent.
Granting skills arrives with the `skills:` agent frontmatter key, described in
the [design spec](superpowers/specs/2026-10-12-rupu-agent-skills-design.md#2-granting-skills).
