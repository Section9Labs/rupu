//! `msg.send`: a message to a participant, a role, a step, the parent, the
//! lead or the operator, or to everyone through the broadcast log, from the
//! run's [`Bus`](rupu_fleet::Bus) participant.

use crate::board::{bus, not_sent, sent};
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::output::{deny_unknown, req_str};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_fleet::Address;
use serde_json::{json, Value};

pub static SEND: ToolDescriptor = ToolDescriptor {
    name: "msg.send",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    uses: &[],
    wildcard: true,
    description: "Send a message. `to` is a participant (lynx#1) or its full codename, \
     role:<agent> or step:<id> (every live participant running it), \"parent\", \
     \"lead\", \"operator\", or \"broadcast\" (every participant sees it once, \
     from the moment it started listening; the broadcast log is capped, so \
     don't spam it). Recipients see it before their next model call. Returns \
     the message's `id` and who it reached; `undelivered` says who it didn't, \
     and why.",
    input_schema: msg_send_schema,
};

fn msg_send_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["to", "body"],
        "properties": {
            "to": {
                "type": "string",
                "description": "Recipient: a participant, role:<agent>, step:<id>, parent, lead, operator or broadcast"
            },
            "body": { "type": "string" }
        }
    })
}

/// `msg.send { to, body }` -> `{sent, id, to, delivery}`: through the bus,
/// which logs it and delivers it to each resolved recipient's inbox (or the
/// broadcast log).
pub struct MsgSendTool;

#[async_trait]
impl Tool for MsgSendTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &SEND
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        deny_unknown(&input, &["to", "body"])?;
        let to = Address::parse(req_str(&input, "to")?)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let body = req_str(&input, "body")?.to_string();
        Ok(match bus(ctx)?.send(&to, &body) {
            Ok(r) => sent(r, json!({ "sent": true })),
            Err(e) => not_sent("msg.send", e),
        })
    }
}
