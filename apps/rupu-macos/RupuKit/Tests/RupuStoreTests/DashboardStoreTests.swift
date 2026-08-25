import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test infra (duplicated from `ActivityStoreTests.swift`)
//
// `ActivityStoreTests`'s `SignalsFactoryBox`/`pollUntil`/`expectEventually`
// are declared `private`, which is file-scoped in Swift — not visible here
// even though both files live in the same `RupuStoreTests` target. Same
// rationale that file's own header comment gives for duplicating
// `CPClientTests.StubURLProtocol`: re-declare locally rather than widen
// another file's access just for this one.

/// Path-routing HTTP stub for `DashboardStore`'s two endpoints
/// (`GET /api/hosts`, `GET /api/dashboard`). Routes on `request.url?.path`.
final class DashboardStubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) static var pathHits: [String: Int] = [:]

    static func session() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [DashboardStubURLProtocol.self]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        pathHits = [:]
        self.handler = handler
    }

    static func hits(_ path: String) -> Int { pathHits[path, default: 0] }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        if let path = request.url?.path {
            DashboardStubURLProtocol.pathHits[path, default: 0] += 1
        }
        guard let handler = DashboardStubURLProtocol.handler else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let (status, body) = handler(request)
        let response = HTTPURLResponse(
            url: request.url!, statusCode: status, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

/// Fakes `DashboardStore`'s `signalsFactory` seam — same shape as
/// `ActivityStoreTests`' own copy (see that file's doc comment for why each
/// `activate()` call needs a *fresh* `AsyncStream`, not a single reused
/// one).
private final class SignalsFactoryBox: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [AsyncStream<StreamSignal<CPEvent>>.Continuation] = []

    func factory() -> AsyncStream<StreamSignal<CPEvent>> {
        let (stream, continuation) = AsyncStream<StreamSignal<CPEvent>>.makeStream()
        lock.withLock { continuations.append(continuation) }
        return stream
    }

    var latest: AsyncStream<StreamSignal<CPEvent>>.Continuation {
        lock.withLock { continuations[continuations.count - 1] }
    }
}

/// Thread-safe call counter — same rationale as `ActivityStoreTests`'s own
/// `Counter` (file-scoped `private`, so re-declared here rather than shared).
private final class HitCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    /// Increments and returns the NEW count (1 on the first call).
    func incrementAndGet() -> Int { lock.withLock { v += 1; return v } }
}

/// De-flakes "wait for something async to land" — same rationale and shape
/// as `ActivityStoreTests`'s own copy.
@MainActor
private func pollUntil(
    timeout: Duration = .seconds(5),
    interval: Duration = .milliseconds(10),
    _ condition: () -> Bool
) async -> Bool {
    let deadline = ContinuousClock.now + timeout
    while true {
        if condition() { return true }
        if ContinuousClock.now >= deadline { return false }
        try? await Task.sleep(for: interval)
    }
}

@MainActor
private func expectEventually(
    timeout: Duration = .seconds(5),
    interval: Duration = .milliseconds(10),
    _ description: String,
    sourceLocation: SourceLocation = #_sourceLocation,
    _ condition: () -> Bool
) async {
    let succeeded = await pollUntil(timeout: timeout, interval: interval, condition)
    #expect(succeeded, "timed out waiting for: \(description)", sourceLocation: sourceLocation)
}

// MARK: - Merge tests (pure — table ported from
// `crates/rupu-cp/src/api/dashboard.rs`'s `merge_tests` module)

@Suite
struct DashboardMergeTests {
    private static let emptyFleet = APIFleetCounts(
        repos: nil, providersConfigured: nil, providersUnhealthy: nil,
        autoflowsEnabled: nil, autoflowsDisabled: nil, workers: nil, claimsActive: nil,
        issuesPending: nil, issuesOpen: nil, issuesCapped: false, inventoryCapturedAt: nil
    )
    private static let zeroActive = APIActiveCounts(running: 0, awaitingApproval: 0, paused: 0, pending: 0)
    private static let emptyCycles = APICycleCounts(total: 0, clean: nil, withFailures: nil)

    private static func response(
        hostID: String = "h",
        findingsPartial: Bool = false,
        cyclesPartial: Bool = false,
        fleetPartial: Bool = false,
        active: APIActiveCounts? = nil,
        activeLongest: APIActiveLongest? = nil,
        terminalBuckets: [APITerminalBucket] = [],
        throughputBuckets: [APIThroughputBucket] = [],
        cycles: APICycleCounts? = nil,
        findingsOpen: Int? = nil,
        fleet: APIFleetCounts? = nil,
        capturedAt: String? = nil
    ) -> APIDashboardResponse {
        APIDashboardResponse(
            hosts: [APIHostFreshness(hostID: hostID, name: hostID, transportKind: "local", state: "ok", capturedAt: capturedAt, reason: nil)],
            findingsPartial: findingsPartial,
            cyclesPartial: cyclesPartial,
            fleetPartial: fleetPartial,
            active: active ?? zeroActive,
            activeLongest: activeLongest,
            terminalBuckets: terminalBuckets,
            throughputBuckets: throughputBuckets,
            cycles: cycles ?? emptyCycles,
            findingsOpen: findingsOpen,
            fleet: fleet ?? emptyFleet,
            capturedAt: capturedAt
        )
    }

    // Ports `findings_open_sums_only_reporting_hosts_and_flags_partial` +
    // `fleet_counts_sum_only_reporting_hosts_and_flag_partial`: sum only the
    // `Some` contributions, never fabricate 0 for a `nil`-reporting host,
    // but also never poison an already-summed value to `nil`.
    @Test func sumsOnlySomeContributionsForFindingsAndFleetAndFlagsPartial() {
        let a = Self.response(
            hostID: "local",
            findingsOpen: 3,
            fleet: APIFleetCounts(
                repos: nil, providersConfigured: 2, providersUnhealthy: nil,
                autoflowsEnabled: 4, autoflowsDisabled: nil, workers: 3, claimsActive: 9,
                issuesPending: nil, issuesOpen: nil, issuesCapped: false, inventoryCapturedAt: nil
            )
        )
        let b = Self.response(hostID: "ssh", findingsOpen: nil, fleet: Self.emptyFleet)

        let merged = DashboardStore.merge([a, b])

        #expect(merged.findingsOpen == 3, "must sum only the Some contribution, never fabricate 0 for the None-reporting host")
        #expect(merged.findingsPartial)
        #expect(merged.fleet.workers == 3, "must not fabricate 0 for the ssh host")
        #expect(merged.fleet.claimsActive == 9)
        #expect(merged.fleet.autoflowsEnabled == 4)
        #expect(merged.fleetPartial, "the ssh host reported nil for every fleet field it was asked about")
    }

    // Ports `fleet_counts_sum_across_hosts_and_are_not_partial_when_all_report`.
    @Test func fleetCountsSumAcrossHostsAndAreNotPartialWhenAllReport() {
        let a = Self.response(fleet: APIFleetCounts(
            repos: 1, providersConfigured: 3, providersUnhealthy: 0, autoflowsEnabled: 1, autoflowsDisabled: 0,
            workers: 2, claimsActive: 1, issuesPending: 0, issuesOpen: 0, issuesCapped: false, inventoryCapturedAt: nil
        ))
        let b = Self.response(fleet: APIFleetCounts(
            repos: 6, providersConfigured: 3, providersUnhealthy: 0, autoflowsEnabled: 2, autoflowsDisabled: 1,
            workers: 5, claimsActive: 4, issuesPending: 0, issuesOpen: 0, issuesCapped: false, inventoryCapturedAt: nil
        ))

        let merged = DashboardStore.merge([a, b])

        #expect(merged.fleet.workers == 7)
        #expect(merged.fleet.claimsActive == 5)
        #expect(merged.fleet.autoflowsEnabled == 3)
        #expect(merged.fleet.autoflowsDisabled == 1)
        #expect(merged.fleet.providersConfigured == 6)
        #expect(!merged.fleetPartial, "every host reported every field it was asked for")
        #expect(!merged.fleet.issuesCapped)
    }

    // Ports `cycle_counts_merge_none_contributor_makes_the_merged_value_none_and_partial`:
    // unlike `findingsOpen`/fleet counts above, `clean`/`withFailures` are
    // truly POISONED — a `nil` from any host permanently pins the merged
    // value to `nil`, never a truncated sum of the hosts that DID report.
    @Test func noneContributorPoisonsCycleBreakdownPermanentlyButNeverTheTotal() {
        let local = Self.response(cycles: APICycleCounts(total: 4, clean: 3, withFailures: 1))
        let ssh = Self.response(cycles: APICycleCounts(total: 2, clean: nil, withFailures: nil))

        let merged = DashboardStore.merge([local, ssh])

        #expect(merged.cycles.total == 6, "total is always a complete sum regardless of the breakdown")
        #expect(merged.cycles.clean == nil, "one host reported nil for clean — must never fabricate a truncated sum")
        #expect(merged.cycles.withFailures == nil)
        #expect(merged.cyclesPartial)
    }

    // Ports `cycle_counts_merge_sums_when_every_host_reports`.
    @Test func cycleCountsSumWhenEveryHostReports() {
        let a = Self.response(cycles: APICycleCounts(total: 3, clean: 2, withFailures: 1))
        let b = Self.response(cycles: APICycleCounts(total: 5, clean: 4, withFailures: 1))

        let merged = DashboardStore.merge([a, b])

        #expect(merged.cycles.total == 8)
        #expect(merged.cycles.clean == 6)
        #expect(merged.cycles.withFailures == 2)
        #expect(!merged.cyclesPartial)
    }

    // Ports `active_longest_merge_picks_the_max_age_across_hosts` (+ the
    // "none reports one" case).
    @Test func activeLongestPicksMaxAgeAcrossHostsAndIsNilWhenNoneReport() {
        let shorter = Self.response(activeLongest: APIActiveLongest(runID: "run_short", workflowName: "wf", ageMs: 5_000))
        let longer = Self.response(activeLongest: APIActiveLongest(runID: "run_long", workflowName: "wf", ageMs: 500_000))

        let merged = DashboardStore.merge([shorter, longer])
        #expect(merged.activeLongest?.runID == "run_long")
        #expect(merged.activeLongest?.ageMs == 500_000)

        let none = DashboardStore.merge([Self.response()])
        #expect(none.activeLongest == nil)
    }

    // Ports `issues_capped_ors_across_hosts_and_inventory_stamp_takes_the_oldest`.
    @Test func issuesCappedOrsAcrossHostsAndInventoryStampTakesTheOldest() {
        let older = "2026-08-20T09:40:00Z"
        let newer = "2026-08-20T10:00:00Z"
        let a = Self.response(fleet: APIFleetCounts(
            repos: nil, providersConfigured: nil, providersUnhealthy: nil, autoflowsEnabled: nil, autoflowsDisabled: nil,
            workers: nil, claimsActive: nil, issuesPending: nil, issuesOpen: 300, issuesCapped: true, inventoryCapturedAt: older
        ))
        let b = Self.response(fleet: APIFleetCounts(
            repos: nil, providersConfigured: nil, providersUnhealthy: nil, autoflowsEnabled: nil, autoflowsDisabled: nil,
            workers: nil, claimsActive: nil, issuesPending: nil, issuesOpen: 12, issuesCapped: false, inventoryCapturedAt: newer
        ))

        let merged = DashboardStore.merge([a, b])

        #expect(merged.fleet.issuesOpen == 312)
        #expect(merged.fleet.issuesCapped, "one capped host makes the merged open count a floor")
        #expect(merged.fleet.inventoryCapturedAt == older, "the honest staleness bound is the OLDEST contributing cache")
    }

    // `capturedAt` (top-level) is the OLDEST reporting host's timestamp, not
    // the newest — same rule as `inventoryCapturedAt`. A `nil` contributor
    // (Task 1's "unknown, never fresh" tolerance) never participates in the
    // comparison at all — it neither wins as "oldest" nor is treated as
    // missing data that blocks the other hosts' timestamps from being used.
    @Test func capturedAtIsTheOldestReportingHostAndNilContributorsNeverParticipate() {
        let newer = Self.response(hostID: "a", capturedAt: "2026-08-20T12:00:00Z")
        let older = Self.response(hostID: "b", capturedAt: "2026-08-18T00:00:00Z")
        let unknown = Self.response(hostID: "c", capturedAt: nil)

        let merged = DashboardStore.merge([newer, older, unknown])
        #expect(merged.capturedAt == "2026-08-18T00:00:00Z")

        let allUnknown = DashboardStore.merge([Self.response(capturedAt: nil)])
        #expect(allUnknown.capturedAt == nil, "must never fabricate a captured-at when no host reported one")
    }

    // Review fix (round 1, Important): a raw `String` `<` comparison gets
    // "oldest" backwards across mixed timestamp precision. The local
    // connector (and this repo's own fixture) emits whole-second timestamps
    // (`"...12:00:00Z"`); a remote host under real load can emit
    // fractional-second ones (`"...12:00:00.500000Z"`, matching the shape
    // CLI logs actually show, e.g. `"...07:00:59.397407Z"`). Lexicographically
    // `"...12:00:00.500000Z"` sorts BEFORE `"...12:00:00Z"` (a `.` sorts
    // before nothing at a shared prefix) despite being chronologically
    // LATER by half a second — so a naive string comparison would pick the
    // WRONG host as "oldest" here. Date-parsed comparison must get this
    // right in both directions (fractional-first-in-the-array and
    // whole-second-first).
    @Test func mixedPrecisionTimestampsCompareByActualDateNotLexicographicStringOrder() {
        let wholeSecond = "2026-08-20T12:00:00Z"
        let fractionalHalfSecondLater = "2026-08-20T12:00:00.500000Z"

        let a = DashboardStore.merge([
            Self.response(hostID: "whole", capturedAt: wholeSecond),
            Self.response(hostID: "fractional", capturedAt: fractionalHalfSecondLater),
        ])
        #expect(a.capturedAt == wholeSecond, "the whole-second timestamp is chronologically OLDER despite sorting lexicographically after the fractional one")

        // Same pair, reversed input order — the result must not depend on
        // which one happened to be merged first.
        let b = DashboardStore.merge([
            Self.response(hostID: "fractional", capturedAt: fractionalHalfSecondLater),
            Self.response(hostID: "whole", capturedAt: wholeSecond),
        ])
        #expect(b.capturedAt == wholeSecond)

        // Same regression for `fleet.inventoryCapturedAt`, which shares the
        // `oldest(_:_:)` helper.
        let c = DashboardStore.merge([
            Self.response(fleet: APIFleetCounts(
                repos: nil, providersConfigured: nil, providersUnhealthy: nil, autoflowsEnabled: nil, autoflowsDisabled: nil,
                workers: nil, claimsActive: nil, issuesPending: nil, issuesOpen: nil, issuesCapped: false,
                inventoryCapturedAt: fractionalHalfSecondLater
            )),
            Self.response(fleet: APIFleetCounts(
                repos: nil, providersConfigured: nil, providersUnhealthy: nil, autoflowsEnabled: nil, autoflowsDisabled: nil,
                workers: nil, claimsActive: nil, issuesPending: nil, issuesOpen: nil, issuesCapped: false,
                inventoryCapturedAt: wholeSecond
            )),
        ])
        #expect(c.fleet.inventoryCapturedAt == wholeSecond)
    }

    // Terminal/throughput buckets for the SAME `ts` across two hosts must
    // merge into exactly one bucket, summed field by field — the client-side
    // analogue of `local_and_ssh_shaped_terminal_buckets_for_the_same_day_merge_into_one`.
    // A different `ts` must NOT merge.
    @Test func bucketsForTheSameTimestampMergeIntoOneSummedBucketAndDifferentTimestampsDoNot() {
        let day = "2026-08-15T00:00:00Z"
        let otherDay = "2026-08-16T00:00:00Z"
        let a = Self.response(
            terminalBuckets: [APITerminalBucket(ts: day, completed: 3, failed: 0, rejected: 0, cancelled: 0)],
            throughputBuckets: [APIThroughputBucket(ts: day, manual: 2, cron: 1, event: 0)],
            findingsOpen: 0
        )
        let b = Self.response(
            terminalBuckets: [
                APITerminalBucket(ts: day, completed: 2, failed: 1, rejected: 0, cancelled: 0),
                APITerminalBucket(ts: otherDay, completed: 1, failed: 0, rejected: 0, cancelled: 0),
            ],
            throughputBuckets: [APIThroughputBucket(ts: day, manual: 1, cron: 0, event: 3)],
            findingsOpen: nil
        )

        let merged = DashboardStore.merge([a, b])

        let dayTerminal = merged.terminalBuckets.filter { $0.ts == day }
        #expect(dayTerminal.count == 1, "same-ts buckets across hosts must merge into exactly one")
        #expect(dayTerminal.first?.completed == 5)
        #expect(dayTerminal.first?.failed == 1)
        #expect(merged.terminalBuckets.contains { $0.ts == otherDay && $0.completed == 1 })

        let dayThroughput = merged.throughputBuckets.filter { $0.ts == day }
        #expect(dayThroughput.count == 1)
        #expect(dayThroughput.first?.manual == 3)
        #expect(dayThroughput.first?.event == 3)

        #expect(merged.findingsPartial, "the second host reported nil findings")
    }

    // Brief requirement: partial flags also OR in each response's OWN
    // already-reported partial flag, not just the field-level `nil`
    // detection this function does itself. Constructed so every raw value
    // is non-`nil` (field-level detection alone would find nothing to
    // flag) — proving the OR-in path is real, not coincidentally matching.
    @Test func partialFlagsAlsoOrInEachResponsesOwnReportedFlagRegardlessOfFieldLevelNils() {
        let response = Self.response(
            findingsPartial: true,
            cyclesPartial: true,
            fleetPartial: true,
            cycles: APICycleCounts(total: 1, clean: 1, withFailures: 1),
            findingsOpen: 5,
            fleet: APIFleetCounts(
                repos: 1, providersConfigured: 1, providersUnhealthy: 1, autoflowsEnabled: 1, autoflowsDisabled: 1,
                workers: 1, claimsActive: 1, issuesPending: 1, issuesOpen: 1, issuesCapped: false, inventoryCapturedAt: nil
            )
        )

        let merged = DashboardStore.merge([response])

        #expect(merged.findingsPartial, "must OR in the response's own findingsPartial even though findingsOpen is non-nil")
        #expect(merged.cyclesPartial, "must OR in the response's own cyclesPartial even though clean/withFailures are non-nil")
        #expect(merged.fleetPartial, "must OR in the response's own fleetPartial even though every fleet field is non-nil")
    }

    // Ports `fleet_is_not_partial_when_no_host_reports_at_all` +
    // `findings_open_is_none_and_not_partial_when_no_host_reports_at_all`:
    // zero inputs means there is nothing to be partial ABOUT.
    @Test func emptyInputYieldsAnEmptyAggregateWithNoPartialFlags() {
        let merged = DashboardStore.merge([])

        #expect(merged.active == Self.zeroActive)
        #expect(merged.activeLongest == nil)
        #expect(merged.terminalBuckets.isEmpty)
        #expect(merged.throughputBuckets.isEmpty)
        #expect(merged.cycles.total == 0)
        #expect(merged.cycles.clean == nil)
        #expect(merged.cycles.withFailures == nil)
        #expect(merged.findingsOpen == nil)
        #expect(merged.fleet.workers == nil)
        #expect(!merged.fleet.issuesCapped)
        #expect(!merged.findingsPartial, "nothing to be partial about with zero reporting hosts")
        #expect(!merged.cyclesPartial)
        #expect(!merged.fleetPartial)
        #expect(merged.capturedAt == nil)
    }
}

// MARK: - Lifecycle tests (mock CPClient over `DashboardStubURLProtocol`)

@Suite(.serialized)
struct DashboardStoreTests {
    private static func hostsJSON(_ hosts: [(id: String, status: String)]) -> Data {
        let items = hosts.map { #"{"id":"\#($0.id)","name":"\#($0.id)","transport_kind":"local","status":"\#($0.status)"}"# }
        return Data("[\(items.joined(separator: ","))]".utf8)
    }

    /// Builds a single-host `GET /api/dashboard` response body. `workers`
    /// is a plain marker value tests read back off `merged.fleet.workers`
    /// to prove which host's (and which cycle's) response actually landed.
    private static func dashboardJSON(
        hostID: String, state: String, capturedAt: String? = nil, reason: String? = nil, workers: Int? = nil
    ) -> Data {
        let capturedJSON = capturedAt.map { #""\#($0)""# } ?? "null"
        let reasonJSON = reason.map { #""\#($0)""# } ?? "null"
        let workersJSON = workers.map(String.init) ?? "null"
        let json = """
        {
          "hosts": [{"host_id":"\(hostID)","name":"\(hostID)","transport_kind":"local","state":"\(state)","captured_at":\(capturedJSON),"reason":\(reasonJSON)}],
          "findings_partial": false,
          "cycles_partial": false,
          "fleet_partial": false,
          "active": {"running": 0, "awaiting_approval": 0, "paused": 0, "pending": 0},
          "active_longest": null,
          "terminal_buckets": [],
          "throughput_buckets": [],
          "cycles": {"total": 0, "clean": null, "with_failures": null},
          "findings_open": null,
          "fleet": {"repos": null, "providers_configured": null, "providers_unhealthy": null, "autoflows_enabled": null, "autoflows_disabled": null, "workers": \(workersJSON), "claims_active": null, "issues_pending": null, "issues_open": null, "issues_capped": false, "inventory_captured_at": null},
          "captured_at": \(capturedJSON)
        }
        """
        return Data(json.utf8)
    }

    private static func queryValue(_ name: String, in req: URLRequest) -> String? {
        guard let url = req.url else { return nil }
        return URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first(where: { $0.name == name })?.value
    }

    @MainActor
    private func makeStore(
        debounceInterval: Duration = .milliseconds(250),
        reconcileInterval: Duration = .seconds(60),
        respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)
    ) -> (store: DashboardStore, box: SignalsFactoryBox) {
        DashboardStubURLProtocol.reset(handler: respond)
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: DashboardStubURLProtocol.session()
        )
        let box = SignalsFactoryBox()
        let store = DashboardStore(
            client: client, signalsFactory: { box.factory() },
            debounceInterval: debounceInterval, reconcileInterval: reconcileInterval
        )
        return (store, box)
    }

    // (a) `activate(range:)` returns once `"local"`'s fetch has landed —
    // `merged` already reflects local truth before a slow online remote
    // host has answered at all. The remote host's contribution then merges
    // in progressively once its artificially delayed response arrives.
    @MainActor @Test func activateRendersLocalImmediatelyThenMergesSlowRemoteHostProgressively() async {
        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online"), ("mini", "online")]))
            }
            guard url.path == "/api/dashboard" else { return (200, Data("{}".utf8)) }
            let hostID = Self.queryValue("host", in: req) ?? ""
            if hostID == "mini" {
                Thread.sleep(forTimeInterval: 0.08) // artificially slow remote host
                return (200, Self.dashboardJSON(hostID: "mini", state: "ok", capturedAt: "2026-08-20T09:00:00Z", workers: 5))
            }
            return (200, Self.dashboardJSON(hostID: "local", state: "ok", capturedAt: "2026-08-20T10:00:00Z", workers: 1))
        }

        await store.activate(range: .d7)

        // Local truth already showing; the slow remote host hasn't been
        // waited on at all.
        #expect(store.merged?.fleet.workers == 1)
        #expect(store.hostStates.first(where: { $0.id == "mini" })?.state == .loading)

        await expectEventually("the slow remote host's contribution merges in") {
            store.merged?.fleet.workers == 6
        }

        #expect(store.merged?.fleet.workers == 6, "1 (local) + 5 (mini)")
        #expect(store.hostStates.first(where: { $0.id == "mini" })?.state == .ok(capturedAt: "2026-08-20T09:00:00Z"))
        // The oldest capturedAt (mini's) wins.
        #expect(store.merged?.capturedAt == "2026-08-20T09:00:00Z")
        #expect(store.pageError == nil)

        store.deactivate()
        box.latest.finish()
    }

    // (b) A remote host whose HTTP call itself fails (transport/decoding/
    // non-2xx) becomes `.unavailable(reason:)` — `merged` stays exactly
    // what local alone produced, never blocked or corrupted by the
    // failure.
    @MainActor @Test func failingRemoteHostBecomesUnavailableAndMergedStaysLocalOnly() async {
        let (store, _) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online"), ("mini", "online")]))
            }
            guard url.path == "/api/dashboard" else { return (200, Data("{}".utf8)) }
            let hostID = Self.queryValue("host", in: req) ?? ""
            if hostID == "mini" {
                return (500, Data(#"{"error":"boom"}"#.utf8))
            }
            return (200, Self.dashboardJSON(hostID: "local", state: "ok", capturedAt: "2026-08-20T10:00:00Z", workers: 1))
        }

        await store.activate(range: .d7)

        await expectEventually("the failing remote host's slice resolves") {
            if case .unavailable = store.hostStates.first(where: { $0.id == "mini" })?.state { return true }
            return false
        }

        guard case .unavailable(let reason) = store.hostStates.first(where: { $0.id == "mini" })?.state else {
            Issue.record("expected mini to be .unavailable")
            return
        }
        #expect(reason != nil && !(reason ?? "").isEmpty)
        #expect(store.merged?.fleet.workers == 1, "the failing host must contribute nothing")
        #expect(store.pageError == nil, "local alone reported ok — no page-wide error")

        store.deactivate()
    }

    // (c) A host whose HTTP call SUCCEEDS but whose own per-host
    // `hosts[].state` inside the 200 body is `"offline"` (the server
    // resolved the connector and found it down) must ALSO become
    // `.offline` and contribute nothing — distinct code path from (b)'s
    // transport-level failure.
    @MainActor @Test func serverReportedOfflineStateBecomesOfflineSliceAndContributesNothing() async {
        let (store, _) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online"), ("kuki", "online")]))
            }
            guard url.path == "/api/dashboard" else { return (200, Data("{}".utf8)) }
            let hostID = Self.queryValue("host", in: req) ?? ""
            if hostID == "kuki" {
                return (200, Self.dashboardJSON(hostID: "kuki", state: "offline", reason: "connection refused"))
            }
            return (200, Self.dashboardJSON(hostID: "local", state: "ok", capturedAt: "2026-08-20T10:00:00Z", workers: 1))
        }

        await store.activate(range: .d7)

        await expectEventually("kuki's slice resolves to offline") {
            store.hostStates.first(where: { $0.id == "kuki" })?.state == .offline
        }

        #expect(store.hostStates.first(where: { $0.id == "kuki" })?.state == .offline)
        #expect(store.merged?.fleet.workers == 1)

        store.deactivate()
    }

    // (d) Zero `"ok"` hosts, all resolved: `merged` stays `nil` and
    // `pageError` is set — honest "nothing could be reported" rather than
    // a blank aggregate that reads as a fleet with zero of everything.
    @MainActor @Test func zeroOkHostsAfterAllResolveSetsPageErrorAndLeavesMergedNil() async {
        let (store, _) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online"), ("kuki", "online")]))
            }
            guard url.path == "/api/dashboard" else { return (200, Data("{}".utf8)) }
            let hostID = Self.queryValue("host", in: req) ?? ""
            if hostID == "kuki" {
                return (200, Self.dashboardJSON(hostID: "kuki", state: "offline", reason: "connection refused"))
            }
            return (500, Data(#"{"error":"boom"}"#.utf8))
        }

        await store.activate(range: .d7)

        await expectEventually("both hosts resolve and pageError lands") {
            store.pageError != nil
        }

        #expect(store.merged == nil)
        #expect(store.pageError != nil)

        store.deactivate()
    }

    // (e) `setRange(_:)` refetches every host from scratch; a stale
    // in-flight result from the SUPERSEDED generation (a slow response for
    // the OLD range) must never land, even once it finally resolves well
    // after the new range's own fetch already completed.
    @MainActor @Test func setRangeRefetchesAllAndDropsStaleInFlightResultsFromThePriorGeneration() async {
        let (store, _) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online"), ("mini", "online")]))
            }
            guard url.path == "/api/dashboard" else { return (200, Data("{}".utf8)) }
            let hostID = Self.queryValue("host", in: req) ?? ""
            let range = Self.queryValue("range", in: req) ?? ""
            if hostID == "mini" && range == "7d" {
                // The stale generation's remote fetch: deliberately slow,
                // and its marker value (999) must never be observed.
                Thread.sleep(forTimeInterval: 0.15)
                return (200, Self.dashboardJSON(hostID: "mini", state: "ok", workers: 999))
            }
            if hostID == "mini" && range == "30d" {
                return (200, Self.dashboardJSON(hostID: "mini", state: "ok", workers: 7))
            }
            // local, any range — instant.
            return (200, Self.dashboardJSON(hostID: "local", state: "ok", workers: 1))
        }

        await store.activate(range: .d7) // fires mini's slow 7d fetch in the background
        #expect(store.merged?.fleet.workers == 1) // local only, so far

        await store.setRange(.d30) // bumps generation; local(30d) lands, returns once it does
        #expect(store.merged?.fleet.workers == 1, "still local-only immediately after setRange returns")

        await expectEventually("mini's 30d fetch merges in") {
            store.merged?.fleet.workers == 8
        }
        #expect(store.merged?.fleet.workers == 8, "1 (local) + 7 (mini, 30d) — never 999")

        // Give the stale 7d fetch's artificial 0.15s delay time to actually
        // resolve, well past its window, and confirm it never corrupted
        // the now-settled 30d state.
        try? await Task.sleep(for: .milliseconds(200))
        #expect(store.merged?.fleet.workers == 8, "the stale generation's 999 marker must never land")

        store.deactivate()
    }

    // (f) `deactivate()` tears the reconcile loop down — asserted via the
    // internal `reconcileTask` reference (`@testable import`) going `nil`,
    // not via timing (the loop's periodic firing is deliberately not
    // timing-tested; see the type's doc comment).
    @MainActor @Test func deactivateCancelsTheReconcileLoop() async {
        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online")]))
            }
            return (200, Self.dashboardJSON(hostID: "local", state: "ok", workers: 1))
        }

        await store.activate(range: .d7)
        #expect(store.reconcileTask != nil)

        store.deactivate()
        #expect(store.reconcileTask == nil)

        box.latest.finish()
    }

    // (g) A firehose signal schedules a debounced LOCAL-ONLY refetch — a
    // burst of several signals back to back coalesces into exactly one
    // extra `/api/dashboard?host=local` hit, and the remote host is never
    // touched by it at all.
    @MainActor @Test func firehoseSignalBurstCoalescesIntoOneDebouncedLocalOnlyRefresh() async {
        let (store, box) = makeStore(debounceInterval: .milliseconds(20)) { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online"), ("mini", "online")]))
            }
            guard url.path == "/api/dashboard" else { return (200, Data("{}".utf8)) }
            let hostID = Self.queryValue("host", in: req) ?? ""
            if hostID == "mini" {
                return (200, Self.dashboardJSON(hostID: "mini", state: "ok", workers: 5))
            }
            return (200, Self.dashboardJSON(hostID: "local", state: "ok", workers: 1))
        }

        await store.activate(range: .d7)
        await expectEventually("mini's first fetch lands") { store.merged?.fleet.workers == 6 }
        let localHitsAfterActivate = DashboardStubURLProtocol.hits("/api/dashboard")

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runStarted(runID: "r1", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:00Z")))
        box.latest.yield(.event(.runStarted(runID: "r2", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:01Z")))
        box.latest.yield(.event(.runStarted(runID: "r3", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:02Z")))

        await expectEventually("the coalesced debounced local refresh lands") {
            DashboardStubURLProtocol.hits("/api/dashboard") == localHitsAfterActivate + 1
        }
        #expect(DashboardStubURLProtocol.hits("/api/dashboard") == localHitsAfterActivate + 1, "exactly one extra hit from the whole burst")
        #expect(store.merged?.fleet.workers == 6, "mini's already-loaded contribution is untouched by the local-only refresh")

        // Settle further and confirm it never overshoots to a second hit.
        try? await Task.sleep(for: .milliseconds(100))
        #expect(DashboardStubURLProtocol.hits("/api/dashboard") == localHitsAfterActivate + 1)

        store.deactivate()
        box.latest.finish()
    }

    // (h) Review fix (round 1, minor): a host already `.ok` from a prior
    // cycle must stay `.ok` (showing its LAST-known-good data) through a
    // subsequent full refetch cycle — never flash back to `.loading` — and
    // only actually change once that new cycle's own fetch for it lands.
    // `setRange(_:)` (same range) drives the identical `refetchAll()` engine
    // the 60s reconcile loop uses, so it stands in for "a reconcile tick
    // fires" without needing to wait out a real interval.
    @MainActor @Test func existingOkSliceStaysOkThroughARefetchCycleUntilItsNewResultLands() async {
        let miniHits = HitCounter()
        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online"), ("mini", "online")]))
            }
            guard url.path == "/api/dashboard" else { return (200, Data("{}".utf8)) }
            let hostID = Self.queryValue("host", in: req) ?? ""
            if hostID == "mini" {
                let hit = miniHits.incrementAndGet()
                if hit == 1 {
                    return (200, Self.dashboardJSON(hostID: "mini", state: "ok", capturedAt: "2026-08-20T09:00:00Z", workers: 5))
                }
                // The second cycle's own fetch for mini: artificially slow,
                // so the test can observe the slice mid-cycle before it
                // lands.
                Thread.sleep(forTimeInterval: 0.08)
                return (200, Self.dashboardJSON(hostID: "mini", state: "ok", capturedAt: "2026-08-20T09:05:00Z", workers: 9))
            }
            return (200, Self.dashboardJSON(hostID: "local", state: "ok", workers: 1))
        }

        await store.activate(range: .d7)
        await expectEventually("mini's first fetch lands") { store.merged?.fleet.workers == 6 }
        guard case .ok(let firstCapturedAt) = store.hostStates.first(where: { $0.id == "mini" })?.state else {
            Issue.record("expected mini to be .ok after the first cycle")
            return
        }
        #expect(firstCapturedAt == "2026-08-20T09:00:00Z")

        // A second full cycle — the same engine the 60s reconcile loop
        // drives — fired but not yet awaited, so the test can inspect
        // mid-flight state before mini's own (slow) refetch resolves.
        let refetchTask = Task { await store.setRange(.d7) }

        // Let the cycle actually start (host discovery + local's instant
        // refetch) without waiting anywhere near mini's 80ms artificial
        // delay.
        try? await Task.sleep(for: .milliseconds(20))
        guard case .ok(let midCycleCapturedAt) = store.hostStates.first(where: { $0.id == "mini" })?.state else {
            Issue.record("mini's slice must stay .ok mid-cycle, got \(String(describing: store.hostStates.first(where: { $0.id == "mini" })?.state))")
            return
        }
        #expect(midCycleCapturedAt == "2026-08-20T09:00:00Z", "must still show the FIRST cycle's data — never flash to .loading — until the new fetch actually lands")

        // `setRange(_:)` itself already returned once LOCAL's (fast) refetch
        // landed — mini's own fetch runs on independently, same
        // local-first/remote-progressive shape `activate(range:)` uses — so
        // wait for mini's slice specifically, not for `refetchTask` (which
        // may already be done).
        await expectEventually("mini's second cycle fetch lands") {
            store.merged?.fleet.workers == 10
        }
        await refetchTask.value

        guard case .ok(let secondCapturedAt) = store.hostStates.first(where: { $0.id == "mini" })?.state else {
            Issue.record("expected mini to be .ok after the second cycle landed")
            return
        }
        #expect(secondCapturedAt == "2026-08-20T09:05:00Z", "the new fetch's result must land once it resolves")
        #expect(store.merged?.fleet.workers == 10, "1 (local) + 9 (mini's fresh value)")

        store.deactivate()
        box.latest.finish()
    }

    // (i) Review fix (round 1, minor): calling `activate(range:)` a second
    // time must CANCEL the first reconcile loop rather than run a second
    // one alongside it — otherwise a screen re-appearing (calling
    // `activate()` more than once) would silently accumulate duplicate
    // background loops, each independently re-fetching the whole fleet on
    // its own cadence. Proven via the reconcile loop's actual tick RATE
    // over a short, overridden interval (not the loop's literal production
    // timing, which stays untested per the type's doc comment) — a
    // duplicated, never-cancelled first loop would roughly DOUBLE the
    // `/api/hosts` hit rate during the observation window.
    @MainActor @Test func activateTwiceReplacesTheReconcileLoopRatherThanRunningItTwice() async {
        // Margins deliberately wide (interval 60ms / window 900ms): tighter
        // values (25/260) are correct on an idle machine but this suite has
        // twice been bitten by Task.sleep under-scheduling during parallel
        // full-suite runs (see 67085ede) — the discriminator here is the
        // ~2x hit-rate gap between one loop and two, which survives wide
        // margins just as well.
        let intervalMS = 60
        let (store, box) = makeStore(reconcileInterval: .milliseconds(intervalMS)) { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online")]))
            }
            return (200, Self.dashboardJSON(hostID: "local", state: "ok", workers: 1))
        }

        await store.activate(range: .d7)
        #expect(store.reconcileTask != nil)

        await store.activate(range: .d7) // must replace, not add, a second concurrent loop
        #expect(store.reconcileTask != nil)

        let hitsBefore = DashboardStubURLProtocol.hits("/api/hosts")
        let windowMS = 900 // ~15 intervals at 60ms for a single surviving loop
        try? await Task.sleep(for: .milliseconds(windowMS))
        let hits = DashboardStubURLProtocol.hits("/api/hosts") - hitsBefore

        #expect(hits >= 1, "the surviving loop must still be ticking")
        // A single loop fires ~windowMS/intervalMS times over the window; a
        // duplicated, uncancelled first loop would fire roughly TWICE that.
        // 22 sits well above the single-loop expectation (~15, allowing for
        // scheduler jitter) and well below the ~30 a real duplicate would
        // produce.
        #expect(hits <= 22, "hit rate (\(hits) hits in \(windowMS)ms at a \(intervalMS)ms interval) suggests a duplicated, uncancelled first reconcile loop")

        store.deactivate()
        box.latest.finish()
    }

    // (j) Final-review fix (Task 1): a THROWN `GET /api/hosts` call on a
    // mid-life cycle (the 60s reconcile tick — stood in for here by a second
    // `setRange(_:)` call, same engine) must not be conflated with a
    // genuinely empty fleet. The prior `(try? await client.hosts()) ?? []`
    // idiom reseeded `hostStates`/`okResponses`/`merged` to empty on a
    // transient blip and stamped a lying "no hosts registered" `pageError` —
    // this proves the cycle now leaves every existing slice untouched
    // instead, and that the very next successful cycle still works
    // (recovers), rather than the store being permanently wedged by the
    // fix.
    @MainActor @Test func hostsFetchFailureOnAMidLifeCycleLeavesExistingStateUntouchedAndRecoversNextCycle() async {
        let hostsAttempts = HitCounter()
        let (store, _) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                let attempt = hostsAttempts.incrementAndGet()
                if attempt == 2 {
                    // The mid-life (second) cycle's own host-discovery call:
                    // fails outright.
                    return (500, Data(#"{"error":"boom"}"#.utf8))
                }
                return (200, Self.hostsJSON([("local", "online")]))
            }
            return (200, Self.dashboardJSON(hostID: "local", state: "ok", capturedAt: "2026-08-20T10:00:00Z", workers: 1))
        }

        await store.activate(range: .d7)
        #expect(store.merged?.fleet.workers == 1)
        #expect(store.pageError == nil)
        let hostStatesBeforeFailure = store.hostStates
        let mergedBeforeFailure = store.merged

        // Second cycle: `GET /api/hosts` throws. Must be a pure no-op for
        // every piece of client-visible state.
        await store.setRange(.d7)
        #expect(store.hostStates == hostStatesBeforeFailure, "a thrown hosts() fetch must not reseed hostStates")
        #expect(store.merged == mergedBeforeFailure, "must keep the last-known-good merged aggregate")
        #expect(store.pageError == nil, "must never lie that no hosts are registered on a transient hosts() failure")

        // Third cycle: `GET /api/hosts` succeeds again — the store recovers
        // on its own, not permanently wedged by the failure-handling fix.
        await store.setRange(.d7)
        #expect(store.merged?.fleet.workers == 1)
        #expect(store.pageError == nil)

        store.deactivate()
    }

    // (k) Final-review fix (Task 4): `pendingFetchCount`'s old raw-counter
    // accounting could be double-decremented by a debounced local-only
    // refresh landing mid-cycle (`performFetch` is reachable from both the
    // cycle proper and `refreshLocalOnly()`), which could flash a false
    // "no host reported dashboard data" `pageError` while a slow remote
    // host was still genuinely in flight. Local fails immediately every
    // time it's asked; mini is deliberately slow but WILL eventually report
    // ok — an extra local-only refresh (fired via a firehose signal, same
    // burst-coalescing path task (g) exercises) landing while mini is still
    // pending must never manufacture a premature "nothing reported" verdict.
    @MainActor @Test func debouncedLocalOnlyRefreshDuringAFailingLocalNeverFalselyFlashesPageErrorWhileARemoteHostIsStillPending() async {
        let (store, box) = makeStore(debounceInterval: .milliseconds(20)) { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("local", "online"), ("mini", "online")]))
            }
            guard url.path == "/api/dashboard" else { return (200, Data("{}".utf8)) }
            let hostID = Self.queryValue("host", in: req) ?? ""
            if hostID == "local" {
                return (500, Data(#"{"error":"boom"}"#.utf8))
            }
            // mini: slow but eventually ok — still in flight when the extra
            // local-only refresh below lands.
            Thread.sleep(forTimeInterval: 0.12)
            return (200, Self.dashboardJSON(hostID: "mini", state: "ok", workers: 5))
        }

        await store.activate(range: .d7)
        #expect(store.pageError == nil, "mini hasn't resolved yet — too early to say nothing reported")

        // A firehose signal schedules the debounced local-only refresh,
        // which re-fetches (and re-fails) local entirely outside the
        // cycle's own pending-tracking, while mini is still in flight.
        box.latest.yield(.connection(true))
        try? await Task.sleep(for: .milliseconds(60)) // let the debounced refresh land and settle

        #expect(store.pageError == nil, "the extra local-only refresh must not manufacture false completion while mini is still pending")

        await expectEventually("mini's slow fetch eventually lands") {
            store.merged?.fleet.workers == 5
        }
        #expect(store.pageError == nil, "mini reported ok — never an error")

        store.deactivate()
        box.latest.finish()
    }
}
