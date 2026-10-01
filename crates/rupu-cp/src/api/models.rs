use crate::{
    error::{ApiError, ApiResult},
    model_catalog::{CatalogProvider, ModelCatalog, ModelCatalogError, RefreshOutcome},
    state::AppState,
};
use axum::{
    body::Bytes,
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/models", get(list_models))
        .route("/api/models/refresh", post(refresh_models))
}

/// `deny_unknown_fields`: a misspelled `provider` key must be a 400, not a
/// silent "refresh every provider".
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshBody {
    #[serde(default)]
    pub provider: Option<String>,
}

/// Parse a refresh request body. An empty (or all-whitespace) body is "no
/// narrowing" = refresh every provider, which keeps a bodyless
/// `curl -X POST` working. Anything else must be a valid `RefreshBody`:
/// a malformed body is a 400, never a silent refresh-all (a request meant to
/// narrow to one provider would otherwise fan out to every vendor with real
/// credentials). Content-Type is deliberately not required.
fn parse_refresh_body(raw: &[u8]) -> ApiResult<RefreshBody> {
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(RefreshBody::default());
    }
    serde_json::from_slice(raw)
        .map_err(|e| ApiError::bad_request(format!("invalid refresh body: {e}")))
}

fn map_err(e: ModelCatalogError) -> ApiError {
    match e {
        ModelCatalogError::UnknownProvider(m) => ApiError::bad_request(m),
        ModelCatalogError::Backend(m) => ApiError::internal(m),
    }
}

fn port(p: Option<Arc<dyn ModelCatalog>>) -> ApiResult<Arc<dyn ModelCatalog>> {
    p.ok_or_else(|| ApiError::not_available("the model catalog requires `rupu cp serve`"))
}

async fn list_with(p: Option<Arc<dyn ModelCatalog>>) -> ApiResult<Vec<CatalogProvider>> {
    port(p)?.list().await.map_err(map_err)
}

/// Synchronous: providers are fetched in parallel, each bounded by the
/// runtime's 10s timeout. A 200 means the refresh ran; each provider's
/// `ok`/`error` outcome is authoritative.
async fn refresh_with(
    p: Option<Arc<dyn ModelCatalog>>,
    body: RefreshBody,
) -> ApiResult<Vec<RefreshOutcome>> {
    port(p)?.refresh(body.provider).await.map_err(map_err)
}

async fn list_models(State(s): State<AppState>) -> ApiResult<Json<Vec<CatalogProvider>>> {
    Ok(Json(list_with(s.model_catalog.clone()).await?))
}

async fn refresh_models(
    State(s): State<AppState>,
    body: Bytes,
) -> ApiResult<Json<Vec<RefreshOutcome>>> {
    let body = parse_refresh_body(&body)?;
    Ok(Json(refresh_with(s.model_catalog.clone(), body).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_catalog::{ModelCatalog, ModelCatalogError};
    use rupu_runtime::model_limits::{CatalogModel, CatalogProvider, RefreshOutcome};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Fake {
        asked: Mutex<Vec<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl ModelCatalog for Fake {
        async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError> {
            Ok(vec![CatalogProvider {
                provider: "anthropic".into(),
                fetched_at: None,
                stale: false,
                models: vec![CatalogModel {
                    id: "claude-a".into(),
                    input_tokens: Some(1000),
                    output_tokens: None,
                    source: "live".into(),
                }],
            }])
        }
        async fn refresh(
            &self,
            provider: Option<String>,
        ) -> Result<Vec<RefreshOutcome>, ModelCatalogError> {
            if provider.as_deref() == Some("nope") {
                return Err(ModelCatalogError::UnknownProvider(
                    "unknown provider 'nope'".into(),
                ));
            }
            self.asked.lock().unwrap().push(provider.clone());
            Ok(vec![RefreshOutcome {
                provider: provider.unwrap_or_else(|| "anthropic".into()),
                ok: true,
                count: 1,
                error: None,
            }])
        }
    }

    #[tokio::test]
    async fn lists_from_port() {
        let port: Arc<dyn ModelCatalog> = Arc::new(Fake::default());
        let out = list_with(Some(port)).await.unwrap();
        assert_eq!(out[0].models[0].input_tokens, Some(1000));
    }

    #[tokio::test]
    async fn refresh_passes_the_provider_through() {
        let fake = Arc::new(Fake::default());
        let port: Arc<dyn ModelCatalog> = fake.clone();
        let out = refresh_with(
            Some(port),
            RefreshBody {
                provider: Some("anthropic".into()),
            },
        )
        .await
        .unwrap();
        assert!(out[0].ok);
        assert_eq!(
            fake.asked.lock().unwrap().as_slice(),
            [Some("anthropic".to_string())]
        );
    }

    #[tokio::test]
    async fn unknown_provider_is_400() {
        let port: Arc<dyn ModelCatalog> = Arc::new(Fake::default());
        let err = refresh_with(
            Some(port),
            RefreshBody {
                provider: Some("nope".into()),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_port_is_501() {
        assert_eq!(
            list_with(None).await.unwrap_err().0,
            axum::http::StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            refresh_with(None, RefreshBody::default())
                .await
                .unwrap_err()
                .0,
            axum::http::StatusCode::NOT_IMPLEMENTED
        );
    }
}
