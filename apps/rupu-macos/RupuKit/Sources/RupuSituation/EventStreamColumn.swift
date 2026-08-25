import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

/// Which chip narrows the stream. `.all` plus one chip per `CardGroup` case
/// — matches `EventStream.tsx`'s five filters (lines 14-20: All / Findings /
/// Agent activity / Awaiting / Errors) exactly. The task-7 brief's own
/// paraphrase ("All/Findings/Needs you/Errors") drops "Agent activity" and
/// renames "Awaiting" to "Needs you" — the verified web source, not the
/// brief's paraphrase, is treated as authoritative here, same precedent
/// `StreamCards.swift`'s file header already documents for a near-identical
/// brief-vs-source correction on Task 6.
enum StreamFilter: Equatable {
    case all
    case group(CardGroup)

    /// Chip order + label, ported from `EventStream.tsx`'s `FILTERS` array.
    static let chips: [(filter: StreamFilter, label: String)] = [
        (.all, "All"),
        (.group(.finding), "Findings"),
        (.group(.activity), "Agent activity"),
        (.group(.await_), "Awaiting"),
        (.group(.error), "Errors"),
    ]

    func matches(_ group: CardGroup) -> Bool {
        switch self {
        case .all: true
        case .group(let g): g == group
        }
    }
}

/// Situation Room — the center live stream. A newest-first `LazyVStack`
/// (Phase 5B's "everything windowed" lesson) of editorial cards, filterable
/// by the chips above. Port of `EventStream.tsx` + `EventCard.tsx`'s
/// layout; see the type doc comments below for the deliberate scope
/// reductions from the web (no "load older" pagination, no Parsed/Raw JSON
/// toggle for error details, no syntax-highlighted code excerpt).
///
/// **Two more scope reductions, added honestly on review (round 1, ruling
/// 11)** — neither is ported, both are parked for a future redesign pass
/// rather than silently dropped:
/// - **No `freshKeys`/`FRESH_MS` "just arrived" highlight** (`Events.tsx`
///   lines 87, 139-144: a card flashes for 2.5s when it first lands).
/// - **No follow/pin-to-top scroll behavior** (`EventStream.tsx` lines
///   43, 57-59, 64-65: the stream auto-scrolls to the newest card unless
///   the operator has scrolled down to read history, in which case new
///   arrivals stop yanking the view). This screen's `ScrollView` today
///   just stays wherever the operator left it.
struct EventStreamColumn: View {
    let cards: [StreamCard]
    @Binding var filter: StreamFilter
    let projectsByWorkspace: [String: APIProjectRow]
    let runToWorkspace: [String: String]
    let pendingActions: PendingActions
    let onApprove: (String, String) async -> Void
    let onReject: (String, String) async -> Void
    let onOpenRun: (String, String?) -> Void

    private var counts: [CardGroup: Int] {
        Dictionary(grouping: cards, by: \.group).mapValues(\.count)
    }

    private var shown: [StreamCard] {
        filter == .all ? cards : cards.filter { filter.matches($0.group) }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Rectangle().fill(Color.rupuBorder).frame(height: 1)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    if shown.isEmpty {
                        emptyState
                    } else {
                        ForEach(shown, id: \.key) { card in
                            EventCardView(
                                card: card,
                                project: resolveCardProject(card, runToWorkspace: runToWorkspace, projectsByWorkspace: projectsByWorkspace),
                                pendingActions: pendingActions,
                                onApprove: onApprove,
                                onReject: onReject,
                                onOpenRun: onOpenRun
                            )
                        }
                    }
                }
                .padding(20)
                .frame(maxWidth: 820)
                .frame(maxWidth: .infinity)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.rupuBg)
    }

    private var header: some View {
        HStack(spacing: 12) {
            Eyebrow("Live stream")
            Text("\(cards.count) events")
                .font(.metaText)
                .foregroundStyle(Color.rupuMute)
                .monospacedDigit()
            Spacer(minLength: 0)
            HStack(spacing: 6) {
                ForEach(StreamFilter.chips, id: \.label) { chip in
                    filterChip(chip.filter, label: chip.label)
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
    }

    private func filterChip(_ f: StreamFilter, label: String) -> some View {
        let active = filter == f
        let count = chipCount(for: f)
        return Button {
            filter = f
        } label: {
            HStack(spacing: 4) {
                Text(label)
                if let count {
                    Text("\(count)").opacity(0.7).monospacedDigit()
                }
            }
            .font(.metaText)
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
        }
        .buttonStyle(.plain)
        .foregroundStyle(active ? Color.rupuBg : Color.rupuDim)
        .background(active ? Color.rupuInk.opacity(0.9) : Color.clear, in: Capsule())
        .overlay(Capsule().stroke(active ? Color.clear : Color.rupuBorder, lineWidth: 1))
    }

    /// A count badge shows only for Findings/Awaiting/Errors — never for
    /// "All" or "Agent activity". Web parity, `EventStream.tsx` line 82:
    /// `ff.key === 'finding' ? counts.finding : ff.key === 'await' ?
    /// counts.await : ff.key === 'error' ? counts.error : undefined` —
    /// review fix round 1, ruling 11 (the prior version showed a count for
    /// `.activity` too, since it matched the general `.group` case).
    private func chipCount(for f: StreamFilter) -> Int? {
        guard case .group(let g) = f, g == .finding || g == .await_ || g == .error else { return nil }
        return counts[g] ?? 0
    }

    private var emptyState: some View {
        Text(cards.isEmpty ? "Waiting for events…" : "Nothing matches this filter.")
            .font(.noteText)
            .foregroundStyle(Color.rupuDim)
            .frame(maxWidth: .infinity)
            .padding(.top, 60)
    }
}

/// One editorial card. Port of `EventCard.tsx`'s per-`CardForm` render
/// branches. Two deliberate scope reductions from the web, both because the
/// task-7 brief's own conditional ("if the screen needs errorDetail.ts/
/// lang.ts derivations ... port them") is a judgment call, and this screen
/// doesn't strictly need either to render honestly:
/// - **No Parsed/Raw JSON toggle** for error `detail` (`ErrorDetail.tsx` /
///   `lib/situationRoom/errorDetail.ts`'s `parseErrorDetail`) — the raw
///   error string renders as wrapped plain text. Still fully honest (no
///   fabrication, nothing hidden), just less polished than the web's
///   pretty-printed JSON view.
/// - **No syntax-highlighted code excerpt** — the brief's own interface
///   note calls for "code excerpt (mono, contained)", which is exactly what
///   this renders (`Color.rupuSurface` block, `Font.dataMono`, bounded
///   height) — porting `lang.ts`'s language inference would only feed a
///   highlighter this screen doesn't have (`CodeHighlight`'s
///   `HIGHLIGHTABLE_LANGUAGES` machinery is web-only; the nearest native
///   analog, `RupuRunDetail/SourcePreview.swift`, lives in a module this one
///   doesn't depend on).
/// - A finding's `fileRef` renders as plain text, not a deep link to the
///   Code viewer (`EventCard.tsx`'s `codeHref` → `/projects/:id/code?path=`)
///   — there is no macOS route for "Project detail, Code tab, at a specific
///   path/line" yet; only `.projectDetail(wsID:)` exists. The finding's
///   `permalink` (an external SCM URL) still opens via a plain `Link`.
private struct EventCardView: View {
    let card: StreamCard
    let project: (label: String?, branch: String?)
    let pendingActions: PendingActions
    let onApprove: (String, String) async -> Void
    let onReject: (String, String) async -> Void
    let onOpenRun: (String, String?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            head
            cardBody(for: card.form)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.rupuPanel)
        .clipShape(RoundedRectangle(cornerRadius: 7))
        .overlay(alignment: .leading) {
            RoundedRectangle(cornerRadius: 1.5).fill(accentColor).frame(width: 3).padding(.vertical, 1)
        }
        .overlay(RoundedRectangle(cornerRadius: 7).stroke(Color.rupuBorder, lineWidth: 1))
    }

    private var head: some View {
        HStack(spacing: 8) {
            Text(card.badge)
                .font(.metaText.weight(.semibold))
                .foregroundStyle(accentColor)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(accentColor.opacity(0.12), in: Capsule())
            if let label = project.label {
                HStack(spacing: 4) {
                    Text(label).font(.metaText).foregroundStyle(Color.rupuDim)
                    if let branch = project.branch {
                        Text(branch).font(.metaText).foregroundStyle(Color.rupuMute)
                    }
                }
            }
            Spacer(minLength: 0)
            if let runID = card.runID {
                Button(String(runID.prefix(8))) {
                    onOpenRun(runID, nil)
                }
                .buttonStyle(.plain)
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuMute)
            }
            if card.ts > 0 {
                Text(relativeTime(card.ts)).font(.metaText).foregroundStyle(Color.rupuMute)
            }
        }
    }

    @ViewBuilder
    private func cardBody(for form: CardForm) -> some View {
        switch form {
        case .finding: findingBody
        case .error: errorBody
        case .await_: awaitBody
        default: activityBody
        }
    }

    private var findingBody: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .top, spacing: 6) {
                Icon(.shieldAlert, size: 14).foregroundStyle(Color.severity(card.severity ?? .info)).padding(.top, 2)
                Text(card.title).font(.leadText).foregroundStyle(Color.rupuInk)
            }
            if let fileRef = card.fileRef {
                Text(fileRef).font(.dataMono(11)).foregroundStyle(Color.rupuDim)
            }
            if let detail = card.detail {
                Text(detail).font(.noteText).foregroundStyle(Color.rupuDim)
            }
            if let code = card.code {
                codeExcerpt(code)
            }
            if let permalink = card.permalink, let url = URL(string: permalink) {
                Link(destination: url) {
                    // A plain "↗" glyph, not an SF Symbol — this codebase's
                    // icon set is exclusively the curated `LucideIcon` enum
                    // (parity with the web's lucide icons), which has no
                    // "external link" case to add without also updating
                    // `extract-lucide.mjs` and its pinned SVG-path test.
                    Text("View on repository ↗")
                }
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
            }
        }
    }

    /// `.horizontal`-only (review fix round 1, ruling 6): a nested
    /// `[.horizontal, .vertical]` `ScrollView` steals the outer stream's
    /// mouse-wheel gestures the moment the cursor is over this card — the
    /// operator scrolling the LIVE STREAM would instead scroll whichever
    /// code excerpt happened to be under the pointer. Vertical containment
    /// comes from `.lineLimit` instead (a truncating cap, not a scroll) —
    /// web parity: `CodeExcerpt.tsx` has no inner vertical scroll either,
    /// only horizontal (for long lines).
    private func codeExcerpt(_ code: String) -> some View {
        ScrollView(.horizontal) {
            Text(code)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(12)
                .padding(8)
        }
        .background(Color.rupuSurface)
        .clipShape(RoundedRectangle(cornerRadius: 5))
    }

    private var errorBody: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Icon(.xCircle, size: 13).foregroundStyle(Color.status(.failed))
                Text(card.title).font(.leadText).foregroundStyle(Color.rupuInk)
            }
            if let detail = card.detail {
                Text(detail).font(.dataMono(11)).foregroundStyle(Color.rupuDim).textSelection(.enabled)
            }
        }
    }

    private var activityBody: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Icon(activityIcon, size: 13).foregroundStyle(accentColor)
                Text(card.title).font(.leadText).foregroundStyle(Color.rupuInk)
            }
            if let detail = card.detail {
                Text(detail).font(.noteText).foregroundStyle(Color.rupuDim)
            }
        }
    }

    private var activityIcon: LucideIcon {
        if card.badge == "Scanning" { return .search }
        switch card.form {
        case .complete: return .checkCircle2
        case .lifecycle: return .play
        case .panel: return .repeatIcon
        default: return .activity
        }
    }

    // MARK: - Await (inline approve/reject)

    private var awaitBody: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(card.title).font(.leadText).foregroundStyle(Color.rupuInk)
            if let detail = card.detail {
                Text(detail).font(.noteText).foregroundStyle(Color.rupuDim)
            }
            if let approvable = card.approvable {
                awaitControls(approvable)
            }
        }
    }

    /// Review fix round 1, ruling 7: `approvable.stepID` is `String?`
    /// (`Approvable`, `StreamCards.swift`) even though `cardForEvent`'s only
    /// producer of an `await`-form card (`.stepAwaitingApproval`) always
    /// carries a concrete step id today — the prior version silently
    /// coerced a hypothetical `nil` to `""` and still posted it as
    /// `?gate=`. The CP parses an explicitly-present-but-empty `gate` query
    /// param as `Some("")`, which then fails lookup as `GateNotFound`
    /// (`crates/rupu-cp/src/api/runs.rs` lines 96, 117-121) — turning "we
    /// don't actually know which gate" into a confusing "gate not found"
    /// server error instead of just not offering an action at all. Renders
    /// the real controls only when a non-nil `stepID` exists, and passes it
    /// straight through — never `?? ""`.
    @ViewBuilder
    private func awaitControls(_ approvable: Approvable) -> some View {
        if let stepID = approvable.stepID {
            awaitActionControls(runID: approvable.runID, stepID: stepID)
        } else {
            Text("Awaiting approval")
                .font(.noteText.weight(.semibold))
                .foregroundStyle(Color.status(.awaiting))
        }
    }

    /// Port of `RunDetailScreen.gateRow`'s control block (`RunDetailScreen.
    /// swift` lines 441-503) — same gate-scoped `ActionKey.gate` pair,
    /// inline Approve/Reject, per-key failure note + Retry
    /// (`gateFailureNote`), and a stale-pending affordance
    /// (`pendingActions.isStale`). Review fix round 1, ruling 3: the first
    /// pass here only showed a bare failure message with no way to retry
    /// and no "this looks stuck" signal — exactly the visibility a
    /// fullscreen ops wall most needs for a gate that isn't resolving.
    @ViewBuilder
    private func awaitActionControls(runID: String, stepID: String) -> some View {
        let approveKey = ActionKey.gate(runID: runID, stepID: stepID, verb: .approve)
        let rejectKey = ActionKey.gate(runID: runID, stepID: stepID, verb: .reject)
        let approveState = pendingActions.state(approveKey)
        let rejectState = pendingActions.state(rejectKey)
        let anyPending = isPending(approveState) || isPending(rejectState)

        if isConfirmed(approveState) {
            resolvedTag("approved", tone: Color.status(.done))
        } else if isConfirmed(rejectState) {
            resolvedTag("rejected", tone: Color.status(.failed))
        } else {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    Button {
                        Task { await onApprove(runID, stepID) }
                    } label: {
                        HStack(spacing: 5) {
                            if isPending(approveState) { ProgressView().controlSize(.mini) }
                            Text("Approve")
                        }
                    }
                    .buttonStyle(RupuButtonStyle.primaryOk)
                    .disabled(anyPending)

                    Button {
                        Task { await onReject(runID, stepID) }
                    } label: {
                        HStack(spacing: 5) {
                            if isPending(rejectState) { ProgressView().controlSize(.mini) }
                            Text("Reject")
                        }
                    }
                    .buttonStyle(RupuButtonStyle.dangerOutline)
                    .disabled(anyPending)
                }

                gateFailureNote(state: approveState) { await onApprove(runID, stepID) }
                gateFailureNote(state: rejectState) { await onReject(runID, stepID) }

                if pendingActions.isStale(approveKey) || pendingActions.isStale(rejectKey) {
                    Text("Still pending — this may be stuck")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuMute)
                }
            }
        }
    }

    /// Inline failure text + Retry for one gate control — only renders when
    /// `state` is `.failed`. Direct port of `RunDetailScreen.
    /// gateFailureNote(state:retry:)`.
    @ViewBuilder
    private func gateFailureNote(state: ActionState, retry: @escaping () async -> Void) -> some View {
        if case .failed(let message) = state {
            HStack(spacing: 6) {
                Text(message)
                    .font(.noteText)
                    .foregroundStyle(Color.status(.failed))
                    .lineLimit(2)
                Button("Retry") {
                    Task { await retry() }
                }
                .buttonStyle(RupuButtonStyle.outline)
            }
        }
    }

    private func resolvedTag(_ label: String, tone: Color) -> some View {
        Text("✓ \(label) · you").font(.noteText.weight(.semibold)).foregroundStyle(tone)
    }

    private func isPending(_ state: ActionState) -> Bool {
        if case .pending = state { return true }
        return false
    }

    private func isConfirmed(_ state: ActionState) -> Bool {
        if case .confirmed = state { return true }
        return false
    }

    private var accentColor: Color {
        switch card.accent {
        case .severity(let s): Color.severity(s)
        case .brand: Color.rupuBrand
        case .await_: Color.status(.awaiting)
        case .error: Color.status(.failed)
        }
    }
}

/// Relative "time ago" from a ms-since-epoch timestamp. Port of
/// `EventCard.tsx`'s `rel(ts)`.
private func relativeTime(_ ms: Int64) -> String {
    let seconds = (Date().timeIntervalSince1970 * 1000 - Double(ms)) / 1000
    if seconds < 5 { return "now" }
    if seconds < 60 { return "\(Int(seconds.rounded()))s" }
    let minutes = seconds / 60
    if minutes < 60 { return "\(Int(minutes.rounded()))m" }
    let hours = minutes / 60
    if hours < 24 { return "\(Int(hours.rounded()))h" }
    return "\(Int((hours / 24).rounded()))d"
}
