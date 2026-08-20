import Foundation

/// Null-discipline formatters shared across the macOS app's read surfaces.
public enum Fmt {
    /// Renders `nil` as an em dash and otherwise as a grouped integer (e.g. `1,234`).
    public static func count(_ n: Int?) -> String {
        guard let n else { return "—" }
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.usesGroupingSeparator = true
        formatter.groupingSeparator = ","
        formatter.decimalSeparator = "."
        return formatter.string(from: NSNumber(value: n)) ?? String(n)
    }

    /// Marks a possibly-incomplete sum with a trailing `+` when the underlying data is partial.
    public static func partial(_ n: Int, isPartial: Bool) -> String {
        isPartial ? "\(count(n))+" : count(n)
    }

    /// Formats a millisecond duration as `0.9s` / `4.2s` below 60s, `1m 12s` below 1h, else `1h 2m`.
    ///
    /// Uses integer (deci-second / second) arithmetic throughout rather than `Double` formatting:
    /// `%.1f` on a raw `ms / 1000.0` double rounds 850ms to "0.8s" (0.85 isn't exactly
    /// representable in binary floating point and lands a hair under it), not the expected "0.9s".
    ///
    /// The unit branch is chosen on the *rounded* value, not the raw `ms`, so a duration that
    /// rounds up to the next unit cascades into that unit's format instead of overflowing the
    /// current one (e.g. 59_950ms rounds to 60.0s, which must render "1m 0s", not "60.0s").
    /// `deciseconds` and `totalSeconds` are each rounded independently from `ms` — not chained
    /// through one another — so a value near the 1h boundary doesn't pick up a second helping of
    /// rounding error from an intermediate decisecond rounding it never needed.
    public static func duration(ms: UInt64) -> String {
        let deciseconds = (ms + 50) / 100
        if deciseconds < 600 {
            let whole = deciseconds / 10
            let frac = deciseconds % 10
            return "\(whole).\(frac)s"
        }
        let totalSeconds = (ms + 500) / 1_000
        if totalSeconds < 3_600 {
            let m = totalSeconds / 60
            let s = totalSeconds % 60
            return "\(m)m \(s)s"
        }
        let h = totalSeconds / 3_600
        let m = (totalSeconds % 3_600) / 60
        return "\(h)h \(m)m"
    }
}
