// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Generates the extent daemon's gRPC API (`proto/fxdax/extentd/v1/extentd.proto`), with protox,
//! which compiles protobuf in Rust: no protoc needed. fxdax: and stamps the build with the commit it
//! is of (`FXDAX_BUILD_COMMIT`, [`crate::BUILD_COMMIT`]), as scripts/build-commit.sh says, which
//! every build of the tree reports (TASK-111.7).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "proto/fxdax/extentd/v1/extentd.proto";
    println!("cargo:rerun-if-changed={proto}");
    let descriptors = protox::compile([proto], ["proto"])?;
    tonic_prost_build::configure().compile_fds(descriptors)?;
    stamp_commit()
}

/// Has the crate's `FXDAX_BUILD_COMMIT` be the commit scripts/build-commit.sh says the tree is
/// of, and the build stamp it again as that changes: as the checkout moves to another commit, or
/// the crate's own sources change, which may make it dirty; or the source a GCE host builds says
/// another (`source-revision.txt`).
fn stamp_commit() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("../..");
    let script = root.join("scripts/build-commit.sh");
    let output = Command::new("bash").arg(&script).arg(&root).output()?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{}: {}: {said}", script.display(), output.status).into());
    }
    let commit = String::from_utf8(output.stdout)?;
    println!("cargo:rustc-env=FXDAX_BUILD_COMMIT={}", commit.trim());
    println!("cargo:rerun-if-env-changed=FXDAX_BUILD_COMMIT");
    println!("cargo:rerun-if-changed={}", script.display());
    // Only where it is: a path missing would have the script run at every build.
    let shipped = root.join("source-revision.txt");
    if shipped.exists() {
        println!("cargo:rerun-if-changed={}", shipped.display());
    }
    println!("cargo:rerun-if-changed=src");
    for path in git_paths(&root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    Ok(())
}

/// The files of `root`'s checkout, if it is one, that change as it moves to another commit: its
/// HEAD, the branch it names, and its index, which a commit writes.
fn git_paths(root: &Path) -> Vec<PathBuf> {
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        output.status.success().then(|| text.trim().to_owned())
    };
    let mut names = vec!["HEAD".to_owned(), "index".to_owned()];
    names.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    (names.iter())
        .filter_map(|name| git(&["rev-parse", "--path-format=absolute", "--git-path", name]))
        .map(PathBuf::from)
        .collect()
}
