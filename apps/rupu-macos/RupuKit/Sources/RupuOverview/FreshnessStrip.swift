import Foundation
import RupuDesign
import RupuStore
import SwiftUI

/// Under this age, a host reads as "live" rather than showing an elapsed
/// time — mirrors the web's `HostFreshnessStrip.tsx` `LIVE_THRESHOLD_MS`.
private let freshnessLiveThreshold: TimeInterval = 5

/// Formats the elapsed time since `capturedAt` the same way the web's
/// `age()` does: `"live"` under the threshold, then `Ns` / `Nm` / `Nh` —
/// deliberately NOT `RelativeDateTimeFormatter`'s "3 minutes ago" wording,
/// since this view's job is content/wording parity with the web strip, not
/// consistency with this app's other relative-time labels (`ActivityTable`,
/// `RunDetailScreen`).
func freshnessAgeLabel(capturedAt: Date, now: Date) -> String {
    let seconds = now.timeIntervalSince(capturedAt)
    if seconds < freshnessLiveThreshold { return "live" }
    if seconds < 60 { return "\(Int(seconds.rounded()))s" }
    if seconds < 3_600 { return "\(Int((seconds / 60).rounded()))m" }
    return "\(Int((seconds / 3_600).rounded()))h"
}

/// Per-host truth about how current this host's dashboard data is (spec
/// §5.4, ported from the web's `HostFreshnessStrip`). One global "live" pill
/// would lie about an SSH host — liveness is per-transport, so every host
/// carries its own pill: a tone dot + name + age. Ticks every second
/// (`TimelineView(.periodic)`) so a "live"/"3s" label advances between
/// `DashboardStore` refetches, same as the web's `setInterval`.
public struct FreshnessStrip: View {
    private let slices: [HostSlice]

    public init(slices: [HostSlice]) {
        self.slices = slices
    }

    public var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            HStack(spacing: 16) {
                ForEach(slices, id: \.id) { slice in
                    FreshnessPill(slice: slice, now: context.date)
                }
            }
        }
    }
}

/// One host's dot + name + age. Tone mapping is fixed per the state's
/// meaning, not a general `StatusDescriptor` lookup (these states are host
/// reporting states, not run/step lifecycle states): `.ok` → `.done`,
/// `.offline` → `.failed`, `.unavailable` → `.awaiting`, `.loading` →
/// `.pending` (this host has never actually reported yet — distinct from
/// "known-stale", so it must not read the same as a resolved-but-not-live
/// host).
private struct FreshnessPill: View {
    let slice: HostSlice
    let now: Date

    private var tone: StatusTone {
        switch slice.state {
        case .loading: .pending
        case .ok: .done
        case .offline: .failed
        case .unavailable: .awaiting
        }
    }

    /// `capturedAt == nil` on an `.ok` slice reads as "unknown", never
    /// "fresh" — a missing timestamp is not evidence of liveness (carried
    /// forward from Task 1/2's note: the server always emits it, but this
    /// stays honest even if that contract were ever violated).
    private var label: String {
        switch slice.state {
        case .loading:
            return "loading…"
        case .ok(let capturedAt):
            guard let capturedAt, let date = parseBucketTimestamp(capturedAt) else { return "unknown" }
            return freshnessAgeLabel(capturedAt: date, now: now)
        case .offline:
            return "offline"
        case .unavailable:
            return "unavailable"
        }
    }

    private var tooltip: String {
        if case .unavailable(let reason) = slice.state, let reason {
            return reason
        }
        return "\(slice.transportKind) host"
    }

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(Color.status(tone))
                .frame(width: 6, height: 6)
            Text(slice.name)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
            Text("·")
                .font(.metaText)
                .foregroundStyle(Color.rupuMute)
            Text(label)
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
        }
        .help(tooltip)
    }
}
