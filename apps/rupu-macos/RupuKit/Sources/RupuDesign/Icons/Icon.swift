import SwiftUI

/// The lucide icons rupu.app uses. Case names mirror lucide's own component names (camelCased);
/// where lucide's *filename* has since diverged from that name (five aliases in lucide-react
/// 0.468.0 — see `apps/rupu-macos/scripts/extract-lucide.mjs`), the case name and glyph are kept
/// stable here regardless — web parity is by glyph, not by lucide's current filename.
///
/// Keep in lockstep with the `ICONS` table in `apps/rupu-macos/scripts/extract-lucide.mjs`: every
/// case needs a matching table entry (enforced by `LucideIconDataTests`, which asserts every case
/// has at least one path).
public enum LucideIcon: String, CaseIterable, Sendable {
    case layoutDashboard
    case activity
    case sparkles
    case workflow
    case repeatIcon = "repeat"
    case messageSquare
    case folderGit2
    case folder
    case shieldCheck
    case shieldAlert
    case network
    case bookMarked
    case server
    case dollarSign
    case settings
    case radio
    case play
    case checkCircle2
    case xCircle
    case pause
    case pauseCircle
    case xOctagon
    case ban
    case skipForward
    case clock
    case arrowLeft
    case archive
    case trash2
    case gitBranch
    case listOrdered
    case fileText
    case chevronUp
    case chevronDown
    case moreHorizontal
    case search
    case lock
    case bot
    case columns3
    case userCheck
    case zap
    case terminal
}

/// Renders a `LucideIcon` by stroking its constituent SVG paths (`LucideIconData.paths(for:)`,
/// parsed via `SVGPathParser`) scaled into a `size × size` frame.
///
/// `weight` is lucide's own stroke width at the 24pt viewBox the source paths were authored for
/// (lucide's default is 2); it's scaled by `size / 24` so line weight stays proportionate at any
/// requested `size`, matching what lucide's React components do.
public struct Icon: View {
    private let icon: LucideIcon
    private let size: CGFloat
    private let weight: CGFloat

    public init(_ icon: LucideIcon, size: CGFloat = 16, weight: CGFloat = 2) {
        self.icon = icon
        self.size = size
        self.weight = weight
    }

    public var body: some View {
        IconShape(icon: icon)
            .stroke(style: StrokeStyle(lineWidth: weight * size / 24, lineCap: .round, lineJoin: .round))
            .frame(width: size, height: size)
    }
}

/// A `Shape` that unions every path `LucideIconData` returns for `icon` into one `CGPath`, scaled
/// from the 24×24 viewBox the paths were authored in into whatever rect SwiftUI lays the shape out
/// in (the `Icon` view above always makes that a `size × size` square).
private struct IconShape: Shape {
    let icon: LucideIcon

    /// Every icon's path strings, parsed exactly once. `SVGPath(d:)` is a full character-scan
    /// parse; `path(in:)` used to re-run it for every path string on every `body` evaluation
    /// (every icon, every render). Swift's `static let` initializer runs at most once and is
    /// thread-safe, so this builds the whole table in one pass over `LucideIcon.allCases` at
    /// first access and every subsequent `path(in:)` call is a dictionary lookup + `cgPath`
    /// scale — zero string parsing per render.
    ///
    /// `fileprivate` (not `private`) so `cachedSVGPaths(for:)` below — a narrow, file-scoped test
    /// seam — can read it without making the cache genuinely public API.
    fileprivate static let parsedPaths: [LucideIcon: [SVGPath]] = {
        var table: [LucideIcon: [SVGPath]] = [:]
        table.reserveCapacity(LucideIcon.allCases.count)
        for icon in LucideIcon.allCases {
            table[icon] = LucideIconData.paths(for: icon).compactMap { SVGPath(d: $0) }
        }
        return table
    }()

    func path(in rect: CGRect) -> Path {
        let combined = CGMutablePath()
        for svgPath in IconShape.parsedPaths[icon] ?? [] {
            combined.addPath(svgPath.cgPath(in: rect, viewBox: 24))
        }
        return Path(combined)
    }
}

/// Test-only seam (Plan 5, Task 1 — allocation-storm fixes): exposes `IconShape`'s cached,
/// already-parsed `SVGPath`s for `icon` so `RupuDesignTests` can assert the cache produces
/// output identical to a fresh `SVGPath(d:)` parse, without promoting `IconShape` (a SwiftUI
/// `Shape` implementation detail) to public API. Internal visibility only — reachable from this
/// package's own `@testable import`, not from outside `RupuKit`.
func cachedSVGPaths(for icon: LucideIcon) -> [SVGPath] {
    IconShape.parsedPaths[icon] ?? []
}
