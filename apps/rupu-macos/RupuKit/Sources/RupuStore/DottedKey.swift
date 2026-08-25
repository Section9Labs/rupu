import Foundation

/// Canonical dotted-key encode/decode — the Swift port of the shared
/// contract implemented three other times: `rupu_config::resolve`'s private
/// `dotted()` (`crates/rupu-config/src/resolve.rs`, ~line 106), rupu-cp's
/// WRITE-path `config_write::split_dotted_key`, and the web CP's READ-path
/// `ConfigEditor.tsx`'s `quoteSegment`/`splitDottedKey`
/// (`crates/rupu-cp/web/src/components/ConfigEditor.tsx:46-116`). All must
/// stay in lockstep — the same dotted-key string flows from the resolved
/// provenance map, through this app's config-field plumbing (and, for policy
/// lock entries, back out through `PUT /api/config/policy`), unchanged.
///
/// A config key SEGMENT can itself contain a `.` — the pricing model table
/// for a model id like `/raid/models/zai-org/GLM-5.2-FP8` produces a dotted
/// key `pricing.oracle."/raid/models/zai-org/GLM-5.2-FP8".input_per_mtok`. A
/// plain `key.split(separator: ".")` would tear that segment in two.
///
/// This module ships only the lenient READ-side decoder (`split`) plus the
/// canonical encoder (`quoteSegment`/`join`). The app DOES write config
/// (`CPClient.putConfigGlobal`/`putConfigProject`/`putConfigPolicy`), but its
/// write bodies are whole-file raw TOML or an already-encoded `lock` string
/// array — nothing here ever needs to DECODE an arbitrary dotted key into
/// validated segments, so there is no Swift counterpart to the Rust/TS
/// WRITE-side `split_dotted_key` that rejects malformed input outright.
public enum DottedKey {
    /// Quote a single key segment: a segment containing a `.` or a `"` is
    /// wrapped in double quotes with embedded `"` escaped as `\"`
    /// (TOML-quoted-key style); anything else is emitted bare. Matches the
    /// web `quoteSegment` and Rust `dotted()`'s per-segment mapping
    /// byte-for-byte.
    public static func quoteSegment(_ s: String) -> String {
        if s.contains(".") || s.contains("\"") {
            return "\"\(s.replacingOccurrences(of: "\"", with: "\\\""))\""
        }
        return s
    }

    /// `join = segments.map(quoteSegment).joined(separator: ".")` — the
    /// canonical encoder, matching Rust `dotted()` and the web's implicit
    /// `segments.map(quoteSegment).join('.')` join step.
    public static func join(_ segments: [String]) -> String {
        segments.map(quoteSegment).joined(separator: ".")
    }

    /// Lenient quote-aware split — the READ-side decoder (used to render a
    /// resolved field's current value against its dotted key), the inverse
    /// of `join`. Splits on `.` except inside a `"…"`-quoted segment (where
    /// `\"` unescapes to `"`). Malformed quoting (an unterminated quote, or
    /// a `"` that doesn't span the whole segment) falls back to a naive
    /// `split(separator:)` rather than throwing — this only ever renders a
    /// UI field, so a bad key string should degrade to "field doesn't
    /// resolve", never crash the app.
    ///
    /// **Strict-write, lenient-read asymmetry (deliberate):** this mirrors
    /// the web's `splitDottedKey`, which stays lenient — including on a
    /// string with an empty segment (a bare `.`, or a leading/trailing/
    /// doubled separator: `"a."` → `["a", ""]`, `"."` → `["", ""]`) rather
    /// than erroring, since a bogus key here just means a field harmlessly
    /// fails to resolve. rupu-cp's WRITE-path `config_write::split_dotted_key`
    /// is the opposite: it REJECTS any empty segment outright, because
    /// accepting one there would let an edit persist a config key that can
    /// never correspond to a real TOML field. Both are intentional — see
    /// that Rust function's doc comment.
    public static func split(_ key: String) -> [String] {
        var segments: [String] = []
        let chars = Array(key)
        let n = chars.count
        var i = 0
        while i <= n {
            if i < n, chars[i] == "\"" {
                var seg = ""
                var j = i + 1
                var closed = false
                while j < n {
                    let c = chars[j]
                    if c == "\"" {
                        closed = true
                        j += 1
                        break
                    }
                    if c == "\\", j + 1 < n, chars[j + 1] == "\"" {
                        seg.append("\"")
                        j += 2
                        continue
                    }
                    seg.append(c)
                    j += 1
                }
                if !closed || (j < n && chars[j] != ".") {
                    return naiveSplit(key) // malformed — best-effort fallback
                }
                segments.append(seg)
                i = j + 1 // skip the '.' (or reach n+1 if this was the last segment)
            } else {
                if let dotOffset = chars[i...].firstIndex(of: ".") {
                    let seg = String(chars[i..<dotOffset])
                    if seg.contains("\"") {
                        return naiveSplit(key) // malformed fallback
                    }
                    segments.append(seg)
                    i = dotOffset + 1
                } else {
                    segments.append(String(chars[i...]))
                    i = n + 1
                }
            }
        }
        return segments
    }

    /// `key.split('.')` semantics matching JavaScript's `String.split`: every
    /// `.` is a separator, adjacent/leading/trailing separators produce
    /// empty-string segments, and an empty input produces `[""]`.
    private static func naiveSplit(_ key: String) -> [String] {
        key.components(separatedBy: ".")
    }
}
