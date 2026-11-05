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

/// fxdax: a pass over a bucket on the host's filesystem (TASK-165.1), from a process of its own:
/// it reclaims the branch's segments its checkpoint covers, which hold no live extent (its fence,
/// the one holding what it freed, the one freeing it), and lists the branch.
#[test]
fn collects_a_file_bucket() {
    use extent_remote::batch::{Batch, Commit};
    use extent_remote::store::Store;
    use extent_remote::{Writer, branch};
    use std::sync::Arc;
    use storage_device::ExtentId;

    let dir = tempfile::tempdir().unwrap();
    let url = url::Url::from_directory_path(dir.path()).unwrap().to_string();
    futures::executor::block_on(async {
        let bucket = Arc::new(Store::from_url(&url).unwrap());
        branch::create(&*bucket, 1, 0).await.unwrap();
        let writer = Writer::open(bucket, 1, 10, 1).await.unwrap();
        let extent = ExtentId::new(1, writer.chain().next_sequence());
        let written =
            Commit { tag: 1, extents: vec![(extent, vec![7; 4096])], ..Commit::default() };
        let freed = Commit { tag: 2, frees: vec![extent], ..Commit::default() };
        for commit in [written, freed] {
            let segment = Batch::from(commit).close(&mut writer.chain()).frame();
            let seq = writer.add_segment(segment).unwrap();
            writer.upload(seq).await.unwrap();
            writer.acknowledge().await.unwrap();
        }
        writer.checkpoint().await.unwrap();
    });
    let out = gc(&[&url, "--grace", "0"]);
    assert_eq!(String::from_utf8_lossy(&out.stderr), "fxdax-gc: deleted 3, compacted 0\n");
    assert!(out.status.success());
    let left = futures::executor::block_on(async {
        use extent_remote::Bucket as _;
        let bucket = Store::from_url(&url).unwrap();
        let listed = bucket.list("").await.unwrap();
        listed
            .into_iter()
            .map(|meta| meta.key)
            .filter(|key| !key.starts_with("gc/"))
            .collect::<Vec<_>>()
    });
    let left: Vec<&str> = left.iter().map(String::as_str).collect();
    let branch = "0000000000000001";
    let kept = [
        format!("branches/{branch}"),
        format!("checkpoints/{branch}/0000000000000002"),
        format!("leases/{branch}"),
    ];
    assert_eq!(left, kept);
    let out = gc(&[&url, "--branches"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");
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
