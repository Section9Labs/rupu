import Foundation
import os

private let sseLogger = Logger(subsystem: "com.section9labs.rupu", category: "sse")

/// Shared decoder for every dispatched SSE frame, across every `JSONEventStream<T>`
/// specialization. Used to be a fresh `JSONDecoder()` per frame — on a busy stream (Situation
/// Room, Activity tail) that's one allocation per event, forever. Foundation's `JSONDecoder` is
/// `Sendable` (`@unchecked`, since it's a mutable class), which is safe here because this
/// instance's configuration is never touched after creation (no custom
/// `dateDecodingStrategy`/`keyDecodingStrategy`), so `decode(_:from:)` carries no mutable state
/// beyond the local decoding context each call builds for itself — concurrent decode calls from
/// different streams' `Task`s are safe against one shared instance. Lives at file scope (not as a
/// static on `JSONEventStream`) for the same reason `sseLogger` above does — see the note on
/// `JSONEventStream.logger` below: a generic type can't hold a static stored property.
private let sseJSONDecoder = JSONDecoder()

/// One dispatched Server-Sent Events frame: an optional `event:` name plus
/// the (possibly multi-line, `\n`-joined) accumulated `data:` payload.
public struct SSEFrame: Equatable, Sendable {
    public var event: String?
    public var data: String

    public init(event: String? = nil, data: String) {
        self.event = event
        self.data = data
    }
}

/// Pure, allocation-light per-line parser for the SSE wire format
/// (https://html.spec.whatwg.org/multipage/server-sent-events.html#event-stream-interpretation).
///
/// Feed it one line at a time (no trailing newline). It accumulates
/// `data:` lines, captures the most recent `event:` line, ignores `:`
/// comments (including keep-alives) and `id:`/`retry:` lines, and
/// dispatches the accumulated frame — resetting its state — when it sees
/// a blank line while data is pending. A blank line with no pending data
/// dispatches nothing.
public struct SSELineParser: Sendable {
    private var eventName: String?
    private var dataLines: [String] = []

    public init() {}

    public mutating func feed(line: String) -> SSEFrame? {
        if line.isEmpty {
            guard !dataLines.isEmpty else {
                // A blank line with no pending data dispatches nothing, but
                // still clears any orphaned `event:` line per the spec.
                eventName = nil
                return nil
            }
            let frame = SSEFrame(event: eventName, data: dataLines.joined(separator: "\n"))
            eventName = nil
            dataLines = []
            return frame
        }

        if line.hasPrefix(":") {
            // Comment / keep-alive — ignored.
            return nil
        }

        let (field, value) = Self.splitField(line)
        switch field {
        case "data":
            dataLines.append(value)
        case "event":
            eventName = value
        case "id", "retry":
            break // Ignored in this phase.
        default:
            break // Unknown field — ignored.
        }
        return nil
    }

    /// Splits `"field: value"` into `(field, value)`, stripping exactly one
    /// leading space from `value` per the SSE spec. Handles both `field:value`
    /// and `field: value`. A line with no colon is treated as a field name
    /// with an empty value.
    private static func splitField(_ line: String) -> (field: Substring, value: String) {
        guard let colonIndex = line.firstIndex(of: ":") else {
            return (line[...], "")
        }
        let field = line[line.startIndex..<colonIndex]
        var valueStart = line.index(after: colonIndex)
        if valueStart < line.endIndex, line[valueStart] == " " {
            valueStart = line.index(after: valueStart)
        }
        return (field, String(line[valueStart...]))
    }
}

/// Dedicated `URLSession` for long-lived SSE streams.
///
/// `URLSession.shared` caps concurrent connections per host at 6
/// (`URLSessionConfiguration`'s default `httpMaximumConnectionsPerHost`),
/// and that pool is shared with every REST call `CPClient` makes. SSE
/// streams are immortal occupants of their slot: with one connection per
/// consumer (shell footer, Activity tail, Situation Room, Overview,
/// RunNotifier, run-detail event + transcript streams) the app can hold six
/// streams, at which point the seventh stream — or any REST request —
/// queues **silently** behind them: no response callback, no frame, no
/// error until the 60 s request timeout, then a reconnect straight back
/// into the same full pool. Which six streams win the slots after an app
/// restart is a race, so the starved consumer varies — pills driven by
/// winning connections honestly report Live while the losing consumer
/// shows zero events forever (reproduced live against `cp serve`
/// 0.75.0-beta.3, 2026-08-25).
///
/// Routing every stream through this dedicated session removes both halves
/// of the failure: streams stop competing with REST for `.shared`'s six
/// slots, and the raised per-host cap gives stream fan-out headroom far
/// beyond any real screen combination.
public enum SSETransport {
    /// Generous headroom over the worst observed concurrent-stream count
    /// (~7); connections to the local control plane are cheap.
    public static let maxConnectionsPerHost = 16

    public static let session: URLSession = {
        let config = URLSessionConfiguration.default
        config.httpMaximumConnectionsPerHost = maxConnectionsPerHost
        return URLSession(configuration: config)
    }()
}

/// Reconnecting SSE client that decodes each dispatched frame's `data` as a
/// `T`. Undecodable frames are skipped (never fatal); the connection is
/// retried with capped exponential backoff on stream end or error, and torn
/// down when the consuming task is cancelled.
///
/// Generalized from the original `CPEvent`-only `EventStreamClient` so the
/// same reconnect/backoff/logging machinery backs any JSON-over-SSE feed
/// (e.g. `TranscriptEvent` from `/api/transcript/stream`). `EventStreamClient`
/// lives on as a type alias for source compatibility.
public final class JSONEventStream<T: Decodable & Sendable>: Sendable {
    // A generic type can't hold a static stored property, so the logger
    // lives at file scope instead (shared across every `T` specialization).
    private static var logger: Logger { sseLogger }

    private let url: URL
    private let token: String?
    /// Internal (not `private`) so tests can assert streams default onto
    /// [`SSETransport.session`] rather than `URLSession.shared`.
    let session: URLSession
    private let onConnectionChange: (@Sendable (Bool) -> Void)?

    /// `onConnectionChange`, when provided, fires `true` the moment a
    /// connection attempt gets a 2xx response (before any frame has
    /// necessarily arrived) and `false` when that attempt ends for any
    /// reason (stream end, error, or cancellation). This is additive to
    /// the original frame-only `events()` stream: a server that's healthy
    /// but idle (no events, only SSE keep-alive comments) never dispatches
    /// a decodable frame, so a consumer that only watched `events()` for a
    /// "connected" signal would show disconnected forever against a
    /// perfectly good stream. `onConnectionChange` is the connection-level
    /// signal; `events()` remains purely event-level and is unchanged.
    public init(
        url: URL,
        token: String?,
        session: URLSession = SSETransport.session,
        onConnectionChange: (@Sendable (Bool) -> Void)? = nil
    ) {
        self.url = url
        self.token = token
        self.session = session
        self.onConnectionChange = onConnectionChange
    }

    public func events() -> AsyncStream<T> {
        AsyncStream { continuation in
            let task = Task {
                var backoffSeconds: UInt64 = 1
                let maxBackoffSeconds: UInt64 = 30

                while !Task.isCancelled {
                    let healthy = await self.runOnce(continuation: continuation)
                    if Task.isCancelled { break }

                    if healthy {
                        backoffSeconds = 1
                    }

                    do {
                        try await Task.sleep(nanoseconds: backoffSeconds * 1_000_000_000)
                    } catch {
                        break // Cancelled during backoff sleep.
                    }
                    backoffSeconds = min(backoffSeconds * 2, maxBackoffSeconds)
                }
                continuation.finish()
            }

            continuation.onTermination = { _ in
                task.cancel()
            }
        }
    }

    /// Runs one connection attempt to completion (stream ends, errors, or
    /// the task is cancelled). Returns `true` if this attempt successfully
    /// established a connection (2xx response), so the caller can reset its
    /// backoff. Backoff resets the moment the connection is established —
    /// not only on the first decoded frame — so a healthy-but-idle stream
    /// (keep-alive comments only, no dispatchable frames yet) doesn't keep
    /// climbing backoff on every reconnect of an otherwise-fine connection.
    private func runOnce(continuation: AsyncStream<T>.Continuation) async -> Bool {
        var request = URLRequest(url: url)
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        if let token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }

        var parser = SSELineParser()
        // Tracks whether this attempt fired `onConnectionChange(true)`, so
        // the matching `false` only fires for an attempt that actually
        // connected — never for one that failed before ever reaching a 2xx
        // response. Also doubles as this attempt's "healthy" return value.
        var didSignalConnected = false
        defer {
            if didSignalConnected {
                onConnectionChange?(false)
            }
        }

        do {
            let (bytes, response) = try await session.bytes(for: request)
            if let httpResponse = response as? HTTPURLResponse,
               !(200..<300).contains(httpResponse.statusCode) {
                // A non-2xx response (e.g. 401 on a bad token) is not a
                // healthy attempt — surface it visibly rather than silently
                // retrying forever indistinguishably from a network blip.
                Self.logger.error("SSE connect failed: HTTP \(httpResponse.statusCode, privacy: .public) for \(self.url, privacy: .public)")
                return false
            }
            didSignalConnected = true
            onConnectionChange?(true)
            for try await line in bytes.lines {
                if Task.isCancelled { return didSignalConnected }
                guard let frame = parser.feed(line: line) else { continue }
                guard let data = frame.data.data(using: .utf8),
                      let event = try? sseJSONDecoder.decode(T.self, from: data) else {
                    continue // Undecodable frame — skip, not fatal.
                }
                continuation.yield(event)
            }
        } catch {
            // Fall through to the caller's backoff/reconnect loop (or exit
            // if cancelled). A `CancellationError` here means the consumer
            // tore the connection down deliberately — that's expected
            // shutdown, not a failure, so it's excluded from the error log.
            // Genuine transport errors are logged so failure modes are
            // visible in Console/log stream even though no user-facing
            // health surface exists yet (that's Task 9's HealthMonitor).
            if !Task.isCancelled {
                Self.logger.error("SSE connection error for \(self.url, privacy: .public): \(String(describing: error), privacy: .public)")
            }
        }
        return didSignalConnected
    }
}

/// Source-compatible alias for the original `CPEvent`-only stream client.
public typealias EventStreamClient = JSONEventStream<CPEvent>
