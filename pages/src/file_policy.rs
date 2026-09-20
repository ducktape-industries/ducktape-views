//! Pages' file-address and upload policy. This is deliberately local to the
//! view: file-domain rules do not travel through a sibling view crate.

use std::collections::BTreeMap;

use base64::Engine as _;
use duck_address::{Address, ChainId, Refused};
use ducktape_view_guest::host;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use unicode_normalization::UnicodeNormalization;

const CHUNK_SIZE: u64 = 1024 * 1024;
const MAX_INLINE_COMMIT_BYTES: usize = 256 * 1024;
const MAX_UPLOAD: u64 = 64 << 20;
const MAX_NAME_BYTES: usize = 255;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DEPTH: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SelectedFile {
    pub token: String,
    pub name: String,
    pub bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Content {
    Inline { b64: String },
    Chunks { size: u64, chunks: Vec<String> },
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
enum FilesMsg {
    Commit {
        base_snapshot: Option<String>,
        message: String,
        changes: Vec<Change>,
    },
}

fn canonical(path: &str) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err("path must be absolute (start with '/')".to_string());
    }
    if path.chars().nfc().collect::<String>() != path {
        return Err("path is not NFC-normalized".to_string());
    }
    if path.contains('\0') {
        return Err("path must not contain a NUL byte".to_string());
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(format!(
            "path exceeds the {MAX_PATH_BYTES}-byte length limit"
        ));
    }
    if path == "/" {
        return Ok(());
    }
    let mut depth = 0;
    for segment in path[1..].split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("path contains an empty or dot segment".to_string());
        }
        if segment.len() > MAX_NAME_BYTES {
            return Err(format!(
                "segment name exceeds the {MAX_NAME_BYTES}-byte limit"
            ));
        }
        depth += 1;
    }
    if depth > MAX_DEPTH {
        return Err(format!("path exceeds the maximum depth of {MAX_DEPTH}"));
    }
    Ok(())
}

/// The duckfs path named by a `duck://<chain>/files/<path…>` address.
pub(crate) fn address_path(address: &str) -> Result<String, Refused> {
    let address = Address::parse(address)?;
    if address.module != "files" || address.path.is_empty() {
        return Err(Refused::new(
            "invalid_input",
            "A file address must name at least one path segment.",
        ));
    }
    let path = format!("/{}", address.path.join("/"));
    canonical(&path).map_err(|why| {
        Refused::new(
            "invalid_input",
            format!("A file address names a duckfs path, and `{path}` is not one: {why}."),
        )
    })?;
    Ok(path)
}

/// The canonical address for a duckfs path on `chain`.
pub(crate) fn file_address(chain: &str, path: &str) -> Result<String, Refused> {
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

async fn submit_bytes(target: &str, bytes: Vec<u8>) -> Result<(), host::Refusal> {
    let ask = serde_json::json!({
        "target": target,
        "body_b64": base64::engine::general_purpose::STANDARD.encode(bytes),
    });
    host::request(
        "op.submit_bytes",
        &serde_json::to_vec(&ask).expect("binary submit envelope"),
    )
    .await
    .map(|_| ())
}

pub(crate) async fn upload(file: SelectedFile, path: String) -> Result<(), host::Refusal> {
    let result = upload_inner(&file, path).await;
    if result.is_ok() {
        release(&file.token).await;
    }
    result
}

pub(crate) async fn release(token: &str) {
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
            chunks.push(chunk_object_id(&chunk));
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
    submit_bytes("files", serde_json::to_vec(&commit).expect("files commit")).await
}

fn encode_putblob(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + bytes.len());
    out.push(0);
    out.extend_from_slice(bytes);
    out
}

fn chunk_object_id(chunk: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update([0]);
    hasher.update(chunk);
    to_hex(&hasher.finalize())
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_addresses_round_trip_and_preserve_producer_spelling() {
        let address = file_address("testnet#0a1b2c3d", "/shared/보고서 Final.pdf").unwrap();
        assert_eq!(
            address,
            "duck://testnet-0a1b2c3d/files/shared/%EB%B3%B4%EA%B3%A0%EC%84%9C%20Final.pdf"
        );
        assert_eq!(address_path(&address).unwrap(), "/shared/보고서 Final.pdf");
    }

    #[test]
    fn upload_codecs_keep_the_file_producer_bytes() {
        assert_eq!(encode_putblob(&[1, 2, 255]), [0, 1, 2, 255]);
        assert_eq!(
            chunk_object_id(&[1, 2, 255]),
            "3d1f57c984978ef98a18378c8166c1cb8ede02c03eeb6aee7e2f121dfeee3e56"
        );
        let commit = FilesMsg::Commit {
            base_snapshot: Some("11".repeat(32)),
            message: "upload README.md".into(),
            changes: vec![Change::Put {
                path: "/shared/README.md".into(),
                exec: false,
                meta: BTreeMap::new(),
                content: Content::Inline {
                    b64: "bWluZQ==".into(),
                },
            }],
        };
        assert_eq!(
            serde_json::to_vec(&commit).unwrap(),
            br#"{"commit":{"base_snapshot":"1111111111111111111111111111111111111111111111111111111111111111","message":"upload README.md","changes":[{"put":{"path":"/shared/README.md","exec":false,"meta":{},"content":{"inline":{"b64":"bWluZQ=="}}}}]}}"#
        );
    }

    #[test]
    fn invalid_paths_and_addresses_are_refused() {
        assert!(file_address("", "/shared/a.md").is_err());
        let refused = file_address("testnet#0a1b2c3d", "/shared/e\u{301}.md").unwrap_err();
        assert!(refused.sentence.contains("NFC"), "{}", refused.sentence);
        for old in [
            "duck://files/shared/a.md",
            "duck://testnet-0a1b2c3d/pages/a",
        ] {
            assert!(address_path(old).is_err(), "{old}");
        }
    }
}
