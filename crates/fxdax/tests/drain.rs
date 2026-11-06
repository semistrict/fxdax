// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-141.4: SIGTERM drains the daemon before it exits (`fxdax::extentd::drain`): a VM process
//! still connected is told its daemon went, and its store is closed, its last commits durable and
//! uploaded, as a host upgrading its daemon needs. Before, the daemon exited at once, its store as
//! a crash leaves it: an ordered commit made just before not uploaded, nor perhaps durable.

#![cfg(target_os = "linux")]

use fxdax::{ExtentId, ExtentStore, RemoteExtentStore, VolatileSlice, VolatileSliceMut};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A daemon serving at `socket`, of placement `token`, keeping its stores' branches in the bucket
/// `bucket`, a directory: what it uploads stays there once it exits, for another to read.
fn daemon(socket: &Path, bucket: &Path, token: u64) -> KillOnDrop {
    let daemon = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"))
        .arg(socket)
        .env("FXDAX_EXTENTD_REMOTE", format!("file://{}", bucket.display()))
        .env("FXDAX_EXTENTD_TOKEN", token.to_string())
        .spawn()
        .unwrap();
    let daemon = KillOnDrop(daemon);
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::os::unix::net::UnixStream::connect(socket).is_err() {
        assert!(Instant::now() < deadline, "the daemon never served");
        std::thread::sleep(Duration::from_millis(20));
    }
    daemon
}

fn read(store: &RemoteExtentStore, extent: ExtentId) -> Vec<u8> {
    let mut data = vec![0; BLOCK];
    store.read(extent, 0, &mut [VolatileSliceMut::from(&mut data[..])]).unwrap();
    data
}

#[test]
fn sigterm_closes_every_store_durable_and_uploaded() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    let (socket, path) = (dir.path().join("extentd.sock"), dir.path().join("store"));
    let mut first = daemon(&socket, &bucket, 1);

    // A VM's store, its last commit only ordered, which its uploader, uploading within a second of
    // a durable commit, has not uploaded as the daemon is told to stop.
    let store = RemoteExtentStore::connect(&socket, &path, 1 << 28).unwrap();
    let (told, lost) = std::sync::mpsc::channel();
    store.on_daemon_lost(move || told.send(()).unwrap());
    let branch = store.branch();
    let extent = ExtentId { branch, sequence: 1 };
    store.write(extent, 0, &[VolatileSlice::from(&[7; BLOCK][..])]).unwrap();
    store.commit_ordered(Some(b"drained")).unwrap();

    // SAFETY: Signals the daemon this test started.
    assert_eq!(unsafe { libc::kill(first.0.id() as i32, libc::SIGTERM) }, 0);
    let status = first.0.wait().unwrap();
    assert!(status.success(), "{status}");
    assert_eq!(lost.recv_timeout(Duration::from_secs(10)), Ok(()), "the VM process wasn't told");
    drop(store);

    // Closed, durable: as of its last commit here.
    let closed = ExtentStore::open(&path).unwrap();
    assert_eq!(closed.root().unwrap(), b"drained");
    drop(closed);

    // Uploaded: a later placement's daemon, on another host, with nothing of the store, restores a
    // VM of the branch from the bucket, its commit there.
    let other = dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let second_socket = other.join("extentd.sock");
    let _second = daemon(&second_socket, &bucket, 2);
    let point: PathBuf = other.join("point");
    let restored = other.join("restored");
    let restored =
        RemoteExtentStore::connect_forked(&second_socket, &restored, &point, branch, 1 << 28);
    let restored = restored.unwrap();
    assert_eq!(restored.root().unwrap(), b"drained");
    assert_eq!(read(&restored, extent), [7; BLOCK]);
}
