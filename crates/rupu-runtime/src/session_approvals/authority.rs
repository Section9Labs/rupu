//! Safe host authority metadata frozen before remote clients are assembled.
//! This digest includes credential revision/presence, never credential values.
use rupu_auth::mcp_credentials::{Binding, Status, Store};
use rupu_config::mcp::Registry;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io, path::Path};

pub fn fingerprint(global: &Path, registry: &Registry) -> io::Result<String> {
    let statuses = if registry
        .servers
        .values()
        .any(|s| s.enabled && s.bearer_store.is_some())
    {
        Store::for_global(global)
            .statuses()
            .map_err(|_| unavailable())?
    } else {
        Vec::new()
    };
    fingerprint_metadata(registry, &statuses)
}
fn unavailable() -> io::Error {
    io::Error::other("approval credential authority is unavailable or changed")
}
fn fingerprint_metadata(registry: &Registry, statuses: &[Status]) -> io::Result<String> {
    let mut credentials = BTreeMap::new();
    for (id, server) in &registry.servers {
        if !server.enabled {
            continue;
        }
        if let Some(generation) = &server.bearer_store {
            let binding = Binding {
                server_id: id.clone(),
                url: server.url.clone(),
                generation: generation.clone(),
            };
            binding.validate().map_err(|_| unavailable())?;
            let status = statuses
                .iter()
                .find(|s| s.binding.generation == *generation);
            if status.is_some_and(|s| s.binding != binding) {
                return Err(unavailable());
            }
            credentials.insert(id, json!({"binding":binding,"revision":status.map_or("absent", |s| s.revision.as_str()),"present":status.is_some_and(|s| s.present)}));
        }
    }
    let bytes =
        serde_json::to_vec(&json!({"version":1,"registry":registry,"credentials":credentials}))
            .map_err(|_| unavailable())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_binding_rotation_and_removal_change_authority() {
        let mut registry = Registry::default();
        registry.servers.insert(
            "fixture".into(),
            rupu_config::mcp::ServerConfig {
                label: "Fixture".into(),
                url: "https://example.invalid/mcp".into(),
                enabled: true,
                bearer_env: None,
                bearer_store: Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".into()),
                tool_effects: BTreeMap::new(),
            },
        );
        let binding = Binding {
            server_id: "fixture".into(),
            url: "https://example.invalid/mcp".into(),
            generation: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
        };
        let status = Status {
            binding,
            revision: "a".repeat(32),
            present: true,
        };
        let before = fingerprint_metadata(&registry, std::slice::from_ref(&status)).unwrap();
        let mut rotated = status.clone();
        rotated.revision = "b".repeat(32);
        assert_ne!(before, fingerprint_metadata(&registry, &[rotated]).unwrap());
        let mut removed = status;
        removed.present = false;
        assert_ne!(before, fingerprint_metadata(&registry, &[removed]).unwrap());
        assert_ne!(before, fingerprint_metadata(&registry, &[]).unwrap());
    }
}
