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

/// A pass over a bucket (in memory: empty) reports what it did, over no cell, and exits.
#[test]
fn collects_a_bucket_once() {
    let out = gc(&["memory:///", "--grace", "1"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "fxdax-gc: deleted 0, compacted 0, retired 0 references, over 0 cells, 0 failed\n"
    );
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
    // TASK-395.4: a pass over another cell, named, leaves this one be.
    let out = gc(&[&url, "--cell", "1/2", "--grace", "0"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "fxdax-gc: deleted 0, compacted 0, retired 0 references, over 1 cells, 0 failed\n"
    );
    // Over the one cell there: the other's pass, of a cell holding nothing, took no lease there.
    let out = gc(&[&url, "--grace", "0"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "fxdax-gc: deleted 2, compacted 0, retired 0 references, over 1 cells, 0 failed\n"
    );
    assert!(out.status.success());
    let left = futures::executor::block_on(async {
        use extent_remote::Bucket as _;
        let bucket = Store::from_url(&url).unwrap();
        let listed = bucket.list("").await.unwrap();
        listed
            .into_iter()
            .map(|meta| meta.key)
            .filter(|key| !key.ends_with("/gc/lease"))
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
    let out = gc(&[&url, "--branches", "--cell", "1/1"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1/1/1\n");
    let out = gc(&[&url, "--branches", "--cell", "1/2"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
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
             run, compacting nothing, doesn't count; and retire 0 references\n"
        )
    );
    assert!(out.status.success());
    assert_eq!(files(), before);

    let out = gc(&[&url, "--mark-only", "--grace", "0"]);
    let at = "tenants/0000000000000001/cells/0000000000000001";
    let marks = format!("{at}/gc/marks/0000000000000001");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "fxdax-gc: cell 1/1: marked 2 ({bytes} bytes) in {marks}, deleted 0, compacted 0, \
             resurrected 0\n"
        )
    );
    let names: Vec<String> = files().into_iter().map(|(name, _)| name).collect();
    for key in [&fence, &holding, &freeing] {
        assert!(names.contains(key), "{key} marked, deleted");
    }
    assert!(names.contains(&marks), "{names:?}");

    let out = gc(&[&url, "--grace", "0"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "fxdax-gc: deleted 2, compacted 0, retired 0 references, over 1 cells, 0 failed\n"
    );
    let names: Vec<String> = files().into_iter().map(|(name, _)| name).collect();
    // The bucket's lock, no object, then the objects, the collector's under the cell's prefix.
    let kept = [
        extent_remote::local::LOCK.to_owned(),
        // The word that the bucket is marked, which an operator removes with the marks.
        extent_remote::layout::MARKING.to_owned(),
        format!("{at}/branches/{branch}"),
        format!("{at}/checkpoints/{branch}/0000000000000002"),
        format!("{at}/gc/lease"),
        marks,
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

/// TASK-394, TASK-395.8: `--usage` over a bucket on the host's filesystem says what each cell
/// takes, by kind, as the last pass over it found it, which matches a listing of the cell after it:
/// what the branches wrote, less what the pass deleted; when the pass listed it; nothing of a cell
/// no pass went over; and each tenant's sum after its cells. `--tenant` says one tenant's. It
/// writes nothing, the probe of conditional writes neither.
#[test]
fn usage_says_what_each_files_cell_and_tenant_takes_as_passes_found() {
    use extent_remote::Bucket as _;
    use extent_remote::batch::{Batch, Commit};
    use extent_remote::layout::{self, Branch, Cell};
    use extent_remote::store::Store;
    use extent_remote::{Writer, branch};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use storage_device::ExtentId;

    let dir = tempfile::tempdir().unwrap();
    let url = url::Url::from_directory_path(dir.path()).unwrap().to_string();
    let bucket = Arc::new(Store::from_url(&url).unwrap());
    let cells =
        [Cell { tenant: 1, id: 1 }, Cell { tenant: 1, id: 0x2f }, Cell { tenant: 0xb, id: 1 }];
    let unpassed = Cell { tenant: 1, id: 0x30 };
    futures::executor::block_on(async {
        for (id, cell) in cells.into_iter().enumerate() {
            branch::create(&*bucket, Branch { cell, id: id as u64 + 1 }, 0).await.unwrap();
        }
        // In the first cell, a segment written and one freeing it: what the pass deletes.
        let writer = Writer::open(bucket.clone(), Branch { cell: cells[0], id: 1 }, 10, 1);
        let writer = writer.await.unwrap();
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
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let before = now().as_millis() as u64;
    let out = gc(&[&url, "--grace", "0"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "fxdax-gc: deleted 2, compacted 0, retired 0 references, over 3 cells, 0 failed\n"
    );
    let after = now().as_millis() as u64;
    // A cell made after the pass, which no pass went over.
    futures::executor::block_on(branch::create(&*bucket, Branch { cell: unpassed, id: 4 }, 0))
        .unwrap();
    let listed = futures::executor::block_on(bucket.list("")).unwrap();

    // Each cell's objects and bytes by kind, as the bucket lists them now.
    let sums = |cell: Cell| {
        let mut kinds: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for meta in listed.iter().filter(|meta| meta.key.starts_with(&cell.prefix())) {
            let sum = kinds.entry(layout::kind(&meta.key).name()).or_default();
            *sum = (sum.0 + 1, sum.1 + meta.size);
        }
        kinds
    };
    let kinds_json = |kinds: &BTreeMap<&str, (u64, u64)>| -> Value {
        let kind = |&(objects, bytes): &(u64, u64)| json!({ "objects": objects, "bytes": bytes });
        kinds.iter().map(|(name, sum)| (name.to_string(), kind(sum))).collect()
    };
    let total = |kinds: &BTreeMap<&str, (u64, u64)>| {
        kinds.values().fold((0, 0), |(objects, bytes), sum| (objects + sum.0, bytes + sum.1))
    };
    let lines = |out: &std::process::Output| -> Vec<Value> {
        assert_eq!(String::from_utf8_lossy(&out.stderr), "");
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        stdout.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    };
    let out = lines(&gc(&[&url, "--usage"]));
    // What each line says when it was measured: the pass's listing's start, which it went over.
    let measured: Vec<Value> = out.iter().map(|line| line["measured_unix_ms"].clone()).collect();
    for when in [&measured[0], &measured[1], &measured[4]] {
        let when = when.as_u64().unwrap();
        assert!(before <= when && when <= after, "{before} {when} {after}");
    }
    let cell_line = |cell: Cell, when: &Value| {
        let kinds = sums(cell);
        let (objects, bytes) = total(&kinds);
        json!({
            "scope": "cell",
            "tenant": format!("{:x}", cell.tenant),
            "cell": cell.to_string(),
            "measured_unix_ms": when,
            "objects": objects,
            "bytes": bytes,
            "kinds": kinds_json(&kinds),
        })
    };
    let tenant_line = |tenant: u64, of: &[Cell], when: &[&Value], unmeasured: u64| {
        let mut kinds: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for cell in of {
            for (name, sum) in sums(*cell) {
                let summed = kinds.entry(name).or_default();
                *summed = (summed.0 + sum.0, summed.1 + sum.1);
            }
        }
        let (objects, bytes) = total(&kinds);
        let when: Vec<u64> = when.iter().map(|when| when.as_u64().unwrap()).collect();
        json!({
            "scope": "tenant",
            "tenant": format!("{tenant:x}"),
            "cells": of.len() as u64 + unmeasured,
            "measured_cells": of.len(),
            "oldest_measured_unix_ms": when.iter().min(),
            "newest_measured_unix_ms": when.iter().max(),
            "objects": objects,
            "bytes": bytes,
            "kinds": kinds_json(&kinds),
        })
    };
    let nothing = json!({
        "scope": "cell",
        "tenant": "1",
        "cell": "1/30",
        "measured_unix_ms": null,
        "objects": null,
        "bytes": null,
        "kinds": null,
    });
    let tenant_b =
        [cell_line(cells[2], &measured[4]), tenant_line(0xb, &cells[2..], &[&measured[4]], 0)];
    let expected = [
        cell_line(cells[0], &measured[0]),
        cell_line(cells[1], &measured[1]),
        nothing,
        tenant_line(1, &cells[..2], &[&measured[0], &measured[1]], 1),
        tenant_b[0].clone(),
        tenant_b[1].clone(),
    ];
    assert_eq!(out, expected);
    // What the pass deleted is gone from it: the first cell's segments the checkpoint covers.
    assert_eq!(out[0]["kinds"]["segment"]["objects"], 1, "{}", out[0]);
    assert_eq!(out[0]["kinds"]["gc-lease"]["objects"], 1, "{}", out[0]);
    assert_eq!(lines(&gc(&[&url, "--usage", "--tenant", "b"])), tenant_b);
    let after = futures::executor::block_on(bucket.list("")).unwrap();
    assert_eq!(after, listed);
}

#[test]
fn without_a_bucket_prints_its_usage() {
    let out = gc(&[]);
    let usage = "usage: fxdax-gc URL [--cell TENANT/CELL] [--mark-only] [--every SECONDS] \
                 [--grace SECONDS] [--compact-below FRACTION] | fxdax-gc URL [--cell TENANT/CELL] \
                 --dry-run [--grace SECONDS] [--compact-below FRACTION] | fxdax-gc URL --branches \
                 [--cell TENANT/CELL] [--held FILE] | fxdax-gc URL --delete-branch \
                 TENANT/CELL/ID | fxdax-gc URL --usage [--tenant TENANT] (every cell, or the one \
                 --cell names; \
                 --mark-only deletes nothing, writing each cell's gc/marks/EPOCH: the bucket \
                 grows; --dry-run writes nothing, and finds less than a pass deletes)";
    assert_eq!(String::from_utf8_lossy(&out.stderr), format!("Error: {usage:?}\n"));
    assert_eq!(out.status.code(), Some(1));
}
