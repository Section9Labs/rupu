//! Operator messaging on this host (spec F1b §3.5): which endpoint an id
//! belongs to, and reading and sending through
//! `rupu_runtime::services::operator::MessageRoot`. The API's local branch
//! and [`crate::host::local::LocalHostConnector`] both come through here.

use std::path::Path;

use rupu_fleet::{Address, BusCaps, OperatorMessage, OperatorVia};
use rupu_runtime::services::operator::{
    MessageError, MessageErrorCode, MessageRoot, MessagesView, OperatorSendReply, RootKind,
    ViewQuery,
};

use crate::host::connector::{HostConnectorError, OperatorMessageRequest};

/// The family of endpoints a message space is reached through, told by the
/// id's prefix: `/api/runs` takes `run_…` (a workflow run or a standalone
/// run), `/api/sessions` takes `ses_…`, `/api/agentiflows` takes `af_…`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRoute {
    Runs,
    Sessions,
    Agentiflows,
}

/// The longest id any route takes (an agentiflow's bound,
/// `rupu_agentiflow::run::validate_run_id`).
const MAX_ID_LEN: usize = 128;

impl MessageRoute {
    pub const ALL: [MessageRoute; 3] = [
        MessageRoute::Runs,
        MessageRoute::Sessions,
        MessageRoute::Agentiflows,
    ];

    /// The route whose ids look like `id`: its prefix, then
    /// `[A-Za-z0-9_-]` only, so it is a safe path component and shell word.
    pub fn of_id(id: &str) -> Option<Self> {
        if id.len() > MAX_ID_LEN
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
        Self::ALL
            .into_iter()
            .find(|r| id.len() > r.prefix().len() && id.starts_with(r.prefix()))
    }

    fn prefix(self) -> &'static str {
        match self {
            MessageRoute::Runs => "run_",
            MessageRoute::Sessions => "ses_",
            MessageRoute::Agentiflows => "af_",
        }
    }

    /// The route's path segment: `/api/<segment>/:id/messages`.
    pub fn segment(self) -> &'static str {
        match self {
            MessageRoute::Runs => "runs",
            MessageRoute::Sessions => "sessions",
            MessageRoute::Agentiflows => "agentiflows",
        }
    }

    fn admits(self, kind: RootKind) -> bool {
        match self {
            MessageRoute::Runs => matches!(kind, RootKind::Workflow | RootKind::Standalone),
            MessageRoute::Sessions => kind == RootKind::Session,
            MessageRoute::Agentiflows => kind == RootKind::Agentiflow,
        }
    }

    /// What a wrong or unknown id is called in a 404.
    pub fn noun(self) -> &'static str {
        match self {
            MessageRoute::Runs => "run",
            MessageRoute::Sessions => "session",
            MessageRoute::Agentiflows => "agentiflow",
        }
    }

    pub fn not_found(self, id: &str) -> MessageError {
        MessageError::new(
            MessageErrorCode::NotFound,
            format!("no {} `{id}`", self.noun()),
        )
    }
}

/// The root `id` names on this host, refused unless its kind is the one its
/// prefix promises.
pub fn root(global: &Path, id: &str) -> Result<(MessageRoute, MessageRoot), MessageError> {
    let route = MessageRoute::of_id(id).ok_or_else(|| {
        MessageError::new(
            MessageErrorCode::NotFound,
            format!("no run, session or agentiflow `{id}`"),
        )
    })?;
    let root = MessageRoot::resolve(global, id).map_err(|e| MessageError::from(&e))?;
    if !route.admits(root.kind) {
        return Err(route.not_found(id));
    }
    Ok((route, root))
}

/// `id`'s messages on this host. Blocking file reads.
pub fn view(global: &Path, id: &str, q: &ViewQuery) -> Result<MessagesView, MessageError> {
    let (_, root) = root(global, id)?;
    root.view(q)
        .map_err(|e| MessageError::new(MessageErrorCode::Internal, e.to_string()))
}

/// Send `req` into `id`'s space on this host, through the CP. Blocking file
/// reads and writes.
pub fn send(
    global: &Path,
    id: &str,
    req: OperatorMessageRequest,
    caps: BusCaps,
) -> Result<OperatorSendReply, MessageError> {
    let (_, root) = root(global, id)?;
    let to = match req.to.as_deref() {
        Some(to) => Address::parse(to)
            .map_err(|e| MessageError::new(MessageErrorCode::Invalid, e.to_string()))?,
        None => root.default_recipient(),
    };
    let msg = OperatorMessage {
        to,
        body: req.body,
        urgent: req.urgent,
        via: OperatorVia::Cp,
        viewer: req.viewer,
        client: req.client,
    };
    let receipt = root
        .operator_send(global, msg, caps, &rupu_orchestrator::runs::pid_is_running)
        .map_err(|e| MessageError::from(&e))?;
    Ok(OperatorSendReply::from(&receipt))
}

/// A messaging failure as a connector error: a refusal the API answers 409
/// rides as the peer's own 409 would; `internal` says whose fault an
/// internal failure is (this host's, or a remote's).
pub fn connector_error(
    e: MessageError,
    internal: impl FnOnce(String) -> HostConnectorError,
) -> HostConnectorError {
    match e.code {
        MessageErrorCode::NotFound => HostConnectorError::NotFound(e.message),
        MessageErrorCode::Invalid => HostConnectorError::Invalid(e.message),
        MessageErrorCode::NotLive | MessageErrorCode::LogFull => {
            HostConnectorError::Remote(409, e.message)
        }
        MessageErrorCode::Internal => internal(e.message),
    }
}

/// The refusal of a transport with no way to reach a host's message spaces
/// (a tunnel node, a bucket worker): the API answers it 501.
pub fn no_message_transport(transport: &str) -> HostConnectorError {
    HostConnectorError::Unsupported(format!(
        "operator messages: a {transport} host has no messaging transport; message the run \
         with `rupu message` on that host"
    ))
}

/// The refusal of a transport with no way to read a run's board (a tunnel
/// node, a bucket worker): the API answers it 501.
pub fn no_board_transport(transport: &str) -> HostConnectorError {
    HostConnectorError::Unsupported(format!(
        "run board: a {transport} host has no board transport"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_names_its_route_by_prefix_and_nothing_unsafe_names_one() {
        let cases = [
            ("run_01J9ZQ", Some(MessageRoute::Runs)),
            ("ses_01J9ZQ", Some(MessageRoute::Sessions)),
            ("af_01J9ZQ-x", Some(MessageRoute::Agentiflows)),
            ("run_", None),
            ("sub_01J9ZQ", None),
            ("run_../x", None),
            ("run_a b", None),
            ("run_'x'", None),
            ("", None),
        ];
        for (id, want) in cases {
            assert_eq!(MessageRoute::of_id(id), want, "{id}");
        }
        assert_eq!(
            MessageRoute::of_id(&format!("af_{}", "x".repeat(200))),
            None
        );
    }

    #[test]
    fn a_root_of_another_kind_than_its_prefix_is_not_found() {
        let g = tempfile::tempdir().unwrap();
        // A session directory named like a run: `resolve` finds a session.
        let dir = g.path().join("sessions").join("run_01ODD");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), r#"{"status":"active"}"#).unwrap();
        let err = root(g.path(), "run_01ODD").unwrap_err();
        assert_eq!(err.code, MessageErrorCode::NotFound);
        assert_eq!(err.message, "no run `run_01ODD`");
    }

    #[test]
    fn errors_keep_their_status_through_a_connector() {
        let e = |code| MessageError::new(code, "why");
        let internal = |m: String| HostConnectorError::Internal(m);
        assert!(matches!(
            connector_error(e(MessageErrorCode::NotFound), internal),
            HostConnectorError::NotFound(m) if m == "why"
        ));
        assert!(matches!(
            connector_error(e(MessageErrorCode::Invalid), internal),
            HostConnectorError::Invalid(_)
        ));
        for code in [MessageErrorCode::NotLive, MessageErrorCode::LogFull] {
            assert!(matches!(
                connector_error(e(code), internal),
                HostConnectorError::Remote(409, m) if m == "why"
            ));
        }
        assert!(matches!(
            connector_error(e(MessageErrorCode::Internal), |m| {
                HostConnectorError::Remote(500, m)
            }),
            HostConnectorError::Remote(500, _)
        ));
    }
}
