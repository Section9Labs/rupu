# SCM enrollment in a running control plane

The CP no longer treats its startup credential registry as permanent. A new
operator request captures a registry built from the instance's current SCM
configuration and credential store. Repository lists, issue accounts and pages,
review continuations, and reviewed write preflight share this boundary. A new
login, logout, credential replacement, account declaration, or account-routing
edit applies without restarting the CP. Existing requests and runs retain their
captured objects; this does not cancel or restart work.

The cache compares effective SCM enrollment, including SSO/API-key precedence,
legacy keys and the selected account's resolver vendor. Unrelated model-provider
credential writes and non-SCM preferences preserve the exact adapter identity.
The native client's existing review, issue and write receipts therefore remain
valid while their authority is unchanged. A changed SCM credential/configuration
produces new connector epochs; an old prepared write must be prepared again.
No provider mutation is automatically retried by this refresh mechanism.

Credential material used for comparison remains private in memory. It has no
debug, serialization or byte/string interface, and no token-derived digest is
logged or returned. The publication guard also compares nonsecret file identity
around construction, so A-to-B-to-A credential publications cannot label mixed
connectors as one coherent snapshot. Discovery is serialized, bounded and
published only when complete. Missing enrollment is a valid empty result;
unreadable, malformed, oversized or failing enrolled credentials are errors.
Unix config/auth opens are nonblocking and require regular files before reading.

MCP catalog advertisement is independent of each call's current service
availability. The same running catalog can use newly enrolled accounts. A failed
refresh omits the SCM service from the call context, yielding an explicit tool
error while other services can remain available; it never substitutes a stale
authenticated registry or reports the failed discovery as a successful empty
repository list. Each registered project's MCP context loads its own layered
configuration. Inventory captures one registry for a sweep's repository and
issue counts; its existing scheduled refresh interval is unchanged.

The source regression fixtures use disposable credential stores, actual auth
store writes and the actual in-process CP routes. Provider HTTP responses are
served only by an owned local fixture. They cover enrollment/removal through one
unchanged router, stable receipts after unrelated provider changes, old-write
refusal after rotation, project/instance isolation, SSO precedence, GitLab,
concurrent capture, transient discovery failure, publication ABA, unsafe file
types, input bounds and invalid URLs. They do not launch a CP process or use
operator credentials. The separate native/packaged acceptance lane must still
verify real login and rendering against its exact released runtime.
