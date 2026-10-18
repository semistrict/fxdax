// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A store a daemon served opens in another process at once once its VM process goes, as a host's
//! check of a guest's crash opens it: whole, as of its last durable commit. The daemon closes the
//! store as the VM process goes, flushing it, and the other process's opening waits for that,
//! as opening a branch another process has open does (`extent_store::Dir::take`), rather than
//! recover the store from under it.

#![cfg(target_os = "linux")]

use fxdax::{ExtentId, ExtentStore, RemoteExtentStore, VolatileSlice};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn stores_open_whole_in_another_process_once_their_vm_goes() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let daemon = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"))
        .arg(&socket)
        .env("FXDAX_EXTENTD_LOCAL_ONLY", "1")
        .env_remove("FXDAX_EXTENTD_REMOTE")
        .spawn()
        .unwrap();
    let _daemon = KillOnDrop(daemon);
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(Instant::now() < deadline, "the daemon never served");
        std::thread::sleep(Duration::from_millis(20));
    }
    let path = dir.path().join("store");
    let mut sequence = 1;
    for round in 0..50 {
        let store = RemoteExtentStore::connect(&socket, &path, 1 << 28).unwrap();
        let mut root = Vec::new();
        for commit in 0..20 {
            for _ in 0..4 {
                let extent = ExtentId { branch: store.branch(), sequence };
                sequence += 1;
                let data = vec![(round * 20 + commit) as u8; 3 * 4096];
                store.write(extent, 0, &[VolatileSlice::from(&data[..])]).unwrap();
            }
            root = format!("round {round} commit {commit}").into_bytes();
            store.commit(Some(&root)).unwrap();
        }
        drop(store);
        let opened = ExtentStore::open(&path).unwrap();
        assert_eq!(opened.root().unwrap(), root, "round {round}");
        sequence = opened.info().unwrap().next_sequence.max(sequence);
    }
}
