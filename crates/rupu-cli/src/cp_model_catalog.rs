//! `cp serve`'s `ModelCatalog` adapter: delegates to `rupu_runtime::model_limits`.
use rupu_cp::model_catalog::{CatalogProvider, ModelCatalog, ModelCatalogError, RefreshOutcome};
use std::path::PathBuf;

pub struct RuntimeModelCatalog {
    pub global_dir: PathBuf,
}

impl RuntimeModelCatalog {
    fn config(&self) -> Result<(rupu_config::Config, PathBuf), ModelCatalogError> {
        let cfg_path = self.global_dir.join("config.toml");
        let cfg = rupu_config::layer_files_locked(Some(&cfg_path), None)
            .map_err(|e| ModelCatalogError::Backend(e.to_string()))?;
        Ok((cfg, cfg_path))
    }
}

#[async_trait::async_trait]
impl ModelCatalog for RuntimeModelCatalog {
    async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError> {
        let (cfg, cfg_path) = self.config()?;
        rupu_runtime::model_limits::catalog(&cfg, &self.global_dir, &cfg_path, None)
            .await
            .map_err(|e| ModelCatalogError::UnknownProvider(e.to_string()))
    }

    async fn refresh(
        &self,
        provider: Option<String>,
    ) -> Result<Vec<RefreshOutcome>, ModelCatalogError> {
        let (cfg, cfg_path) = self.config()?;
        let resolver = crate::accounts::resolver_for(&cfg);
        rupu_runtime::model_limits::refresh(
            &cfg,
            &self.global_dir,
            &cfg_path,
            &resolver,
            provider.as_deref(),
        )
        .await
        .map_err(|e| ModelCatalogError::UnknownProvider(e.to_string()))
    }
}
