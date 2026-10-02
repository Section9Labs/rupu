# rupu engagement profiles + asset model — design

Date: 2026-09-30
Status: approved in brainstorming, pending spec review

## Problem

rupu assesses one thing: source code. The agent runtime, permission model,
transcript, orchestrator, executor, and control-plane surfaces are all
domain-agnostic — an engagement is already just a bundle of agent `.md` files,
a toolset, and a workflow. But the *finding and coverage layer* bakes "code"
into the core:

- A finding is implicitly *about a file*. The coverage ledger discovers and
  tracks *files* (`discover_targets`, `FileTouchEvent`, `file_views`).
- The finding report's taxonomy is CWE-only (`cwe: Vec<String>`), its
  completeness checklist is a fixed code-review list, and its evidence renders
  as code excerpts / diffs / ast_grep CSTs.

Two real jobs need more: a **binary reverse-engineering** engagement and a
**network security assessment**. Neither fits "a finding about a file." The
framework is agnostic; the *profiles* are not — and code is the only profile
we have.

## Decisions (from brainstorming)

1. **A finding is about an `asset`, and the asset has a `kind`.** Code
   (file/function) is one kind; binary blob, function@address, host, service
   are others. The kind drives everything currently hardcoded: locator,
   evidence shape, taxonomy, and what coverage counts.
2. **The core stays data-driven (option A′ — typed primitive vocabulary).**
   The core understands a small, fixed set of *primitives* — locator
   coordinates and evidence blocks — and ships renderers/validators/guardrails
   for those. A **profile is a data package** that composes primitives; adding
   a profile is authoring a data file, not a Rust PR. This is the same shape
   rupu already uses for agents (`.md`), workflows (YAML), and MCP tools
   (registry). The finding/coverage layer is the last corner that hardcodes its
   content; this finishes the hexagonal architecture on it. New code is only
   needed to add a new *primitive* — rare, and shared by every future profile.
3. **Assets nest into a graph; its roots are the engagement's top-level
   assets.** Assets nest (service → host; function → binary → firmware image).
   Coverage is the asset tree discovered under the roots plus each node's
   examination depth. There is no rules-of-engagement, scope matching, or tool
   gating — agents reach targets with bash and their own tooling; rupu records
   and validates the resulting *findings*, it does not run or gate traffic.
4. **Two orthogonal profile axes.** Keep `FindingProfile` (`full`/`summary`,
   report *verbosity*) exactly as-is — its wire name is used by workflow YAML,
   agent frontmatter, and `--findings-profile`. Add a new **engagement profile**
   axis (`code`/`binary`/`network`, the *asset family* + taxonomy + completeness
   + coverage + default bundle). An engagement may activate **several profiles at once** (a
   pentest = `network` + `web`), directly or via a **composite profile** that
   `includes` them; each asset kind is namespaced by its owning profile
   (`network:service`), so a finding routes to the right profile's validation by
   its asset's kind.
5. **Naming.** `asset` = the assessment subjects. The existing content-addressed
   `artifact` blob store keeps its name **for the pilot** (renaming it across
   `rupu-coverage` + CP endpoints + docs would bloat the pilot); "evidence
   store" is parked as a graduation-time cleanup.
6. **Pilot binary first; network as a fast-follow** on the same ABI. Binary is
   fully static/offline, so it exercises the asset graph and the new evidence
   primitives. Network then adds only `host`/`port`/`url` coordinates and
   `http_exchange`/`scan_output`/`pcap_ref` blocks — no new machinery.
7. **Profiles live in rupu; several ship built-in.** Built-in profiles are
   embedded in the binary; operators/projects override or add their own under
   `~/.rupu/profiles/` (global) and `.rupu/profiles/` (project), mirroring
   agent/workflow discovery. Selection and enforcement span every launch path
   (CLI, workflow, autoflow, session, CP launcher, triggers, MCP), fail-closed —
   see "Selection, resolution & enforcement".

## Design

### The primitive vocabulary (the ABI — the only code the core owns per-kind)

```rust
/// The typed primitives the core understands. Adding a *coordinate* is the
/// only reason to touch this enum — rare, shared by every profile. The whole
/// anticipated set is declared now (see the profile catalog below): an unused
/// variant costs nothing, and pre-declaring keeps every profile pure data from
/// the day it is authored.
pub enum Coordinate {
    // static / code / binary
    Path(String),
    LineRange { start: u32, end: u32 },
    Symbol(String),
    Commit(String),                            // VCS revision (secrets / git history)
    Sha256(String),
    Offset(u64),
    Address(u64),                              // virtual address
    // network
    Host(String),
    Port { number: u16, proto: Proto },
    Url(String),
    // web / appsec
    HttpRoute { method: String, path: String },
    Param(String),
    // cloud / saas / k8s
    ResourceId { scheme: String, id: String }, // e.g. arn:… , k8s:… , gcp:…
}
pub struct Locator(pub Vec<Coordinate>);
```

Staging: `path`/`line_range`/`symbol`/`sha256`/`offset`/`address` are the binary
pilot; `host`/`port`/`url` land with network; `http_route`/`param` with web;
`resource_id` with cloud. Each is inert until its profile ships.

### Asset + the asset graph

```rust
pub struct Asset {
    pub id: AssetId,                 // derived from kind + locator, stable
    pub kind: String,                // profile-namespaced kind id ("network:service", "web:route")
    pub parent: Option<AssetId>,     // the graph; roots == top-level assets
    pub locator: Locator,
    pub label: String,               // rendered from the kind's label template
    pub depth: Option<String>,       // profile-declared coverage depth state
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>, // opaque escape hatch
}
```

`attributes` is the deliberate escape hatch for profile-specific data the core
need not understand; it is *not* the primary channel — anything the core should
render, validate, or guard on is a first-class primitive, not an attribute.

Asset kinds are **namespaced by their owning profile** (`network:service`,
`web:route`). When several profiles are active at once, that namespace is how a
finding finds its validating profile — from the asset it is about — and why two
active profiles can never collide on a kind id. An engagement may have more than
one root (a pentest carries `network:host` roots and `web:site` roots).

### Evidence blocks

The report renderer already handles text/markdown, code slices, and diffs
(md/html/pdf). Evidence becomes an explicit list of typed blocks:

```rust
pub enum EvidenceBlock {
    Text(String),                                              // exists
    CodeSlice { locator: Locator, excerpt: String, lang: Option<String> }, // exists
    Diff(String),                                              // exists
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },    // generic structured (cheap)
    Image { artifact: ArtifactRef, caption: Option<String> },  // screenshots (blob store)
    Hexdump { base: u64, artifact: ArtifactRef, rendered: Option<String> }, // NEW (pilot)
    Disasm { arch: String, listing: Vec<DisasmLine> },         // NEW (pilot)
    Decompile { lang: String, listing: String },               // binary/firmware (later)
    HttpExchange { request: String, response: String },        // network / web
    ScanOutput { tool: String, output: String },               // ≈ Text+label
    PcapRef { artifact: ArtifactRef, summary: String },        // blob-ref
}
```

Only **two genuinely new renderers** in the pilot: `hexdump` and `disasm`.
`table`/`image` are generic and cheap; `scan_output`/`pcap_ref` reuse text
rendering + the existing blob store; `decompile` is a later (firmware) addition.
Backwards-compat: today's `excerpt`/`lang` on an `EvidenceClaim` map to a
`CodeSlice` block; `binary_va` maps to an `Address` coordinate. Old fields keep
deserializing.

### Classification

`cwe: Vec<String>` generalizes to a list that spans systems:

```rust
pub struct Classification {
    pub system: String,          // "CWE" | "CVE" | "CAPEC" | "ATT&CK"
    pub id: String,              // "CWE-306" | "CVE-2024-1234" | "T1190"
    pub vector: Option<String>,  // e.g. a CVSS vector
}
```

`cwe` and `cvss_v3` keep deserializing and fold into `Classification`. Lockstep
sites to update together: report model, embedded schema, validator, web DTO.

### Completeness as data

The fixed code-review checklist becomes profile-declared, evaluated by a
**deliberately tiny** predicate vocabulary — not a DSL:

```rust
pub struct CompletenessCheck { pub id: String, pub label: String,
                               pub required: bool, pub satisfied_when: Predicate }
pub enum Predicate {
    HasField(String),                 // a report field is present / non-sentinel
    HasBlockKind(String),             // >=1 evidence block of this kind
    HasClassificationSystem(String),  // >=1 classification from this system
    LocatorHasCoordinate(String),     // the finding's asset locator carries this coordinate
    MinSeverity(Severity),
    All(Vec<Predicate>), Any(Vec<Predicate>),
}
```

`All`/`Any` is the ceiling of acceptable composition. If a profile ever needs
more, that is a signal to add a *primitive*, not a scripting language. (`LocatorHasCoordinate` was itself surfaced this
way — network/firmware completeness needs "affected asset pinned to host + port".)

### Coverage as an asset tree

The file-centric ledger generalizes to asset nodes + a per-node depth state
drawn from a **profile-declared ladder**:

- code: `unreviewed → reviewed` (maps onto today's file events)
- binary: `located → disassembled → analyzed`
- network: `discovered → enumerated → tested → exploited`

`discover_targets`/`file_views` become one instantiation (the code profile) of
"enumerate assets of the declared kind(s)."

### The engagement-profile package (pure data)

Shipped built-in (embedded) with project override under `.rupu/profiles/`,
mirroring agent/workflow discovery. Illustrative (invented) `binary` profile:

```toml
id = "binary"
name = "Binary reverse engineering"

[[asset_kinds]]
id = "binary"
coordinates = ["sha256"]
label = "{sha256:short}"

[[asset_kinds]]
id = "function"
parent = "binary"
coordinates = ["sha256", "address", "symbol"]
label = "{symbol} @ {address:hex}"

evidence_blocks       = ["text", "code_slice", "diff", "hexdump", "disasm"]
classification_systems = ["CWE", "CVE"]

[[completeness]]
id = "evidence_has_listing"
label = "Evidence includes a disassembly or hexdump"
required = true
satisfied_when = { any = [ { has_block_kind = "disasm" }, { has_block_kind = "hexdump" } ] }

[coverage]
enumerates   = ["binary", "function"]
depth_ladder = ["located", "disassembled", "analyzed"]

[bundle]
agents    = ["binary-triage", "binary-analyst"]
tools     = ["mcp:radare2", "mcp:ghidra"]
workflows = ["binary-assessment"]
```

**Composite profiles.** A profile may `include` others; the loader expands it to
the union of their asset kinds, evidence blocks, taxonomies, coverage ladders,
and completeness rules. This is how one engagement runs several domains at once —
a pentest is a pure composite:

```toml
id = "pentest"
name = "Penetration test"
includes = ["network", "web"]   # union of both; findings route by asset-kind namespace
```

A composite may also add its own kinds/taxonomy on top of what it includes —
`mobile` is `includes = ["binary", "web"]` plus a `package` root kind and the
MASVS classification system.

Selection: `rupu run --engagement-profile <id>` for one, or
`--engagement-profiles a,b` / workflow `defaults.engagement_profiles: [a, b]` for
an explicit set; a composite id expands to its set. Built-in default is `code`.

## Engagement profile catalog (forward-compatibility)

These profiles are specified now to prove the ABI spans the coding *and*
security engagements we intend to run; they are **not** all built in the pilot.
rupu ships several as built-in profiles embedded in the binary; operators and
projects add or override their own under `~/.rupu/profiles/` (global) and
`.rupu/profiles/` (project), mirroring agent/workflow discovery. Only `binary`
is built in the pilot — the rest ship as pure data as their tooling lands. The
matrix reads off each one's true new-primitive cost: after the anticipated
coordinate + block set exists, most add zero Rust.

| Profile | Family | Root asset | Key coordinates | Evidence blocks | Taxonomies | Coverage ladder | New primitives |
|---|---|---|---|---|---|---|---|
| `code` | coding | repo | path, line_range, symbol | text, code_slice, diff | CWE, OWASP | unreviewed→reviewed | — (native) |
| `sca` | coding | repo | path, resource_id (purl) | table, text | CVE, GHSA | inventoried→resolved→triaged | — |
| `iac` | coding | repo | path, line_range | code_slice, diff, table | CIS, CWE | mapped→reviewed | — |
| `secrets` | coding | repo | path, line_range, commit | code_slice | CWE-798 | scanned→triaged→confirmed | commit |
| `container` | coding·sec | image | sha256, path, resource_id | table, text | CVE | inventoried→scanned→triaged | — |
| `binary` | security | blob | sha256, offset, address, symbol | +hexdump, disasm | CWE, CVE | located→disassembled→analyzed | hexdump, disasm *(pilot)* |
| `firmware` | security | image | +path | +decompile | CWE, CVE | acquired→extracted→unpacked→analyzed | decompile |
| `network` | security | host | host, port, url | scan_output, http_exchange, pcap_ref, table | CVE, CWE, CAPEC, ATT&CK | discovered→enumerated→tested→exploited | host/port/url + those blocks |
| `web` | security | site | url, http_route, param | http_exchange, image | OWASP, CWE, CVE | mapped→crawled→tested→exploited | http_route/param + image |
| `api` | security | service | url, http_route, param | http_exchange, table | OWASP-API, CWE | mapped→tested→exploited | — (reuses web) |
| `cloud` | security | account | resource_id | table, code_slice | CIS, CWE, ATT&CK | inventoried→policy-evaluated→validated | resource_id |
| `mobile` | security | package | sha256, address, symbol, http_route | disasm, http_exchange, image | MASVS, OWASP, CWE | unpacked→static→dynamic | none — composite (includes binary + web) |
| `threat-model` | design | system | (none — attributes) | text, image, table | STRIDE, CAPEC, ATT&CK | modeled→reviewed | none — coordinate-less |
| `redteam` | security | host | host, resource_id | text, scan_output, image | ATT&CK | planned→executed→validated | none — reuses network |
| `pentest` | composite | host + site | network + web kinds | network + web blocks | CVE, CWE, OWASP, CAPEC | both ladders | none — includes network + web |

Two rows stress the model on purpose:

- **Composite profiles** (`pentest`, `mobile`) activate several profiles at once
  via `includes`; findings route to the right sub-profile by their asset-kind
  namespace. `pentest = [network, web]` is pure composition; `mobile = [binary,
  web]` adds a `package` root + MASVS on top. This is the strongest proof the
  vocabulary generalizes — the direct answer to "one engagement, many domains".
- **`threat-model` is coordinate-less** — a design-level finding ("this trust
  boundary lacks authentication") has no file or address. Its assets carry an
  empty `Locator` and describe themselves through `attributes`; completeness
  leans on `HasField`/`HasClassificationSystem`, never `LocatorHasCoordinate`.
  `Locator(vec![])` is valid, so the model already admits it.

`firmware` is `binary`'s kinds nested under `image → partition → file → function`
(ladder `acquired→extracted→unpacked→analyzed`); it adds only the `decompile`
block and inherits `hexdump`/`disasm` from the pilot — the compounding payoff in
one profile.

### Worked profiles

`network`:

```toml
id = "network"
name = "Network security assessment"

evidence_blocks        = ["text", "table", "scan_output", "http_exchange", "pcap_ref"]
classification_systems = ["CVE", "CWE", "CAPEC", "ATT&CK"]

[[asset_kinds]]
id          = "host"             # root
coordinates = ["host"]
label       = "{host}"

[[asset_kinds]]
id          = "service"
parent      = "host"
coordinates = ["host", "port"]
label       = "{host}:{port} {port.proto}"

[[completeness]]
id             = "service_identified"
label          = "Affected service is pinned to host + port"
required       = true
satisfied_when = { all = [ { locator_has_coordinate = "host" }, { locator_has_coordinate = "port" } ] }

[coverage]
enumerates   = ["host", "service"]
depth_ladder = ["discovered", "enumerated", "tested", "exploited"]

[bundle]
agents    = ["recon", "service-analyst", "exploit-verifier"]
tools     = ["mcp:nmap", "mcp:nuclei", "mcp:tls-scan"]
workflows = ["network-assessment"]
```

`web`:

```toml
id = "web"
name = "Web application assessment"

evidence_blocks        = ["text", "http_exchange", "image", "code_slice", "diff"]
classification_systems = ["OWASP", "CWE", "CVE"]

[[asset_kinds]]
id          = "site"            # root
coordinates = ["url"]
label       = "{url}"

[[asset_kinds]]
id          = "route"
parent      = "site"
coordinates = ["http_route"]
label       = "{http_route.method} {http_route.path}"

[[asset_kinds]]
id          = "parameter"
parent      = "route"
coordinates = ["http_route", "param"]
label       = "{param}"

[[completeness]]
id             = "request_response"
label          = "Evidence includes the triggering request/response"
required       = true
satisfied_when = { has_block_kind = "http_exchange" }

[coverage]
enumerates   = ["route", "parameter"]
depth_ladder = ["mapped", "crawled", "tested", "exploited"]

[bundle]
agents    = ["crawler", "appsec-tester"]
tools     = ["mcp:http", "mcp:ffuf", "mcp:browser"]
workflows = ["web-assessment"]
```

## Selection, resolution & enforcement

An engagement profile must be nameable — and enforced — on **every path that
launches agent work**, with the same fail-closed discipline as the autoflow
author gate: a missing or conflicting profile is an error, never a silent
default into the wrong domain.

**Selection surfaces:**

- **CLI** — `rupu run --engagement-profile <id>` or `--engagement-profiles a,b`
  (mirrors `--findings-profile`).
- **Workflow YAML** — `defaults.engagement_profiles: [network, web]` (or a single
  composite id), plus an optional per-step *narrowing* to a subset (a pure-recon
  step to `network` only). Steps need not split by profile.
- **Agent frontmatter** — `engagementProfiles: [binary, firmware]`, the set an
  agent is written for (a disasm agent serves both); advisory for launch and
  checked for conflicts below.
- **Autoflows** — each autoflow (or entity path) carries a profile, resolved and
  enforced on the same paths the tool-grant author gate already guards.
- **Sessions** — a standalone/interactive session takes a profile (default
  `code`), overridable per session.
- **CP launcher / triggers / MCP** — the launcher sets it when starting a run;
  cron/workflow triggers inherit the workflow's; the MCP finding-record tools
  validate against the run's active profile.

**Resolution.** The **active set** of profiles is resolved at run level (workflow
`defaults` / composite expansion / `--engagement-profiles` / built-in default
`code`), and a step may *narrow* it to a subset. A finding's *validating* profile
is not resolved separately — it is the namespace of its asset's kind, which must
be in the active (possibly step-narrowed) set.

**What the active profile governs:**

- **Finding validation** — `report_finding` / `findings.record` validate the
  record against **the profile that owns the finding's asset kind** (its
  namespace): schema, taxonomy, and completeness, rejecting a mismatch at write
  time (today's strict `full` behavior).
- **Coverage** — each asset's kind and depth-ladder state are checked against its
  owning profile; a kind no active profile declares is rejected.

**What it deliberately does *not* govern — tool authorization.** Tool access
stays the permission resolver + agent frontmatter `tools:`. A profile's
`bundle.tools` is **launcher prefill only**, not a grant, so authorization keeps
one source of truth and no second, divergent grant path appears. (Open decision 2
revisits whether a profile should additionally hard-cap the toolset.)

**Multi-profile engagements.** A run activates a *set* of profiles (a pentest:
`network` + `web`, directly or via a `pentest` composite). Assets and findings
are not tagged with a step's profile — each is routed by its asset-kind namespace
to the profile that owns it, so one agent in one step can file a `network:service`
finding and a `web:route` finding side by side. There is no global "the run's one
profile".

**Fail-closed rules:**

- Unknown profile id anywhere → hard error at load/launch.
- An asset/finding whose kind namespace is not in the active (step-narrowed) set
  → rejected at write time.
- Agent `engagementProfiles` disjoint from the active set at launch → hard error
  (misconfiguration, not a silent pick).
- A legacy finding with no asset kind → `code`, for backwards-compat with
  existing runs.

**Open decisions (for discussion):**

1. **Active-set + route-by-kind (resolved).** A pentest showed per-step-single-
   profile is too rigid; the model is now an active *set* (composite or explicit),
   optionally narrowed per step, with each finding's profile taken from its
   asset-kind namespace. Remaining sub-question: may a step *widen* beyond the
   run's set, or only narrow? Recommendation: **narrow only**.
2. **Tool bundle: prefill vs. hard-cap** — keep `bundle.tools` as launcher
   prefill only (recommended), or also let a profile *cap* the toolset as a
   second enforcement layer over frontmatter?
3. **Agent declaration shape** — one profile per agent, or a compatibility set
   `engagementProfiles: [...]` (recommended — agents are often domain-shared)?
4. **Discovery precedence** — built-in embedded < `~/.rupu/profiles/` <
   `.rupu/profiles/` (recommended, mirrors agents/workflows). Confirm.

## Already exists vs. new (honest split)

| Concern | Reuse as-is | New (one-time, shared) |
|---|---|---|
| Locator | `path`, `line_range`, `sha256`; `binary_va` field | `Coordinate` enum: `offset`, `address`, `symbol` (pilot); `host`, `port`, `url` (network) |
| Evidence | text/code/diff renderers; content-addressed blob store | `hexdump`, `disasm` renderers (pilot); `http_exchange`/`scan`/`pcap` (network) |
| Taxonomy | `cwe: Vec<String>`, `cvss_v3` | `Classification { system, id, vector }` |
| Completeness | strict validation machinery | profile-declared checklist + `Predicate` vocabulary |
| Coverage | ledger, concern assertions, `Surface`/`FindingScope` | `Asset` graph store + profile depth ladder |
| Profile | `FindingProfile` resolve plumbing | engagement-profile package format + loader + resolution |

## Pilot scope

**In:** `Coordinate`/`Locator`, `Asset` + graph store, `EvidenceBlock` with
`hexdump`+`disasm` renderers, `Classification`, profile-declared completeness +
`Predicate`, coverage-as-asset-tree, the engagement-profile loader + resolution,
and a `binary` profile authored as data, wired end-to-end so a binary RE
workflow records a finding whose disasm evidence renders.

**Deferred / non-goals:**
- Renaming the blob store to "evidence store."
- Every profile in the catalog except `binary` (network, firmware, web, api,
  cloud, mobile, sca, iac, secrets, container, threat-model, redteam) — specified
  for forward-compatibility, shipped as data as their tooling lands.
- Any scope/RoE enforcement, sandboxing, egress control, or tool gating. Agents
  reach targets with bash and their own tooling; rupu validates findings, not
  traffic.
- CP web asset-graph viewer polish; macOS app (deprecated — CLI + CP web only).
- Export polish beyond md/html/pdf for the new blocks.
- Combined multi-profile engagements at runtime (the model allows it; the pilot
  runs a single profile).

## Open questions (for spec review)

1. **Do we re-express the `code` profile as a data package in the pilot, or keep
   it native and have it register through the same profile interface?**
   Recommendation: **keep code native, register through the new registry.**
   Rewriting code-as-data is a large backwards-compat surface (every existing
   finding/ledger reader); proving the abstraction with `binary` while code
   registers through the same seam de-risks the pilot. Full code-as-data
   migration becomes a later, isolated change.
2. Final `Predicate` set — is the four-primitive + `All`/`Any` vocabulary enough
   for binary, or does binary completeness need one more primitive?
3. Asset-id derivation — content+coordinate hash for stability across re-runs;
   confirm this survives a re-disassembly producing new addresses.
4. The selection/enforcement open decisions (hard-constraint vs. default, tool
   bundle prefill vs. hard-cap, agent declaration shape, discovery precedence)
   are enumerated in "Selection, resolution & enforcement".
