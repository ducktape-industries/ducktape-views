use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

use crate::call_wire::{self, MAX_AUDIO_PAYLOAD};

const MAX_PEERS: usize = 32;
const JITTER_FRAMES: usize = 3;
/// The hub mixes every peer into one playout stream, so the host decodes it
/// with one decoder under this name.
pub const HUB: &str = "hub";

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Props {
    pub channel: String,
    pub muted: bool,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Beacon {
    pub muted: bool,
    pub camera_on: bool,
    pub sharing: bool,
    pub speaking: bool,
}

#[derive(Clone, Debug)]
pub enum Event {
    Properties(Props),
    /// One encoded frame from this device's microphone, with the host's
    /// verdict on whether it carried sound. The energy that decides `sound`
    /// is measured on the samples, which live and die at the device: the
    /// guest owns what SPEAKING means to the room (hangover, beacons), never
    /// the audio itself.
    LocalAudio {
        frame: Vec<u8>,
        sound: bool,
    },
    LocalImage {
        timestamp_ms: u32,
        jpeg: Vec<u8>,
    },
    Peer {
        peer: String,
        beacon: Beacon,
    },
    /// The room's committed huddle roster, as node keys: the fan-out set the
    /// hub is steered with, and the list a peer has left when it is gone from.
    Roster(Vec<String>),
    /// One frame of the hub's mixed playout.
    RemoteAudio(Vec<u8>),
    RemoteImage {
        peer: String,
        jpeg: Vec<u8>,
    },
    Tick,
}

#[derive(Clone, Debug)]
pub enum Effect {
    SendText(serde_json::Value),
    SendBinary(Vec<u8>),
    SelfState(Beacon),
    Capture(String),
    Mute(bool),
    /// This tick's audio: the hub's mixed frame, if one was due, under
    /// [`HUB`]. The host decodes each named stream with its own decoder —
    /// a codec is stateful per stream — so the name stays on the frame.
    Play(Vec<(String, Vec<u8>)>),
    Image {
        peer: String,
        jpeg: Vec<u8>,
    },
    DropImage(String),
}

#[derive(Default)]
pub struct Machine {
    pub props: Props,
    pub peers: BTreeMap<String, Beacon>,
    audio: VecDeque<Vec<u8>>,
    ticks: u64,
    last_sound: Option<u64>,
    speaking: bool,
}

impl Machine {
    pub fn step(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Properties(props) => self.properties(props),
            Event::LocalAudio { frame, sound } => self.local_audio(frame, sound),
            Event::LocalImage { timestamp_ms, jpeg } => self.local_image(timestamp_ms, jpeg),
            Event::Peer { peer, beacon } => self.peer(peer, beacon),
            Event::Roster(peers) => self.roster(peers),
            Event::RemoteAudio(frame) => self.remote_audio(frame),
            Event::RemoteImage { peer, jpeg } => self.remote_image(peer, jpeg),
            Event::Tick => self.tick(),
        }
    }

    fn state(&self) -> Beacon {
        Beacon {
            muted: self.props.muted,
            camera_on: self.props.source == "camera",
            sharing: self.props.source == "screen",
            speaking: self.speaking,
        }
    }

    fn beacon(&self) -> Vec<Effect> {
        let state = self.state();
        vec![
            Effect::SendText(serde_json::json!({"type": "beacon", "muted": state.muted,
            "camera_on": state.camera_on, "sharing": state.sharing, "speaking": state.speaking})),
            Effect::SelfState(state),
        ]
    }

    fn properties(&mut self, props: Props) -> Vec<Effect> {
        let source_changed = self.props.source != props.source;
        self.props = props;
        if self.props.muted {
            self.speaking = false;
            self.last_sound = None;
        }
        let mut effects = vec![Effect::Mute(self.props.muted)];
        if source_changed {
            effects.push(Effect::Capture(self.props.source.clone()));
        }
        effects.extend(self.beacon());
        effects
    }

    fn local_audio(&mut self, frame: Vec<u8>, sound: bool) -> Vec<Effect> {
        let carries_one_frame = !frame.is_empty() && frame.len() <= MAX_AUDIO_PAYLOAD;
        let may_send = !self.props.muted && carries_one_frame;
        if !may_send {
            return Vec::new();
        }
        let mut effects = Vec::new();
        if sound {
            self.last_sound = Some(self.ticks);
            if !self.speaking {
                self.speaking = true;
                effects.extend(self.beacon());
            }
        }
        effects.push(Effect::SendBinary(call_wire::encode_audio(&frame)));
        effects
    }

    fn local_image(&mut self, timestamp_ms: u32, jpeg: Vec<u8>) -> Vec<Effect> {
        let capturing = matches!(self.props.source.as_str(), "camera" | "screen");
        if !capturing || jpeg.len() > 65_530 {
            return Vec::new();
        }
        // every captured image stands alone, so every one is a keyframe
        vec![Effect::SendBinary(call_wire::encode_captured(
            true,
            timestamp_ms,
            &jpeg,
        ))]
    }

    fn peer(&mut self, peer: String, beacon: Beacon) -> Vec<Effect> {
        let room_full = !self.peers.contains_key(&peer) && self.peers.len() >= MAX_PEERS;
        if room_full {
            return Vec::new();
        }
        self.peers.insert(peer.clone(), beacon.clone());
        let mut effects = Vec::new();
        if !beacon.camera_on && !beacon.sharing {
            effects.push(Effect::DropImage(peer.clone()));
        }
        effects
    }

    fn roster(&mut self, peers: Vec<String>) -> Vec<Effect> {
        let gone: Vec<String> = self
            .peers
            .keys()
            .filter(|peer| !peers.contains(peer))
            .cloned()
            .collect();
        for peer in &gone {
            self.peers.remove(peer);
        }
        let mut effects = vec![Effect::SendText(
            serde_json::json!({"type": "recipients", "peers": peers}),
        )];
        effects.extend(gone.into_iter().map(Effect::DropImage));
        effects
    }

    fn remote_audio(&mut self, frame: Vec<u8>) -> Vec<Effect> {
        let carries_one_frame = !frame.is_empty() && frame.len() <= MAX_AUDIO_PAYLOAD;
        if !carries_one_frame {
            return Vec::new();
        }
        if self.audio.len() == JITTER_FRAMES {
            self.audio.pop_front();
        }
        self.audio.push_back(frame);
        Vec::new()
    }

    fn remote_image(&mut self, peer: String, jpeg: Vec<u8>) -> Vec<Effect> {
        let Some(beacon) = self.peers.get(&peer) else {
            return Vec::new();
        };
        let capturing = beacon.camera_on || beacon.sharing;
        if !capturing {
            return Vec::new();
        }
        vec![Effect::Image { peer, jpeg }]
    }

    fn tick(&mut self) -> Vec<Effect> {
        self.ticks += 1;
        let mut effects = Vec::new();
        let expired = self.speaking
            && self
                .last_sound
                .is_none_or(|last| self.ticks.saturating_sub(last) >= 20);
        if expired {
            self.speaking = false;
            effects.extend(self.beacon());
        }
        // the hub's mixed frame, if one is due. An empty list is the silence
        // a playout tick still has to hear about.
        let due: Vec<(String, Vec<u8>)> = self
            .audio
            .pop_front()
            .map(|frame| (HUB.to_owned(), frame))
            .into_iter()
            .collect();
        effects.push(Effect::Play(due));
        effects
    }
}

pub fn binary(bytes: &[u8]) -> Result<Event, String> {
    if let Some(frame) = call_wire::decode_audio(bytes) {
        return Ok(Event::RemoteAudio(frame.to_vec()));
    }
    match call_wire::decode_peer(bytes) {
        Some(frame) if frame.data.len() <= 65_530 => Ok(Event::RemoteImage {
            peer: frame.peer,
            jpeg: frame.data,
        }),
        _ => Err("invalid media frame".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// one encoded frame, as the host hands it over. The guest never reads
    /// inside one, so a marker byte stands in for a codec here.
    fn voice(marker: u8) -> Vec<u8> {
        vec![marker; 80]
    }

    #[test]
    fn mute_and_source_switch_are_guest_protocol_decisions() {
        let mut machine = Machine::default();
        let effects = machine.step(Event::Properties(Props {
            muted: true,
            source: "screen".into(),
            channel: "r".into(),
        }));
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Capture(source) if source == "screen"))
        );
        assert!(
            machine
                .step(Event::LocalAudio {
                    frame: voice(7),
                    sound: true
                })
                .is_empty(),
            "a muted device sends nothing, however loud the room"
        );
        assert!(
            matches!(machine.step(Event::LocalImage { timestamp_ms: 7, jpeg: vec![9] }).as_slice(), [Effect::SendBinary(bytes)] if bytes == &[2,1,0,0,0,7,9])
        );
    }
    /// The view-framing stage of #2237's latency decomposition. Every event a
    /// live call turns on runs through `step`, so this is the whole cost the
    /// guest adds between a captured frame and the byte handed to the host.
    /// The payload is rebuilt each round, so the same construction is timed
    /// alone and subtracted: what is left is the step.
    ///
    /// Program output: the harness is run by hand and the numbers ARE the
    /// result.
    #[test]
    #[ignore = "measurement harness"]
    fn framing_cost_per_event() {
        const ROUNDS: u32 = 20_000;
        let mut machine = Machine::default();
        machine.step(Event::Properties(Props {
            channel: "room".into(),
            muted: false,
            source: "camera".into(),
        }));
        for index in 0..4u32 {
            machine.step(Event::Peer {
                peer: format!("{index:064x}"),
                beacon: Beacon {
                    camera_on: true,
                    ..Beacon::default()
                },
            });
        }
        let speaker = machine.peers.keys().next().expect("a seated peer").clone();
        let jpeg = vec![7u8; 16 * 1024];
        let sources: [(&str, &dyn Fn() -> Event); 5] = [
            ("local_audio", &|| Event::LocalAudio {
                frame: vec![7u8; 80],
                sound: true,
            }),
            ("local_image", &|| Event::LocalImage {
                timestamp_ms: 7,
                jpeg: vec![7u8; 16 * 1024],
            }),
            ("remote_audio", &|| Event::RemoteAudio(vec![9u8; 80])),
            ("remote_image", &|| Event::RemoteImage {
                peer: format!("{:064x}", 0),
                jpeg: vec![7u8; 16 * 1024],
            }),
            ("tick", &|| Event::Tick),
        ];
        assert_eq!(speaker, format!("{:064x}", 0), "the peer the sources name");
        assert_eq!(jpeg.len(), 16 * 1024);
        println!("== view framing, {ROUNDS} rounds, 4 seated peers ==");
        println!("event\tstep_ns\tbuild_ns");
        for (name, build) in sources {
            let idle = std::time::Instant::now();
            for _ in 0..ROUNDS {
                std::hint::black_box(build());
            }
            let idle = idle.elapsed().as_nanos() / u128::from(ROUNDS);
            let busy = std::time::Instant::now();
            for _ in 0..ROUNDS {
                machine.step(build());
            }
            let busy = busy.elapsed().as_nanos() / u128::from(ROUNDS);
            println!("{name}\t{}\t{idle}", busy.saturating_sub(idle));
        }
    }

    #[test]
    fn sound_hangover_and_bounded_jitter_are_owned_by_the_guest() {
        let mut machine = Machine::default();
        machine.step(Event::LocalAudio {
            frame: voice(1),
            sound: true,
        });
        assert!(machine.speaking);
        // silence does not end the turn — the hangover does
        machine.step(Event::LocalAudio {
            frame: voice(1),
            sound: false,
        });
        assert!(machine.speaking);
        for _ in 0..20 {
            machine.step(Event::Tick);
        }
        assert!(!machine.speaking);
        for _ in 0..20 {
            machine.step(Event::RemoteAudio(voice(9)));
        }
        assert_eq!(machine.audio.len(), JITTER_FRAMES);
        // the tick hands the host the hub's mixed frame, named, for it to
        // decode — the guest moves audio it cannot read.
        assert!(
            matches!(machine.step(Event::Tick).as_slice(), [Effect::Play(due)]
                if due.len() == 1 && due[0].0 == HUB && due[0].1 == voice(9))
        );
        machine.audio.clear();
        assert!(
            matches!(machine.step(Event::Tick).as_slice(), [Effect::Play(due)] if due.is_empty())
        );
    }

    /// The roster is what the hub is told to fan out to, and a peer gone
    /// from it is gone from the room — the hub sends no leave.
    #[test]
    fn the_roster_steers_fan_out_and_retires_peers_gone_from_it() {
        let mut machine = Machine::default();
        for peer in ["a", "b"] {
            machine.step(Event::Peer {
                peer: peer.into(),
                beacon: Beacon {
                    camera_on: true,
                    ..Beacon::default()
                },
            });
        }
        let effects = machine.step(Event::Roster(vec!["b".into(), "c".into()]));
        assert!(matches!(&effects[0], Effect::SendText(text)
            if *text == serde_json::json!({"type":"recipients","peers":["b","c"]})));
        assert!(matches!(&effects[1], Effect::DropImage(peer) if peer == "a"));
        assert_eq!(effects.len(), 2);
        assert_eq!(machine.peers.keys().collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn frames_off_the_wire_become_events_or_nothing() {
        assert!(matches!(binary(&[1, 5, 5]), Ok(Event::RemoteAudio(frame)) if frame == [5, 5]));
        let mut video = vec![3, 1, 0, 0, 0, 7];
        video.extend_from_slice(&[2; 32]);
        video.push(9);
        assert!(
            matches!(binary(&video), Ok(Event::RemoteImage { peer, jpeg })
            if peer == "02".repeat(32) && jpeg == [9])
        );
        assert!(binary(&[4, 0, 0]).is_err());
        assert!(binary(&video[..38]).is_err());
    }

    /// A frame the host could not have produced is refused at the seam it
    /// arrives on, in both directions: these bytes reach the guest from an
    /// untrusted peer through the hub, and from its own host.
    #[test]
    fn an_audio_frame_outside_the_bound_is_dropped() {
        let mut machine = Machine::default();
        machine.step(Event::Peer {
            peer: "a".into(),
            beacon: Beacon::default(),
        });
        for frame in [Vec::new(), vec![3; MAX_AUDIO_PAYLOAD + 1]] {
            assert!(
                machine
                    .step(Event::LocalAudio {
                        frame: frame.clone(),
                        sound: true
                    })
                    .is_empty()
            );
            machine.step(Event::RemoteAudio(frame));
            assert!(machine.audio.is_empty());
        }
        assert!(!machine.speaking, "a dropped frame is not a spoken one");
    }
}
