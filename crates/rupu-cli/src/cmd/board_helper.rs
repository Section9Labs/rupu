//! The hidden `rupu __board <id>`: what an SSH coordinator runs on a remote
//! host to read a run's board. Its stdout is a contract with `rupu-cp`'s SSH
//! connector (`rupu_cp::host::ssh::board_argv`), emitted by
//! [`emit_helper_reply`] as `rupu __message`'s is:
//!
//! - success prints one JSON object, the `BoardView`
//!   (`rupu_runtime::services::board`) of the message space `<id>` names, and
//!   exits 0;
//! - any failure prints `{"error": {"code", "message"}}`
//!   (`MessageErrorReply`; code `not_found` or `internal`) and exits 3.
//!
//! `<id>` is exact (no fragment resolution). Nothing else is ever printed on
//! stdout.

use std::path::Path;
use std::process::ExitCode;

use rupu_runtime::services::board::{board_view, BoardView};
use rupu_runtime::services::operator::{MessageError, MessageErrorCode, MessageRoot};

use crate::cmd::message::emit_helper_reply;
use crate::paths;

pub fn handle(id: &str) -> ExitCode {
    emit_helper_reply(
        paths::global_dir()
            .map_err(|e| MessageError::new(MessageErrorCode::Internal, e.to_string()))
            .and_then(|global| view(&global, id)),
    )
}

fn view(global: &Path, id: &str) -> Result<BoardView, MessageError> {
    let root = MessageRoot::resolve(global, id).map_err(|e| MessageError::from(&e))?;
    board_view(&root.space, chrono::Utc::now()).map_err(|e| {
        MessageError::new(
            MessageErrorCode::Internal,
            format!("`{id}`: the board is unreadable: {e}"),
        )
    })
}
