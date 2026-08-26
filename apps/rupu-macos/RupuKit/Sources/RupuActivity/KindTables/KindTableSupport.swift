import SwiftUI
import RupuStore
import RupuDesign

/// Column widths shared by more than one per-kind table (perf & interaction
/// arc, Plan 5 Task 4 — the old merged `ActivityTable`'s own
/// `ActivityTableLayout`, `RupuActivity/ActivityTable.swift`, was deleted
/// along with it once the `.all` parent stopped showing a table at all; see
/// `ActivityScreen`'s doc comment). Each kind table's OWN file still owns a
/// private per-table layout enum for its kind-specific columns (Source/
/// Trigger/Event/Model/Issue Ref/Worker/etc.) — this is only the handful of
/// columns every run-shaped kind table repeats verbatim (Status/Run/Host/
/// token triplet/Cost/Turns/Duration/Started/Actions).
enum KindTableLayout {
    static let status: CGFloat = 100
    static let run: CGFloat = 72
    static let host: CGFloat = 88
    static let tokenCell: CGFloat = 56
    static let cost: CGFloat = 60
    static let turns: CGFloat = 44
    static let duration: CGFloat = 64
    static let started: CGFloat = 88
    /// Same reserved-but-usually-empty column `ActivityTableLayout.actions`
    /// already establishes.
    static let actions: CGFloat = 52
}

/// One relative-time formatter shared by every per-kind table's Started/
/// Duration cells — `ActivityTable`/`ClaimTableRow` each built their own
/// identical instance privately; four more per-kind tables duplicating it a
/// fourth-through-seventh time isn't a new precedent worth repeating.
@MainActor
enum KindTableFormat {
    static let relative: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()

    /// `Fmt.count` takes `Int?`; every per-kind token/turn field on
    /// `ActivityRow` is `UInt64?` — this is the one narrowing conversion
    /// site every table's token/turns cell goes through, so a value that
    /// somehow exceeds `Int.max` (never happens in practice — real usage
    /// counts are nowhere near that range) fails safe to `nil` (renders
    /// `—`) rather than trapping.
    static func count(_ n: UInt64?) -> String {
        Fmt.count(n.flatMap { Int(exactly: $0) })
    }

    /// First 8 characters of an id — the short-id convention
    /// `EventStreamColumn.swift` already uses for run-id buttons, reused
    /// here for the Run/Session identity columns.
    static func shortID(_ id: String) -> String {
        String(id.prefix(8))
    }
}

/// Compact ✓/✕ pair for an `.awaiting` row whose navigation is `.run(id:
/// host:)` — the same shape the old `ActivityTableRow.awaitingActions`
/// (`RupuActivity/ActivityTable.swift`, deleted this task once the `.all`
/// parent stopped rendering a merged table at all — see `ActivityScreen`'s
/// doc comment) used to render for every kind at once. Shared here by the
/// three run-shaped per-kind tables (Agent/Workflow/Autoflow — Sessions
/// never carries `.awaiting`) rather than a third duplicate of the same
/// resolve-then-post round trip — same "shared resolution, per-caller
/// chrome" precedent `NeedsYouGateActions`'s doc comment already established
/// for a different pair of call sites (that one keeps its OWN chrome
/// deliberately; this one is plain enough to share outright).
struct KindTableAwaitingActions: View {
    let runID: String
    let host: String?
    let store: ActivityStore
    let backend: BackendController

    @State private var isBusy = false

    var body: some View {
        HStack(spacing: 6) {
            Button {
                Task { await resolveGateAndApprove() }
            } label: {
                if isBusy {
                    ProgressView().controlSize(.mini)
                } else {
                    Icon(.checkCircle2, size: 14)
                }
            }
            .buttonStyle(.plain)
            .foregroundStyle(Color.status(.done))
            .disabled(isBusy)
            .help("Approve")

            Button {
                Task { await resolveGateAndReject() }
            } label: {
                if isBusy {
                    ProgressView().controlSize(.mini)
                } else {
                    Icon(.xCircle, size: 14)
                }
            }
            .buttonStyle(.plain)
            .foregroundStyle(Color.status(.failed))
            .disabled(isBusy)
            .help("Reject")
        }
    }

    private func resolveGateAndApprove() async {
        isBusy = true
        defer { isBusy = false }
        guard let client = backend.client() else { return }
        guard let gate = await ActivityStore.resolveSoleAwaitingGate(client: client, runID: runID, host: host) else { return }
        await store.approve(runID: runID, gate: gate, host: host)
    }

    private func resolveGateAndReject() async {
        isBusy = true
        defer { isBusy = false }
        guard let client = backend.client() else { return }
        guard let gate = await ActivityStore.resolveSoleAwaitingGate(client: client, runID: runID, host: host) else { return }
        await store.reject(runID: runID, gate: gate, host: host)
    }
}

/// Reusable trailing-actions cell: renders `KindTableAwaitingActions` only
/// when `row.status == .awaiting` AND its navigation actually resolves to a
/// run id (an autoflow event that's `.awaiting` with no materialized run yet
/// has nothing to approve/reject against — same guard `ActivityTableRow`
/// applies) — otherwise empty, but always width-reserved by the caller so
/// every row in a table stays column-aligned.
@ViewBuilder
func kindTableAwaitingActionsCell(_ row: ActivityRow, store: ActivityStore, backend: BackendController) -> some View {
    if row.status == .awaiting, case .run(let runID, let host) = row.navigation {
        KindTableAwaitingActions(runID: runID, host: host, store: store, backend: backend)
    }
}

/// Status dot + label — identical presentation to `ActivityTableRow.
/// statusCell`, reused verbatim by every per-kind table's Status column.
struct KindTableStatusCell: View {
    let status: ActivityStatus

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(Color.status(status.tone))
                .frame(width: 6, height: 6)
            Text(status.displayLabel)
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
        }
    }
}

// MARK: - Infinite scroll + Find (perf & interaction arc, Plan 5 Task 5)

/// The compact Find text field every per-kind table renders above its list —
/// client-side substring search over already-loaded rows (never a server
/// round trip; the fields matched against are per-kind, see each table's own
/// `matches(_:query:)`). Bound directly to the RAW query — debouncing into a
/// second `debouncedQuery` (what the table actually filters by) is the
/// caller's job via `.debouncedKindTableSearch(query:into:)` below, so the
/// text field itself always feels instant to type into even though the
/// filter it drives lags slightly behind.
struct KindTableSearchField: View {
    let placeholder: String
    @Binding var query: String

    var body: some View {
        HStack(spacing: 6) {
            Icon(.search, size: 12)
                .foregroundStyle(Color.rupuMute)
            TextField(placeholder, text: $query)
                .textFieldStyle(.plain)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
            if !query.isEmpty {
                Button {
                    query = ""
                } label: {
                    Icon(.xCircle, size: 12)
                        .foregroundStyle(Color.rupuMute)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(Color.rupuPanel)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.rupuBorder, lineWidth: 1))
        .frame(maxWidth: 220)
    }
}

/// Debounces `query` into `debouncedQuery` after a short pause in typing —
/// `.task(id: query)` cancels its own prior debounce automatically whenever
/// `query` changes again before the delay elapses (SwiftUI's standard
/// `.task(id:)` contract), so this needs no manual timer bookkeeping. The
/// filter itself is a cheap in-memory `Array.filter` over already-loaded
/// rows — this debounce exists to avoid re-filtering/re-rendering the whole
/// visible list on every keystroke of a fast typist, not because the filter
/// is expensive.
private struct KindTableSearchDebounce: ViewModifier {
    let query: String
    @Binding var debouncedQuery: String

    func body(content: Content) -> some View {
        content.task(id: query) {
            try? await Task.sleep(for: .milliseconds(200))
            guard !Task.isCancelled else { return }
            debouncedQuery = query
        }
    }
}

extension View {
    func debouncedKindTableSearch(query: String, into debouncedQuery: Binding<String>) -> some View {
        modifier(KindTableSearchDebounce(query: query, debouncedQuery: debouncedQuery))
    }
}

/// The trailing sentinel/footer row every per-kind table (+ the Cycles
/// sub-table) renders as its list's last row — ports the web's four honesty
/// states verbatim (`pages/runs/*.tsx`, `Sessions.tsx`,
/// `ProjectRunsTab.tsx`/`ProjectSessionsTab.tsx`):
///   - a client-side Find query is active → "{n} matches of {m} loaded"
///     (never competes with the loading/scroll states below — a narrowed
///     view doesn't need "scroll for more" to keep making sense of the same
///     honesty contract for a narrower slice; matches the web precedent,
///     which also short-circuits to this state first)
///   - otherwise: "loading more…" while a `loadMore()` fetch is in flight,
///     "scroll for more" while more pages exist and none is in flight, or
///     "— end of {m} —" once the source is exhausted.
///
/// `onAppear` fires `loadMore()` (guarded by `hasMore && !isLoadingMore`) —
/// this row IS the scroll sentinel, exactly like the web's `<div
/// ref={sentinelRef}>` doubles as its own footer text; a `List` naturally
/// re-triggers `.onAppear` each time this row scrolls back into view (or is
/// visible from the start on a short list), which is what drives paging.
struct KindTableFooter: View {
    let visibleCount: Int
    let loadedCount: Int
    let hasMore: Bool
    let isLoadingMore: Bool
    let isSearchActive: Bool
    let onLoadMore: () -> Void

    var body: some View {
        Text(label)
            .font(.noteText)
            .foregroundStyle(Color.rupuMute)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 10)
            .onAppear {
                guard hasMore, !isLoadingMore else { return }
                onLoadMore()
            }
    }

    private var label: String {
        if isSearchActive {
            return "\(visibleCount) matches of \(loadedCount) loaded"
        }
        if isLoadingMore {
            return "loading more…"
        }
        if hasMore {
            return "scroll for more"
        }
        return "— end of \(loadedCount) —"
    }
}

// MARK: - Custom date range (perf & interaction arc, Plan 5 Task 5)

/// One optional date bound ("From" or "To") — a checkbox that toggles
/// whether this bound is set at all, revealing a compact native `DatePicker`
/// once it is. Plain SwiftUI has no first-class "optional date" control, and
/// a bound that's merely blank-until-touched (rather than explicitly
/// enabled/disabled) would leave no way to CLEAR it back to "no bound" once
/// set — the checkbox is what makes "no filter" a reachable state again.
private struct KindTableDateBound: View {
    let label: String
    let date: Date?
    let onChange: (Date?) -> Void

    var body: some View {
        HStack(spacing: 4) {
            Toggle(isOn: Binding(
                get: { date != nil },
                set: { isOn in onChange(isOn ? (date ?? Date()) : nil) }
            )) {
                Text(label)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuDim)
            }
            .toggleStyle(.checkbox)
            .controlSize(.small)

            if let date {
                DatePicker(
                    "",
                    selection: Binding(get: { date }, set: onChange),
                    displayedComponents: .date
                )
                .datePickerStyle(.field)
                .labelsHidden()
                .frame(width: 108)
            }
        }
    }
}

/// The "From"/"To" custom date-range filter every per-kind table renders in
/// its `FilterBar` — server-side (unlike the status chips alongside it):
/// picking a date reaches `onChange`, which the caller wires to
/// `ActivityStore.setDateRange(since:until:)`/`CyclesStore.setDateRange(
/// since:until:)`, resetting paging back to page 0 against the server's
/// narrowed result set (see those methods' own doc comments).
///
/// **Day-boundary normalization happens here, not in the store**: a
/// `DatePicker(displayedComponents: .date)` always yields midnight of the
/// selected day. Passed straight through, a closed-range `since <= ts <=
/// until` server-side filter (see `pagination::DateRangeQuery`'s Rust doc
/// comment) would make "To Aug 26" exclude nearly all of Aug 26 itself — so
/// "From" is sent as the start of that day (a no-op normalization, but
/// explicit) and "To" as the LAST instant of that day, matching the web's
/// own `windowFromDayRange` convention (`23:59:59.999`) for the same
/// whole-day-inclusive semantics.
struct KindTableDateRangeFilter: View {
    let since: Date?
    let until: Date?
    let onChange: (_ since: Date?, _ until: Date?) -> Void

    var body: some View {
        HStack(spacing: 12) {
            KindTableDateBound(label: "From", date: since) { newSince in
                onChange(newSince.map(Self.startOfDay), until)
            }
            KindTableDateBound(label: "To", date: until) { newUntil in
                onChange(since, newUntil.map(Self.endOfDay))
            }
        }
    }

    private static func startOfDay(_ date: Date) -> Date {
        Calendar.current.startOfDay(for: date)
    }

    private static func endOfDay(_ date: Date) -> Date {
        let start = Calendar.current.startOfDay(for: date)
        return start.addingTimeInterval(86_400 - 0.001)
    }
}
