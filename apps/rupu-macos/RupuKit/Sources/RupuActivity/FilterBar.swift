import SwiftUI
import RupuStore
import RupuDesign

/// Kind segmented control (bound *through* `model.route` — the sidebar and
/// this control share one piece of state, never two that can drift apart),
/// additive status chips (`store.statusFilter` narrows synchronously, no
/// refetch), a live-tail switch, and — when `pendingNewRuns > 0` — a "N new
/// runs" pill that calls `applyPendingRefresh()`.
struct FilterBar: View {
    @Bindable var model: AppModel
    @Bindable var store: ActivityStore

    private static let chipStatuses: [ActivityStatus] = [
        .pending, .running, .completed, .failed, .awaiting, .rejected, .cancelled, .paused,
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                kindPicker
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
            MicroLabel("\(store.pendingNewRuns) new runs")
                .foregroundStyle(Color.rupuBrandHi)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Color.rupuBrand.opacity(0.16))
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

    private func statusChip(_ status: ActivityStatus) -> some View {
        let isOn = store.statusFilter.contains(status)
        let tone = Color.status(status.tone)
        return Button {
            toggle(status)
        } label: {
            MicroLabel(status.displayLabel)
                .foregroundStyle(isOn ? Color.rupuInk : Color.rupuDim)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(tone.opacity(0.12))
                .clipShape(Capsule())
                .overlay(Capsule().stroke(tone.opacity(0.3), lineWidth: 1))
                .opacity(isOn ? 1.0 : 0.6)
        }
        .buttonStyle(.plain)
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
