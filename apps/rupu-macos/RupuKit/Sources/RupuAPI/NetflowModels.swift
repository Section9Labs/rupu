import Foundation

/// `GET /api/runs/:id/netflow` response envelope. There is no server-side
/// "unexpected host" flag: the UI renders a rollup row in a fail color when
/// `errors > 0` or a flow's `outcome` isn't `"ok"`; a true allowlist diff is
/// future work.
public struct APINetflow: Decodable, Sendable {
    public let flows: [APIFlow]
    public let hosts: [APIHostRollup]
    public let droppedTotal: UInt64
    public let asnLoaded: Bool

    public init(flows: [APIFlow], hosts: [APIHostRollup], droppedTotal: UInt64, asnLoaded: Bool) {
        self.flows = flows
        self.hosts = hosts
        self.droppedTotal = droppedTotal
        self.asnLoaded = asnLoaded
    }

    private enum CodingKeys: String, CodingKey {
        case flows
        case hosts
        case droppedTotal = "dropped_total"
        case asnLoaded = "asn_loaded"
    }
}

/// A flattened `FlowRecord` + optional `asn` lookup (`FlowView` on the Rust
/// side). `status` is `nil` for a flow that never got an HTTP response
/// (e.g. `outcome == "transport_error"`).
public struct APIFlow: Decodable, Sendable {
    public let id: String
    public let ts: String
    public let method: String
    public let scheme: String
    public let host: String
    public let port: UInt16
    public let path: String
    public let status: UInt16?
    public let outcome: String
    public let error: String?
    public let bytesIn: UInt64?
    public let bytesOut: UInt64?
    public let durationMS: UInt64?
    public let ctx: APIFlowCtx
    public let asn: APIAsn?
    /// The TOP-LEVEL run this flow folds into, resolved server-side from
    /// the ledger file's own id (`FlowView.run_id` on the Rust side) —
    /// distinct from `ctx.runID`, which is unset on every production
    /// flow. `nil` when no run record accounts for the ledger id.
    public let runID: String?
    /// `RunRecord::workflow_name` of that root run; `nil` with `runID`.
    public let workflow: String?

    public init(
        id: String,
        ts: String,
        method: String,
        scheme: String,
        host: String,
        port: UInt16,
        path: String,
        status: UInt16?,
        outcome: String,
        error: String?,
        bytesIn: UInt64?,
        bytesOut: UInt64?,
        durationMS: UInt64?,
        ctx: APIFlowCtx,
        asn: APIAsn?,
        runID: String? = nil,
        workflow: String? = nil
    ) {
        self.id = id
        self.ts = ts
        self.method = method
        self.scheme = scheme
        self.host = host
        self.port = port
        self.path = path
        self.status = status
        self.outcome = outcome
        self.error = error
        self.bytesIn = bytesIn
        self.bytesOut = bytesOut
        self.durationMS = durationMS
        self.ctx = ctx
        self.asn = asn
        self.runID = runID
        self.workflow = workflow
    }

    private enum CodingKeys: String, CodingKey {
        case id, ts, method, scheme, host, port, path, status, outcome, error
        case bytesIn = "bytes_in"
        case bytesOut = "bytes_out"
        case durationMS = "duration_ms"
        case ctx
        case asn
        case runID = "run_id"
        case workflow
    }
}

/// The dispatch context embedded on every flow (`ctx` on the Rust side) —
/// which run/step/agent initiated this network call.
public struct APIFlowCtx: Decodable, Sendable {
    public let runID: String?
    public let stepID: String?
    public let agent: String?

    public init(runID: String?, stepID: String?, agent: String?) {
        self.runID = runID
        self.stepID = stepID
        self.agent = agent
    }

    private enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case stepID = "step_id"
        case agent
    }
}

/// ASN lookup attached to a flow when `asn_loaded` is true.
public struct APIAsn: Decodable, Sendable {
    public let asn: UInt32
    public let org: String

    public init(asn: UInt32, org: String) {
        self.asn = asn
        self.org = org
    }
}

/// `HostRollup` on the Rust side — per-destination aggregate stats.
public struct APIHostRollup: Decodable, Sendable {
    public let host: String
    public let port: UInt16
    public let calls: UInt64
    public let bytesIn: UInt64?
    public let bytesOut: UInt64?
    public let errors: UInt64
    public let p50MS: UInt64?
    public let p95MS: UInt64?

    public init(
        host: String,
        port: UInt16,
        calls: UInt64,
        bytesIn: UInt64?,
        bytesOut: UInt64?,
        errors: UInt64,
        p50MS: UInt64?,
        p95MS: UInt64?
    ) {
        self.host = host
        self.port = port
        self.calls = calls
        self.bytesIn = bytesIn
        self.bytesOut = bytesOut
        self.errors = errors
        self.p50MS = p50MS
        self.p95MS = p95MS
    }

    private enum CodingKeys: String, CodingKey {
        case host, port, calls
        case bytesIn = "bytes_in"
        case bytesOut = "bytes_out"
        case errors
        case p50MS = "p50_ms"
        case p95MS = "p95_ms"
    }
}
