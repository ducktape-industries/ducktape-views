//! The slice of the chat module's wire this view speaks: the ops it submits,
//! the index views it asks for and the rows those return. Copied from the
//! module (crates/modules/apps/chat in ducktape-modules), fields the view
//! reads only — serde skips the rest. `tests` drives the view over fixtures
//! in these shapes.
use serde::{Deserialize, Serialize};

pub use crate::message::{Block, Mark, Party, Span, parse_message};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PostPolicy {
    Open,
    MembersOnly,
}

/// The ops this view submits (`op.submit`, target `chat`).
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatMsg {
    CreateChannel {
        channel_id: String,
        name: String,
        post_policy: PostPolicy,
    },
    CreateVoiceChannel {
        channel_id: String,
        name: String,
    },
    CreateDmChannel {
        counterpart: u64,
        name: String,
    },
    PostMessage {
        channel_id: String,
        message_id: String,
        blocks: Vec<Block>,
        thread: Option<u64>,
    },
    EditMessage {
        channel_id: String,
        seq: u64,
        blocks: Vec<Block>,
        base_rev: Option<u32>,
    },
    DeleteMessage {
        channel_id: String,
        seq: u64,
    },
    AddReaction {
        channel_id: String,
        seq: u64,
        emoji: String,
    },
    RemoveReaction {
        channel_id: String,
        seq: u64,
        emoji: String,
    },
}

/// One message head as the index serves it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MsgRow {
    pub seq: u64,
    /// `user:{hex}`, `acct:{n}`, `module:{id}` or `system`.
    pub author: String,
    pub blocks: Vec<Block>,
    pub deleted: bool,
    pub rev: u32,
    pub thread: Option<u64>,
    pub reply_count: u64,
    pub reactions: Vec<ReactionSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReactionSummary {
    pub emoji: String,
    pub count: u64,
    pub reacted_by_me: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelRow {
    pub id: String,
    pub name: String,
    pub archived: bool,
    pub huddle: Vec<HuddleEntry>,
    pub voice: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HuddleEntry {
    pub party: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberRow {
    pub party: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelInfo {
    #[serde(flatten)]
    pub channel: ChannelRow,
    pub head_seq: u64,
}

/// The index views this view asks for (`rpc.view`, target `chat`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatViewQuery {
    Channels {
        after: Option<String>,
        limit: Option<usize>,
    },
    Channel {
        channel_id: String,
    },
    Roots {
        channel_id: String,
        viewer_handles: Vec<String>,
        before_seq: Option<u64>,
        limit: Option<usize>,
    },
    MessagesAround {
        channel_id: String,
        seq: u64,
        viewer_handles: Vec<String>,
        limit: Option<usize>,
    },
    Thread {
        channel_id: String,
        root_seq: u64,
        viewer_handles: Vec<String>,
        after_reply_seq: Option<u64>,
        limit: Option<usize>,
    },
    Members {
        channel_id: String,
        after: Option<String>,
        limit: Option<usize>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatViewReply {
    Channels {
        channels: Vec<ChannelInfo>,
        has_more: bool,
        #[serde(default)]
        next_after: Option<String>,
    },
    Channel(Option<ChannelInfo>),
    Roots {
        roots: Vec<MsgRow>,
        has_more: bool,
        #[serde(default)]
        next_before_seq: Option<u64>,
    },
    Messages(Vec<MsgRow>),
    Thread {
        replies: Vec<MsgRow>,
    },
    Members {
        members: Vec<MemberRow>,
    },
}

/// The handle the index stamps for a party — how every row names an actor.
pub fn party_handle(party: &Party) -> String {
    match party {
        Party::Account(account) => format!("acct:{account}"),
        Party::Key(key) => format!("user:{}", user_handle(key)),
        Party::Module(module) => format!("module:{module}"),
        Party::System => "system".to_string(),
    }
}

/// A key as the index renders it: printable bytes verbatim, anything else hex.
fn user_handle(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.is_empty() && !text.chars().any(char::is_control) => text.to_string(),
        _ => hex(bytes),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// An even-length all-hex string back to its bytes; anything else is not hex.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty()
        || !text.len().is_multiple_of(2)
        || !text.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}
