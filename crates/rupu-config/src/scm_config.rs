//! SCM and issue-tracker configuration. Spec §7c.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScmSection {
    pub default: Option<ScmDefault>,
    /// Per-platform overrides: `[scm.github]`, `[scm.gitlab]`.
    /// Keyed by lower-case platform name.
    #[serde(flatten, with = "platforms_serde")]
    pub platforms: BTreeMap<String, ScmPlatformConfig>,
    /// Ordered account-selection rules. First match wins within a tier;
    /// see `rupu_scm::rules::resolve_account` for the precedence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<ScmRule>,
}

/// One account-selection rule. Exactly one of `owner` / `path` is set;
/// a rule with both, or neither, is a config error (validated in
/// `Config::validate`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmRule {
    /// Owner glob, e.g. `acme/*` or `acme`. Matched against
    /// `RepoRef.owner`. This is the form that works for daemons, which
    /// know the owner but have no cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Filesystem path glob, e.g. `~/Code/work/*`. Matched against the
    /// caller's cwd. Covers the interactive case before a remote is
    /// known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The account this rule selects.
    pub account: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssuesSection {
    pub default: Option<IssuesDefault>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmDefault {
    /// Consumed by [`rupu_scm::Registry::default_platform`] (ISSUES.md
    /// I-15).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// **Deprecated, inert, and never read** (ISSUES.md I-73). Every real
    /// consumer that would need a single default repo — `rupu repos
    /// attach|prefer|forget` (which permanently associate a repo with a
    /// workspace path) and every SCM MCP tool's `owner`/`repo` args —
    /// requires them explicit for a reason: silently defaulting *which*
    /// repo a command targets is the exact "ambiguous command silently
    /// targets the wrong repo" failure mode a default must not create.
    /// `[issues.default].project` (paired with `.tracker`) is the safe
    /// analog: it feeds `rupu issues list --repo`'s existing
    /// cwd-autodetect fallback tier, a command that already tolerates an
    /// implicit target.
    ///
    /// `skip_serializing` keeps it out of `/api/config` and out of
    /// anything that round-trips this struct back to TOML, so the key
    /// disappears the first time a config is rewritten. The warning in
    /// [`crate::Config::warn_deprecated_keys`] tells the user to delete
    /// it. Kept as `Option<String>` rather than the `Option<toml::Value>`
    /// shim used for `[retry]`/`[cp]`'s deprecated keys: `ScmDefault` is
    /// not `#[serde(deny_unknown_fields)]` (unlike `Config`/`CpConfig`),
    /// so there is no parse-failure risk this indirection exists to
    /// avoid — an unknown-shaped value here already fails loudly with a
    /// normal serde type error instead of silently swallowing the rest of
    /// the user's config, which is strictly better for a field that's
    /// still typed as a plain string.
    #[serde(default, skip_serializing)]
    pub owner: Option<String>,
    /// **Deprecated, inert, and never read.** See `owner`'s doc — same
    /// reasoning and shim shape.
    #[serde(default, skip_serializing)]
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuesDefault {
    /// Consumed by [`rupu_scm::Registry::default_tracker`] (ISSUES.md
    /// I-15).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<String>,
    /// `<owner>/<repo>` for the configured `tracker`. Consumed by
    /// `rupu-cli`'s `configured_default_repo` (ISSUES.md I-73) as a
    /// fallback tier for `rupu issues list --repo`, between an explicit
    /// `--repo` and cwd git-remote autodetection — the same "unavailable
    /// configured default falls through rather than erroring" contract
    /// `tracker` above already established. Requires `tracker` to also be
    /// set and to name a currently-supported platform (`github` /
    /// `gitlab`); either condition failing, or `project` not parsing as
    /// `<owner>/<repo>`, is treated as unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmPlatformConfig {
    /// The platform this account talks to (`github` / `gitlab`). `None`
    /// means the account name IS the platform — the back-compat rule
    /// from design spec §3.1, which is what keeps a pre-existing
    /// `[scm.github]` table working with no edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
    /// "https" or "ssh"; default chosen by the connector at clone time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_protocol: Option<String>,
}

mod platforms_serde {
    //! Serialize/deserialize `BTreeMap<String, ScmPlatformConfig>` as
    //! flattened sub-tables, but EXCLUDING the reserved `default` key
    //! (which is its own typed field on `ScmSection`).
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::ScmPlatformConfig;

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<String, ScmPlatformConfig>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        map.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, ScmPlatformConfig>, D::Error> {
        let mut raw: BTreeMap<String, ScmPlatformConfig> = BTreeMap::deserialize(d)?;
        // Drop the reserved keys if they slipped through (they're typed
        // separately on ScmSection as `default` / `rules`). In practice
        // serde's flatten routing already excludes any key matching a
        // named sibling field before it reaches this map, so these are
        // defensive no-ops — kept for symmetry and to document intent.
        raw.remove("default");
        raw.remove("rules");
        Ok(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scm_account_declares_a_kind() {
        let toml = r#"
[gh-work]
kind = "github"

[acme-ghe]
kind = "github"
base_url = "https://git.acme.internal/api/v3"
"#;
        let sec: ScmSection = toml::from_str(toml).unwrap();
        assert_eq!(
            sec.platforms.get("gh-work").and_then(|p| p.kind.as_deref()),
            Some("github")
        );
        assert_eq!(
            sec.platforms
                .get("acme-ghe")
                .and_then(|p| p.base_url.as_deref()),
            Some("https://git.acme.internal/api/v3")
        );
    }

    /// Back-compat: `[scm.github]` with no `kind` is still valid — the
    /// account name IS the platform, exactly as spec §3.1 does for LLM
    /// providers.
    #[test]
    fn bare_platform_table_still_parses_without_kind() {
        let sec: ScmSection = toml::from_str("[github]\ntimeout_ms = 5000\n").unwrap();
        let gh = sec.platforms.get("github").unwrap();
        assert_eq!(gh.kind, None);
        assert_eq!(gh.timeout_ms, Some(5000));
    }

    #[test]
    fn rules_parse_owner_and_path_forms() {
        let toml = r#"
[[rules]]
owner = "acme/*"
account = "gh-work"

[[rules]]
path = "~/Code/work/*"
account = "gh-work"
"#;
        let sec: ScmSection = toml::from_str(toml).unwrap();
        assert_eq!(sec.rules.len(), 2);
        assert_eq!(sec.rules[0].owner.as_deref(), Some("acme/*"));
        assert_eq!(sec.rules[0].account, "gh-work");
        assert_eq!(sec.rules[1].path.as_deref(), Some("~/Code/work/*"));
        assert!(sec.rules[1].owner.is_none());
    }

    /// `rules` is a reserved key like `default` — it must not be
    /// swallowed by the flattened per-account map.
    #[test]
    fn rules_is_not_treated_as_an_account() {
        let sec: ScmSection =
            toml::from_str("[[rules]]\nowner = \"a/*\"\naccount = \"x\"\n").unwrap();
        assert!(!sec.platforms.contains_key("rules"));
    }
}
