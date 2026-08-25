import Foundation
import Observation
import RupuAPI

/// Owns the Project Detail screen's Code tab (Phase 6B, Task 4): a
/// workspace-relative directory browser (`GET /api/projects/:ws_id/tree`),
/// a read-only file viewer (`GET /api/projects/:ws_id/source`), and a
/// client-side filename filter fed by the whole-project file list
/// (`GET /api/projects/:ws_id/files`).
///
/// **Viewer only — no editing affordances.** This store and `CodeTab`
/// together only ever GET; there is no write path here (no server-side
/// counterpart exists for `/source` either) — the umbrella spec's project
/// code surface is read-only by design.
///
/// **One `generation` counter guards BOTH `navigate(path:)` and
/// `open(path:)`** — same "bump on entry, capture locally, drop a
/// late-arriving result whose captured value no longer matches" idiom
/// `PaletteStore.openGeneration` establishes. Sharing a SINGLE counter
/// across both operations (rather than one each) is deliberate: the brief
/// calls for "rapid dir clicks must not interleave — single generation
/// covers tree+file" — a `navigate` that starts while an earlier `open` is
/// still in flight must be able to invalidate that `open`'s eventual
/// result (a file click followed by a fast dir click shouldn't let the
/// file's late response land after the operator has already moved on), and
/// symmetrically an `open` must be able to invalidate an in-flight
/// `navigate`. Two independent counters would only protect each operation
/// against a same-kind race, not a cross-kind one.
///
/// **`filter` fetches once, not per-generation.** `GET .../files` returns
/// the whole project's file list regardless of `currentPath`, so unlike
/// `tree`/`file` there is no directory-scoped staleness for a generation
/// counter to guard against — the one-shot contract is presence-based
/// instead (`filter == nil` gates every call after the first); see
/// `loadFilter()`'s doc comment.
@MainActor
@Observable
public final class CodeStore {
    /// The current directory's listing. Never `.empty` — an empty
    /// directory still carries meaningful `parent`/`path` metadata for the
    /// `..` row, so `CodeTab` renders "no entries" inside the `.content`
    /// case rather than losing that metadata to a separate `.empty` state.
    public private(set) var tree: BlockState<APITreeResult> = .loading

    /// The selected file's content — `nil` means "nothing selected yet"
    /// (distinct from `.loading`, which means a selection IS in flight).
    public private(set) var file: BlockState<APIFileContent>?

    /// The whole-project file list backing the filter field — `nil` means
    /// "never requested" (the filter field is empty and has never been
    /// typed into); see `loadFilter()`.
    public private(set) var filter: BlockState<APIFileList>?

    /// The workspace-relative directory `tree` currently reflects (or is
    /// loading/failed for) — `CodeTab` reads this to build the `..` row's
    /// target and to re-drive a tree Retry.
    public private(set) var currentPath: String

    /// The workspace-relative file `file` currently reflects (or is
    /// loading/failed for) — `nil` exactly when `file == nil`.
    public private(set) var selectedPath: String?

    private let wsID: String
    private let client: CPClient
    private let rootPath: String

    /// See the type doc comment's "One `generation` counter" section.
    private var generation = 0

    public init(wsID: String, client: CPClient, rootPath: String = "") {
        self.wsID = wsID
        self.client = client
        self.rootPath = rootPath
        self.currentPath = rootPath
    }

    /// Loads the workspace root's listing — `ProjectDetailScreen` calls this
    /// the first time the Code tab is selected (same lazy-tab discipline
    /// every other `ProjectDetailStore` tab follows), and it's safe to call
    /// again on a re-appearance (repeatable, like every other store's
    /// `activate()` in this codebase).
    public func activate() async {
        await navigate(path: rootPath)
    }

    /// Loads `path`'s immediate children into `tree` — a dir row tap, the
    /// `..` row, or the tree Retry button all call this the same way.
    /// Clears `file`/`selectedPath` (a directory change invalidates
    /// whatever was previously selected in — and only meaningfully
    /// navigable from — the OLD listing). See the type doc comment for the
    /// shared-generation contract with `open(path:)`.
    public func navigate(path: String) async {
        generation += 1
        let myGeneration = generation
        currentPath = path
        tree = .loading
        file = nil
        selectedPath = nil
        do {
            let result = try await client.projectTree(wsID: wsID, path: path)
            guard myGeneration == generation else { return }
            tree = .content(result)
        } catch {
            guard !isCancellation(error) else { return }
            guard myGeneration == generation else { return }
            tree = .failed(String(describing: error))
        }
    }

    /// Loads `path` into the viewer (`file`) — a file row tap or the viewer
    /// Retry button both call this. See the type doc comment for the
    /// shared-generation contract with `navigate(path:)`.
    public func open(path: String) async {
        generation += 1
        let myGeneration = generation
        selectedPath = path
        file = .loading
        do {
            let content = try await client.projectFile(wsID: wsID, path: path)
            guard myGeneration == generation else { return }
            file = .content(content)
        } catch {
            guard !isCancellation(error) else { return }
            guard myGeneration == generation else { return }
            file = .failed(String(describing: error))
        }
    }

    /// Fetches the whole-project file list into `filter` — only on the
    /// FIRST call (`filter == nil` gates every later call, success or
    /// failure alike), matching the brief's "fetched once on first filter
    /// keystroke" contract: `CodeTab` calls this from the filter field's
    /// `onChange`/`.task`, and every keystroke after the first is a
    /// no-op re-fetch-wise (the client-side contains-match then re-filters
    /// the already-loaded `filter.value?.files` on every keystroke instead).
    /// `CodeTab`'s filter-failure Retry calls `reloadFilter()` instead,
    /// which bypasses this guard.
    public func loadFilter() async {
        guard filter == nil else { return }
        await reloadFilter()
    }

    /// Unconditional filter fetch — the Retry path for a `.failed` filter.
    /// `loadFilter()`'s one-shot guard would otherwise never let a failed
    /// fetch be retried, since `filter` is already non-nil once it reaches
    /// `.failed`.
    public func reloadFilter() async {
        filter = .loading
        do {
            let result = try await client.projectFiles(wsID: wsID)
            filter = result.files.isEmpty ? .empty : .content(result)
        } catch {
            guard !isCancellation(error) else { return }
            filter = .failed(String(describing: error))
        }
    }
}
