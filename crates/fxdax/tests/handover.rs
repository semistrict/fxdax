// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A store a daemon served opens in another process at once once its VM process goes, as a host's
//! check of a guest's crash opens it: whole, as of its last commit, durable or only ordered, as a
//! guest's fsyncs are (TASK-26.6.2). The daemon closes the
//! store as the VM process goes, flushing it, and the other process's opening waits for that,
//! as opening a branch another process has open does (`extent_store::Dir::take`), rather than
//! recover the store from under it.

#![cfg(target_os = "linux")]

use fxdax::{ExtentId, ExtentStore, RemoteExtentStore, VolatileSlice, VolatileSliceMut};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A daemon serving in `dir`, and its socket.
fn daemon(dir: &Path) -> (KillOnDrop, PathBuf) {
    let socket = dir.join("extentd.sock");
    let daemon = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"))
        .arg(&socket)
        .env("FXDAX_EXTENTD_REMOTE", "memory:///branches")
        .env("FXDAX_EXTENTD_TOKEN", "1")
        .spawn()
        .unwrap();
    let daemon = KillOnDrop(daemon);
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(Instant::now() < deadline, "the daemon never served");
        std::thread::sleep(Duration::from_millis(20));
    }
    (daemon, socket)
}

#[test]
fn stores_open_whole_in_another_process_once_their_vm_goes() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, socket) = daemon(dir.path());
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

/// A guest's fsyncs are ordered commits, which its host makes durable within its lag: one whose VM
/// goes right after them opens in another process with every one of them, as the daemon makes
/// them durable before letting the store go. The crash test's lost fsyncs (TASK-26.6.2) were its
/// check opening the store before then, which recovered it as of the daemon's last flush.
#[test]
fn ordered_commits_are_whole_in_another_process_once_their_vm_goes() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, socket) = daemon(dir.path());
    let path = dir.path().join("store");
    let mut sequence = 1;
    for round in 0..50u64 {
        let store = RemoteExtentStore::connect(&socket, &path, 1 << 28).unwrap();
        let mut last = Vec::new();
        for commit in 0..20u64 {
            let extent = ExtentId { branch: store.branch(), sequence };
            sequence += 1;
            let data = vec![(round * 20 + commit) as u8; 4096];
            store.write(extent, 0, &[VolatileSlice::from(&data[..])]).unwrap();
            store.commit_ordered(None).unwrap();
            last.push((extent, data));
        }
        drop(store);
        let opened = ExtentStore::open(&path).unwrap();
        for (extent, data) in &last {
            let mut read = vec![0; data.len()];
            let found = opened.read(*extent, 0, &mut [VolatileSliceMut::from(&mut read[..])]);
            assert_eq!(found.map(|()| read), Ok(data.clone()), "round {round}: {extent:?}");
        }
        sequence = opened.info().unwrap().next_sequence.max(sequence);
    }
}

/// TASK-50.2: a store whose daemon goes is told at once, and what it asks fails from then on; one
/// that closes its connection itself is told nothing.
#[test]
fn stores_are_told_when_their_daemon_goes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut daemon, socket) = daemon(dir.path());
    let closed = RemoteExtentStore::connect(&socket, dir.path().join("closed"), 1 << 28).unwrap();
    let (told, lost) = std::sync::mpsc::channel();
    let told_closed = told.clone();
    closed.on_daemon_lost(move || told_closed.send("closed").unwrap());
    drop(closed);

    let store = RemoteExtentStore::connect(&socket, dir.path().join("store"), 1 << 28).unwrap();
    store.on_daemon_lost(move || told.send("store").unwrap());
    let write = |sequence| {
        let extent = ExtentId { branch: store.branch(), sequence };
        store.write(extent, 0, &[VolatileSlice::from(&[7; 4096][..])])
    };
    write(1).unwrap();
    daemon.0.kill().unwrap();
    daemon.0.wait().unwrap();
    assert_eq!(lost.recv_timeout(Duration::from_secs(10)), Ok("store"));
    assert_eq!(write(2), Err(fxdax::Errno::EIO));
    assert!(lost.try_recv().is_err(), "a store closed was told");
}
