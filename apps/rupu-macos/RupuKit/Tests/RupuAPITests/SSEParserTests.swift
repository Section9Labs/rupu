import Testing
@testable import RupuAPI

@Test func parsesSingleDataFrame() {
    var p = SSELineParser()
    #expect(p.feed(line: #"data: {"type":"run_paused","run_id":"r1"}"#) == nil)
    let frame = p.feed(line: "")
    #expect(frame?.data == #"{"type":"run_paused","run_id":"r1"}"#)
}
@Test func joinsMultiLineData() {
    var p = SSELineParser()
    _ = p.feed(line: "data: {\"a\":")
    _ = p.feed(line: "data: 1}")
    #expect(p.feed(line: "")?.data == "{\"a\":\n1}")
}
@Test func ignoresCommentsAndKeepAlives() {
    var p = SSELineParser()
    #expect(p.feed(line: ": keep-alive") == nil)
    #expect(p.feed(line: "") == nil)   // blank with no pending data → no frame
}
@Test func capturesEventName() {
    var p = SSELineParser()
    _ = p.feed(line: "event: message")
    _ = p.feed(line: "data: x")
    #expect(p.feed(line: "") == SSEFrame(event: "message", data: "x"))
}

// Additional edge cases beyond the brief's Step 1 examples.

@Test func dataLineWithNoSpaceAfterColonIsHandled() {
    var p = SSELineParser()
    _ = p.feed(line: "data:x")
    #expect(p.feed(line: "")?.data == "x")
}

@Test func idAndRetryLinesAreIgnored() {
    var p = SSELineParser()
    _ = p.feed(line: "id: 42")
    _ = p.feed(line: "retry: 3000")
    _ = p.feed(line: "data: x")
    #expect(p.feed(line: "")?.data == "x")
}

@Test func parserResetsAfterDispatch() {
    var p = SSELineParser()
    _ = p.feed(line: "event: message")
    _ = p.feed(line: "data: first")
    #expect(p.feed(line: "") == SSEFrame(event: "message", data: "first"))

    // Next frame must not carry over the previous event name or data.
    _ = p.feed(line: "data: second")
    #expect(p.feed(line: "") == SSEFrame(event: nil, data: "second"))
}

@Test func onlyOneLeadingSpaceIsStrippedFromDataValue() {
    var p = SSELineParser()
    _ = p.feed(line: "data:  leading two spaces")
    #expect(p.feed(line: "")?.data == " leading two spaces")
}
