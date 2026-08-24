import SwiftUI
import Testing
@testable import RupuDesign

@Test func metaTextIsSans10() {
    #expect(Font.metaText == Font.system(size: 10))
}

@Test func noteTextIsSans11() {
    #expect(Font.noteText == Font.system(size: 11))
}

@Test func uiTextIsSans12() {
    #expect(Font.uiText == Font.system(size: 12))
}

@Test func leadTextIsSans13() {
    #expect(Font.leadText == Font.system(size: 13))
}

@Test func dataMonoIsMonospacedWithMonospacedDigits() {
    let sizes: [CGFloat] = [8, 10, 11.5, 13]
    for size in sizes {
        let expected = Font.system(size: size, design: .monospaced).monospacedDigit()
        #expect(Font.dataMono(size) == expected, "dataMono(\(size))")
    }
}

@Test func eyebrowUppercasesDisplayText() {
    #expect(Eyebrow("mixed Case Text").displayText == "MIXED CASE TEXT")
    #expect(Eyebrow("already upper").displayText == "ALREADY UPPER")
    #expect(Eyebrow("").displayText == "")
}

@Test func panelStyleRadiiAreSevenAndSix() {
    #expect(PanelStyle.Variant.panel.cornerRadius == 7)
    #expect(PanelStyle.Variant.innerCard.cornerRadius == 6)
}
