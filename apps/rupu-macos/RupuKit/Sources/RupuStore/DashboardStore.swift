import Foundation
import Observation
import RupuAPI

/// One host's reporting state in the Overview screen's freshness strip.
/// Seeded from `GET /api/hosts` immediately (`.loading` for every row), then
/// updated in place as each host's own `GET /api/dashboard?host=<id>` call
/// resolves. `state` is deliberately three-valued past `.loading`, mirroring
/// the server's own `HostFreshness.state` (`"ok"` | `"offline"` |
/// `"unavailable"`, see `crates/rupu-cp/src/api/dashboard.rs`): a host that
/// cannot report is NOT a host with zero activity, so `.offline`/
/// `.unavailable` must never be read as zeroed counts.
public struct HostSlice: Equatable, Sendable {
    public let id: String
    public let name: String
    public let transportKind: String
    public var state: SliceState

    public init(id: String, name: String, transportKind: String, state: SliceState) {
        self.id = id
        self.name = name
        self.transportKind = transportKind
        self.state = state
    }
}

public enum SliceState: Equatable, Sendable {
    case loading
    case ok(capturedAt: String?)
    case offline
    case unavailable(reason: String?)
}

/// The client-side fleet-wide aggregate — the exact shape (and merge
/// semantics) of `crates/rupu-cp/src/api/dashboard.rs`'s
/// `merge_dashboard_summaries`, ported so `DashboardStore` can fan a
/// `host=<id>`-scoped `GET /api/dashboard` call out per host (progressive,
/// per-host loading, same model as `ActivityStore`) instead of relying on
/// the server's own no-`host` fan-out, which is sequential server-side and
/// turns one slow/offline host into a several-second stall for the whole
/// screen — the exact class of bug `ActivityStore`'s per-host loading was
/// built to avoid. See `DashboardStore.merge(_:)`'s doc comment for the
/// field-by-field semantics.
public struct MergedDashboard: Equatable, Sendable {
    public let active: APIActiveCounts
    public let activeLongest: APIActiveLongest?
    public let terminalBuckets: [APITerminalBucket]
    public let throughputBuckets: [APIThroughputBucket]
    public let cycles: APICycleCounts
    public let findingsOpen: Int?
    public let fleet: APIFleetCounts
    public let findingsPartial: Bool
    public let cyclesPartial: Bool
    public let fleetPartial: Bool
    public let capturedAt: String?

    public init(
        active: APIActiveCounts,
        activeLongest: APIActiveLongest?,
        terminalBuckets: [APITerminalBucket],
        throughputBuckets: [APIThroughputBucket],
        cycles: APICycleCounts,
        findingsOpen: Int?,
        fleet: APIFleetCounts,
        findingsPartial: Bool,
        cyclesPartial: Bool,
        fleetPartial: Bool,
        capturedAt: String?
    ) {
        self.active = active
        self.activeLongest = activeLongest
        self.terminalBuckets = terminalBuckets
        self.throughputBuckets = throughputBuckets
        self.cycles = cycles
        self.findingsOpen = findingsOpen
        self.fleet = fleet
        self.findingsPartial = findingsPartial
        self.cyclesPartial = cyclesPartial
        self.fleetPartial = fleetPartial
        self.capturedAt = capturedAt
    }
}

/// Per-host progressive fetch for the Overview screen's dashboard, plus the
/// client-side port of `merge_dashboard_summaries`'s aggregation semantics.
///
/// **Why this exists at all** (rather than the app just calling `GET
/// /api/dashboard` with no `host` and letting the server fan out): the
/// server's own fan-out is sequential across hosts (`futures_util::
/// future::join_all` over per-host awaits still means the whole response
/// waits on the *slowest* one), so a fleet with one offline node turns the
/// Overview's load into the exact multi-second stall `ActivityStore`'s
/// per-host progressive loading was built to fix (see that type's doc
/// comment — matt's directive, verbatim: "lazy load things as you gather
/// them ... you should not fail all for a single host"). This store instead
/// issues one `host=<id>`-scoped call per registered host, applies each as
/// it lands, and re-runs `merge(_:)` — the exact aggregation the server
/// would have done for a full fan-out — over whichever hosts have answered
/// `"ok"` so far.
///
/// **Local-first, remote-progressive**, same shape as `ActivityStore.
/// activate(kind:)`: `activate(range:)` returns once the `"local"` host's
/// fetch has landed (so the screen never blocks on a remote host at all),
/// then fires every other seeded host's fetch as an independent `Task` —
/// one slow or offline host can never delay or fail another's.
///
/// **Failure honesty**: a host's `GET /api/dashboard?host=<id>` call can
/// fail two different ways, and both are handled, distinctly, per host:
/// - The HTTP call itself throws (transport/decoding/non-2xx) → that slice
///   goes `.unavailable(reason:)`, via the exact same message mapping every
///   mutation method in this module already uses for a thrown error.
/// - The call succeeds (200 JSON), but the server's own per-host
///   `HostFreshness.state` inside the response is `"offline"` or
///   `"unavailable"` (the connector resolution failed, or the host simply
///   doesn't support the endpoint) — see `dashboard.rs`'s doc comment on
///   why a non-reporting host must never collapse into zeroed counts. That
///   host's slice is set to `.offline` / `.unavailable(reason:)`
///   accordingly, from the response's own `hosts[]` entry.
///
/// Either way, a non-`"ok"` host contributes nothing to `merged` — it is
/// simply absent from the set `merge(_:)` is called over, never folded in
/// as a zero. If every seeded host has finished and none reported `"ok"`,
/// `merged` stays `nil` and `pageError` is set so the screen can show an
/// honest "nothing could be reported" state rather than a blank aggregate
/// that reads as "the fleet has zero of everything".
///
/// **Live refresh**: a firehose signal (any element off the
/// `signalsFactory()` stream — connection change or decoded event; this
/// store doesn't distinguish, unlike `ActivityStore`'s event-shape-aware
/// reduction, since a dashboard aggregate has no per-row identity to patch
/// in place) schedules a 250ms-coalesced **local-only** refetch — re-fires
/// `"local"`'s own `GET /api/dashboard?host=local` and re-merges, without
/// touching any already-loaded remote host's slice. A 60s reconcile loop
/// (weak-self-per-iteration, same idiom as `RupuBackend.HealthMonitor`)
/// re-runs the full per-host fetch cycle so a remote host's freshness (or a
/// fleet composition change) doesn't go stale between explicit reloads; the
/// loop itself is deliberately not timing-tested (same rationale as
/// `HostsFooterStore`'s 60s poll loop) — only the refetch it drives.
@MainActor
@Observable
public final class DashboardStore {
    public private(set) var hostStates: [HostSlice] = []
    public private(set) var merged: MergedDashboard?

    /// Set only once every seeded host's fetch for the current cycle has
    /// resolved and *none* of them reported `"ok"` — see the type's doc
    /// comment. Cleared the moment any host's response merges successfully
    /// (`recomputeMerged()`), so a page-load failure followed by a
    /// successful firehose-triggered local refresh doesn't leave a stale
    /// error banner standing.
    public private(set) var pageError: String?

    private let client: CPClient
    private let signalsFactory: @Sendable () -> AsyncStream<StreamSignal<CPEvent>>
    private let debounceInterval: Duration
    private let reconcileInterval: Duration

    private var range: TimeRange = .d7

    /// Bumped at the top of every `refetchAll()` (the shared engine behind
    /// `activate(range:)`, `setRange(_:)`, and the reconcile loop's tick)
    /// and captured by each host's fetch at the point it starts. A
    /// completion whose captured generation no longer matches
    /// `generation` belongs to a superseded cycle (a `setRange` call that
    /// landed while an older cycle's remote fetches were still in flight,
    /// or a `deactivate()`) and is dropped rather than mutating
    /// `hostStates`/`okResponses` out from under the current cycle.
    private var generation = 0

    /// Every host that has answered `"ok"` in the current cycle, keyed by
    /// host id — `merge(_:)` is re-run over `Array(okResponses.values)`
    /// every time any host's fetch resolves, so `merged` grows (or shrinks,
    /// on a host that flips from `ok` to erroring on a later refresh)
    /// progressively.
    private var okResponses: [String: APIDashboardResponse] = [:]

    /// Every seeded host id from the current cycle that has not yet resolved
    /// at least once (`.ok`, `.offline`, or `.unavailable` all count as
    /// resolved) — emptying it is what triggers `pageError` when
    /// `okResponses` is still empty. Deliberately a `Set`, not a counter
    /// (final-review fix, Task 4): `performFetch` is also reachable from
    /// `refreshLocalOnly()`'s debounced local-only refresh, which runs
    /// OUTSIDE the cycle this set was sized for. A raw counter decremented
    /// unconditionally on every `performFetch` call double-counts that extra
    /// refresh and can under-run the real cycle's total, flashing a false
    /// "no host reported dashboard data" `pageError` the moment the actual
    /// cycle's last host resolves. Removing a host id by id is idempotent —
    /// a stray extra refresh for a host already resolved this cycle is a
    /// harmless no-op instead of phantom progress.
    private var pendingHostIDs: Set<String> = []

    /// Every in-flight non-local host `Task` from the current cycle, so a
    /// new `refetchAll()` (and `deactivate()`) can cancel them outright
    /// rather than letting a now-superseded fetch run to completion in the
    /// background.
    private var hostTasks: [Task<Void, Never>] = []

    private var signalsTask: Task<Void, Never>?
    private var debounceTask: Task<Void, Never>?

    /// Internal (not `private`) so `DashboardStoreTests` can assert
    /// `deactivate()` actually tears the loop down, without needing to wait
    /// out a real interval to prove it — see the type's doc comment on why
    /// the loop's periodic firing itself is not timing-tested.
    var reconcileTask: Task<Void, Never>?

    private static let localHostID = "local"

    public init(
        client: CPClient,
        signalsFactory: @escaping @Sendable () -> AsyncStream<StreamSignal<CPEvent>>,
        debounceInterval: Duration = .milliseconds(250),
        reconcileInterval: Duration = .seconds(60)
    ) {
        self.client = client
        self.signalsFactory = signalsFactory
        self.debounceInterval = debounceInterval
        self.reconcileInterval = reconcileInterval
    }

    /// Seeds `hostStates`, fetches `"local"` first (this call returns once
    /// that lands — see the type's doc comment), fires every other seeded
    /// host's fetch progressively in the background, then (re)starts the
    /// firehose consumer and the 60s reconcile loop. Safe to call more than
    /// once — every call fully rebuilds both.
    public func activate(range: TimeRange) async {
        self.range = range
        await refetchAll()
        startSignalConsumer()
        startReconcileLoop()
    }

    /// Sets the range and refetches every seeded host from scratch — the
    /// same full cycle `activate(range:)` runs, just without touching the
    /// firehose consumer or the reconcile loop (both already running).
    public func setRange(_ newRange: TimeRange) async {
        self.range = newRange
        await refetchAll()
    }

    /// Stops the firehose consumer, cancels any pending debounced refresh,
    /// cancels the reconcile loop, and cancels every in-flight host fetch.
    /// Idempotent — safe to call more than once, or before `activate(range:)`
    /// ever ran.
    public func deactivate() {
        generation += 1
        cancelHostTasks()
        signalsTask?.cancel()
        signalsTask = nil
        debounceTask?.cancel()
        debounceTask = nil
        reconcileTask?.cancel()
        reconcileTask = nil
    }

    // MARK: - Merge (pure)

    /// Ports `merge_dashboard_summaries` (`crates/rupu-cp/src/api/
    /// dashboard.rs`) field-by-field. Each input is one host's own
    /// already-`host=`-scoped `APIDashboardResponse` — the server ran this
    /// exact aggregation for that single host already, so its own
    /// `findingsPartial`/`cyclesPartial`/`fleetPartial` are folded into the
    /// result (ORed in) alongside the field-level `nil` detection this
    /// function does itself over the raw values, rather than trusting only
    /// one source.
    ///
    /// Three distinct aggregation shapes, matching the Rust source exactly
    /// (not simplified to one rule — they really do differ):
    /// - `active` (running/awaitingApproval/paused/pending): plain sums —
    ///   every field is non-optional on the wire, so there is nothing to
    ///   poison.
    /// - `findingsOpen` and every `fleet` count: sum only the `Some`
    ///   (non-`nil`) contributions, and flag the matching partial bool on
    ///   any `nil` — but a `nil` from one host does NOT null out an
    ///   already-summed value from another. Two hosts, one reporting
    ///   `Some(3)` and one reporting `nil`, merge to `Some(3)` (partial),
    ///   never `nil`.
    /// - `cycles.clean`/`cycles.withFailures`: true poisoning. The first
    ///   `nil` any host contributes permanently pins the merged value to
    ///   `nil` for the rest of the merge — a later `Some`-reporting host
    ///   can never resurrect it into a truncated sum. `cycles.total` is
    ///   always a complete sum regardless (unaffected by poisoning).
    ///
    /// `activeLongest` is the max by `ageMs` across every host that has one
    /// (ties keep whichever was seen first). `capturedAt` (top-level) and
    /// `fleet.inventoryCapturedAt` are each the OLDEST non-`nil` value
    /// across hosts — the honest staleness bound, not the freshest, per
    /// `dashboard.rs`'s own doc comment. `issuesCapped` is a disjunction,
    /// not a sum: one capped host makes the whole aggregate a floor.
    /// Terminal/throughput buckets merge by exact `ts` match, summed field
    /// by field (no day-key re-normalization here — each host's own
    /// single-host server-side merge already zero-fills and aligns its own
    /// grid; this only combines grids that are already comparably shaped).
    ///
    /// Deviation from the Rust source: `capturedAt` has no `now` fallback
    /// when no host reports one (unlike the Rust merge, which falls back
    /// to `now` for its non-optional `DateTime` field) — a `nil` here
    /// means "unknown", and fabricating "now" would dishonestly claim
    /// freshness this store has no basis for (matches Task 1's note that a
    /// missing `capturedAt` must read as unknown, never fresh).
    public nonisolated static func merge(_ summaries: [APIDashboardResponse]) -> MergedDashboard {
        var runningSum = 0
        var awaitingSum = 0
        var pausedSum = 0
        var pendingSum = 0
        var activeLongest: APIActiveLongest?

        var cyclesTotal = 0
        var cyclesClean: Int?
        var cyclesWithFailures: Int?
        var cleanPoisoned = false
        var withFailuresPoisoned = false

        var findingsOpen: Int?
        var findingsPartial = false
        var fleetPartial = false
        var cyclesPartialFromResponses = false

        var repos: Int?
        var providersConfigured: Int?
        var providersUnhealthy: Int?
        var autoflowsEnabled: Int?
        var autoflowsDisabled: Int?
        var workers: Int?
        var claimsActive: Int?
        var issuesPending: Int?
        var issuesOpen: Int?
        var issuesCapped = false
        var inventoryCapturedAt: String?

        var terminalByTS: [String: APITerminalBucket] = [:]
        var throughputByTS: [String: APIThroughputBucket] = [:]
        var oldestCapturedAt: String?

        func sumOptional(_ acc: inout Int?, _ value: Int?) {
            guard let value else {
                fleetPartial = true
                return
            }
            acc = (acc ?? 0) + value
        }

        // Review fix (round 1, Important): raw `String` `<` comparison is
        // WRONG for RFC 3339 timestamps of differing precision — chrono
        // (server-side) emits a fractional-seconds suffix only when nanos
        // are non-zero, and both shapes are real in this repo (the fixture
        // uses whole-second `"...12:00:00Z"`; CLI logs show fractional
        // `"...07:00:59.397407Z"`). Lexicographically `"12:00:00.5Z"` sorts
        // BEFORE `"12:00:00Z"` (`.` < nothing, i.e. the shorter string wins
        // a prefix tie) despite being chronologically LATER — the exact
        // inversion that would make "oldest" silently wrong for a fleet
        // mixing precisions. Parse both sides to `Date` and compare those
        // instead.
        //
        // Final-review fix (Task 3): this used to be a locally-duplicated
        // fractional-then-plain `ISO8601DateFormatter` pair — identical,
        // third-time-duplicated logic to `ActivityRow.parseISO` (`RupuStore/
        // ActivityRow.swift`) and (until this same fix) `RupuOverview`'s own
        // `parseBucketTimestamp`. Delegates to the one canonical
        // implementation instead; `ActivityRow.parseISO` takes `String?`, so
        // a non-optional `String` argument here promotes implicitly.
        func parseTimestamp(_ s: String) -> Date? {
            ActivityRow.parseISO(s)
        }

        // An unparseable candidate is treated as unknown and never WINS the
        // comparison (never displaces whatever `acc` currently holds). The
        // very first value ever seen (`acc == nil`) is seeded as-is with no
        // comparison to make — including an unparseable one, since showing
        // *something* honestly-first-seen beats fabricating `nil` when
        // literally no host's timestamp parses. If `acc` itself is still
        // that unparsed seed and a later candidate DOES parse, the
        // well-formed value takes over (more informative than a raw,
        // unorderable placeholder). Two unparseable values in a row fall
        // through to "keep first" by construction: the second one's parse
        // fails, so it never wins, leaving the first (already-unparseable)
        // `acc` standing.
        func oldest(_ acc: inout String?, _ candidate: String?) {
            guard let candidate else { return }
            guard let current = acc else {
                acc = candidate
                return
            }
            guard let candidateDate = parseTimestamp(candidate) else { return }
            guard let currentDate = parseTimestamp(current) else {
                acc = candidate
                return
            }
            if candidateDate < currentDate { acc = candidate }
        }

        for summary in summaries {
            oldest(&oldestCapturedAt, summary.capturedAt)

            runningSum += summary.active.running
            awaitingSum += summary.active.awaitingApproval
            pausedSum += summary.active.paused
            pendingSum += summary.active.pending

            if let candidate = summary.activeLongest {
                if let current = activeLongest, current.ageMs >= candidate.ageMs {
                    // Keep `current` — ties keep whichever host was merged
                    // first, matching the Rust source's `>=` comparison.
                } else {
                    activeLongest = candidate
                }
            }

            cyclesTotal += summary.cycles.total
            if let n = summary.cycles.clean {
                if !cleanPoisoned { cyclesClean = (cyclesClean ?? 0) + n }
            } else {
                cleanPoisoned = true
                cyclesClean = nil
            }
            if let n = summary.cycles.withFailures {
                if !withFailuresPoisoned { cyclesWithFailures = (cyclesWithFailures ?? 0) + n }
            } else {
                withFailuresPoisoned = true
                cyclesWithFailures = nil
            }

            if let n = summary.findingsOpen {
                findingsOpen = (findingsOpen ?? 0) + n
            } else {
                findingsPartial = true
            }

            sumOptional(&repos, summary.fleet.repos)
            sumOptional(&providersConfigured, summary.fleet.providersConfigured)
            sumOptional(&providersUnhealthy, summary.fleet.providersUnhealthy)
            sumOptional(&autoflowsEnabled, summary.fleet.autoflowsEnabled)
            sumOptional(&autoflowsDisabled, summary.fleet.autoflowsDisabled)
            sumOptional(&workers, summary.fleet.workers)
            sumOptional(&claimsActive, summary.fleet.claimsActive)
            sumOptional(&issuesPending, summary.fleet.issuesPending)
            sumOptional(&issuesOpen, summary.fleet.issuesOpen)

            issuesCapped = issuesCapped || summary.fleet.issuesCapped
            oldest(&inventoryCapturedAt, summary.fleet.inventoryCapturedAt)

            for bucket in summary.terminalBuckets {
                if let existing = terminalByTS[bucket.ts] {
                    terminalByTS[bucket.ts] = APITerminalBucket(
                        ts: bucket.ts,
                        completed: existing.completed + bucket.completed,
                        failed: existing.failed + bucket.failed,
                        rejected: existing.rejected + bucket.rejected,
                        cancelled: existing.cancelled + bucket.cancelled
                    )
                } else {
                    terminalByTS[bucket.ts] = bucket
                }
            }
            for bucket in summary.throughputBuckets {
                if let existing = throughputByTS[bucket.ts] {
                    throughputByTS[bucket.ts] = APIThroughputBucket(
                        ts: bucket.ts,
                        manual: existing.manual + bucket.manual,
                        cron: existing.cron + bucket.cron,
                        event: existing.event + bucket.event
                    )
                } else {
                    throughputByTS[bucket.ts] = bucket
                }
            }

            findingsPartial = findingsPartial || summary.findingsPartial
            fleetPartial = fleetPartial || summary.fleetPartial
            cyclesPartialFromResponses = cyclesPartialFromResponses || summary.cyclesPartial
        }

        let cyclesPartial = cleanPoisoned || withFailuresPoisoned || cyclesPartialFromResponses

        return MergedDashboard(
            active: APIActiveCounts(running: runningSum, awaitingApproval: awaitingSum, paused: pausedSum, pending: pendingSum),
            activeLongest: activeLongest,
            terminalBuckets: terminalByTS.keys.sorted().compactMap { terminalByTS[$0] },
            throughputBuckets: throughputByTS.keys.sorted().compactMap { throughputByTS[$0] },
            cycles: APICycleCounts(total: cyclesTotal, clean: cyclesClean, withFailures: cyclesWithFailures),
            findingsOpen: findingsOpen,
            fleet: APIFleetCounts(
                repos: repos,
                providersConfigured: providersConfigured,
                providersUnhealthy: providersUnhealthy,
                autoflowsEnabled: autoflowsEnabled,
                autoflowsDisabled: autoflowsDisabled,
                workers: workers,
                claimsActive: claimsActive,
                issuesPending: issuesPending,
                issuesOpen: issuesOpen,
                issuesCapped: issuesCapped,
                inventoryCapturedAt: inventoryCapturedAt
            ),
            findingsPartial: findingsPartial,
            cyclesPartial: cyclesPartial,
            fleetPartial: fleetPartial,
            capturedAt: oldestCapturedAt
        )
    }

    // MARK: - Fetch cycle

    /// The shared engine behind `activate(range:)`, `setRange(_:)`, and the
    /// reconcile loop's tick: bumps `generation`, discovers the fleet
    /// (`GET /api/hosts`), reseeds `hostStates` from the fresh host list,
    /// then fetches `"local"` first (awaited — see the type's doc comment)
    /// and every other seeded host as an independent background `Task`.
    ///
    /// Review fix (round 1, minor): a HOST ALREADY PRESENT in `hostStates`
    /// keeps its current slice `state` across the reseed rather than being
    /// reset to `.loading` — the original behavior made every periodic 60s
    /// reconcile tick visibly flash every already-`.ok` host back to
    /// "loading" for the duration of its refetch, even though nothing about
    /// that host had actually changed yet. Only a host id that's genuinely
    /// NEW to this cycle's `GET /api/hosts` response (never seen before, or
    /// returning after having dropped out of a prior cycle) seeds as
    /// `.loading`, since there's no prior state to preserve for it. Each
    /// host's own fetch still updates its slice exactly as before once it
    /// resolves; the `generation` guard in `performFetch` (checked BEFORE
    /// `apply(_:hostID:)` ever runs) is what keeps a stale arrival from a
    /// superseded cycle from clobbering a kept slice — verified, not just
    /// assumed, by `setRangeRefetchesAllAndDropsStaleInFlightResults...`.
    ///
    /// `okResponses`/`merged` follow the same "no flash" principle: rather
    /// than blanking them at the top of every cycle, only entries for a
    /// host that fell OUT of the fleet (no longer present in this cycle's
    /// host list) are pruned — a host that's still registered keeps
    /// contributing its last-known-good numbers to `merged` until its own
    /// fresh fetch lands and replaces them.
    private func refetchAll() async {
        generation += 1
        let currentGeneration = generation
        cancelHostTasks()

        let hosts: [APIHostRow]
        do {
            hosts = try await client.hosts()
        } catch {
            // Final-review fix (Task 1): a THROWN `GET /api/hosts` call is a
            // transient failure, not evidence the fleet is empty. The prior
            // `(try? await client.hosts()) ?? []` idiom conflated the two —
            // a blip on this 60s reconcile tick reseeded `hostStates` empty,
            // pruned every `okResponses` entry, nil'd `merged`, and stamped
            // a lying "no hosts registered" `pageError` (this type keeps
            // last-known-good on every OTHER failure path; this was the one
            // exception). Leave every existing slice, `okResponses`, and
            // `merged` exactly as they were — the cycle simply doesn't
            // advance this tick. The next reconcile tick (or an explicit
            // `activate`/`setRange`) retries from scratch.
            return
        }
        guard currentGeneration == generation else { return }

        let previousStates = Dictionary(uniqueKeysWithValues: hostStates.map { ($0.id, $0.state) })
        hostStates = hosts.map { host in
            HostSlice(
                id: host.id, name: host.name, transportKind: host.transportKind,
                state: previousStates[host.id] ?? .loading
            )
        }

        let currentIDs = Set(hostStates.map(\.id))
        okResponses = okResponses.filter { currentIDs.contains($0.key) }
        recomputeMerged()

        pendingHostIDs = currentIDs

        // A genuinely EMPTY `GET /api/hosts` response (200, zero entries) —
        // as opposed to the thrown case handled above — really does mean
        // "no hosts registered", so this message stays honest here.
        guard !hostStates.isEmpty else {
            pageError = "no hosts registered"
            return
        }

        if let localSlice = hostStates.first(where: { $0.id == Self.localHostID }) {
            await performFetch(hostID: localSlice.id, generation: currentGeneration)
        }

        for slice in hostStates where slice.id != Self.localHostID {
            let task = Task { [weak self] in
                guard let self else { return }
                await self.performFetch(hostID: slice.id, generation: currentGeneration)
            }
            hostTasks.append(task)
        }
    }

    /// One host's fetch: resolves to an outcome, applies it (if this cycle
    /// is still current), and — once every seeded host in the cycle has
    /// resolved with zero `"ok"` — sets `pageError`. Shared by the full
    /// cycle above and `refreshLocalOnly()` below; `pendingHostIDs.remove`
    /// is idempotent so a call from the latter (outside the cycle it was
    /// sized for) can never manufacture phantom progress toward "every host
    /// resolved" — see that property's doc comment.
    private func performFetch(hostID: String, generation: Int) async {
        let outcome = await fetchOutcome(hostID: hostID)
        guard generation == self.generation else { return }
        apply(outcome, hostID: hostID)
        pendingHostIDs.remove(hostID)
        if pendingHostIDs.isEmpty && okResponses.isEmpty {
            pageError = "no host reported dashboard data"
        }
    }

    private enum FetchOutcome {
        case ok(APIDashboardResponse, capturedAt: String?)
        case offline
        case unavailable(reason: String?)
    }

    /// Two distinct failure shapes, both mapped to a slice state — see the
    /// type's doc comment. A thrown error (transport/decoding/non-2xx) maps
    /// via the same `mutationErrorMessage` every write-path method in this
    /// module already uses. A successful response's own per-host
    /// `hosts[]` entry (matched by id; falls back to the sole entry if the
    /// id doesn't match anything, which should not happen against a
    /// well-formed server but is handled rather than crashing) carries the
    /// server's own verdict for that host.
    private func fetchOutcome(hostID: String) async -> FetchOutcome {
        do {
            let response = try await client.dashboard(range: range.rawValue, host: hostID)
            let freshness = response.hosts.first(where: { $0.hostID == hostID }) ?? response.hosts.first
            switch freshness?.state {
            case "ok":
                return .ok(response, capturedAt: freshness?.capturedAt)
            case "offline":
                return .offline
            default:
                return .unavailable(reason: freshness?.reason)
            }
        } catch {
            return .unavailable(reason: mutationErrorMessage(error))
        }
    }

    private func apply(_ outcome: FetchOutcome, hostID: String) {
        guard let index = hostStates.firstIndex(where: { $0.id == hostID }) else { return }
        switch outcome {
        case .ok(let response, let capturedAt):
            hostStates[index].state = .ok(capturedAt: capturedAt)
            okResponses[hostID] = response
        case .offline:
            hostStates[index].state = .offline
            okResponses.removeValue(forKey: hostID)
        case .unavailable(let reason):
            hostStates[index].state = .unavailable(reason: reason)
            okResponses.removeValue(forKey: hostID)
        }
        recomputeMerged()
    }

    /// Re-runs `merge(_:)` over every currently-`"ok"` host. Clears
    /// `pageError` the moment there is at least one `"ok"` host to merge —
    /// see that property's doc comment.
    private func recomputeMerged() {
        let oks = Array(okResponses.values)
        guard !oks.isEmpty else {
            merged = nil
            return
        }
        merged = Self.merge(oks)
        pageError = nil
    }

    private func cancelHostTasks() {
        for task in hostTasks { task.cancel() }
        hostTasks.removeAll()
    }

    // MARK: - Firehose (local-only, debounced refresh)

    /// (Re)builds the firehose consumer from a fresh `signalsFactory()`
    /// call. Every signal off the stream — connection change or decoded
    /// event alike — schedules the debounced local refetch; this store has
    /// no per-row identity to reduce a specific event kind against (unlike
    /// `ActivityStore`), so "something happened, the aggregate may be
    /// stale" is the only signal worth reading out of the stream at all.
    private func startSignalConsumer() {
        signalsTask?.cancel()
        let stream = signalsFactory()
        signalsTask = Task { [weak self] in
            for await _ in stream {
                guard let self else { return }
                self.scheduleDebouncedLocalRefresh()
            }
        }
    }

    /// Coalesces a burst of firehose signals into one refetch: each new
    /// signal cancels and restarts the wait, so only the burst's last
    /// signal actually pays for a round trip.
    private func scheduleDebouncedLocalRefresh() {
        debounceTask?.cancel()
        debounceTask = Task { [weak self, debounceInterval] in
            try? await Task.sleep(for: debounceInterval)
            guard !Task.isCancelled, let self else { return }
            await self.refreshLocalOnly()
        }
    }

    /// Refetches `"local"` alone, under the cycle's current generation —
    /// never touches any other host's already-loaded slice. A no-op if
    /// `"local"` isn't (yet, or ever) a seeded host.
    private func refreshLocalOnly() async {
        guard hostStates.contains(where: { $0.id == Self.localHostID }) else { return }
        await performFetch(hostID: Self.localHostID, generation: generation)
    }

    // MARK: - Reconcile loop

    /// Starts (or restarts) the 60s-by-default reconcile loop: sleeps,
    /// then runs the full `refetchAll()` cycle, weak-self-per-iteration —
    /// same idiom as `RupuBackend.HealthMonitor.start()`, so a caller that
    /// drops its last strong reference to this store without calling
    /// `deactivate()` still lets the loop notice and unwind on its next
    /// tick rather than the task/self pair keeping each other alive
    /// forever. Sleeps *before* the first `refetchAll()` — `activate(range:)`
    /// already ran one full cycle immediately before calling this, so an
    /// unconditional poll-then-sleep (`HealthMonitor`'s own order) would
    /// duplicate it.
    private func startReconcileLoop() {
        reconcileTask?.cancel()
        let interval = reconcileInterval
        reconcileTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: interval)
                guard !Task.isCancelled else { return }
                guard let self else { return }
                await self.refetchAll()
            }
        }
    }
}
