import Testing
import RupuDesign
@testable import RupuBuilder

/// Store-level wiring coverage for the Workflow Builder screen shell (macOS
/// design plan, Task 10) — SwiftUI bodies aren't render-tested in this
/// package (same convention as every other screen module: see
/// `RupuLibraryTests`/`RupuUsageTests`'s own file-header comments). These
/// exercise the two pure seams the header/rail wiring is built on:
/// `validDotTone(serverValid:problems:)` (`BuilderHeader.swift` — backs the
/// header's valid/invalid dot) and `WorkflowBuilderScreen.
/// railTab(afterSelecting:current:)` (backs "selecting a node flips the
/// rail to Step").

// MARK: - validDotTone

@Test func validDotToneIsNilWhileUnchecked() {
    // `serverValid == nil` (no `revalidate()` has completed yet) always
    // renders the mute/unopinionated dot, regardless of local `problems`.
    #expect(validDotTone(serverValid: nil, problems: [:]) == nil)
    #expect(validDotTone(serverValid: nil, problems: ["step1": ["unreachable"]]) == nil)
}

@Test func validDotToneIsDoneWhenServerValidAndProblemFree() {
    #expect(validDotTone(serverValid: true, problems: [:]) == .done)
}

@Test func validDotToneIsFailedWhenServerRejects() {
    #expect(validDotTone(serverValid: false, problems: [:]) == .failed)
    #expect(validDotTone(serverValid: false, problems: ["step1": ["cycle"]]) == .failed)
}

@Test func validDotToneIsFailedWhenServerValidButLocalProblemsExist() {
    // `problems` always wins pessimistically — a server check that ran
    // before a since-added local problem (the 400ms debounce hasn't caught
    // up) must never show green.
    #expect(validDotTone(serverValid: true, problems: ["step1": ["unreachable"]]) == .failed)
}

// MARK: - railTab(afterSelecting:current:)

// `WorkflowBuilderScreen` is a SwiftUI View and therefore `@MainActor`, so
// its static helpers inherit that isolation. Swift Testing runs `@Test`
// funcs in a nonisolated context by default, which makes a synchronous call
// to one of them a hard error rather than a warning. These two tests are the
// only ones in this file that touch the view type — the `validDotTone` ones
// above call a free function — so the annotation goes here rather than on
// the whole suite.
@MainActor
@Test func selectingANodeFlipsRailToStep() {
    #expect(WorkflowBuilderScreen.railTab(afterSelecting: "step1", current: .blocks) == .step)
    #expect(WorkflowBuilderScreen.railTab(afterSelecting: "step1", current: .settings) == .step)
    #expect(WorkflowBuilderScreen.railTab(afterSelecting: "step1", current: .step) == .step)
}

@MainActor
@Test func clearingSelectionLeavesTheCurrentTabUntouched() {
    // Esc / deselect never forces the rail back to Blocks — mirrors the web
    // editor, which only navigates the panel on an explicit tab click.
    #expect(WorkflowBuilderScreen.railTab(afterSelecting: nil, current: .step) == .step)
    #expect(WorkflowBuilderScreen.railTab(afterSelecting: nil, current: .settings) == .settings)
    #expect(WorkflowBuilderScreen.railTab(afterSelecting: nil, current: .blocks) == .blocks)
}
