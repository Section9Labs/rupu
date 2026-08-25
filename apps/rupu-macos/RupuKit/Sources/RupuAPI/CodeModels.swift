import Foundation

/// One line of source text (`SourceLine` on the Rust side, shared by
/// `crates/rupu-cp/src/api/code.rs` and `.../source.rs`). Plain field names
/// on the wire (`n`, `text`) — no `rename_all` on the Rust struct.
public struct APISourceLine: Decodable, Equatable, Sendable {
    public let n: Int
    public let text: String

    public init(n: Int, text: String) {
        self.n = n
        self.text = text
    }
}

/// One entry in a `GET /api/projects/:ws_id/tree` listing (`TreeEntry` on
/// the Rust side). `kind` is `"dir"` or `"file"`.
public struct APITreeEntry: Decodable, Equatable, Sendable {
    public let name: String
    public let path: String
    public let kind: String

    public init(name: String, path: String, kind: String) {
        self.name = name
        self.path = path
        self.kind = kind
    }
}

/// `GET /api/projects/:ws_id/tree?path=` response (`TreeResult` on the Rust
/// side) — the immediate children of one workspace-relative directory.
/// `parent` is `nil` only at the workspace root; every subdirectory (even
/// one directly under the root) carries a parent, `""` for the root itself
/// — see `code_tree.json`'s fixture (`parent: ""`), which decodes to
/// `Optional("")`, not `nil`.
public struct APITreeResult: Decodable, Equatable, Sendable {
    public let path: String
    public let parent: String?
    public let entries: [APITreeEntry]

    public init(path: String, parent: String?, entries: [APITreeEntry]) {
        self.path = path
        self.parent = parent
        self.entries = entries
    }
}

/// `GET /api/projects/:ws_id/source?path=` response (`FileContent` on the
/// Rust side) — a whole workspace file, or a soft `available: false` with a
/// `reason` (missing/oversized/binary — never an HTTP error). The Rust
/// struct carries `#[serde(rename_all = "camelCase")]`, so `totalLines`
/// (NOT `total_lines`) is the wire key — verified against the
/// `code_file.json` fixture.
public struct APIFileContent: Decodable, Equatable, Sendable {
    public let available: Bool
    public let path: String?
    public let language: String?
    public let totalLines: Int?
    public let lines: [APISourceLine]?
    public let reason: String?

    public init(
        available: Bool,
        path: String?,
        language: String?,
        totalLines: Int?,
        lines: [APISourceLine]?,
        reason: String?
    ) {
        self.available = available
        self.path = path
        self.language = language
        self.totalLines = totalLines
        self.lines = lines
        self.reason = reason
    }

    private enum CodingKeys: String, CodingKey {
        case available
        case path
        case language
        case totalLines
        case lines
        case reason
    }
}

/// `GET /api/projects/:ws_id/files` response (`FileListResult` on the Rust
/// side) — every workspace-relative file path, capped at 20,000 with
/// `truncated: true` past the cap. Plain field names on the wire, no
/// `rename_all`.
public struct APIFileList: Decodable, Equatable, Sendable {
    public let files: [String]
    public let truncated: Bool

    public init(files: [String], truncated: Bool) {
        self.files = files
        self.truncated = truncated
    }
}
