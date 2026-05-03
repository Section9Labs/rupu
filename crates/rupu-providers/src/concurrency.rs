//! Per-provider concurrency limits.
//!
//! Slice B-1 spec §7c: each provider has its own Semaphore so that a
//! saturated rate-limit on one vendor doesn't drain capacity for the
//! others. Defaults are conservative; override via
//! `[providers.<name>].max_concurrency` in `~/.rupu/config.toml`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tokio::sync::Semaphore;

/// Default per-provider permits. Mirrors documented per-key rate limits.
pub fn default_permits(provider: &str) -> usize {
    match provider {
        "anthropic" => 4,
        "openai" => 8,
        "gemini" => 4,
        "copilot" => 4,
        _ => 4,
    }
}

/// Process-wide semaphore registry. Lazily initialized per provider.
static REGISTRY: OnceLock<std::sync::Mutex<HashMap<String, Arc<Semaphore>>>> = OnceLock::new();

fn registry() -> &'static std::sync::Mutex<HashMap<String, Arc<Semaphore>>> {
    REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Look up (or create) the semaphore for `provider`. `permits_override`
/// applies only the first time the entry is created; subsequent calls
/// re-use the existing semaphore.
pub fn semaphore_for(provider: &str, permits_override: Option<usize>) -> Arc<Semaphore> {
    let mut map = registry().lock().expect("semaphore registry poisoned");
    map.entry(provider.to_string())
        .or_insert_with(|| {
            let permits = permits_override.unwrap_or_else(|| default_permits(provider));
            Arc::new(Semaphore::new(permits))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_permits_match_spec() {
        assert_eq!(default_permits("anthropic"), 4);
        assert_eq!(default_permits("openai"), 8);
        assert_eq!(default_permits("gemini"), 4);
        assert_eq!(default_permits("copilot"), 4);
        assert_eq!(default_permits("unknown"), 4);
    }

    #[tokio::test]
    async fn semaphore_for_returns_isolated_semaphores() {
        let a = semaphore_for("alpha-test", Some(2));
        let b = semaphore_for("beta-test", Some(2));
        let _g1 = a.clone().acquire_owned().await.unwrap();
        let _g2 = a.clone().acquire_owned().await.unwrap();
        // alpha at 0 permits; beta should still allow acquire.
        let _g3 = b.clone().acquire_owned().await.unwrap();
        assert_eq!(a.available_permits(), 0);
        assert_eq!(b.available_permits(), 1);
    }

    #[tokio::test]
    async fn semaphore_for_caches_first_call() {
        let a1 = semaphore_for("gamma-test", Some(3));
        let a2 = semaphore_for("gamma-test", Some(99));
        // Same Arc → same permits.
        assert_eq!(a1.available_permits(), 3);
        assert_eq!(a2.available_permits(), 3);
        assert!(std::sync::Arc::ptr_eq(&a1, &a2));
    }
}
