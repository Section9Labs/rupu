/// Canonical block-style YAML emitter for `YAMLValue`, the counterpart to
/// ``YAMLParser``. Produces 2-space-indented block YAML: mapping keys in
/// stored order, sequences as `- ` items with nested content indented under
/// the dash js-yaml-style (`- id: x` inline first key, later keys aligned
/// under it), multiline strings as literal block scalars, and scalars
/// double-quoted only when a plain emission would not round-trip back to the
/// same `YAMLValue` through ``YAMLParser``.
///
/// The single correctness invariant this file is built around: a plain
/// (unquoted, single-line) scalar emission must re-parse via
/// `YAMLParser.parse("k: \(scalar)\n")` to `.string(scalar)` (for strings) or
/// the original typed value (for bool/int/double/null). `needsQuoting` and
/// `looksNumericLike` are deliberately kept in lockstep with
/// `YAMLParser.swift`'s `resolveScalar`/`looksNumeric` for exactly this
/// reason — see the doc comments on each for the specific parser behavior
/// they're mirroring.
public enum YAMLEmitter {
    public static func dump(_ value: YAMLValue) -> String {
        emitTopLevel(value).joined(separator: "\n") + "\n"
    }

    // MARK: - Top level

    private static func emitTopLevel(_ value: YAMLValue) -> [String] {
        switch value {
        case .mapping(let entries):
            if entries.isEmpty { return ["{}"] }
            var lines: [String] = []
            for (key, entryValue) in entries {
                emitMappingEntry(key: key, value: entryValue, indent: 0, into: &lines)
            }
            return lines
        case .sequence(let items):
            if items.isEmpty { return ["[]"] }
            var lines: [String] = []
            for item in items {
                emitSequenceItem(item, indent: 0, into: &lines)
            }
            return lines
        default:
            switch scalarEmission(for: value) {
            case .inline(let text):
                return [text]
            case .block(let header, let contentLines):
                var lines = [header]
                appendBlockContent(contentLines, contentIndent: 2, into: &lines)
                return lines
            }
        }
    }

    // MARK: - Mapping entries

    /// Emits `key: value` at `indent` (the column the key starts at),
    /// recursing for nested mappings/sequences at `indent + 2`. Used both
    /// for ordinary mapping entries and (via `emitSequenceItem`'s splicing)
    /// as the source of a sequence item's inline-first-key line.
    private static func emitMappingEntry(key: String, value: YAMLValue, indent: Int, into lines: inout [String]) {
        let pad = String(repeating: " ", count: indent)
        let keyText = emitKey(key)
        switch value {
        case .mapping(let entries):
            if entries.isEmpty {
                lines.append("\(pad)\(keyText): {}")
            } else {
                lines.append("\(pad)\(keyText):")
                for (childKey, childValue) in entries {
                    emitMappingEntry(key: childKey, value: childValue, indent: indent + 2, into: &lines)
                }
            }
        case .sequence(let items):
            if items.isEmpty {
                lines.append("\(pad)\(keyText): []")
            } else {
                lines.append("\(pad)\(keyText):")
                for item in items {
                    emitSequenceItem(item, indent: indent + 2, into: &lines)
                }
            }
        default:
            switch scalarEmission(for: value) {
            case .inline(let text):
                lines.append("\(pad)\(keyText): \(text)")
            case .block(let header, let contentLines):
                lines.append("\(pad)\(keyText): \(header)")
                appendBlockContent(contentLines, contentIndent: indent + 2, into: &lines)
            }
        }
    }

    // MARK: - Sequence items

    /// Emits one `- ` item at `indent` (the column the dash starts at). A
    /// mapping item writes its first key inline after `- ` (js-yaml style)
    /// by delegating to `emitMappingEntry` at `indent + 2` and splicing the
    /// dash onto the first produced line — safe because `"- "` is exactly 2
    /// characters, matching the `indent + 2` the nested-key lines already
    /// use, so the leading `indent + 2`-space pad of the first line is
    /// exactly replaceable by `pad + "- "`.
    private static func emitSequenceItem(_ item: YAMLValue, indent: Int, into lines: inout [String]) {
        let pad = String(repeating: " ", count: indent)
        switch item {
        case .mapping(let entries):
            if entries.isEmpty {
                lines.append("\(pad)- {}")
                return
            }
            let firstIndent = indent + 2
            var firstEntryLines: [String] = []
            emitMappingEntry(
                key: entries[0].key, value: entries[0].value, indent: firstIndent, into: &firstEntryLines)
            let firstLine = firstEntryLines[0]
            let splicedFirstLine = "\(pad)- \(firstLine.dropFirst(firstIndent))"
            lines.append(splicedFirstLine)
            lines.append(contentsOf: firstEntryLines.dropFirst())
            for (childKey, childValue) in entries.dropFirst() {
                emitMappingEntry(key: childKey, value: childValue, indent: firstIndent, into: &lines)
            }
        case .sequence(let items):
            if items.isEmpty {
                lines.append("\(pad)- []")
                return
            }
            lines.append("\(pad)-")
            for subItem in items {
                emitSequenceItem(subItem, indent: indent + 2, into: &lines)
            }
        default:
            switch scalarEmission(for: item) {
            case .inline(let text):
                lines.append("\(pad)- \(text)")
            case .block(let header, let contentLines):
                lines.append("\(pad)- \(header)")
                appendBlockContent(contentLines, contentIndent: indent + 2, into: &lines)
            }
        }
    }

    private static func appendBlockContent(_ contentLines: [String], contentIndent: Int, into lines: inout [String]) {
        let contentPad = String(repeating: " ", count: contentIndent)
        for line in contentLines {
            lines.append(line.isEmpty ? "" : contentPad + line)
        }
    }

    // MARK: - Scalar emission

    private enum ValueEmission {
        /// Appended directly after `key: ` / `- `.
        case inline(String)
        /// `header` (`"|"` or `"|-"`) goes after `key: ` / `- `; `lines` are
        /// the literal block's content lines, each indented 2 further than
        /// the key/dash by the caller.
        case block(header: String, lines: [String])
    }

    private static func scalarEmission(for value: YAMLValue) -> ValueEmission {
        switch value {
        case .null:
            return .inline("null")
        case .bool(let b):
            return .inline(b ? "true" : "false")
        case .int(let i):
            return .inline(String(i))
        case .double(let d):
            return .inline(formatDouble(d))
        case .string(let s):
            if let (header, lines) = blockScalarLines(for: s) {
                return .block(header: header, lines: lines)
            }
            return .inline(emitScalarString(s))
        case .mapping, .sequence:
            preconditionFailure("scalarEmission called on a non-scalar YAMLValue")
        }
    }

    private static func formatDouble(_ d: Double) -> String {
        let text = "\(d)"
        if text.contains(".") || text.contains("e") || text.contains("E") {
            return text
        }
        return text + ".0"
    }

    private static func emitScalarString(_ s: String) -> String {
        needsQuoting(s) ? doubleQuote(s) : s
    }

    private static func emitKey(_ key: String) -> String {
        needsQuoting(key) ? doubleQuote(key) : key
    }

    // MARK: - Multiline strings → literal block scalars

    /// Decides whether `s` can be represented as a literal block scalar
    /// (`|` for clip mode — exactly one trailing `\n` — or `|-` for strip
    /// mode — no trailing `\n`) that `YAMLParser.parseBlockScalar` will
    /// decode back to exactly `s`. Deliberately never produces `|+` (keep
    /// mode) per the emitter contract — a string needing that (2+ trailing
    /// newlines) instead falls through to a quoted single-line scalar with
    /// `\n` escapes, same as any other shape this can't represent (e.g. a
    /// line that is non-empty but all-whitespace, which the parser's
    /// blank-line detection would collapse to `""`, losing it).
    ///
    /// Rather than hand-deriving which shapes are safe, this simulates
    /// `YAMLParser.parseBlockScalar`'s own trailing-blank-stripping and
    /// chomp logic against the candidate `lines` and only accepts the
    /// candidate if reconstructing from it reproduces `s` exactly — the
    /// same technique the golden-corpus round-trip test exercises, just
    /// run ahead of time per-string so single-string failures fall back to
    /// quoting instead of breaking the whole document.
    private static func blockScalarLines(for s: String) -> (header: String, lines: [String])? {
        guard s.contains("\n") else { return nil }

        let endsWithNewline = s.hasSuffix("\n")
        let header = endsWithNewline ? "|" : "|-"
        let content = endsWithNewline ? String(s.dropLast()) : s
        let lines = content.components(separatedBy: "\n")

        // A line that is present but entirely whitespace would be
        // misclassified as a blank separator line by the parser's
        // `text.trimmingCharacters(in: .whitespaces).isEmpty` check,
        // silently losing its whitespace content on re-parse.
        for line in lines where !line.isEmpty && line.allSatisfy({ $0 == " " || $0 == "\t" }) {
            return nil
        }

        var collected = lines
        while let last = collected.last, last.isEmpty {
            collected.removeLast()
        }
        let body = collected.joined(separator: "\n")
        let reconstructed = endsWithNewline ? (collected.isEmpty ? "" : body + "\n") : body
        guard reconstructed == s else { return nil }

        return (header, lines)
    }

    // MARK: - Quoting

    /// YAML-significant characters that must not lead a plain (unquoted)
    /// scalar — mirrors the brief's baseline list. Prefix-only (matching
    /// `YAMLParser`'s own prefix-based dispatch in `parseValue`/`parseNode`
    /// rather than a "contains" check), except where noted below.
    private static let leadingSignificantChars: Set<Character> = [
        "-", "?", ":", "#", "&", "*", "!", "|", ">", "'", "\"", "%", "@", "`", "[", "]", "{", "}", ",",
    ]

    /// True when a plain (unquoted) emission of `s` would not re-parse back
    /// to `.string(s)` via `YAMLParser` — either because it would be
    /// misdetected as a different node/type, or because trimming/comment-
    /// stripping along the way would silently alter it. Applied uniformly
    /// to mapping keys, mapping scalar values, and sequence scalar items:
    /// keys never actually get type-resolved by `YAMLParser.resolveKey`, so
    /// this is a conservative superset of what a key strictly needs, but
    /// over-quoting is always safe.
    private static func needsQuoting(_ s: String) -> Bool {
        if s.isEmpty { return true }
        if s.hasPrefix(" ") || s.hasSuffix(" ") || s.hasPrefix("\t") || s.hasSuffix("\t") { return true }
        // A plain scalar can never carry a literal newline — this is a
        // safety net for the quoted-with-\n-escapes fallback path for
        // multiline strings `blockScalarLines` declined to handle.
        if s.contains("\n") { return true }
        if let first = s.first, leadingSignificantChars.contains(first) { return true }
        // `key: value` splitting only ever finds the *first* top-level
        // `": "` on a line, so this only matters where a scalar is the
        // *entire* value text being re-scanned for a colon — a bare
        // sequence item ("- a: b" is read as an inline mapping opener) —
        // but is applied everywhere for uniformity and to stay conservative.
        if s.contains(": ") { return true }
        // `stripCommentAndTrim` starts a comment at any `#` preceded by
        // whitespace (or at the very start, already covered above).
        if s.contains(" #") { return true }
        // `resolveScalar` keyword collisions: only these exact lowercase
        // forms (plus bare `~`, covered by the leading-char set above)
        // resolve to non-string types.
        switch s {
        case "~", "null", "true", "false":
            return true
        default:
            break
        }
        if Int(s) != nil { return true }
        if looksNumericLike(s), Double(s) != nil { return true }
        return false
    }

    /// A copy of `YAMLParser.swift`'s private `looksNumeric` — kept
    /// duplicated (rather than exposed across the file-private boundary)
    /// because this function's entire purpose is mirroring that one
    /// exactly, character for character, so a plain-emitted scalar's
    /// float-vs-string fate matches `resolveScalar`'s decision on re-parse.
    private static func looksNumericLike(_ text: String) -> Bool {
        let chars = Array(text)
        guard !chars.isEmpty else { return false }
        var i = 0
        if chars[i] == "+" || chars[i] == "-" {
            i += 1
        }
        var hasIntDigits = false
        while i < chars.count, chars[i].isASCII, chars[i].isNumber {
            i += 1
            hasIntDigits = true
        }
        var hasDot = false
        if i < chars.count, chars[i] == "." {
            hasDot = true
            i += 1
            var hasFracDigits = false
            while i < chars.count, chars[i].isASCII, chars[i].isNumber {
                i += 1
                hasFracDigits = true
            }
            if !hasIntDigits, !hasFracDigits {
                return false
            }
        }
        if !hasIntDigits, !hasDot {
            return false
        }
        if i < chars.count, chars[i] == "e" || chars[i] == "E" {
            i += 1
            if i < chars.count, chars[i] == "+" || chars[i] == "-" {
                i += 1
            }
            var hasExpDigits = false
            while i < chars.count, chars[i].isASCII, chars[i].isNumber {
                i += 1
                hasExpDigits = true
            }
            if !hasExpDigits {
                return false
            }
        }
        return i == chars.count
    }

    /// Escapes exactly the sequences `YAMLParser`'s double-quoted-scalar
    /// reader (`FlowScanner.parseDoubleQuoted`) decodes: `\\`, `\"`, `\n`,
    /// `\t`. Any other character is copied through literally — the parser
    /// does the same for an unrecognized `\`-escape (it drops the backslash
    /// and keeps the following character), so anything else must never be
    /// backslash-prefixed here.
    private static func doubleQuote(_ s: String) -> String {
        var out = "\""
        for c in s {
            switch c {
            case "\\": out += "\\\\"
            case "\"": out += "\\\""
            case "\n": out += "\\n"
            case "\t": out += "\\t"
            default: out.append(c)
            }
        }
        out += "\""
        return out
    }
}
