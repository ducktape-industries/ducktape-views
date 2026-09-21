//! The frame: rooms on the left, the open room and its composer in the
//! middle, the open thread on the right.
use crate::chat::{ChannelInfo, ChatMsg, MemberRow, MsgRow};
use crate::client::{
    ChatMember, ChatMessage, ChatReader, NameDirectory, chat_message, mark_message_groups,
};
use ducktape_view_guest::view::{Cx, Effect, Loaded};
use ducktape_view_guest::wire::{ButtonPreset, Length, Node, kit, kit::Tone};

use crate::Chat;
use crate::api::{JoinHuddle, JoinVoice, LeaveHuddle, ShowHuddle};
use crate::composer::{self, send::Target};

pub fn render(chat: &mut Chat, cx: &mut Cx<Chat>) -> Node {
    let mut panes = vec![kit::pane("sidebar", sidebar(chat, cx), Length::Fixed(240.))];
    match &chat.room {
        Some(room) => {
            let room_id = room.id.clone();
            panes.push(kit::sized(
                room_view(chat, &room_id, cx),
                Some(Length::Fill),
                Some(Length::Fill),
            ));
            if let Some(root) = chat.room.as_ref().and_then(|room| room.thread) {
                panes.push(kit::pane(
                    "thread",
                    thread_view(chat, &room_id, root, cx),
                    Length::Fixed(360.),
                ));
            }
        }
        None => panes.push(kit::empty_state(
            "no-room",
            "Pick a room",
            "Channels and direct messages are on the left.",
        )),
    }
    let mut body = vec![kit::sized(
        kit::row("panes", panes),
        Some(Length::Fill),
        Some(Length::Fill),
    )];
    if !chat.notice.is_empty() {
        let dismiss = cx.on(|chat, _| chat.notice.clear());
        body.insert(
            0,
            kit::notice(
                "notice",
                kit::row(
                    "notice/row",
                    [
                        kit::text("notice/text", chat.notice.clone()),
                        kit::spacer(),
                        kit::button("dismiss", "Dismiss", Some(dismiss), ButtonPreset::Subtle),
                    ],
                ),
                Tone::Warning,
            ),
        );
    }
    kit::page("chat", body)
}

fn sidebar(chat: &mut Chat, cx: &mut Cx<Chat>) -> Node {
    let mut items = vec![kit::heading(
        "rooms/title",
        chat.session.network_name.clone(),
    )];
    match &chat.channels {
        Loaded::Ready(channels) => {
            let open = chat.room.as_ref().map(|room| room.id.clone());
            for info in channels.iter().filter(|info| !info.channel.archived) {
                let id = info.channel.id.clone();
                let chosen = open.as_deref() == Some(id.as_str());
                let press = cx.on(move |chat, cx| chat.open(id.clone(), cx));
                let mut cells = vec![kit::text(
                    format!("room/{}/name", info.channel.id),
                    room_name(info),
                )];
                if !info.channel.huddle.is_empty() {
                    cells.push(kit::spacer());
                    cells.push(kit::badge(
                        format!("room/{}/huddle", info.channel.id),
                        format!("🎙 {}", info.channel.huddle.len()),
                        Tone::Success,
                    ));
                }
                items.push(kit::list_row(
                    format!("room/{}", info.channel.id),
                    kit::row(format!("room/{}/row", info.channel.id), cells),
                    chosen,
                    Some(press),
                ));
            }
        }
        Loaded::Failed(refusal) => {
            items.push(kit::caption("rooms/failed", refusal.sentence.clone()))
        }
        _ => items.push(kit::caption("rooms/loading", "Loading rooms…")),
    }
    items.push(kit::spacer());
    let typed = cx.on_value(|chat, name: String, _| chat.channel_name = name);
    let create = cx.on(|chat, cx| chat.create_channel(false, cx));
    let voice = cx.on(|chat, cx| chat.create_channel(true, cx));
    items.push(kit::field(
        "new-channel",
        "New channel",
        kit::column(
            "new-channel/body",
            [
                kit::input(
                    "channel-name",
                    "channel name",
                    chat.channel_name.clone(),
                    typed,
                    Some(create),
                ),
                kit::row(
                    "new-channel/actions",
                    [
                        kit::button(
                            "create-channel",
                            "Create",
                            Some(create),
                            ButtonPreset::Primary,
                        ),
                        kit::button(
                            "create-voice",
                            "Voice",
                            Some(voice),
                            ButtonPreset::Secondary,
                        ),
                    ],
                ),
            ],
        ),
    ));
    let typed = cx.on_value(|chat, peer: String, _| chat.dm_peer = peer);
    let open = cx.on(|chat, cx| chat.open_dm(cx));
    items.push(kit::field(
        "new-dm",
        "Direct message",
        kit::row(
            "new-dm/body",
            [
                kit::input(
                    "dm-peer",
                    "account number",
                    chat.dm_peer.clone(),
                    typed,
                    Some(open),
                ),
                kit::button("open-dm", "Open", Some(open), ButtonPreset::Secondary),
            ],
        ),
    ));
    kit::spaced(
        kit::sized(
            kit::column("rooms", items),
            Some(Length::Fill),
            Some(Length::Fill),
        ),
        6.,
    )
}

fn room_name(info: &ChannelInfo) -> String {
    if info.channel.name.is_empty() {
        kit::short_id(&info.channel.id, 8)
    } else {
        info.channel.name.clone()
    }
}

fn room_view(chat: &mut Chat, room_id: &str, cx: &mut Cx<Chat>) -> Node {
    let info = chat
        .channels
        .ready()
        .and_then(|channels| channels.iter().find(|info| info.channel.id == room_id))
        .cloned();
    let title = info
        .as_ref()
        .map_or_else(|| kit::short_id(room_id, 8), room_name);
    let members = chat
        .room
        .as_ref()
        .and_then(|room| room.members.ready())
        .map_or(0, Vec::len);
    let mut header = vec![
        kit::heading("room/title", title),
        kit::caption("room/members", format!("{members} members")),
        kit::spacer(),
    ];
    if let Some(info) = &info {
        header.extend(huddle_controls(chat, info, cx));
    }
    let rows = chat.room.as_ref().map(|room| &room.messages);
    let timeline = match rows {
        Some(Loaded::Ready(rows)) => {
            let messages = fold(chat, rows);
            if messages.is_empty() {
                kit::empty_state("room/empty", "Nothing here yet", "Say something below.")
            } else {
                let nodes: Vec<Node> = messages
                    .iter()
                    .map(|(message, mine)| message_view(message, *mine, room_id, true, cx))
                    .collect();
                kit::scroll(
                    "room/scroll",
                    kit::spaced(kit::column("room/messages", nodes), 8.),
                )
            }
        }
        Some(Loaded::Failed(refusal)) => kit::notice(
            "room/failed",
            kit::text("room/failed/text", refusal.sentence.clone()),
            Tone::Danger,
        ),
        _ => kit::caption("room/loading", "Loading messages…"),
    };
    let target = Target::Post {
        channel: room_id.to_string(),
        thread: None,
    };
    kit::spaced(
        kit::column(
            "room",
            [
                kit::centered_row("room/header", header),
                kit::divider("room/rule"),
                kit::sized(timeline, Some(Length::Fill), Some(Length::Fill)),
                composer_view(chat, target, "Message the room", cx),
            ],
        ),
        8.,
    )
}

fn huddle_controls(chat: &Chat, info: &ChannelInfo, cx: &mut Cx<Chat>) -> Vec<Node> {
    let session = &chat.session;
    let here = session.huddle_joined && session.huddle_channel == info.channel.id;
    let mut controls = Vec::new();
    for (index, seat) in info.channel.huddle.iter().enumerate() {
        let label = chat.names.ready().map_or_else(
            || seat.party.clone(),
            |names| names.member_label(&seat.party),
        );
        controls.push(kit::avatar(
            format!("seat/{index}"),
            kit::initials(&label),
            Tone::Accent,
        ));
    }
    if here {
        let show = cx.on(|_, cx| cx.notify::<ShowHuddle>(()));
        let leave = cx.on(|_, cx| cx.notify::<LeaveHuddle>(()));
        controls.push(kit::button(
            "show-huddle",
            "Huddle",
            Some(show),
            ButtonPreset::Secondary,
        ));
        controls.push(kit::button(
            "leave-huddle",
            "Leave",
            Some(leave),
            ButtonPreset::Danger,
        ));
    } else if info.channel.voice {
        let id = info.channel.id.clone();
        let join = cx.on(move |_, cx| cx.notify::<JoinVoice>(serde_json::json!({ "id": id })));
        controls.push(kit::button(
            "join-voice",
            "Join voice",
            Some(join),
            ButtonPreset::Primary,
        ));
    } else {
        let join = cx.on(|_, cx| cx.notify::<JoinHuddle>(()));
        let label = if info.channel.huddle.is_empty() {
            "Start huddle"
        } else {
            "Join huddle"
        };
        controls.push(kit::button(
            "join-huddle",
            label,
            Some(join),
            ButtonPreset::Secondary,
        ));
    }
    controls
}

fn thread_view(chat: &mut Chat, room_id: &str, root: u64, cx: &mut Cx<Chat>) -> Node {
    let close = cx.on(|chat, _| chat.close_thread());
    let header = kit::centered_row(
        "thread/header",
        [
            kit::heading("thread/title", "Thread"),
            kit::spacer(),
            kit::button("close-thread", "Close", Some(close), ButtonPreset::Subtle),
        ],
    );
    let root_row = chat
        .room
        .as_ref()
        .and_then(|room| room.messages.ready())
        .and_then(|rows| rows.iter().find(|row| row.seq == root).cloned());
    let mut rows: Vec<MsgRow> = root_row.into_iter().collect();
    let replies = chat.room.as_ref().map(|room| &room.replies);
    let body = match replies {
        Some(Loaded::Ready(replies)) => {
            rows.extend(replies.iter().cloned());
            let nodes: Vec<Node> = fold(chat, &rows)
                .iter()
                .map(|(message, mine)| message_view(message, *mine, room_id, false, cx))
                .collect();
            kit::scroll(
                "thread/scroll",
                kit::spaced(kit::column("thread/messages", nodes), 8.),
            )
        }
        Some(Loaded::Failed(refusal)) => kit::text("thread/failed", refusal.sentence.clone()),
        _ => kit::caption("thread/loading", "Loading thread…"),
    };
    let target = Target::Post {
        channel: room_id.to_string(),
        thread: Some(root),
    };
    kit::spaced(
        kit::column(
            "thread/column",
            [
                header,
                kit::sized(body, Some(Length::Fill), Some(Length::Fill)),
                composer_view(chat, target, "Reply in thread", cx),
            ],
        ),
        8.,
    )
}

fn composer_view(chat: &mut Chat, target: Target, hint: &str, cx: &mut Cx<Chat>) -> Node {
    let key = crate::draft_key(&target);
    let choices = chat.mention_choices();
    let draft = chat.drafts.entry(key.clone()).or_default();
    let editable = chat.session.connected && !chat.session.me.is_empty();
    let _ = cx;
    composer::view(draft, &key, &key, hint, editable, &choices, move |event| {
        let target = target.clone();
        Effect::once(move |chat: &mut Chat, cx: &mut Cx<Chat>| chat.composer(target, event, cx))
    })
}

/// Rows to messages, named by the directory the reader has.
fn fold(chat: &Chat, rows: &[MsgRow]) -> Vec<(ChatMessage, bool)> {
    let empty = NameDirectory::empty();
    let names = chat.names.ready().unwrap_or(&empty);
    let key = chat.me_key();
    let reader = ChatReader::new((!key.is_empty()).then_some(key.as_slice()), names);
    let mine: Vec<bool> = rows.iter().map(|row| reader.is_me(&row.author)).collect();
    let mut messages: Vec<ChatMessage> = rows
        .iter()
        .cloned()
        .map(|row| chat_message(row, names))
        .collect();
    mark_message_groups(&mut messages);
    messages.into_iter().zip(mine).collect()
}

pub fn member(row: &MemberRow, names: &NameDirectory) -> ChatMember {
    ChatMember {
        key: row.party.clone(),
        label: names.member_label(&row.party),
    }
}

fn message_view(
    message: &ChatMessage,
    mine: bool,
    room_id: &str,
    in_room: bool,
    cx: &mut Cx<Chat>,
) -> Node {
    let pane = if in_room { "room" } else { "thread" };
    let k = |part: &str| format!("msg/{pane}/{}/{part}", message.seq);
    let mut lines = Vec::new();
    if message.show_author {
        lines.push(kit::row(
            k("head"),
            [
                kit::strong(k("author"), message.author.clone()),
                kit::gap(6.),
                kit::caption(k("meta"), message.meta.clone()),
            ],
        ));
    }
    for (index, block) in message.blocks.iter().enumerate() {
        let key = k(&format!("block/{index}"));
        lines.push(match block.kind.as_str() {
            "code" => kit::card(format!("{key}/card"), kit::mono(key, block.text.clone())),
            "quote" => kit::secondary(key, block.text.clone()),
            "divider" => kit::divider(key),
            _ => kit::text(key, block.text.clone()),
        });
    }
    let mut actions = Vec::new();
    let (channel, seq) = (room_id.to_string(), message.seq);
    for (index, reaction) in message.reactions.iter().enumerate() {
        let (channel, emoji, added) = (
            channel.clone(),
            reaction.emoji.clone(),
            !reaction.reacted_by_me,
        );
        let press = cx.on(move |chat, cx| {
            let (channel_id, emoji) = (channel.clone(), emoji.clone());
            let op = if added {
                ChatMsg::AddReaction {
                    channel_id,
                    seq,
                    emoji,
                }
            } else {
                ChatMsg::RemoveReaction {
                    channel_id,
                    seq,
                    emoji,
                }
            };
            chat.submit(op, cx);
        });
        let tone = if reaction.reacted_by_me {
            Tone::Accent
        } else {
            Tone::Neutral
        };
        let badge = kit::badge(
            k(&format!("reaction/{index}/badge")),
            format!("{} {}", reaction.emoji, reaction.count),
            tone,
        );
        actions.push(kit::list_row(
            k(&format!("reaction/{index}")),
            badge,
            reaction.reacted_by_me,
            Some(press),
        ));
    }
    if !message.deleted {
        let thumbs = channel.clone();
        let react = cx.on(move |chat, cx| {
            chat.submit(
                ChatMsg::AddReaction {
                    channel_id: thumbs.clone(),
                    seq,
                    emoji: "👍".into(),
                },
                cx,
            )
        });
        actions.push(kit::button(
            k("react"),
            "👍",
            Some(react),
            ButtonPreset::Subtle,
        ));
        if in_room {
            let reply = cx.on(move |chat, cx| chat.open_thread(seq, cx));
            let label = if message.reply_count > 0 {
                format!("{} replies", message.reply_count)
            } else {
                "Reply".to_string()
            };
            actions.push(kit::button(
                k("reply"),
                label,
                Some(reply),
                ButtonPreset::Subtle,
            ));
        }
        if mine {
            let channel = channel.clone();
            let delete = cx.on(move |chat, cx| {
                chat.submit(
                    ChatMsg::DeleteMessage {
                        channel_id: channel.clone(),
                        seq,
                    },
                    cx,
                )
            });
            actions.push(kit::button(
                k("delete"),
                "Delete",
                Some(delete),
                ButtonPreset::Subtle,
            ));
        }
    }
    lines.push(kit::wrapped_row(k("actions"), actions));
    kit::spaced(kit::column(k("column"), lines), 2.)
}
