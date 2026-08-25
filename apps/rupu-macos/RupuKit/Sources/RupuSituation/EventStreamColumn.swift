import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

/// Reports the top-of-stream marker's position within `EventStreamColumn`'s
/// named scroll coordinate space — the sole input `scrollOffset` is derived
/// from. macOS's deployment target here is `.v14` (`RupuKit/Package.swift`),
/// which predates `ScrollView.onScrollGeometry` (macOS 15+); this
/// `GeometryReader`-behind-an-invisible-marker plus `PreferenceKey` is the
/// standard SwiftUI idiom for tracking scroll position on `.v14`.
private struct StreamScrollOffsetKey: PreferenceKey {
    static let defaultValue: Double = 0
    static func reduce(value: inout Double, nextValue: () -> Double) { value = nextValue() }
}

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
/// **Follow/pin + fresh-highlight** (redesign pass, Task 4 — the 6B doc
/// comment here previously parked both as out of scope; both now land,
/// closing that gap). `follow`/`scrollOffset`/`renderState` below are owned
/// at THIS component's level, matching the web's own ownership exactly:
/// `EventStream.tsx`'s `follow`/`scrollRef` state (lines 56-58) lives inside
/// that component's own function body, not its page (`Events.tsx`, whose
/// only Situation-Room-scroll-relevant state is `freshKeys` — passed down as
/// a prop, `Events.tsx:329`, same as `freshKeys` is passed into this view
/// below). See `StreamFollow.swift`'s file header for the full read of
/// `EventStream.tsx` lines 56-65 (verified: no separate "jump to latest (N
/// new)" affordance exists anywhere in that 142-line file or its sole page
/// consumer — this task's own brief describes one, but it isn't in the
/// cited source; built here anyway as a macOS-native addition backed by a
/// REAL deferred count, not a fabricated one — see `planStreamRender`'s doc
/// comment for where that count comes from).
struct EventStreamColumn: View {
    let cards: [StreamCard]
    let freshKeys: Set<String>
    @Binding var filter: StreamFilter
    let projectsByWorkspace: [String: APIProjectRow]
    let runToWorkspace: [String: String]
    let pendingActions: PendingActions
    let onApprove: (String, String) async -> Void
    let onReject: (String, String) async -> Void
    let onOpenRun: (String, String?) -> Void

    /// Points scrolled down from the top of the stream — driven by
    /// `StreamScrollOffsetKey` below, an initial-value-relative delta (not
    /// an absolute geometry reading) so the `LazyVStack`'s own top padding
    /// doesn't have to be accounted for separately. See `isFollowing`'s doc
    /// comment (`StreamFollow.swift`) for the 48pt threshold this feeds.
    @State private var scrollOffset: Double = 0
    @State private var scrollOffsetBaseline: Double?
    @State private var renderState = StreamRenderState()

    private static let topAnchorID = "situation-stream-top"
    private static let scrollSpace = "situationEventStreamScroll"

    private var following: Bool { isFollowing(offsetFromTop: scrollOffset) }

    private var counts: [CardGroup: Int] {
        Dictionary(grouping: cards, by: \.group).mapValues(\.count)
    }

    /// Follow-suspended cards are held back from `cards` itself — see
    /// `planStreamRender`'s doc comment — BEFORE the chip filter narrows
    /// what's actually displayed, mirroring `EventStream.tsx`'s own
    /// dependency scope: its pin-to-top effect keys on the full `cards.length`
    /// prop (line 59), not the filtered `shown` memo, so this port's
    /// suspend/defer state tracks the same full, unfiltered stream.
    private var renderPlan: (shown: [StreamCard], deferredCount: Int) {
        planStreamRender(all: cards, state: renderState)
    }

    private var shown: [StreamCard] {
        let base = renderPlan.shown
        return filter == .all ? base : base.filter { filter.matches($0.group) }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Rectangle().fill(Color.rupuBorder).frame(height: 1)
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        topAnchor
                        if shown.isEmpty {
                            emptyState
                        } else {
                            ForEach(shown, id: \.key) { card in
                                EventCardView(
                                    card: card,
                                    project: resolveCardProject(card, runToWorkspace: runToWorkspace, projectsByWorkspace: projectsByWorkspace),
                                    pendingActions: pendingActions,
                                    fresh: freshKeys.contains(card.key),
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
                .coordinateSpace(name: Self.scrollSpace)
                .onPreferenceChange(StreamScrollOffsetKey.self) { minY in
                    if scrollOffsetBaseline == nil { scrollOffsetBaseline = minY }
                    scrollOffset = (scrollOffsetBaseline ?? minY) - minY
                }
                .onChange(of: following) { _, newValue in
                    renderState = nextStreamRenderState(renderState, following: newValue, currentKeys: Set(cards.map(\.key)))
                    pinToTopIfFollowing(proxy: proxy)
                }
                .onChange(of: cards.count) { _, _ in
                    pinToTopIfFollowing(proxy: proxy)
                }
                .safeAreaInset(edge: .top, spacing: 0) {
                    if !following, renderPlan.deferredCount > 0 {
                        jumpToLatestBar(count: renderPlan.deferredCount) {
                            withAnimation(.easeOut(duration: 0.2)) {
                                proxy.scrollTo(Self.topAnchorID, anchor: .top)
                            }
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.rupuBg)
    }

    /// Invisible marker at the very top of the stream — both the
    /// `scrollTo` target for pinning/jumping to latest, and (via its
    /// `GeometryReader` background) the sole source of `scrollOffset`
    /// above.
    private var topAnchor: some View {
        Color.clear
            .frame(height: 0)
            .id(Self.topAnchorID)
            .background(
                GeometryReader { geo in
                    Color.clear.preference(
                        key: StreamScrollOffsetKey.self,
                        value: Double(geo.frame(in: .named(Self.scrollSpace)).minY)
                    )
                }
            )
    }

    /// Pins to the newest card WHILE following — port of `EventStream.tsx`'s
    /// `useLayoutEffect` (lines 56-59): `if (follow) scrollTop = 0`. Called
    /// from both `.onChange(of: following)` (the web effect's `follow` dep)
    /// and `.onChange(of: cards.count)` (the web effect's `cards.length`
    /// dep) — a no-op when NOT following, matching the web's own guard.
    private func pinToTopIfFollowing(proxy: ScrollViewProxy) {
        guard following else { return }
        withAnimation(.easeOut(duration: 0.15)) {
            proxy.scrollTo(Self.topAnchorID, anchor: .top)
        }
    }

    /// "N new — jump to latest" — see this type's file header for why this
    /// affordance is a macOS-native addition, not a literal web port, and
    /// why its count is real rather than fabricated.
    private func jumpToLatestBar(count: Int, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Icon(.chevronDown, size: 11)
                Text("\(count) new event\(count == 1 ? "" : "s") — jump to latest")
            }
            .font(.metaText.weight(.semibold))
            .padding(.horizontal, 14)
            .padding(.vertical, 7)
        }
        .buttonStyle(.plain)
        .foregroundStyle(Color.rupuBg)
        .background(Color.rupuBrand, in: Capsule())
        .padding(.top, 10)
        .frame(maxWidth: .infinity)
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
    /// Fresh-arrival highlight — port of `EventCard.tsx`'s `fresh` prop
    /// (`is-fresh` class, `EventCard.tsx:100`). **Visual divergence from the
    /// web, stated honestly**: the web's own `.sr-ev.is-fresh` rule
    /// (`styles.css:340`) is a ONE-SHOT 0.5s slide/fade-in `@keyframes`
    /// animation that plays once when the class is first applied and does
    /// not loop — the class then sits on the element, visually inert, for
    /// the remaining ~2s of its full `FRESH_MS` (2500ms) life. This port
    /// instead ties a persistent accent tint (border + background wash) to
    /// `fresh` for its FULL window — a native list redraws cells instantly
    /// (no CSS transition-in the way a DOM mutation gets one for free), so a
    /// highlight that visually settles after 0.5s would be nearly
    /// imperceptible in a fast-moving burst. Same constant (2.5s,
    /// `freshHighlightSeconds`, `SituationSelection.swift`), same expiry
    /// semantics (marks on arrival, clears after the window) — only how
    /// long the highlight stays VISIBLE differs, and only because a longer
    /// visible window is what makes the highlight legible on this platform.
    let fresh: Bool
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
        .background(fresh ? Color.rupuBrand.opacity(0.07) : Color.rupuPanel)
        .clipShape(RoundedRectangle(cornerRadius: 7))
        .overlay(alignment: .leading) {
            RoundedRectangle(cornerRadius: 1.5).fill(accentColor).frame(width: 3).padding(.vertical, 1)
        }
        .overlay(RoundedRectangle(cornerRadius: 7).stroke(fresh ? Color.rupuBrand.opacity(0.5) : Color.rupuBorder, lineWidth: fresh ? 1.5 : 1))
        .animation(.easeOut(duration: 0.3), value: fresh)
    }

    private var head: some View {
        HStack(spacing: 8) {
            Text(card.badge)
                .font(.metaText.weight(.semibold))
                .foregroundStyle(accentColor)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(accentColor.opacity(0.12), in: ChromeShape.pill)
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
