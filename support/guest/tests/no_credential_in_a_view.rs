//! A credential never crosses into a view: the kernel holds the secret, a
//! guest gets a topic.
//!
//! The kernel reads the node's 0600 link token and attaches it to the socket
//! it opens. What reaches a guest is a topic and the frames on it. A view that
//! named the token instead would be asking for a copy of it inside a WASM
//! sandbox — and a secret that lives in fifteen sandboxes is one guest's bug
//! away from leaving the node, with the 0600 on the file left as decoration.
//!
//! So the rule is about the names a view's SOURCE writes, not about who calls
//! whom: no `.rs` file in this workspace — every crate and its tests, build
//! output aside — may name the kernel's token reader or either of the two
//! dotted credential fields. Plain substrings, because that is how a leak
//! starts: a name typed into a view, then a field read, then a frame. This
//! file obeys its own rule, so it spells the three names by halves.
//!
//! This rule belongs to the repo that holds the source it is about: the app
//! tests the kernel that reads the token (ducktape-app#22), this tests the
//! views.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The three ways a credential is named, each split across a `concat!` so this
/// file contains none of them verbatim and does not trip its own walk.
const CREDENTIAL_NAMES: [&str; 3] = [
    concat!("read_link", "_token"),
    concat!("link", ".token"),
    concat!("admin", ".token"),
];

#[test]
fn no_credential_string_reaches_a_view() {
    let views_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the views workspace root resolves");

    let mut read = 0usize;
    let mut found = BTreeSet::new();
    for member in members(&views_root) {
        for source in rust_sources(&member) {
            read += 1;
            let text = fs::read_to_string(&source)
                .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
            for (number, line) in text.lines().enumerate() {
                for name in CREDENTIAL_NAMES {
                    if line.contains(name) {
                        let path = source.strip_prefix(&views_root).unwrap_or(&source);
                        found.insert(format!("{}:{}: {name}", path.display(), number + 1));
                    }
                }
            }
        }
    }

    assert!(
        read > 0,
        "no view source was read under {}: a walk over nothing proves nothing",
        views_root.display(),
    );
    assert!(
        found.is_empty(),
        "views name a credential the kernel keeps to itself: {found:#?}\n\
         the kernel reads the node's 0600 link token and attaches it to the \
         socket it opens — what crosses into a view is a topic and the frames on it",
    );
}

/// One directory per workspace member: what cargo itself resolves.
fn members(views_root: &Path) -> Vec<PathBuf> {
    let root = views_root.join("Cargo.toml");
    let text = fs::read_to_string(&root).expect("the views workspace manifest reads");
    let members = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("members"))
        .expect("the views workspace lists its members on one line");
    members
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|member| views_root.join(member))
        .collect()
}

/// Every `.rs` file under a directory, build output aside.
fn rust_sources(directory: &Path) -> Vec<PathBuf> {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()));
    let mut sources = Vec::new();
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
            .path();
        if path.is_dir() {
            if path.file_name() != Some("target".as_ref()) {
                sources.extend(rust_sources(&path));
            }
        } else if path.extension() == Some("rs".as_ref()) {
            sources.push(path);
        }
    }
    sources
}
