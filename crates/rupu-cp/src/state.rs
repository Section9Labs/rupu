use rupu_orchestrator::runs::RunStore;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub global_dir: PathBuf,
    pub run_store: Arc<RunStore>,
    pub pricing: rupu_config::PricingConfig,
}

impl AppState {
    pub fn new(global_dir: PathBuf, pricing: rupu_config::PricingConfig) -> Self {
        let run_store = Arc::new(RunStore::new(global_dir.join("runs")));
        Self {
            global_dir,
            run_store,
            pricing,
        }
    }
}
