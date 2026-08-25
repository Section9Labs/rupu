import Foundation
import Observation
import RupuAPI

/// Owns the Settings screen's config editor (Phase 6A, Task 4): the
/// resolved `view` (`GET /api/config[?project=]`, `RupuAPI.APIConfigView`)
/// plus the three raw-TOML write routes (`PUT /api/config/global`,
/// `PUT /api/config/project/:id`, `PUT /api/config/policy`).
///
/// **`client` is a per-call parameter, not a captured closure** — unlike
/// most stores in this module (`ProjectDetailStore`/`CoverageDetailStore`),
/// which capture fetch closures at `init` for a fake-closure test seam, this
/// store's produced surface (Task 4 brief) takes `client: CPClient`
/// directly on every method. Tests therefore drive it through a real
/// `CPClient` pointed at a stub `URLProtocol` — the same rig
/// `DashboardStoreTests` already uses (`DashboardStubURLProtocol`), just
/// with the client handed in per call instead of captured at `init`.
///
/// **Generation-guarded `load`** — same idiom `UsageStore`/`DashboardStore`
/// already establish: `load(client:project:)` bumps a counter and only
/// applies its result if that counter is still current when the fetch
/// resolves, so switching `selectedProject` mid-flight (a fast second
/// `load` landing before a slow first one) can never let the stale
/// response overwrite the fresh one.
///
/// **Read-only detection**: a 501 from ANY write route (`saveGlobalRaw`/
/// `saveProjectRaw`/`savePolicy`) means this `cp serve` has no `RunLauncher`
/// installed (`require_writable` in `crates/rupu-cp/src/api/config.rs`) —
/// there is no in-process way to persist an edit at all. `readOnly` latches
/// `true` the first time this happens and stays there for the store's
/// lifetime (a fresh `load` doesn't clear it — the deployment's writability
/// doesn't change between calls, and there's no signal that it did).
/// `saveError` is set to the server's own message text
/// (`"editing config requires \`rupu cp serve\`"`, hard-coded here to match
/// `require_writable`'s literal string rather than parsed out of the JSON
/// body — see `handleSaveError`'s doc comment).
///
/// **400 (validation failure)**: `saveError` carries the server's own TOML/
/// policy-lock validation message, unwrapped from the `{"error": "..."}`
/// JSON envelope every `ApiError` response body uses (final-review fix —
/// M2; `crates/rupu-cp/src/error.rs`'s `IntoResponse` builds that envelope
/// for EVERY status, 400 and 501 alike, so the previous claim that a 400
/// body is bare message text while only 501 wraps it was simply wrong, and
/// the inline error under the editor rendered literal JSON at the
/// operator). `readOnly` stays `false`; a 400 means the deployment CAN
/// write config, this particular edit just didn't validate.
///
/// **Every successful save re-`load`s** — the server is the source of
/// truth for `view` (resolved config, provenance, raw text); a save never
/// patches `view` locally. `saveGlobalRaw`/`saveProjectRaw`/`savePolicy`
/// all return `Bool`, `true` only when the write AND the following
/// re-`load` both succeeded (final-review fix — M3: they previously
/// returned `true` on the strength of the write alone, so a save whose
/// reload 500'd or timed out reported success to a caller that had just
/// been left staring at `view == .failed`). "The reload succeeded" is read
/// off `view` itself landing on `.content`, which also correctly returns
/// `false` when a concurrent `load` superseded this one's reload — the
/// value this call would have confirmed against is genuinely not the one
/// on screen.
@MainActor
@Observable
public final class ConfigStore {
    /// The fixed message a 501 write maps `saveError` to — see
    /// `handleSaveError`'s doc comment. Public so any UI surface that wants
    /// to explain a disabled Save BEFORE the operator has even attempted one
    /// (e.g. `RupuShell.ConfigSaveGate`) can quote the exact same string
    /// `saveError` would show after a real failed attempt, with nothing
    /// keeping the two wordings in lockstep except this one shared constant.
    /// `nonisolated` — a plain constant with no actor-isolated state behind
    /// it, and non-`@MainActor` call sites (e.g. `RupuShell.ConfigSaveGate`,
    /// itself a plain `enum` with no actor) need to read it from a static
    /// stored-property initializer, which runs in a nonisolated context.
    nonisolated public static let readOnlyMessage = "editing config requires `rupu cp serve`"

    public private(set) var view: BlockState<APIConfigView> = .loading
    /// `nil` = global-only view (no project scope selected). Set by
    /// `load(client:project:)`; every write route besides `saveProjectRaw`
    /// is independent of it, but a successful save always re-`load`s with
    /// this same value so the screen's current scope survives the refresh.
    public private(set) var selectedProject: String?
    /// Latches `true` the first time any write (or, in principle, a future
    /// read-side probe) returns 501 — see the type doc comment's
    /// "Read-only detection" section. Never reset back to `false`.
    public private(set) var readOnly = false
    /// The last save attempt's error, verbatim for a 400 (the server's TOML/
    /// policy validation message) or the fixed 501 message otherwise —
    /// cleared at the START of the next save attempt (any of the three
    /// methods), not by `load`.
    public private(set) var saveError: String?
    public private(set) var saving = false
    /// See the type doc comment's "Every successful save re-`load`s"
    /// section and `restartKeysIfChanged`'s doc comment for the heuristic
    /// this is computed from. Reset to `[]` at the start of every save
    /// attempt (any of the three methods) and populated only by a
    /// SUCCESSFUL `saveGlobalRaw` whose diff touched a restart-relevant
    /// line.
    public private(set) var lastSaveRestartKeys: [String] = []

    /// See the type doc comment's "Generation-guarded `load`" section.
    private var generation = 0

    public init() {}

    // MARK: - Load

    /// Fetches `GET /api/config[?project=]` and replaces `view` wholesale.
    /// Generation-guarded: if another `load` call starts before this one's
    /// fetch resolves, the stale response is dropped rather than clobbering
    /// whatever the newer call already landed (or is still loading).
    public func load(client: CPClient, project: String?) async {
        selectedProject = project
        generation += 1
        let currentGeneration = generation
        view = .loading
        do {
            let result = try await client.fetchConfig(project: project)
            guard currentGeneration == generation else { return }
            view = .content(result)
        } catch {
            guard !isCancellation(error) else { return }
            guard currentGeneration == generation else { return }
            view = .failed(String(describing: error))
        }
    }

    // MARK: - Save

    /// The message `saveProjectRaw` reports when it is called with no
    /// project scope selected — see that method's doc comment. Public for
    /// the same reason `readOnlyMessage` is: a UI surface (or a test)
    /// asserting this state should quote the one string, not a copy.
    nonisolated public static let noProjectScopeMessage =
        "no project scope selected — choose a project above before saving its layer"

    /// `PUT /api/config/global`. On success, computes `lastSaveRestartKeys`
    /// (see `restartKeysIfChanged`'s doc comment) from the pre-save
    /// `view.value?.rawGlobal` vs. the just-saved `raw`, then re-`load`s.
    /// Returns `true` only if BOTH the write and the following re-`load`
    /// succeeded (see the type doc comment's "Every successful save
    /// re-`load`s" section).
    public func saveGlobalRaw(_ raw: String, client: CPClient) async -> Bool {
        beginSave()
        let previousRawGlobal = view.value?.rawGlobal ?? ""
        let candidateKeys = view.value?.status.restartRequiredKeys ?? []
        do {
            try await client.putConfigGlobal(raw: raw)
            lastSaveRestartKeys = Self.restartKeysIfChanged(
                previousRaw: previousRawGlobal, newRaw: raw, candidateKeys: candidateKeys
            )
            await load(client: client, project: selectedProject)
            saving = false
            return reloadSucceeded
        } catch {
            handleSaveError(error)
            saving = false
            return false
        }
    }

    /// `PUT /api/config/project/:id` — requires `selectedProject` (set by a
    /// prior `load(client:project:)`); a `nil` `selectedProject` is a
    /// caller error (no project scope to write into) and this returns
    /// `false` without ever reaching the network.
    ///
    /// **That refusal is reported, not silent** (final-review fix — I3):
    /// the guard sets `saveError` to `noProjectScopeMessage` first. It used
    /// to `return false` with nothing set anywhere, so the Raw tab's
    /// Project-layer Save — reachable whenever `rawLayer` was left at
    /// `.project` after the scope picker returned to "Global only" — looked
    /// exactly like a successful save that simply hadn't refreshed. (The
    /// same fix also stops `ConfigTab` from getting into that state at all;
    /// this half is the honest failure for any other route into it.)
    public func saveProjectRaw(_ raw: String, client: CPClient) async -> Bool {
        guard let project = selectedProject else {
            saveError = Self.noProjectScopeMessage
            lastSaveRestartKeys = []
            return false
        }
        beginSave()
        do {
            try await client.putConfigProject(id: project, raw: raw)
            await load(client: client, project: selectedProject)
            saving = false
            return reloadSucceeded
        } catch {
            handleSaveError(error)
            saving = false
            return false
        }
    }

    /// `PUT /api/config/policy` — sets the GLOBAL `[policy].lock` enforced-
    /// key list. Independent of `selectedProject` (the route has no `:id`),
    /// but the following re-`load` still uses it so the screen's current
    /// scope survives the refresh.
    public func savePolicy(lock: [String], client: CPClient) async -> Bool {
        beginSave()
        do {
            try await client.putConfigPolicy(lock: lock)
            await load(client: client, project: selectedProject)
            saving = false
            return reloadSucceeded
        } catch {
            handleSaveError(error)
            saving = false
            return false
        }
    }

    /// Did the post-save re-`load` actually leave content on screen? Read
    /// directly off `view` rather than tracked separately, so it can never
    /// disagree with what the screen is rendering — including the
    /// generation-guard case, where a concurrent `load` superseded this
    /// one's reload and `view` reflects that other call instead.
    private var reloadSucceeded: Bool {
        if case .content = view { return true }
        return false
    }

    /// Shared save-attempt prelude: clears the previous attempt's
    /// `saveError`/`lastSaveRestartKeys` and marks `saving` — every save
    /// method calls this first, per `saveError`'s "cleared on next attempt"
    /// contract.
    private func beginSave() {
        saveError = nil
        lastSaveRestartKeys = []
        saving = true
    }

    /// Maps a failed write to `readOnly`/`saveError`. A 501 means this
    /// deployment has no `RunLauncher` installed at all (`require_writable`
    /// in `crates/rupu-cp/src/api/config.rs`) — `readOnly` latches `true`
    /// and `saveError` gets a fixed, readable message. This is hard-coded
    /// to the literal string `require_writable` actually sends (rather than
    /// parsed out of its `{"error": "..."}` JSON body) because the store
    /// has no reason to depend on the wire's exact envelope shape just to
    /// show a message it already knows verbatim.
    ///
    /// Any other non-2xx (chiefly 400 — a TOML parse/policy-lock
    /// validation failure) surfaces the server's own message in
    /// `saveError`, unwrapped from the `{"error": "..."}` envelope via
    /// `displayMessage(forBody:)`. `readOnly` is left untouched — a 400
    /// means the deployment CAN write config, this specific edit just
    /// didn't validate.
    ///
    /// A transport/decoding/unauthorized failure (not `.http`) falls back
    /// to `String(describing: error)`, same as every other store's failure
    /// path in this module.
    private func handleSaveError(_ error: Error) {
        guard !isCancellation(error) else { return }
        if case CPError.http(let status, let body) = error {
            if status == 501 {
                readOnly = true
                saveError = Self.readOnlyMessage
            } else {
                saveError = Self.displayMessage(forBody: body)
            }
            return
        }
        saveError = String(describing: error)
    }

    /// The operator-facing text inside a `cp` error response body.
    ///
    /// Every `ApiError` this app can provoke serializes as
    /// `{"error": "<message>"}` — `crates/rupu-cp/src/error.rs`'s
    /// `IntoResponse` builds that one envelope for every status it can
    /// produce, so there is no status for which the body is bare message
    /// text (final-review fix — M2: `saveError` used to be assigned the
    /// whole body for any non-501 status, and the inline error under the
    /// Raw/Policy editors rendered literal JSON braces and escaping at the
    /// operator).
    ///
    /// Falls back to the body verbatim whenever it doesn't parse as that
    /// exact shape — an unexpected body is still better shown raw than
    /// swallowed, and this keeps the store from asserting a wire contract
    /// it can't verify (e.g. a reverse proxy's own HTML error page).
    nonisolated static func displayMessage(forBody body: String) -> String {
        guard let data = body.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let message = object["error"] as? String
        else { return body }
        return message
    }

    /// **Heuristic restart-required banner (Task 4 brief, Step 2 ruling).**
    /// The wire's `put_global` response always reports `restart_required:
    /// []` (see `put_global` in `crates/rupu-cp/src/api/config.rs` — it's
    /// hard-coded, never derived from the edit that was just saved); the
    /// keys that ACTUALLY need a `cp serve` restart to take effect live in
    /// `APIRuntimeStatus.restartRequiredKeys` instead (currently always
    /// `["bind", "token"]`).
    ///
    /// This computes the banner client-side instead: split `previousRaw`
    /// and `newRaw` into lines, take the symmetric difference (every line
    /// that was added, removed, or changed), and check whether ANY of those
    /// changed lines contains one of `candidateKeys` as a plain substring.
    /// If so, this returns `candidateKeys` in full (the banner names every
    /// restart-relevant key, not just the one that matched); otherwise `[]`.
    ///
    /// **Deliberately a heuristic, not a guarantee** — the screen's banner
    /// text must say "may require restart", never "requires":
    /// - False positive: a comment or an unrelated string value that
    ///   happens to contain a candidate key's name (e.g. a log message
    ///   mentioning `"bind"`) on a changed line.
    /// - False negative: a value change that doesn't alter which line it's
    ///   on (unlikely for single-key TOML lines, but not ruled out by a
    ///   pure line-set diff).
    private static func restartKeysIfChanged(previousRaw: String, newRaw: String, candidateKeys: [String]) -> [String] {
        guard !candidateKeys.isEmpty, previousRaw != newRaw else { return [] }
        let previousLines = Set(previousRaw.split(separator: "\n", omittingEmptySubsequences: false))
        let newLines = Set(newRaw.split(separator: "\n", omittingEmptySubsequences: false))
        let changedLines = previousLines.symmetricDifference(newLines)
        let touchedRestartKey = changedLines.contains { line in
            candidateKeys.contains { key in line.contains(key) }
        }
        return touchedRestartKey ? candidateKeys : []
    }
}
