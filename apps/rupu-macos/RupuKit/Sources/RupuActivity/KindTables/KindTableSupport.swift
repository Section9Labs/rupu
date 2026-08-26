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
