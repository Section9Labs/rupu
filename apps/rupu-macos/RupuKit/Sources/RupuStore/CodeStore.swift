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
/// **PER-BLOCK generation counters** (`treeGeneration`/`fileGeneration`) —
/// same "bump on entry, capture locally, drop a late-arriving result whose
/// captured value no longer matches" idiom `PaletteStore.openGeneration`
/// establishes, but deliberately split rather than shared. Review round 1
/// (fix round 1): an earlier revision of this store used ONE counter for
/// both `navigate(path:)` and `open(path:)`, which meant an `open()` that
/// superseded an in-flight `navigate()` bumped the SAME counter the
/// navigate's own guard checked — so the navigate's tree result, once it
/// eventually arrived, was always dropped as "stale," even though nothing
/// about it actually was: the tree fetch was still answering the CURRENT
/// `currentPath`. That stranded `tree` at `.loading` forever any time an
/// operator clicked a file while a directory listing was still in flight —
/// a real, pinned-by-test bug, not the "cross-kind protection" the shared
/// counter was intended to provide.
///
/// The fix keeps each block's own guard scoped to what can actually make
/// IT stale:
/// - `treeGeneration` is bumped ONLY by `navigate(path:)` — an `open(path:)`
///   never invalidates an in-flight tree fetch, so a file click while a
///   directory listing is loading lets that listing land normally once it
///   resolves.
/// - `fileGeneration` is bumped by BOTH `open(path:)` (a second file click)
///   AND `navigate(path:)` (a directory change still synchronously clears
///   `file`/`selectedPath` — see `navigate(path:)`'s doc comment — and must
///   also invalidate whatever `open(path:)` call was already in flight, so
///   that call's late-arriving result can never resurrect the file panel
///   after the operator has moved to a different directory).
///
/// **`filter` fetches once, not per-generation.** `GET .../files` returns
/// the whole project's file list regardless of `currentPath`, so unlike
/// `tree`/`file` there is no directory-scoped staleness for a generation
/// counter to guard against — the one-shot contract is presence-based
/// instead (`filter == nil` gates every call after the first); see
/// `loadFilter()`'s doc comment.
///
/// **Cancellation always leaves the store re-dispatchable.** Every
/// `isCancellation(error)` early-return below skips the `.failed(...)`
/// assignment but never leaves any internal latch that would block a LATER
/// call from trying again — `navigate(path:)`/`open(path:)` have no
/// "already requested" guard of their own (that lazy-load latch is the
/// SCREEN's concern — see `ProjectDetailScreen.codeRequested`'s doc
/// comment for the fix-round-1 change that makes ITS latch clear on a
/// cancelled dispatch too). A cancelled call here simply leaves `tree`/
/// `file` at whatever they were synchronously set to at the top of the
/// call (typically `.loading`) — a fresh `navigate`/`open` call re-sets
/// that state and proceeds exactly as if nothing had been in flight.
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

    /// See the type doc comment's "PER-BLOCK generation counters" section.
    private var treeGeneration = 0
    private var fileGeneration = 0

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
    /// navigable from — the OLD listing) and bumps `fileGeneration` (NOT
    /// just `treeGeneration`) so a still-in-flight `open(path:)` call from
    /// before this navigation can never resurrect `file` after the fact.
    /// See the type doc comment's "PER-BLOCK generation counters" section.
    public func navigate(path: String) async {
        treeGeneration += 1
        fileGeneration += 1
        let myTreeGeneration = treeGeneration
        currentPath = path
        tree = .loading
        file = nil
        selectedPath = nil
        do {
            let result = try await client.projectTree(wsID: wsID, path: path)
            guard myTreeGeneration == treeGeneration else { return }
            tree = .content(result)
        } catch {
            guard !isCancellation(error) else { return }
            guard myTreeGeneration == treeGeneration else { return }
            tree = .failed(String(describing: error))
        }
    }

    /// Loads `path` into the viewer (`file`) — a file row tap or the viewer
    /// Retry button both call this. Bumps ONLY `fileGeneration` — an
    /// `open(path:)` never invalidates an in-flight `navigate(path:)`, so a
    /// file click while a directory listing is still loading lets that
    /// listing land normally once it resolves (see the type doc comment's
    /// "PER-BLOCK generation counters" section for the bug this fixes).
    public func open(path: String) async {
        fileGeneration += 1
        let myFileGeneration = fileGeneration
        selectedPath = path
        file = .loading
        do {
            let content = try await client.projectFile(wsID: wsID, path: path)
            guard myFileGeneration == fileGeneration else { return }
            file = .content(content)
        } catch {
            guard !isCancellation(error) else { return }
            guard myFileGeneration == fileGeneration else { return }
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
