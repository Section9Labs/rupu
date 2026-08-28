import Foundation

/// Errors produced by ``YAMLParser``. Both cases carry the 1-based source
/// line the problem was detected on.
public enum YAMLError: Error, Equatable, Sendable {
    /// A YAML feature outside the supported subset (anchors, aliases, tags,
    /// multiple documents, complex `? ` keys).
    case unsupported(String, line: Int)
    /// A structural problem within the supported subset (duplicate keys,
    /// unterminated quotes/flow collections, malformed lines).
    case malformed(String, line: Int)
}

/// A hand-rolled recursive-descent parser for the YAML subset that
/// `Workflow::parse`-able workflows in this repo use (see
/// `.rupu/workflows/*.yaml`): block mappings/sequences at any nesting,
/// one level of flow-collection nesting (`[a, b]` / `{a: 1}`, whose values
/// may themselves be flow collections or scalars), typed plain scalars,
/// single/double-quoted scalars, literal/folded block scalars with
/// chomping, and comments. Anchors, aliases, tags, multiple documents, and
/// complex (`? `) keys are rejected with ``YAMLError/unsupported(_:line:)``.
///
/// This is intentionally not a general-purpose YAML implementation — it
/// exists to parse workflow definitions the CLI's own `rupu-orchestrator`
/// crate already accepts, nothing more.
public enum YAMLParser {
    public static func parse(_ text: String) throws -> YAMLValue {
        let parser = Parser(text)
        return try parser.parseDocument()
    }
}

// MARK: - Parser

/// Owns the mutable line cursor (`pos`) shared by every structural parsing
/// function below. `peek()` is the single place that skips blank lines,
/// comment-only lines, and the (at most one, leading) `---` document-start
/// marker; everything else reads lines through it.
private final class Parser {
    struct RawLine {
        let number: Int
        let text: String
    }

    struct LineInfo {
        let indent: Int
        let content: String
        let lineNumber: Int
    }

    let rawLines: [RawLine]
    var pos = 0
    var sawContent = false

    init(_ text: String) {
        let normalized = text.replacingOccurrences(of: "\r\n", with: "\n")
        let parts = normalized.components(separatedBy: "\n")
        rawLines = parts.enumerated().map { RawLine(number: $0.offset + 1, text: $0.element) }
    }

    // MARK: Line cursor

    /// Returns the next structurally-relevant line without consuming it
    /// (repeated calls at the same `pos` return the same result), skipping
    /// blank lines, comment-only lines, and a single leading `---` marker
    /// along the way. A `---` seen after real content has already started
    /// means a second document, which this parser doesn't support.
    func peek() throws -> LineInfo? {
        while pos < rawLines.count {
            let raw = rawLines[pos]
            let indent = raw.text.prefix(while: { $0 == " " }).count
            let bodyAfterIndent = String(raw.text.dropFirst(indent))
            let stripped = stripCommentAndTrim(bodyAfterIndent)
            if stripped.isEmpty {
                pos += 1
                continue
            }
            if stripped == "---" {
                if sawContent {
                    throw YAMLError.unsupported("multiple documents are not supported", line: raw.number)
                }
                pos += 1
                continue
            }
            sawContent = true
            return LineInfo(indent: indent, content: stripped, lineNumber: raw.number)
        }
        return nil
    }

    // MARK: Document

    func parseDocument() throws -> YAMLValue {
        guard let line = try peek() else { return .null }
        let (value, next) = try parseNode(indent: line.indent)
        pos = next
        if let extra = try peek() {
            throw YAMLError.malformed("unexpected content after document", line: extra.lineNumber)
        }
        return value
    }

    // MARK: Node dispatch

    func parseNode(indent: Int) throws -> (YAMLValue, Int) {
        guard let line = try peek(), line.indent >= indent else {
            return (.null, pos)
        }
        if line.content.hasPrefix("? ") || line.content == "?" {
            throw YAMLError.unsupported("complex mapping keys are not supported", line: line.lineNumber)
        }
        if line.content.hasPrefix("- ") || line.content == "-" {
            return try parseSequence(indent: line.indent)
        }
        if findTopLevelColon(line.content) != nil {
            return try parseMapping(indent: line.indent)
        }
        let (value, next) = try parseValue(line.content, rawIndex: pos, indent: line.indent)
        pos = next
        return (value, pos)
    }

    // MARK: Block mapping

    func parseMapping(indent: Int) throws -> (YAMLValue, Int) {
        var entries: [(key: String, value: YAMLValue)] = []
        while let line = try peek(), line.indent == indent, findTopLevelColon(line.content) != nil {
            let rawIndex = pos
            let lineNumber = line.lineNumber
            let (key, value) = try parseSingleMappingEntry(
                content: line.content, rawIndex: rawIndex, lineNumber: lineNumber, indent: indent)
            if entries.contains(where: { $0.key == key }) {
                throw YAMLError.malformed("duplicate key '\(key)'", line: lineNumber)
            }
            entries.append((key, value))
        }
        return (.mapping(entries), pos)
    }

    /// Parses one `key: value` pair whose `content` is already known to
    /// start at `indent`. Requires `pos == rawIndex` on entry (the line has
    /// not been consumed yet) and leaves `pos` at the first line after this
    /// entry (which may span many raw lines, for a nested block or a block
    /// scalar) on return.
    func parseSingleMappingEntry(content: String, rawIndex: Int, lineNumber: Int, indent: Int) throws -> (
        key: String, value: YAMLValue
    ) {
        guard let colonIdx = findTopLevelColon(content) else {
            throw YAMLError.malformed("expected 'key: value'", line: lineNumber)
        }
        if content.hasPrefix("? ") || content == "?" {
            throw YAMLError.unsupported("complex mapping keys are not supported", line: lineNumber)
        }
        let keyRaw = String(content[content.startIndex..<colonIdx])
        let key = try resolveKey(keyRaw, lineNumber: lineNumber)
        let afterColon = content.index(after: colonIdx)
        let valueText = String(content[afterColon...]).trimmingCharacters(in: .whitespaces)
        if valueText.isEmpty {
            pos = rawIndex + 1
            if let next = try peek(), next.indent > indent {
                let (nested, nextIdx) = try parseNode(indent: next.indent)
                pos = nextIdx
                return (key, nested)
            }
            return (key, .null)
        }
        let (value, nextIdx) = try parseValue(valueText, rawIndex: rawIndex, indent: indent)
        pos = nextIdx
        return (key, value)
    }

    // MARK: Block sequence

    func parseSequence(indent: Int) throws -> (YAMLValue, Int) {
        var items: [YAMLValue] = []
        while let line = try peek(), line.indent == indent, line.content.hasPrefix("- ") || line.content == "-" {
            let dashRawIndex = pos
            let dashLineNumber = line.lineNumber

            // Column right after "- " (or "-") is the virtual indent for
            // any mapping keys that continue this item on later lines.
            var idx = line.content.index(after: line.content.startIndex)
            var extraSpaces = 0
            while idx < line.content.endIndex, line.content[idx] == " " {
                idx = line.content.index(after: idx)
                extraSpaces += 1
            }
            let virtualIndent = indent + 1 + extraSpaces
            let remainder = String(line.content[idx...])
            let trimmedRemainder = remainder.trimmingCharacters(in: .whitespaces)

            let itemValue: YAMLValue
            if trimmedRemainder.isEmpty {
                pos = dashRawIndex + 1
                if let next = try peek(), next.indent > indent {
                    let (nested, nextIdx) = try parseNode(indent: next.indent)
                    pos = nextIdx
                    itemValue = nested
                } else {
                    itemValue = .null
                }
            } else if findTopLevelColon(remainder) != nil {
                var entries: [(key: String, value: YAMLValue)] = []
                let (key0, value0) = try parseSingleMappingEntry(
                    content: remainder, rawIndex: dashRawIndex, lineNumber: dashLineNumber, indent: virtualIndent)
                entries.append((key0, value0))
                while let line2 = try peek(), line2.indent == virtualIndent,
                    findTopLevelColon(line2.content) != nil
                {
                    let rawIndex2 = pos
                    let lineNumber2 = line2.lineNumber
                    let (key2, value2) = try parseSingleMappingEntry(
                        content: line2.content, rawIndex: rawIndex2, lineNumber: lineNumber2, indent: virtualIndent)
                    if entries.contains(where: { $0.key == key2 }) {
                        throw YAMLError.malformed("duplicate key '\(key2)'", line: lineNumber2)
                    }
                    entries.append((key2, value2))
                }
                itemValue = .mapping(entries)
            } else {
                let (value, nextIdx) = try parseValue(remainder, rawIndex: dashRawIndex, indent: indent)
                pos = nextIdx
                itemValue = value
            }
            items.append(itemValue)
        }
        return (.sequence(items), pos)
    }

    // MARK: Shared value parsing (mapping values, sequence scalar items, top-level scalars)

    /// Parses `text` (already comment-stripped and trimmed) as a value.
    /// `rawIndex` is the raw line the value's key/dash lives on and
    /// `indent` is that line's indent — used as the parent indent for block
    /// scalar termination. Quote detection runs before flow detection, so a
    /// quoted scalar containing `{{ ... }}` or flow-looking punctuation
    /// stays a string.
    func parseValue(_ text: String, rawIndex: Int, indent: Int) throws -> (YAMLValue, Int) {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        let lineNumber = rawLines[rawIndex].number
        if trimmed.isEmpty {
            return (.null, rawIndex + 1)
        }
        if trimmed.hasPrefix("\"") {
            let (str, consumedAll) = try parseDoubleQuotedFull(trimmed, lineNumber: lineNumber)
            guard consumedAll else {
                throw YAMLError.malformed("unexpected content after quoted scalar", line: lineNumber)
            }
            return (.string(str), rawIndex + 1)
        }
        if trimmed.hasPrefix("'") {
            let (str, consumedAll) = try parseSingleQuotedFull(trimmed, lineNumber: lineNumber)
            guard consumedAll else {
                throw YAMLError.malformed("unexpected content after quoted scalar", line: lineNumber)
            }
            return (.string(str), rawIndex + 1)
        }
        if trimmed.hasPrefix("[") || trimmed.hasPrefix("{") {
            var scanner = FlowScanner(trimmed, lineNumber: lineNumber)
            let value = try scanner.parseValue()
            scanner.skipWhitespace()
            guard scanner.isAtEnd else {
                throw YAMLError.malformed("unexpected content after flow collection", line: lineNumber)
            }
            return (value, rawIndex + 1)
        }
        if trimmed.hasPrefix("|") || trimmed.hasPrefix(">") {
            return try parseBlockScalar(indicator: trimmed, startRawIndex: rawIndex, parentIndent: indent)
        }
        if trimmed.hasPrefix("&") {
            throw YAMLError.unsupported("anchors are not supported", line: lineNumber)
        }
        if trimmed.hasPrefix("*") {
            throw YAMLError.unsupported("aliases are not supported", line: lineNumber)
        }
        if trimmed.hasPrefix("!") {
            throw YAMLError.unsupported("tags are not supported", line: lineNumber)
        }
        return (resolveScalar(trimmed), rawIndex + 1)
    }

    func resolveKey(_ raw: String, lineNumber: Int) throws -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("\"") {
            let (str, consumedAll) = try parseDoubleQuotedFull(trimmed, lineNumber: lineNumber)
            guard consumedAll else {
                throw YAMLError.malformed("unexpected content in key", line: lineNumber)
            }
            return str
        }
        if trimmed.hasPrefix("'") {
            let (str, consumedAll) = try parseSingleQuotedFull(trimmed, lineNumber: lineNumber)
            guard consumedAll else {
                throw YAMLError.malformed("unexpected content in key", line: lineNumber)
            }
            return str
        }
        if trimmed.hasPrefix("&") {
            throw YAMLError.unsupported("anchors are not supported", line: lineNumber)
        }
        if trimmed.hasPrefix("*") {
            throw YAMLError.unsupported("aliases are not supported", line: lineNumber)
        }
        if trimmed.hasPrefix("!") {
            throw YAMLError.unsupported("tags are not supported", line: lineNumber)
        }
        return trimmed
    }

    // MARK: Block scalars

    private enum BlockStyle { case literal, folded }
    private enum ChompMode { case strip, clip, keep }

    /// Consumes raw lines (untouched by comment-stripping — `#` is literal
    /// content inside a block scalar) starting right after the `key: |`/
    /// `key: >` line, stopping at the first non-blank line indented at or
    /// below `parentIndent`. Returns the resulting string and the raw index
    /// to resume structural parsing from.
    func parseBlockScalar(indicator: String, startRawIndex: Int, parentIndent: Int) throws -> (YAMLValue, Int) {
        let lineNumber = rawLines[startRawIndex].number
        var chars = Substring(indicator)
        guard let first = chars.first, first == "|" || first == ">" else {
            throw YAMLError.malformed("invalid block scalar indicator", line: lineNumber)
        }
        let style: BlockStyle = (first == "|") ? .literal : .folded
        chars = chars.dropFirst()
        var chomp: ChompMode = .clip
        if let c = chars.first, c == "-" || c == "+" {
            chomp = (c == "-") ? .strip : .keep
            chars = chars.dropFirst()
        }
        guard chars.allSatisfy({ $0 == " " }) else {
            throw YAMLError.unsupported(
                "explicit block scalar indentation indicators are not supported", line: lineNumber)
        }

        var index = startRawIndex + 1
        var collected: [String] = []
        var baseIndent: Int?
        while index < rawLines.count {
            let text = rawLines[index].text
            if text.trimmingCharacters(in: .whitespaces).isEmpty {
                collected.append("")
                index += 1
                continue
            }
            let lineIndent = text.prefix(while: { $0 == " " }).count
            if baseIndent == nil {
                if lineIndent <= parentIndent {
                    break
                }
                baseIndent = lineIndent
            }
            guard let base = baseIndent, lineIndent >= base else { break }
            collected.append(String(text.dropFirst(base)))
            index += 1
        }

        var trailingBlankCount = 0
        while let last = collected.last, last.isEmpty {
            collected.removeLast()
            trailingBlankCount += 1
        }
        let core = collected

        let body: String
        switch style {
        case .literal:
            body = core.joined(separator: "\n")
        case .folded:
            body = foldLines(core)
        }

        let result: String
        switch chomp {
        case .strip:
            result = body
        case .clip:
            result = core.isEmpty ? "" : body + "\n"
        case .keep:
            result = body + String(repeating: "\n", count: trailingBlankCount)
        }
        return (.string(result), index)
    }

    /// Folds consecutive non-blank lines into a single space-joined run;
    /// each run of N blank lines between two such runs becomes N literal
    /// newlines (matching plain-YAML folded-scalar semantics closely enough
    /// for the corpus this parser targets).
    private func foldLines(_ lines: [String]) -> String {
        var parts: [String] = []
        var i = 0
        while i < lines.count {
            if lines[i].isEmpty {
                var count = 0
                while i < lines.count, lines[i].isEmpty {
                    count += 1
                    i += 1
                }
                parts.append(String(repeating: "\n", count: count))
            } else {
                var group: [String] = []
                while i < lines.count, !lines[i].isEmpty {
                    group.append(lines[i])
                    i += 1
                }
                parts.append(group.joined(separator: " "))
            }
        }
        return parts.joined()
    }
}

// MARK: - Flow collections

/// A small in-line tokenizer/parser for flow sequences (`[a, b]`) and flow
/// mappings (`{a: 1}`), recursing for nested flow collections. Operates
/// over a single already comment-stripped line (or line fragment).
private struct FlowScanner {
    private let chars: [Character]
    private var i = 0
    private let lineNumber: Int

    init(_ s: String, lineNumber: Int) {
        chars = Array(s)
        self.lineNumber = lineNumber
    }

    var isAtEnd: Bool { i >= chars.count }

    mutating func skipWhitespace() {
        while i < chars.count, chars[i] == " " || chars[i] == "\t" {
            i += 1
        }
    }

    mutating func parseValue() throws -> YAMLValue {
        skipWhitespace()
        guard i < chars.count else {
            throw YAMLError.malformed("unexpected end of flow collection", line: lineNumber)
        }
        switch chars[i] {
        case "[":
            return try parseSequence()
        case "{":
            return try parseMapping()
        case "\"":
            return .string(try parseDoubleQuoted())
        case "'":
            return .string(try parseSingleQuoted())
        default:
            return try parseScalar()
        }
    }

    private mutating func parseSequence() throws -> YAMLValue {
        i += 1
        var items: [YAMLValue] = []
        skipWhitespace()
        if i < chars.count, chars[i] == "]" {
            i += 1
            return .sequence(items)
        }
        while true {
            items.append(try parseValue())
            skipWhitespace()
            guard i < chars.count else {
                throw YAMLError.malformed("unterminated flow sequence", line: lineNumber)
            }
            if chars[i] == "," {
                i += 1
                skipWhitespace()
                if i < chars.count, chars[i] == "]" {
                    i += 1
                    break
                }
                continue
            } else if chars[i] == "]" {
                i += 1
                break
            } else {
                throw YAMLError.malformed("expected ',' or ']' in flow sequence", line: lineNumber)
            }
        }
        return .sequence(items)
    }

    private mutating func parseMapping() throws -> YAMLValue {
        i += 1
        var entries: [(key: String, value: YAMLValue)] = []
        skipWhitespace()
        if i < chars.count, chars[i] == "}" {
            i += 1
            return .mapping(entries)
        }
        while true {
            skipWhitespace()
            let key = try parseKey()
            skipWhitespace()
            guard i < chars.count, chars[i] == ":" else {
                throw YAMLError.malformed("expected ':' in flow mapping", line: lineNumber)
            }
            i += 1
            skipWhitespace()
            let value = try parseValue()
            if entries.contains(where: { $0.key == key }) {
                throw YAMLError.malformed("duplicate key '\(key)' in flow mapping", line: lineNumber)
            }
            entries.append((key, value))
            skipWhitespace()
            guard i < chars.count else {
                throw YAMLError.malformed("unterminated flow mapping", line: lineNumber)
            }
            if chars[i] == "," {
                i += 1
                skipWhitespace()
                if i < chars.count, chars[i] == "}" {
                    i += 1
                    break
                }
                continue
            } else if chars[i] == "}" {
                i += 1
                break
            } else {
                throw YAMLError.malformed("expected ',' or '}' in flow mapping", line: lineNumber)
            }
        }
        return .mapping(entries)
    }

    private mutating func parseKey() throws -> String {
        guard i < chars.count else {
            throw YAMLError.malformed("expected key in flow mapping", line: lineNumber)
        }
        if chars[i] == "\"" { return try parseDoubleQuoted() }
        if chars[i] == "'" { return try parseSingleQuoted() }
        var s = ""
        while i < chars.count, !",:}]".contains(chars[i]) {
            s.append(chars[i])
            i += 1
        }
        return s.trimmingCharacters(in: .whitespaces)
    }

    private mutating func parseScalar() throws -> YAMLValue {
        var s = ""
        while i < chars.count, !",}]".contains(chars[i]) {
            s.append(chars[i])
            i += 1
        }
        let trimmed = s.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("&") {
            throw YAMLError.unsupported("anchors are not supported", line: lineNumber)
        }
        if trimmed.hasPrefix("*") {
            throw YAMLError.unsupported("aliases are not supported", line: lineNumber)
        }
        if trimmed.hasPrefix("!") {
            throw YAMLError.unsupported("tags are not supported", line: lineNumber)
        }
        return resolveScalar(trimmed)
    }

    mutating func parseDoubleQuoted() throws -> String {
        i += 1  // consume opening quote
        var s = ""
        while i < chars.count {
            let c = chars[i]
            if c == "\\" {
                i += 1
                guard i < chars.count else {
                    throw YAMLError.malformed("unterminated escape in quoted scalar", line: lineNumber)
                }
                switch chars[i] {
                case "n": s.append("\n")
                case "t": s.append("\t")
                case "\"": s.append("\"")
                case "\\": s.append("\\")
                default: s.append(chars[i])
                }
                i += 1
                continue
            }
            if c == "\"" {
                i += 1
                return s
            }
            s.append(c)
            i += 1
        }
        throw YAMLError.malformed("unterminated quoted scalar", line: lineNumber)
    }

    mutating func parseSingleQuoted() throws -> String {
        i += 1  // consume opening quote
        var s = ""
        while i < chars.count {
            let c = chars[i]
            if c == "'" {
                if i + 1 < chars.count, chars[i + 1] == "'" {
                    s.append("'")
                    i += 2
                    continue
                }
                i += 1
                return s
            }
            s.append(c)
            i += 1
        }
        throw YAMLError.malformed("unterminated quoted scalar", line: lineNumber)
    }
}

// MARK: - Shared free helpers

/// Parses a standalone (non-flow) double-quoted scalar occupying the whole
/// (or a prefix of) `s`, returning whether it consumed every remaining
/// character — used to detect trailing garbage after the closing quote.
private func parseDoubleQuotedFull(_ s: String, lineNumber: Int) throws -> (String, Bool) {
    var scanner = FlowScanner(s, lineNumber: lineNumber)
    let value = try scanner.parseDoubleQuoted()
    scanner.skipWhitespace()
    return (value, scanner.isAtEnd)
}

private func parseSingleQuotedFull(_ s: String, lineNumber: Int) throws -> (String, Bool) {
    var scanner = FlowScanner(s, lineNumber: lineNumber)
    let value = try scanner.parseSingleQuoted()
    scanner.skipWhitespace()
    return (value, scanner.isAtEnd)
}

/// Finds the first top-level `key: value` colon separator in `s` — a `:`
/// immediately followed by a space or end-of-string, outside single/double
/// quotes. Not flow-bracket-aware: callers only use this on whole lines,
/// where the key always precedes any flow collection in the value.
private func findTopLevelColon(_ s: String) -> String.Index? {
    var inSingle = false
    var inDouble = false
    var i = s.startIndex
    while i < s.endIndex {
        let c = s[i]
        if inDouble {
            if c == "\\" {
                let next = s.index(after: i)
                i = next < s.endIndex ? s.index(after: next) : next
                continue
            }
            if c == "\"" { inDouble = false }
            i = s.index(after: i)
            continue
        }
        if inSingle {
            if c == "'" {
                let next = s.index(after: i)
                if next < s.endIndex, s[next] == "'" {
                    i = s.index(after: next)
                    continue
                }
                inSingle = false
            }
            i = s.index(after: i)
            continue
        }
        if c == "\"" {
            inDouble = true
            i = s.index(after: i)
            continue
        }
        if c == "'" {
            inSingle = true
            i = s.index(after: i)
            continue
        }
        if c == ":" {
            let next = s.index(after: i)
            if next == s.endIndex || s[next] == " " {
                return i
            }
        }
        i = s.index(after: i)
    }
    return nil
}

/// Strips a leading-indent-free line down to its content: drops a trailing
/// ` # comment` (respecting quotes — a `#` inside a quoted scalar is
/// content, not a comment start) and trims trailing whitespace. A `#` only
/// starts a comment when it is the first character on the line or is
/// preceded by whitespace, matching the brief's "trailing ` #`" wording.
private func stripCommentAndTrim(_ s: String) -> String {
    var result = ""
    var inSingle = false
    var inDouble = false
    var i = s.startIndex
    while i < s.endIndex {
        let c = s[i]
        if inDouble {
            result.append(c)
            if c == "\\" {
                let next = s.index(after: i)
                if next < s.endIndex {
                    result.append(s[next])
                    i = s.index(after: next)
                    continue
                }
            } else if c == "\"" {
                inDouble = false
            }
            i = s.index(after: i)
            continue
        }
        if inSingle {
            result.append(c)
            if c == "'" {
                let next = s.index(after: i)
                if next < s.endIndex, s[next] == "'" {
                    result.append("'")
                    i = s.index(after: next)
                    continue
                }
                inSingle = false
            }
            i = s.index(after: i)
            continue
        }
        if c == "\"" {
            inDouble = true
            result.append(c)
            i = s.index(after: i)
            continue
        }
        if c == "'" {
            inSingle = true
            result.append(c)
            i = s.index(after: i)
            continue
        }
        if c == "#" {
            if result.isEmpty || result.last == " " || result.last == "\t" {
                break
            }
            result.append(c)
            i = s.index(after: i)
            continue
        }
        result.append(c)
        i = s.index(after: i)
    }
    while let last = result.last, last == " " || last == "\t" {
        result.removeLast()
    }
    return result
}

/// Resolves an unquoted plain scalar's text to a typed `YAMLValue`, per
/// YAML 1.2 core-lite: only the lowercase `true`/`false`/`null` keywords
/// (plus bare `~`) resolve to non-string types; everything else that
/// doesn't parse as an int or float stays a string.
private func resolveScalar(_ text: String) -> YAMLValue {
    if text.isEmpty || text == "~" || text == "null" {
        return .null
    }
    if text == "true" { return .bool(true) }
    if text == "false" { return .bool(false) }
    if let i = Int(text) {
        return .int(i)
    }
    if looksNumeric(text), let d = Double(text) {
        return .double(d)
    }
    return .string(text)
}

/// A conservative check that `text` looks like a decimal integer or float
/// literal, run before handing it to `Double.init` — `Double` alone accepts
/// forms (hex floats, "inf", "nan", ...) that would otherwise mis-type
/// ordinary prose as a number.
private func looksNumeric(_ text: String) -> Bool {
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
