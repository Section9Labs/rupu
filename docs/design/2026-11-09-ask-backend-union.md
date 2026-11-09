# Ask approval backend: additive integration

This source-only delta composes reviewed Ask input `50da22c31a055fe29265fb728811706c63182ab1f0ec79f07376a46c79e1d708` onto applied backend commit `9f9afd6d98e6747b80fd5d04ef6d03827bca1a39` (tree `01b86fbf73077d7e10438f462213c2f1016d56c2`). Prior backend, scoped-read and dependency exports remain unchanged in their frozen checkouts.

## Shared-source decisions

- Runtime assembly and ToolServices carry both the exact scoped-reader service and the private approval authorizer. Scoped readers retain root SessionTurn/parent restrictions; Ask in-process children retain their separately checked approval owner. Unsupported Ask process children still refuse.
- The worker retains exact per-binding reader selection from creation markers/current settings and the pre/post MCP approval-authority checks. SessionRecord remains byte-identical to the reviewed scoped-read union: optional markers, version6 write floor, legacy exact opt-in, frozen declarations and retention leases are preserved.
- SessionStarter exposes independent scoped-read and approval support methods. Host information keeps every existing capability gate and adds the Unix authenticated/broker-ready approval gate independently from retention. CP defaults remain unchanged; `--enable-session-tool-approvals` is default-off and requires authenticated CP plus a working broker.
- Both scoped-reader and approval test suffixes are retained. The combined Ask preparation fixture additionally checks version6/default provenance and verifies that an explicit empty tool selection remains empty despite a private Ask channel. These are authored assertions, not execution evidence.
- All existing operator inventory names/exclusions are retained. The four Ask approval HTTP routes remain excluded from model/operator tool catalog exposure; no tool count changes.
- Runtime rustix already exists from retention. The merged manifest adds hmac once and retains the earlier duplicate-dev-dependency cleanup. The frozen Ask export omitted its new direct hmac dependency from Cargo.lock; this integration adds only the already-resolved hmac membership to rupu-runtime, without package/version changes. Workspace rustix adds the reviewed stdio feature for descriptor handoff.

Thirty-eight of55 original Ask input files remain byte-identical. The17 shared files preserve current backend/scoped-read content. The extra lockfile member and this integration note are the only new paths beyond that55-file input. No approval or scoped-read source file is replaced wholesale by an older input.

## Validation handoff

The Ask input authors48 test declarations, including one ignored scalability fixture; none ran here. Scoped-read22 and prior backend115 fixtures retain their separate provenance. Focused additional filters: `rupu-runtime --lib session_approvals`, `rupu-agent --lib approval`, `rupu-tools --lib approval`, `rupu-tools --lib permission`, `rupu-transcript --lib approval`, `rupu-cli --lib session_harness_tests`, `rupu-cli --test it argv_roundtrip`, `rupu-cp --lib session_approvals`, and `rupu-cp --test it mcp_routes`. Repeat the relevant scoped reader/assembly tests in the combined candidate. Rustfmt source parsing is not typechecking.

No builds, test execution, APIs, providers, services, actual approval journals or warm checkout were used in this lane. Original provider, lifecycle, environment-credential revocation and non-atomic external-config qualifications remain. Native Ask UI, authenticated disposable acceptance and any feature enablement remain separate root-owned decisions.
