use super::*;
use crate::issue_discussion::Session;
use serde_json::json;

const LINEAR_TEAM: &str = "query RupuIssueDiscoveryTeam($id:String!){team(id:$id){id key}}";
const LINEAR_PAGE:&str="query RupuIssueDiscovery($id:String!,$after:String,$filter:IssueFilter){team(id:$id){id key issues(first:25,after:$after,orderBy:updatedAt,filter:$filter){nodes{identifier title updatedAt url team{id key} state{type}} pageInfo{hasNextPage endCursor}}}}";

impl Session {
    pub(crate) async fn discover(
        &self,
        request: &DiscoveryRequest,
    ) -> Result<DiscoveryPage, ScmError> {
        validate(request, self.provider)?;
        self.check()?;
        let (provider_project_id, (rows, next, end_observed, truncated)) = match self.provider {
            IssueTracker::Github => self.discover_github(request).await?,
            IssueTracker::Gitlab => self.discover_gitlab(request).await?,
            IssueTracker::Jira => self.discover_jira(request).await?,
            IssueTracker::Linear => self.discover_linear(request).await?,
        };
        self.check()?;
        if rows.len() > PAGE_SIZE
            || rows
                .iter()
                .map(|r| &r.number)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != rows.len()
        {
            return Err(failed());
        }
        Ok(DiscoveryPage {
            project: request.project.clone(),
            provider_project_id,
            connector_epoch: self.epoch.clone(),
            rows,
            next,
            end_observed,
            truncated,
            search_mode: if request.title_query.is_some() {
                "provider_title_query"
            } else {
                "none"
            }
            .into(),
        })
    }
    async fn discover_github(&self, r: &DiscoveryRequest) -> Result<(String, PageParts), ScmError> {
        let page = page(r.after.as_deref())?;
        let path = format!("/repos/{}", r.project);
        let proof = self.get(&path, MAX_PAGE_BYTES).await?;
        let pid = github_project_id(&proof.value, r, &self.origin)?;
        let url = query(
            &format!("{path}/issues"),
            &[
                ("per_page", "25".into()),
                ("page", page.to_string()),
                ("state", rest_state(r.state, false).into()),
                ("sort", "updated".into()),
                ("direction", "desc".into()),
            ],
        );
        let response = self.get(&url, MAX_PAGE_BYTES).await?;
        let parsed = parse_github(&response.value, r, &self.origin, page)?;
        Ok((
            pid,
            header_continuation(
                parsed,
                &response.headers,
                &self.origin,
                &url,
                page,
                rows(&response.value)?.len(),
            )?,
        ))
    }
    async fn discover_gitlab(&self, r: &DiscoveryRequest) -> Result<(String, PageParts), ScmError> {
        let page = page(r.after.as_deref())?;
        let project = r.project.replace('/', "%2F");
        let path = format!("/projects/{project}");
        let proof = self.get(&path, MAX_PAGE_BYTES).await?;
        same(&proof.value["path_with_namespace"], &r.project)?;
        let pid = decimal(&proof.value["id"])?;
        let mut pairs = vec![
            ("per_page", "25".into()),
            ("page", page.to_string()),
            ("state", rest_state(r.state, true).into()),
            ("scope", "all".into()),
            ("order_by", "updated_at".into()),
            ("sort", "desc".into()),
        ];
        if let Some(q) = &r.title_query {
            pairs.push(("search", q.clone()));
            pairs.push(("in", "title".into()));
        }
        let url = query(&format!("/projects/{pid}/issues"), &pairs);
        let response = self.get(&url, MAX_PAGE_BYTES).await?;
        let parsed = parse_gitlab(&response.value, r, &pid, page)?;
        let parsed = header_continuation(
            parsed,
            &response.headers,
            &self.origin,
            &url,
            page,
            rows(&response.value)?.len(),
        )?;
        Ok((pid, parsed))
    }
    async fn discover_jira(&self, r: &DiscoveryRequest) -> Result<(String, PageParts), ScmError> {
        let key = r
            .project
            .rsplit('/')
            .next()
            .filter(|s| valid(s, 128) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .ok_or_else(invalid)?;
        let proof = self
            .get(&format!("/rest/api/3/project/{key}"), MAX_PAGE_BYTES)
            .await?;
        same(&proof.value["key"], key)?;
        let pid = decimal(&proof.value["id"])?;
        same(
            &proof.value["self"],
            &format!("{}/rest/api/3/project/{pid}", self.origin),
        )?;
        // key is validated above; query language never contains title input.
        let filter = match r.state {
            DiscoveryState::Open => " AND statusCategory != Done",
            DiscoveryState::Closed => " AND statusCategory = Done",
            DiscoveryState::All => "",
        };
        let mut pairs = vec![
            (
                "jql",
                format!("project = \"{key}\"{filter} ORDER BY updated DESC"),
            ),
            ("fields", "summary,status,project,updated".into()),
            ("maxResults", "25".into()),
        ];
        if let Some(after) = &r.after {
            pairs.push(("nextPageToken", after.clone()));
        }
        let response = self
            .get(&query("/rest/api/3/search/jql", &pairs), MAX_PAGE_BYTES)
            .await?;
        let parsed = parse_jira(&response.value, r, &self.origin, &pid, key)?;
        Ok((pid, parsed))
    }
    async fn discover_linear(&self, r: &DiscoveryRequest) -> Result<(String, PageParts), ScmError> {
        let proof = self
            .graphql(LINEAR_TEAM, json!({"id":r.project}), MAX_PAGE_BYTES)
            .await?;
        let pid = text(&proof.value["team"]["id"], 256)?;
        if !pid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(failed());
        }
        let key = text(&proof.value["team"]["key"], 128)?;
        if r.project != pid && r.project != key {
            return Err(failed());
        }
        let mut filter = json!({});
        match r.state {
            DiscoveryState::Open => {
                filter["state"] = json!({"type":{"in":["triage","backlog","unstarted","started"]}})
            }
            DiscoveryState::Closed => {
                filter["state"] = json!({"type":{"in":["completed","canceled"]}})
            }
            DiscoveryState::All => {}
        }
        if let Some(q) = &r.title_query {
            filter["title"] = json!({"contains":q});
        }
        let response = self
            .graphql(
                LINEAR_PAGE,
                json!({"id":pid,"after":r.after,"filter":filter}),
                MAX_PAGE_BYTES,
            )
            .await?;
        let parsed = parse_linear(&response.value, r, &pid, &key)?;
        Ok((pid, parsed))
    }
}
type PageParts = (Vec<DiscoveryRow>, Option<String>, bool, bool);
fn rest_state(state: DiscoveryState, gitlab: bool) -> &'static str {
    match state {
        DiscoveryState::Open => {
            if gitlab {
                "opened"
            } else {
                "open"
            }
        }
        DiscoveryState::Closed => "closed",
        DiscoveryState::All => "all",
    }
}
pub(super) fn parse_github(
    value: &Value,
    r: &DiscoveryRequest,
    origin: &str,
    page: u32,
) -> Result<PageParts, ScmError> {
    let input = rows(value)?;
    let mut output = Vec::new();
    for row in input {
        let number = decimal(&row["number"])?;
        same(
            &row["repository_url"],
            &format!("{origin}/repos/{}", r.project),
        )?;
        same(
            &row["url"],
            &format!("{origin}/repos/{}/issues/{number}", r.project),
        )?;
        // The REST issue list includes PRs. Their identities must also pass
        // before exclusion; continuation is based on raw provider rows.
        if let Some(pr) = row.get("pull_request") {
            if !pr.is_object() {
                return Err(failed());
            }
            continue;
        }
        let state = match row["state"].as_str() {
            Some("open") => "open",
            Some("closed") => "closed",
            _ => return Err(failed()),
        };
        selected(r, state)?;
        output.push(summary(
            number,
            &row["title"],
            state,
            &row["updated_at"],
            &row["html_url"],
        )?);
    }
    let (next, end, truncated) = rest_continuation(page, input.len());
    Ok((output, next, end, truncated))
}
pub(super) fn parse_gitlab(
    value: &Value,
    r: &DiscoveryRequest,
    pid: &str,
    page: u32,
) -> Result<PageParts, ScmError> {
    let input = rows(value)?;
    let mut output = Vec::new();
    for row in input {
        if decimal(&row["project_id"])? != pid {
            return Err(failed());
        }
        let state = match row["state"].as_str() {
            Some("opened") => "open",
            Some("closed") => "closed",
            _ => return Err(failed()),
        };
        selected(r, state)?;
        output.push(summary(
            decimal(&row["iid"])?,
            &row["title"],
            state,
            &row["updated_at"],
            &row["web_url"],
        )?);
    }
    let (next, end, truncated) = rest_continuation(page, input.len());
    Ok((output, next, end, truncated))
}
pub(super) fn parse_jira(
    value: &Value,
    r: &DiscoveryRequest,
    origin: &str,
    pid: &str,
    key: &str,
) -> Result<PageParts, ScmError> {
    let input = rows(&value["issues"])?;
    let mut output = Vec::new();
    for row in input {
        let fields = &row["fields"];
        same(&fields["project"]["key"], key)?;
        if decimal(&fields["project"]["id"])? != pid {
            return Err(failed());
        }
        let identifier = text(&row["key"], 256)?;
        let number = identifier
            .strip_prefix(&format!("{key}-"))
            .ok_or_else(failed)?;
        let number = decimal(&Value::String(number.into()))?;
        let iid = decimal(&row["id"])?;
        same(&row["self"], &format!("{origin}/rest/api/3/issue/{iid}"))?;
        let state = match fields["status"]["statusCategory"]["key"].as_str() {
            Some("new" | "indeterminate") => "open",
            Some("done") => "closed",
            _ => return Err(failed()),
        };
        selected(r, state)?;
        output.push(summary(
            number,
            &fields["summary"],
            state,
            &fields["updated"],
            &Value::String(format!("{origin}/browse/{identifier}")),
        )?);
    }
    let end = value["isLast"].as_bool().ok_or_else(failed)?;
    let next = opaque(&value["nextPageToken"], !end, r.after.as_deref())?;
    Ok((output, next, end, false))
}
pub(super) fn parse_linear(
    value: &Value,
    r: &DiscoveryRequest,
    pid: &str,
    key: &str,
) -> Result<PageParts, ScmError> {
    let team = &value["team"];
    same(&team["id"], pid)?;
    same(&team["key"], key)?;
    let connection = &team["issues"];
    let input = rows(&connection["nodes"])?;
    let mut output = Vec::new();
    for row in input {
        same(&row["team"]["id"], pid)?;
        same(&row["team"]["key"], key)?;
        let identifier = text(&row["identifier"], 256)?;
        let number = identifier
            .strip_prefix(&format!("{key}-"))
            .ok_or_else(failed)?;
        let number = decimal(&Value::String(number.into()))?;
        let state = match row["state"]["type"].as_str() {
            Some("triage" | "backlog" | "unstarted" | "started") => "open",
            Some("completed" | "canceled") => "closed",
            _ => return Err(failed()),
        };
        selected(r, state)?;
        output.push(summary(
            number,
            &row["title"],
            state,
            &row["updatedAt"],
            &row["url"],
        )?);
    }
    let more = connection["pageInfo"]["hasNextPage"]
        .as_bool()
        .ok_or_else(failed)?;
    let next = opaque(
        &connection["pageInfo"]["endCursor"],
        more,
        r.after.as_deref(),
    )?;
    Ok((output, next, !more, false))
}

/// Reuse the reviewed header parser, mapping our page-size fullness to its
/// fixed discussion page size. Provider URLs are validated, never followed.
pub(super) fn header_continuation(
    mut result: PageParts,
    headers: &reqwest::header::HeaderMap,
    origin: &str,
    path: &str,
    current: u32,
    raw_count: usize,
) -> Result<PageParts, ScmError> {
    let fullness = if raw_count == PAGE_SIZE {
        crate::issue_discussion::PAGE_SIZE
    } else {
        0
    };
    let next = crate::issue_discussion::rest_next(headers, origin, path, current, fullness, false)?;
    if next
        .as_deref()
        .is_some_and(|s| s.parse::<u32>().is_ok_and(|n| n > 10_000))
    {
        result.1 = None;
        result.2 = false;
        result.3 = true;
    } else {
        result.2 = next.is_none();
        result.1 = next;
        result.3 = false;
    }
    Ok(result)
}

pub(super) fn github_project_id(
    value: &Value,
    request: &DiscoveryRequest,
    origin: &str,
) -> Result<String, ScmError> {
    same(&value["full_name"], &request.project)?;
    same(
        &value["url"],
        &format!("{origin}/repos/{}", request.project),
    )?;
    decimal(&value["id"])
}
