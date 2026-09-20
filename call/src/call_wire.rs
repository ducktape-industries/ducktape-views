//! The binary frames of the node's voice hub, byte for byte as core defines
//! them in `crates/services/media/src/call_wire.rs` (the only definition
//! site; this is a mirror, not a dependency). First byte selects the kind:
//!
//! ```text
//! audio    [0x01][encoded voice frame …]                    — both directions
//! captured [0x02][flags u8][ts_ms u32 BE][video …]           — client → hub
//! peer     [0x03][flags u8][ts_ms u32 BE][peer 32][video …]  — hub → client
//! ```
//! `flags` bit 0 marks a keyframe. Audio down is the hub's one mixed stream,
//! so it names no peer. Every decoder answers `None` on bad input: these
//! bytes come off the network, and a malformed frame is a dropped frame.

pub const TAG_AUDIO: u8 = 0x01;
pub const TAG_VIDEO_CAPTURED: u8 = 0x02;
pub const TAG_VIDEO_PEER: u8 = 0x03;
pub const FLAG_KEYFRAME: u8 = 0b0000_0001;
const VIDEO_CAPTURED_HEADER: usize = 6;
const VIDEO_PEER_HEADER: usize = 38;

/// One 20 ms encoded voice frame at most, core's `voice::MAX_ENCODED`.
pub const MAX_AUDIO_PAYLOAD: usize = 1275;

pub fn encode_audio(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + payload.len());
    out.push(TAG_AUDIO);
    out.extend_from_slice(payload);
    out
}

pub fn decode_audio(frame: &[u8]) -> Option<&[u8]> {
    let payload = frame.strip_prefix(&[TAG_AUDIO])?;
    let carries_one_frame = !payload.is_empty() && payload.len() <= MAX_AUDIO_PAYLOAD;
    carries_one_frame.then_some(payload)
}

pub fn encode_captured(keyframe: bool, ts_ms: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(VIDEO_CAPTURED_HEADER + data.len());
    out.push(TAG_VIDEO_CAPTURED);
    out.push(if keyframe { FLAG_KEYFRAME } else { 0 });
    out.extend_from_slice(&ts_ms.to_be_bytes());
    out.extend_from_slice(data);
    out
}

#[derive(Debug, PartialEq, Eq)]
pub struct PeerFrame {
    /// The sending peer's node key, as the hex the roster spells it in.
    pub peer: String,
    pub keyframe: bool,
    pub ts_ms: u32,
    pub data: Vec<u8>,
}

pub fn decode_peer(frame: &[u8]) -> Option<PeerFrame> {
    if frame.len() <= VIDEO_PEER_HEADER || frame[0] != TAG_VIDEO_PEER {
        return None;
    }
    Some(PeerFrame {
        peer: frame[6..38]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        keyframe: frame[1] & FLAG_KEYFRAME != 0,
        ts_ms: u32::from_be_bytes(frame[2..6].try_into().expect("4 bytes")),
        data: frame[VIDEO_PEER_HEADER..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_is_the_tag_and_the_opaque_payload() {
        assert_eq!(encode_audio(&[0x78, 1, 2]), [0x01, 0x78, 1, 2]);
        assert_eq!(decode_audio(&[0x01, 0x78, 1, 2]), Some(&[0x78u8, 1, 2][..]));
        assert_eq!(decode_audio(&[0x01]), None);
        assert_eq!(decode_audio(&[0x02, 1]), None);
        assert_eq!(decode_audio(&[0x01; 2 + MAX_AUDIO_PAYLOAD]), None);
    }

    #[test]
    fn golden_captured_video_be() {
        assert_eq!(
            encode_captured(true, 0x0102_0304, &[0xAA, 0xBB]),
            [0x02, 0x01, 0x01, 0x02, 0x03, 0x04, 0xAA, 0xBB]
        );
    }

    #[test]
    fn golden_peer_video_be() {
        let mut frame = vec![0x03, 0x00, 0x0A, 0x0B, 0x0C, 0x0D];
        frame.extend_from_slice(&[0x11; 32]);
        frame.push(0xF0);
        assert_eq!(
            decode_peer(&frame),
            Some(PeerFrame {
                peer: "11".repeat(32),
                keyframe: false,
                ts_ms: 0x0A0B_0C0D,
                data: vec![0xF0],
            })
        );
        frame[1] = 0x01;
        assert!(decode_peer(&frame).unwrap().keyframe);
        assert_eq!(decode_peer(&frame[..38]), None);
        frame[0] = 0x02;
        assert_eq!(decode_peer(&frame), None);
    }
}
