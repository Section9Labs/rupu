import Foundation

/// Null-discipline formatters shared across the macOS app's read surfaces.
public enum Fmt {
    /// Shared `NumberFormatter` for `count`/`partial`. `Fmt.count`/`Fmt.cost` used to allocate and
    /// configure a fresh `NumberFormatter` on every call — a hot path across 174+ table/pill call
    /// sites. Foundation's `NumberFormatter` is `Sendable` (`@unchecked`, since it's a mutable
    /// class — but safe here because this instance's configuration is set once below and never
    /// mutated again, and `NumberFormatter.string(from:)` doesn't mutate that configuration; every
    /// call site is also a SwiftUI view body, i.e. `@MainActor`-confined, on top of that).
    private static let countFormatter: NumberFormatter = {
        let f = NumberFormatter()
        f.numberStyle = .decimal
        f.usesGroupingSeparator = true
        f.groupingSeparator = ","
        f.decimalSeparator = "."
        return f
    }()

    /// Renders `nil` as an em dash and otherwise as a grouped integer (e.g. `1,234`).
    public static func count(_ n: Int?) -> String {
        guard let n else { return "—" }
        return countFormatter.string(from: NSNumber(value: n)) ?? String(n)
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

    /// Shared `en_US_POSIX`-pinned `NumberFormatter` for `cost`. Same rationale as
    /// `countFormatter` above for why a shared instance is safe: configuration (including the
    /// locale, which is the whole reason `cost` doesn't render `,` decimals under EU locales) is
    /// set once below and never mutated again.
    private static let costFormatter: NumberFormatter = {
        let f = NumberFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.numberStyle = .decimal
        f.minimumFractionDigits = 2
        f.maximumFractionDigits = 2
        f.decimalSeparator = "."
        f.usesGroupingSeparator = false
        return f
    }()

    /// Renders `nil` as an em dash and otherwise a fixed-point USD amount
    /// (`"$0.12"`, `"$12.50"`). Locale-independent by construction: a plain
    /// `String(format: "$%.2f", ...)` (or a `NumberFormatter` left on the
    /// user's current locale) renders the decimal separator as `,` under
    /// most EU locales — `"$0,12"` — which is wrong for a USD literal
    /// regardless of the reader's region. `NumberFormatter` pinned to
    /// `en_US_POSIX` (Apple's documented recipe for locale-invariant fixed
    /// formats — see the "POSIX" note in `Locale` docs) sidesteps that
    /// rather than depending on `%f`'s own locale sensitivity.
    public static func cost(_ usd: Double?) -> String {
        guard let usd else { return "—" }
        let digits = costFormatter.string(from: NSNumber(value: usd)) ?? String(format: "%.2f", usd)
        return "$\(digits)"
    }

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
