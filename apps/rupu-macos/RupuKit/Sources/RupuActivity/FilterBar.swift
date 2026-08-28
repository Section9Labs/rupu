import SwiftUI
import RupuStore
import RupuDesign

/// Additive status chips (`store.statusFilter` narrows synchronously, no
/// refetch), a live-tail switch, and — when `pendingNewRuns > 0` — a "N new
/// runs" pill that calls `applyPendingRefresh()`.
///
/// **The kind segmented picker is GONE** (perf & interaction arc, Plan 5
/// Task 4 — matt's direct restructure feedback: the Activity parent stops
/// showing one combined table with a kind-picker; each kind gets its own
/// dedicated table, reached via the sidebar's existing disclosure children
/// — Task 0 — which already drive `model.route` directly). This `View` now
/// only ever renders on a kind page (never on the `.all` parent, which shows
/// `ActivityStatsView` instead and has no `FilterBar` at all — see
/// `ActivityScreen.body`), so it no longer needs `model`/`Route` at all.
///
/// **`showRunsChrome` (review fix, round 1, predates this task)** — `false`
/// while the Activity screen's autoflows-kind Claims sub-tab is showing: the
/// status chips, live-tail toggle, and "+N new runs" pill all act on
/// `ActivityStore`'s per-kind table, which isn't even on screen at that
/// point (`ClaimsTable` is) — leaving them visible-and-live-but-inert
/// violates the no-dead-controls rule. Getting back OUT of Claims no longer
/// needs a control THIS view renders — the `AutoflowsSubTab` picker
/// (`ActivityScreen.autoflowsSubTabPicker`) already does that, unaffected by
/// this change; `ActivityScreen.body` skips this whole view when
/// `showRunsChrome` is false rather than rendering it empty.
struct FilterBar: View {
    @Bindable var store: ActivityStore

    private static let chipStatuses: [ActivityStatus] = [
        .pending, .running, .completed, .failed, .awaiting, .rejected, .cancelled, .paused,
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                dateRangeFilter
                Spacer(minLength: 0)
                if store.pendingNewRuns > 0 {
                    newRunsPill
                }
                liveTailToggle
            }
            statusChips
        }
        .padding(12)
        .panelStyle(.panel)
    }

    /// Server-side custom date range (perf & interaction arc, Plan 5 Task 5)
    /// — see `KindTableDateRangeFilter`'s own doc comment for the day-
    /// boundary normalization and why this resets paging rather than just
    /// narrowing `rows` in place like the status chips below do.
    private var dateRangeFilter: some View {
        KindTableDateRangeFilter(since: store.since, until: store.until) { since, until in
            Task { await store.setDateRange(since: since, until: until) }
        }
    }

    private var liveTailToggle: some View {
        Toggle("Live tail", isOn: $store.liveTail)
            .toggleStyle(.switch)
            .controlSize(.small)
    }

    private var newRunsPill: some View {
        Button {
            Task { await store.applyPendingRefresh() }
        } label: {
            Text("\(store.pendingNewRuns) new runs")
                .font(.metaText)
                .foregroundStyle(Color.rupuBrand700)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Color.rupuBrand.opacity(0.12))
                .clipShape(Capsule())
                .overlay(Capsule().stroke(Color.rupuBrand.opacity(0.4), lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    private var statusChips: some View {
        HStack(spacing: 6) {
            ForEach(Self.chipStatuses, id: \.self) { status in
                statusChip(status)
            }
            Spacer(minLength: 0)
        }
    }

    /// Task 2 (chrome-kit fidelity, A5): neutral-outline-until-selected, per
    /// web's `FilterPills` (`components/ui/FilterPills.tsx:41-45`) — the real
    /// web analog for this control (`WorkflowRuns.tsx`/`AgentRuns.tsx` use it
    /// for their lifecycle/status filter row). Inactive = `border-border
    /// bg-panel text-ink-dim`; active = `border-brand-600 bg-brand-600
    /// text-white`, never per-status color-tinted — this chip no longer
    /// reads `status.tone` at all. Shape stays `Capsule`, NOT `ChromeShape.
    /// pill`: `FilterPills` is `rounded-full` (its own doc comment: "Rounded-
    /// full, single-select, brand-filled active pill"), a deliberately
    /// different, genuinely-round family from `StatusPill`'s 4pt `rounded`
    /// (see `ChromeShape`'s doc comment) — the audit's assumption that this
    /// chip "rides the same 4pt kit constant" didn't hold up against the
    /// actual source and is corrected here. Multi-select toggle semantics
    /// (vs. web's single-select tabs) are unchanged — out of this task's
    /// shape/tint-fidelity scope.
    private func statusChip(_ status: ActivityStatus) -> some View {
        let isOn = store.statusFilter.contains(status)
        return Button {
            toggle(status)
        } label: {
            Text(chipLabel(status))
                .font(.metaText)
                .foregroundStyle(isOn ? Color.white : Color.rupuDim)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(isOn ? Color.rupuBrand600 : Color.rupuPanel)
                .clipShape(Capsule())
                .overlay(Capsule().stroke(isOn ? Color.rupuBrand600 : Color.rupuBorder, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    /// `status.displayLabel` verbatim for every case except `.awaiting`
    /// (redesign-pass fix — audit A4). `displayLabel` itself now carries
    /// the full canonical "Awaiting approval" (matched to the Activity
    /// table's status column, which reads that property directly), but this
    /// chip is one of a row of narrow capsules sharing the filter bar's
    /// width — the audit explicitly left the short "Awaiting" form here as
    /// an accepted call rather than a divergence to fix, so this override
    /// is deliberate, not a leftover from before the table's label grew.
    private func chipLabel(_ status: ActivityStatus) -> String {
        status == .awaiting ? "Awaiting" : status.displayLabel
    }

    private func toggle(_ status: ActivityStatus) {
        if store.statusFilter.contains(status) {
            store.statusFilter.remove(status)
        } else {
            store.statusFilter.insert(status)
        }
    }
}
