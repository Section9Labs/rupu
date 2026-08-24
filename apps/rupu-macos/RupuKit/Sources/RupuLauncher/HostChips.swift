import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// One chip per `store.hosts` row plus a standalone "FAN OUT: ALL HEALTHY"
/// toggle chip. A host chip shows its name and, when the server reported
/// one, its `activeRunCount` in `Font.numeral` — omitted entirely rather
/// than showing a placeholder when `nil` (null discipline: "unknown" and
/// "zero" are different facts). Offline/stale hosts render dim and
/// disabled — never a dead-looking-but-tappable control. `fanOutAllHealthy`
/// supersedes any individual selection (`LauncherStore.resolvedTargets()`
/// ignores `selectedHosts` while it's on), so every host chip is disabled
/// too while the fan-out toggle is active — tapping one would silently do
/// nothing otherwise.
struct HostChips: View {
    @Bindable var store: LauncherStore

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            MicroLabel("Targets")
                .foregroundStyle(Color.rupuMute)

            if store.hosts.isEmpty {
                MicroLabel("LOADING HOSTS…")
                    .foregroundStyle(Color.rupuMute)
            }

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(store.hosts, id: \.id) { host in
                        hostChip(host)
                    }
                    fanOutChip
                }
            }
        }
    }

    private func hostChip(_ host: APIHostRow) -> some View {
        let isOnline = host.status == "online"
        let isSelected = store.selectedHosts.contains(host.id) && !store.fanOutAllHealthy
        let disabled = !isOnline || store.fanOutAllHealthy

        return Button {
            toggle(host.id)
        } label: {
            HStack(spacing: 6) {
                Circle()
                    .fill(isOnline ? Color.status(.done) : Color.rupuMute)
                    .frame(width: 5, height: 5)
                Text(host.name)
                    .font(.system(size: 11.5))
                    .foregroundStyle(isOnline ? Color.rupuInk : Color.rupuMute)
                if let count = host.activeRunCount {
                    Text("\(count)")
                        .font(.numeral(size: 10))
                        .foregroundStyle(Color.rupuDim)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(isSelected ? Color.rupuBrand.opacity(0.15) : Color.rupuSurface)
            .clipShape(Capsule())
            .overlay(Capsule().stroke(Color.rupuBorder, lineWidth: 1))
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.45 : 1)
    }

    private var fanOutChip: some View {
        Button {
            store.fanOutAllHealthy.toggle()
        } label: {
            MicroLabel("Fan out: all healthy")
                .foregroundStyle(store.fanOutAllHealthy ? Color.rupuBrandHi : Color.rupuMute)
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(store.fanOutAllHealthy ? Color.rupuBrand.opacity(0.15) : Color.rupuSurface)
                .clipShape(Capsule())
                .overlay(
                    Capsule().stroke(store.fanOutAllHealthy ? Color.rupuBrand : Color.rupuBorder, lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
    }

    private func toggle(_ id: String) {
        if store.selectedHosts.contains(id) {
            store.selectedHosts.remove(id)
        } else {
            store.selectedHosts.insert(id)
        }
    }
}
