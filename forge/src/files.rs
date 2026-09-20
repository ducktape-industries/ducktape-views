//! Forge-owned attachment policy and duckfs producer codec.
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

const CHUNK_SIZE: u64 = 1024 * 1024;
const MAX_INLINE_COMMIT_BYTES: usize = 256 * 1024;
const MAX_UPLOAD: u64 = 64 << 20;
const MAX_NAME_BYTES: usize = 255;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DEPTH: usize = 128;

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

fn canonical(path: &str) -> Result<Vec<String>, String> {
    if !path.starts_with('/') {
        return Err("files: path must be absolute (start with '/')".into());
    }
    if path.chars().nfc().collect::<String>() != path {
        return Err("files: path is not NFC-normalized".into());
    }
    if path.contains('\0') {
        return Err("files: path must not contain a NUL byte".into());
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(format!(
            "files: path exceeds the {MAX_PATH_BYTES}-byte length limit"
        ));
    }
    if path == "/" {
        return Ok(Vec::new());
    }
    let mut segments = Vec::new();
    for segment in path[1..].split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("files: path contains an empty or dot segment".into());
        }
        if segment.len() > MAX_NAME_BYTES {
            return Err(format!(
                "files: segment name exceeds the {MAX_NAME_BYTES}-byte limit"
            ));
        }
        segments.push(segment.to_owned());
    }
    if segments.len() > MAX_DEPTH {
        return Err(format!(
            "files: path exceeds the maximum depth of {MAX_DEPTH}"
        ));
    }
    Ok(segments)
}

/// The address the file upload's producer commits under.
pub fn file_address(chain: &str, path: &str) -> Result<String, Refused> {
    let chain: ChainId = chain.parse()?;
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = format!("/{path}");
    canonical(&path).map_err(|why| {
        Refused::new(
            "invalid_input",
            format!("A file address names a duckfs path, and `{path}` is not one: {why}."),
        )
    })?;
    Ok(Address::new(
        chain,
        "files",
        path[1..].split('/').map(str::to_owned).collect(),
    )?
    .to_string())
}

pub async fn upload(file: SelectedFile, path: String) -> Result<(), host::Refusal> {
    let result = upload_inner(&file, path).await;
    if result.is_ok() {
        release(&file.token).await;
    }
    result
}

pub async fn release(token: &str) {
    let _ = host::request("fs.release", token.as_bytes()).await;
}

async fn upload_inner(file: &SelectedFile, path: String) -> Result<(), host::Refusal> {
    if file.bytes > MAX_UPLOAD {
        return Err(host::Refusal::new(
            "too_large",
            "Files must be at most 64 MiB",
        ));
    }
    canonical(&path).map_err(|said| host::Refusal::new("invalid_path", said))?;
    let ask = serde_json::json!({"target":"files", "query":{"refs":{}}});
    let refs: serde_json::Value = serde_json::from_slice(
        &host::request("rpc.query", &serde_json::to_vec(&ask).expect("refs query")).await?,
    )
    .map_err(|error| host::malformed(error.to_string()))?;
    let mut chunks = Vec::new();
    let mut chunk = Vec::new();
    let mut offset = 0u64;
    let inline = file.bytes <= MAX_INLINE_COMMIT_BYTES as u64;
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
            chunks.push(object_id_hex(&chunk));
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

fn encode_putblob(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    out.push(0);
    out.extend_from_slice(bytes);
    out
}

fn object_id_hex(chunk: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update([0]);
    hash.update(chunk);
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_and_its_address_are_one_another() {
        let address = file_address("testnet#0a1b2c3d", "/shared/보고서 Final.pdf").unwrap();
        assert_eq!(
            address,
            "duck://testnet-0a1b2c3d/files/shared/%EB%B3%B4%EA%B3%A0%EC%84%9C%20Final.pdf"
        );
    }

    #[test]
    fn file_address_fixture_matches_the_producer_spelling() {
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/file-address.json")).unwrap();
        let uri = file_address("testnet#0a1b2c3d", "/shared/notes.txt").unwrap();
        assert_eq!(uri, expected["uri"]);
    }

    #[test]
    fn no_chain_or_noncanonical_path_has_no_address() {
        assert!(file_address("", "/shared/a.md").is_err());
        let refused = file_address("testnet#0a1b2c3d", "/shared/e\u{301}.md").unwrap_err();
        assert!(refused.sentence.contains("NFC"), "{}", refused.sentence);
    }

    #[test]
    fn chunk_id_and_putblob_keep_the_producer_bytes() {
        let chunk = [9, 8, 7];
        assert_eq!(encode_putblob(&chunk), [0, 9, 8, 7]);
        assert_eq!(
            object_id_hex(&chunk),
            "5b5ef453f01a1d59d4744eff6399d37c759c9220455eb519a5f738e3ee6e6573"
        );
    }
}
