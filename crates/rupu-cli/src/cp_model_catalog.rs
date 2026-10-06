//! `cp serve`'s `ModelCatalog` adapter: delegates to `rupu_runtime::model_limits`.
use rupu_cp::model_catalog::{CatalogProvider, ModelCatalog, ModelCatalogError, RefreshOutcome};
use std::path::PathBuf;

pub struct RuntimeModelCatalog {
    pub global_dir: PathBuf,
}

impl RuntimeModelCatalog {
    fn config(&self) -> Result<(rupu_config::Config, PathBuf), ModelCatalogError> {
        let cfg_path = self.global_dir.join("config.toml");
        let cfg = rupu_config::layer_files_locked(rupu_config::LayerPaths::global_only(&cfg_path))
            .map_err(|e| ModelCatalogError::Backend(e.to_string()))?;
        Ok((cfg, cfg_path))
    }
}

#[async_trait::async_trait]
impl ModelCatalog for RuntimeModelCatalog {
    async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError> {
        let (cfg, cfg_path) = self.config()?;
        let cache_dir = rupu_runtime::model_limits::cache_dir(&self.global_dir);
        rupu_runtime::model_limits::catalog(&cfg, &cache_dir, &cfg_path, None)
            .await
            .map_err(ModelCatalogError::from)
    }

    async fn refresh(
        &self,
        provider: Option<String>,
    ) -> Result<Vec<RefreshOutcome>, ModelCatalogError> {
        let (cfg, cfg_path) = self.config()?;
        let resolver = std::sync::Arc::new(crate::accounts::resolver_for(&cfg));
        rupu_runtime::model_limits::refresh(
            &cfg,
            &rupu_runtime::model_limits::cache_dir(&self.global_dir),
            &cfg_path,
            resolver,
            provider.as_deref(),
            rupu_runtime::model_limits::FETCH_TIMEOUT,
        )
        .await
        // `cp serve` keeps running: a job that outlived its wait finishes
        // detached (dropping a `JoinHandle` never aborts the task).
        .map(|report| report.outcomes)
        .map_err(ModelCatalogError::from)
    }
}
