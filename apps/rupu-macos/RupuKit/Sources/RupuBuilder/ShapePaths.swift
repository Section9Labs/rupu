import SwiftUI
import RupuDesign
import RupuFlowKit

/// The node body silhouette — wraps `shapeFor(name, w:h:).path` (a `CGPath`
/// authored in LOCAL box coordinates starting at `(0, 0)`) into a SwiftUI
/// `Shape`, offset into whatever `rect` SwiftUI hands it. Kept generically
/// rect-driven (not hardcoded to the 176×68 node box) so a smaller preview
/// (e.g. a future palette-chip swatch) gets correct geometry too, same as
/// `RupuFlowKit`'s own `shapeFor` is size-agnostic.
struct SilhouetteShape: Shape {
    let name: ShapeName

    func path(in rect: CGRect) -> Path {
        let geo = shapeFor(name, w: rect.width, h: rect.height)
        return Path(geo.path).offsetBy(dx: rect.minX, dy: rect.minY)
    }
}

/// The silhouette's extra stroke-only decoration (`NodeShapeGeometry.extra`
/// — the subroutine's rails, the stacked shape's layer lines) — never
/// filled, painted on top of `SilhouetteShape`'s own stroke.
struct SilhouetteExtras: Shape {
    let name: ShapeName

    func path(in rect: CGRect) -> Path {
        let geo = shapeFor(name, w: rect.width, h: rect.height)
        var combined = Path()
        for cgPath in geo.extra {
            combined.addPath(Path(cgPath))
        }
        return combined.offsetBy(dx: rect.minX, dy: rect.minY)
    }
}

/// `KindAccent` token -> resolved themed `Color`. `RupuFlowKit` stays
/// UI-free (no `RupuDesign`/SwiftUI dependency — see `KindVisuals.swift`'s
/// file-header comment), so this mapping lives here, the first layer that
/// actually paints a kind accent. Task 11 brief's exact table.
///
/// `View` carries its own (deprecated) `accentColor(_ color: Color?) ->
/// some View` method — inside a `View` conformer, unqualified lookup
/// prefers that method over a free function of the same name. Call sites
/// inside a `View` body must therefore spell this `RupuBuilder.
/// accentColor(_:)` (the module name as an explicit namespace) to reach
/// THIS function; see `NodeView.swift`'s `accent` property for the pattern.
func accentColor(_ a: KindAccent) -> Color {
    switch a {
    case .statusRunning: Color.status(.running)
    case .brand500: Color.rupuBrand
    case .sevCritical: Color.severity(.crit)
    case .statusAwaiting: Color.status(.awaiting)
    case .statusDone: Color.status(.done)
    case .statusPaused: Color.status(.paused)
    case .sevInfo: Color.severity(.info)
    case .sevMedium: Color.severity(.med)
    case .brand600: Color.rupuBrand600
    case .brand700: Color.rupuBrand700
    }
}
