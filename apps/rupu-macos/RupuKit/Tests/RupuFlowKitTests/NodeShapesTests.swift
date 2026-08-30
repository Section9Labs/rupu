import Testing

@testable import RupuFlowKit

@Test func allShapesAreClosedPolygonsInsideBox() {
    for name in ShapeName.allCases {
        let s = shapeFor(name, w: 176, h: 68)
        for p in s.points {
            #expect(p.x >= 0 && p.x <= 176 && p.y >= 0 && p.y <= 68, "\(name) point escapes box")
        }
        #expect(s.safe.x >= 0 && s.safe.x + s.safe.w <= 176 && s.safe.y >= 0 && s.safe.y + s.safe.h <= 68, "\(name) safe rect escapes box")
    }
}

@Test func hexagonInsetsClampAtPaletteSize() {
    // 34x20 palette chip: POINT (22) must clamp to (34-4)*0.3 = 9.
    let s = shapeFor(.hexagon, w: 34, h: 20)
    #expect(abs(s.points[0].x - 9) < 0.001)
}

@Test func vhexHasTwoLabelledSources() {
    let s = shapeFor(.vhex, w: 176, h: 68)
    #expect(s.sources.map(\.arm) == ["then", "else"])
    #expect(s.sources[1].anchor.side == .bottom)
    #expect(s.centered)
}

@Test func parallelogramHandleInsetIsSlantMidpoint() {
    let s = shapeFor(.parallelogram, w: 176, h: 68)
    #expect(abs(s.sources[0].anchor.inset - 11) < 0.001) // (SHEAR 20 + I 2)/2
}

@Test func stackedSourceInsetClearsLayers() {
    let s = shapeFor(.stacked, w: 176, h: 68)
    #expect(abs(s.sources[0].anchor.inset - 11) < 0.001) // LAYER 9 + I 2
}

@Test func subroutineHasTwoRails() {
    #expect(shapeFor(.subroutine, w: 176, h: 68).extra.count == 2)
}
