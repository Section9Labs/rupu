import Foundation

/// `GET /api/runs/:id/source` response (`SourceSlice` on the Rust side,
/// `crates/rupu-cp/src/api/source.rs`) — a windowed slice of one file's
/// lines centered on a target line, or a soft `available: false` with a
/// `reason` (remote-host run, missing/oversized/binary file — never an HTTP
/// error). The Rust struct carries `#[serde(rename_all = "camelCase")]`, so
/// every multi-word key (`startLine`, `endLine`, `targetLine`, `totalLines`)
/// is camelCase on the wire — verified against the `run_source.json`
/// fixture, whose second element is the unavailable case with
/// `reason == "Source preview is not available for remote-host runs yet."`
/// (the `REMOTE_NOT_SUPPORTED` constant's Rust NAME, not its string value).
public struct APISourceSlice: Decodable, Equatable, Sendable {
    public let available: Bool
    public let path: String?
    public let language: String?
    public let startLine: Int?
    public let endLine: Int?
    public let targetLine: Int?
    public let totalLines: Int?
    public let lines: [APISourceLine]?
    public let reason: String?

    public init(
        available: Bool,
        path: String?,
        language: String?,
        startLine: Int?,
        endLine: Int?,
        targetLine: Int?,
        totalLines: Int?,
        lines: [APISourceLine]?,
        reason: String?
    ) {
        self.available = available
        self.path = path
        self.language = language
        self.startLine = startLine
        self.endLine = endLine
        self.targetLine = targetLine
        self.totalLines = totalLines
        self.lines = lines
        self.reason = reason
    }

    private enum CodingKeys: String, CodingKey {
        case available
        case path
        case language
        case startLine
        case endLine
        case targetLine
        case totalLines
        case lines
        case reason
    }
}

/// One tree-sitter CST node (`AstNode` on the Rust side, `crates/rupu-ast/
/// src/lib.rs`) — recursive via `children`. `#[serde(rename_all =
/// "camelCase")]` on the Rust struct, so `startLine`/`startCol`/`endLine`/
/// `endCol` are camelCase; `field` is the tree-sitter field name this node
/// occupies under its parent (e.g. `"name"`), absent for unnamed/anonymous
/// slots. `matched` marks the deepest node containing the caller's
/// requested (line, col) — exactly one node in the subtree is `true`
/// (verified against `run_ast.json`'s deepest `identifier` node). `children`
/// is a plain (possibly empty) array, never optional, on the Rust side.
public struct APIAstNode: Decodable, Equatable, Sendable {
    public let kind: String
    public let named: Bool
    public let field: String?
    public let startLine: Int
    public let startCol: Int
    public let endLine: Int
    public let endCol: Int
    public let matched: Bool
    public let children: [APIAstNode]

    public init(
        kind: String,
        named: Bool,
        field: String?,
        startLine: Int,
        startCol: Int,
        endLine: Int,
        endCol: Int,
        matched: Bool,
        children: [APIAstNode]
    ) {
        self.kind = kind
        self.named = named
        self.field = field
        self.startLine = startLine
        self.startCol = startCol
        self.endLine = endLine
        self.endCol = endCol
        self.matched = matched
        self.children = children
    }

    private enum CodingKeys: String, CodingKey {
        case kind
        case named
        case field
        case startLine
        case startCol
        case endLine
        case endCol
        case matched
        case children
    }
}

/// `GET /api/runs/:id/ast` response (`AstResponse` on the Rust side) — a
/// bounded subtree of a file's CST around a target `(line, col)`, or a soft
/// `available: false` with a `reason` (remote-host run, no grammar for the
/// file type, parse failure — never an HTTP error). Same `camelCase`
/// `rename_all` as `APISourceSlice`.
public struct APIAstResponse: Decodable, Equatable, Sendable {
    public let available: Bool
    public let language: String?
    public let root: APIAstNode?
    public let truncated: Bool?
    public let reason: String?

    public init(
        available: Bool,
        language: String?,
        root: APIAstNode?,
        truncated: Bool?,
        reason: String?
    ) {
        self.available = available
        self.language = language
        self.root = root
        self.truncated = truncated
        self.reason = reason
    }

    private enum CodingKeys: String, CodingKey {
        case available
        case language
        case root
        case truncated
        case reason
    }
}
