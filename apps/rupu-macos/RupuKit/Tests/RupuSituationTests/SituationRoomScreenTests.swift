import Testing
@testable import RupuSituation

// Final-review fix, item 3: the Situation Room's two `.onChange` handlers
// spawn UNSTRUCTURED `Task`s (an `.onChange` closure is synchronous, so
// unlike `.task` SwiftUI never cancels them for us). One of those could run
// AFTER `.onDisappear` had already torn the store down, rebuilding it and
// starting a fresh firehose subscription for a scene that no longer exists.
// The guard is a screen-level teardown epoch captured at spawn and
// re-checked immediately before `activate()`; `shouldActivate` is its pure
// half, extracted so it can be pinned without a SwiftUI host (same
// "view-member pure logic gets its own testable static func" idiom
// `SourcePreview.taskID` / `CodeTab.lineNumberGutterWidth` establish).

@Test func aSpawnedActivateRunsWhileTheScreenHasNotTornDown() {
    #expect(SituationRoomScreen.shouldActivate(spawnEpoch: 0, currentEpoch: 0))
    #expect(SituationRoomScreen.shouldActivate(spawnEpoch: 7, currentEpoch: 7))
}

@Test func aSpawnedActivateIsSkippedOnceOnDisappearHasBumpedTheEpoch() {
    // `.onDisappear` bumps the epoch BEFORE calling `deactivate()`/clearing
    // `store`, so any already-spawned task that has not yet re-checked sees
    // the bump and returns without starting a stream.
    #expect(!SituationRoomScreen.shouldActivate(spawnEpoch: 0, currentEpoch: 1))
}

@Test func repeatedTeardownsKeepSkippingEveryTaskSpawnedBeforeThem() {
    // A window reopened and torn down again keeps bumping — a task spawned
    // under ANY earlier epoch stays skipped, never accidentally re-matching.
    for spawnEpoch in 0..<3 {
        #expect(!SituationRoomScreen.shouldActivate(spawnEpoch: spawnEpoch, currentEpoch: 3))
    }
}
