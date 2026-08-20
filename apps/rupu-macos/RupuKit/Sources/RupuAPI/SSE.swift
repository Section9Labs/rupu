import Foundation

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
    private let url: URL
    private let token: String?
    private let session: URLSession

    public init(url: URL, token: String?, session: URLSession = .shared) {
        self.url = url
        self.token = token
        self.session = session
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

        do {
            let (bytes, _) = try await session.bytes(for: request)
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
            // Transport error or cancellation — fall through to the caller's
            // backoff/reconnect loop (or exit if cancelled).
        }
        return sawHealthyFrame
    }
}
