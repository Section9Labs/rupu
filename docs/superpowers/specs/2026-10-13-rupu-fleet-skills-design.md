# Fleet skills: `Section9Labs/rupu-skills` and pinned skill bundles

Date: 2026-10-13. Status: design approved in conversation, awaiting spec review.

## 1. Goal

The stock security fleet's agents are thin: `recon` runs one `nmap -sn` sweep and one
`nmap -sV --top-ports 200`, and nothing more. This spec gives the fleet real methodology
through Agent Skills, kept in a central public repository, `Section9Labs/rupu-skills`, and
installed by `rupu fleet install` at a pinned commit.

Success looks like this:

- `rupu fleet install` on a connected machine installs the fleet's agents, workflows and
  skills, and `rupu skill update` knows where every fleet skill came from.
- On an air-gapped engagement box, `rupu fleet install --skills-from <checkout>` does the same
  from a local git checkout that contains the pinned commit.
- A `network-assessment` run discovers hosts and services with a scope-enforcing,
  rate-capped methodology, splits what it found into work units, and fans the service
  analysis out over them.
- Every analyst records findings in the full profile, with evidence, root cause, chain, PoC
  and verification.
- The skills are useful to clients other than rupu.

## 2. Decisions

| # | Decision | Source |
|---|---|---|
| D1 | Skills live in the public repo `Section9Labs/rupu-skills`, not in the fleet templates. | matt, 2026-10-13 |
| D2 | Each built-in profile's `[bundle]` names its skills with a source and a pin. | matt |
| D3 | The pin is a commit. A bundle entry carries `tag` for display and `commit` for fetching; the source record stores the commit as its ref. A moved or deleted tag cannot change what runs. | matt (option A) |
| D4 | `rupu fleet install` requires the skills. A failed fetch exits non-zero after writing agents and workflows. `--skills-from <dir>` installs from a local git checkout that contains the pinned commit. | matt (option A) |
| D5 | rupu-skills has its own semver (`v1.0.0`, …), independent of rupu, because other people may use it. Each skill declares `compatibility`; `COMPATIBILITY.md` lists tested clients. | matt (option B) |
| D6 | This arc ships the cross-cutting skills and the network profile (Plan 1), then web and API (Plan 2). Binary, cloud and the other profiles are a later arc. | matt (option A) |
| D7 | Skills are client-agnostic. rupu specifics live in a skill's `references/rupu.md`; a skill that only makes sense in rupu is named `rupu-*`. | follows from D5 |
| D8 | Skill content is authored with Claude Mythos (`claude-mythos-5`, `auth: api-key`, verified served on 2026-10-13) from public methodology, and every line is reviewed before commit. | task brief |
| D9 | Scripts are verified for real on matt's Kali VMs: kali6 (192.168.64.6) scans a docker-compose lab on kali-b (192.168.64.8). | matt, 2026-10-13 |

## 3. Non-goals

- Shipping skill content to remote hosts for placed units. That is
  `docs/superpowers/specs/2026-10-13-rupu-skills-on-placed-units-design.md`.
- Skills for the binary, cloud, container, iac, sca, secrets, code, firmware, mobile,
  redteam and threat-model profiles (later arc; their analyst agents still get
  `rupu-finding-report` now).
- Changing how `rupu skill install` / `update` work for skills installed by hand.
- Anything adapted from private assessments. Both repositories are public; content comes
  from public tool documentation and public methodology, or from Apache-2.0 material vendored
  with attribution (§6.4).

## 4. Research summary (2026-10-13)

No published skill set is complete, permissively licensed and worth vendoring whole. Most
published offensive-security skills are cheat sheets: none has a script that enforces a
target allowlist, none checks for root or CAP_NET_RAW and falls back, and splitting results
into work units and writing reports against a findings schema are barely covered.

What is worth using, all Apache-2.0:

- `TerminalSkills/skills` `skills/kali-linux`: spec-valid; vendored unchanged.
- `x-glacier/kali-pentest` per-tool `references/` (nmap, masscan, naabu, httpx, nuclei,
  testssl.sh, enum4linux-ng, netexec): selected sections adapted with attribution. Its
  SKILL.md frontmatter does not validate and is not used; the repo LICENSE is Apache-2.0
  while the frontmatter says MIT, so it is treated as Apache-2.0 and attributed.
- `BitterSecurity/Decepticon` `active-recon`: phase and rate-budget sections adapted.
- `crazyMarky/pentest-skills` `recon-port-scan` parse/merge scripts: ideas only; our parsers
  are written fresh because they must sit behind the scope gate.

Avoided: `blacklanternsecurity/red-run` (GPL-3.0, Claude-Code-specific), `trailofbits/skills`
and `AgentSecOps/SecOpsAgentKit` docs (CC-BY-SA share-alike), repositories with no licence
(reference only), and `mukul975/Anthropic-Cybersecurity-Skills` (frontmatter does not validate,
generic content).

The agentskills.io reference validator (`skills-ref`) rejects any frontmatter key outside
`name`, `description`, `license`, `compatibility`, `metadata` and `allowed-tools`. Our skills
use only those keys, with string-valued `metadata`.

## 5. rupu side

### 5.1 Bundle format

`rupu_coverage::profile::Bundle` gains `skills: Vec<BundleSkill>`:

```rust
#[serde(deny_unknown_fields)]
pub struct BundleSkill {
    pub name: String,    // the skill's directory name
    pub source: String,  // what `rupu skill install` accepts: `owner/repo` or an https git URL
    pub tag: String,     // display only; never fetched
    pub commit: String,  // full 40-hex SHA; what is fetched
}
```

```toml
[bundle]
agents = ["assessment-lead", "recon", "service-analyst", "exploit-verifier"]
workflows = ["network-assessment"]
skills = [
  { name = "network-recon",       source = "Section9Labs/rupu-skills", tag = "v1.0.0", commit = "<40-hex>" },
  { name = "service-enum",        source = "Section9Labs/rupu-skills", tag = "v1.0.0", commit = "<40-hex>" },
  { name = "work-units",          source = "Section9Labs/rupu-skills", tag = "v1.0.0", commit = "<40-hex>" },
  { name = "rupu-finding-report", source = "Section9Labs/rupu-skills", tag = "v1.0.0", commit = "<40-hex>" },
]
```

Parsing a profile rejects a `commit` that is not exactly 40 lowercase hex characters, so a
short SHA or a branch name is a loud error. Like the rest of `[bundle]`, a bundled skill is
never a grant; agents grant skills in their own `skills:` frontmatter. `rupu-coverage` stays
free of a `rupu-skills` dependency; skill-name validity is checked by the lockstep test.

### 5.2 `rupu fleet install`

After writing agents and workflows exactly as today, `install` installs the fleet's skills:
the union of every built-in profile's `bundle.skills`, deduplicated by name, into the global
skills directory, or the project's with `--project`.

- **One fetch per (source, commit)** through `rupu_skills::install_from`, with
  `Source::Git { url, git_ref: Some(<commit>), path: None }` and
  `Selection::Named(<the names pinned to that commit>)`. The fetched tree's commit must equal
  the pin; otherwise nothing from that tree is installed and the error names both commits.
- **The source record** is the normal one: `url`, `ref` = the SHA, `commit` = the SHA,
  `content_hash`. `rupu skill update` therefore re-fetches the same commit and reports a
  fleet skill up to date; new skill versions arrive by upgrading rupu (a new pin) and
  re-running `rupu fleet install`.
- **Per skill:**

  | On disk | Result | Line |
  |---|---|---|
  | not installed | install | `CREATED skills/<name>` |
  | installed from the same source and commit | nothing | `SKIPPED skills/<name> (pinned)` |
  | installed from the same source at another commit, content matching its record | reinstall at the pin | `UPDATED skills/<name> <old12> → <new12>` |
  | content no longer matches its record (edited locally) | keep | `KEPT skills/<name> (locally modified; --force replaces it)` |
  | no record, or a record naming another source | keep | `KEPT skills/<name> (installed from elsewhere; --force replaces it)` |

  `--force` replaces the kept ones, as it does for agents and workflows.
- **Failure.** A failed fetch or install does not stop the other skills. At the end, if any
  skill is missing, the command prints which, and the command to retry (with
  `--skills-from` for offline boxes), and exits 1. Agents and workflows already written stay.
- **`--skills-from <dir>`** is a second `rupu_skills::Fetch` implementation, `PinnedCheckout`.
  For each pinned (source, commit) it opens `<dir>` with git2 and requires the
  pinned commit to be in its object database (else it says to `git fetch --tags`). HEAD, the
  index and the working tree are ignored and left untouched: the commit's tree is exported into
  a temporary directory, so modified, untracked, ignored or `skip-worktree` files can never be
  installed. It returns `FetchedTree::with_guard(<export>, Some(<commit>), <tempdir>)`. Because the
  `Source` passed to `install_from` is still the git source, the record names the GitHub URL
  and the SHA, and `rupu skill update` works later from a connected machine. With one pinned
  commit across the fleet (the lockstep rule), one checkout serves every skill.
- `rupu fleet list` adds a `Skills:` section: name, tag, short commit.

The fetch/verify/install logic lives in a new `rupu_skills::pinned` module (it needs
`install_from`, records and `Fetch`); `cmd/fleet.rs` stays arg parsing plus printing.

### 5.3 Fleet agents and the network workflow

Plan 1 grants (agent frontmatter `skills:`):

| Agent | Skills |
|---|---|
| `recon` | `network-recon`, `work-units` |
| `service-analyst` | `service-enum`, `rupu-finding-report` |
| `exploit-verifier` | `rupu-finding-report` |
| `assessment-lead` | `work-units` |
| every other analyst (`api-tester`, `appsec-tester`, `binary-analyst`, `cloud-auditor`, `code-auditor`, `container-scanner`, `crawler`, `firmware-analyst`, `iac-reviewer`, `mobile-analyst`, `redteam-operator`, `sca-auditor`, `secret-scanner`, `threat-modeler`) | `rupu-finding-report` |

Each profile's `bundle.skills` is then exactly the union of its agents' grants:
`rupu-finding-report` everywhere, `work-units` in every profile bundling `recon` or
`assessment-lead` (network, pentest, web, api, redteam), and `network-recon` / `service-enum`
in network and pentest. The agents' prompt bodies are
rewritten to point at their skills (load it, follow its phases) instead of carrying inline
nmap lines.

`network-assessment.yaml`:

1. `recon` runs `network-recon`, then `work-units` to write `reports/network-units.json` (a
   JSON array, one object per unit).
2. `analyze` becomes `for_each: "{{ read_file('reports/network-units.json') }}"` with a
   `max_parallel`, each unit handed to `service-analyst`.
3. `verify` runs once over the collected results (`steps.analyze.results`).

### 5.4 Tests (offline; `crates/rupu-cli/tests/it/` and `crates/rupu-skills/tests/it/`)

- `fleet_bundle_coverage.rs`, extended:
  - every bundled skill name is a valid `SkillName`, and every pin is a 40-hex commit;
  - all built-in profiles agree on the pin for a given skill name;
  - for every profile P and every agent A in `P.bundle.agents`, A's `skills:` and
    `preloadSkills:` are a subset of `P.bundle.skills`' names;
  - every bundled skill is granted by at least one fleet agent (no orphans).
- `rupu-skills` `pinned` tests, with a fixture `Fetch` that serves a generated tree holding one
  valid skill per requested name and reports a configurable commit:
  - a first install writes every skill and a record with `url`, `ref` = `commit` = the pin;
  - a second install skips them all;
  - a new pin updates an unmodified skill and keeps a locally modified one, and `force`
    replaces it;
  - a skill installed from another source is kept;
  - a fetched commit that differs from the pin installs nothing;
  - one failing tree does not stop another source's skills, and the report lists the missing.
- `PinnedCheckout` against a temporary git repository built with git2: a checkout whose HEAD is the pin, or a later commit, serves
  the pin's content; a pin missing from the object database is refused; modified, untracked,
  ignored and `skip-worktree` files in the working tree never appear in the export.
- No offline end-to-end test of the `rupu fleet install` binary: the built-in pins name real
  commits, and a test-only pin override in production code is not worth it. `cmd/fleet.rs`
  stays thin (it calls `rupu_skills::pinned` and prints), and the networked pin check (§5.5)
  runs the real binary before every release.

### 5.5 The networked pin check

`scripts/check-skill-pins.sh` builds nothing: it takes a `rupu` binary, runs
`rupu fleet install` into a temporary `RUPU_HOME`, then `rupu skill validate --all`, and fails
on any error. `release-beta.yml` runs it with the release candidate binary, before tagging, so a
pin that does not resolve or a skill that does not validate blocks the release. It is not a
cargo test, so cargo tests stay offline.

### 5.6 Docs

`docs/skills.md` (fleet skills, pins, `--skills-from`, updating), the `fleet install` section
of `docs/cli-reference.md`, the `[bundle]` sections of `docs/engagement-profiles.md` and
`docs/using-rupu.md`, and the CLAUDE.md `rupu-cli`, `rupu-skills` and `rupu-coverage` entries.

## 6. The `Section9Labs/rupu-skills` repository

### 6.1 Layout

```
rupu-skills/
  README.md           what is here; installing with rupu and other clients; authorized use only
  LICENSE             Apache-2.0
  NOTICE              attribution for vendored and adapted material
  CHANGELOG.md
  COMPATIBILITY.md    skills version → tested clients (rupu versions)
  skills/
    kali-linux/                 vendored unchanged + its LICENSE + UPSTREAM (repo, commit)
    network-recon/              SKILL.md references/ scripts/
    service-enum/               SKILL.md references/ scripts/
    work-units/                 SKILL.md references/ scripts/
    rupu-finding-report/        SKILL.md references/
  lib/                the one copy of the shared code: scope.py, preflight.py
  tests/              script tests + fixtures (sample nmap XML, naabu, httpx, OpenAPI, SARIF, prowler)
  Makefile            sync | check | test
  .github/workflows/ci.yml
```

### 6.2 Skill format

Frontmatter uses only `name`, `description`, `license`, `compatibility` and `metadata` (string
values: `version`, and `source` for adapted material). `name` matches the directory. SKILL.md
stays under 500 lines; depth goes in `references/`.

### 6.3 Scripts

- Python 3, standard library only, plus a thin bash entry point where useful. No pip install
  is needed to run a skill.
- `lib/scope.py` and `lib/preflight.py` are copied into each skill's `scripts/_lib/` by
  `make sync`; `make check` (and CI) fails when a copy differs. An installed skill is
  self-contained.
- **Scope gate** (`scope.py`), called by every script before any target is touched:
  - The scope file has one entry per line: an IP, a CIDR, a hostname, or a `*.domain`
    wildcard. `!` marks a deny entry; deny always wins. `#` starts a comment.
  - A hostname target is resolved, and every address it resolves to must also be in scope.
  - An out-of-scope target is refused with the reason; a run left with no in-scope target
    exits non-zero.
  - Rates have a default (masscan / naabu 1000 pps) and a hard ceiling (10000 pps). A higher
    `--rate` needs both an explicit flag and a `max-rate` line in the scope file.
- **Preflight** (`preflight.py`):
  - Detects missing tools and prints how to install them (apt on Kali/Debian; `go install` /
    `pipx` otherwise). It runs `apt-get install` only with `--install`, and only as root or
    with passwordless sudo.
  - Checks for root or CAP_NET_RAW (`/proc/self/status` CapEff, `getcap` on the binary). Without
    raw sockets it selects `nmap -sT` and naabu's connect scan, and says so.

### 6.4 Licensing and attribution

The repo is Apache-2.0. `skills/kali-linux` keeps its upstream LICENSE and an `UPSTREAM` file
with the source repository and commit. Adapted sections carry a line naming their source, and
`NOTICE` lists every upstream (TerminalSkills/skills, x-glacier/kali-pentest,
BitterSecurity/Decepticon).

### 6.5 CI and releases

CI on every PR and on `main`: shellcheck, ruff, `make check`, the script tests (including
refusal of out-of-scope targets and of over-ceiling rates), the agentskills.io reference
validator on every skill, and `rupu skill validate` with the latest rupu release downloaded
from `Section9Labs/rupu` releases. `main` is protected and CI is required.

Releases are semver tags cut only on `main` commits, so the full clone the fetcher makes always
reaches them. A release updates `CHANGELOG.md` and `COMPATIBILITY.md`; a rupu PR then moves
the pin in every built-in profile at once.

Creating the repository on GitHub is an outward action and waits for matt's explicit go-ahead.

## 7. Skill content (Plan 1)

- **`network-recon`.** SKILL.md is the methodology; one script per phase:
  1. `scope.py check`: validate the scope file and the targets.
  2. `preflight.py`: tools, raw-socket capability, fallbacks.
  3. `discover.py`: host discovery with masscan or naabu at the capped rate, falling back to
     `nmap -sn`; scope only.
  4. `ports.py`: top 1000 TCP; then all TCP on live hosts; then a curated UDP list (53, 67, 69,
     123, 137, 161, 500, 623, 1900, 5353, …).
  5. `services.py`: `nmap -sV` with `default,safe` NSE only, `-oA` output.
  6. `parse.py`: normalized JSON records (host, port, protocol, service, product, version, tls).

  References: timing and rates; NSE category choice (never `intrusive`, `dos` or `brute` in
  recon); IPv6; the service → follow-on tool handoff table; `rupu.md` (`assets.mark` and the
  depth ladder).
- **`service-enum`.** A reference and a scope-gated wrapper per service class: http (httpx, then
  nuclei with safe templates and a rate limit), tls (testssl.sh, sslscan), smb (enum4linux-ng,
  netexec with null or guest sessions only), ldap (anonymous bind), snmp (onesixtyone,
  snmpwalk), ssh / ftp / rdp / databases (version and auth-method checks). It stops at verified
  exposures; exploitation is the verifier's.
- **`work-units`.** `split.py <kind> <input> --by <key> [--max-per-unit N]` writes a JSON array
  of `{id, title, targets, hints}` objects, what `for_each:` reads and what a lead hands to
  `dispatch`. Kinds: normalized network JSON / nmap XML / naabu (by host or service class),
  httpx (by origin), OpenAPI (by resource), SARIF (by file or rule), prowler (by account,
  region and service). References: unit sizing per profile; `rupu.md` (the `read_file` +
  `for_each` pattern, and `dispatch`).
- **`rupu-finding-report`.** How to write a full-profile `findings.report`: each field of
  `rupu findings schema`, the evidence block kinds and when each fits, root cause, attack chain,
  a reproducible PoC, and verification status. One worked example on an invented host.
- **`kali-linux`.** Vendored unchanged, for the repository's other users. No fleet agent grants
  it and no profile bundles it: `preflight.py` covers what the fleet needs.

Plan 2 adds `web-recon`, `web-testing` and `api-testing`, their `work-units` references, the web
and API agents' grants and the `web-assessment` / `api-assessment` workflow changes.

## 8. Authoring and verification

- **Authoring.** An authoring agent (`provider: anthropic`, `auth: api-key`,
  `model: claude-mythos-5`) runs from a scratch checkout; its file is committed to neither
  repository. Each skill gets a written brief: the outline in §7, the public sources it may use
  (tool documentation, the vendored Apache-2.0 material), the frontmatter rules, the scope-gate
  contract, and a hard rule to use public methodology only. Mythos drafts; every line is
  reviewed and fixed or rewritten before commit.
- **Lab verification.** kali6 runs the scripts against a docker-compose lab on kali-b (nginx with
  TLS, samba, openldap, snmpd). The scope file names kali-b only, and a run with a target
  outside it (kali6 itself, another 192.168.64.0/24 address) must be refused before any packet
  is sent. With and without raw-socket capability.

## 9. Order of work

Plan 1:

1. **rupu machinery PR**: `BundleSkill`, `rupu_skills::pinned`, `fleet install` / `list`,
   `--skills-from`, the tests in §5.4 (the lockstep tests pass with empty `bundle.skills`), the
   pin-check script and its `release-beta.yml` step.
2. **rupu-skills repository** (after matt approves its creation): layout, lib, CI, the five
   skills, lab verification, tag `v1.0.0`.
3. **rupu pin PR**: the `[bundle] skills` entries at `v1.0.0`'s commit, the agents' grants and
   rewritten bodies, the `network-assessment` change, docs.

Plan 2: web and API skills (rupu-skills `v1.1.0`), then the rupu pin and agent PR.
