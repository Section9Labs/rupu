/// Minimum `rupu` server version the app is willing to talk to.
///
/// Comparison is a numeric dot-segment compare (`major.minor.patch`),
/// tolerant of a trailing prerelease suffix (`0.72.0-beta.1` compares as
/// `0.72.0`). Anything that doesn't parse as at least three numeric
/// segments is treated as incompatible.
public enum VersionGate {
    public static let minimum = "0.74.0"

    public static func compatible(_ version: String) -> Bool {
        guard let candidate = numericSegments(of: version) else { return false }
        guard let floor = numericSegments(of: minimum) else { return false }
        return compare(candidate, floor) >= 0
    }

    /// Parses `major.minor.patch` (ignoring any `-prerelease`/`+build`
    /// suffix on the final segment) into an array of integers. Returns
    /// `nil` if fewer than three numeric segments are present.
    private static func numericSegments(of version: String) -> [Int]? {
        let core = version.split(separator: "+", maxSplits: 1)[0]
        let parts = core.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count >= 3 else { return nil }

        var segments: [Int] = []
        for (index, part) in parts.prefix(3).enumerated() {
            let numeric: Substring
            if index == 2 {
                // Trailing segment may carry a "-beta.1" style suffix.
                numeric = part.split(separator: "-", maxSplits: 1)[0]
            } else {
                numeric = part
            }
            guard let value = Int(numeric) else { return nil }
            segments.append(value)
        }
        return segments
    }

    /// Lexicographic compare of two equal-length integer arrays.
    private static func compare(_ lhs: [Int], _ rhs: [Int]) -> Int {
        for (l, r) in zip(lhs, rhs) {
            if l != r { return l < r ? -1 : 1 }
        }
        return 0
    }
}
