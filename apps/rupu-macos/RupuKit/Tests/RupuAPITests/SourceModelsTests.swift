import Testing
import Foundation
@testable import RupuAPI

// MARK: - Decode: run_source.json fixture

@Test func decodesRunSourceFixtureAvailableAndUnavailableCases() throws {
    let slices = try JSONDecoder().decode([APISourceSlice].self, from: Fixtures.data("run_source.json"))
    #expect(slices.count == 2)

    let available = slices[0]
    #expect(available.available)
    #expect(available.path == "src/auth/session.rs")
    #expect(available.language == "rust")
    #expect(available.startLine == 40)
    #expect(available.endLine == 44)
    #expect(available.targetLine == 42)
    #expect(available.totalLines == 120)
    let lines = try #require(available.lines)
    #expect(lines.count == 5)
    #expect(lines[0].n == 40)
    #expect(lines[2].text == "        if token == stored_token {")
    #expect(available.reason == nil)

    let unavailable = slices[1]
    #expect(!unavailable.available)
    #expect(unavailable.path == nil)
    #expect(unavailable.lines == nil)
    // The brief's "REMOTE_NOT_SUPPORTED" is the Rust constant's NAME
    // (`crates/rupu-cp/src/api/source.rs:55`), not the wire value — the
    // actual `reason` string is the full sentence below.
    #expect(unavailable.reason == "Source preview is not available for remote-host runs yet.")
}

// MARK: - Decode: run_ast.json fixture

@Test func decodesRunAstFixtureWithDepthThreeAndMatchedDeepestNode() throws {
    let ast = try JSONDecoder().decode(APIAstResponse.self, from: Fixtures.data("run_ast.json"))
    #expect(ast.available)
    #expect(ast.language == "rust")
    #expect(ast.truncated == false)
    #expect(ast.reason == nil)

    let root = try #require(ast.root)
    #expect(root.kind == "source_file")
    #expect(root.named)
    #expect(root.field == nil)
    #expect(root.startLine == 1)
    #expect(root.startCol == 1)
    #expect(root.endLine == 3)
    #expect(root.endCol == 2)
    #expect(!root.matched)
    #expect(root.children.count == 1)

    // Depth 2: function_item, unmatched.
    let functionItem = root.children[0]
    #expect(functionItem.kind == "function_item")
    #expect(!functionItem.matched)
    #expect(functionItem.children.count == 1)

    // Depth 3: identifier — the deepest node, matched:true, field "name",
    // no children. Reaching this asserts recursion depth >= 3.
    let identifier = functionItem.children[0]
    #expect(identifier.kind == "identifier")
    #expect(identifier.named)
    #expect(identifier.field == "name")
    #expect(identifier.startLine == 2)
    #expect(identifier.startCol == 8)
    #expect(identifier.endLine == 2)
    #expect(identifier.endCol == 12)
    #expect(identifier.matched)
    #expect(identifier.children.isEmpty)
}
