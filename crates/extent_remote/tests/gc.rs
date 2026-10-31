// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax-gc, the program: a pass over a bucket, and its usage.

use std::process::Command;

fn gc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fxdax-gc")).args(args).output().unwrap()
}

/// A pass over a bucket (in memory: empty) reports what it did, and exits.
#[test]
fn collects_a_bucket_once() {
    let out = gc(&["memory:///", "--grace", "1"]);
    assert_eq!(String::from_utf8_lossy(&out.stderr), "fxdax-gc: deleted 0, compacted 0\n");
    assert!(out.status.success());
}

/// TASK-79.85: a bucket (in memory: empty) has no live branches, and a branch is deleted, as an
/// operator's commands do them.
#[test]
fn lists_and_deletes_branches() {
    let out = gc(&["memory:///", "--branches"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert!(out.status.success());
    let out = gc(&["memory:///", "--delete-branch", "ff"]);
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert!(out.status.success());
}

#[test]
fn without_a_bucket_prints_its_usage() {
    let out = gc(&[]);
    let usage = "usage: fxdax-gc URL [--every SECONDS] [--grace SECONDS] \
                 [--compact-below FRACTION] | fxdax-gc URL --branches [--held FILE] | \
                 fxdax-gc URL --delete-branch ID";
    assert_eq!(String::from_utf8_lossy(&out.stderr), format!("Error: {usage:?}\n"));
    assert_eq!(out.status.code(), Some(1));
}
