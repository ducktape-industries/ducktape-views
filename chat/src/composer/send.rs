//! What a committed draft becomes on the wire: the chat op the view submits.
use super::Send;
use crate::chat::{ChatMsg, parse_message};
use ducktape_view_guest::host::Refusal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    Post {
        channel: String,
        thread: Option<u64>,
    },
    Edit {
        channel: String,
        seq: u64,
        base_rev: u32,
    },
}

pub fn body(send: &Send) -> String {
    send.body.clone()
}

pub fn op(id: String, send: &Send, target: &Target) -> Result<ChatMsg, Refusal> {
    let body = body(send);
    if body.is_empty() || body.len() > 16 * 1024 {
        return Err(Refusal::new(
            "invalid_body",
            "Message must contain between 1 byte and 16 KiB",
        ));
    }
    let blocks = parse_message(&body);
    Ok(match target.clone() {
        Target::Post { channel, thread } => ChatMsg::PostMessage {
            channel_id: channel,
            message_id: id,
            blocks,
            thread,
        },
        Target::Edit {
            channel,
            seq,
            base_rev,
        } => ChatMsg::EditMessage {
            channel_id: channel,
            seq,
            blocks,
            base_rev: Some(base_rev),
        },
    })
}
