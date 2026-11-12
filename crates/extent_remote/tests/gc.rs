// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax-gc, the program: a pass over a bucket, and its usage.

use std::process::Command;

/// Branch `id` of tenant 1's cell 1, the tests'.
fn test_branch(id: u64) -> extent_remote::layout::Branch {
    extent_remote::layout::Branch { cell: extent_remote::layout::Cell { tenant: 1, id: 1 }, id }
}

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
    let out = gc(&["memory:///", "--delete-branch", "1/1/ff"]);
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert!(out.status.success());
}

/// fxdax: a pass over a bucket on the host's filesystem (TASK-165.1), from a process of its own:
/// it reclaims the branch's segments its checkpoint covers, which hold no live extent (the one
/// holding what it freed, the one freeing it), but its fence, which it keeps (TASK-371), and lists
/// the branch.
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
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let writer = Writer::open(bucket, test_branch(1), 10, 1).await.unwrap();
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
    assert_eq!(String::from_utf8_lossy(&out.stderr), "fxdax-gc: deleted 2, compacted 0\n");
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
    let at = "tenants/0000000000000001/cells/0000000000000001";
    let branch = "0000000000000001";
    // The writer's fence too, which a writer taken over must find (TASK-371).
    let kept = [
        format!("{at}/branches/{branch}"),
        format!("{at}/checkpoints/{branch}/0000000000000002"),
        format!("{at}/leases/{branch}"),
        format!("{at}/segments/{branch}/0000000000000000"),
    ];
    assert_eq!(left, kept);
    let out = gc(&[&url, "--branches"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1/1/1\n");
}

/// TASK-264: a dry run over a bucket on the host's filesystem lists what a pass would delete,
/// writing nothing, the collector's lease neither, nor the probe of its conditional writes; a
/// mark-only pass marks it, deleting nothing; a pass after deletes it, leaving the marks.
#[test]
fn dry_runs_and_mark_only_passes_delete_nothing() {
    use extent_remote::batch::{Batch, Commit};
    use extent_remote::store::Store;
    use extent_remote::{Writer, branch};
    use std::sync::Arc;
    use storage_device::ExtentId;

    let dir = tempfile::tempdir().unwrap();
    let url = url::Url::from_directory_path(dir.path()).unwrap().to_string();
    futures::executor::block_on(async {
        let bucket = Arc::new(Store::from_url(&url).unwrap());
        branch::create(&*bucket, test_branch(1), 0).await.unwrap();
        let writer = Writer::open(bucket, test_branch(1), 10, 1).await.unwrap();
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
    let files = || -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = walk(dir.path())
            .into_iter()
            .map(|path| {
                let name = path.strip_prefix(dir.path()).unwrap().display().to_string();
                (name, std::fs::read(&path).unwrap())
            })
            .collect();
        files.sort();
        files
    };
    let before = files();
    let out = gc(&[&url, "--dry-run", "--grace", "0"]);
    let branch = "0000000000000001";
    let segment =
        |seq: u64| format!("tenants/{branch}/cells/{branch}/segments/{branch}/{seq:016x}");
    // Not the fence, which the collector keeps (TASK-371).
    let (fence, holding, freeing) = (segment(0), segment(1), segment(2));
    let listed = String::from_utf8_lossy(&out.stdout).into_owned();
    let sizes: Vec<&str> = listed.lines().map(|line| line.split(' ').nth(2).unwrap()).collect();
    let expected: String = [&holding, &freeing]
        .iter()
        .zip(&sizes)
        .map(|(key, size)| format!("{key} segment {size} delete\n"))
        .collect();
    assert_eq!(listed, expected);
    let bytes: u64 = sizes.iter().map(|size| size.parse::<u64>().unwrap()).sum();
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "fxdax-gc: dry run: a pass would delete 2 objects ({bytes} bytes), and more: it would \
             compact 0 (0 bytes), and delete what it compacts a grace period after, which a dry \
             run, compacting nothing, doesn't count\n"
        )
    );
    assert!(out.status.success());
    assert_eq!(files(), before);

    let out = gc(&[&url, "--mark-only", "--grace", "0"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "fxdax-gc: marked 2 ({bytes} bytes) in gc/marks/0000000000000001, deleted 0, \
             compacted 0, resurrected 0\n"
        )
    );
    let names: Vec<String> = files().into_iter().map(|(name, _)| name).collect();
    for key in [&fence, &holding, &freeing] {
        assert!(names.contains(key), "{key} marked, deleted");
    }
    assert!(names.contains(&"gc/marks/0000000000000001".to_owned()), "{names:?}");

    let out = gc(&[&url, "--grace", "0"]);
    assert_eq!(String::from_utf8_lossy(&out.stderr), "fxdax-gc: deleted 2, compacted 0\n");
    let names: Vec<String> = files().into_iter().map(|(name, _)| name).collect();
    // The bucket's lock, no object, then the objects.
    let at = "tenants/0000000000000001/cells/0000000000000001";
    let kept = [
        extent_remote::local::LOCK.to_owned(),
        "gc/lease".to_owned(),
        "gc/marks/0000000000000001".to_owned(),
        format!("{at}/branches/{branch}"),
        format!("{at}/checkpoints/{branch}/0000000000000002"),
        format!("{at}/leases/{branch}"),
        fence,
    ];
    assert_eq!(names, kept);
}

/// The files under `dir`, at every depth.
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        match path.is_dir() {
            true => files.extend(walk(&path)),
            false => files.push(path),
        }
    }
    files
}

#[test]
fn without_a_bucket_prints_its_usage() {
    let out = gc(&[]);
    let usage = "usage: fxdax-gc URL [--mark-only] [--every SECONDS] [--grace SECONDS] \
                 [--compact-below FRACTION] | fxdax-gc URL --dry-run [--grace SECONDS] \
                 [--compact-below FRACTION] | fxdax-gc URL --branches [--held FILE] | \
                 fxdax-gc URL --delete-branch TENANT/CELL/ID (--mark-only deletes nothing, writing \
                 gc/marks/EPOCH: the bucket grows; --dry-run writes nothing, and finds less than \
                 a pass deletes)";
    assert_eq!(String::from_utf8_lossy(&out.stderr), format!("Error: {usage:?}\n"));
    assert_eq!(out.status.code(), Some(1));
}
