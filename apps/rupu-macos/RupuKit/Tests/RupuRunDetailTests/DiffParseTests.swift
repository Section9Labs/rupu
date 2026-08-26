import Testing
@testable import RupuRunDetail

/// Pure `parseUnifiedDiff` tests — ported case-for-case from the web's own
/// `DiffView.test.ts` (`crates/rupu-cp/web/src/components/transcript/
/// DiffView.test.ts`), with every `ctx` expectation on a file-header line
/// (`---`/`+++`/`diff --git`/`index `) updated to this port's distinct
/// `meta` case — see `DiffLine`'s doc comment for why.

@Test func parsesMinimalHunkDelAdd() {
    let result = parseUnifiedDiff("@@ -1,1 +1,1 @@\n- old\n+ new")
    #expect(result == [
        .hunk("@@ -1,1 +1,1 @@"),
        .del("- old"),
        .add("+ new"),
    ])
}

@Test func doesNotMistypeFileHeaderMinusAsDel() {
    let result = parseUnifiedDiff("--- a/src/foo.rs\n+++ b/src/foo.rs")
    #expect(result[0] == .meta("--- a/src/foo.rs"))
    #expect(result[1] == .meta("+++ b/src/foo.rs"))
}

@Test func doesNotMistypeFileHeaderPlusAsAdd() {
    let result = parseUnifiedDiff("+++ b/src/bar.ts")
    #expect(result[0] == .meta("+++ b/src/bar.ts"))
}

@Test func treatsDiffGitLineAsMeta() {
    let result = parseUnifiedDiff("diff --git a/x b/x")
    #expect(result[0] == .meta("diff --git a/x b/x"))
}

@Test func treatsIndexLineAsMeta() {
    let result = parseUnifiedDiff("index abc123..def456 100644")
    #expect(result[0] == .meta("index abc123..def456 100644"))
}

@Test func treatsContextLinesAsCtx() {
    let result = parseUnifiedDiff(" unchanged line")
    #expect(result[0] == .ctx(" unchanged line"))
}

@Test func handlesAFullRealisticDiff() {
    let diff = [
        "diff --git a/src/lib.rs b/src/lib.rs",
        "index abc..def 100644",
        "--- a/src/lib.rs",
        "+++ b/src/lib.rs",
        "@@ -1,3 +1,3 @@",
        " pub fn hello() {",
        "-    println!(\"old\");",
        "+    println!(\"new\");",
        " }",
    ].joined(separator: "\n")

    let result = parseUnifiedDiff(diff)

    #expect(result[0] == .meta("diff --git a/src/lib.rs b/src/lib.rs"))
    #expect(result[1] == .meta("index abc..def 100644"))
    #expect(result[2] == .meta("--- a/src/lib.rs"))
    #expect(result[3] == .meta("+++ b/src/lib.rs"))
    #expect(result[4] == .hunk("@@ -1,3 +1,3 @@"))
    #expect(result[5] == .ctx(" pub fn hello() {"))
    #expect(result[6] == .del("-    println!(\"old\");"))
    #expect(result[7] == .add("+    println!(\"new\");"))
    #expect(result[8] == .ctx(" }"))
}

@Test func returnsEmptyArrayForEmptyString() {
    #expect(parseUnifiedDiff("") == [])
}

@Test func skipsEmptyTrailingLine() {
    let result = parseUnifiedDiff("@@ -1 +1 @@\n")
    #expect(result.count == 1)
    #expect(result[0] == .hunk("@@ -1 +1 @@"))
}

@Test func keepsANonTrailingEmptyLineAsCtx() {
    // Only the LAST empty line (from a trailing "\n") is dropped — an
    // empty line in the middle of a diff is rare but valid context.
    let result = parseUnifiedDiff("@@ -1,2 +1,2 @@\n ctx before\n\n ctx after")
    #expect(result == [
        .hunk("@@ -1,2 +1,2 @@"),
        .ctx(" ctx before"),
        .ctx(""),
        .ctx(" ctx after"),
    ])
}
