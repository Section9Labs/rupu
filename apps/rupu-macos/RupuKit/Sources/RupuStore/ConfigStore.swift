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
/// `CPClient` pointed at a stub `URLProtocol`, same rig
/// `DashboardStore`/`ActivityStore`/`LauncherStore`'s tests already use for
/// their own per-call-`client` methods.
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
/// body — see `handleSaveError`'s doc comment) rather than the raw
/// `{"error": ...}` body a 400 surfaces verbatim.
///
/// **400 (validation failure)**: `saveError` carries the response body
/// verbatim — it's the server's own TOML/policy-lock validation error,
/// meant to be shown to the operator as-is. `readOnly` stays `false`; a 400
/// means the deployment CAN write config, this particular edit just didn't
/// validate.
///
/// **Every successful save re-`load`s** — the server is the source of
/// truth for `view` (resolved config, provenance, raw text); a save never
/// patches `view` locally. `saveGlobalRaw`/`saveProjectRaw`/`savePolicy`
/// all return `Bool` (`true` on a save that both wrote AND the following
/// re-`load` succeeded) so the screen can drive a "saved" confirmation off
/// one call.
@MainActor
@Observable
public final class ConfigStore {
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

    /// `PUT /api/config/global`. On success, computes `lastSaveRestartKeys`
    /// (see `restartKeysIfChanged`'s doc comment) from the pre-save
    /// `view.value?.rawGlobal` vs. the just-saved `raw`, then re-`load`s.
    /// Returns `true` only if BOTH the write and the following re-`load`
    /// succeeded.
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
            return true
        } catch {
            handleSaveError(error)
            saving = false
            return false
        }
    }

    /// `PUT /api/config/project/:id` — requires `selectedProject` (set by a
    /// prior `load(client:project:)`); a `nil` `selectedProject` is a
    /// caller error (no project scope to write into) and this is a no-op
    /// returning `false` without ever reaching the network.
    public func saveProjectRaw(_ raw: String, client: CPClient) async -> Bool {
        guard let project = selectedProject else { return false }
        beginSave()
        do {
            try await client.putConfigProject(id: project, raw: raw)
            await load(client: client, project: selectedProject)
            saving = false
            return true
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
            return true
        } catch {
            handleSaveError(error)
            saving = false
            return false
        }
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
    /// validation failure) surfaces its body VERBATIM in `saveError`; the
    /// server's 400 body IS the message meant for the operator to read
    /// (the TOML validation error), unlike 501's generic JSON envelope.
    /// `readOnly` is left untouched — a 400 means the deployment CAN write
    /// config, this specific edit just didn't validate.
    ///
    /// A transport/decoding/unauthorized failure (not `.http`) falls back
    /// to `String(describing: error)`, same as every other store's failure
    /// path in this module.
    private func handleSaveError(_ error: Error) {
        guard !isCancellation(error) else { return }
        if case CPError.http(let status, let body) = error {
            if status == 501 {
                readOnly = true
                saveError = "editing config requires `rupu cp serve`"
            } else {
                saveError = body
            }
            return
        }
        saveError = String(describing: error)
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
