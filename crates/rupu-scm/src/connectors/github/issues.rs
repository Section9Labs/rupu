//! GitHub IssueConnector. Implementation lands in Task 12.

use async_trait::async_trait;

use crate::connectors::IssueConnector;
use crate::error::ScmError;
use crate::platform::IssueTracker;
use crate::types::{Comment, CreateIssue, Issue, IssueFilter, IssueRef, IssueState};

use super::client::GithubClient;

/// Render octocrab's `AuthorAssociation` as the same string GitHub's API
/// wire format uses (e.g. `"OWNER"`, `"CONTRIBUTOR"`). The enum's variants
/// are `#[serde(rename_all = "SCREAMING_SNAKE_CASE")]` and its catch-all
/// `Other(String)` case is `#[serde(untagged)]`, so round-tripping through
/// `serde_json` reproduces GitHub's raw value for every variant —
/// including any association github adds in the future — without a
/// hand-maintained match arm per variant.
fn author_association_str(assoc: &octocrab::models::AuthorAssociation) -> Option<String> {
    serde_json::to_value(assoc)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
}

pub struct GithubIssueConnector {
    client: GithubClient,
}

impl GithubIssueConnector {
    pub fn new(client: GithubClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl IssueConnector for GithubIssueConnector {
    fn tracker(&self) -> IssueTracker {
        IssueTracker::Github
    }

    fn forge_actions(&self) -> Vec<crate::forge_write::ForgeAction> {
        use crate::forge_write::ForgeAction::*;
        vec![
            ReviewComment,
            ReviewCreate,
            BranchCreate,
            WorkflowDispatch,
            IssueComment,
            IssueCreate,
            IssueSetState,
        ]
    }
    async fn forge_session(
        &self,
    ) -> Result<crate::forge_write::ForgeSession, crate::forge_write::ForgeError> {
        self.client.forge_session()
    }

    fn discussion_supported(&self) -> bool {
        self.operator_read_capabilities().origin.is_some()
    }
    fn discussion_epoch(&self) -> Option<String> {
        self.discussion_supported()
            .then(|| self.client.review_identity_epoch())
    }
    async fn discussion_page(
        &self,
        issue: &IssueRef,
        after: Option<&str>,
    ) -> Result<crate::issue_discussion::DiscussionPage, ScmError> {
        self.operator_expected_issue(issue)?;
        self.client.discussion_session()?.page(issue, after).await
    }

    fn discovery_supported(&self) -> bool {
        self.discussion_supported()
    }
    fn discovery_search_supported(&self) -> bool {
        false
    }
    fn discovery_epoch(&self) -> Option<String> {
        self.discussion_epoch()
    }
    async fn discovery_page(
        &self,
        request: &crate::issue_discovery::DiscoveryRequest,
    ) -> Result<crate::issue_discovery::DiscoveryPage, ScmError> {
        crate::issue_discovery::validate(request, self.tracker())?;
        // Synthetic number validates project/account issuer only; no issue 1 read.
        let expected = self.operator_expected_issue(&IssueRef {
            tracker: self.tracker(),
            project: request.project.clone(),
            number: 1,
        })?;
        let mut request = request.clone();
        request.project = expected.project;
        self.client.discussion_session()?.discover(&request).await
    }

    fn operator_read_capabilities(&self) -> crate::operator_issues::IssueReadCapabilities {
        crate::operator_issues::IssueReadCapabilities::configured(
            self.client.operator_read_origin(),
            true,
        )
    }

    async fn operator_get_issue(&self, issue: &IssueRef) -> Result<Issue, ScmError> {
        self.operator_expected_issue(issue)?;
        let _permit = self.client.permit().await;
        let path = format!("/repos/{}/issues/{}", issue.project, issue.number);
        let value = self.client.operator_json(&path).await?;
        let origin = self
            .client
            .operator_read_origin()
            .ok_or_else(crate::operator_issues::rejected)?;
        if value["url"].as_str() != Some(format!("{origin}{path}").as_str())
            || value["repository_url"].as_str()
                != Some(format!("{origin}/repos/{}", issue.project).as_str())
            || value["number"].as_u64() != Some(issue.number)
        {
            return Err(crate::operator_issues::rejected());
        }
        let model =
            serde_json::from_value(value).map_err(|_| crate::operator_issues::rejected())?;
        Ok(issue_from_octocrab(issue.project.clone(), model))
    }

    async fn operator_comments_page(
        &self,
        issue: &IssueRef,
        page: u32,
    ) -> Result<crate::operator_issues::IssueCommentPage, ScmError> {
        self.operator_expected_issue(issue)?;
        if page == 0 || page > 10000 {
            return Err(crate::operator_issues::rejected());
        }
        let _permit = self.client.permit().await;
        let value = self
            .client
            .operator_json(&format!(
                "/repos/{}/issues/{}/comments?per_page=50&page={page}",
                issue.project, issue.number
            ))
            .await?;
        let rows = value
            .as_array()
            .ok_or_else(crate::operator_issues::rejected)?;
        if rows.len() > 50 {
            return Err(crate::operator_issues::rejected());
        }
        let origin = self
            .client
            .operator_read_origin()
            .ok_or_else(crate::operator_issues::rejected)?;
        let expected_issue = format!("{origin}/repos/{}/issues/{}", issue.project, issue.number);
        let mut items = Vec::new();
        for row in rows {
            if row["issue_url"].as_str() != Some(expected_issue.as_str()) {
                return Err(crate::operator_issues::rejected());
            }
            let id = row["id"]
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(crate::operator_issues::rejected)?
                .to_string();
            items.push(Comment {
                id,
                author: row["user"]["login"]
                    .as_str()
                    .ok_or_else(crate::operator_issues::rejected)?
                    .into(),
                body: row["body"].as_str().unwrap_or_default().into(),
                created_at: serde_json::from_value(row["created_at"].clone())
                    .map_err(|_| crate::operator_issues::rejected())?,
                author_association: row["author_association"].as_str().map(str::to_string),
            });
        }
        let full = items.len() == 50;
        Ok(crate::operator_issues::IssueCommentPage {
            items,
            next_page: (full && page < 10000).then_some(page + 1),
            end_observed: !full,
        })
    }

    async fn list_issues(
        &self,
        project: &str,
        filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(project)?;
        let inner = self.client.inner.clone();
        let labels: Vec<String> = filter.labels.clone();
        let project_str = project.to_string();
        let pages = self
            .client
            .with_retry_octocrab(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let labels = labels.clone();
                async move {
                    let handler = inner.issues(&owner, &repo);
                    let mut req = handler.list();
                    if !labels.is_empty() {
                        req = req.labels(&labels);
                    }
                    req.send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(pages
            .items
            .into_iter()
            .map(|item| issue_from_octocrab(project_str.clone(), item))
            .collect())
    }

    async fn get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let project = i.project.clone();
        let inner = self.client.inner.clone();
        let model = self
            .client
            .with_retry_octocrab(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .get(number)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(issue_from_octocrab(project, model))
    }

    async fn comment_issue(&self, i: &IssueRef, body: &str) -> Result<Comment, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let inner = self.client.inner.clone();
        let body = body.to_string();
        let model = self
            .client
            .with_retry_octocrab(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let body = body.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .create_comment(number, &body)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(Comment {
            id: model.id.to_string(),
            author: model.user.login,
            body,
            created_at: model.created_at,
            // Comes free on the create-comment response model — no
            // extra API call, so there's no reason to withhold it.
            author_association: author_association_str(&model.author_association),
        })
    }

    async fn list_comments(
        &self,
        i: &IssueRef,
        limit: Option<u32>,
    ) -> Result<Vec<Comment>, ScmError> {
        if limit == Some(0) {
            return Ok(Vec::new());
        }
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let inner = self.client.inner.clone();
        // GitHub caps per_page at 100.
        let per_page: u8 = 100;
        // `None` keeps its documented meaning: a single default page,
        // never paginated further. A `limit` above one page walks
        // successive pages (oldest-first, matching GitHub's native order)
        // until `limit` is satisfied or a short page signals the last
        // page. `MAX_PAGES` bounds how far an absurd `limit` can walk so a
        // caller can't make this spin forever; 50 pages * 100/page = 5000
        // comments, comfortably above any real operator-control thread.
        const MAX_PAGES: u32 = 50;

        let mut out: Vec<Comment> = Vec::new();
        let mut page_num: u32 = 1;
        loop {
            // Acquired per page, not once for the whole walk: the GitHub
            // semaphore has only 4 permits process-wide, and a single
            // `list_comments` call can otherwise occupy one for up to
            // `MAX_PAGES` requests (each independently retriable with an
            // uncapped `Retry-After`), starving every other GitHub call.
            let _permit = self.client.permit().await;
            let page_items = self
                .client
                .with_retry_octocrab(|| {
                    let inner = inner.clone();
                    let owner = owner.clone();
                    let repo = repo.clone();
                    async move {
                        inner
                            .issues(&owner, &repo)
                            .list_comments(number)
                            .per_page(per_page)
                            .page(page_num)
                            .send()
                            .await
                            .map_err(super::client::classify_octocrab_error)
                    }
                })
                .await?;

            let fetched = page_items.items.len();
            out.extend(page_items.items.into_iter().map(|m| Comment {
                id: m.id.to_string(),
                author: m.user.login,
                // octocrab models an issue comment body as Option<String>
                // (a comment can be body-less after redaction).
                body: m.body.unwrap_or_default(),
                created_at: m.created_at,
                author_association: author_association_str(&m.author_association),
            }));

            let short_page = fetched < per_page as usize;
            let limit_satisfied = limit.is_some_and(|n| out.len() >= n as usize);
            let single_default_page = limit.is_none();
            let cap_reached = page_num >= MAX_PAGES;

            if cap_reached && !short_page {
                tracing::warn!(
                    issue = i.number,
                    project = %i.project,
                    max_pages = MAX_PAGES,
                    per_page,
                    "github: list_comments hit MAX_PAGES with a full last page; result is truncated and may be missing comments"
                );
            }

            if short_page || limit_satisfied || single_default_page || cap_reached {
                break;
            }
            page_num += 1;
        }

        if let Some(n) = limit {
            out.truncate(n as usize);
        }
        Ok(out)
    }

    async fn create_issue(&self, project: &str, opts: CreateIssue) -> Result<Issue, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(project)?;
        let inner = self.client.inner.clone();
        let project_str = project.to_string();
        let model = self
            .client
            .with_retry_octocrab(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let opts = opts.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .create(opts.title)
                        .body(opts.body)
                        .labels(opts.labels)
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(issue_from_octocrab(project_str, model))
    }

    async fn update_issue_state(&self, i: &IssueRef, state: IssueState) -> Result<(), ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let inner = self.client.inner.clone();
        self.client
            .with_retry_octocrab(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .update(number)
                        .state(match state {
                            IssueState::Open => octocrab::models::IssueState::Open,
                            IssueState::Closed => octocrab::models::IssueState::Closed,
                        })
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    Ok(())
                }
            })
            .await
    }
}

fn parse_project(project: &str) -> Result<(String, String), ScmError> {
    let (o, r) = project
        .split_once('/')
        .ok_or_else(|| ScmError::BadRequest {
            message: format!("project must be `owner/repo`: {project}"),
        })?;
    Ok((o.to_string(), r.to_string()))
}

fn issue_from_octocrab(project: String, item: octocrab::models::issues::Issue) -> Issue {
    // Walk labels once, building both the name list and the
    // name->hex map. octocrab exposes `Label.color` as a hex string
    // without `#`; persist as-is so the renderer can reuse it
    // verbatim.
    let mut labels = Vec::with_capacity(item.labels.len());
    let mut label_colors = std::collections::BTreeMap::new();
    for l in item.labels {
        if !l.color.is_empty() {
            label_colors.insert(l.name.clone(), l.color);
        }
        labels.push(l.name);
    }
    Issue {
        r: IssueRef {
            tracker: IssueTracker::Github,
            project,
            number: item.number,
        },
        title: item.title,
        body: item.body.unwrap_or_default(),
        state: match item.state {
            octocrab::models::IssueState::Open => IssueState::Open,
            _ => IssueState::Closed,
        },
        labels,
        label_colors,
        author: item.user.login,
        created_at: item.created_at,
        updated_at: item.updated_at,
    }
}
