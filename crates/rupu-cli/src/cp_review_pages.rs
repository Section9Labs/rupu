//! Exact-account provider projections for head-bound live review continuation.
//! All endpoints/documents are fixed here; vendor cursors are server-only data.
use rupu_cp::repos::{review_pages::*, RepoListError, ReviewSelection};
use rupu_scm::connectors::{github::GithubExtras, gitlab::GitlabExtras};
use rupu_scm::{review_transport::ReviewResponse, Platform, Registry, RepoRef};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};
const ID_BYTES: usize = 64 * 1024;
fn failed() -> RepoListError {
    RepoListError::Backend("Review continuation provider contract unavailable".into())
}
fn changed() -> RepoListError {
    RepoListError::SelectionChanged
}
fn string(v: &Value) -> Result<String, RepoListError> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .map(str::to_owned)
        .ok_or_else(failed)
}
fn numeric(v: &Value) -> Result<String, RepoListError> {
    v.as_u64()
        .filter(|n| *n > 0)
        .map(|n| n.to_string())
        .ok_or_else(failed)
}
fn bounded(v: &Value, max: usize) -> Result<String, RepoListError> {
    let text = v.as_str().ok_or_else(failed)?;
    if text.len() > max {
        return Err(failed());
    }
    Ok(text.into())
}
fn graphql(response: ReviewResponse) -> Result<(Value, usize, Option<String>), RepoListError> {
    if response
        .body
        .get("errors")
        .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(failed());
    }
    let body = response
        .body
        .get("data")
        .filter(|v| v.is_object())
        .cloned()
        .ok_or_else(failed)?;
    Ok((body, response.bytes, response.version))
}
fn link(v: &Value, origin: &str) -> Value {
    let result = v
        .as_str()
        .filter(|s| s.len() <= 4096)
        .and_then(|s| url::Url::parse(s).ok())
        .filter(|u| {
            let Ok(o) = url::Url::parse(origin) else {
                return false;
            };
            u.scheme() == o.scheme()
                && u.host_str() == o.host_str()
                && u.port_or_known_default() == o.port_or_known_default()
                && u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
        });
    json!(result.map(|u| u.to_string()))
}
fn nullable_text(v: &Value, max: usize) -> Result<Value, RepoListError> {
    if v.is_null() {
        Ok(Value::Null)
    } else {
        Ok(json!(bounded(v, max)?))
    }
}
fn nullable_bool(v: &Value) -> Result<Value, RepoListError> {
    if v.is_null() {
        Ok(Value::Null)
    } else {
        v.as_bool().map(|b| json!(b)).ok_or_else(failed)
    }
}
fn nullable_line(v: &Value) -> Result<Value, RepoListError> {
    if v.is_null() {
        Ok(Value::Null)
    } else {
        v.as_u64()
            .filter(|n| *n > 0)
            .map(|n| json!(n))
            .ok_or_else(failed)
    }
}
fn encoded(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
fn prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn text_child(id: &str, text: &str, field: &str) -> Vec<Child> {
    if text.len() > 4096 {
        vec![Child {
            field: field.into(),
            section: "text".into(),
            target: Target::Text {
                id: id.into(),
                text: text.into(),
                offset: 0,
            },
        }]
    } else {
        vec![]
    }
}
fn page_number(request: &Request) -> Result<u32, RepoListError> {
    match request.after.as_deref() {
        None => Ok(1),
        Some(s) => s
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 1 && *n <= 512)
            .ok_or_else(failed),
    }
}
fn rest_next(
    r: &ReviewResponse,
    n: u32,
    section: &str,
) -> Result<(Option<Request>, bool, Vec<String>), RepoListError> {
    if !r.pagination_known {
        return Ok((None, false, vec!["pagination_unreported".into()]));
    }
    match r.next_page {
        Some(next) if next > n && next <= 512 => Ok((
            Some(Request {
                section: section.into(),
                after: Some(next.to_string()),
                parent: None,
            }),
            false,
            vec![],
        )),
        Some(_) => Err(failed()),
        None => Ok((None, true, vec![])),
    }
}
fn graph_next(connection: &Value, request: &Request) -> Result<Option<Request>, RepoListError> {
    match connection["pageInfo"]["hasNextPage"]
        .as_bool()
        .ok_or_else(failed)?
    {
        false => Ok(None),
        true => {
            let cursor = string(&connection["pageInfo"]["endCursor"])?;
            if request.after.as_ref() == Some(&cursor) {
                return Err(failed());
            }
            Ok(Some(Request {
                after: Some(cursor),
                ..request.clone()
            }))
        }
    }
}
fn rows(connection: &Value, max: usize) -> Result<&Vec<Value>, RepoListError> {
    connection
        .as_array()
        .filter(|v| v.len() <= max)
        .ok_or_else(failed)
}
enum Connector {
    Github(Arc<GithubExtras>),
    Gitlab(Arc<GitlabExtras>),
}
impl Connector {
    fn epoch(&self) -> String {
        match self {
            Self::Github(c) => c.review_identity_epoch(),
            Self::Gitlab(c) => c.review_identity_epoch(),
        }
    }
    fn profile(&self) -> String {
        match self {
            Self::Github(c) => c.review_api_profile(),
            Self::Gitlab(c) => c.review_api_profile(),
        }
    }
}
fn resolve(
    registry: &Registry,
    repo: &RepoRef,
    cwd: &Path,
    origin: &str,
    selection: &ReviewSelection,
) -> Result<Connector, RepoListError> {
    match repo.platform {
        Platform::Github => {
            let (account, extras) = registry
                .github_extras_for(repo, Some(cwd), None)
                .map_err(|_| failed())?;
            if account.to_string() != selection.account || !extras.review_matches_origin(origin) {
                return Err(changed());
            }
            Ok(Connector::Github(extras))
        }
        Platform::Gitlab => {
            let (account, extras) = registry
                .gitlab_extras_for(repo, Some(cwd), None)
                .map_err(|_| failed())?;
            if account.to_string() != selection.account || !extras.review_matches_origin(origin) {
                return Err(changed());
            }
            Ok(Connector::Gitlab(extras))
        }
    }
}
async fn identity(
    conn: &Connector,
    repo: &RepoRef,
    number: u32,
    selection: &ReviewSelection,
) -> Result<(Identity, usize), RepoListError> {
    let epoch = conn.epoch();
    let (repository_id, base, start, pipeline, file_count, version, bytes) = match conn {
        Connector::Github(c) => {
            let (data, bytes, version) = graphql(
                c.review_graphql_bounded(
                    GH_ID,
                    json!({"owner":repo.owner,"name":repo.repo,"number":number}),
                    ID_BYTES,
                )
                .await
                .map_err(|_| failed())?,
            )?;
            let repository = &data["repository"];
            let row = &repository["pullRequest"];
            let repository_id = string(&repository["id"])?;
            if row["number"].as_u64() != Some(number.into())
                || row["headRefName"].as_str() != Some(&selection.branch)
                || row["headRefOid"].as_str() != Some(&selection.head)
                || row["headRepository"]["id"].as_str() != Some(&repository_id)
                || !repository["nameWithOwner"].as_str().is_some_and(|v| {
                    v.eq_ignore_ascii_case(&format!("{}/{}", repo.owner, repo.repo))
                })
            {
                return Err(changed());
            }
            (
                repository_id,
                string(&row["baseRefOid"])?,
                None,
                None,
                Some(row["changedFiles"].as_u64().ok_or_else(failed)?),
                version,
                bytes,
            )
        }
        Connector::Gitlab(c) => {
            let response = c
                .review_get_bounded(
                    &format!(
                        "/projects/{}/merge_requests/{number}",
                        encoded(&format!("{}/{}", repo.owner, repo.repo))
                    ),
                    ID_BYTES,
                )
                .await
                .map_err(|_| failed())?;
            let row = &response.body;
            let repository_id = numeric(&row["project_id"])?;
            if row["iid"].as_u64() != Some(number.into())
                || row["source_branch"].as_str() != Some(&selection.branch)
                || row["diff_refs"]["head_sha"].as_str() != Some(&selection.head)
                || row["source_project_id"] != row["project_id"]
                || row["target_project_id"] != row["project_id"]
            {
                return Err(changed());
            }
            let pipeline = if row["head_pipeline"].is_null() {
                None
            } else {
                if row["head_pipeline"]["sha"].as_str() != Some(&selection.head) {
                    return Err(changed());
                }
                Some(numeric(&row["head_pipeline"]["id"])?)
            };
            (
                repository_id,
                string(&row["diff_refs"]["start_sha"])?,
                Some(string(&row["diff_refs"]["base_sha"])?),
                pipeline,
                None,
                response.version,
                response.bytes,
            )
        }
    };
    if epoch != conn.epoch() {
        return Err(changed());
    }
    Ok((
        Identity {
            account: selection.account.clone(),
            connector_epoch: epoch,
            api_profile: conn.profile(),
            provider_version: version,
            repository_id,
            number,
            branch: selection.branch.clone(),
            head: selection.head.clone(),
            base,
            start,
            pipeline,
            file_count,
        },
        bytes,
    ))
}

pub(crate) async fn read_identity(
    registry: &Registry,
    repo: &RepoRef,
    cwd: &Path,
    origin: &str,
    number: u32,
    selection: &ReviewSelection,
) -> Result<Identity, RepoListError> {
    let conn = resolve(registry, repo, cwd, origin, selection)?;
    let (id, _) = identity(&conn, repo, number, selection).await?;
    if resolve(registry, repo, cwd, origin, selection)?.epoch() != id.connector_epoch {
        return Err(changed());
    }
    Ok(id)
}
pub(crate) async fn read(
    registry: &Registry,
    repo: &RepoRef,
    cwd: &Path,
    origin: &str,
    number: u32,
    selection: &ReviewSelection,
    expected: Option<&Identity>,
    request: &Request,
) -> Result<Page, RepoListError> {
    if !matches!(
        request.section.as_str(),
        "checks" | "threads" | "comments" | "diff_files"
    ) || request.after.as_ref().is_some_and(|v| v.len() > 4096)
        || request.parent.as_ref().is_some_and(|v| v.len() > 4096)
    {
        return Err(failed());
    }
    let conn = resolve(registry, repo, cwd, origin, selection)?;
    let (before, bytes) = identity(&conn, repo, number, selection).await?;
    if expected.is_some_and(|e| e != &before) {
        return Err(changed());
    }
    let mut page = match &conn {
        Connector::Github(c) => github(c, repo, origin, &before, request).await?,
        Connector::Gitlab(c) => gitlab(c, origin, &before, request).await?,
    };
    let (after, after_bytes) = identity(&conn, repo, number, selection).await?;
    if before != after
        || resolve(registry, repo, cwd, origin, selection)?.epoch() != before.connector_epoch
    {
        return Err(changed());
    }
    page.input_bytes += bytes + after_bytes;
    if page.input_bytes > 4 * 1024 * 1024 {
        return Err(failed());
    }
    Ok(page)
}
fn empty(id: &Identity, scope: &str) -> Page {
    Page {
        identity: id.clone(),
        scope: scope.into(),
        items: vec![],
        next: None,
        provider_exhausted: true,
        omissions: vec![],
        input_bytes: 0,
        page_version: None,
    }
}
fn check_version(id: &Identity, version: &Option<String>) -> Result<(), RepoListError> {
    if &id.provider_version != version {
        Err(changed())
    } else {
        Ok(())
    }
}

async fn github(
    c: &GithubExtras,
    repo: &RepoRef,
    origin: &str,
    id: &Identity,
    request: &Request,
) -> Result<Page, RepoListError> {
    let mut vars =
        json!({"owner":repo.owner,"name":repo.repo,"number":id.number,"after":request.after});
    if request.section == "diff_files" {
        let n = page_number(request)?;
        if n > 150 {
            return Err(failed());
        }
        let response = c
            .review_get(&format!(
                "/repos/{}/{}/pulls/{}/files?per_page=20&page={n}",
                encoded(&repo.owner),
                encoded(&repo.repo),
                id.number
            ))
            .await
            .map_err(|_| failed())?;
        // REST and GraphQL can report different schema/version headers. The
        // API profile pins both dialects; compare identity markers only across
        // identity reads, not an unrelated REST marker.
        let (mut next, exhausted, mut omissions) = rest_next(&response, n, "diff_files")?;
        if n == 150 {
            next = None;
            if id.file_count.is_some_and(|n| n > 3000) || !exhausted {
                omissions.push("provider_file_limit".into());
            }
        }
        let mut items = vec![];
        for row in rows(&response.body, 20)? {
            let path = string(&row["filename"])?;
            let patch = row["patch"].as_str();
            if !row["patch"].is_null() && patch.is_none() {
                return Err(failed());
            }
            if patch.is_none() {
                omissions.push("provider_patch_missing".into());
            }
            let value = json!({"id":path,"new_path":path,"old_path":row["previous_filename"].as_str().unwrap_or(&path),"status":string(&row["status"] )?,"additions":row["additions"].as_u64().ok_or_else(failed)?,"deletions":row["deletions"].as_u64().ok_or_else(failed)?,"patch":patch.map(|s|prefix(s,4096)),"patch_truncated":patch.is_some_and(|p|p.len()>4096),"fragment":true});
            items.push(Item {
                id: path.clone(),
                value,
                children: patch
                    .map(|p| text_child(&format!("file:{path}"), p, "patch_cursor"))
                    .unwrap_or_default(),
            });
        }
        omissions.sort();
        omissions.dedup();
        return Ok(Page {
            identity: id.clone(),
            scope: "pull_request_files".into(),
            items,
            next,
            provider_exhausted: exhausted,
            omissions,
            input_bytes: response.bytes,
            page_version: response.version,
        });
    }
    let query = match request.section.as_str() {
        "checks" => {
            vars["head"] = json!(id.head);
            GH_CHECKS
        }
        "threads" => GH_THREADS,
        "comments" => {
            vars = json!({"id":request.parent.as_ref().ok_or_else(failed)?,"after":request.after});
            GH_COMMENTS
        }
        _ => return Err(failed()),
    };
    // GraphQL rejects unused variables. Build exact variable sets per document.
    if request.section == "checks" {
        vars = json!({"owner":repo.owner,"name":repo.repo,"head":id.head,"after":request.after});
    }
    let (data, bytes, version) =
        graphql(c.review_graphql(query, vars).await.map_err(|_| failed())?)?;
    check_version(id, &version)?;
    let (connection, scope) = match request.section.as_str() {
        "checks" => {
            let repository = &data["repository"];
            let commit = &repository["object"];
            if repository["id"].as_str() != Some(&id.repository_id)
                || commit["oid"].as_str() != Some(&id.head)
            {
                return Err(changed());
            }
            if commit["statusCheckRollup"].is_null() {
                let mut page = empty(id, "head_commit_contexts");
                page.input_bytes = bytes;
                page.page_version = version;
                return Ok(page);
            }
            (
                &commit["statusCheckRollup"]["contexts"],
                "head_commit_contexts".to_owned(),
            )
        }
        "threads" => {
            let repository = &data["repository"];
            let pr = &repository["pullRequest"];
            if repository["id"].as_str() != Some(&id.repository_id)
                || pr["number"].as_u64() != Some(id.number.into())
            {
                return Err(changed());
            }
            (&pr["reviewThreads"], "inline_review_threads".to_owned())
        }
        "comments" => {
            let thread = &data["node"];
            if thread["id"].as_str() != request.parent.as_deref()
                || thread["pullRequest"]["number"].as_u64() != Some(id.number.into())
                || thread["pullRequest"]["repository"]["id"].as_str() != Some(&id.repository_id)
            {
                return Err(changed());
            }
            (
                &thread["comments"],
                format!(
                    "inline_thread_comments:{}",
                    request.parent.as_deref().ok_or_else(failed)?
                ),
            )
        }
        _ => return Err(failed()),
    };
    let next = graph_next(connection, request)?;
    let mut items = vec![];
    for row in rows(
        &connection["nodes"],
        if request.section == "checks" { 40 } else { 20 },
    )? {
        let item_id = string(&row["id"])?;
        let (value, children) = match request.section.as_str() {
            "checks" => {
                let (name, state, url) = match row["__typename"].as_str() {
                    Some("CheckRun") => (
                        &row["name"],
                        if row["conclusion"].is_null() {
                            &row["status"]
                        } else {
                            &row["conclusion"]
                        },
                        &row["detailsUrl"],
                    ),
                    Some("StatusContext") => (&row["context"], &row["state"], &row["targetUrl"]),
                    _ => return Err(failed()),
                };
                (
                    json!({"id":item_id,"name":bounded(name,4096)?,"state":string(state)?.to_ascii_lowercase(),"url":link(url,origin)}),
                    vec![],
                )
            }
            "threads" => {
                let count = row["comments"]["totalCount"].as_u64().ok_or_else(failed)?;
                let children = if count > 0 {
                    vec![Child {
                        field: "comments_cursor".into(),
                        section: "comments".into(),
                        target: Target::Provider(Request {
                            section: "comments".into(),
                            after: None,
                            parent: Some(item_id.clone()),
                        }),
                    }]
                } else {
                    vec![]
                };
                (
                    json!({"id":item_id,"path":bounded(&row["path"],4096)?,"line":nullable_line(&row["line"])?,"resolved":row["isResolved"].as_bool().ok_or_else(failed)?,"comment_count":count,"comments":[]}),
                    children,
                )
            }
            "comments" => {
                let text = row["body"].as_str().ok_or_else(failed)?;
                (
                    json!({"id":item_id,"author":nullable_text(&row["author"]["login"],1024)?,"body":prefix(text,4096),"body_truncated":text.len()>4096,"created_at":string(&row["createdAt"] )?,"url":link(&row["url"],origin)}),
                    text_child(&format!("comment:{item_id}"), text, "body_cursor"),
                )
            }
            _ => return Err(failed()),
        };
        items.push(Item {
            id: item_id,
            value,
            children,
        });
    }
    Ok(Page {
        identity: id.clone(),
        scope,
        items,
        provider_exhausted: next.is_none(),
        next,
        omissions: vec![],
        input_bytes: bytes,
        page_version: version,
    })
}

async fn gitlab(
    c: &GitlabExtras,
    origin: &str,
    id: &Identity,
    request: &Request,
) -> Result<Page, RepoListError> {
    if request.parent.is_some() || request.section == "comments" {
        return Err(failed());
    }
    let n = page_number(request)?;
    let (path,scope,max)=match request.section.as_str(){
        "checks"=>match &id.pipeline{None=>return Ok(empty(id,"head_pipeline_jobs")),Some(pipeline)=>(format!("/projects/{}/pipelines/{pipeline}/jobs?per_page=40&page={n}&include_retried=false",id.repository_id),"head_pipeline_jobs",40)},
        "threads"=>(format!("/projects/{}/merge_requests/{}/discussions?per_page=20&page={n}",id.repository_id,id.number),"merge_request_discussions",20),
        "diff_files"=>(format!("/projects/{}/merge_requests/{}/diffs?per_page=20&page={n}",id.repository_id,id.number),"merge_request_diff_files",20),_=>return Err(failed())};
    let response = c.review_get(&path).await.map_err(|_| failed())?;
    check_version(id, &response.version)?;
    let (next, exhausted, mut omissions) = rest_next(&response, n, &request.section)?;
    let mut items = vec![];
    let mut total_notes = 0usize;
    for row in rows(&response.body, max)? {
        let item = match request.section.as_str() {
            "checks" => {
                if row["pipeline"]["id"].as_u64().map(|n| n.to_string()) != id.pipeline
                    || row["pipeline"]["sha"].as_str() != Some(&id.head)
                {
                    return Err(changed());
                }
                let item_id = numeric(&row["id"])?;
                Item {
                    id: item_id.clone(),
                    value: json!({"id":item_id,"name":bounded(&row["name"],4096)?,"state":string(&row["status"] )?,"url":link(&row["web_url"],origin)}),
                    children: vec![],
                }
            }
            "threads" => {
                let item_id = string(&row["id"])?;
                // GitLab has no declared independently paged notes endpoint.
                // Only these exact captured notes get an internal continuation.
                let notes = rows(&row["notes"], 2000)?;
                total_notes += notes.len();
                if total_notes > 2000 {
                    return Err(failed());
                }
                let mut comments = vec![];
                for note in notes {
                    let note_id = numeric(&note["id"])?;
                    let text = note["body"].as_str().ok_or_else(failed)?;
                    comments.push(Item{id:note_id.clone(),value:json!({"id":note_id,"author":nullable_text(&note["author"]["username"],1024)?,"body":prefix(text,4096),"body_truncated":text.len()>4096,"created_at":string(&note["created_at"] )?,"url":link(&note["url"],origin)}),children:text_child(&format!("note:{note_id}"),text,"body_cursor")});
                }
                let count = comments.len();
                let children = if comments.is_empty() {
                    vec![]
                } else {
                    vec![Child {
                        field: "comments_cursor".into(),
                        section: "comments".into(),
                        target: Target::Captured {
                            scope: format!("captured_discussion_notes:{item_id}"),
                            items: comments,
                        },
                    }]
                };
                Item {
                    id: item_id.clone(),
                    value: json!({"id":item_id,"path":nullable_text(&row["notes"][0]["position"]["new_path"],4096)?,"line":nullable_line(&row["notes"][0]["position"]["new_line"])?,"resolved":nullable_bool(&row["notes"][0]["resolved"])?,"comment_count":count,"comments":[]}),
                    children,
                }
            }
            "diff_files" => {
                let new_path = string(&row["new_path"])?;
                let old_path = string(&row["old_path"])?;
                let item_id =
                    serde_json::to_string(&(&old_path, &new_path)).map_err(|_| failed())?;
                let patch = row["diff"].as_str();
                if !row["diff"].is_null() && patch.is_none() {
                    return Err(failed());
                }
                if row["collapsed"] == true {
                    omissions.push("provider_diff_collapsed".into());
                }
                if row["too_large"] == true {
                    omissions.push("provider_diff_limit".into());
                }
                if patch.is_none() {
                    omissions.push("provider_patch_missing".into());
                }
                // Older versions omit limit flags. Do not infer a complete diff
                // just because pagination itself reached the final response.
                if !row["collapsed"].is_boolean() || !row["too_large"].is_boolean() {
                    omissions.push("diff_limit_metadata_unreported".into());
                }
                Item {
                    id: item_id.clone(),
                    value: json!({"id":item_id,"new_path":new_path,"old_path":old_path,"new_file":row["new_file"].as_bool().ok_or_else(failed)?,"deleted_file":row["deleted_file"].as_bool().ok_or_else(failed)?,"renamed_file":row["renamed_file"].as_bool().ok_or_else(failed)?,"patch":patch.map(|s|prefix(s,4096)),"patch_truncated":patch.is_some_and(|p|p.len()>4096),"collapsed":nullable_bool(&row["collapsed"])?,"too_large":nullable_bool(&row["too_large"])?,"fragment":true}),
                    children: patch
                        .map(|p| text_child(&format!("file:{item_id}"), p, "patch_cursor"))
                        .unwrap_or_default(),
                }
            }
            _ => return Err(failed()),
        };
        items.push(item);
    }
    omissions.sort();
    omissions.dedup();
    Ok(Page {
        identity: id.clone(),
        scope: scope.into(),
        items,
        next,
        provider_exhausted: exhausted,
        omissions,
        input_bytes: response.bytes,
        page_version: response.version,
    })
}
const GH_ID: &str = r#"query RupuReviewPageIdentity($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){id nameWithOwner pullRequest(number:$number){number headRefName headRefOid baseRefOid changedFiles headRepository{id}}}}"#;
const GH_CHECKS: &str = r#"query RupuReviewChecksPage($owner:String!,$name:String!,$head:String!,$after:String){repository(owner:$owner,name:$name){id object(expression:$head){... on Commit{oid statusCheckRollup{contexts(first:40,after:$after){pageInfo{hasNextPage endCursor} nodes{__typename ... on CheckRun{id name status conclusion detailsUrl} ... on StatusContext{id context state targetUrl}}}}}}}}"#;
const GH_THREADS: &str = r#"query RupuReviewThreadsPage($owner:String!,$name:String!,$number:Int!,$after:String){repository(owner:$owner,name:$name){id pullRequest(number:$number){number reviewThreads(first:20,after:$after){pageInfo{hasNextPage endCursor} nodes{id path line isResolved comments(first:1){totalCount}}}}}}"#;
const GH_COMMENTS: &str = r#"query RupuReviewCommentsPage($id:ID!,$after:String){node(id:$id){... on PullRequestReviewThread{id pullRequest{number repository{id}} comments(first:20,after:$after){pageInfo{hasNextPage endCursor} nodes{id author{login} body createdAt url}}}}}"#;

#[cfg(test)]
mod tests;
