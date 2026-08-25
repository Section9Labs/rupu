import Foundation

// MARK: - Config view (read)

/// Mirrors `rupu_config::resolve::KeySource` (`crates/rupu-config/src/
/// resolve.rs`), which is `#[serde(rename_all = "lowercase")]` — the wire
/// values are exactly `"global"`/`"project"`/`"default"`.
public enum APIKeySource: String, Decodable, Sendable {
    case global
    case project
    case `default`
}

/// One entry in `APIConfigView.provenance`. Mirrors `rupu_config::resolve::
/// KeyProvenance`.
public struct APIKeyProvenance: Decodable, Equatable, Sendable {
    public let source: APIKeySource
    public let locked: Bool
}

/// Mirrors `RuntimeStatus` in `crates/rupu-cp/src/api/config.rs`. `bind` and
/// `tokenSet` reflect the CP host's OWN running state (not a config value —
/// `Config` has no bind/token field, see that file's module doc comment on
/// why secrets are never echoed); `restartRequiredKeys` names the config
/// keys that only take effect after a `cp serve` restart (currently always
/// `["bind", "token"]` — the handler hard-codes it, not derived from the
/// candidate edit).
public struct APIRuntimeStatus: Decodable, Equatable, Sendable {
    public let bind: String
    public let tokenSet: Bool
    public let restartRequiredKeys: [String]

    private enum CodingKeys: String, CodingKey {
        case bind
        case tokenSet = "token_set"
        case restartRequiredKeys = "restart_required_keys"
    }
}

/// `GET /api/config[?project=<ws_id>]` response. Mirrors `ConfigView` in
/// `crates/rupu-cp/src/api/config.rs`.
///
/// The wire's `cp` field is deliberately NOT decoded here: the handler sets
/// it to `serde_json::to_value(&resolved.config.cp)`, and `effective` is
/// `serde_json::to_value(&resolved.config)` — since `Config` embeds `cp` as
/// one of its own fields, that same value is already reachable at
/// `effective.cp` (see the `config_view.json` fixture, whose top-level `cp`
/// and `effective.cp` are identical objects). A second Swift property here
/// would just duplicate `effective`'s own key.
public struct APIConfigView: Decodable, Sendable {
    /// Untyped `serde_json::to_value(&resolved.config)` — every config key
    /// the server currently knows plus any it adds later. `JSONValue` (see
    /// `TranscriptModels.swift`) is RupuAPI's one existing untyped-JSON
    /// representation; reused here rather than adding a second one.
    public let effective: JSONValue
    public let provenance: [String: APIKeyProvenance]
    public let rawGlobal: String
    public let rawProject: String?
    public let status: APIRuntimeStatus

    private enum CodingKeys: String, CodingKey {
        case effective
        case provenance
        case rawGlobal = "raw_global"
        case rawProject = "raw_project"
        case status
    }
}

// MARK: - Config write (write) — request bodies

/// `PUT /api/config/global` and `PUT /api/config/project/:id` body. Mirrors
/// `ConfigWriteBody` in `crates/rupu-cp/src/api/config.rs`, which also
/// accepts an alternative `patch` (flat form-patch, merged onto the existing
/// TOML) field — deliberately unused here. The macOS write surface only
/// ever sends raw TOML text (typed/form editing is deferred to a later
/// task), so `patch` is never encoded and always resolves to `None`
/// server-side.
struct ConfigRawWriteBody: Encodable {
    let raw: String
}

/// `PUT /api/config/policy` body. Mirrors `PolicyBody` in `crates/rupu-cp/
/// src/api/config.rs`.
struct ConfigPolicyWriteBody: Encodable {
    let lock: [String]
}
