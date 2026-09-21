//! Chat: channels, direct messages, threads and the room's huddle, on the
//! view-guest `View` shape. State here, handlers as closures, module data
//! read through the module's wire (`wire`, a local copy of the slice this
//! view speaks) and folded to rows at render time.
mod api;
mod chat;
mod client;
mod composer;
mod message;
mod ui;

use std::collections::BTreeMap;

use chat::{ChannelInfo, ChatMsg, ChatViewQuery, ChatViewReply, MemberRow, MsgRow, PostPolicy};
use client::{NameDirectory, dm_channel_id, mention_token};
use ducktape_view_guest::host::{Refusal, malformed};
use ducktape_view_guest::view::{
    Cx, Effect, Live, Loaded, Query, Submit, View, ViewOf, Watching, ask,
};
use ducktape_view_guest::{export_view, wire};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use api::{ChatApi, Id, Identity, Props, Session};
use composer::send::Target;
use composer::{Draft, Event, Outcome};

const PAGE: usize = 64;
const WINDOW: usize = 256;

#[derive(Serialize, Deserialize, Default)]
pub struct Chat {
    session: Session,
    #[serde(skip)]
    names: Loaded<NameDirectory>,
    channels: Loaded<Vec<ChannelInfo>>,
    room: Option<Room>,
    drafts: BTreeMap<String, Draft>,
    channel_name: String,
    dm_peer: String,
    notice: String,
    #[serde(skip)]
    props: Option<Watching>,
    #[serde(skip)]
    live: Option<Watching>,
}

#[derive(Serialize, Deserialize, Default)]
struct Room {
    id: String,
    messages: Loaded<Vec<MsgRow>>,
    members: Loaded<Vec<MemberRow>>,
    thread: Option<u64>,
    replies: Loaded<Vec<MsgRow>>,
}

impl View for Chat {
    const PREFERRED_WINDOW_SIZE: &'static str = "1180x760";

    fn boot(cx: &mut Cx<Self>) -> Self {
        let mut chat = Self::default();
        chat.restored(cx);
        chat
    }

    fn restored(&mut self, cx: &mut Cx<Self>) {
        // a send that was in flight when the snapshot was taken never came
        // back: park its body as a failed send the composer can restore.
        for draft in self.drafts.values_mut() {
            draft.retire_device_requests();
        }
        self.props = Some(cx.watch::<Props>((), |chat, item, cx| {
            if let Ok(next) = item
                && !next.me.is_empty()
            {
                chat.session_changed(next, cx);
            }
        }));
        self.live = Some(cx.watch::<Live>("chat".into(), |chat, _, cx| chat.refresh(cx)));
        if self.names.is_idle() {
            self.names = cx.load(roster(), |chat| &mut chat.names);
        }
        if self.channels.is_idle() {
            self.channels = cx.load(channels(), |chat| &mut chat.channels);
        }
        if let Some(room) = &self.room {
            let (id, thread) = (room.id.clone(), room.thread);
            self.open(id, cx);
            if let Some(root) = thread {
                self.open_thread(root, cx);
            }
        }
    }

    fn render(&mut self, cx: &mut Cx<Self>) -> wire::Node {
        ui::render(self, cx)
    }
}

impl Chat {
    fn session_changed(&mut self, next: Session, cx: &mut Cx<Self>) {
        let prev = std::mem::replace(&mut self.session, next);
        wire::kit::set_dark(self.session.dark);
        if self.session.names_serial != prev.names_serial || self.session.me != prev.me {
            self.names = cx.load(roster(), |chat| &mut chat.names);
        }
        let steered = self.session.active_channel != prev.active_channel
            || self.session.land_seq != prev.land_seq;
        if steered && !self.session.active_channel.is_empty() {
            self.open(self.session.active_channel.clone(), cx);
        }
        if self.session.dm_serial != prev.dm_serial && !self.session.dm_peer.is_empty() {
            self.dm_peer = self.session.dm_peer.clone();
            self.open_dm(cx);
        }
    }

    /// Every state change of the chat module: re-read what is on screen,
    /// keeping the rows already there until the fresh ones land.
    fn refresh(&mut self, cx: &mut Cx<Self>) {
        refresh_into(cx, channels(), |chat| &mut chat.channels);
        let Some(room) = &self.room else { return };
        let (id, viewer) = (room.id.clone(), self.viewer());
        refresh_into(
            cx,
            messages(id.clone(), viewer.clone(), self.session.land_seq),
            room_slot,
        );
        refresh_into(cx, members(id.clone()), |chat| {
            &mut room_slot_of(chat).members
        });
        if let Some(root) = room.thread {
            refresh_into(cx, thread(id, root, viewer), |chat| {
                &mut room_slot_of(chat).replies
            });
        }
    }

    fn open(&mut self, id: String, cx: &mut Cx<Self>) {
        let viewer = self.viewer();
        let land = self.session.land_seq;
        let same = self.room.as_ref().is_some_and(|room| room.id == id);
        let room = self.room.get_or_insert_default();
        if !same {
            *room = Room {
                id: id.clone(),
                ..Room::default()
            };
        }
        room.messages = cx.load(messages(id.clone(), viewer, land), room_slot);
        room.members = cx.load(members(id), |chat| &mut room_slot_of(chat).members);
    }

    fn open_thread(&mut self, root: u64, cx: &mut Cx<Self>) {
        let viewer = self.viewer();
        let Some(room) = &mut self.room else { return };
        room.thread = Some(root);
        room.replies = cx.load(thread(room.id.clone(), root, viewer), |chat| {
            &mut room_slot_of(chat).replies
        });
    }

    fn close_thread(&mut self) {
        if let Some(room) = &mut self.room {
            room.thread = None;
            room.replies = Loaded::Idle;
        }
    }

    fn viewer(&self) -> Vec<String> {
        let me = &self.session.me;
        if me.is_empty() {
            Vec::new()
        } else {
            vec![me.clone()]
        }
    }

    fn me_key(&self) -> Vec<u8> {
        let hex = &self.session.me_key;
        (0..hex.len() / 2)
            .filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
            .collect()
    }

    fn mention_choices(&self) -> Vec<composer::MentionChoice> {
        let Some(names) = self.names.ready() else {
            return Vec::new();
        };
        let members: Vec<_> = self
            .room
            .as_ref()
            .and_then(|room| room.members.ready())
            .map(|rows| rows.iter().map(|row| ui::member(row, names)).collect())
            .unwrap_or_default();
        client::mention_choices(names, &members)
            .into_iter()
            .map(|choice| composer::MentionChoice {
                token: mention_token(&choice.party),
                label: choice.label,
            })
            .collect()
    }

    fn submit(&mut self, op: ChatMsg, cx: &mut Cx<Self>) {
        cx.spawn(async move {
            let result = ask::<Submit<ChatApi>>(op).await;
            move |chat: &mut Chat, cx: &mut Cx<Chat>| match result {
                Ok(_) => chat.refresh(cx),
                Err(refusal) => chat.notice = refusal.sentence,
            }
        });
    }

    fn create_channel(&mut self, voice: bool, cx: &mut Cx<Self>) {
        let name = self.channel_name.trim().to_string();
        if name.is_empty() || name.len() > 128 {
            self.notice = "Enter a channel name of at most 128 bytes".into();
            return;
        }
        self.channel_name.clear();
        cx.spawn(async move {
            let result = async {
                let channel_id = ask::<Id>("channel").await?;
                let op = if voice {
                    ChatMsg::CreateVoiceChannel {
                        channel_id: channel_id.clone(),
                        name,
                    }
                } else {
                    ChatMsg::CreateChannel {
                        channel_id: channel_id.clone(),
                        name,
                        post_policy: PostPolicy::Open,
                    }
                };
                ask::<Submit<ChatApi>>(op).await?;
                Ok::<_, Refusal>(channel_id)
            }
            .await;
            move |chat: &mut Chat, cx: &mut Cx<Chat>| match result {
                Ok(id) => chat.open(id, cx),
                Err(refusal) => chat.notice = refusal.sentence,
            }
        });
    }

    fn open_dm(&mut self, cx: &mut Cx<Self>) {
        let Some(mine) = self.session.me.strip_prefix("acct:") else {
            self.notice = "This key is on no account; a DM needs one".into();
            return;
        };
        let Ok(peer) = self.dm_peer.trim().parse::<u64>() else {
            self.notice = "A DM peer is an account number".into();
            return;
        };
        let name = self.names.ready().map_or_else(
            || format!("acct:{peer}"),
            |names| names.member_label(&format!("acct:{peer}")),
        );
        let id = dm_channel_id(mine, &peer.to_string());
        self.dm_peer.clear();
        cx.spawn(async move {
            let result = async {
                let existing = ask::<ViewOf<ChatApi>>(ChatViewQuery::Channel {
                    channel_id: id.clone(),
                })
                .await?;
                if !matches!(existing, ChatViewReply::Channel(Some(_))) {
                    ask::<Submit<ChatApi>>(ChatMsg::CreateDmChannel {
                        counterpart: peer,
                        name,
                    })
                    .await?;
                }
                Ok::<_, Refusal>(id)
            }
            .await;
            move |chat: &mut Chat, cx: &mut Cx<Chat>| match result {
                Ok(id) => chat.open(id, cx),
                Err(refusal) => chat.notice = refusal.sentence,
            }
        });
    }

    /// The composer's events for one target, run through its draft.
    fn composer(&mut self, target: Target, event: Event<Effect<Self>>, cx: &mut Cx<Self>) {
        let choices = self.mention_choices();
        let key = draft_key(&target);
        let draft = self.drafts.entry(key.clone()).or_default();
        match draft.handle(event, &choices) {
            Outcome::Updated => {}
            Outcome::Message(effect) => effect.run(self, cx),
            Outcome::Enqueue(tag) => cx.widget(wire::WidgetCommand::EditorAction {
                target: format!("{key}/editor"),
                tag,
            }),
            Outcome::Action(tag) => match tag.as_str() {
                "send" => {
                    if let Some(send) = draft.submitted.take() {
                        draft.in_flight.push(send.clone());
                        self.send(key, send, target, cx);
                    }
                }
                "restore" => {
                    if let Some(send) = draft.failed_send.take() {
                        draft.seed(&send.body, &choices);
                    }
                }
                _ => {}
            },
        }
    }

    fn send(&mut self, key: String, send: composer::Send, target: Target, cx: &mut Cx<Self>) {
        cx.spawn(async move {
            let result = async {
                let id = ask::<Id>("message").await?;
                let op = composer::send::op(id, &send, &target)?;
                ask::<Submit<ChatApi>>(op).await.map(|_| ())
            }
            .await;
            move |chat: &mut Chat, cx: &mut Cx<Chat>| {
                let draft = chat.drafts.entry(key).or_default();
                draft.complete_send(&send);
                match result {
                    Ok(()) => chat.refresh(cx),
                    Err(refusal) => {
                        draft.note = refusal.sentence;
                        draft.failed(send);
                    }
                }
            }
        });
    }
}

fn draft_key(target: &Target) -> String {
    match target {
        Target::Post {
            channel,
            thread: None,
        } => format!("draft-{channel}"),
        Target::Post {
            channel,
            thread: Some(root),
        } => format!("draft-{channel}-{root}"),
        Target::Edit { channel, seq, .. } => format!("edit-{channel}-{seq}"),
    }
}

fn room_slot_of(chat: &mut Chat) -> &mut Room {
    chat.room.get_or_insert_default()
}

fn room_slot(chat: &mut Chat) -> &mut Loaded<Vec<MsgRow>> {
    &mut room_slot_of(chat).messages
}

/// A re-read that leaves the slot's rows in place until fresh ones land.
fn refresh_into<T: 'static>(
    cx: &mut Cx<Chat>,
    work: impl Future<Output = Result<T, Refusal>> + 'static,
    at: impl Fn(&mut Chat) -> &mut Loaded<T> + 'static,
) {
    cx.spawn(async move {
        let result = work.await;
        move |chat: &mut Chat, _: &mut Cx<Chat>| {
            if let Ok(value) = result {
                *at(chat) = Loaded::Ready(value);
            }
        }
    });
}

fn wrong_reply() -> Refusal {
    malformed("the chat module answered another question".into())
}

async fn channels() -> Result<Vec<ChannelInfo>, Refusal> {
    let mut all = Vec::new();
    let mut after = None;
    loop {
        let ChatViewReply::Channels {
            channels,
            has_more,
            next_after,
        } = ask::<ViewOf<ChatApi>>(ChatViewQuery::Channels {
            after,
            limit: Some(PAGE),
        })
        .await?
        else {
            return Err(wrong_reply());
        };
        all.extend(channels);
        if !has_more || next_after.is_none() {
            return Ok(all);
        }
        after = next_after;
    }
}

/// The room's window: the newest roots up to `WINDOW`, or the rows around
/// a landing seq, oldest first either way.
async fn messages(
    channel_id: String,
    viewer: Vec<String>,
    land: i64,
) -> Result<Vec<MsgRow>, Refusal> {
    if land > 0 {
        let ChatViewReply::Messages(rows) = ask::<ViewOf<ChatApi>>(ChatViewQuery::MessagesAround {
            channel_id,
            seq: land as u64,
            viewer_handles: viewer,
            limit: Some(WINDOW / 2),
        })
        .await?
        else {
            return Err(wrong_reply());
        };
        return Ok(sorted(rows));
    }
    let mut all = Vec::new();
    let mut before_seq = None;
    loop {
        let ChatViewReply::Roots {
            roots,
            has_more,
            next_before_seq,
        } = ask::<ViewOf<ChatApi>>(ChatViewQuery::Roots {
            channel_id: channel_id.clone(),
            viewer_handles: viewer.clone(),
            before_seq,
            limit: Some(PAGE),
        })
        .await?
        else {
            return Err(wrong_reply());
        };
        all.extend(roots);
        if !has_more || next_before_seq.is_none() || all.len() >= WINDOW {
            return Ok(sorted(all));
        }
        before_seq = next_before_seq;
    }
}

fn sorted(mut rows: Vec<MsgRow>) -> Vec<MsgRow> {
    rows.sort_by_key(|row| row.seq);
    rows
}

async fn members(channel_id: String) -> Result<Vec<MemberRow>, Refusal> {
    match ask::<ViewOf<ChatApi>>(ChatViewQuery::Members {
        channel_id,
        after: None,
        limit: Some(WINDOW),
    })
    .await?
    {
        ChatViewReply::Members { members, .. } => Ok(members),
        _ => Err(wrong_reply()),
    }
}

async fn thread(
    channel_id: String,
    root_seq: u64,
    viewer: Vec<String>,
) -> Result<Vec<MsgRow>, Refusal> {
    match ask::<ViewOf<ChatApi>>(ChatViewQuery::Thread {
        channel_id,
        root_seq,
        viewer_handles: viewer,
        after_reply_seq: None,
        limit: Some(WINDOW),
    })
    .await?
    {
        ChatViewReply::Thread { replies, .. } => Ok(sorted(replies)),
        _ => Err(wrong_reply()),
    }
}

/// The identity roster, paged, folded into the name directory.
async fn roster() -> Result<NameDirectory, Refusal> {
    let mut accounts = Vec::new();
    let mut from = 0u64;
    loop {
        let reply: Value =
            ask::<Query<Identity>>(json!({"all": {"from": from, "limit": PAGE}})).await?;
        let page = reply
            .get("accounts")
            .and_then(Value::as_array)
            .ok_or_else(wrong_reply)?;
        for account in page {
            let number = account["number"].as_u64().ok_or_else(wrong_reply)?;
            let keys = account["keys"]
                .as_array()
                .map(|keys| {
                    keys.iter()
                        .filter_map(|key| serde_json::from_value(key["pubkey"].clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            accounts.push((
                number,
                account["name"].as_str().unwrap_or_default().to_string(),
                account["control"].get("program").is_some(),
                keys,
            ));
            from = number + 1;
        }
        if page.len() < PAGE {
            return Ok(NameDirectory::from_roster(accounts));
        }
    }
}

export_view!(
    Chat,
    "Chat",
    "Channels, direct messages, threads and the live call of this workspace.",
    ["chat"]
);

#[cfg(test)]
mod tests;
