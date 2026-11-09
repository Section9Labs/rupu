//! Operator HTTP-only, head-bound live review pages. No launch or forge writes.
use crate::{
    error::{ApiError, ApiResult},
    repos::{review_pages::*, RepoListError, RepoLister, ReviewSelection},
    state::AppState,
};
use axum::{
    extract::{Path, RawQuery, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path as FsPath,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Mutex as AsyncMutex;

pub const FEATURE: &str = "project.scm_review_pages.v1";
const TTL: Duration = Duration::from_secs(600);
const MAX_OBSERVATIONS: usize = 8; // Each reserves 8 MiB: a hard 64 MiB retained ceiling.
const MAX_RETAINED: usize = 8 * 1024 * 1024;
const MAX_OUTPUT: usize = 512 * 1024;
const MAX_FRAMES: usize = 512;

#[derive(Default)]
pub struct Cache {
    entries: Mutex<HashMap<String, Arc<Observation>>>,
}
struct Observation {
    owner: Owner,
    identity: Identity,
    created: Instant,
    first: String,
    // Encoded cache bytes give an exact retained payload budget; only the two
    // admitted readers decode temporary working copies.
    state: AsyncMutex<Vec<u8>>,
}
#[derive(Default, Serialize, Deserialize)]
struct Pages {
    frames: HashMap<String, Frame>,
    order: Vec<String>,
    seen: HashMap<String, BTreeMap<String, String>>,
    versions: HashMap<String, Option<String>>,
    issued: usize,
}
#[derive(Clone, Serialize, Deserialize)]
struct Frame {
    section: String,
    target: Option<Target>,
    cached: Option<Value>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Owner {
    ws: String,
    cwd: String,
    origin: String,
    account: String,
    branch: String,
    head: String,
    base: Option<String>,
    number: u32,
}
struct Query {
    host: Option<String>,
    origin: String,
    account: String,
    branch: String,
    head: String,
    base: Option<String>,
    section: String,
    cursor: Option<String>,
}
fn expired() -> ApiError {
    ApiError(
        StatusCode::GONE,
        "observation_expired; refresh review context".into(),
    )
}
fn changed() -> ApiError {
    ApiError::conflict("selection_changed; refresh review context")
}
fn busy() -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Review readers are busy; retry this read later".into(),
    )
}
fn malformed() -> ApiError {
    ApiError::bad_gateway("Review page did not satisfy the declared provider contract")
}
fn translate(e: RepoListError) -> ApiError {
    match e {
        RepoListError::SelectionChanged => changed(),
        _ => ApiError::bad_gateway("Review page unavailable from the owning account"),
    }
}
fn nonce() -> String {
    format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new())
}

impl Query {
    fn parse(raw: Option<String>) -> ApiResult<Self> {
        let raw = raw.unwrap_or_default();
        if raw.len() > 16 * 1024 {
            return Err(ApiError::bad_request("Review query exceeds its limit"));
        }
        let mut values = HashMap::new();
        for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
            if !matches!(
                k.as_ref(),
                "host"
                    | "expected_origin"
                    | "expected_account"
                    | "expected_branch"
                    | "expected_head"
                    | "expected_base"
                    | "section"
                    | "cursor"
            ) || values.insert(k.into_owned(), v.into_owned()).is_some()
            {
                return Err(ApiError::bad_request(
                    "Unknown or duplicate review query field",
                ));
            }
        }
        let mut required = |key: &str, max: usize| -> ApiResult<String> {
            values
                .remove(key)
                .filter(|v| !v.is_empty() && v.len() <= max && !v.chars().any(char::is_control))
                .ok_or_else(|| {
                    ApiError::bad_request("A complete captured review selection is required")
                })
        };
        let origin = required("expected_origin", 4096)?;
        let account = required("expected_account", 256)?;
        let branch = required("expected_branch", 1024)?;
        let head = required("expected_head", 128)?;
        let section = required("section", 32)?;
        let cursor = values.remove("cursor");
        let base = values.remove("expected_base");
        if !matches!(
            section.as_str(),
            "checks" | "threads" | "comments" | "diff_files" | "text"
        ) || cursor
            .as_ref()
            .is_some_and(|v| v.len() > 8192 || v.is_empty())
            || base
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 128 || v.chars().any(char::is_control))
            || (cursor.is_none() && matches!(section.as_str(), "comments" | "text"))
        {
            return Err(ApiError::bad_request(
                "Invalid review section or continuation",
            ));
        }
        Ok(Self {
            host: values.remove("host"),
            origin,
            account,
            branch,
            head,
            base,
            section,
            cursor,
        })
    }
}

pub async fn get(
    State(s): State<AppState>,
    Path((ws, number)): Path<(String, u32)>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    let q = Query::parse(raw)?;
    super::fleet::require_local(q.host.as_deref(), "project review pages")?;
    if number == 0 {
        return Err(ApiError::bad_request("Review number must be positive"));
    }
    let _permit = super::project_scm::REVIEW_READERS
        .clone()
        .try_acquire_owned()
        .map_err(|_| busy())?;
    tokio::time::timeout(Duration::from_secs(25), read(s, ws, number, q))
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::GATEWAY_TIMEOUT,
                "Review page deadline exceeded; retry this read or refresh".into(),
            )
        })?
        .map(Json)
}

async fn checkout(
    s: &AppState,
    ws: &str,
    number: u32,
    q: &Query,
) -> ApiResult<(Owner, rupu_scm::RepoRef)> {
    let Json(v) = super::project_scm::context(
        State(s.clone()),
        Path(ws.into()),
        axum::extract::Query(super::project_scm::ContextQuery::default()),
    )
    .await?;
    if v["state"] != "ready"
        || v["branch"].as_str() != Some(&q.branch)
        || v["repository"]["home_url"].as_str() != Some(&q.origin)
    {
        return Err(changed());
    }
    let required = |v: &Value| {
        v.as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(changed)
    };
    let repo = rupu_scm::RepoRef {
        platform: required(&v["repository"]["forge"])?
            .parse()
            .map_err(|_| changed())?,
        owner: required(&v["repository"]["owner"])?,
        repo: required(&v["repository"]["name"])?,
    };
    Ok((
        Owner {
            ws: ws.into(),
            cwd: required(&v["checkout_path"])?,
            origin: q.origin.clone(),
            account: q.account.clone(),
            branch: q.branch.clone(),
            head: q.head.clone(),
            base: q.base.clone(),
            number,
        },
        repo,
    ))
}
fn selection(owner: &Owner) -> ReviewSelection {
    ReviewSelection {
        account: owner.account.clone(),
        branch: owner.branch.clone(),
        head: owner.head.clone(),
        include_diff: false,
    }
}
fn valid_identity(owner: &Owner, id: &Identity) -> ApiResult<()> {
    if id.account != owner.account
        || id.branch != owner.branch
        || id.head != owner.head
        || id.number != owner.number
        || owner.base.as_ref().is_some_and(|b| b != &id.base)
    {
        return Err(changed());
    }
    if [
        &id.connector_epoch,
        &id.api_profile,
        &id.repository_id,
        &id.base,
    ]
    .iter()
    .any(|s| s.is_empty() || s.len() > 4096)
    {
        return Err(malformed());
    }
    Ok(())
}

async fn verify(
    port: &Arc<dyn RepoLister>,
    repo: &rupu_scm::RepoRef,
    owner: &Owner,
    id: &Identity,
) -> ApiResult<()> {
    let current = port
        .review_page_identity(
            repo,
            FsPath::new(&owner.cwd),
            &owner.origin,
            owner.number,
            &selection(owner),
        )
        .await
        .map_err(translate)?;
    valid_identity(owner, &current)?;
    if &current != id {
        return Err(changed());
    }
    Ok(())
}

async fn read(s: AppState, ws: String, number: u32, q: Query) -> ApiResult<Value> {
    let s = crate::repos::snapshot_state(&s).await.map_err(translate)?;
    let port = s
        .repos
        .clone()
        .filter(|p| p.supports_review_pages())
        .ok_or_else(|| ApiError::not_available("Review continuation adapter unavailable"))?;
    let (owner, repo) = checkout(&s, &ws, number, &q).await?;
    if let Some(cursor) = &q.cursor {
        let (observation_id, frame_id) = cursor
            .split_once('.')
            .filter(|(a, b)| a.len() == 52 && b.len() == 52)
            .ok_or_else(expired)?;
        let observation = {
            let mut cache = s.review_pages.entries.lock().map_err(|_| expired())?;
            cache.retain(|_, o| o.created.elapsed() < TTL);
            cache.get(observation_id).cloned().ok_or_else(expired)?
        };
        if observation.owner != owner {
            return Err(changed());
        }
        // At most two callers are admitted by the shared semaphore. A second
        // same-cursor read waits for publication, then returns the cached page.
        let mut stored = observation.state.lock().await;
        let mut pages: Pages = serde_json::from_slice(&stored).map_err(|_| expired())?;
        let frame = pages.frames.get(frame_id).cloned().ok_or_else(expired)?;
        if frame.section != q.section {
            return Err(changed());
        }
        let result = if let Some(cached) = frame.cached {
            verify(&port, &repo, &owner, &observation.identity).await?;
            cached
        } else {
            let target = frame.target.ok_or_else(expired)?;
            let page = match target {
                Target::Provider(request) => port
                    .review_page(
                        &repo,
                        FsPath::new(&owner.cwd),
                        &owner.origin,
                        number,
                        &selection(&owner),
                        Some(&observation.identity),
                        &request,
                    )
                    .await
                    .map_err(translate)?,
                target => {
                    verify(&port, &repo, &owner, &observation.identity).await?;
                    captured(&observation.identity, target)?
                }
            };
            if page.identity != observation.identity {
                return Err(changed());
            }
            if checkout(&s, &ws, number, &q).await?.0 != owner {
                return Err(changed());
            }
            publish(
                &mut pages,
                &observation,
                observation_id,
                frame_id,
                &q.section,
                page,
            )?
        };
        if checkout(&s, &ws, number, &q).await?.0 != owner {
            return Err(changed());
        }
        *stored = serde_json::to_vec(&pages).map_err(|_| malformed())?;
        Ok(result)
    } else {
        let page = port
            .review_page(
                &repo,
                FsPath::new(&owner.cwd),
                &owner.origin,
                number,
                &selection(&owner),
                None,
                &Request {
                    section: q.section.clone(),
                    after: None,
                    parent: None,
                },
            )
            .await
            .map_err(translate)?;
        valid_identity(&owner, &page.identity)?;
        if checkout(&s, &ws, number, &q).await?.0 != owner {
            return Err(changed());
        }
        let observation_id = nonce();
        let frame_id = nonce();
        let observation = Arc::new(Observation {
            owner,
            identity: page.identity.clone(),
            created: Instant::now(),
            first: chrono::Utc::now().to_rfc3339(),
            state: AsyncMutex::new(Vec::new()),
        });
        let mut pages = Pages::default();
        let result = publish(
            &mut pages,
            &observation,
            &observation_id,
            &frame_id,
            &q.section,
            page,
        )?;
        *observation.state.lock().await = serde_json::to_vec(&pages).map_err(|_| malformed())?;
        let mut cache = s.review_pages.entries.lock().map_err(|_| expired())?;
        cache.retain(|_, o| o.created.elapsed() < TTL);
        if cache.len() >= MAX_OBSERVATIONS {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Review observation budget reached; refresh after earlier observations expire"
                    .into(),
            ));
        }
        cache.insert(observation_id, observation);
        Ok(result)
    }
}

fn captured(id: &Identity, target: Target) -> ApiResult<Page> {
    match target {
        Target::Captured { scope, items } => Ok(Page {
            identity: id.clone(),
            scope,
            items,
            next: None,
            provider_exhausted: true,
            omissions: vec![],
            input_bytes: 0,
            page_version: None,
        }),
        Target::Text {
            id: content_id,
            text,
            offset,
        } => {
            if offset > text.len() || !text.is_char_boundary(offset) {
                return Err(malformed());
            }
            let mut end = (offset + 64 * 1024).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let mut children = vec![];
            if end < text.len() {
                children.push(Child {
                    field: "text_cursor".into(),
                    section: "text".into(),
                    target: Target::Text {
                        id: content_id.clone(),
                        text: text.clone(),
                        offset: end,
                    },
                });
            }
            use sha2::{Digest, Sha256};
            let value = json!({"id":content_id,"text":text[offset..end],"byte_start":offset,"byte_end":end,"total_bytes":text.len(),"sha256":format!("{:x}",Sha256::digest(text.as_bytes())),"fragment":true});
            Ok(Page {
                identity: id.clone(),
                scope: "captured_text".into(),
                items: vec![Item {
                    id: format!("{content_id}:{offset}"),
                    value,
                    children,
                }],
                next: None,
                provider_exhausted: true,
                omissions: vec![],
                input_bytes: 0,
                page_version: None,
            })
        }
        Target::Provider(_) => Err(malformed()),
    }
}

fn add_frame(
    pages: &mut Pages,
    observation_id: &str,
    section: String,
    target: Target,
) -> ApiResult<String> {
    if pages.issued >= MAX_FRAMES {
        return Err(ApiError(
            StatusCode::GONE,
            "observation_budget; refresh review context".into(),
        ));
    }
    let id = nonce();
    pages.issued += 1;
    pages.frames.insert(
        id.clone(),
        Frame {
            section,
            target: Some(target),
            cached: None,
        },
    );
    Ok(format!("{observation_id}.{id}"))
}

fn publish(
    pages: &mut Pages,
    observation: &Observation,
    observation_id: &str,
    frame_id: &str,
    section: &str,
    mut page: Page,
) -> ApiResult<Value> {
    if page.input_bytes > 4 * 1024 * 1024 || page.items.len() > 2000 {
        return Err(malformed());
    }
    // Publish transactionally: a rejected page must never leave orphan capabilities
    // or advance the frontier. Bounded clone is covered by two admitted readers.
    let mut draft = Pages {
        frames: pages.frames.clone(),
        order: pages.order.clone(),
        seen: pages.seen.clone(),
        versions: pages.versions.clone(),
        issued: pages.issued,
    };
    if draft
        .versions
        .get(&page.scope)
        .is_some_and(|v| v != &page.page_version)
    {
        return Err(changed());
    }
    draft
        .versions
        .insert(page.scope.clone(), page.page_version.clone());
    let limit = if section == "checks" { 40 } else { 20 };
    let remainder = if page.items.len() > limit {
        page.items.split_off(limit)
    } else {
        vec![]
    };
    let mut items = vec![];
    for mut row in page.items {
        if row.id.is_empty() || row.id.len() > 4096 || !row.value.is_object() {
            return Err(malformed());
        }
        // Duplicate identities in a new page require refresh. Cached replay does
        // not enter this path. No edited duplicate silently replaces a shown row.
        let lineage = format!("{section}:{}", page.scope);
        let seen = draft.seen.entry(lineage).or_default();
        if seen.len() >= 10000 || seen.contains_key(&row.id) {
            return Err(changed());
        }
        seen.insert(row.id.clone(), String::new());
        for child in row.children {
            if !matches!(
                child.field.as_str(),
                "comments_cursor" | "body_cursor" | "patch_cursor" | "text_cursor"
            ) {
                return Err(malformed());
            }
            row.value[child.field] = json!(add_frame(
                &mut draft,
                observation_id,
                child.section,
                child.target
            )?);
        }
        items.push(row.value);
    }
    // Provider adapters request exact output row limits. Captured notes can be
    // split internally. Never discard an extra provider row before advancing.
    let next = if !remainder.is_empty() {
        if page.next.is_some() {
            return Err(malformed());
        }
        Some(add_frame(
            &mut draft,
            observation_id,
            section.into(),
            Target::Captured {
                scope: page.scope.clone(),
                items: remainder,
            },
        )?)
    } else if let Some(next) = page.next {
        Some(add_frame(
            &mut draft,
            observation_id,
            next.section.clone(),
            Target::Provider(next),
        )?)
    } else {
        None
    };
    let result = json!({"ws_id":observation.owner.ws,"host_id":"local","origin":observation.owner.origin,"account":observation.identity.account,
        "number":observation.identity.number,"branch":observation.identity.branch,"head_sha":observation.identity.head,"base_sha":observation.identity.base,
        "merge_base_sha":observation.identity.start,"section":section,"scope":page.scope,"items":items,"cursor":format!("{observation_id}.{frame_id}"),"next_cursor":next,
        "provider_exhausted":page.provider_exhausted,"partial":!page.omissions.is_empty(),"omissions":page.omissions,"atomic":false,"consistency":"head_bound_live",
        "first_observed_at":observation.first,"observed_at":chrono::Utc::now().to_rfc3339(),"provider_version":observation.identity.provider_version,"page_provider_version":page.page_version});
    if serde_json::to_vec(&result).map_err(|_| malformed())?.len() > MAX_OUTPUT {
        return Err(ApiError::bad_gateway(
            "output_byte_limit; this provider page cannot be displayed within the review budget",
        ));
    }
    draft.frames.insert(
        frame_id.into(),
        Frame {
            section: section.into(),
            target: None,
            cached: Some(result.clone()),
        },
    );
    draft.order.push(frame_id.into());
    while draft.order.len() > 16 {
        let old = draft.order.remove(0);
        draft.frames.remove(&old);
    }
    if serde_json::to_vec(&draft).map_err(|_| malformed())?.len() > MAX_RETAINED {
        return Err(ApiError(
            StatusCode::GONE,
            "observation_budget; refresh review context".into(),
        ));
    }
    *pages = draft;
    Ok(result)
}

#[cfg(test)]
mod tests;
