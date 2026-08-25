import Testing
import Foundation
import RupuDesign
import RupuStore
@testable import RupuOverview

/// `FreshnessTone.tone(for:now:)` is the pure, testable seam behind
/// `FreshnessStrip`'s dot color — a plain enum static, never a `View`
/// member, so none of these need `@MainActor`.

private func iso8601(_ date: Date) -> String {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime]
    return formatter.string(from: date)
}

@Test func freshnessToneFreshOkIsDone() {
    let now = Date()
    let slice = HostSlice(id: "local", name: "local", transportKind: "local", state: .ok(capturedAt: iso8601(now.addingTimeInterval(-1))))
    #expect(FreshnessTone.tone(for: slice, now: now) == .done)
}

@Test func freshnessToneThreeMinuteOldOkIsPending() {
    // Past the 120s (2x the 60s reconcile cadence) staleness threshold —
    // a host that hasn't actually reported in a while must not keep
    // showing a confident green dot.
    let now = Date()
    let slice = HostSlice(id: "remote", name: "remote", transportKind: "ssh", state: .ok(capturedAt: iso8601(now.addingTimeInterval(-180))))
    #expect(FreshnessTone.tone(for: slice, now: now) == .pending)
}

@Test func freshnessToneNilCapturedAtOkIsPending() {
    // Unknown is never "fresh" (Task 1's rule) — a missing timestamp on an
    // `.ok` slice must not render the same green dot a genuinely fresh one
    // gets.
    let now = Date()
    let slice = HostSlice(id: "remote", name: "remote", transportKind: "ssh", state: .ok(capturedAt: nil))
    #expect(FreshnessTone.tone(for: slice, now: now) == .pending)
}

@Test func freshnessToneUnparseableCapturedAtOkIsPending() {
    let now = Date()
    let slice = HostSlice(id: "remote", name: "remote", transportKind: "ssh", state: .ok(capturedAt: "not-a-timestamp"))
    #expect(FreshnessTone.tone(for: slice, now: now) == .pending)
}

@Test func freshnessToneOfflineIsFailedUnchanged() {
    let now = Date()
    let slice = HostSlice(id: "h", name: "h", transportKind: "ssh", state: .offline)
    #expect(FreshnessTone.tone(for: slice, now: now) == .failed)
}

@Test func freshnessToneUnavailableIsAwaitingUnchanged() {
    let now = Date()
    let slice = HostSlice(id: "h", name: "h", transportKind: "ssh", state: .unavailable(reason: "boom"))
    #expect(FreshnessTone.tone(for: slice, now: now) == .awaiting)
}

@Test func freshnessToneLoadingIsPendingUnchanged() {
    let now = Date()
    let slice = HostSlice(id: "h", name: "h", transportKind: "ssh", state: .loading)
    #expect(FreshnessTone.tone(for: slice, now: now) == .pending)
}
