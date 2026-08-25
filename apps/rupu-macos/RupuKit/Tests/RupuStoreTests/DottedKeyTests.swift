import Testing
import Foundation
@testable import RupuStore

/// Parity coverage for `DottedKey` against its web source of truth,
/// `crates/rupu-cp/web/src/components/ConfigEditor.dotted.test.tsx`'s
/// `quoteSegment`/`splitDottedKey` describe blocks — same cases, ported.
/// Pure functions, no store, no MainActor.

private let GLM_MODEL = "/raid/models/zai-org/GLM-5.2-FP8"
private let GLM_DOTTED_KEY = "pricing.oracle.\"\(GLM_MODEL)\".input_per_mtok"

// MARK: - quoteSegment

@Test func quoteSegmentLeavesAPlainSegmentBare() {
    #expect(DottedKey.quoteSegment("oracle") == "oracle")
    #expect(DottedKey.quoteSegment("default_model") == "default_model")
}

@Test func quoteSegmentQuotesASegmentContainingADot() {
    #expect(DottedKey.quoteSegment(GLM_MODEL) == "\"\(GLM_MODEL)\"")
}

@Test func quoteSegmentEscapesAnEmbeddedQuote() {
    #expect(DottedKey.quoteSegment("has\"quote") == "\"has\\\"quote\"")
}

// MARK: - split

@Test func splitSplitsADottedKeyWithAQuotedDotBearingSegment() {
    #expect(DottedKey.split(GLM_DOTTED_KEY) == ["pricing", "oracle", GLM_MODEL, "input_per_mtok"])
}

@Test func splitSplitsSimpleKeysExactlyLikeANaiveSplit() {
    #expect(DottedKey.split("autoflow.max_active") == ["autoflow", "max_active"])
    #expect(DottedKey.split("default_model") == ["default_model"])
}

@Test func splitFallsBackToANaiveSplitOnAnUnterminatedQuote() {
    let key = "pricing.oracle.\"unterminated"
    #expect(DottedKey.split(key) == key.components(separatedBy: "."))
}

@Test func splitFallsBackToANaiveSplitWhenAQuoteDoesNotSpanTheWholeSegment() {
    let key = "pricing.oracle.\"a\"b.input_per_mtok"
    #expect(DottedKey.split(key) == key.components(separatedBy: "."))
}

// Read-side leniency (deliberate asymmetry with the Rust write-side
// `config_write::split_dotted_key`, which REJECTS every one of these as an
// empty segment — see that function's doc comment). Locked in here so a
// future edit can't accidentally tighten the read path and silently change
// what a malformed/empty key resolves to.
@Test func splitIsLenientAboutEmptySegmentsFromALeadingTrailingOrDoubledSeparator() {
    #expect(DottedKey.split(".") == ["", ""])
    #expect(DottedKey.split("a.") == ["a", ""])
    #expect(DottedKey.split(".x") == ["", "x"])
    #expect(DottedKey.split("a..b") == ["a", "", "b"])
    #expect(DottedKey.split("\"x\".") == ["x", ""])
}

// MARK: - round-trips (join(segments.map(quoteSegment)) inverts split)

@Test func joinThenSplitRoundTripsAKeyWithAQuotedDotBearingSegment() {
    let segments = ["providers", "GLM-5.2-FP8", "model"]
    let joined = DottedKey.join(segments)
    #expect(joined == "providers.\"GLM-5.2-FP8\".model")
    #expect(DottedKey.split(joined) == segments)
}

@Test func joinThenSplitRoundTripsASegmentContainingAQuote() {
    let segments = ["pricing", "has\"quote", "input_per_mtok"]
    let joined = DottedKey.join(segments)
    #expect(DottedKey.split(joined) == segments)
}

@Test func joinThenSplitRoundTripsAPlainKey() {
    let segments = ["a", "b", "c"]
    let joined = DottedKey.join(segments)
    #expect(joined == "a.b.c")
    #expect(DottedKey.split(joined) == segments)
}
