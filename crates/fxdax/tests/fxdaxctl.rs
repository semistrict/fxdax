// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of fxdaxctl, fxdax's tool for VMs' disks, on stores no daemon serves.

use fxdax::{ExtentId, ExtentStore, VolatileSlice, VolatileSliceMut};
use std::path::Path;
use std::process::Command;

const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

/// Runs fxdaxctl with `args`: what it prints, or, if it fails, what it says.
fn fxdaxctl(args: &[&Path]) -> Result<String, String> {
    let output = Command::new(env!("CARGO_BIN_EXE_fxdaxctl"))
        .args(args)
        .env_remove("FXDAX_EXTENTD")
        .env_remove("FXDAX_EXTENTD_REMOTE")
        .output()
        .expect("running fxdaxctl failed");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    match output.status.success() {
        true => Ok(String::from_utf8(output.stdout).unwrap()),
        false => Err(stderr),
    }
}

fn read(store: &ExtentStore, extent: ExtentId) -> Vec<u8> {
    let mut data = vec![0; BLOCK];
    store.read(extent, 0, &mut [VolatileSliceMut::from(&mut data[..])]).unwrap();
    data
}

/// TASK-39.4: `fxdaxctl delete` deletes a store whose fork lives, which keeps what it shares of
/// it, and deletes the store's directory with the fork.
#[test]
fn stores_are_deleted_while_their_forks_live() {
    let dir = tempfile::tempdir().unwrap();
    let (path, fork) = (dir.path().join("store"), dir.path().join("fork"));
    let store = ExtentStore::create(&path, 1 << 24).unwrap();
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    store.write(extent, 0, &[VolatileSlice::from(&[7; BLOCK][..])]).unwrap();
    store.commit(None).unwrap();
    drop(store);
    let printed = fxdaxctl(&[Path::new("fork"), &path, &fork]).unwrap();
    let branch = u64::from_str_radix(printed.trim(), 16).unwrap();

    fxdaxctl(&[Path::new("delete"), &path]).unwrap();
    assert!(path.exists(), "the store's directory went while its fork lives");
    let error = ExtentStore::open(&path).err().expect("opened a deleted store");
    assert_eq!(
        format!("{error:#}"),
        "the store was deleted: its directory holds its forks' data alone"
    );
    let forked = ExtentStore::open(&fork).unwrap();
    assert_eq!(forked.branch(), branch);
    assert_eq!(read(&forked, extent), [7; BLOCK]);
    let own = ExtentId { branch, sequence: 1 };
    forked.write(own, 0, &[VolatileSlice::from(&[9; BLOCK][..])]).unwrap();
    forked.commit(None).unwrap();
    drop(forked);
    let error = fxdaxctl(&[Path::new("delete"), &path]).expect_err("deleted a store twice");
    assert!(error.contains("the store was deleted"), "{error}");

    fxdaxctl(&[Path::new("delete"), &fork]).unwrap();
    assert!(!fork.exists() && !path.exists());
}
