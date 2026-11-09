# Native desktop control-plane startup

The desktop app can supervise a foreground, authenticated local CP without
putting a bearer token in argv, a startup URL, or a log. Existing `cp serve`
behavior is unchanged unless the three managed-launch flags are supplied.

```text
rupu cp serve --bind 127.0.0.1:0 --no-open \
  --token-file /absolute/private/launch/token \
  --ready-file /absolute/private/launch/ready.json \
  --launch-id 352b0b5b-8851-4c1c-9c91-5c347bdc2f18
```

The native client owns the child process, its environment, and a new launch
directory. It supplies an explicitly selected `RUPU_HOME`; the CLI does not
adopt another home, daemon, or existing process. No `--writable` flag is needed:
the normal CLI CP installs the API adapters for sessions and other operations.
`--enable-session-tool-approvals` is a separate opt-in with its own authenticated
readiness contract.

## Private launch inputs

All three flags are required together. `--token-file` conflicts with `--token`.
Managed startup accepts only the literal IPv4 loopback address `127.0.0.1`,
including port zero for an OS-assigned port. Startup never opens a browser or
prints access-bearing links or MCP credential examples in managed mode.

Both paths must be absolute, with no symlink or parent traversal component.
The client should canonicalize the parent before constructing them; for example,
use `/private/tmp`, not the macOS `/tmp` symlink. Each immediate parent must
already exist, belong to the effective user, and have mode `0700`.

The token must be a regular file owned by that user, mode `0600`, with one hard
link. Its entire content is at most 8192 bytes, containing at least 16 ASCII
bearer-token characters (`A-Z`, `a-z`, `0-9`, `-._~+/=`), optionally followed by
CR or LF. The client generates a fresh unpredictable token; validation of its
format does not establish its entropy. The CLI leaves this file intact so the
client can unlink it after validating the handoff.

The readiness path must not exist, including as a symlink. Publication uses a
private completed staging file and an atomic no-replace link in the same
directory. A race that creates the destination causes startup failure, not
replacement. Keep the launch directory private for the lifetime of startup.

## Receipt and authenticated readiness

After binding the socket, the CLI publishes a mode-`0600` JSON receipt:

```json
{
  "schema": "rupu.cp.bootstrap.v1",
  "launch_id": "352b0b5b-8851-4c1c-9c91-5c347bdc2f18",
  "endpoint": "http://127.0.0.1:49152",
  "pid": 1234,
  "version": "0.88.0"
}
```

The sample PID and port are illustrative. `version` is the compiled Cargo
package version, matching `/healthz`'s `x-rupu-version` header and
`GET /api/host/info`. It is not a published-release claim or a source hash.
The app's separate signed runtime manifest pins its source and binary identity.

**The receipt means that this launch bound a socket, not that HTTP is ready.**
Background service setup and router initialization still follow. The client
must compare the launch ID, its owned child PID, the expected runtime version,
and the exact literal-loopback endpoint, then wait for `/healthz` and an
authenticated `GET /api/host/info` using its token. The latter also advertises
capabilities; health alone proves neither authentication nor feature support.
Managed `/healthz` also returns `x-rupu-launch-id` with this launch's UUID. The
client must compare that live identity to its receipt and owned launch record,
including on recovery; a PID and executable path alone can be reused. Ordinary
CP health responses do not carry this header. The launch ID is not a secret or
a substitute for authenticated readiness.
`/api/config` reports the assigned address rather than the requested port zero.

The receipt contains no token, credentials, project data, or home path.
An existing or invalid receipt is never a reason to stop a process. On normal
Rust scope exit, cleanup removes only the receipt inode this launch published;
abrupt termination can leave a stale file. Native lifecycle ownership and
liveness checks remain mandatory. This contract adds no process adoption or
stop-other-CP mechanism and does not promise to terminate detached run workers.

## Fresh-home resources and packaging

The `rupu-session` agent is embedded in the runtime. The source-owned
`skills/rupu-session-operations` directory is packaged separately and can be
installed through the existing installer into the explicitly selected home:

```text
rupu skill install /absolute/bundled/rupu-session-operations --skill rupu-session-operations
```

Omit `--project` and `--force`. Reuse or report an existing installation rather
than overwriting edited instructions. Serialize installation into the app-owned
home. The skill installer's existing no-force checks are not an atomic
no-replace guarantee against an external writer creating an empty destination
between its check and final rename; this launch protocol does not change that
installer. Verify the packaged source against its manifest and report the
post-install catalog's actual availability, not an unsupported claim that an
existing customized skill exactly matches the shipped version.

The full skill preset may report missing
optional members; packaging this one skill does not claim the external suite is
installed. No user-global provider, project, session, or skill state belongs in
the distributable app.

The app must sign the runtime as nested executable code before signing the
outer bundle and include the licenses for the resources it distributes. The
desktop installation owner is described in [desktop-runtime-ownership.md](desktop-runtime-ownership.md).
