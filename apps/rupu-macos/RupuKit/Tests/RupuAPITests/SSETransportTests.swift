import Foundation
import Testing
@testable import RupuAPI

// SSETransport exists because `URLSession.shared` caps concurrent
// connections per host at 6 and shares that pool with REST — a seventh SSE
// stream (or any REST call behind six streams) queues silently forever.
// These tests pin the two load-bearing facts: the dedicated session is not
// `.shared`, and its per-host cap clears the app's worst-case stream count.

@Test func sseTransportIsNotTheSharedSession() {
    #expect(SSETransport.session !== URLSession.shared)
}

@Test func sseTransportRaisesThePerHostConnectionCap() {
    #expect(
        SSETransport.session.configuration.httpMaximumConnectionsPerHost
            == SSETransport.maxConnectionsPerHost
    )
    // Worst observed concurrent streams is ~7 (shell + activity + situation
    // + overview + notifier + run-detail events + transcript); the cap must
    // clear that with room for future consumers.
    #expect(SSETransport.maxConnectionsPerHost >= 12)
}

@Test func jsonEventStreamDefaultsToTheDedicatedSession() {
    let stream = JSONEventStream<CPEvent>(
        url: URL(string: "http://127.0.0.1:7420/api/events/stream")!,
        token: nil
    )
    #expect(stream.session === SSETransport.session)
    #expect(stream.session !== URLSession.shared)
}
