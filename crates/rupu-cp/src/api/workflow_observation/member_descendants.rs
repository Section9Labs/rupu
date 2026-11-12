//! Bounded sealed ancestry proof. Catalog rows never become caller path authority.
use super::*;
use tokio_util::sync::CancellationToken;
const MAX_DEPTH: usize = 16;
const MAX_PROLOGUE: usize = 64 * 1024;
struct Work<'a> {
    cancel: &'a CancellationToken,
    start: Instant,
}
impl Work<'_> {
    fn check(&self) -> ApiResult<()> {
        check_cancel(self.cancel)?;
        if self.start.elapsed() > Duration::from_secs(5) {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "descendant_proof_deadline".into(),
            ));
        }
        Ok(())
    }
}
pub(super) fn descendant(
    s: &AppState,
    root: &Authority,
    hop: Hop,
    cancel: &CancellationToken,
) -> ApiResult<VerifiedMember> {
    if !hop.prologue_sha256.is_empty()
        || !hop.parent_file_epoch.is_empty()
        || root.ancestry.len() >= MAX_DEPTH
    {
        return Err(ApiError::conflict(
            "descendant_depth_or_initial_proof_invalid",
        ));
    }
    let mut authority = root.clone();
    authority.ancestry.push(hop);
    verify_chain(s, authority, true, cancel)
}
pub(super) fn reauthorize(
    s: &AppState,
    authority: &Authority,
    cancel: &CancellationToken,
) -> ApiResult<VerifiedMember> {
    verify_chain(s, authority.clone(), false, cancel)
}
fn verify_chain(
    s: &AppState,
    mut authority: Authority,
    establish_last: bool,
    cancel: &CancellationToken,
) -> ApiResult<VerifiedMember> {
    let work = Work {
        cancel,
        start: Instant::now(),
    };
    work.check()?;
    if authority.ancestry.len() > MAX_DEPTH || authority.prologue_sha256.len() != 64 {
        return Err(ApiError::conflict("invalid member ancestry"));
    }
    if authority.child_run_id == authority.workflow_run_id {
        return Err(ApiError::conflict("descendant_root_identity_cycle"));
    }
    let mut ids = std::collections::BTreeSet::from([
        authority.workflow_run_id.clone(),
        authority.child_run_id.clone(),
    ]);
    let mut parent = authority.child_run_id.as_str();
    for (index, hop) in authority.ancestry.iter().enumerate() {
        work.check()?;
        if hop.parent_run_id != parent
            || !ids.insert(hop.child_run_id.clone())
            || (!(establish_last && index + 1 == authority.ancestry.len())
                && hop.prologue_sha256.len() != 64)
        {
            return Err(ApiError::conflict("descendant_ancestry_mismatch"));
        }
        super::super::super::runs::validate_id(&hop.child_run_id)?;
        parent = &hop.child_run_id;
    }
    let mut root_authority = authority.clone();
    root_authority.ancestry.clear();
    let mut member = verify(s, root_authority, Some(&authority.prologue_sha256))?;
    work.check()?;
    if authority.ancestry.is_empty() {
        return Ok(member);
    }
    let global = s
        .global_dir
        .canonicalize()
        .map_err(|_| ApiError::conflict("global transcript root unavailable"))?;
    let scope = super::super::descendants::scope(s, &authority)?;
    for index in 0..authority.ancestry.len() {
        work.check()?;
        let mut hop = authority.ancestry[index].clone();
        let parent_epoch = file_epoch(&member.file)?;
        if establish_last && index + 1 == authority.ancestry.len() {
            hop.parent_file_epoch = parent_epoch;
            authority.ancestry[index].parent_file_epoch = hop.parent_file_epoch.clone();
        } else if parent_epoch != hop.parent_file_epoch {
            return Err(ApiError::conflict("descendant_ancestor_file_replaced"));
        }
        super::super::descendants::validate_edge_scoped(&scope, &hop, cancel)?;
        work.check()?;
        let path = PathBuf::from(&hop.path);
        if hop.path.len() > 4096
            || !path.is_absolute()
            || path.extension().and_then(|s| s.to_str()) != Some("jsonl")
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(ApiError::conflict("descendant_transcript_locator_invalid"));
        }
        if !path.starts_with(&global)
            && !scope
                .registered_project_root()
                .is_some_and(|root| path.starts_with(FsPath::new(root)))
        {
            return Err(ApiError::conflict(
                "descendant_transcript_outside_registered_project",
            ));
        }
        work.check()?;
        let mut file = super::super::super::fs_open::open_anchored_regular_file(&path)
            .map_err(|_| ApiError::conflict("descendant_transcript_unavailable"))?;
        let stamp = Stamp::of(
            &file
                .metadata()
                .map_err(|_| ApiError::conflict("descendant_metadata_unavailable"))?,
        )?;
        let mut reader = BufReader::with_capacity(4096, (&mut file).take(MAX_PROLOGUE as u64));
        let mut line = Vec::new();
        reader
            .read_until(b'\n', &mut line)
            .map_err(|_| ApiError::conflict("descendant_prologue_read_failed"))?;
        drop(reader);
        work.check()?;
        if !line.ends_with(b"\n") {
            return Err(ApiError::conflict(
                "descendant_prologue_incomplete_or_oversized",
            ));
        }
        let event: rupu_transcript::Event = serde_json::from_slice(&line)
            .map_err(|_| ApiError::conflict("descendant_prologue_malformed"))?;
        let raw: Value = serde_json::from_slice(&line).expect("validated prologue JSON");
        let rupu_transcript::Event::RunStart {
            run_id,
            workspace_id,
            agent,
            provider,
            model,
            started_at,
            ..
        } = event
        else {
            return Err(ApiError::conflict(
                "descendant_first_record_not_owner_proof",
            ));
        };
        if run_id != hop.child_run_id
            || workspace_id != authority.workspace_id
            || [
                &raw["host"],
                &raw["host_id"],
                &raw["data"]["host"],
                &raw["data"]["host_id"],
            ]
            .iter()
            .any(|v| !v.is_null() && v.as_str() != Some("local"))
            || (!raw["run_id"].is_null() && raw["run_id"] != run_id)
            || (!raw["workspace_id"].is_null() && raw["workspace_id"] != workspace_id)
        {
            return Err(ApiError::conflict("descendant_prologue_owner_mismatch"));
        }
        if [&agent, &provider, &model].iter().any(|v| v.len() > 4096) {
            return Err(too_large("descendant prologue metadata limit"));
        }
        let hash = format!("{:x}", Sha256::digest(&line));
        if !(establish_last && index + 1 == authority.ancestry.len()) && hash != hop.prologue_sha256
        {
            return Err(ApiError::conflict("descendant_prologue_changed"));
        }
        work.check()?;
        super::super::descendants::validate_edge_scoped(&scope, &hop, cancel)?;
        check_stamp(&path, &file, &stamp).map_err(|error| {
            if error.0 == StatusCode::CONFLICT {
                ApiError::conflict("transcript_window_changed_during_read")
            } else {
                error
            }
        })?;
        authority.ancestry[index].prologue_sha256 = hash;
        member = VerifiedMember {
            authority: authority.clone(),
            file,
            path,
            stamp,
            prologue: json!({"run_id":run_id,"workspace_id":workspace_id,"agent":agent,"provider":provider,"model":model,"started_at":started_at}),
        };
    }
    work.check()?;
    scope.verify(s, &authority)?;
    work.check()?;
    member.authority = authority;
    Ok(member)
}

fn file_epoch(file: &File) -> ApiResult<String> {
    use std::os::unix::fs::MetadataExt;
    let m = file
        .metadata()
        .map_err(|_| ApiError::conflict("descendant_ancestor_metadata_unavailable"))?;
    let created = m
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos());
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(m.dev(), m.ino(), created)).expect("file epoch JSON"))
    ))
}
