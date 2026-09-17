//! No view declares a link opener of its own: the kernel's is the only door out.
//!
//! [`ducktape_view_guest::host::open_link`] sends the request kind
//! `host.open_link`, and the app answers it from one table for every view —
//! same link plane, same refusal, one place to audit where a click can send
//! somebody. A view that named an opener under its own module instead would
//! ask the app to grow a second opener per module, each with its own idea of
//! what a `duck://` address means, and the audit would be as wide as the
//! registry.
//!
//! So the rule is about the request kind a view NAMES, not about who calls
//! whom: wherever this workspace's source writes one out — a string literal,
//! a doc comment's code span — it reads `host.open_link`. Prose counts because
//! prose is where a second door gets proposed before anyone writes it, and a
//! comment promising a module-owned opener is already the wrong design claim.
//! This file obeys its own rule, so it names the two shapes by escape.
//!
//! This rule belongs to the repo that holds the source it is about: the app
//! tests the door tables it serves (ducktape-app#8), this tests the views.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The tail of a request kind that asks for a link to be opened, in the two
/// shapes source writes one in: a string literal and a doc comment's code
/// span. Each terminator is escaped so this file names no opener itself.
const OPENER_KINDS: [&str; 2] = [".open_link\"", ".open_link\u{60}"];

#[test]
fn no_view_declares_its_own_open_link() {
    let views_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the views workspace root resolves");

    let mut found = BTreeSet::new();
    for member in members(&views_root) {
        for source in rust_sources(&member) {
            let text = fs::read_to_string(&source)
                .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
            for (number, line) in text.lines().enumerate() {
                for kind in OPENER_KINDS {
                    for (index, _) in line.match_indices(kind) {
                        if opener(&line[..index]) == "host" {
                            continue;
                        }
                        let path = source.strip_prefix(&views_root).unwrap_or(&source);
                        found.insert(format!(
                            "{}:{}: {}",
                            path.display(),
                            number + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }

    assert!(
        found.is_empty(),
        "views name a link opener that is not the kernel's: {found:#?}\n\
         a view leaves through the one door the app already answers — \
         `host::open_link`, which sends `host.open_link`",
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

/// The capability a kind belongs to: the identifier the text ends on.
fn opener(before_kind: &str) -> &str {
    before_kind
        .rsplit(|character: char| !character.is_alphanumeric() && character != '_')
        .next()
        .unwrap_or_default()
}
