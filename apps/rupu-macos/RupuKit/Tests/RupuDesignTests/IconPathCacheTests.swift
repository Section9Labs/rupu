import CoreGraphics
import Testing
@testable import RupuDesign

/// Equivalence tests (Plan 5, Task 1 — allocation-storm fixes) for `IconShape`'s one-time
/// `parsedPaths` cache: every `LucideIcon` case's cached `SVGPath`s must render an identical
/// `CGPath` (same element count, same bounding box) to a fresh `SVGPath(d:)` parse of
/// `LucideIconData.paths(for:)` — the exact computation `path(in:)` used to redo on every render.

private let testRect = CGRect(x: 0, y: 0, width: 24, height: 24)
private let viewBox: CGFloat = 24

private func elementCount(_ path: CGPath) -> Int {
    var count = 0
    path.applyWithBlock { _ in count += 1 }
    return count
}

private func union(_ paths: [SVGPath]) -> CGPath {
    let combined = CGMutablePath()
    for p in paths {
        combined.addPath(p.cgPath(in: testRect, viewBox: viewBox))
    }
    return combined
}

@Test func cachedIconPathsMatchFreshParseForEveryIcon() {
    for icon in LucideIcon.allCases {
        let cached = cachedSVGPaths(for: icon)
        let fresh = LucideIconData.paths(for: icon).compactMap { SVGPath(d: $0) }

        #expect(cached.count == fresh.count, "\(icon): cached path count != fresh parse count")

        let cachedPath = union(cached)
        let freshPath = union(fresh)
        #expect(
            elementCount(cachedPath) == elementCount(freshPath),
            "\(icon): CGPath element count mismatch between cache and fresh parse"
        )
        #expect(
            cachedPath.boundingBoxOfPath == freshPath.boundingBoxOfPath,
            "\(icon): CGPath bounding box mismatch between cache and fresh parse"
        )
    }
}

/// Guards against the cache silently going empty for some future `LucideIcon` case (e.g. a
/// generated-data drift) — every case must have parsed to at least one non-empty `SVGPath`
/// (mirrors `LucideIconDataTests`' existing "every case has at least one path" guarantee).
@Test func cachedIconPathsAreNeverEmpty() {
    for icon in LucideIcon.allCases {
        #expect(!cachedSVGPaths(for: icon).isEmpty, "\(icon): cached path list is empty")
    }
}
