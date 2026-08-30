/// An ordered YAML value tree.
///
/// Mirrors the shape js-yaml produces when parsing a workflow document:
/// scalars, sequences, and mappings. Swift's `Dictionary` does not preserve
/// insertion order, but round-tripping a workflow YAML document (in
/// particular re-emitting `meta.rest`/passthrough keys the app doesn't
/// otherwise understand) depends on preserving the author's original key
/// order — so `.mapping` carries an ordered `[(key: String, value:
/// YAMLValue)]` array rather than a `Dictionary`.
public enum YAMLValue: Sendable {
    case null
    case bool(Bool)
    case int(Int)
    case double(Double)
    case string(String)
    case sequence([YAMLValue])
    case mapping([(key: String, value: YAMLValue)])

    /// Looks up a key in a `.mapping`. Returns `nil` for non-mappings or a
    /// missing key.
    public subscript(key: String) -> YAMLValue? {
        mappingValue?.first { $0.key == key }?.value
    }

    public var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    /// Unwraps `.int` directly, or an exact-integral `.double` (e.g. `4.0`
    /// but not `4.5`).
    public var intValue: Int? {
        switch self {
        case .int(let value):
            return value
        case .double(let value):
            guard value.truncatingRemainder(dividingBy: 1) == 0,
                let exact = Int(exactly: value)
            else { return nil }
            return exact
        default:
            return nil
        }
    }

    public var boolValue: Bool? {
        if case .bool(let value) = self { return value }
        return nil
    }

    public var sequenceValue: [YAMLValue]? {
        if case .sequence(let value) = self { return value }
        return nil
    }

    public var mappingValue: [(key: String, value: YAMLValue)]? {
        if case .mapping(let value) = self { return value }
        return nil
    }

    /// Returns a copy of `self` with `key` set to `value`: replaces the
    /// existing entry in place (preserving its position) if `key` is
    /// already present, otherwise appends a new entry. Non-mapping
    /// receivers are returned unchanged.
    public func mapping(settingKey key: String, to value: YAMLValue) -> YAMLValue {
        guard var entries = mappingValue else { return self }
        if let index = entries.firstIndex(where: { $0.key == key }) {
            entries[index] = (key: key, value: value)
        } else {
            entries.append((key: key, value: value))
        }
        return .mapping(entries)
    }
}

extension YAMLValue: Equatable {
    // Tuple-array associated values don't get synthesized `Equatable`
    // conformance, so `.mapping` is compared manually: same count, then
    // zipped key/value pairs in order (order-sensitive, per the doc above).
    public static func == (lhs: YAMLValue, rhs: YAMLValue) -> Bool {
        switch (lhs, rhs) {
        case (.null, .null):
            return true
        case (.bool(let l), .bool(let r)):
            return l == r
        case (.int(let l), .int(let r)):
            return l == r
        case (.double(let l), .double(let r)):
            return l == r
        case (.string(let l), .string(let r)):
            return l == r
        case (.sequence(let l), .sequence(let r)):
            return l == r
        case (.mapping(let l), .mapping(let r)):
            guard l.count == r.count else { return false }
            return zip(l, r).allSatisfy { $0.key == $1.key && $0.value == $1.value }
        default:
            return false
        }
    }
}
