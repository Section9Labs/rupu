import Foundation
import os

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

/// Reconnecting SSE client that decodes each dispatched frame's `data` as a
/// `CPEvent`. Undecodable frames are skipped (never fatal); the connection
/// is retried with capped exponential backoff on stream end or error, and
/// torn down when the consuming task is cancelled.
public final class EventStreamClient: Sendable {
    private static let logger = Logger(subsystem: "com.section9labs.rupu", category: "sse")

    private let url: URL
    private let token: String?
    private let session: URLSession
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
        session: URLSession = .shared,
        onConnectionChange: (@Sendable (Bool) -> Void)? = nil
    ) {
        self.url = url
        self.token = token
        self.session = session
        self.onConnectionChange = onConnectionChange
    }

    public func events() -> AsyncStream<CPEvent> {
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
    /// the task is cancelled). Returns `true` if at least one frame decoded
    /// successfully into a `CPEvent` during this attempt, so the caller can
    /// reset its backoff.
    private func runOnce(continuation: AsyncStream<CPEvent>.Continuation) async -> Bool {
        var request = URLRequest(url: url)
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        if let token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }

        var sawHealthyFrame = false
        var parser = SSELineParser()
        // Tracks whether this attempt fired `onConnectionChange(true)`, so
        // the matching `false` only fires for an attempt that actually
        // connected — never for one that failed before ever reaching a 2xx
        // response.
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
                if Task.isCancelled { return sawHealthyFrame }
                guard let frame = parser.feed(line: line) else { continue }
                guard let data = frame.data.data(using: .utf8),
                      let event = try? JSONDecoder().decode(CPEvent.self, from: data) else {
                    continue // Undecodable frame — skip, not fatal.
                }
                sawHealthyFrame = true
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
        return sawHealthyFrame
    }
}
