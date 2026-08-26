import RupuDesign
import SwiftUI

/// Classified unified-diff line, ported from the web's `DiffLineType`
/// (`crates/rupu-cp/web/src/components/transcript/DiffView.tsx:27` —
/// `'hunk' | 'add' | 'del' | 'ctx'`) with ONE deliberate addition: a
/// distinct `meta` case for the four file-header guard lines
/// (`---`/`+++`/`diff --git ...`/`index ...`) that the web lumps into
/// `ctx` (`DiffView.tsx:58-59`, both rendered `text-ink`). This port's task
/// brief calls for `meta` to render dimmer (`rupuDim`) than a genuine
/// unchanged-context line (`rupuInk`), so those four header shapes get
/// their own case rather than sharing `ctx`'s color.
public enum DiffLine: Equatable, Sendable {
    case hunk(String)
    case add(String)
    case del(String)
    case ctx(String)
    case meta(String)
}

/// Pure unified-diff line classifier — port of `parseDiff`
/// (`DiffView.tsx:42-70`), `meta` split noted on `DiffLine` above. Guard
/// order matters, exactly as upstream: the `---`/`+++`/`diff --git`/
/// `index ` guards run BEFORE the single-character `-`/`+` checks so a
/// file-header line never misclassifies as a removed/added line.
///
/// Trailing-empty-line handling mirrors the web's own `filter` (`DiffView.
/// tsx:47-50`): a single trailing empty string produced by `split("\n")`
/// on a diff ending in `"\n"` is dropped; an empty diff returns `[]`.
public func parseUnifiedDiff(_ text: String) -> [DiffLine] {
    guard !text.isEmpty else { return [] }

    var lines = text.components(separatedBy: "\n")
    if lines.last == "" {
        lines.removeLast()
    }

    return lines.map { line in
        if line.hasPrefix("@@") { return .hunk(line) }
        // File-header guards — must come BEFORE the +/- single-char checks.
        if line.hasPrefix("---") || line.hasPrefix("+++") { return .meta(line) }
        if line.hasPrefix("diff --git") || line.hasPrefix("index ") { return .meta(line) }
        if line.hasPrefix("-") { return .del(line) }
        if line.hasPrefix("+") { return .add(line) }
        return .ctx(line)
    }
}

/// Renders a parsed unified diff, one row per `DiffLine`, horizontally
/// scrollable for long lines. Tone mapping per the task brief: `add` →
/// `rupuOkBg`/`rupuOk`, `del` → `rupuErrBg`/`rupuErr`, `hunk` → `rupuMute`
/// (no tint background), `meta` → `rupuDim` (no tint background), `ctx` →
/// `rupuInk` (plain body text, matching the web's own `text-ink` — the one
/// tone the brief didn't need to call out since it's this view's default).
public struct DiffView: View {
    private let lines: [DiffLine]

    public init(diff: String) {
        self.lines = parseUnifiedDiff(diff)
    }

    public var body: some View {
        ScrollView(.horizontal, showsIndicators: true) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                    lineRow(line)
                }
            }
        }
        .background(Color.rupuPanel)
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }

    @ViewBuilder
    private func lineRow(_ line: DiffLine) -> some View {
        switch line {
        case .add(let text): row(text, fg: .rupuOk, bg: .rupuOkBg)
        case .del(let text): row(text, fg: .rupuErr, bg: .rupuErrBg)
        case .hunk(let text): row(text, fg: .rupuMute, bg: nil)
        case .meta(let text): row(text, fg: .rupuDim, bg: nil)
        case .ctx(let text): row(text, fg: .rupuInk, bg: nil)
        }
    }

    private func row(_ text: String, fg: Color, bg: Color?) -> some View {
        Text(text.isEmpty ? " " : text)
            .font(.dataMono(11.5))
            .foregroundStyle(fg)
            .textSelection(.enabled)
            .padding(.horizontal, 8)
            .padding(.vertical, 1)
            .fixedSize(horizontal: true, vertical: false)
            .frame(minWidth: 0, alignment: .leading)
            .background(bg ?? Color.clear)
    }
}
