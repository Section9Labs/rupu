//! Explicit host-owned MCP registry. Deliberately outside layered `Config`:
//! repository/customer configuration cannot replace endpoints or effect policy.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::Path};

const MAX_CONFIG_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    Read,
    Write,
    #[default]
    External,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub label: String,
    pub url: String,
    #[serde(default)]
    pub enabled: bool,
    /// A dedicated MCP environment variable, never a provider/CP credential source.
    pub bearer_env: Option<String>,
    /// Opaque binding generation for the dedicated host MCP credential store.
    /// This is identity metadata, never a token or a provider auth-file path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_store: Option<String>,
    #[serde(default)]
    pub tool_effects: BTreeMap<String, ToolEffect>,
}
impl std::fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerConfig")
            .field("enabled", &self.enabled)
            .field(
                "authenticated",
                &(self.bearer_env.is_some() || self.bearer_store.is_some()),
            )
            .finish_non_exhaustive()
    }
}
impl ServerConfig {
    pub fn effect(&self, remote_tool: &str) -> ToolEffect {
        self.tool_effects
            .get(remote_tool)
            .copied()
            .unwrap_or_default()
    }
    pub fn validate(&self) -> Result<(), RegistryError> {
        if !valid_text(&self.label, 256)
            || self.tool_effects.len() > 256
            || self.tool_effects.keys().any(|key| !valid_text(key, 256))
        {
            return Err(RegistryError::Invalid("invalid label or tool policy"));
        }
        let endpoint =
            url::Url::parse(&self.url).map_err(|_| RegistryError::Invalid("invalid endpoint"))?;
        let loopback = match endpoint.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if endpoint.host().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
        {
            return Err(RegistryError::Invalid("endpoint must be HTTPS or literal loopback HTTP, without userinfo, query or fragment"));
        }
        if let Some(generation) = &self.bearer_store {
            if self.bearer_env.is_some()
                || generation.len() != 26
                || generation.as_bytes()[0] > b'7'
                || !generation
                    .bytes()
                    .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
            {
                return Err(RegistryError::Invalid(
                    "invalid or ambiguous MCP credential binding",
                ));
            }
        }
        if let Some(name) = &self.bearer_env {
            if !name.starts_with("RUPU_MCP_")
                || name.len() <= "RUPU_MCP_".len()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(RegistryError::Invalid(
                    "bearer_env must name a dedicated RUPU_MCP_ variable",
                ));
            }
        }
        Ok(())
    }
}
fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= max
        && !value.chars().any(char::is_control)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub version: u32,
    #[serde(default)]
    pub servers: BTreeMap<String, ServerConfig>,
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            version: 1,
            servers: BTreeMap::new(),
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("cannot read host MCP registry: {0}")]
    Io(std::io::Error),
    // Do not include TOML's rendered source: a malformed file may contain secrets.
    #[error("invalid host MCP registry syntax or fields")]
    Parse,
    #[error("invalid host MCP registry: {0}")]
    Invalid(&'static str),
    #[error("configured MCP server is missing or disabled")]
    Unavailable,
}
impl Registry {
    /// Load only this explicit global root. Never walk cwd, customer or project roots.
    pub fn load(global: &Path) -> Result<Self, RegistryError> {
        let file = match std::fs::File::open(global.join("mcp.toml")) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(RegistryError::Io(e)),
        };
        let mut text = String::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(RegistryError::Io)?;
        Self::parse(&text)
    }
    pub fn parse(text: &str) -> Result<Self, RegistryError> {
        if text.len() as u64 > MAX_CONFIG_BYTES {
            return Err(RegistryError::Invalid("registry is too large"));
        }
        let registry: Self = toml::from_str(text).map_err(|_| RegistryError::Parse)?;
        registry.validate()?;
        Ok(registry)
    }
    pub fn validate(&self) -> Result<(), RegistryError> {
        if self.version != 1 || self.servers.len() > 32 {
            return Err(RegistryError::Invalid(
                "unsupported version or too many servers",
            ));
        }
        for (id, server) in &self.servers {
            if id.is_empty()
                || id.len() > 64
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return Err(RegistryError::Invalid("invalid server identifier"));
            }
            server.validate()?;
        }
        Ok(())
    }
    pub fn enabled_server(&self, id: &str) -> Result<&ServerConfig, RegistryError> {
        self.validate()?;
        self.servers
            .get(id)
            .filter(|s| s.enabled)
            .ok_or(RegistryError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(extra: &str) -> String {
        format!("version=1\n[servers.fixture]\nlabel='Fixture'\nurl='http://127.0.0.1:12345/mcp'\n{extra}")
    }
    #[test]
    fn disabled_and_external_are_defaults_and_explicit_policy_is_exact() {
        let registry =
            Registry::parse(&source("[servers.fixture.tool_effects]\nquery='read'")).unwrap();
        assert!(registry.enabled_server("fixture").is_err());
        assert_eq!(
            registry.servers["fixture"].effect("query"),
            ToolEffect::Read
        );
        assert_eq!(
            registry.servers["fixture"].effect("query_other"),
            ToolEffect::External
        );
    }
    #[test]
    fn load_is_global_only_and_missing_differs_from_malformed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("project/.rupu")).unwrap();
        std::fs::write(
            root.path().join("project/.rupu/mcp.toml"),
            source("enabled=true"),
        )
        .unwrap();
        assert!(Registry::load(root.path()).unwrap().servers.is_empty());
        std::fs::write(root.path().join("mcp.toml"), "malformed secret-content").unwrap();
        let error = Registry::load(root.path()).unwrap_err().to_string();
        assert!(!error.contains("secret-content"));
    }
    #[test]
    fn endpoint_and_auth_sources_cannot_smuggle_credentials_or_process_execution() {
        for url in [
            "http://example.com/mcp",
            "https://user:secret@example.com/mcp",
            "https://example.com/mcp?key=secret",
            "file:///tmp/tool",
            "https://example.com/mcp#fragment",
        ] {
            let text = source("enabled=true").replace("http://127.0.0.1:12345/mcp", url);
            assert!(Registry::parse(&text).is_err());
        }
        for extra in [
            "bearer_env='OPENAI_API_KEY'",
            "bearer_env='RUPU_MCP_'",
            "command='bash'",
            "auth_file='/tmp/auth'",
            "[servers.fixture.tool_effects]\nquery='spawn'",
            "[servers.fixture.tool_effects]\nquery='record'",
        ] {
            assert!(Registry::parse(&source(extra)).is_err());
        }
        assert!(
            Registry::parse(&source("enabled=true\nbearer_env='RUPU_MCP_FIXTURE_TOKEN'")).is_ok()
        );
    }
    #[test]
    fn host_credential_binding_is_typed_exclusive_and_absent_backcompatible() {
        let generation = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let registry = Registry::parse(&source(&format!("bearer_store='{generation}'"))).unwrap();
        assert_eq!(
            registry.servers["fixture"].bearer_store.as_deref(),
            Some(generation)
        );
        for extra in [
            "bearer_store=''",
            "bearer_store='DO_NOT_ECHO_SECRET'",
            "bearer_store='ZZZZZZZZZZZZZZZZZZZZZZZZZZ'",
            "bearer_store='01ARZ3NDEKTSV4RRFFQ69G5FAI'",
            "bearer_store='01ARZ3NDEKTSV4RRFFQ69G5FAV'\nbearer_env='RUPU_MCP_FIXTURE'",
        ] {
            let error = Registry::parse(&source(extra)).unwrap_err().to_string();
            assert!(!error.contains("DO_NOT_ECHO_SECRET"));
        }
        let legacy = Registry::parse(&source("")).unwrap();
        assert!(legacy.servers["fixture"].bearer_store.is_none());
        assert!(!serde_json::to_string(&legacy)
            .unwrap()
            .contains("bearer_store"));
    }
    #[test]
    fn ordinary_layered_config_does_not_accept_mcp_server_registry() {
        assert!(toml::from_str::<crate::Config>(
            "[mcp.servers.fixture]\nurl='https://example.com/mcp'"
        )
        .is_err());
    }
}
