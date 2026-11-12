use super::*;
use serde_json::json;
use transport::{rest_next, Session};

pub(super) const GH_ID:&str="query RupuIssueDiscussionIdentity($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){id nameWithOwner issue(number:$number){id number}}}";
pub(super) const GL_ID:&str="query RupuIssueDiscussionIdentity($path:ID!,$iid:String!){project(fullPath:$path){id fullPath issue(iid:$iid){id iid}}}";
pub(super) const LINEAR_TEAM: &str =
    "query RupuIssueDiscussionTeam($id:String!){team(id:$id){id key}}";
pub(super) const LINEAR_ID: &str =
    "query RupuIssueDiscussionIdentity($id:String!){issue(id:$id){id identifier team{id key}}}";
pub(super) const LINEAR_PAGE:&str="query RupuIssueDiscussionPage($id:String!,$after:String){issue(id:$id){id identifier team{id key} comments(first:20,after:$after,orderBy:createdAt){nodes{id body createdAt updatedAt user{name} issue{id}} pageInfo{hasNextPage endCursor}}}}";

impl Session {
    pub(crate) async fn page(
        &self,
        issue: &IssueRef,
        after: Option<&str>,
    ) -> Result<DiscussionPage, ScmError> {
        if self.provider != issue.tracker || after.is_some_and(|s| !valid(s, 2048)) {
            return Err(failed());
        }
        crate::operator_issues::validate_project(issue)?;
        self.check()?;
        let (identity, mut input_bytes) = self.proof(issue).await?;
        let (items, next, order, bytes) = match self.provider {
            IssueTracker::Github => self.github(issue, &identity, after).await?,
            IssueTracker::Gitlab => self.gitlab(issue, &identity, after).await?,
            IssueTracker::Linear => self.linear(issue, &identity, after).await?,
            IssueTracker::Jira => self.jira(issue, &identity, after).await?,
        };
        input_bytes += bytes;
        let (latest, bytes) = self.proof(issue).await?;
        input_bytes += bytes;
        if latest != identity {
            return Err(changed());
        }
        self.check()?;
        if items.len() > PAGE_SIZE
            || items
                .iter()
                .map(|i| &i.id)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != items.len()
        {
            return Err(failed());
        }
        Ok(DiscussionPage {
            identity,
            items,
            provider_exhausted: next.is_none(),
            next,
            order: order.into(),
            input_bytes,
        })
    }
    async fn proof(&self, issue: &IssueRef) -> Result<(DiscussionIdentity, usize), ScmError> {
        match self.provider {
            IssueTracker::Github => {
                let (owner, name) = issue.project.split_once('/').ok_or_else(failed)?;
                let number = i32::try_from(issue.number).map_err(|_| unsupported())?;
                let r = self
                    .graphql(
                        GH_ID,
                        json!({"owner":owner,"name":name,"number":number}),
                        MAX_IDENTITY_BYTES,
                    )
                    .await?;
                let repo = &r.value["repository"];
                unchanged(&text(&repo["nameWithOwner"], 512)?, &issue.project)?;
                let row = &repo["issue"];
                unchanged(&decimal(&row["number"])?, &issue.number.to_string())?;
                Ok((
                    self.identity(id(&repo["id"])?, id(&row["id"])?, issue.number.to_string()),
                    r.bytes,
                ))
            }
            IssueTracker::Gitlab => {
                let r = self
                    .graphql(
                        GL_ID,
                        json!({"path":issue.project,"iid":issue.number.to_string()}),
                        MAX_IDENTITY_BYTES,
                    )
                    .await?;
                let project = &r.value["project"];
                unchanged(&text(&project["fullPath"], 512)?, &issue.project)?;
                let row = &project["issue"];
                unchanged(&decimal(&row["iid"])?, &issue.number.to_string())?;
                Ok((
                    self.identity(
                        gitlab_gid(&project["id"], "Project")?,
                        gitlab_gid(&row["id"], "Issue")?,
                        issue.number.to_string(),
                    ),
                    r.bytes,
                ))
            }
            IssueTracker::Linear => {
                let team = self
                    .graphql(LINEAR_TEAM, json!({"id":issue.project}), MAX_IDENTITY_BYTES)
                    .await?;
                let row = &team.value["team"];
                let pid = id(&row["id"])?;
                let key = text(&row["key"], 128)?;
                if issue.project != pid && issue.project != key {
                    return Err(changed());
                }
                let identifier = format!("{key}-{}", issue.number);
                let r = self
                    .graphql(LINEAR_ID, json!({"id":identifier}), MAX_IDENTITY_BYTES)
                    .await?;
                let row = &r.value["issue"];
                unchanged(&text(&row["identifier"], 256)?, &identifier)?;
                unchanged(&id(&row["team"]["id"])?, &pid)?;
                unchanged(&text(&row["team"]["key"], 128)?, &key)?;
                Ok((
                    self.identity(pid, id(&row["id"])?, identifier),
                    team.bytes + r.bytes,
                ))
            }
            IssueTracker::Jira => {
                let project = issue
                    .project
                    .rsplit('/')
                    .next()
                    .filter(|s| {
                        valid(s, 128) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    })
                    .ok_or_else(failed)?;
                let key = format!("{project}-{}", issue.number);
                let r = self
                    .get(
                        &format!("/rest/api/3/issue/{key}?fields=project"),
                        MAX_IDENTITY_BYTES,
                    )
                    .await?;
                unchanged(&text(&r.value["key"], 256)?, &key)?;
                unchanged(&text(&r.value["fields"]["project"]["key"], 128)?, project)?;
                let iid = decimal(&r.value["id"])?;
                unchanged(
                    &text(&r.value["self"], 4096)?,
                    &format!("{}/rest/api/3/issue/{iid}", self.origin),
                )?;
                Ok((
                    self.identity(decimal(&r.value["fields"]["project"]["id"])?, iid, key),
                    r.bytes,
                ))
            }
        }
    }
    async fn github(
        &self,
        issue: &IssueRef,
        _identity: &DiscussionIdentity,
        after: Option<&str>,
    ) -> Result<(Vec<DiscussionComment>, Option<String>, &'static str, usize), ScmError> {
        let page = page_number(after, 1)?;
        let path = format!(
            "/repos/{}/issues/{}/comments?per_page=20&page={page}",
            issue.project, issue.number
        );
        let r = self.get(&path, MAX_PAGE_BYTES).await?;
        let rows = rows(&r.value)?;
        let expected = format!(
            "{}/repos/{}/issues/{}",
            self.origin, issue.project, issue.number
        );
        let mut items = Vec::new();
        for row in rows {
            unchanged(&text(&row["issue_url"], 4096)?, &expected)?;
            let cid = decimal(&row["id"])?;
            unchanged(
                &text(&row["url"], 4096)?,
                &format!(
                    "{}/repos/{}/issues/comments/{cid}",
                    self.origin, issue.project
                ),
            )?;
            let mut c = markdown(
                row,
                &row["user"]["login"],
                &row["created_at"],
                &row["updated_at"],
            )?;
            c.id = cid;
            items.push(c);
        }
        let next = rest_next(&r.headers, &self.origin, &path, page, items.len(), false)?;
        Ok((items, next, "oldest_first", r.bytes))
    }
    async fn gitlab(
        &self,
        issue: &IssueRef,
        identity: &DiscussionIdentity,
        after: Option<&str>,
    ) -> Result<(Vec<DiscussionComment>, Option<String>, &'static str, usize), ScmError> {
        let page = page_number(after, 1)?;
        let path = format!(
            "/projects/{}/issues/{}/notes?per_page=20&page={page}&order_by=created_at&sort=asc",
            identity.project_id, issue.number
        );
        let r = self.get(&path, MAX_PAGE_BYTES).await?;
        let mut items = Vec::new();
        for row in rows(&r.value)? {
            unchanged(&text(&row["noteable_type"], 64)?, "Issue")?;
            unchanged(&decimal(&row["noteable_id"])?, &identity.issue_id)?;
            // The project-qualified endpoint proves project + iid. GitLab versions
            // may omit these redundant fields; if returned they must agree.
            if !row["project_id"].is_null() {
                unchanged(&decimal(&row["project_id"])?, &identity.project_id)?;
            }
            if !row["noteable_iid"].is_null() {
                unchanged(&decimal(&row["noteable_iid"])?, &issue.number.to_string())?;
            }
            let system = row["system"].as_bool().ok_or_else(failed)?;
            let mut c = markdown(
                row,
                &row["author"]["username"],
                &row["created_at"],
                &row["updated_at"],
            )?;
            c.id = decimal(&row["id"])?;
            if system {
                c.kind = "system".into();
            }
            items.push(c);
        }
        let next = rest_next(&r.headers, &self.origin, &path, page, items.len(), true)?;
        Ok((items, next, "oldest_first", r.bytes))
    }
    async fn linear(
        &self,
        _issue: &IssueRef,
        identity: &DiscussionIdentity,
        after: Option<&str>,
    ) -> Result<(Vec<DiscussionComment>, Option<String>, &'static str, usize), ScmError> {
        let r = self
            .graphql(
                LINEAR_PAGE,
                json!({"id":identity.issue_id,"after":after}),
                MAX_PAGE_BYTES,
            )
            .await?;
        let issue = &r.value["issue"];
        unchanged(&id(&issue["id"])?, &identity.issue_id)?;
        unchanged(&id(&issue["team"]["id"])?, &identity.project_id)?;
        unchanged(&text(&issue["identifier"], 256)?, &identity.identifier)?;
        let connection = &issue["comments"];
        let mut items = Vec::new();
        for row in rows(&connection["nodes"])? {
            unchanged(&id(&row["issue"]["id"])?, &identity.issue_id)?;
            items.push(markdown(
                row,
                &row["user"]["name"],
                &row["createdAt"],
                &row["updatedAt"],
            )?);
        }
        let more = connection["pageInfo"]["hasNextPage"]
            .as_bool()
            .ok_or_else(failed)?;
        let next = if more {
            let next = text(&connection["pageInfo"]["endCursor"], 2048)?;
            if after == Some(next.as_str()) {
                return Err(failed());
            }
            Some(next)
        } else {
            None
        };
        Ok((items, next, "provider_created_order", r.bytes))
    }
    async fn jira(
        &self,
        _issue: &IssueRef,
        identity: &DiscussionIdentity,
        after: Option<&str>,
    ) -> Result<(Vec<DiscussionComment>, Option<String>, &'static str, usize), ScmError> {
        let offset = page_number(after, 0)?;
        let path = format!(
            "/rest/api/3/issue/{}/comment?startAt={offset}&maxResults=20&orderBy=created",
            identity.issue_id
        );
        let r = self.get(&path, MAX_PAGE_BYTES).await?;
        if r.value["startAt"].as_u64() != Some(offset as u64) {
            return Err(failed());
        }
        let maximum = r.value["maxResults"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= PAGE_SIZE as u64)
            .ok_or_else(failed)?;
        let total = r.value["total"].as_u64().ok_or_else(failed)?;
        let rows = rows(&r.value["comments"])?;
        if rows.len() as u64 > maximum {
            return Err(failed());
        }
        let end = (offset as u64)
            .checked_add(rows.len() as u64)
            .ok_or_else(failed)?;
        if end > total || (rows.is_empty() && end < total) {
            return Err(failed());
        }
        let next = if end < total {
            if end > 1_000_000 {
                return Err(failed());
            }
            Some(end.to_string())
        } else {
            None
        };
        let mut items = Vec::new();
        for row in rows {
            let cid = decimal(&row["id"])?;
            unchanged(
                &text(&row["self"], 4096)?,
                &format!(
                    "{}/rest/api/3/issue/{}/comment/{cid}",
                    self.origin, identity.issue_id
                ),
            )?;
            let (author, author_truncated) = author(&row["author"]["displayName"])?;
            // Visibility metadata is not authorization proof and is omitted by
            // this text projection; expose the omission rather than hide it.
            let metadata_truncated = author_truncated || !row["visibility"].is_null();
            let rendered = adf::render(&row["body"])?;
            items.push(DiscussionComment {
                id: cid,
                kind: "comment".into(),
                author,
                created_at: timestamp(&row["created"])?,
                updated_at: optional_timestamp(&row["updated"])?,
                parent_comment_id: None,
                body: rendered.text,
                format: "plain_text".into(),
                source_format: "adf".into(),
                projection_complete: rendered.nodes == 0 && rendered.marks == 0,
                unsupported_nodes: rendered.nodes,
                unsupported_marks: rendered.marks,
                metadata_truncated,
            });
        }
        Ok((items, next, "oldest_first", r.bytes))
    }
}
fn rows(v: &Value) -> Result<&Vec<Value>, ScmError> {
    v.as_array()
        .filter(|a| a.len() <= PAGE_SIZE)
        .ok_or_else(failed)
}
fn gitlab_gid(v: &Value, kind: &str) -> Result<String, ScmError> {
    let s = text(v, 256)?;
    let numeric = s
        .strip_prefix(&format!("gid://gitlab/{kind}/"))
        .ok_or_else(failed)?;
    decimal(&json!(numeric))
}
fn markdown(
    row: &Value,
    by: &Value,
    created: &Value,
    updated: &Value,
) -> Result<DiscussionComment, ScmError> {
    let (author, metadata_truncated) = author(by)?;
    let (body, complete) = if row["body"].is_null() {
        (String::new(), false)
    } else {
        (row["body"].as_str().ok_or_else(failed)?.to_owned(), true)
    };
    if body.len() > MAX_PAGE_BYTES {
        return Err(failed());
    }
    Ok(DiscussionComment {
        id: id(&row["id"])?,
        kind: "comment".into(),
        author,
        created_at: timestamp(created)?,
        updated_at: optional_timestamp(updated)?,
        parent_comment_id: None,
        body,
        format: "markdown".into(),
        source_format: "markdown".into(),
        projection_complete: complete,
        unsupported_nodes: 0,
        unsupported_marks: 0,
        metadata_truncated,
    })
}
