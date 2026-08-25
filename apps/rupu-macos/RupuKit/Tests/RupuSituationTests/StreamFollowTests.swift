import Testing
@testable import RupuSituation

// Table tests for `StreamFollow.swift`'s pure functions (redesign pass,
// Task 4). Plain `@Test func`s, no `@MainActor` needed — these are
// top-level functions, not members of a View type (unlike
// `SituationRoomScreen.shouldActivate`, which DOES need it — see that
// type's own test file header for why).

private func card(_ key: String, group: CardGroup = .activity) -> StreamCard {
    StreamCard(key: key, ts: 0, form: .activity, group: group, accent: .brand, badge: "b", title: "t")
}

@Suite
struct IsFollowingTests {
    @Test func atTheVeryTopIsFollowing() {
        #expect(isFollowing(offsetFromTop: 0))
    }

    @Test func justBelowTheThresholdIsStillFollowing() {
        #expect(isFollowing(offsetFromTop: 47.9))
    }

    @Test func exactlyAtTheThresholdIsNotFollowing() {
        // `EventStream.tsx` line 65: `follow = scrollTop < 48` — strictly
        // less-than, so 48 itself already suspends.
        #expect(!isFollowing(offsetFromTop: 48))
    }

    @Test func wellPastTheThresholdIsNotFollowing() {
        #expect(!isFollowing(offsetFromTop: 400))
    }

    @Test func aCustomThresholdIsHonored() {
        #expect(!isFollowing(offsetFromTop: 10, threshold: 5))
        #expect(isFollowing(offsetFromTop: 4, threshold: 5))
    }
}

@Suite
struct NextStreamRenderStateTests {
    @Test func resumingAlwaysClearsAnyExistingFreeze() {
        let frozen = StreamRenderState(frozenKeys: ["a", "b"])
        let next = nextStreamRenderState(frozen, following: true, currentKeys: ["a", "b", "c"])
        #expect(next.frozenKeys == nil)
    }

    @Test func resumingFromAlreadyUnfrozenStaysUnfrozen() {
        let next = nextStreamRenderState(StreamRenderState(), following: true, currentKeys: ["a"])
        #expect(next.frozenKeys == nil)
    }

    @Test func suspendingFromUnfrozenCapturesTheCurrentKeysAsTheBaseline() {
        let next = nextStreamRenderState(StreamRenderState(), following: false, currentKeys: ["a", "b"])
        #expect(next.frozenKeys == ["a", "b"])
    }

    @Test func suspendingAgainWhileAlreadyFrozenIsANoOpThatKeepsTheOriginalBaseline() {
        // The operator scrolled further without ever resuming — a second
        // scroll event that still reads as "suspended" must not re-capture
        // a NEWER baseline; the original reading position's meaning must
        // hold for the whole uninterrupted read.
        let frozen = StreamRenderState(frozenKeys: ["a"])
        let next = nextStreamRenderState(frozen, following: false, currentKeys: ["a", "b", "c"])
        #expect(next.frozenKeys == ["a"], "the baseline from the FIRST suspend must survive, not the later, larger key set")
    }
}

@Suite
struct PlanStreamRenderTests {
    @Test func followingRendersTheFullStreamWithNothingDeferred() {
        let all = [card("a"), card("b"), card("c")]
        let plan = planStreamRender(all: all, state: StreamRenderState())
        #expect(plan.shown.map(\.key) == ["a", "b", "c"])
        #expect(plan.deferredCount == 0)
    }

    @Test func suspendedRendersOnlyTheFrozenSubsetInTheFullStreamsOrder() {
        let all = [card("new2"), card("new1"), card("a"), card("b")]
        let state = StreamRenderState(frozenKeys: ["a", "b"])
        let plan = planStreamRender(all: all, state: state)
        #expect(plan.shown.map(\.key) == ["a", "b"], "order follows `all`, not the frozen set's own order")
        #expect(plan.deferredCount == 2, "new2 and new1 are held back — the real count, not an estimate")
    }

    @Test func aFrozenKeyThatFellOutOfTheStreamEntirelyIsSimplyAbsentNotACrash() {
        // e.g. the underlying cap trimmed it — `planStreamRender` must not
        // assume every frozen key still exists in `all`.
        let all = [card("a")]
        let state = StreamRenderState(frozenKeys: ["a", "evicted"])
        let plan = planStreamRender(all: all, state: state)
        #expect(plan.shown.map(\.key) == ["a"])
        #expect(plan.deferredCount == 0)
    }

    @Test func emptyFrozenSetRendersNothingButStillReportsEveryArrivalAsDeferred() {
        let all = [card("a"), card("b")]
        let state = StreamRenderState(frozenKeys: [])
        let plan = planStreamRender(all: all, state: state)
        #expect(plan.shown.isEmpty)
        #expect(plan.deferredCount == 2)
    }
}
