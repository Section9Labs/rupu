import Testing
@testable import RupuProjects
import RupuAPI

/// Exercises `CodeTab`'s pure static seams directly — a SwiftUI `body`
/// can't be meaningfully unit-rendered, so (same idiom `ClaimsTableTests`
/// establishes for `ClaimTableRow`/`ClaimsTable`) the view-member logic
/// that decides WHAT to show is pulled out as `static func`s these tests
/// call directly. `CodeTab` the TYPE is `public` (it's rendered from
/// `RupuProjects`'s own `ProjectDetailScreen`); it's these `static func`
/// seams specifically that are `internal` (not `private`, but not `public`
/// either) — `@testable import RupuProjects` is what reaches past that
/// (review round 1 fix — this previously, incorrectly, claimed the TYPE
/// itself was internal).
@Suite
@MainActor
struct CodeTabTests {
    // MARK: - `sortedEntries` — dirs first, alphabetical within each group

    @Test func sortedEntriesPutsDirectoriesBeforeFilesRegardlessOfInputOrder() {
        let entries = [
            APITreeEntry(name: "main.rs", path: "main.rs", kind: "file"),
            APITreeEntry(name: "auth", path: "auth", kind: "dir"),
            APITreeEntry(name: "README.md", path: "README.md", kind: "file"),
            APITreeEntry(name: "utils", path: "utils", kind: "dir"),
        ]

        let sorted = CodeTab.sortedEntries(entries)

        // Files sort case-insensitively too: "main.rs" < "README.md".
        #expect(sorted.map(\.name) == ["auth", "utils", "main.rs", "README.md"])
    }

    @Test func sortedEntriesOrdersWithinEachGroupCaseInsensitively() {
        let entries = [
            APITreeEntry(name: "zebra", path: "zebra", kind: "dir"),
            APITreeEntry(name: "Apple", path: "Apple", kind: "dir"),
            APITreeEntry(name: "banana.txt", path: "banana.txt", kind: "file"),
            APITreeEntry(name: "Apple.txt", path: "Apple.txt", kind: "file"),
        ]

        let sorted = CodeTab.sortedEntries(entries)

        #expect(sorted.map(\.name) == ["Apple", "zebra", "Apple.txt", "banana.txt"])
    }

    @Test func sortedEntriesOfAnEmptyListIsEmpty() {
        #expect(CodeTab.sortedEntries([]).isEmpty)
    }

    // MARK: - `showsParentRow` — `""` is a valid parent, `nil` is not

    @Test func showsParentRowIsTrueForTheEmptyStringParent() {
        // The workspace root's own direct children carry `parent: ""`, not
        // `nil` — `""` is a real, navigable target (the root itself), so
        // the `..` row must still show. Never nil-coalesce this away.
        #expect(CodeTab.showsParentRow(parent: "") == true)
    }

    @Test func showsParentRowIsTrueForANonEmptyParent() {
        #expect(CodeTab.showsParentRow(parent: "src") == true)
    }

    @Test func showsParentRowIsFalseOnlyForANilParent() {
        // `nil` means `tree` already IS the workspace root's own listing —
        // there is nowhere further up to navigate.
        #expect(CodeTab.showsParentRow(parent: nil) == false)
    }

    // MARK: - `filteredFiles` — client-side contains-match

    @Test func filteredFilesMatchesCaseInsensitiveSubstringAnywhereInThePath() {
        let files = ["README.md", "src/auth/session.rs", "src/main.rs"]

        #expect(CodeTab.filteredFiles(files, query: "auth") == ["src/auth/session.rs"])
        #expect(CodeTab.filteredFiles(files, query: "SRC/MAIN") == ["src/main.rs"])
        #expect(CodeTab.filteredFiles(files, query: "rs") == ["src/auth/session.rs", "src/main.rs"])
    }

    @Test func filteredFilesOfAnEmptyQueryReturnsEveryFile() {
        let files = ["a.rs", "b.rs"]
        #expect(CodeTab.filteredFiles(files, query: "") == files)
    }

    @Test func filteredFilesOfNoMatchesIsEmpty() {
        let files = ["a.rs", "b.rs"]
        #expect(CodeTab.filteredFiles(files, query: "nonexistent").isEmpty)
    }

    // MARK: - `truncationFooterText`

    @Test func truncationFooterTextIsNilWhenNotTruncated() {
        #expect(CodeTab.truncationFooterText(truncated: false) == nil)
    }

    @Test func truncationFooterTextIsTheExactWordingWhenTruncated() {
        #expect(CodeTab.truncationFooterText(truncated: true) == "+ more (list truncated)")
    }

    // MARK: - `unavailableMessage` — honest `reason` disclosure

    @Test func unavailableMessageRendersTheServerReasonVerbatim() {
        let file = APIFileContent(
            available: false, path: "bin/blob", language: nil, totalLines: nil, lines: nil,
            reason: "binary file"
        )
        #expect(CodeTab.unavailableMessage(file) == "binary file")
    }

    @Test func unavailableMessageFallsBackHonestlyWhenReasonIsAbsent() {
        let file = APIFileContent(
            available: false, path: "bin/blob", language: nil, totalLines: nil, lines: nil,
            reason: nil
        )
        #expect(!CodeTab.unavailableMessage(file).isEmpty, "must never render a blank pane")
    }

    // MARK: - `lineNumberGutterWidth` — sized to fit the digit count (review round 1)

    @Test func lineNumberGutterWidthKeepsTheOriginalFloorForSmallFiles() {
        #expect(CodeTab.lineNumberGutterWidth(totalLines: 3) == 40)
        #expect(CodeTab.lineNumberGutterWidth(totalLines: nil) == 40)
    }

    @Test func lineNumberGutterWidthGrowsWithDigitCountForLargeFiles() {
        let fiveDigitWidth = CodeTab.lineNumberGutterWidth(totalLines: 12345)
        let sixDigitWidth = CodeTab.lineNumberGutterWidth(totalLines: 123456)

        #expect(fiveDigitWidth > 40, "a 5-digit line count must not be clipped by the small-file floor")
        #expect(sixDigitWidth > fiveDigitWidth, "more digits must widen the gutter further")
    }
}
