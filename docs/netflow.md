# Netflow: where a run reaches out to

rupu records the network connections a run makes. Every request rupu's own
HTTP client sends for an agent (model providers, SCM and issue connectors) and
every TCP or UDP connection opened by a process the agent started through its
`bash` tool becomes a **flow**. Flows go to an append-only ledger per run, are
enriched with the peer's ASN and organisation when you read them, and can be
explored in the control plane's **Network** views or printed with
`rupu netflow show`.

Netflow is telemetry only. It does not block anything, it carries no severity,
and it never turns into findings.

---

## What is recorded

There are two capture paths, and both write to the same per-run ledger.

**rupu's own HTTP traffic.** Every provider and SCM client is built through one
instrumented HTTP client, so each request is recorded with its method, host,
port, path (with the query string removed), peer IP, status, byte counts and
timings. The run, step and agent are bound when the client is built for a run.
The GitHub connector uses its own HTTP stack, so its flows are recorded at a
coarser level (host, outcome and timing only).

**Connections from `bash` subprocesses.** A `curl`, `git fetch`, `pip install`
or scanner started by the agent's `bash` tool does not use rupu's HTTP client.
rupu reads the socket accounting the operating system already keeps and
attributes each socket to the `bash` call whose processes opened it. Nothing is
put in the command's network path: there is no proxy, no root requirement, no
change to the command's environment or privileges, and raw-socket tools keep
working.

| Platform | How sockets are observed | How they are attributed |
|----------|--------------------------|-------------------------|
| macOS | The kernel's network-statistics interface (the source `nettop` uses), available to a normal user: each TCP and UDP socket's addresses, owning process and byte counts as it opens, updates and closes | By process ancestry: a socket belongs to a call when its owning process descends from that call's shell |
| Linux | `sock_diag` netlink: a poll of every TCP/UDP socket (every `subprocess_poll_ms`, default 50 ms) plus the kernel's socket-close notifications with final byte counts | By cgroup: each `bash` call's shell moves itself into a per-call cgroup before running anything, so every descendant, even a detached one, is attributed. This needs cgroup v2 and either a systemd user manager or a writable, delegated cgroup; without them, capture is reported unavailable with the reason |
| Other platforms | Not supported | Capture is reported unavailable |

A socket flow records the transport, remote IP and port, local address, the
process name and pid, open and close times, and bytes in and out where the OS
reports them. It carries the id of the `bash`
tool call that caused it, so the control plane links the flow to that call in
the transcript. There is no URL, method or HTTP status for a socket flow:
those are inside the (usually encrypted) stream.

If a capture backend cannot start, the run records why, once ("subprocess
capture unavailable: …"), and the control plane shows it. Capture problems
never affect the command or the run.

### What is not recorded

- **Hostnames for subprocess traffic.** Socket capture sees IP addresses, not
  names, so a subprocess flow shows an IP and its ASN organisation.
- **Some short or detached connections.** A UDP `sendto` without `connect` has
  no destination. On macOS a process that detaches from its shell before
  connecting can be missed; on Linux a connection that opens and closes
  between two polls can be missed.
- **rupu's non-HTTP egress.** `git2` clones (often the largest volume in a
  run), object-store bucket traffic and the tunnel node's WebSocket.
- **Traffic with no run behind it.** The updater, login and OAuth, the control
  plane's own host traffic and the ASN download are not recorded at all.
- **Remote hosts' flows.** A unit placed on a remote host records its flows in
  that host's ledger; they are not brought back to the coordinator.
- **Query strings and headers**, deliberately: they routinely carry tokens.
  There is no setting to record them.

---

## Fidelity

Every flow says how much of it is known. The control plane shows it as a badge
on each row.

| Fidelity | Source | What is known |
|----------|--------|---------------|
| `http` | rupu's instrumented client | Exact request and response metadata, including peer IP and byte counts |
| `coarse` | A connector whose HTTP stack rupu does not own (GitHub) | Host, outcome and timing. No byte counts or peer IP, so no ASN |
| `socket` | The OS socket table (`bash` subprocesses) | Process, remote IP and port, timing, and bytes where the OS reports them. No URL, method or status |
| `full` | Reserved for frame-level capture | Not emitted today |

An unknown value is left out, never written as zero: a missing byte count
renders as a dash, not `0 B`, and a latency with no measured duration is a
dash, not `0 ms`.

---

## The ledgers

Each run, workflow step, fan-out unit, sub-agent and session turn has its own
ledger:

| Path | Used when |
|------|-----------|
| `<project>/.rupu/netflow/<run_id>.jsonl` | The project has a `.rupu/netflow/` directory (`rupu init` creates it) |
| `~/.rupu/netflow/<run_id>.jsonl` (or under `$RUPU_HOME`) | Otherwise, so a repository that was never initialised never gets a ledger inside it |

The ledger directory gets its own `.gitignore` containing `*` when it is
created (an existing one is left alone), because a ledger lists every host, IP
and path a run contacted.

Ledgers are append-only JSONL. Each line has a `type`:

| `type` | Meaning |
|--------|---------|
| `flow` | One flow record (fields below) |
| `complete` | Finalises a streamed HTTP response written earlier: the flow `id`, the observed `bytes_in` and `duration_ms` |
| `socket_complete` | Finalises a socket flow when the socket closes: `id`, `duration_ms`, and the final `bytes_in`, `bytes_out`, `outcome` and `error` where known |
| `capture` | Subprocess-capture state for the run: `state: active` with its `backend`, or `state: unavailable` with a `reason`, plus an optional loss `note` |
| `dropped` | A `count` of records lost because the writer's buffer overflowed, and when |

`complete` and `socket_complete` lines are folded into their flow when the
ledger is read. A missing file reads as an empty ledger, and a malformed line
(for example a torn write at the end) is skipped rather than failing the read.

A socket flow and the line that closes it, invented for illustration (one
JSON object per line in the file; wrapped here):

```json
{"type":"flow","id":"01JB7Q3V9F8M2K4N6P0R5T7W9X","ts":"2026-10-07T09:14:03Z",
 "ctx":{"run_id":"run_01JB7Q2A","step_id":"scan","agent":"dep-checker",
        "tool_call_id":"toolu_01AbCdEf","origin":{"kind":"subprocess","name":"curl"}},
 "fidelity":"socket","method":"","scheme":"tcp","host":"203.0.113.7","port":443,"path":"",
 "peer_ip":"203.0.113.7","process":{"pid":4412,"name":"curl"},
 "local_addr":"192.0.2.10:53122","outcome":"ok","body_complete":false}
{"type":"socket_complete","id":"01JB7Q3V9F8M2K4N6P0R5T7W9X","duration_ms":812,
 "bytes_in":48211,"bytes_out":1290,"outcome":"ok"}
```

### Flow fields

| Field | Meaning |
|-------|---------|
| `id` | ULID of the flow |
| `ts` | When the record was written (UTC) |
| `ctx` | Attribution: `run_id`, `step_id`, `agent`, `workspace_id`, `tool_call_id` (socket flows only) and `origin`, each present when known. Which run a flow belongs to is decided by the ledger file it is in, not by `ctx.run_id` (HTTP flows leave it out) |
| `ctx.origin` | What opened the connection: `{"kind":"provider","name":"anthropic"}`, `{"kind":"scm","name":"github"}`, `{"kind":"subprocess","name":"curl"}` |
| `fidelity` | `http`, `coarse`, `socket` or `full` (see above) |
| `method`, `scheme`, `host`, `port`, `path` | The request. `path` never contains a query string |
| `peer_ip` | The address actually connected to; this is what ASN enrichment uses |
| `resolved_ips` | Every address DNS returned for the host |
| `process`, `local_addr` | Socket flows only: owning `pid` and `name`, and the local `ip:port` |
| `direction` | `outbound` or `inbound`. Defined in the schema, but no capture backend fills it in yet |
| `http_version`, `status` | When known |
| `outcome` | `ok`, `http_error`, `transport_error` or `timeout` |
| `error` | Error text for a failed flow, with URLs and query strings kept out |
| `bytes_out`, `bytes_in` | When observable. For a streamed response, `bytes_in` is filled in by the later `complete` line |
| `body_complete` | `false` while a streamed body is still being read. Socket flows are written with `false` and a `socket_complete` line does not change it |
| `ttfb_ms`, `duration_ms` | Time to first byte and total duration |

There is no TLS version, cipher or ASN on the record. The ASN is looked up when
the ledger is read (below), so a newer dataset improves old records too.

Flows observed during a run are also written to the run's transcript as
`net_flow` events, which the CLI's transcript views show inline (see
[transcript-schema.md](transcript-schema.md)).

---

## CLI

```sh
rupu netflow show run_01J...                     # one run's flows
rupu --format json netflow show run_01J...       # full records, plus dropped_total
rupu netflow prune --older-than 14d --dry-run    # preview
rupu netflow prune --older-than 14d              # delete
```

`rupu netflow show <run-id>` prints the run's flows (time, method, host, path,
status), merging its ledger with the `net_flow` events in its transcript, and
reports any dropped records. It finds the run in the global run store or in any
registered project.

`rupu netflow prune` deletes ledgers older than `--older-than` (default `30d`)
from the current project's `.rupu/netflow/` and the global `~/.rupu/netflow/`.
It goes by file modification time, never touches a file modified in the last
hour, and rejects a zero or negative cutoff. A ledger whose run is still live is
never deleted, however long it has been idle: an unfinished workflow run (paused
and awaiting-approval included, unless it is `running`/`pending` with a dead
recorded runner pid) keeps its own, its steps', its fan-out units' and its
sub-agents' ledgers, and a standalone `rupu run` whose process is alive keeps its
own and its sub-agents'. Those rows report `skipped_live`. `--dry-run` previews.
It supports `--format json|csv`.

Nothing deletes ledgers automatically.

---

## In the control plane

`rupu cp serve` shows flows in three places, all rendering the same **Network**
explorer at a different scope:

| Surface | Scope |
|---------|-------|
| Run → **Network** tab | The run's ledger plus those of its fan-out units and sub-agents. Opens on the run's own time span and shows whether subprocess capture was active |
| Project → **Network** tab | Every ledger in the project's `.rupu/netflow/`, plus the project's runs whose ledger fell back to the global directory. Opens on the last 24 hours |
| **Network** page (global) | Every registered project and the global directory. Opens on the last 24 hours |

There is no Network view on a workflow definition: a flow belongs to a run.

The explorer has:

- an **activity strip** across the whole retained range with a time-range
  picker; drag across it to zoom, and the rest of the page follows the window;
- **KPIs**: flows, endpoints, networks (distinct organisations), error rate,
  bytes and p95 latency. Byte totals are marked partial when any flow in view
  had an unknown count;
- a **topology** from workflows to origins (a provider, an SCM, or a subprocess
  such as `curl`) to the ASN organisations they reached, weighted by call
  count;
- a **timeline** with one lane per endpoint, grouped by organisation;
- **filters**: click a workflow, origin, organisation or host to filter every
  section; chips show what is applied;
- a **flow table**, sortable, with a detail panel per flow. Socket rows show
  the process (`curl (pid 4412)`), the transport and peer
  (`tcp → 203.0.113.7:443`), a dash for path and status, and a link to the
  `bash` call in the transcript.

A coverage note on every Network view says what netflow can and cannot see at
that scope, explains the fidelity badges, counts dropped records (even on an
empty table), and says when ASN data is unavailable, so an empty table or a
blank Network column is never mistaken for "no activity".

### The index

`rupu cp serve` keeps an index of every ledger, built when the server starts.
It detects changed files with a `stat` rather than re-reading them, reads only
newly appended lines, keeps a small summary of every file in memory, and keeps
compact flow rows up to `[netflow].cp_index_budget_mb`. Over the budget, rows
from the files with the oldest flows are dropped from memory and re-read when a
view needs them, so a wide window gets slower but the answer does not change.
Each section of a Network page loads on its own, and long tables are
virtualised.

### API

All read-only:

```
GET /api/runs/:id/netflow                 # a run's flows (?from=&to= to window)
GET /api/projects/:id/netflow             # a project's flows
GET /api/netflow                          # every flow
GET /api/netflow/explorer?scope=run:<id>|project:<id>   # the explorer's aggregates (no scope = global)
GET /api/netflow/graph                    # the topology graph
GET /api/netflow/index                    # index size, budget and evictions
```

---

## ASN enrichment

A peer IP is shown with its autonomous system and organisation (for example
`AS64500 Example Networks`). You don't need to run anything for this. rupu
downloads a combined IPv4 and IPv6 prefix-to-ASN table, compacts it into
`~/.rupu/netflow/asn.db`, and refreshes it:

- from `rupu cp serve`'s background loop, and
- whenever a netflow read finds the table missing or older than
  `asn_refresh_interval_days`, in the background (concurrent requests share
  one download).

The ASN is looked up when flows are read, never stored on the record, so a
table that arrives later improves every older flow, and an offline or
air-gapped install simply shows "ASN data not loaded" while everything else
works. A failed download leaves the existing table untouched, and a download
that parses to an empty table is refused. `coarse` flows have no peer IP and
so no ASN.

---

## Configuration

Capture needs no configuration: HTTP capture is always on, and subprocess
capture is on wherever the platform supports it. The `[netflow]` section of
`config.toml` tunes it. Unknown keys are rejected; any key left out takes its
default.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `subprocess_capture` | bool | `true` | Record connections opened by `bash` tool processes |
| `subprocess_poll_ms` | integer (ms) | `50` | Linux socket-table poll interval. Lower catches shorter connections at slightly more CPU. macOS is event-driven and ignores it |
| `subprocess_linger_ms` | integer (ms) | `3000` | How long a finished `bash` call stays attributable, so sockets that close just after the shell exits still count (capped at one day) |
| `cp_index_budget_mb` | integer (MiB) | `256` | Memory for the flow rows `cp serve`'s index keeps resident. Check usage with `GET /api/netflow/index`. A change saved through the control plane applies on the next request; a hand edit needs a restart |
| `asn_auto_refresh` | bool | `true` | Download and refresh the ASN table automatically. Set `false` on an air-gapped install |
| `asn_refresh_interval_days` | integer (days) | `7` | Age after which the table is refreshed. `0` means always refresh |
| `asn_source_url` | string | `https://iptoasn.com/data/ip2asn-combined.tsv.gz` | Where to download the table from, for example an internal mirror |

```toml
# ~/.rupu/config.toml — every [netflow] key at its default
[netflow]
subprocess_capture = true
subprocess_poll_ms = 50
subprocess_linger_ms = 3000
cp_index_budget_mb = 256
asn_auto_refresh = true
asn_refresh_interval_days = 7
asn_source_url = "https://iptoasn.com/data/ip2asn-combined.tsv.gz"
```

`RUPU_NETFLOW_SUBPROCESS=0` turns subprocess capture off for that process
whatever the config says (only the value `0` does). It is useful for switching
capture off on one host without editing config. The backend is chosen once per
process.

See [configuration.md](configuration.md) for how the global, customer and
project layers combine.

---

## Limits and what is not built yet

- **No full-fidelity capture.** The `full` fidelity level is reserved for
  frame-level capture in an isolated runtime, which would add hostnames and
  URLs for subprocess traffic, every packet of a UDP conversation, and DNS
  lookups. It is not built.
- **Non-HTTP egress** (`git2` clones, bucket traffic, the tunnel WebSocket) is
  not captured.
- **Remote hosts' flows** stay on the remote host.
- **GitHub connector flows are `coarse`.**
- **No enforcement.** Netflow records connections; it does not block them.
- **No automatic retention.** Use `rupu netflow prune`.

## See also

- [using-rupu.md](using-rupu.md) — day-to-day commands
- [configuration.md](configuration.md) — the full `config.toml` reference
- [transcript-schema.md](transcript-schema.md) — the `net_flow` transcript event
