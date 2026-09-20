//! No path dependency escapes this workspace, or the build fails.
//!
//! The views workspace is its own cargo root, so cargo hashes the ABSOLUTE
//! location of a path dependency into that package's `-C metadata` whenever the
//! package sits outside the root — and `-C metadata` seeds every symbol hash the
//! package emits. So each escape is a crate whose compiled bytes move with the
//! checkout, which is the whole reason `ops/build-views.sh` compiles this
//! workspace through one constant path.
//!
//! Each escape also costs the views lock the escaping package and its entire
//! graph. Where it lands depends on the table: a `dependencies` escape is linked
//! into the wasm component a view ships, a `dev-dependencies` one reaches only
//! the native test binary — `cargo build` never links it.
//!
//! Since this repo stands on its own, nothing may cross. Every crate the views
//! share with the rest of ducktape arrives as a git dependency on ducktape-sdk,
//! which pins by revision and carries no absolute path, so a `path =` pointing
//! out of this checkout is always the mistake and always fails here.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn no_path_dependency_escapes_the_workspace() {
    let views_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the views workspace root resolves");

    let mut found = BTreeSet::new();
    for manifest in manifests(&views_root) {
        let directory = manifest.parent().expect("a manifest has a directory");
        let text = fs::read_to_string(&manifest)
            .unwrap_or_else(|error| panic!("read {}: {error}", manifest.display()));
        let mut table = String::new();
        for line in text.lines().map(str::trim) {
            if let Some(header) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                table = header.to_string();
                continue;
            }
            let (table, keyed_name) = match table.rsplit_once('.') {
                Some((head, name)) if head.ends_with("dependencies") => (head, Some(name)),
                _ => (table.as_str(), None),
            };
            let declares_dependency = table.ends_with("dependencies") && !line.starts_with('#');
            if !declares_dependency {
                continue;
            }
            let Some(relative) = path_value(line) else {
                continue;
            };
            let dependency = directory
                .join(relative)
                .canonicalize()
                .unwrap_or_else(|error| {
                    panic!(
                        "{} names a missing path dependency {relative}: {error}",
                        manifest.display()
                    )
                });
            let escapes = !dependency.starts_with(&views_root);
            if !escapes {
                continue;
            }
            let name = keyed_name
                .map(str::to_string)
                .unwrap_or_else(|| package_name(line));
            found.insert(format!("{table} {name} -> {}", dependency.display()));
        }
    }

    assert!(
        found.is_empty(),
        "path dependencies escape the views workspace root: {found:?}\n\
         a crate shared with the rest of ducktape crosses as a git dependency on \
         ducktape-sdk, not as a path out of this checkout",
    );
}

/// The workspace manifest plus one per member: what cargo itself resolves.
fn manifests(views_root: &Path) -> Vec<PathBuf> {
    let root = views_root.join("Cargo.toml");
    let text = fs::read_to_string(&root).expect("the views workspace manifest reads");
    let members = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("members"))
        .expect("the views workspace lists its members on one line");
    let mut manifests = vec![root];
    manifests.extend(
        members
            .split('"')
            .skip(1)
            .step_by(2)
            .map(|member| views_root.join(member).join("Cargo.toml")),
    );
    manifests
}

/// The `path = "…"` value of an inline dependency line, if it has one.
fn path_value(line: &str) -> Option<&str> {
    let after_key = line.split_once("path")?.1.trim_start().strip_prefix('=')?;
    let quoted = after_key.trim_start().strip_prefix('"')?;
    quoted.split_once('"').map(|(value, _)| value)
}

/// The dependency name an inline line declares: everything left of its `=`.
fn package_name(line: &str) -> String {
    line.split_once('=')
        .map(|(name, _)| name.trim().to_string())
        .unwrap_or_else(|| line.to_string())
}
