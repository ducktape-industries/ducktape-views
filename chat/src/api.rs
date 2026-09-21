//! What this view says to the host: the chat module's types, the identity
//! roster, the host's props stream and the intents the host acts on.
use crate::chat::{ChatMsg, ChatViewQuery, ChatViewReply};
use ducktape_view_guest::host::{Refusal, malformed};
use ducktape_view_guest::view::{Capability, Module};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub struct ChatApi;
impl Module for ChatApi {
    const NAME: &'static str = "chat";
    type Op = ChatMsg;
    type Query = Value;
    type Reply = Value;
    type ViewQuery = ChatViewQuery;
    type ViewReply = ChatViewReply;
}

/// The name directory's source; spoken as plain JSON so this view links no
/// identity crate.
pub struct Identity;
impl Module for Identity {
    const NAME: &'static str = "identity";
    type Op = Value;
    type Query = Value;
    type Reply = Value;
    type ViewQuery = Value;
    type ViewReply = Value;
}

/// `host.id`: a fresh id of the named kind.
pub struct Id;
impl Capability for Id {
    const KIND: &'static str = "host.id";
    type Request = &'static str;
    type Reply = String;
    fn encode(kind: &&'static str) -> Vec<u8> {
        kind.as_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<String, Refusal> {
        String::from_utf8(bytes.to_vec()).map_err(|error| malformed(error.to_string()))
    }
}

/// The host's session facts, pushed on `chat.props`. An item that carries
/// only a `background` participation decodes to defaults and is skipped by
/// the view (`me` empty).
pub struct Props;
impl Capability for Props {
    const KIND: &'static str = "chat.props";
    type Request = ();
    type Reply = Session;
    fn encode(_: &()) -> Vec<u8> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub dark: bool,
    pub connected: bool,
    pub network_name: String,
    /// this view's own network, `<label>#<salt>`, what its links carry
    pub chain: String,
    pub status: String,
    pub block_height: i64,
    /// the reader's rendered handle (`acct:7` / `user:<hex>`) and key hex
    pub me: String,
    pub me_key: String,
    pub names_serial: i64,
    /// steered by `duck://` links, notifications and the tray
    pub active_channel: String,
    pub dm_peer: String,
    pub dm_serial: i64,
    /// the seq a landing asks the room to open around; 0 is the live tail
    pub land_seq: i64,
    pub busy: bool,
    pub huddle_joined: bool,
    pub huddle_channel: String,
    pub huddle_channel_name: String,
    pub call_muted: bool,
    pub call_speaking: bool,
    pub call_peers: Vec<CallPeer>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CallPeer {
    pub peer: String,
    pub muted: bool,
    pub speaking: bool,
}

macro_rules! intent {
    ($name:ident, $kind:literal, $request:ty) => {
        pub struct $name;
        impl Capability for $name {
            const KIND: &'static str = $kind;
            type Request = $request;
            type Reply = ();
        }
    };
}
intent!(ShowHuddle, "chat.show_huddle", ());
intent!(LeaveHuddle, "chat.leave_huddle", ());
intent!(JoinHuddle, "chat.join_huddle", ());
intent!(JoinVoice, "chat.join_voice", Value);
