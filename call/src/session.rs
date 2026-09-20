//! Execute protocol effects through the ordinary view host contract.
use std::collections::BTreeMap;

use ducktape_view_guest::host;
use futures::{FutureExt as _, StreamExt as _, stream::LocalBoxStream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Message;
use crate::protocol::{self, Beacon, Effect, Event, Machine, Props};

/// One item off a host subscription, or `None` when the stream ended. The
/// refusal is converted to its sentence at [`answer`], the one place this view
/// leaves the host's shape — nothing below branches on a token, except the
/// hub's own four at [`refused`].
type Answer = Option<host::Answer>;

enum Input {
    Props(Answer),
    Network(Answer),
    Live(Answer),
    Audio(Answer),
    Video(Answer),
    Clock(Answer),
}
enum Run {
    Start,
    Panel(host::Subscription),
    Live(Box<Session>),
    End,
}

struct Session {
    properties: host::Subscription,
    network: host::Subscription,
    /// The chat plane moving: the committed huddle roster may have changed.
    live: host::Subscription,
    audio: host::Subscription,
    video: Option<host::Subscription>,
    clock: host::Subscription,
    machine: Machine,
    images: BTreeMap<String, (u64, String)>,
    preview: String,
    presentation: Option<Presentation>,
    /// The newest control frame of each kind the transport has not accepted
    /// yet. Beacons and recipients are absolute and last-write-wins, so only
    /// the newest of each is worth keeping.
    unsent: BTreeMap<String, Value>,
}

#[derive(PartialEq, Eq, Serialize)]
struct Peer {
    peer: String,
    image: String,
    #[serde(flatten)]
    beacon: Beacon,
}

#[derive(PartialEq, Eq)]
struct Presentation {
    stage: String,
    video_live: bool,
    peers: Vec<Peer>,
    tiles: Vec<String>,
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON effect")
}
fn notify(kind: &str, value: Value) {
    host::notify(kind, &bytes(&value));
}
fn emit(value: Value) {
    notify("host.emit", value);
}
fn status(kind: &str, message: &str) {
    emit(json!({"kind": kind, "message": message, "status": status_text(kind, message)}));
}

fn status_text(kind: &str, message: &str) -> String {
    match kind {
        "connecting" | "closed" => kind.into(),
        "live" => {
            if message.is_empty() {
                "live".into()
            } else {
                format!("live · {message}")
            }
        }
        _ => message.into(),
    }
}
fn answer(answer: Answer) -> Result<Vec<u8>, String> {
    answer
        .ok_or_else(|| "session stream closed".to_owned())?
        .map_err(host::said)
}

fn properties(bytes: &[u8]) -> Result<Props, String> {
    let props: Props = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let valid = !props.channel.is_empty()
        && props.channel.len() <= 256
        && matches!(props.source.as_str(), "off" | "camera" | "screen");
    if !valid {
        return Err("invalid call properties".into());
    }
    Ok(props)
}

/// The hub refuses the huddle with one of four reasons; each is a fact about
/// the room or the node, so the person reads it as one, not as the host's own
/// words. Any other refusal is shown as the host said it.
fn refused(refusal: host::Refusal) -> String {
    match refusal.reason.as_str() {
        "key_without_account" => {
            "To join a call, create or join an account in Settings → Account.".into()
        }
        "not_in_huddle" => "You are not in this call.".into(),
        "no_call_hub" => "Voice is not on in this network: the node runs no call hub.".into(),
        "node_unreachable" => "Your node did not answer.".into(),
        _ => host::said(refusal),
    }
}

/// The room's committed huddle roster as node keys — consensus state, the
/// same list the panel shows — which is what the hub is told to fan out to.
async fn roster(channel: &str) -> Result<Vec<String>, String> {
    let reply = host::request(
        "rpc.view",
        &bytes(&json!({"target": "chat", "query": {"channel": {"channel_id": channel}}})),
    )
    .await
    .map_err(host::said)?;
    let reply: Value = serde_json::from_slice(&reply).map_err(|error| error.to_string())?;
    reply["channel"]["huddle"]
        .as_array()
        .ok_or("missing call roster")?
        .iter()
        .map(|seat| {
            seat["node"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "missing call node".into())
        })
        .collect()
}

pub fn run() -> LocalBoxStream<'static, Message> {
    futures::stream::unfold(Run::Start, |state| async move {
        let next = match state {
            Run::Start => begin().await,
            Run::Panel(mut properties) => {
                let item = answer(properties.next().await).and_then(|bytes| panel_properties(&bytes));
                item.map(|panel| (Message::Panel(Box::new(panel)), Run::Panel(properties)))
            }
            Run::Live(mut session) => session.advance().await.map(|()| (Message::Progress, Run::Live(session))),
            Run::End => return None,
        };
        let next = match next {
            Ok(next) => next,
            Err(error) => {
                emit(json!({"kind":"presentation", "stage":"", "video_live":false, "peers":[], "tiles":[]}));
                status("error", &error);
                host::notify("host.finish", &[]);
                (Message::Progress, Run::End)
            }
        };
        Some(next)
    })
    .boxed_local()
}

fn panel_properties(bytes: &[u8]) -> Result<crate::panel::Panel, String> {
    #[derive(Deserialize)]
    struct Envelope {
        panel: crate::panel::Panel,
    }
    serde_json::from_slice::<Envelope>(bytes)
        .map(|value| value.panel)
        .map_err(|error| error.to_string())
}

async fn begin() -> Result<(Message, Run), String> {
    let mut stream = host::subscribe("call.props", &[]);
    let bytes = answer(stream.next().await)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if value.get("panel").is_some() {
        return Ok((
            Message::Panel(Box::new(panel_properties(&bytes)?)),
            Run::Panel(stream),
        ));
    }
    let session = Session::connect(stream, properties(&bytes)?).await?;
    Ok((Message::Progress, Run::Live(Box::new(session))))
}

impl Session {
    async fn connect(properties_stream: host::Subscription, props: Props) -> Result<Self, String> {
        status("connecting", "");
        // The room's huddle lives on this member's own node: the host resolves
        // the channel to its hub, so the view names no account and no gateway leg.
        let mut network = host::subscribe("voice.hub", &bytes(&json!({"channel": props.channel})));
        let first = match network.next().await {
            Some(Err(refusal)) => return Err(refused(refusal)),
            item => answer(item)?,
        };
        let Network::Text(ready) = network_message(&first)? else {
            return Err("the call hub sent no ready message".into());
        };
        if ready.get("type").and_then(Value::as_str) != Some("ready") {
            return Err("the call hub refused the session".into());
        }
        let peers = roster(&props.channel).await?;
        let mut session = Self {
            properties: properties_stream,
            network,
            live: host::subscribe("rpc.live", b"chat"),
            audio: host::subscribe("media.audio", b"{}"),
            video: None,
            clock: host::subscribe("clock.ticks", &20i64.to_le_bytes()),
            machine: Machine::default(),
            images: BTreeMap::new(),
            preview: String::new(),
            presentation: None,
            unsent: BTreeMap::new(),
        };
        for event in [Event::Properties(props), Event::Roster(peers)] {
            let effects = session.machine.step(event);
            session.execute(effects).await?;
        }
        status("live", "");
        Ok(session)
    }

    async fn advance(&mut self) -> Result<(), String> {
        let input = {
            let properties = self.properties.next().fuse();
            let network = self.network.next().fuse();
            let live = self.live.next().fuse();
            let audio = self.audio.next().fuse();
            let clock = self.clock.next().fuse();
            let video = async {
                match self.video.as_mut() {
                    Some(video) => video.next().await,
                    None => std::future::pending().await,
                }
            }
            .fuse();
            futures::pin_mut!(properties, network, live, audio, clock, video);
            futures::select_biased! {
                item = properties => Input::Props(item),
                item = network => Input::Network(item),
                item = live => Input::Live(item),
                item = clock => Input::Clock(item),
                item = audio => Input::Audio(item),
                item = video => Input::Video(item),
            }
        };
        let events = self.events(input).await?;
        for event in events {
            let effects = self.machine.step(event);
            self.execute(effects).await?;
        }
        Ok(())
    }

    async fn events(&mut self, input: Input) -> Result<Vec<Event>, String> {
        match input {
            Input::Props(item) => self.changed_properties(item),
            Input::Network(item) => self.received(item),
            Input::Live(item) => {
                answer(item)?;
                Ok(vec![Event::Roster(
                    roster(&self.machine.props.channel).await?,
                )])
            }
            Input::Audio(item) => self.captured_audio(item),
            Input::Video(item) => self.captured_video(item),
            Input::Clock(item) => self.clock_tick(item),
        }
    }

    fn changed_properties(&mut self, item: Answer) -> Result<Vec<Event>, String> {
        let props = properties(&answer(item)?)?;
        if props.channel != self.machine.props.channel {
            return Err("call room changed".into());
        }
        Ok(vec![Event::Properties(props)])
    }

    fn received(&mut self, item: Answer) -> Result<Vec<Event>, String> {
        match network_message(&answer(item)?)? {
            Network::Text(value) => control(value),
            Network::Binary(bytes) => protocol::binary(&bytes).map(|event| vec![event]),
        }
    }

    fn captured_audio(&mut self, item: Answer) -> Result<Vec<Event>, String> {
        let value: Value =
            serde_json::from_slice(&answer(item)?).map_err(|error| error.to_string())?;
        if value.get("ready") == Some(&Value::Bool(true)) {
            status(
                "live",
                value
                    .get("note")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            return Ok(Vec::new());
        }
        // the device hands over an ENCODED frame and its own verdict on
        // whether it carried sound: the samples the verdict was measured on
        // never leave the host.
        let frame =
            serde_json::from_value(value.get("frame").cloned().ok_or("missing voice frame")?)
                .map_err(|error| error.to_string())?;
        let sound = value
            .get("sound")
            .and_then(Value::as_bool)
            .unwrap_or_default();
        Ok(vec![Event::LocalAudio { frame, sound }])
    }

    fn captured_video(&mut self, item: Answer) -> Result<Vec<Event>, String> {
        #[derive(Deserialize)]
        struct Image {
            timestamp_ms: u32,
            jpeg: Vec<u8>,
            preview: String,
        }
        let bytes = match answer(item) {
            Ok(bytes) => bytes,
            Err(error) => {
                status("live", &error);
                self.video.take();
                let mut props = self.machine.props.clone();
                props.source = "off".into();
                return Ok(vec![Event::Properties(props)]);
            }
        };
        let image: Image = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        self.preview = image.preview;
        Ok(vec![Event::LocalImage {
            timestamp_ms: image.timestamp_ms,
            jpeg: image.jpeg,
        }])
    }

    fn clock_tick(&mut self, item: Answer) -> Result<Vec<Event>, String> {
        answer(item)?;
        Ok(vec![Event::Tick])
    }

    async fn execute(&mut self, effects: Vec<Effect>) -> Result<(), String> {
        // A control frame an earlier turn could not hand over goes out ahead
        // of this turn's effects, so state still reaches the room in the
        // order the machine decided it.
        self.flush().await;
        for effect in effects {
            match effect {
                Effect::SendText(text) => self.send_text(text).await,
                Effect::SendBinary(binary) => notify(
                    "net.send",
                    json!({"stream": self.network.id(), "frame": {"binary": binary}}),
                ),
                Effect::SelfState(state) => emit(
                    json!({"kind": "self", "muted": state.muted, "camera_on": state.camera_on, "sharing": state.sharing, "speaking": state.speaking}),
                ),
                Effect::Capture(source) => self.capture(&source),
                Effect::Mute(muted) => notify(
                    "media.mute",
                    json!({"audio": self.audio.id(), "muted": muted}),
                ),
                Effect::Play(due) => {
                    let frames: Vec<Value> = due
                        .into_iter()
                        .map(|(peer, frame)| json!({"peer": peer, "frame": frame}))
                        .collect();
                    notify(
                        "media.play",
                        json!({"audio": self.audio.id(), "frames": frames}),
                    )
                }
                Effect::Image { peer, jpeg } => self.picture(peer, jpeg).await?,
                Effect::DropImage(peer) => self.drop_picture(&peer),
            }
        }
        self.present();
        Ok(())
    }

    async fn send_text(&mut self, text: Value) {
        let kind = text["type"].as_str().unwrap_or_default().to_owned();
        self.unsent.insert(kind, text);
        self.flush().await;
    }

    /// Control frames (beacon, recipients) are absolute and last-write-wins
    /// per kind, so a transport that REFUSES to queue one keeps the newest
    /// and offers it again on the next turn rather than ending the call. A
    /// congested writer refuses exactly this way, and losing the room because
    /// a control frame arrived a tick late is a worse outcome than the late
    /// tick.
    ///
    /// A transport that is genuinely gone still ends the session: the network
    /// subscription delivers its own terminal item and `received` fails on it.
    /// Every turn emits `Effect::Play`, so the retry runs at the playout tick.
    async fn flush(&mut self) {
        for (kind, text) in std::mem::take(&mut self.unsent) {
            let queued = host::request(
                "net.send",
                &bytes(&json!({"stream": self.network.id(), "frame": {"text": text.to_string()}})),
            )
            .await;
            if queued.is_err() {
                self.unsent.insert(kind, text);
            }
        }
    }

    fn present(&mut self) {
        let remote = self.machine.peers.iter().find_map(|(peer, beacon)| {
            if !beacon.sharing {
                return None;
            }
            self.images.get(peer).map(|(_, key)| key.clone())
        });
        let stage = remote.unwrap_or_else(|| match self.machine.props.source.as_str() {
            "screen" => self.preview.clone(),
            _ => String::new(),
        });
        let local_video = matches!(self.machine.props.source.as_str(), "camera" | "screen");
        let remote_video = self
            .machine
            .peers
            .values()
            .any(|peer| peer.camera_on || peer.sharing);
        let peers = self
            .machine
            .peers
            .iter()
            .map(|(peer, beacon)| Peer {
                peer: peer.clone(),
                image: self
                    .images
                    .get(peer)
                    .map(|(_, key)| key.clone())
                    .unwrap_or_default(),
                beacon: beacon.clone(),
            })
            .collect();
        let mut tiles: Vec<String> = self
            .images
            .values()
            .map(|(_, key)| key)
            .filter(|key| **key != stage)
            .cloned()
            .collect();
        let show_preview = local_video && !self.preview.is_empty() && self.preview != stage;
        if show_preview {
            tiles.push(self.preview.clone());
        }
        let current = Presentation {
            stage,
            video_live: local_video || remote_video,
            peers,
            tiles,
        };
        let unchanged = self.presentation.as_ref() == Some(&current);
        if unchanged {
            return;
        }
        emit(
            json!({"kind":"presentation", "stage":current.stage, "video_live":current.video_live, "peers":current.peers, "tiles":current.tiles}),
        );
        self.presentation = Some(current);
    }

    fn capture(&mut self, source: &str) {
        self.video.take();
        self.preview.clear();
        if source != "off" {
            self.video = Some(host::subscribe(
                "media.video",
                &bytes(&json!({"source": source, "max_bytes": 65_530})),
            ));
        }
    }

    async fn picture(&mut self, peer: String, jpeg: Vec<u8>) -> Result<(), String> {
        if !self.images.contains_key(&peer) {
            #[derive(Deserialize)]
            struct Image {
                image: u64,
                key: String,
            }
            let image = host::request("media.image", b"{}")
                .await
                .map_err(host::said)?;
            let image: Image = serde_json::from_slice(&image).map_err(|error| error.to_string())?;
            self.images.insert(peer.clone(), (image.image, image.key));
        }
        notify(
            "media.put",
            json!({"image": self.images[&peer].0, "jpeg": jpeg}),
        );
        Ok(())
    }

    fn drop_picture(&mut self, peer: &str) {
        if let Some((image, _)) = self.images.remove(peer) {
            notify("media.drop", json!({"image": image}));
        }
    }
}

enum Network {
    Text(Value),
    Binary(Vec<u8>),
}

fn network_message(bytes: &[u8]) -> Result<Network, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return serde_json::from_str(text)
            .map(Network::Text)
            .map_err(|error| error.to_string());
    }
    let binary = value
        .get("binary")
        .cloned()
        .ok_or("unknown network frame")?;
    serde_json::from_value(binary)
        .map(Network::Binary)
        .map_err(|error| error.to_string())
}

fn control(value: Value) -> Result<Vec<Event>, String> {
    fn peer(value: &Value) -> Result<String, String> {
        let peer = value
            .get("peer")
            .and_then(Value::as_str)
            .ok_or("missing peer")?;
        let canonical = peer.len() == 64
            && peer
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        canonical
            .then(|| peer.to_owned())
            .ok_or_else(|| "invalid peer".into())
    }
    match value.get("type").and_then(Value::as_str) {
        Some("peer_beacon") => Ok(vec![Event::Peer {
            peer: peer(&value)?,
            beacon: serde_json::from_value(value).map_err(|error| error.to_string())?,
        }]),
        // `ready` names no peers (the roster is consensus state); every
        // captured image already stands alone, so a keyframe request is
        // met; and a rate hint has no knob on this side yet.
        Some("ready" | "keyframe_request") => Ok(Vec::new()),
        Some("rate_hint") if value["max_kbps"].is_u64() => Ok(Vec::new()),
        _ => Err("unknown call control".into()),
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn status_labels_belong_to_the_deployed_session() {
        assert_eq!(status_text("connecting", ""), "connecting");
        assert_eq!(status_text("live", ""), "live");
        assert_eq!(status_text("live", "no microphone"), "live · no microphone");
        assert_eq!(status_text("refused", "not seated"), "not seated");
        assert_eq!(status_text("closed", ""), "closed");
    }
}
