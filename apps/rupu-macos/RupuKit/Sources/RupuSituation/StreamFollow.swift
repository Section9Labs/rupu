// Situation Room (redesign pass, Task 4) — follow/pin scroll contract. Port
// of `EventStream.tsx` lines 56-65's `follow`/`scrollRef` story, verified
// full read of the surrounding component (the whole 142-line file — no
// separate "jump to latest (N new)" affordance exists anywhere in that
// component or its sole page consumer, `Events.tsx`; see
// `EventStreamColumn.swift`'s file header for that correction against this
// task's own brief).
//
// Everything here is a free, non-isolated, pure function or value type —
// `EventStreamColumn` (the sole caller) drives it from `@State`, but nothing
// in this file itself touches SwiftUI, so it's directly `@Test`-able with
// no `@MainActor` needed (these are not View-type members — the
// `SituationRoomScreen.shouldActivate` precedent that DOES need
// `@MainActor` is a `static func` on a View struct; these are plain
// top-level functions).

/// Follow/pin decision — the web's `EventStream.tsx` line 58 default
/// (`useState(true)`) plus line 65's `setFollow(el.scrollTop < 48)`: no
/// hysteresis, no separate "resume" gesture — scrolling back within 48px of
/// the top on the web silently resumes following on its own, purely as a
/// function of the CURRENT scroll position. `offsetFromTop` is this port's
/// analog of the web's `scrollTop` (pixels scrolled down from the top) —
/// points scrolled down from the top of `EventStreamColumn`'s newest-first
/// stream. Both are logical, DPI-independent units on their own platform,
/// so the 48px web threshold ports unchanged as 48pt. `Double`, not
/// `CGFloat` — this file otherwise touches nothing from `CoreGraphics`/
/// `SwiftUI` (see the file header), so the caller (`EventStreamColumn`,
/// which already imports `SwiftUI`) converts its `CGFloat` scroll offset at
/// the call site instead.
public func isFollowing(offsetFromTop: Double, threshold: Double = 48) -> Bool {
    offsetFromTop < threshold
}

/// The frozen baseline while the operator has scrolled away — see
/// `EventStreamColumn.swift`'s file header for why this exists at all (the
/// "content must not shift under a reading operator" contract, and why this
/// port takes a different mechanism than the web's own to hold it).
/// `nil` while following: nothing is held back, `planStreamRender` below
/// just passes the full stream through.
public struct StreamRenderState: Equatable, Sendable {
    public var frozenKeys: Set<String>?

    public init(frozenKeys: Set<String>? = nil) {
        self.frozenKeys = frozenKeys
    }
}

/// Advances `state` at a follow-state EDGE — call this only when
/// `following` itself changes (`EventStreamColumn`'s `.onChange(of:
/// following)`), never on every card arrival; `planStreamRender` below is
/// what runs on every arrival instead, against whatever baseline this last
/// left in place.
///
/// - Resuming (`following == true`) always clears the freeze outright —
///   matches the web's own "no partial resume" shape: the moment `follow`
///   flips true, the very next layout effect pins straight back to the
///   newest card (`EventStream.tsx` lines 56-59), nothing stays held back.
/// - Suspending (`following == false`) captures `currentKeys` as the new
///   frozen baseline — but ONLY if nothing is already frozen. A second
///   consecutive suspend (already suspended, scrolled further without ever
///   resuming) is a no-op: the ORIGINAL baseline from the moment reading
///   started stays fixed, so a still-reading operator's viewport keeps
///   meaning the same thing throughout one uninterrupted read, not just
///   between individual scroll events.
public func nextStreamRenderState(
    _ state: StreamRenderState,
    following: Bool,
    currentKeys: Set<String>
) -> StreamRenderState {
    if following { return StreamRenderState(frozenKeys: nil) }
    if state.frozenKeys != nil { return state }
    return StreamRenderState(frozenKeys: currentKeys)
}

/// What `EventStreamColumn` actually renders this tick — pure, safe to call
/// on every body evaluation (no state mutation of its own): the full merged
/// stream while nothing is frozen; only the frozen subset, in `all`'s own
/// order, while suspended — plus the real count of arrivals being held
/// back, which backs the "N new" jump-back affordance.
///
/// **Why this diverges from the web's own mechanism, stated honestly**
/// (this task's interface note: "port the web's actual approach: check
/// whether the web inserts always and relies on scroll anchoring, or defers
/// rendering while scrolled"). Verified: the web inserts always.
/// `EventStream.tsx` lines 104-124 render the FULL, unfiltered-by-follow
/// `shown` array every time regardless of `follow`, and the file has no
/// other scroll-offset compensation anywhere in it — it relies entirely on
/// the browser's OWN native CSS scroll anchoring (`overflow-anchor: auto`,
/// on by default in every evergreen browser since ~2017), which
/// transparently keeps a scrolled-down reader's visual position stable when
/// the DOM grows above the fold. SwiftUI's `ScrollView`/`LazyVStack` has no
/// platform equivalent of that feature, and a `GeometryReader`-based
/// content-offset patch against a LAZY, virtualized stack is a well-known
/// fragile pattern (a cell's geometry isn't stable while it scrolls in/out
/// of the render window, so the "measure the delta and correct scrollTop"
/// trick that works against a plain `VStack` does not transfer cleanly).
/// So this port takes the documented alternative instead: defer rendering.
/// New arrivals are held out of the list entirely while suspended, rather
/// than inserted above an anchor this platform can't stabilize — the
/// already-rendered rows literally never move, a STRONGER guarantee than
/// the web's platform-assisted approximation, not a weaker one — and the
/// exact `deferredCount` this produces is real data, not an estimate,
/// because it's just "how many cards exist that aren't in the frozen set."
public func planStreamRender(all: [StreamCard], state: StreamRenderState) -> (shown: [StreamCard], deferredCount: Int) {
    guard let frozen = state.frozenKeys else { return (all, 0) }
    let shown = all.filter { frozen.contains($0.key) }
    return (shown, all.count - shown.count)
}
