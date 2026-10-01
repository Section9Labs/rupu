//! `ModelCatalog` port — the discovered model-limits catalog + manual
//! refetch (spec 2026-09-30 model-limits discovery §8.2). rupu-cp defines
//! it; rupu-cli's `cp serve` provides the runtime-backed adapter.
pub use rupu_runtime::model_limits::{CatalogModel, CatalogProvider, RefreshOutcome};

#[derive(Debug, thiserror::Error)]
pub enum ModelCatalogError {
    #[error("{0}")]
    UnknownProvider(String),
    #[error("model catalog failed: {0}")]
    Backend(String),
}

/// The runtime's only catalog/refresh error: a `provider` filter that names
/// nothing. Explicit, so a new runtime error type fails to compile here
/// instead of being mapped to a 400 by hand.
impl From<rupu_runtime::model_limits::UnknownProvider> for ModelCatalogError {
    fn from(e: rupu_runtime::model_limits::UnknownProvider) -> Self {
        Self::UnknownProvider(e.0)
    }
}

#[async_trait::async_trait]
pub trait ModelCatalog: Send + Sync {
    async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError>;
    async fn refresh(
        &self,
        provider: Option<String>,
    ) -> Result<Vec<RefreshOutcome>, ModelCatalogError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtime's "no such provider" error becomes the 400-mapped
    /// `UnknownProvider`, message intact — through an explicit conversion,
    /// so a new runtime error type can't silently be mapped to a 400.
    #[test]
    fn unknown_provider_converts_to_the_400_variant() {
        let e: ModelCatalogError =
            rupu_runtime::model_limits::UnknownProvider("unknown provider 'nope'".into()).into();
        assert!(
            matches!(&e, ModelCatalogError::UnknownProvider(m) if m == "unknown provider 'nope'"),
            "{e:?}"
        );
    }
}
