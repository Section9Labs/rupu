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

#[async_trait::async_trait]
pub trait ModelCatalog: Send + Sync {
    async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError>;
    async fn refresh(
        &self,
        provider: Option<String>,
    ) -> Result<Vec<RefreshOutcome>, ModelCatalogError>;
}
