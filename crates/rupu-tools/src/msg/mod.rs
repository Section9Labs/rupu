//! `msg.send`: a message to one participant's inbox, or to the flow's
//! broadcast log, from the run's [`Bus`](rupu_fleet::Bus) participant.

use crate::board::{bus, now};
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::output::{failed, ok, req_str};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_fleet::FleetMessage;
use serde_json::Value;

/// The `msg.send` recipient that fans out to every participant.
const BROADCAST: &str = "broadcast";

pub static SEND: ToolDescriptor = ToolDescriptor {
    name: "msg.send",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    uses: &[],
    wildcard: true,
    description: "Send a message. `to` is a participant id, a role, \"parent\", or \
     \"lead\" (delivered to exactly that inbox), or \"broadcast\" (every \
     participant sees it once, from the moment it started listening; the \
     broadcast log is capped per run, so don't spam it).",
    input_schema: msg_send_schema,
};

fn msg_send_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["to", "body"],
        "properties": {
            "to": {
                "type": "string",
                "description": "Recipient inbox: participant id, role, parent, lead or broadcast"
            },
            "body": { "type": "string" }
        }
    })
}

/// `msg.send { to, body }` -> appends to the recipient's inbox, or to the
/// shared broadcast log when `to` is `"broadcast"`.
pub struct MsgSendTool;

#[async_trait]
impl Tool for MsgSendTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &SEND
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let to = req_str(&input, "to")?;
        let body = req_str(&input, "body")?.to_string();
        let bus = bus(ctx)?;
        let msg = FleetMessage {
            from: bus.participant.clone(),
            ts: now(),
            body,
        };
        // "broadcast" is not an inbox: it is an append-only log each reader
        // walks with its own cursor, so one reader cannot consume it for the
        // rest.
        let sent = if to == BROADCAST {
            bus.mailbox.broadcast_send(&msg, bus.caps.msg_cap)
        } else {
            bus.mailbox.send(to, &msg, bus.caps.msg_cap)
        };
        Ok(match sent {
            Ok(()) => ok(format!("sent to {to}")),
            Err(e) => failed(format!("msg.send failed: {e}")),
        })
    }
}
