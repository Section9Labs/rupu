import Foundation
import RupuAPI
import RupuDesign

// Situation Room — pure aggregation for the right-hand project roster. Port
// of `crates/rupu-cp/web/src/lib/situationRoom/roster.ts` (verified full
// read, 226 lines) up to (not including) `Vitals`/`buildVitals`, which live
// in `Vitals.swift`.
//
// `deriveActivity`'s items type: `roster.ts` line 34 takes
// `{ ts: number; event: RunEvent }[]`. `RupuAPI.CPEventRow` (`Models.swift`
// lines 37-52) is exactly that pair on the Swift side — it already
// separates the server-injected `ts`/`pos` from the decoded `CPEvent` — so
// it's reused here directly rather than inventing a parallel wrapper type.
//
// `buildRoster`'s `projects` type: `roster.ts` line 135 takes
// `ProjectRow[]`. `RupuAPI.APIProjectRow` (`Models.swift`) is the Swift
// decode of the same wire `ProjectRow` — Task 6 added its previously-
// undecoded `branch`/`last_active` fields (see that struct's doc comment)
// specifically so `foldRoster` below could consume it directly instead of a
// second parallel project shape.

/// Per-run live state, distilled from the event stream. Port of `roster.ts`
/// lines 21-29's `RunActivity` interface.
public struct RunActivity: Equatable, Sendable {
    public enum State: Equatable, Sendable {
        case running, awaiting, paused, done, failed
    }

    public let runID: String
    public let state: State
    /// Short human label of what the run is doing right now (agent · step).
    public let action: String?
    /// ms since epoch of the last event seen for this run.
    public let ts: Int64

    public init(runID: String, state: State, action: String? = nil, ts: Int64) {
        self.runID = runID
        self.state = state
        self.action = action
        self.ts = ts
    }
}

/// Latest state per run, folded from a newest-first event list. Port of
/// `roster.ts` lines 34-59's `deriveActivity`. The caller supplies items
/// newest-first (matching the Events page's merged stream — see that
/// function's own doc comment, `roster.ts` lines 31-33); this does not sort.
public func deriveActivity(_ items: [CPEventRow]) -> [String: RunActivity] {
    var out: [String: RunActivity] = [:]
    for item in items {
        guard let runID = item.event.runID else { continue } // no identity to key on
        if out[runID] != nil { continue } // first hit = newest event for this run

        var state: RunActivity.State = .running
        var action: String?

        // roster.ts lines 42-54 — this switch intentionally covers only the
        // 10 event kinds the web's own switch names; every other kind
        // (including step_failed/step_skipped/unit_started/unit_completed/
        // run_started/run_resumed, none of which appear in the web's
        // switch) falls to `default: break` there and to the `default` case
        // here, leaving state=.running/action=nil — ported as-is, not
        // "completed" to a symmetric set.
        switch item.event {
        case .runCompleted:
            state = .done
        case .runFailed:
            state = .failed
        case let .stepAwaitingApproval(_, stepID, _):
            state = .awaiting
            action = "awaiting · \(stepID)"
        case .runPaused:
            state = .paused
            action = "paused"
        case let .stepPaused(_, stepID):
            state = .paused
            action = "paused · \(stepID)"
        case let .stepStarted(_, stepID, _, agent, _):
            action = (agent?.isEmpty == false) ? "\(agent!) · \(stepID)" : stepID
        case let .stepWorking(_, stepID, note, _):
            let trimmed = note?.trimmingCharacters(in: .whitespacesAndNewlines)
            action = (trimmed?.isEmpty == false) ? trimmed : stepID
        case let .stepCompleted(_, stepID, _, _, _):
            action = stepID
        case let .stepResumed(_, stepID):
            action = stepID
        case let .panelRound(_, stepID, round, _, _):
            action = "\(stepID) · round \(round)"
        default:
            break
        }

        out[runID] = RunActivity(runID: runID, state: state, action: action, ts: item.ts)
    }
    return out
}

/// Persisted `run.json` statuses that mean the run is over. Port of
/// `roster.ts` line 61's `TERMINAL_RUN_STATUSES`.
private let terminalRunStatuses: Set<String> = ["completed", "failed", "cancelled", "rejected"]

/// Reconcile event-derived activity with the authoritative persisted run
/// status. Port of `roster.ts` lines 72-89's `reconcileActivity`.
public func reconcileActivity(
    _ activity: [String: RunActivity],
    persistedStatus: [String: String]
) -> [String: RunActivity] {
    var out = activity
    for (runID, act) in activity {
        if act.state == .done || act.state == .failed { continue }
        guard let status = persistedStatus[runID], terminalRunStatuses.contains(status) else { continue }
        out[runID] = RunActivity(
            runID: act.runID,
            state: (status == "completed" || status == "cancelled") ? .done : .failed,
            action: nil,
            ts: act.ts
        )
    }
    return out
}

/// Port of `roster.ts` lines 91-98's `SevCounts` interface.
public struct SevCounts: Equatable, Sendable {
    public var critical: Int = 0
    public var high: Int = 0
    public var medium: Int = 0
    public var low: Int = 0
    public var info: Int = 0
    public var total: Int = 0

    public init(critical: Int = 0, high: Int = 0, medium: Int = 0, low: Int = 0, info: Int = 0, total: Int = 0) {
        self.critical = critical
        self.high = high
        self.medium = medium
        self.low = low
        self.info = info
        self.total = total
    }

    fileprivate mutating func increment(_ sev: Severity) {
        switch sev {
        case .crit: critical += 1
        case .high: high += 1
        case .med: medium += 1
        case .low: low += 1
        case .info: info += 1
        }
        total += 1
    }
}

/// Group findings by workspace into severity counts. Port of `roster.ts`
/// lines 116-125's `findingsByWorkspace`.
public func findingsByWorkspace(_ findings: [APIFinding]) -> [String: SevCounts] {
    var out: [String: SevCounts] = [:]
    for f in findings {
        var counts = out[f.wsID] ?? SevCounts()
        // roster.ts line 120 goes through `normFindingSeverity`, which
        // lowercases the raw wire string before matching — lowercase here
        // too, for the same reason as `cardForFinding` (fix round 1,
        // finding 2; see StreamCards.swift's matching comment).
        counts.increment(Severity(wireString: f.severity.lowercased()))
        out[f.wsID] = counts
    }
    return out
}

/// One project card in the roster. Port of `roster.ts` lines 100-109's
/// `RosterProject` interface (named `RosterEntry` per the task-6 brief's
/// interface sketch).
public struct RosterEntry: Equatable, Sendable {
    public enum Status: Equatable, Sendable {
        case running, await_, idle
    }

    public let wsID: String
    public let name: String
    public let branch: String?
    public let status: Status
    public let action: String?
    public let activeRuns: Int
    public let findings: SevCounts
    public let lastActiveTS: Int64?

    public init(
        wsID: String, name: String, branch: String? = nil, status: Status, action: String? = nil,
        activeRuns: Int, findings: SevCounts, lastActiveTS: Int64? = nil
    ) {
        self.wsID = wsID
        self.name = name
        self.branch = branch
        self.status = status
        self.action = action
        self.activeRuns = activeRuns
        self.findings = findings
        self.lastActiveTS = lastActiveTS
    }
}

/// Sort rank: awaiting → running → idle. Port of `roster.ts` line 127's
/// `STATUS_RANK`.
private func statusRank(_ s: RosterEntry.Status) -> Int {
    switch s {
    case .await_: return 0
    case .running: return 1
    case .idle: return 2
    }
}

/// Build the roster: one card per project, ordered awaiting → running →
/// idle, then most-recently-active first. Port of `roster.ts` lines 134-182's
/// `buildRoster` (named `foldRoster` per the task-6 brief's interface
/// sketch).
public func foldRoster(
    projects: [APIProjectRow],
    runToWs: [String: String],
    activity: [String: RunActivity],
    findings: [APIFinding]
) -> [RosterEntry] {
    let findMap = findingsByWorkspace(findings)

    // Invert run→ws into ws→[activity] so each project sees only its own runs.
    var byWs: [String: [RunActivity]] = [:]
    for (runID, act) in activity {
        guard let ws = runToWs[runID] else { continue }
        byWs[ws, default: []].append(act)
    }

    // `projects.enumerated()` carries each project's input-array position
    // alongside its built `RosterEntry` — an explicit tiebreak (fix round 1,
    // finding 4) so a `lastActiveTS` tie in the sort below resolves to
    // project input order deterministically, rather than riding `sorted`'s
    // incidental (if Swift-guaranteed) stability surviving a future
    // refactor. Mirrors the web's own implicit reliance on
    // `Array.prototype.sort`'s ES2019 stability guarantee for the same tie.
    let indexed: [(index: Int, entry: RosterEntry)] = projects.enumerated().map { index, p in
        let acts = byWs[p.wsID] ?? []
        let live = acts.filter { $0.state == .running || $0.state == .awaiting }
        let awaiting = live.contains { $0.state == .awaiting }
        let status: RosterEntry.Status = awaiting ? .await_ : (live.count > 0 ? .running : .idle)

        // Current action = the most recent live run's action, tiebroken on
        // `runID` when two live runs share a `ts` (fix round 1, finding 3):
        // `acts`' element order depends on `activity`'s `Dictionary`
        // iteration order, which is not stable across process launches, so
        // without an explicit tiebreak the picked `action` could vary
        // launch-to-launch for the same input.
        let newestLive = live.sorted { a, b in
            a.ts != b.ts ? a.ts > b.ts : a.runID < b.runID
        }.first

        let projLastActive: Int64 = p.lastActive.map(rfc3339ToMS) ?? 0
        let lastActiveTS = acts.reduce(projLastActive) { max($0, $1.ts) }

        let entry = RosterEntry(
            wsID: p.wsID,
            name: p.name,
            branch: p.branch,
            status: status,
            action: newestLive?.action,
            activeRuns: live.count,
            findings: findMap[p.wsID] ?? SevCounts(),
            lastActiveTS: lastActiveTS > 0 ? lastActiveTS : nil
        )
        return (index, entry)
    }

    let sorted = indexed.sorted { a, b in
        let ra = statusRank(a.entry.status), rb = statusRank(b.entry.status)
        if ra != rb { return ra < rb }
        let la = a.entry.lastActiveTS ?? 0, lb = b.entry.lastActiveTS ?? 0
        if la != lb { return la > lb }
        return a.index < b.index // stable fallback: original project input order
    }
    return sorted.map(\.entry)
}

/// Count of projects with at least one active (running/awaiting) run. Port
/// of `roster.ts` lines 185-187's `projectsLive`.
public func projectsLive(_ roster: [RosterEntry]) -> Int {
    roster.filter { $0.status != .idle }.count
}
