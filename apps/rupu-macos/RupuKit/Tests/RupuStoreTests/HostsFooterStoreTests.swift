import Testing
@testable import RupuStore
import RupuAPI

/// `HostsFooterStore.apply` is the pure seam: given a batch of `/api/hosts`
/// rows, it derives `(total, down)` without touching the network — the
/// `activate(client:)` 60s poll loop itself is deliberately not
/// timing-tested (Task 1 brief), only this pure mapping.
@MainActor @Test func applyComputesDownCountFromNonOnlineStatuses() {
    let store = HostsFooterStore()
    store.apply([
        APIHostRow(id: "local", name: "local", transportKind: "embedded", status: "online"),
        APIHostRow(id: "mini", name: "mini", transportKind: "ssh", status: "online"),
        APIHostRow(id: "kuki", name: "kuki", transportKind: "tunnel", status: "offline"),
    ])

    #expect(store.summary?.total == 3)
    #expect(store.summary?.down == 1)
}

@MainActor @Test func applyWithEmptyRowsYieldsZeroZero() {
    let store = HostsFooterStore()
    store.apply([])

    #expect(store.summary?.total == 0)
    #expect(store.summary?.down == 0)
}
