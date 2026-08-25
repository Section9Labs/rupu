import Testing
import Foundation
@testable import RupuAPI

// MARK: - Decode: code_tree.json fixture

@Test func decodesCodeTreeFixtureAndParentSurvivesAsEmptyStringNotNil() throws {
    let tree = try JSONDecoder().decode(APITreeResult.self, from: Fixtures.data("code_tree.json"))
    #expect(tree.path == "src")
    // `parent == ""` (the workspace root) must decode to `Optional("")`, not
    // `nil` — only the root's own listing carries a `nil` parent.
    #expect(tree.parent == "")
    #expect(tree.entries.count == 2)

    let dir = tree.entries[0]
    #expect(dir.name == "auth")
    #expect(dir.path == "src/auth")
    #expect(dir.kind == "dir")

    let file = tree.entries[1]
    #expect(file.name == "main.rs")
    #expect(file.path == "src/main.rs")
    #expect(file.kind == "file")
}

// MARK: - Decode: code_file.json fixture

@Test func decodesCodeFileFixtureWithCamelCaseTotalLines() throws {
    let file = try JSONDecoder().decode(APIFileContent.self, from: Fixtures.data("code_file.json"))
    #expect(file.available)
    #expect(file.path == "src/main.rs")
    #expect(file.language == "rust")
    // The wire key is `totalLines` (Rust's `FileContent` carries
    // `#[serde(rename_all = "camelCase")]`), not `total_lines`.
    #expect(file.totalLines == 3)
    #expect(file.reason == nil)

    let lines = try #require(file.lines)
    #expect(lines.count == 3)
    #expect(lines[0].n == 1)
    #expect(lines[0].text == "fn main() {")
    #expect(lines[2].text == "}")
}

// MARK: - Decode: code_files.json fixture

@Test func decodesCodeFilesFixtureAsTruncated() throws {
    let files = try JSONDecoder().decode(APIFileList.self, from: Fixtures.data("code_files.json"))
    #expect(files.truncated)
    #expect(files.files == ["README.md", "src/auth/session.rs", "src/main.rs"])
}
