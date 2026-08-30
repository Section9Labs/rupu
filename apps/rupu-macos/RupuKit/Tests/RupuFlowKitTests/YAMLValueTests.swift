import Testing
@testable import RupuFlowKit

@Test func orderedMappingPreservesInsertionOrder() {
    let v = YAMLValue.mapping([("b", .int(1)), ("a", .int(2))])
    #expect(v.mappingValue?.map(\.key) == ["b", "a"])
}

@Test func subscriptFindsKey() {
    let v = YAMLValue.mapping([("name", .string("x"))])
    #expect(v["name"]?.stringValue == "x")
    #expect(v["missing"] == nil)
}

@Test func equalityIsOrderSensitiveForMappings() {
    #expect(YAMLValue.mapping([("a", .int(1)), ("b", .int(2))]) != YAMLValue.mapping([("b", .int(2)), ("a", .int(1))]))
}

@Test func intValueUnwrapsIntegralDouble() {
    #expect(YAMLValue.double(4).intValue == 4)
    #expect(YAMLValue.double(4.5).intValue == nil)
}

@Test func settingKeyReplacesInPlaceOrAppends() {
    let v = YAMLValue.mapping([("a", .int(1)), ("b", .int(2))])
    #expect(v.mapping(settingKey: "a", to: .int(9)).mappingValue?.map(\.key) == ["a", "b"])
    #expect(v.mapping(settingKey: "c", to: .int(3)).mappingValue?.map(\.key) == ["a", "b", "c"])
}
