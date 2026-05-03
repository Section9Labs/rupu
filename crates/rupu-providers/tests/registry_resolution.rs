use rupu_providers::model_pool::{ModelCost, ModelInfo, ModelStatus};
use rupu_providers::model_registry::{ModelRegistry, ModelSource};
use rupu_providers::provider_id::ProviderId;

fn mi(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        provider: ProviderId::OpenaiCodex,
        context_window: 0,
        max_output_tokens: 0,
        capabilities: Vec::new(),
        cost: ModelCost::default(),
        status: ModelStatus::default(),
    }
}

#[tokio::test]
async fn custom_models_take_precedence_over_live() {
    let dir = tempfile::tempdir().unwrap();
    let reg = ModelRegistry::with_cache_dir(dir.path());
    reg.set_custom("openai", vec![mi("gpt-5-internal-finetune")])
        .await;
    reg.set_live_cache("openai", vec![mi("gpt-5"), mi("gpt-5-internal-finetune")])
        .await;
    let models = reg.list("openai").await;
    let custom = models
        .iter()
        .find(|m| m.entry.id == "gpt-5-internal-finetune")
        .unwrap();
    assert_eq!(custom.source, ModelSource::Custom);
}

#[tokio::test]
async fn unknown_model_resolution_errors_with_actionable_message() {
    let dir = tempfile::tempdir().unwrap();
    let reg = ModelRegistry::with_cache_dir(dir.path());
    reg.set_live_cache("openai", vec![mi("gpt-5")]).await;
    let res = reg.resolve("openai", "gpt-9000").await;
    let err = res.unwrap_err().to_string();
    assert!(err.contains("not found"));
    assert!(err.contains("rupu models list"));
}

#[tokio::test]
async fn known_model_resolves_from_live_cache() {
    let dir = tempfile::tempdir().unwrap();
    let reg = ModelRegistry::with_cache_dir(dir.path());
    reg.set_live_cache("openai", vec![mi("gpt-5")]).await;
    let entry = reg.resolve("openai", "gpt-5").await.unwrap();
    assert_eq!(entry.entry.id, "gpt-5");
    assert_eq!(entry.source, ModelSource::Live);
}
