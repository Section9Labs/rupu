import SwiftUI
import RupuStore
import RupuDesign

/// Kind segmented control (bound *through* `model.route` — the sidebar and
/// this control share one piece of state, never two that can drift apart),
/// additive status chips (`store.statusFilter` narrows synchronously, no
/// refetch), a live-tail switch, and — when `pendingNewRuns > 0` — a "N new
/// runs" pill that calls `applyPendingRefresh()`.
///
/// **`showRunsChrome` (review fix, round 1)** — `false` while the Activity
/// screen's autoflows-kind Claims sub-tab is showing: the status chips,
/// live-tail toggle, and "+N new runs" pill all act on `ActivityStore`'s
/// `ActivityTable`, which isn't even on screen at that point (`ClaimsTable`
/// is) — leaving them visible-and-live-but-inert violates the no-dead-
/// controls rule (toggling live-tail would keep ticking a table nobody can
/// see; the pill would apply a refresh to rows not shown). The kind picker
/// stays regardless of this flag — it's the only way back OUT of the
/// autoflows kind, never inert.
struct FilterBar: View {
    @Bindable var model: AppModel
    @Bindable var store: ActivityStore
    let showRunsChrome: Bool

    private static let chipStatuses: [ActivityStatus] = [
        .pending, .running, .completed, .failed, .awaiting, .rejected, .cancelled, .paused,
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                kindPicker
                Spacer(minLength: 0)
                if showRunsChrome {
                    if store.pendingNewRuns > 0 {
                        newRunsPill
                    }
                    liveTailToggle
                }
            }
            if showRunsChrome {
                statusChips
            }
        }
        .padding(12)
        .panelStyle(.panel)
    }

    private var kindPicker: some View {
        Picker("Kind", selection: kindBinding) {
            ForEach(RunKindFilter.allCases, id: \.self) { kind in
                Text(kind.filterLabel).tag(kind)
            }
        }
        .pickerStyle(.segmented)
        .frame(width: 380)
        .labelsHidden()
    }

    /// The one piece of state the kind filter reads/writes — `model.route`.
    /// There is no local `@State` mirror: this binding IS the sidebar's own
    /// selection source, so a segmented-control change and a sidebar click
    /// can never disagree.
    private var kindBinding: Binding<RunKindFilter> {
        Binding(
            get: {
                if case .activity(let kind) = model.route { return kind }
                return .all
            },
            set: { model.route = .activity($0) }
        )
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

private extension RunKindFilter {
    var filterLabel: String {
        switch self {
        case .all: "All"
        case .agents: "Agents"
        case .workflows: "Workflows"
        case .autoflows: "Autoflows"
        case .sessions: "Sessions"
        }
    }
}
