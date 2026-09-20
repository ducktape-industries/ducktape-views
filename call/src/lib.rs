//! Deployed call protocol over generic media devices and Gateway streams.
mod call_wire;
mod composer;
mod panel;
mod protocol;
mod session;

use ducktape_view_guest::{Subscription, Task, wire};

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct CallView {
    panel: Option<panel::Panel>,
    invited: std::collections::BTreeSet<String>,
    room: panel::Room,
    error: String,
    #[serde(skip)]
    invitations: std::collections::BTreeMap<String, ducktape_view_guest::task::Handle>,
}
#[derive(Clone)]
pub enum Message {
    Progress,
    Panel(Box<panel::Panel>),
    Action(panel::Action),
    InviteFinished(panel::RoomKey, String, Result<(), String>),
    RoomLoaded(panel::RoomKey, Result<panel::Room, String>),
}

impl CallView {
    const PREFERRED_WINDOW_SIZE: &'static str = "none";
    fn boot() -> (Self, Task<Message>) {
        (Self::default(), Task::none())
    }
    fn view(&self) -> wire::Node {
        self.panel.as_ref().map_or_else(wire::Node::empty, |panel| {
            panel.view(&self.room, &self.invited, &self.error)
        })
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Progress => self.on_progress(),
            Message::Panel(panel) => self.on_panel(*panel),
            Message::Action(action) => self.on_action(action),
            Message::RoomLoaded(key, result) => self.on_room_loaded(key, result),
            Message::InviteFinished(instance, key, result) => {
                self.on_invite_finished(instance, key, result)
            }
        }
    }
    fn on_progress(&mut self) -> Task<Message> {
        Task::none()
    }
    fn on_panel(&mut self, panel: panel::Panel) -> Task<Message> {
        let changed = self.panel.as_ref().is_none_or(|previous| {
            previous.room_key() != panel.room_key() || previous.joined != panel.joined
        });
        if changed {
            self.invitations.clear();
            self.invited.clear();
            self.room = panel::Room::default();
            self.error.clear();
        }
        self.panel = Some(panel);
        Task::none()
    }
    fn on_room_loaded(
        &mut self,
        key: panel::RoomKey,
        result: Result<panel::Room, String>,
    ) -> Task<Message> {
        let current = self
            .panel
            .as_ref()
            .is_some_and(|panel| panel.joined && panel.room_key() == key);
        if !current {
            return Task::none();
        }
        match result {
            Ok(room) => self.room = room,
            Err(error) => self.error = error,
        }
        Task::none()
    }
    fn on_action(&mut self, action: panel::Action) -> Task<Message> {
        match action {
            panel::Action::Mute => self.control("call.mute"),
            panel::Action::Camera => self.control("call.camera"),
            panel::Action::Screen => self.control("call.screen"),
            panel::Action::Share(index) => self.share(index),
            panel::Action::ShareCancel => self.control("call.share_cancel"),
            panel::Action::Channel => self.control("call.channel"),
            panel::Action::Leave => self.control("call.leave"),
            panel::Action::Invite(key) => self.invite(key),
        }
    }
    fn control(&self, kind: &str) -> Task<Message> {
        panel::notify(kind);
        Task::none()
    }
    /// The picked row, which the host resolves against the targets it offered.
    fn share(&self, index: usize) -> Task<Message> {
        panel::notify_with("call.share", &index.to_string());
        Task::none()
    }
    fn invite(&mut self, key: String) -> Task<Message> {
        let Some(panel) = &self.panel else {
            return Task::none();
        };
        let available = panel.joined
            && !panel.loading
            && !panel.channel.is_empty()
            && self.room.members.iter().any(|member| member.key == key);
        if !available || !self.invited.insert(key.clone()) {
            return Task::none();
        }
        self.error.clear();
        let room = panel.room_key();
        let member = key.clone();
        let (task, handle) = Task::perform(
            panel::invite(panel.channel.clone(), self.room.title.clone(), key.clone()),
            move |result| Message::InviteFinished(room.clone(), key.clone(), result),
        )
        .abortable();
        self.invitations.insert(member, handle.abort_on_drop());
        task
    }
    fn on_invite_finished(
        &mut self,
        room: panel::RoomKey,
        key: String,
        result: Result<(), String>,
    ) -> Task<Message> {
        let current = self
            .panel
            .as_ref()
            .is_some_and(|panel| panel.joined && panel.room_key() == room);
        if !current {
            return Task::none();
        }
        self.invitations.remove(&key);
        if let Err(error) = result {
            self.invited.remove(&key);
            self.error = error;
        }
        Task::none()
    }
    fn subscription(&self) -> Subscription<Message> {
        let protocol = Subscription::run(session::run);
        let Some(panel) = &self.panel else {
            return protocol;
        };
        if !panel.joined || panel.channel.is_empty() {
            return protocol;
        }
        Subscription::batch([protocol, panel::rooms(panel.room_key())])
    }
    // Protocol resources restart; panel interaction state survives replacement.
    fn snapshot(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|error| error.to_string())
    }
    fn restore(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|error| error.to_string())
    }
}

ducktape_view_guest::export_app!(
    CallView,
    "Call",
    "Background audio and video session",
    ["call", "media", "net", "rpc", "host", "clock"]
);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    #[test]
    fn panel_replacement_preserves_invites_and_rejects_an_old_rooms_reply() {
        let mut view = CallView::default();
        let _ = view.on_panel(panel::Panel {
            instance: 7,
            joined: true,
            channel: "room".into(),
            ..Default::default()
        });
        view.invited.insert("acct:8".into());
        let mut restored = CallView::restore(&view.snapshot().unwrap()).unwrap();
        let _ = restored.on_panel(panel::Panel {
            instance: 7,
            joined: true,
            channel: "room".into(),
            muted: true,
            ..Default::default()
        });
        assert!(restored.invited.contains("acct:8"));
        let _ = restored.on_panel(panel::Panel {
            instance: 8,
            joined: true,
            channel: "other".into(),
            ..Default::default()
        });
        restored.invited.insert("acct:8".into());
        let old = view.panel.as_ref().unwrap().room_key();
        let current = restored.panel.as_ref().unwrap().room_key();
        let _ = restored.on_invite_finished(old, "acct:8".into(), Err("old failure".into()));
        assert!(restored.error.is_empty());
        assert!(restored.invited.contains("acct:8"));
        let _ =
            restored.on_invite_finished(current, "acct:8".into(), Err("current failure".into()));
        assert_eq!(restored.error, "current failure");
        assert!(restored.invited.is_empty());
    }

    #[test]
    fn identity_changes_cancel_pending_invites_and_discard_the_old_rooms_state() {
        use futures::{FutureExt as _, StreamExt as _};
        let base = panel::Panel {
            instance: 7,
            channel: "room".into(),
            joined: true,
            network: "chain-a".into(),
            endpoint: "node-a".into(),
            account: "7".into(),
            user_key: "key-a".into(),
            ..Default::default()
        };
        let alternatives = [
            panel::Panel {
                network: "chain-b".into(),
                ..base.clone()
            },
            panel::Panel {
                endpoint: "node-b".into(),
                ..base.clone()
            },
            panel::Panel {
                account: "8".into(),
                ..base.clone()
            },
            panel::Panel {
                user_key: "key-b".into(),
                ..base.clone()
            },
            panel::Panel {
                joined: false,
                ..base.clone()
            },
        ];
        for changed in alternatives {
            let mut view = CallView::default();
            let _ = view.on_panel(base.clone());
            view.room.title = "old room".into();
            view.invited.insert("acct:9".into());
            let (task, handle) = Task::future(std::future::pending::<Message>()).abortable();
            view.invitations
                .insert("acct:9".into(), handle.abort_on_drop());
            let _ = view.on_panel(changed);
            assert!(view.invited.is_empty());
            assert!(view.room.title.is_empty());
            assert!(view.invitations.is_empty());
            assert!(matches!(
                task.into_stream().next().now_or_never(),
                Some(None)
            ));
            let _ = view.on_invite_finished(
                base.room_key(),
                "acct:9".into(),
                Err("old failure".into()),
            );
            let _ = view.on_room_loaded(base.room_key(), Err("old lookup".into()));
            assert!(view.error.is_empty());
        }
    }

    #[test]
    fn room_queries_belong_to_the_current_panel_instance() {
        let mut view = CallView::default();
        let old = panel::Panel {
            instance: 1,
            ..Default::default()
        }
        .room_key();
        let current = panel::Panel {
            instance: 2,
            joined: true,
            ..Default::default()
        };
        let key = current.room_key();
        let _ = view.on_panel(current);
        let _ = view.on_room_loaded(old, Err("old room".into()));
        assert!(view.error.is_empty());
        let _ = view.on_room_loaded(
            key,
            Ok(panel::Room {
                title: "current".into(),
                ..Default::default()
            }),
        );
        assert_eq!(view.room.title, "current");
    }

    #[test]
    fn panel_properties_do_not_start_devices_or_transport() {
        let mut host = Host::new();
        host.step(Vec::new());
        host.item("call.props", json!({"panel": {"title": "Engineering"}}));
        assert_eq!(
            host.streams.keys().map(String::as_str).collect::<Vec<_>>(),
            ["call.props"]
        );
        assert!(host.effects.is_empty());
        host.item(
            "call.props",
            json!({"panel": {"title": "Other room", "muted": true}}),
        );
        assert_eq!(host.streams.len(), 1);
        assert!(host.effects.is_empty());
    }

    struct Host {
        guest: ducktape_view_guest::Driver<CallView>,
        streams: BTreeMap<String, u64>,
        effects: Vec<(String, Value)>,
        /// What a congested writer does to a control frame: the host's stream
        /// send queue is one frame deep and refuses rather than blocking.
        refuse_beacon: bool,
        /// The hub's reason token when the huddle is refused at the join.
        refuse_hub: Option<&'static str>,
        /// The room's committed huddle roster, as node keys.
        roster: Vec<String>,
    }

    impl Host {
        fn new() -> Self {
            Self {
                guest: ducktape_view_guest::Driver::new(),
                streams: BTreeMap::new(),
                effects: Vec::new(),
                refuse_beacon: false,
                refuse_hub: None,
                roster: Vec::new(),
            }
        }
        fn response(id: u64, value: Value, done: bool) -> wire::Event {
            wire::Event::Response {
                id,
                result: Ok(serde_json::to_vec(&value).unwrap()),
                done,
            }
        }
        fn step(&mut self, mut events: Vec<wire::Event>) {
            loop {
                let frame = self.guest.tick(events);
                events = Vec::new();
                for request in frame.requests {
                    match request.kind.as_str() {
                        "call.props" | "media.audio" | "media.video" | "clock.ticks"
                        | "rpc.live" => {
                            self.streams.insert(request.kind, request.id);
                        }
                        "rpc.view" => {
                            let query: Value = serde_json::from_slice(&request.payload).unwrap();
                            assert_eq!(query["target"], "chat");
                            assert_eq!(query["query"]["channel"]["channel_id"], "room");
                            let seats: Vec<Value> = self
                                .roster
                                .iter()
                                .map(|node| json!({"party": "acct:1", "node": node}))
                                .collect();
                            events.push(Self::response(
                                request.id,
                                json!({"channel": {"name": "room", "huddle": seats}}),
                                true,
                            ));
                        }
                        "voice.hub" => {
                            let hub: Value = serde_json::from_slice(&request.payload).unwrap();
                            assert_eq!(hub, json!({"channel": "room"}));
                            self.streams.insert(request.kind, request.id);
                            if let Some(reason) = self.refuse_hub {
                                events.push(wire::Event::Response {
                                    id: request.id,
                                    result: Err(wire::Refusal::new(reason, "the hub's own words")),
                                    done: true,
                                });
                                continue;
                            }
                            events.push(Self::response(
                                request.id,
                                json!({"text": r#"{"type":"ready"}"#}),
                                false,
                            ));
                        }
                        "media.image" => events.push(Self::response(
                            request.id,
                            json!({"image": 99, "key": "opaque-image"}),
                            true,
                        )),
                        _ => {
                            // `host.finish` carries no payload, and a test that
                            // panics on parsing it cannot report which
                            // assertion the session ending actually broke.
                            let payload: Value =
                                serde_json::from_slice(&request.payload).unwrap_or(Value::Null);
                            let beacon =
                                request.kind == "net.send" && payload["frame"]["text"].is_string();
                            let refused = beacon && self.refuse_beacon;
                            let result = match refused {
                                true => Err(wire::Refusal::new(
                                    "stream_queue_full",
                                    "stream send queue is full",
                                )),
                                false => Ok(Vec::new()),
                            };
                            self.effects.push((request.kind, payload));
                            events.push(wire::Event::Response {
                                id: request.id,
                                result,
                                done: true,
                            });
                        }
                    }
                }
                if events.is_empty() && !frame.busy {
                    break;
                }
            }
        }
        fn item(&mut self, stream: &str, value: Value) {
            self.step(vec![Self::response(self.streams[stream], value, false)]);
        }
        fn control(&mut self, value: Value) {
            self.item("voice.hub", json!({"text": value.to_string()}));
        }
        /// Every control frame handed to the hub, in order, as JSON.
        fn sent(&self) -> Vec<Value> {
            self.effects
                .iter()
                .filter(|(kind, _)| kind == "net.send")
                .filter_map(|(_, body)| body["frame"]["text"].as_str())
                .map(|text| serde_json::from_str(text).unwrap())
                .collect()
        }
        fn errors(&self) -> Vec<String> {
            self.effects
                .iter()
                .filter(|(kind, body)| kind == "host.emit" && body["kind"] == "error")
                .map(|(_, body)| body["status"].as_str().unwrap().to_owned())
                .collect()
        }
    }

    /// The join is one subscription on the member's own node — no owner
    /// lookup, no gateway leg — and the hub is steered with the room's
    /// committed roster the moment it is ready, and again whenever it moves.
    #[test]
    fn the_huddle_is_joined_over_the_nodes_voice_hub_and_steered_by_the_roster() {
        let mut host = Host::new();
        let (a, b) = ("0a".repeat(32), "0b".repeat(32));
        host.roster = vec![a.clone(), b.clone()];
        host.step(Vec::new());
        host.item(
            "call.props",
            json!({"channel": "room", "muted": false, "source": "off"}),
        );
        assert!(host.streams.contains_key("voice.hub"));
        assert!(host.streams.contains_key("rpc.live"));
        assert!(host.errors().is_empty());
        let recipients: Vec<Value> = host
            .sent()
            .into_iter()
            .filter(|text| text["type"] == "recipients")
            .collect();
        assert_eq!(
            recipients,
            [json!({"type": "recipients", "peers": [a.clone(), b.clone()]})]
        );
        assert!(
            host.sent().iter().any(|text| text["type"] == "beacon"),
            "joining publishes this side's state"
        );

        // a `ready` with peers on it is tolerated and says nothing
        host.control(json!({"type": "ready", "peers": [{"peer": a}]}));
        // the chat plane moves: the roster is re-read and the hub re-steered
        host.roster = vec![b.clone()];
        host.item("rpc.live", json!({"block": 9}));
        assert_eq!(
            host.sent().last().unwrap(),
            &json!({"type": "recipients", "peers": [b]})
        );
        assert!(host.errors().is_empty());
    }

    #[test]
    fn a_refused_huddle_is_told_as_a_fact_and_ends_the_session() {
        let sentences = [
            (
                "key_without_account",
                "To join a call, create or join an account in Settings → Account.",
            ),
            ("not_in_huddle", "You are not in this call."),
            (
                "no_call_hub",
                "Voice is not on in this network: the node runs no call hub.",
            ),
            ("node_unreachable", "Your node did not answer."),
            ("some_new_reason", "the hub's own words"),
        ];
        for (reason, sentence) in sentences {
            let mut host = Host::new();
            host.refuse_hub = Some(reason);
            host.step(Vec::new());
            host.item(
                "call.props",
                json!({"channel": "room", "muted": false, "source": "off"}),
            );
            assert_eq!(host.errors(), [sentence], "{reason}");
            assert!(
                host.effects.iter().any(|(kind, _)| kind == "host.finish"),
                "{reason}: the session finishes instead of waiting on a hub that refused"
            );
            assert!(
                !host.effects.iter().any(|(kind, _)| kind == "rpc.view"),
                "{reason}: no roster is read for a huddle the node refused"
            );
        }
    }

    /// The hub's own control frames are read for what they carry; a request
    /// for a keyframe is met by construction (every captured image stands
    /// alone) and a rate hint has no knob here, so neither moves anything.
    #[test]
    fn hub_controls_are_understood_and_an_unknown_one_ends_the_call() {
        let mut host = Host::new();
        host.step(Vec::new());
        host.item("call.props", json!({"channel": "room", "source": "off"}));
        let peer = "02".repeat(32);
        host.control(json!({"type": "peer_beacon", "peer": peer, "muted": true, "camera_on": false, "sharing": false, "speaking": true}));
        let shown = host
            .effects
            .iter()
            .rev()
            .find(|(kind, body)| kind == "host.emit" && body["kind"] == "presentation")
            .unwrap()
            .1["peers"]
            .clone();
        assert_eq!(shown[0]["peer"], peer);
        assert_eq!(shown[0]["muted"], true);
        assert_eq!(shown[0]["speaking"], true);
        host.control(json!({"type": "keyframe_request"}));
        host.control(json!({"type": "rate_hint", "max_kbps": 300}));
        assert!(host.errors().is_empty());
        host.control(json!({"type": "peer_left", "peer": peer}));
        assert_eq!(host.errors(), ["unknown call control"]);
    }

    #[test]
    fn real_guest_contract_moves_hub_frames_and_changes_sources_without_native_call_protocol() {
        let mut host = Host::new();
        host.step(Vec::new());
        host.item(
            "call.props",
            json!({"channel": "room", "muted": false, "source": "off"}),
        );
        assert!(host.streams.contains_key("voice.hub"));
        host.item(
            "media.audio",
            json!({"frame": vec![7u8; 80], "sound": true}),
        );
        let mut audio_up = vec![1];
        audio_up.extend_from_slice(&[7u8; 80]);
        assert!(
            host.effects.iter().any(
                |(kind, body)| kind == "net.send" && body["frame"]["binary"] == json!(audio_up)
            )
        );

        let peer = "02".repeat(32);
        host.control(json!({"type":"peer_beacon","peer":peer,"muted":false,"camera_on":true,"sharing":false,"speaking":true}));
        // the hub's mixed playout names no peer
        let mut audio = vec![1];
        audio.extend_from_slice(&[5u8; 80]);
        host.item("voice.hub", json!({"binary": audio}));
        host.step(vec![wire::Event::Response {
            id: host.streams["clock.ticks"],
            result: Ok(Vec::new()),
            done: false,
        }]);
        assert!(host.effects.iter().any(|(kind, body)| kind == "media.play"
            && body["frames"][0]["peer"] == protocol::HUB
            && body["frames"][0]["frame"] == json!(vec![5u8; 80])));

        host.item(
            "call.props",
            json!({"channel": "room", "muted": true, "source": "screen"}),
        );
        host.item(
            "media.video",
            json!({"timestamp_ms": 7, "jpeg": [8,9], "preview":"local-preview"}),
        );
        assert!(host.effects.iter().any(|(kind, body)| kind == "net.send"
            && body["frame"]["binary"] == json!([2, 1, 0, 0, 0, 7, 8, 9])));
        let before = host.effects.len();
        host.item(
            "media.audio",
            json!({"frame": vec![7u8; 80], "sound": true}),
        );
        assert!(
            !host.effects[before..]
                .iter()
                .any(|(kind, body)| kind == "net.send" && body["frame"]["binary"][0] == 1)
        );

        let mut video = vec![3, 1, 0, 0, 0, 7];
        video.extend_from_slice(&[2; 32]);
        video.push(9);
        host.item("voice.hub", json!({"binary": video}));
        assert!(host.effects.iter().any(|(kind, body)| kind == "media.put"
            && body["image"] == 99
            && body["jpeg"] == json!([9])));
        assert!(
            host.effects
                .iter()
                .any(|(kind, body)| kind == "host.emit"
                    && body["peers"][0]["image"] == "opaque-image")
        );
    }
    /// A congested writer refuses a queued frame; the host's stream send queue
    /// is one frame deep, so a beacon lands on that refusal whenever the
    /// socket is behind. Losing the room over it would turn every impaired
    /// network into a dropped call, and the beacon is absolute state that the
    /// next turn can carry just as well.
    #[test]
    fn a_refused_beacon_is_retried_and_does_not_end_the_call() {
        fn beacons(host: &Host) -> usize {
            host.sent()
                .iter()
                .filter(|text| text["type"] == "beacon")
                .count()
        }
        let mut host = Host::new();
        host.step(Vec::new());
        host.item(
            "call.props",
            json!({"channel": "room", "muted": false, "source": "off"}),
        );
        let connected = beacons(&host);
        assert!(connected > 0, "joining publishes this side's state");

        // The writer backs up. The next speaking transition's beacon is
        // refused, and the room must survive it.
        host.refuse_beacon = true;
        host.item(
            "media.audio",
            json!({"frame": vec![7u8; 80], "sound": true}),
        );
        assert!(
            beacons(&host) > connected,
            "the refused beacon was actually attempted"
        );
        assert!(
            !host
                .effects
                .iter()
                .any(|(kind, body)| kind == "host.emit" && body["kind"] == "error"),
            "a refused control frame must not end the call"
        );
        assert!(
            !host.effects.iter().any(|(kind, _)| kind == "host.finish"),
            "a refused control frame must not finish the session"
        );
        // Voice keeps flowing while the control frame is outstanding.
        let before = host.effects.len();
        host.item(
            "media.audio",
            json!({"frame": vec![7u8; 80], "sound": true}),
        );
        assert!(
            host.effects[before..]
                .iter()
                .any(|(kind, body)| kind == "net.send" && body["frame"]["binary"][0] == 1),
            "audio keeps being sent while a beacon is outstanding"
        );

        // The writer drains; the newest state reaches the room on the next
        // playout tick without the guest being asked again.
        host.refuse_beacon = false;
        let before = beacons(&host);
        host.step(vec![wire::Event::Response {
            id: host.streams["clock.ticks"],
            result: Ok(Vec::new()),
            done: false,
        }]);
        assert!(
            beacons(&host) > before,
            "the retained beacon is offered again once the transport accepts"
        );
    }

    #[test]
    fn guest_selects_the_stage_and_emits_only_presentation_changes() {
        fn shown(host: &Host) -> Value {
            host.effects
                .iter()
                .rev()
                .find(|(kind, body)| kind == "host.emit" && body["kind"] == "presentation")
                .expect("guest presentation")
                .1
                .clone()
        }
        let mut host = Host::new();
        host.step(Vec::new());
        host.item("call.props", json!({"channel":"room", "source":"off"}));
        assert_eq!(
            shown(&host),
            json!({"kind":"presentation", "stage":"", "video_live":false, "peers":[], "tiles":[]})
        );
        host.item("call.props", json!({"channel":"room", "source":"screen"}));
        host.item(
            "media.video",
            json!({"timestamp_ms":1, "jpeg":[9], "preview":"local-preview"}),
        );
        assert_eq!(shown(&host)["stage"], "local-preview");
        assert_eq!(shown(&host)["tiles"], json!([]));
        assert_eq!(shown(&host)["video_live"], true);
        let peer = "02".repeat(32);
        host.roster = vec![peer.clone()];
        host.control(json!({"type":"peer_beacon", "peer":peer, "sharing":true}));
        assert_eq!(
            shown(&host)["stage"],
            "local-preview",
            "a beacon alone has no image to show"
        );
        let mut video = vec![3, 1, 0, 0, 0, 7];
        video.extend_from_slice(&[2; 32]);
        video.push(9);
        host.item("voice.hub", json!({"binary":video}));
        assert_eq!(
            shown(&host)["stage"],
            "opaque-image",
            "remote share takes priority"
        );
        assert_eq!(shown(&host)["tiles"], json!(["local-preview"]));
        let before = host
            .effects
            .iter()
            .filter(|(_, body)| body["kind"] == "presentation")
            .count();
        host.item(
            "media.video",
            json!({"timestamp_ms":2, "jpeg":[9], "preview":"local-preview"}),
        );
        assert_eq!(
            host.effects
                .iter()
                .filter(|(_, body)| body["kind"] == "presentation")
                .count(),
            before
        );
        // the peer leaves the committed roster: the hub sends no leave
        host.roster.clear();
        host.item("rpc.live", json!({"block": 2}));
        assert_eq!(shown(&host)["stage"], "local-preview");
        assert_eq!(shown(&host)["tiles"], json!([]));
        host.item("call.props", json!({"channel":"room", "source":"camera"}));
        assert_eq!(
            shown(&host)["tiles"],
            json!([]),
            "old capture preview is retired"
        );
        host.item(
            "media.video",
            json!({"timestamp_ms":3, "jpeg":[9], "preview":"camera-preview"}),
        );
        assert_eq!(shown(&host)["stage"], "");
        assert_eq!(shown(&host)["tiles"], json!(["camera-preview"]));
        host.item("call.props", json!({"channel":"room", "source":"off"}));
        assert_eq!(
            shown(&host),
            json!({"kind":"presentation", "stage":"", "video_live":false, "peers":[], "tiles":[]})
        );
    }
    #[test]
    fn guest_presentation_replaces_peer_state_without_native_folding() {
        fn peers(host: &Host) -> Value {
            host.effects
                .iter()
                .rev()
                .find(|(kind, body)| kind == "host.emit" && body["kind"] == "presentation")
                .unwrap()
                .1["peers"]
                .clone()
        }
        let mut host = Host::new();
        host.step(Vec::new());
        host.item("call.props", json!({"channel":"room", "source":"off"}));
        assert_eq!(peers(&host), json!([]));
        let peer = "02".repeat(32);
        for muted in [true, false] {
            host.control(
                json!({"type":"peer_beacon", "peer":peer, "muted":muted, "speaking":true}),
            );
            let list = peers(&host);
            assert_eq!(list.as_array().unwrap().len(), 1);
            assert_eq!(list[0]["peer"], peer);
            assert_eq!(list[0]["muted"], muted);
            assert_eq!(list[0]["speaking"], true);
        }
        let before = host.effects.len();
        host.control(json!({"type":"peer_beacon", "peer":peer, "muted":false, "speaking":true}));
        assert!(
            !host.effects[before..]
                .iter()
                .any(|(_, body)| body["kind"] == "presentation")
        );
        host.item("rpc.live", json!({"block": 2}));
        assert_eq!(peers(&host), json!([]));
    }
}
