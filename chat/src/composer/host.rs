//! Conversation payloads and attachment uploads run in the guest.
use super::{AttachmentState, Send};
use base64::Engine as _;
use duck_address::{Address, ChainId, Refused};
use ducktape_view_guest::host;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedFile {
    pub token: String,
    pub name: String,
    pub bytes: u64,
}
/// Re-exported so a composer site converts a refusal with the same
/// `host::said` its neighbours use, rather than spelling the guest crate out.
pub use ducktape_view_guest::host::said;
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Clipboard {
    pub text: String,
    pub files: Vec<SelectedFile>,
}
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

pub async fn pick() -> Result<Vec<SelectedFile>, host::Refusal> {
    let bytes = host::request("fs.pick", b"{}").await?;
    serde_json::from_slice(&bytes).map_err(|error| host::malformed(error.to_string()))
}
pub async fn clipboard() -> Result<Clipboard, host::Refusal> {
    let bytes = host::request("clipboard.read", b"{}").await?;
    serde_json::from_slice(&bytes).map_err(|error| host::malformed(error.to_string()))
}
pub async fn copy(text: &str) -> Result<(), host::Refusal> {
    host::request("clipboard.write", text.as_bytes())
        .await
        .map(|_| ())
}
pub async fn release(token: &str) {
    let _ = host::request("fs.release", token.as_bytes()).await;
}

/// The absolute duckfs path a `duck://<chain>/files/…` link names.
pub fn address_path(link: &str) -> Result<String, Refused> {
    let address = Address::parse(link)?;
    if address.module != "files" || address.path.is_empty() {
        return Err(Refused::new(
            "invalid_input",
            "A file address must name at least one path segment.",
        ));
    }
    let path = format!("/{}", address.path.join("/"));
    canonical_path(&path).map_err(|why| {
        Refused::new(
            "invalid_input",
            format!("A file address names a duckfs path, and `{path}` is not one: {why}."),
        )
    })?;
    Ok(path)
}

/// The duckfs file address for a canonical path on `chain`.
pub fn file_address(chain: &str, path: &str) -> Result<String, Refused> {
    let chain: ChainId = chain.parse()?;
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = format!("/{path}");
    let segments = canonical_path(&path).map_err(|why| {
        Refused::new(
            "invalid_input",
            format!("A file address names a duckfs path, and `{path}` is not one: {why}."),
        )
    })?;
    Address::new(chain, "files", segments).map(|address| address.to_string())
}
pub async fn id() -> Result<String, host::Refusal> {
    let bytes = host::request("host.id", b"message").await?;
    String::from_utf8(bytes).map_err(|error| host::malformed(error.to_string()))
}

pub fn body(send: &Send) -> String {
    let mut lines: Vec<String> = send.body.lines().map(str::to_owned).collect();
    for file in &send.attachments {
        if let AttachmentState::Ready { uri } = &file.state {
            lines.push(format!("[{}]({uri})", safe_name(&file.name)));
        }
    }
    lines.join("\n")
}
fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '/') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

pub async fn submit(id: String, send: &Send, target: &Target) -> Result<(), host::Refusal> {
    let body = body(send);
    let invalid_body = body.is_empty() || body.len() > 16 * 1024;
    if invalid_body {
        return Err(host::Refusal::new(
            "invalid_body",
            "Message must contain between 1 byte and 16 KiB",
        ));
    }
    let blocks = super::message::parse_message(&body);
    let payload = match target {
        Target::Post { channel, thread } => {
            serde_json::json!({"post_message": {"channel_id":channel, "message_id":id, "blocks":blocks, "thread":thread}})
        }
        Target::Edit {
            channel,
            seq,
            base_rev,
        } => {
            serde_json::json!({"edit_message": {"channel_id":channel,"seq":seq,"blocks":blocks,"base_rev":base_rev}})
        }
    };
    let ask = serde_json::json!({"target":"chat", "payload":payload});
    host::request(
        "op.submit",
        &serde_json::to_vec(&ask).expect("message envelope"),
    )
    .await
    .map(|_| ())
}

/// Uploads `file` and answers the address every member opens it by, on
/// `chain` (the view's `<label>#<salt>`). The address is built FIRST: a name
/// or a chain that has no address is refused before a byte is stored, so a
/// refusal never leaves an attachment nobody can link to.
pub async fn upload(file: SelectedFile, chain: String) -> Result<String, host::Refusal> {
    let attachment_id = String::from_utf8(host::request("host.id", b"attachment").await?)
        .map_err(|error| host::malformed(error.to_string()))?;
    let path = format!(
        "/shared/attachments/{attachment_id}/{}",
        safe_name(&file.name)
    );
    let address = file_address(&chain, &path)
        .map_err(|refused| host::Refusal::new(refused.reason, refused.sentence))?;
    upload_inner(&file, path).await?;
    release(&file.token).await;
    Ok(address)
}

const CHUNK_SIZE: u64 = 1024 * 1024;
const MAX_INLINE_COMMIT_BYTES: u64 = 256 * 1024;
const MAX_UPLOAD: u64 = 64 << 20;
const MAX_NAME_BYTES: usize = 255;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DEPTH: usize = 128;

fn canonical_path(path: &str) -> Result<Vec<String>, String> {
    if !path.starts_with('/') {
        return Err("path must be absolute (start with '/')".to_owned());
    }
    if path.chars().nfc().collect::<String>() != path {
        return Err("path is not NFC-normalized".to_owned());
    }
    if path.contains('\0') {
        return Err("path must not contain a NUL byte".to_owned());
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(format!(
            "path exceeds the {MAX_PATH_BYTES}-byte length limit"
        ));
    }
    if path == "/" {
        return Ok(Vec::new());
    }
    let mut segments = Vec::new();
    for segment in path[1..].split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("path contains an empty or dot segment".to_owned());
        }
        if segment.len() > MAX_NAME_BYTES {
            return Err(format!(
                "segment name exceeds the {MAX_NAME_BYTES}-byte limit"
            ));
        }
        segments.push(segment.to_owned());
    }
    if segments.len() > MAX_DEPTH {
        return Err(format!("path exceeds the maximum depth of {MAX_DEPTH}"));
    }
    Ok(segments)
}

async fn upload_inner(file: &SelectedFile, path: String) -> Result<(), host::Refusal> {
    if file.bytes > MAX_UPLOAD {
        return Err(host::Refusal::new(
            "too_large",
            "Files must be at most 64 MiB",
        ));
    }
    canonical_path(&path).map_err(|said| host::Refusal::new("invalid_path", said))?;
    let ask = serde_json::json!({"target":"files", "query":{"refs":{}}});
    let refs: serde_json::Value = serde_json::from_slice(
        &host::request("rpc.query", &serde_json::to_vec(&ask).expect("refs query")).await?,
    )
    .map_err(|error| host::malformed(error.to_string()))?;
    let mut chunks = Vec::new();
    let mut chunk = Vec::new();
    let mut offset = 0u64;
    let inline = file.bytes <= MAX_INLINE_COMMIT_BYTES;
    while offset < file.bytes {
        let len = (file.bytes - offset).min(256 << 10) as usize;
        let ask = serde_json::json!({"token":file.token, "offset":offset, "len":len});
        let bytes = host::request(
            "fs.read",
            &serde_json::to_vec(&ask).expect("file read envelope"),
        )
        .await?;
        if bytes.is_empty() || bytes.len() > len {
            return Err(host::Refusal::new(
                "file_changed",
                "The selected file changed during its upload",
            ));
        }
        offset += bytes.len() as u64;
        chunk.extend_from_slice(&bytes);
        let chunk_ready = !inline && (chunk.len() as u64 == CHUNK_SIZE || offset == file.bytes);
        if chunk_ready {
            submit_bytes("files", encode_putblob(&chunk)).await?;
            chunks.push(hex(&object_id(0, &chunk)));
            chunk.clear();
        }
    }
    let content = if inline {
        Content::Inline {
            b64: base64::engine::general_purpose::STANDARD.encode(chunk),
        }
    } else {
        Content::Chunks {
            size: file.bytes,
            chunks,
        }
    };
    let commit = FilesMsg::Commit {
        base_snapshot: refs["refs"]["head"].as_str().map(str::to_owned),
        message: format!("upload {}", file.name),
        changes: vec![Change::Put {
            path,
            exec: false,
            meta: BTreeMap::new(),
            content,
        }],
    };
    submit_bytes("files", serde_json::to_vec(&commit).expect("files message")).await
}

async fn submit_bytes(target: &str, bytes: Vec<u8>) -> Result<(), host::Refusal> {
    let ask = serde_json::json!({
        "target": target,
        "body_b64": base64::engine::general_purpose::STANDARD.encode(bytes)
    });
    host::request(
        "op.submit_bytes",
        &serde_json::to_vec(&ask).expect("binary submit envelope"),
    )
    .await
    .map(|_| ())
}

fn encode_putblob(bytes: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(bytes.len() + 1);
    encoded.push(0);
    encoded.extend_from_slice(bytes);
    encoded
}

fn object_id(kind: u8, body: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update([kind]);
    digest.update(body);
    digest.finalize().into()
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum FilesMsg {
    Commit {
        base_snapshot: Option<String>,
        message: String,
        changes: Vec<Change>,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Change {
    Put {
        path: String,
        exec: bool,
        meta: BTreeMap<String, String>,
        content: Content,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Content {
    Inline { b64: String },
    Chunks { size: u64, chunks: Vec<String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_addresses_keep_the_producer_spelling() {
        let address = file_address("testnet#0a1b2c3d", "/shared/보고서 Final.pdf").unwrap();
        assert_eq!(
            address,
            "duck://testnet-0a1b2c3d/files/shared/%EB%B3%B4%EA%B3%A0%EC%84%9C%20Final.pdf"
        );
        assert_eq!(address_path(&address).unwrap(), "/shared/보고서 Final.pdf");
    }

    #[test]
    fn file_policy_rejects_paths_the_producer_cannot_hold() {
        assert!(file_address("", "/shared/a.md").is_err());
        assert!(
            file_address("testnet#0a1b2c3d", "/shared/e\u{301}.md")
                .unwrap_err()
                .sentence
                .contains("NFC")
        );
        for old in [
            "duck://files/shared/a.md",
            "duck://testnet-0a1b2c3d/pages/a",
        ] {
            assert!(address_path(old).is_err(), "{old}");
        }
    }

    #[test]
    fn file_commit_and_putblob_bytes_keep_the_producer_shape() {
        assert_eq!(encode_putblob(b"abc"), [0, b'a', b'b', b'c']);
        let commit = FilesMsg::Commit {
            base_snapshot: None,
            message: "upload hello.txt".into(),
            changes: vec![Change::Put {
                path: "/shared/hello.txt".into(),
                exec: false,
                meta: BTreeMap::new(),
                content: Content::Inline { b64: "YWJj".into() },
            }],
        };
        assert_eq!(
            serde_json::to_value(commit).unwrap(),
            serde_json::json!({
                "commit": {
                    "base_snapshot": null,
                    "message": "upload hello.txt",
                    "changes": [{
                        "put": {
                            "path": "/shared/hello.txt",
                            "exec": false,
                            "meta": {},
                            "content": {"inline": {"b64": "YWJj"}}
                        }
                    }]
                }
            })
        );
    }
}
