import Testing
@testable import RupuProjects

/// `ShowAllFooter.resolve(...)` is a plain, non-`View` function (no
/// `@MainActor` needed, per the CLAUDE.md CI rule — same as
/// `RupuDesignTests.ListSortTests` for `sortRows`), so the review fix's
/// three-state "show all" reconciliation is asserted here directly rather
/// than through a render pass.
@Test func totalWithinCapRendersAShowAllButtonThenHidesOnceComplete() {
    // Not yet fetched all — a plain "Show all N" button, honest because
    // showAllLimit can actually reach total.
    let beforeFetch = ShowAllFooter.resolve(loaded: 50, total: 200, showingAll: false, showAllLimit: 1000, noun: "runs")
    #expect(beforeFetch == .button(label: "Show all 200 runs"))

    // After a show-all fetch that genuinely reached every row, the store
    // reports `showingAll == true` — the footer disappears entirely rather
    // than lingering as a now-meaningless button.
    let afterFetch = ShowAllFooter.resolve(loaded: 200, total: 200, showingAll: true, showAllLimit: 1000, noun: "runs")
    #expect(afterFetch == .hidden)
}

@Test func totalBeyondCapRendersATruthfulFirstNLabelThenAPersistentNote() {
    // Not yet fetched — the button's own label commits to exactly what
    // tapping it will produce: 1,000 of 5,000, not "all".
    let beforeFetch = ShowAllFooter.resolve(loaded: 50, total: 5000, showingAll: false, showAllLimit: 1000, noun: "runs")
    #expect(beforeFetch == .button(label: "Show first 1,000 of 5,000 runs"))

    // After the capped fetch: `loaded == showAllLimit`, `showingAll` still
    // false (the store never claims completeness it can't prove) — a
    // persistent note, not a dead re-fetch button.
    let afterFetch = ShowAllFooter.resolve(loaded: 1000, total: 5000, showingAll: false, showAllLimit: 1000, noun: "runs")
    #expect(afterFetch == .note("Showing 1,000 of 5,000 runs"))
}

@Test func hiddenWhenTotalIsUnknownOrAlreadyComplete() {
    #expect(ShowAllFooter.resolve(loaded: 50, total: nil, showingAll: false, showAllLimit: 1000, noun: "sessions") == .hidden)
    #expect(ShowAllFooter.resolve(loaded: 10, total: 10, showingAll: false, showAllLimit: 1000, noun: "sessions") == .hidden)
    #expect(ShowAllFooter.resolve(loaded: 10, total: 10, showingAll: true, showAllLimit: 1000, noun: "sessions") == .hidden)
}

/// Same reconciliation as `totalWithinCapRendersAShowAllButtonThenHidesOnceComplete`/
/// `totalBeyondCapRendersATruthfulFirstNLabelThenAPersistentNote`, with the
/// `noun` the Sessions tab actually passes — the brief calls for applying
/// the fix "identically to runs AND sessions", so both nouns get their own
/// assertion rather than assuming the string substitution is bug-free.
@Test func sessionsNounProducesTheSameThreeStatesAsRuns() {
    #expect(ShowAllFooter.resolve(loaded: 50, total: 200, showingAll: false, showAllLimit: 1000, noun: "sessions") == .button(label: "Show all 200 sessions"))
    #expect(ShowAllFooter.resolve(loaded: 50, total: 5000, showingAll: false, showAllLimit: 1000, noun: "sessions") == .button(label: "Show first 1,000 of 5,000 sessions"))
    #expect(ShowAllFooter.resolve(loaded: 1000, total: 5000, showingAll: false, showAllLimit: 1000, noun: "sessions") == .note("Showing 1,000 of 5,000 sessions"))
}
