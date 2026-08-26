/// The run graph's kind vocabulary — a Swift-side mirror of the web CP's `StepKind`
/// (`crates/rupu-cp/web/src/lib/workflowGraph.ts`) restricted to the seven kinds a
/// `StepNodeDto.kind` string can actually carry (`step`/`for_each`/`parallel`/`panel`/`gate`/
/// `action`/`run`). The editor-only kinds (`branch`/`split`/`join`) have no run-model
/// counterpart, so they aren't cases here — same restriction the web `kindBridge.ts` module
/// applies via its `RunKind` type.
///
/// Lives in `RupuStore` (perf & interaction arc, Plan 5 Task 3 — moved here alongside
/// `GraphLayout.swift`'s `layoutGraph`/`GraphNodeVM`, which need it): `RunDetailStore`'s derived
/// `graphVM` is store-side state now, and `RupuStore` cannot depend back on `RupuRunDetail`
/// (which depends on it) to reach this type. `RupuRunDetail/KindBridge.swift` still hosts the
/// `accent`/`icon`/`label` presentation extension on this type — those need `RupuDesign`'s
/// `Color`/`LucideIcon`, which `RupuStore` has no reason to expose as its own public surface.
public enum StepKind: String, CaseIterable, Sendable {
    case step
    case forEach = "for_each"
    case parallel
    case panel
    case gate
    case action
    case run

    /// Maps a raw `StepNodeDto.kind` string onto a `StepKind`, falling back to `.step` for
    /// anything unrecognized — the same fallback the web bridge's `STEP_KIND` lookup would hit
    /// if `RunKind` ever grew a member ahead of this enum (there, a compile-time exhaustiveness
    /// failure; here, deliberately permissive since raw strings arrive over the wire).
    public init(raw: String) {
        self = StepKind(rawValue: raw) ?? .step
    }
}
