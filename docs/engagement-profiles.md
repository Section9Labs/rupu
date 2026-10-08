# Engagement profiles

An **engagement profile** tells rupu what kind of thing an engagement is about:
source code, a binary, a network, a web application, a cloud account, a threat
model. It is a small TOML file. It declares the **asset kinds** that exist in
that domain, the **coordinates** that pin each one down, a **completeness
checklist** that every finding must pass, and a **depth ladder** that records
how far each asset has been examined.

rupu ships 15 profiles. You can override any of them, or add your own, by
writing a TOML file. Supporting a new kind of engagement is a matter of
writing data. It needs no Rust.

- [Concepts](#concepts)
- [Selecting profiles](#selecting-profiles)
- [The built-in catalog](#the-built-in-catalog)
- [Composites and routing](#composites-and-routing)
- [Recording assets](#recording-assets)
- [Writing a profile](#writing-a-profile)
- [Profiles, finding profiles and concern catalogs](#profiles-finding-profiles-and-concern-catalogs)

Design spec: `docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md`.

---

## Concepts

**Asset.** A finding is always about something: a file, a function at an
address in a binary, a service on a host, a route on a website, a cloud
resource. rupu calls that thing an *asset*. An asset has a kind, a locator
and, optionally, a depth.

**Kind.** Every asset kind belongs to a profile and carries that profile's
namespace: `network:service`, `web:route`, `binary:function`, `code:file`.
Kinds can form a hierarchy within their profile (a `service` sits under a
`host`).

**Coordinates and locators.** A *locator* is the set of typed coordinates that
pins an asset down, for example a host plus a port. The core knows exactly 13
coordinate types, and every profile builds on those:

| Tag | JSON value (`{"t": <tag>, "v": <value>}`) | Typical use |
|-----|-------------------------------------------|-------------|
| `path` | string | file or archive path |
| `line_range` | `{"start": 10, "end": 24}` | lines in a file |
| `symbol` | string | function or symbol name |
| `commit` | string | VCS commit |
| `sha256` | string | content hash of a binary, image or package |
| `offset` | integer | byte offset |
| `address` | integer | virtual address |
| `host` | string | hostname or IP |
| `port` | `{"number": 443, "proto": "tcp"}` (`tcp` \| `udp` \| `other`) | network port |
| `url` | string | URL |
| `http_route` | `{"method": "GET", "path": "/api/items"}` | HTTP method + path |
| `param` | string | request parameter |
| `resource_id` | `{"scheme": "arn", "id": "…"}` | cloud resource, package URL, objective id |

A locator can be empty. A threat-model component, for example, has no
coordinates at all.

**Routing.** When a finding names an asset, its kind's namespace picks the
profile that owns it (`network:service` → `network`). That profile, and only
that profile, judges the finding.

**Validation.** The owning profile's required completeness checks are run
against the finding's report and the asset's locator. The `network` profile,
for instance, refuses a finding that isn't pinned to both a host and a port.
A finding that fails is rejected, and the error lists each unmet check, so the
agent can fix the finding and record it again.

**Depth.** Each profile has a depth ladder, such as `discovered → enumerated →
tested → exploited` for `network`. The ladder records how deeply each asset
was examined, separately from any findings about it.

> **Not a sandbox.** A profile governs how findings are *typed, routed and
> validated*. It doesn't run, gate or scope tools, and it doesn't limit what an
> agent can reach. Agents examine binaries, hosts and services with `bash` and
> whatever tooling they like (nmap, curl, radare2, …). rupu records and
> validates the findings that result, not the traffic.

---

## Selecting profiles

| Surface | How | Notes |
|---------|-----|-------|
| `rupu run` | `--engagement-profile <id>` | Alias `--engagement-profiles`. Repeat the flag or pass a comma-separated list. Sub-agents started with `dispatch_agent` inherit the selection. |
| `rupu workflow run` | `--engagement-profile <id>` | Applies to every agent step and to `action: findings.record` steps. The selection is recorded on the run (`engagement_profiles` in `run.json`), so `rupu workflow resume`, `workflow approve` and a gate approved in the control plane continue under it. A recorded profile that no longer resolves fails the resume. |
| Agentiflows | `engagement_profiles:` in the definition (required) | rupu validates the definition against the profiles, and passes the set to the lead and to every unit it launches (as `--engagement-profile`). See [agentiflows.md](agentiflows.md#4-engagement-profiles). |
| CP agent launch API | `engagement_profiles: [..]` in the `POST /api/agents/:name/run` body | Passed to `rupu run --engagement-profile`, locally or on the chosen host (see the remote row). The web launcher doesn't offer it. |
| Sessions, autoflows | — | These don't accept a selection and always run on the `code` path. |
| Remote workflow units (`host:` / `distribute:`) | inherited from `rupu workflow run` | The ids travel to the host as `rupu run --engagement-profile`, on every transport. A host must advertise `agent.engagement_profile` (HTTP `/api/host/info` `features`, tunnel `Hello`, bucket worker marker); otherwise the launch is refused, never run on the `code` path. An SSH remote too old for the flag rejects it and the launch fails. The host resolves the ids against its own profiles, so install a custom profile there too. |
| `rupu coverage rerun` | — | Replays on the `code` path. The original run's selection isn't recorded. |

```bash
rupu run --engagement-profile binary  firmware-re  "reverse ./dist/updater.bin"
rupu run --engagement-profile network recon        "map 10.20.0.0/24"
rupu run --engagement-profile network,web assessor "..."   # two profiles
rupu run --engagement-profiles pentest assessor    "..."   # a composite
rupu workflow run edge-sweep --engagement-profile network
```

The rules:

- **No selection** gives the native `code` path. This is not the same as
  selecting `code`: no engagement is active, so `assets.mark` isn't offered and
  `asset` arguments are ignored.
- **An unknown id** fails the launch. It never falls back to a default.
- **A kind claimed twice** is an error. If the same asset kind would come from
  two selections, the launch fails. For example, `pentest,network` fails
  because `pentest` already includes `network`.
- **Overlay profiles load at launch.** If a discovered profile file fails to
  parse, the launch fails, even if you didn't select that profile.

---

## The built-in catalog

`code` is the default. The table below lists every profile shipped in the
binary (sources: `crates/rupu-coverage/src/profile/builtin/*.toml`). Each
asset-kind entry gives its locator coordinates, with its parent kind in
parentheses where it has one. Completeness checks are the profile's checklist,
and every built-in check is `required = true`.

| Profile | Purpose | Asset kinds → coordinates | Depth ladder |
|---------|---------|---------------------------|--------------|
| `code` | Source code review | `repo` → path; `file` (repo) → path, line_range; `function` (file) → path, symbol, line_range | unreviewed → reviewed |
| `binary` | Binary reverse engineering | `binary` → sha256; `function` (binary) → sha256, address, symbol | located → disassembled → analyzed |
| `firmware` | Firmware assessment | `image` → sha256; `partition` (image) → sha256, path; `file` (partition) → path, sha256; `function` (file) → path, address, symbol | acquired → extracted → unpacked → analyzed |
| `network` | Network security assessment | `host` → host; `service` (host) → host, port | discovered → enumerated → tested → exploited |
| `web` | Web application assessment | `site` → url; `route` (site) → url, http_route, param | mapped → crawled → tested → exploited |
| `api` | API security assessment | `service` → url; `endpoint` (service) → url, http_route, param | mapped → tested → exploited |
| `cloud` | Cloud configuration review | `account` → resource_id; `resource` (account) → resource_id | inventoried → policy-evaluated → validated |
| `sca` | Software composition analysis | `repo` → path; `dependency` (repo) → path, resource_id | inventoried → resolved → triaged |
| `iac` | Infrastructure-as-code review | `repo` → path; `resource` (repo) → path, line_range | mapped → reviewed |
| `secrets` | Secret scanning | `repo` → path; `secret` (repo) → path, line_range, commit | scanned → triaged → confirmed |
| `container` | Container image assessment | `image` → sha256; `layer` (image) → sha256, path; `package` (image) → resource_id | inventoried → scanned → triaged |
| `redteam` | Red team operation | `host` → host; `objective` (host) → host, resource_id | planned → executed → validated |
| `threat-model` | Threat model | `system` → (none); `component` (system) → (none) | modeled → reviewed |
| `mobile` | Mobile application assessment (composite: includes `binary`, `web`) | `package` → sha256 | unpacked → static → dynamic |
| `pentest` | Penetration test (composite: includes `network`, `web`) | none of its own | none of its own |

### Completeness, evidence and taxonomies

The required checks are what `findings.report` enforces. **Evidence blocks**
and **taxonomies** describe the evidence and classification systems the
engagement expects, but they're advisory. rupu enforces them only when a
completeness check names one.

| Profile | Required completeness checks | Evidence blocks | Taxonomies |
|---------|------------------------------|-----------------|------------|
| `code` | `located`: locator has `path` · `has_root_cause`: `root_cause` filled | text, code_slice, diff | CWE, OWASP |
| `binary` | `evidence_has_listing`: a `disasm` or `hexdump` block · `has_root_cause` · `classified`: a CWE | text, code_slice, diff, hexdump, disasm | CWE, CVE |
| `firmware` | `evidence_has_listing`: a `disasm`, `decompile` or `hexdump` block · `classified`: a CWE | text, hexdump, disasm, decompile, table | CWE, CVE |
| `network` | `service_identified`: `host` **and** `port` · `has_root_cause` · `classified`: a CVE or CWE | text, table, scan_output, http_exchange, pcap_ref | CVE, CWE, CAPEC, ATT&CK |
| `web` | `route_identified`: `http_route` or `url` · `evidence_has_exchange`: an `http_exchange` block · `classified`: OWASP or CWE | text, http_exchange, image, table | OWASP, CWE, CVE |
| `api` | `endpoint_identified`: `http_route` · `evidence_has_exchange`: an `http_exchange` block | text, http_exchange, table | OWASP-API, CWE |
| `cloud` | `resource_identified`: `resource_id` · `classified`: CIS or CWE | text, table, code_slice | CIS, CWE, ATT&CK |
| `sca` | `dependency_identified`: `resource_id` (a purl) · `classified`: CVE or GHSA | table, text | CVE, GHSA |
| `iac` | `located`: `path` **and** `line_range` · `classified`: CIS or CWE | code_slice, diff, table | CIS, CWE |
| `secrets` | `located`: `path` **and** `line_range` · `classified`: a CWE | code_slice, text | CWE |
| `container` | `component_identified`: `resource_id` or `path` · `classified`: a CVE | table, text | CVE |
| `redteam` | `mapped_to_attack`: an ATT&CK classification · `has_narrative`: `description` filled | text, scan_output, image | ATT&CK |
| `threat-model` | `has_threat`: `description` filled · `classified`: a STRIDE classification | text, image, table | STRIDE, CAPEC, ATT&CK |
| `mobile` | `package_identified`: `sha256`, `address` or `http_route` (for its own `mobile:package` kind) | disasm, http_exchange, image | MASVS, OWASP, CWE |
| `pentest` | none of its own | — | — |

Taxonomy matching ignores case. A report's legacy `cwe` list counts as `CWE`
classifications.

Every built-in also declares a `[bundle]`: the agents and workflows an
engagement of that kind typically uses. Those agents and workflows ship as the
**stock fleet** (`rupu fleet install`; see [The stock fleet](#the-stock-fleet)).
A bundle is metadata only. It never grants tools, and nothing selects or loads
it at run time.

---

## Composites and routing

A **composite** is a profile with an `includes` list:

- `pentest` includes `network` and `web`.
- `mobile` includes `binary` and `web`, and adds its own root kind,
  `mobile:package`, plus MASVS.

Selecting a composite is shorthand for selecting its members. Each member
stays its **own routing target**. The members are not merged into one profile:

- During a `pentest` run, a `network:service` finding is judged by `network`'s
  checks (host + port, root cause, CVE/CWE). A `web:route` finding is judged by
  `web`'s checks (route, HTTP exchange, OWASP/CWE).
- During a `mobile` run, `binary:function`, `web:route` and `mobile:package`
  findings are each judged by the profile that owns them. `mobile`'s
  `package_identified` check covers `mobile:package` findings only.

`includes` is transitive, so a composite can include another composite. An
include cycle, or an include of a profile that doesn't exist, is a load error.

---

## Recording assets

Two tools write assets. Both write to the target's asset ledger,
`.rupu/coverage/<target>/assets.jsonl`, which sits next to `findings.jsonl`.

### `findings.report` with an `asset`

When an engagement is active, `findings.report` takes an optional `asset`. So does
the MCP `findings.record` tool that a workflow `action:` step calls, with the same
schema and handling:

```json
{
  "scope": "edge-sweep",
  "report": { "...": "a complete finding report" },
  "asset": {
    "kind": "network:service",
    "coordinates": [
      { "t": "host", "v": "198.51.100.20" },
      { "t": "port", "v": { "number": 8443, "proto": "tcp" } }
    ],
    "label": "198.51.100.20:8443 admin console"
  }
}
```

When the finding comes in, rupu:

1. **Routes** it by `kind`, which must be namespaced. If no active profile owns
   the kind, the call fails with an error that names the active set.
2. **Checks completeness.** Every `required` check of the owning profile is
   evaluated against the report and the locator, under the `full` finding
   profile. If any check fails, the finding is rejected and the error lists
   the labels of the failing checks. Under the `summary` finding profile
   there's no report to evaluate, so this step is skipped.
3. **Stamps the asset.** rupu writes a line to `assets.jsonl` and streams it to
   the run's `coverage.jsonl`, so a coordinator collecting a remote unit
   receives it. The new line keeps the asset's existing depth, parent,
   attributes and label, unless the call supplies a new label.

The finding record itself does not store an asset id. When no engagement is
active, `asset` is ignored. When an engagement is active but the finding has no
`asset`, the finding is recorded on the plain path without a completeness
check.

`rupu findings import` records no asset.

### `assets.mark`

`assets.mark` records how far an asset has been examined:

```json
{ "kind": "web:route",
  "coordinates": [ { "t": "url", "v": "https://shop.example.test" },
                   { "t": "http_route", "v": { "method": "POST", "path": "/cart/apply" } } ],
  "depth": "tested" }
```

- `depth` must be a rung of the owning profile's ladder. Any other value is an
  error, and the error message lists the ladder.
- **Depth only moves forward.** If an asset is already at `exploited`, marking
  it `tested` leaves it at `exploited`. The tool returns the rung that ends up
  recorded (`effective_depth`).
- The engine offers `assets.mark`, and `findings.report`, to every agent in an
  engagement run, whatever the agent's `tools:` list says. Without an active
  engagement, `assets.mark` doesn't exist.

### The ledger and its views

`assets.jsonl` only ever gets appended to. Each line is one full `Asset`:
`id`, `kind`, optional `parent`, `locator`, `label`, optional `depth` and
`attributes`. When the file is read, the last line for each id wins. An asset's
id is `<kind>:<16 hex>`, derived from the kind plus the sorted locator, so the
same host and port always map to the same asset, across reruns too. The
profile's `label` template is declarative and isn't rendered: an asset's label
is whatever the caller passed, or else its kind.

Remote units' asset lines arrive through the coverage stream and are merged
into the coordinator, deduplicated by occurrence. Two control-plane views read
the ledger, both through `GET /api/assets`:

- **Security → Assets** lists every asset from every registered workspace, with
  its profile, kind, label, coordinates and depth.
- An agentiflow's **Assets** tab shows the same data, filtered with `?ws_id=` /
  `?target=`.

Agentiflow coverage goals measure this ledger. A goal can require that a
fraction of the assets of the kinds a profile enumerates reach a given rung.

---

## The stock fleet

A profile describes what an engagement's assets and findings look like; it
doesn't provide agents. The **stock fleet** does: a set of generic agents and
workflows covering every built-in profile, which you install once and point at
a target by giving them scope.

```bash
rupu fleet list                 # what ships
rupu fleet install              # into ~/.rupu/agents and ~/.rupu/workflows
rupu fleet install --project    # into this project's .rupu/ instead
rupu fleet install --force      # overwrite files that already exist
```

`install` writes into the global rupu root (`~/.rupu`, or `$RUPU_HOME`) by
default, so every project can use the fleet. `--project` writes into the
current project's `.rupu/` and fails outside a project (run `rupu init`
first). A file that already exists is kept, so your edits survive a reinstall;
`--force` overwrites it. Each file is reported as `CREATED`, `SKIPPED` or
`OVERWROTE`. A project's own agent or workflow of the same name still shadows
the global one.

The fleet is exactly what the built-in profiles' bundles name:

| Profile | Agents | Workflows |
|---------|--------|-----------|
| `network` | `assessment-lead`, `recon`, `service-analyst`, `exploit-verifier` | `network-assessment` |
| `web` | `assessment-lead`, `crawler`, `appsec-tester` | `web-assessment` |
| `api` | `assessment-lead`, `api-tester` | `api-assessment` |
| `pentest` | `assessment-lead`, `recon`, `service-analyst`, `exploit-verifier`, `crawler`, `appsec-tester` | `network-assessment`, `web-assessment` |
| `redteam` | `assessment-lead`, `redteam-operator` | — |
| `code` | `code-auditor` | — |
| `binary` | `binary-analyst` | — |
| `firmware` | `firmware-analyst` | — |
| `mobile` | `mobile-analyst` | — |
| `cloud` | `cloud-auditor` | — |
| `sca` | `sca-auditor` | — |
| `iac` | `iac-reviewer` | — |
| `secrets` | `secret-scanner` | — |
| `container` | `container-scanner` | — |
| `threat-model` | `threat-modeler` | — |

That is 18 agents and 3 workflows. A test keeps the two in step: every name a
built-in bundle lists must ship in the fleet, and every fleet file must be
named by some bundle.

**How the agents behave.** Every fleet agent runs in `bypass` mode and asks for
the full tool set (`tools: ["*"]`) plus `findings.report`, `assets.mark`,
`findings.verify`, `findings.query` and `findings.tag`. None of them has a target
built in: each reads its authorized scope from its task prompt (and the
engagement's scope), treats it as a hard boundary, records what it finds with
`assets.mark` at the right depth rung, and records verified issues with
`findings.report`. Run them under the matching profile so `assets.mark` is
offered and their findings are checked against it:

```bash
rupu run --engagement-profile network recon "Authorized scope: 198.51.100.0/28"
rupu workflow run web-assessment --engagement-profile web \
  --input scope="https://staging.example.test (and nothing else)"
```

The three workflows each take one required input, `scope`, and chain the
specialists in order: `network-assessment` runs recon → service analysis →
exploit verification; `web-assessment` runs crawl → application testing;
`api-assessment` runs one API-testing step.

**`assessment-lead` and agentiflows.** `assessment-lead` is the fleet's
coordinator, written to be an agentiflow's `lead:`. It does not test anything
itself: it lists the pool (`agents.list`, `workflows.list`), dispatches
discovery agents first, then analysis and testing agents over what they
enumerated, then a verification pass, checking `goal.status`,
`goal.coverage` and `budget.status` between rounds and winding down when the
scope is covered or the budget runs low. An agentiflow that uses it puts it in
both `lead:` and `pool.agents`, alongside the specialists:

```yaml
# .rupu/agentiflows/lab-network.yaml (invented example)
name: lab-network
lead: assessment-lead
engagement_profiles: [network]
goals:
  - id: gateway
    # The lead also sees scope.roots (below); spelling the limits out here helps.
    objective: "Authorized scope: 198.51.100.0/28 only. Enumerate and test every service; start with the gateway at 198.51.100.1."
    target:
      asset: { kind: "network:host", locator: { host: "198.51.100.1" } }
      depth_at_least: tested
    required: false
coverage:
  reach: 1.0             # stop once every discovered service...
  depth: tested          # ...is tested or deeper
  kinds: [network:service]
scope:
  authorized: true
  roots:
    - kind: network:host
      host: "198.51.100.1"
pool:
  agents: [assessment-lead, recon, service-analyst, exploit-verifier]
  workflows: [network-assessment]
budget:
  usd: 15
```

The lead's first-round mission lists the goals' `objective` text, the coverage
target and the authorized scope (`scope.mode` and each `scope.roots` entry with
its coordinates). The roots are shown, not enforced: units can still reach
anything their tools can. See
[agentiflows.md](agentiflows.md) for the definition format and how a run
proceeds.

---

## Writing a profile

### Where profiles live and which wins

| Source | Location | Precedence |
|--------|----------|------------|
| Built-in | compiled into the binary | lowest |
| Global | `~/.rupu/profiles/*.toml` (`<RUPU_HOME>/profiles/`) | overrides built-in |
| Project | `.rupu/profiles/*.toml` in the run's workspace (for an agentiflow: the directory you launch from) | highest |

- rupu identifies a profile by its `id` field. The file name doesn't matter.
- A later source with the same `id` **replaces** the earlier profile
  completely. Nothing is merged, so to change one check of a built-in, copy the
  whole built-in and edit the copy.
- Within one directory, files load in name order. If two files declare the same
  `id`, the one that sorts last wins.
- Only `*.toml` files are read, and a missing directory is fine.

### Schema

All keys are strict. **An unknown key in any table is a parse error.**

**Top level.** Put these before the first `[[asset_kinds]]` table, or TOML
assigns them to that kind and the file fails to parse.

| Key | Type | Required | Meaning |
|-----|------|----------|---------|
| `id` | string | yes | The profile id, and the namespace of its kinds (`<id>:<kind>`) |
| `name` | string | yes | Human-readable name |
| `includes` | string list | no | Profile ids this one includes (makes it a composite) |
| `evidence_blocks` | string list | no | Evidence-block kinds the engagement expects (advisory) |
| `classification_systems` | string list | no | Taxonomies the engagement uses (advisory) |

**`[[asset_kinds]]`** (repeatable):

| Key | Type | Required | Meaning |
|-----|------|----------|---------|
| `id` | string | yes | The bare kind id. rupu namespaces it as `<profile>:<id>`. |
| `parent` | string | no | Parent kind id in the same profile. Omit for a root kind. |
| `coordinates` | string list | no | Coordinate tags an asset of this kind carries (descriptive) |
| `label` | string | yes | Label template, e.g. `"{host}:{port}"` (descriptive, not rendered) |

**`[[completeness]]`** (repeatable):

| Key | Type | Required | Meaning |
|-----|------|----------|---------|
| `id` | string | yes | Check id |
| `label` | string | yes | Shown in the rejection message when the check fails |
| `required` | bool | no (default `true`) | `false` makes the check informational. It isn't evaluated. |
| `satisfied_when` | predicate | yes | See below |

**`[coverage]`**:

| Key | Type | Meaning |
|-----|------|---------|
| `enumerates` | string list | The bare kind ids that agentiflow coverage goals count by default |
| `depth_ladder` | string list | Ordered rungs, shallowest first. `assets.mark` accepts only these. The last rung is the default target depth for a coverage goal. |

**`[bundle]`** holds `agents`, `workflows` and `tools`, all string lists. It's
descriptive metadata: it names the definitions an engagement of this kind
typically uses, and never grants anything. rupu does not load or check the
names in your own profile's bundle; the built-in bundles are kept in step with
the stock fleet by a test.

### Predicates

`satisfied_when` is one predicate, written as an inline table:

| Predicate | True when |
|-----------|-----------|
| `{ has_field = "<field>" }` | the report field is filled in. Fields: `root_cause`, `remediation`, `impact`, `description`, `attack_vector`, `category` (each must be non-blank), `replication_steps` (non-empty), `evidence` (any evidence claim or block). |
| `{ has_block_kind = "<kind>" }` | the report has at least one evidence block of that kind: `text`, `code_slice`, `diff`, `table`, `image`, `hexdump`, `disasm`, `decompile`, `http_exchange`, `scan_output`, `pcap_ref`. |
| `{ has_classification_system = "<system>" }` | at least one classification uses that system (case-insensitive; legacy `cwe` entries count as `CWE`). |
| `{ locator_has_coordinate = "<tag>" }` | the asset's locator has a coordinate with that tag (one of the 13 above). |
| `{ min_severity = "<Level>" }` | the report's `rating.risk_rating` is at least `Low`, `Medium`, `High` or `Critical`. These names are capitalized. |
| `{ all = [ … ] }` / `{ any = [ … ] }` | every / at least one nested predicate holds |

That is the whole predicate grammar. It isn't meant to grow into a scripting
language.

### Failure modes

| Mistake | When it shows up | What you see |
|---------|------------------|--------------|
| Unknown key, wrong type, or a top-level key after `[[asset_kinds]]` | launch (any run with a selection) | `engagement profiles: unloadable profiles: <file>: profile parse: …`. The run fails even if this profile wasn't selected. |
| Unreadable file or directory | launch | Same message, with the IO error |
| `includes` names a missing profile, or includes form a cycle | launch | ``profile `x` is included but not defined`` / ``include cycle at profile `x` `` |
| Selected id doesn't exist | launch | ``unknown engagement profile `x` `` |
| Two selections contribute the same kind | launch | ``asset kind `x:y` is contributed by more than one selected profile`` |
| Typo in a predicate's field, block kind or coordinate tag | the first finding that routes to the profile | That finding is rejected with `completeness predicate error for profile …` |
| Unknown rung in an agentiflow goal, or a kind no profile owns | agentiflow load | The definition is refused |

Loading checks only the TOML structure. A predicate's field names, block
kinds and coordinate tags are first checked when a finding reaches it, so test
a new profile with one real finding before you rely on it.

### Worked example: an IoT fleet engagement

The profile below is invented for this guide. It covers fleets of embedded
devices: a device is found by its address, it exposes services, and each
firmware build is identified by its hash. Save it as
`.rupu/profiles/iot-fleet.toml`:

```toml
id = "iot-fleet"
name = "IoT device fleet assessment"
evidence_blocks = ["text", "scan_output", "hexdump", "http_exchange", "table"]
classification_systems = ["CWE", "CVE"]

[[asset_kinds]]
id = "device"
coordinates = ["host"]
label = "{host}"

[[asset_kinds]]
id = "service"
parent = "device"
coordinates = ["host", "port"]
label = "{host}:{port}"

[[asset_kinds]]
id = "firmware"
parent = "device"
coordinates = ["host", "sha256"]
label = "{sha256}"

[[completeness]]
id = "pinned"
label = "Pinned to a device service or a firmware build"
satisfied_when = { all = [
  { locator_has_coordinate = "host" },
  { any = [ { locator_has_coordinate = "port" }, { locator_has_coordinate = "sha256" } ] },
] }

[[completeness]]
id = "evidence"
label = "Scan output, hexdump or HTTP exchange attached"
satisfied_when = { any = [
  { has_block_kind = "scan_output" },
  { has_block_kind = "hexdump" },
  { has_block_kind = "http_exchange" },
] }

[[completeness]]
id = "classified"
label = "Classified (CWE or CVE)"
satisfied_when = { any = [ { has_classification_system = "CWE" }, { has_classification_system = "CVE" } ] }

[[completeness]]
id = "has_fix"
label = "Remediation for high-severity issues"
required = false        # informational: not evaluated
satisfied_when = { has_field = "remediation" }

[coverage]
enumerates = ["device", "service", "firmware"]
depth_ladder = ["discovered", "fingerprinted", "tested", "exploited"]
```

Run with it:

```bash
rupu run --engagement-profile iot-fleet fleet-recon "assess the lab segment 192.0.2.0/28"
```

During the run, the agent might call:

- `assets.mark` with kind `iot-fleet:device`, coordinates
  `[{"t":"host","v":"192.0.2.7"}]` and depth `fingerprinted`.
- `findings.report` with an `asset` of kind `iot-fleet:service`, pinned by
  `host` `192.0.2.7` and `port` `{"number": 23, "proto": "tcp"}`, and a report
  that has a `scan_output` block and a CWE classification.

If the agent omits the port and the hash, the finding is rejected with
`Pinned to a device service or a firmware build`, and the agent can add the
missing coordinates and try again.

---

## Profiles, finding profiles and concern catalogs

Three separate rupu features have names that sound alike. They don't depend
on each other:

| | Engagement profile | Finding profile | Concern catalog |
|---|---|---|---|
| Answers | *What is this engagement about, and when is a finding about it complete?* | *How much does each finding record?* | *What should the agent check for, file by file?* |
| Values | `code`, `network`, `web`, … or your own TOML | `full` (default) or `summary` | `owasp-top10-2021`, `cwe-top25-2023`, … or your own YAML |
| Chosen by | `--engagement-profile`, agentiflow `engagement_profiles:` | agent `findingsProfile`, workflow `defaults.findings_profile` / step `findings_profile`, `rupu run --findings-profile` | the agent's `concerns:` frontmatter block |
| Lives in | `crates/rupu-coverage/src/profile/builtin/`, `~/.rupu/profiles/`, `.rupu/profiles/` | built into rupu | `crates/rupu-coverage/templates/concerns/`, `~/.rupu/concerns/`, `.rupu/concerns/` |
| Records | `assets.jsonl` + completeness-gated findings | the shape of each record in `findings.jsonl` | `concerns.jsonl` (file × concern verdicts) |

The three combine as follows:

- **Engagement profiles depend on `full` reports.** Completeness checks run
  only on a `full` report. Under `summary` an asset is still routed and
  recorded, but its completeness checks are skipped. See
  [coverage.md → Finding reports](coverage.md#finding-reports).
- **Concern catalogs are lists of things to check.** A catalog lists concerns
  that an agent judges file by file (`coverage.mark`). It's independent of the
  engagement: a `code` engagement can use `owasp-top10-2021`, and so can a run
  with no engagement.

The bundled concern catalogs (`rupu coverage templates list`; print one with
`rupu coverage templates show <name>`):

| Catalog | Contents |
|---------|----------|
| `owasp-top10-2021` | OWASP Top 10 web risks, 2021 (10 concerns) |
| `owasp-api-top10-2023` | OWASP API Security Top 10, 2023 (10) |
| `cwe-top25-2023` | 2023 CWE Top 25 most dangerous weaknesses (25) |
| `cwe-software-development` | MITRE CWE Software Development view (~399) |
| `cwe-research` | MITRE CWE Research view (~944) |
| `stride` | STRIDE threat categories (6) |
| `secrets-in-source` | hardcoded secrets in source (1) |
| `code-smells` | Fowler-style code smells (12) |
| `web-security-default` | bundle: owasp-top10-2021 + cwe-top25-2023 + secrets-in-source |
| `api-security-default` | bundle: owasp-api-top10-2023 + cwe-top25-2023 + secrets-in-source |

See [coverage.md → Turning it on](coverage.md#turning-it-on-the-concerns-block)
for the `concerns:` block.

## See also

- [agentiflows.md](agentiflows.md): goals and coverage stops measured against assets
- [coverage.md](coverage.md): the coverage harness, finding reports, queries, tags and exports
- [using-rupu.md](using-rupu.md#run-a-workflow-under-an-engagement-profile): running a workflow under a profile
