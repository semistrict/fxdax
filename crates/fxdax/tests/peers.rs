// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Two extent daemons sharing a bucket, as two hosts do (TASK-406.2): a point made on one, forked
//! and read on the other, whose fetchers ask the first for the parts it owns. The second fetches
//! from the bucket only the parts it owns itself, and the first serves the rest from its part
//! cache, which its uploads left them in, fetching nothing; without peers, the second fetches
//! every part.

#![cfg(target_os = "linux")]

use fxdax::extentd::control::Client;
use fxdax::{ExtentId, RemoteExtentStore, VolatileSlice, VolatileSliceMut};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const CAPACITY: u64 = 1 << 30;
const EXTENTS: u64 = 16;
const EXTENT: usize = 1 << 20;

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A daemon of a host sharing a bucket: its process, its VM socket, a client of its control API,
/// and its stats file.
struct Daemon {
    _process: KillOnDrop,
    vms: PathBuf,
    control: Client,
    stats: PathBuf,
}

/// A daemon serving in `dir`, keeping its stores' branches in the file bucket at `bucket`, and its
/// part cache in `dir`; serving its peers at `peers`' first address, and asking them all, if given.
fn daemon(dir: &Path, bucket: &Path, peers: Option<&[String]>) -> Daemon {
    let (vms, control, stats) =
        (dir.join("extentd.sock"), dir.join("control.sock"), dir.join("stats"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"));
    command
        .arg(&vms)
        .env("FXDAX_EXTENTD_REMOTE", format!("file://{}/", bucket.display()))
        .env("FXDAX_EXTENTD_TOKEN", "1")
        .env("FXDAX_EXTENTD_CACHE", dir.join("cache"))
        .env("FXDAX_EXTENTD_STATS", &stats)
        .env("FXDAX_EXTENTD_GRPC", &control);
    if let Some(peers) = peers {
        command.env("FXDAX_EXTENTD_PEER_ADDRESS", &peers[0]);
        command.env("FXDAX_EXTENTD_PEERS", peers.join(","));
    }
    let process = KillOnDrop(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::os::unix::net::UnixStream::connect(&vms).is_err() {
        assert!(Instant::now() < deadline, "the daemon never served");
        std::thread::sleep(Duration::from_millis(20));
    }
    let control = Client::connect(&control).unwrap();
    Daemon { _process: process, vms, control, stats }
}

/// Two loopback addresses no one listens at yet, for two daemons' peers.
fn addresses() -> [String; 2] {
    let listeners = [(); 2].map(|()| std::net::TcpListener::bind("127.0.0.1:0").unwrap());
    listeners.map(|listener| listener.local_addr().unwrap().to_string())
}

/// Bytes of extent `sequence` that don't compress.
fn data(sequence: u64) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ sequence;
    (0..EXTENT)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// The count `name` of the line of `what` in the stats file `file`, as last written.
fn stat(file: &Path, what: &str, name: &str) -> u64 {
    let text = std::fs::read_to_string(file).unwrap();
    let line = text.lines().find(|line| line.starts_with(&format!("{what}:"))).unwrap();
    let field = line.split(' ').find_map(|field| field.strip_prefix(&format!("{name}=")));
    field.unwrap_or_else(|| panic!("no {name} in {line}")).parse().unwrap()
}

/// Waits for what the stats file `file` says of the bucket and the peers to stop changing: the
/// daemon writes it within 200 ms of each change. Its other lines change as it writes it, counting
/// its own writes.
fn settled(file: &Path) {
    let counts = || {
        let text = std::fs::read_to_string(file).unwrap();
        let read = |line: &&str| line.starts_with("object-store:") || line.starts_with("peers:");
        text.lines().filter(read).collect::<Vec<_>>().join("\n")
    };
    let (mut last, deadline) = (String::new(), Instant::now() + Duration::from_secs(30));
    loop {
        assert!(Instant::now() < deadline, "{} kept changing: {last}", file.display());
        std::thread::sleep(Duration::from_millis(500));
        let now = counts();
        if now == last {
            return;
        }
        last = now;
    }
}

/// What the second host's GETs of ranges, the first's, the parts the second's peers gave it, and
/// the parts the first served came to: a point made on the first host, of [`EXTENTS`] extents,
/// forked and read whole on the second; each asking the other if `peered`.
fn run(peered: bool) -> (u64, u64, u64, u64) {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("bucket");
    let hosts = [dir.path().join("first"), dir.path().join("second")];
    for path in [&bucket, &hosts[0], &hosts[1]] {
        std::fs::create_dir(path).unwrap();
    }
    let [first_at, second_at] = addresses();
    let peers = |me: &String, other: &String| vec![me.clone(), other.clone()];
    let (first_peers, second_peers) = (peers(&first_at, &second_at), peers(&second_at, &first_at));
    let first = daemon(&hosts[0], &bucket, peered.then_some(&first_peers[..]));
    let second = daemon(&hosts[1], &bucket, peered.then_some(&second_peers[..]));

    let base = hosts[0].join("base");
    let store = RemoteExtentStore::connect(&first.vms, &base, CAPACITY).unwrap();
    let branch = store.branch();
    for sequence in 1..=EXTENTS {
        let extent = ExtentId { branch, sequence };
        store.write(extent, 0, &[VolatileSlice::from(&data(sequence)[..])]).unwrap();
    }
    store.commit(None).unwrap();
    drop(store);
    first.control.sync_store(&base).unwrap();
    let point = first.control.point_store(&base, &hosts[0].join("point"), false).unwrap();

    let (copy, fork) = (hosts[1].join("point"), hosts[1].join("fork"));
    second.control.create_store(&copy, CAPACITY, point, None).unwrap();
    second.control.fork_store(&copy, &fork).unwrap();
    let store = RemoteExtentStore::connect(&second.vms, &fork, CAPACITY).unwrap();
    for sequence in 1..=EXTENTS {
        let mut read = vec![0; EXTENT];
        let extent = ExtentId { branch, sequence };
        store.read(extent, 0, &mut [VolatileSliceMut::from(&mut read[..])]).unwrap();
        assert!(read == data(sequence), "extent {sequence} read otherwise");
    }
    drop(store);
    settled(&second.stats);
    settled(&first.stats);
    let gets = |daemon: &Daemon| stat(&daemon.stats, "object-store", "get-range");
    let (given, served) = match peered {
        true => (stat(&second.stats, "peers", "given"), stat(&first.stats, "peers", "served")),
        false => (0, 0),
    };
    (gets(&second), gets(&first), given, served)
}

/// The second host fetches from the bucket only the parts it owns: the first gives it the rest,
/// from its part cache, fetching none of them: no GET more than it makes alone. A point's base read
/// on another host comes from the bucket once, not once on each host that reads it.
#[test]
fn a_point_read_on_another_host_comes_from_its_peer() {
    let (alone, first_alone, _, _) = run(false);
    let (second, first, given, served) = run(true);
    assert!(given > 0, "the first host owned none of the parts read");
    assert_eq!(served, given, "parts served, and those given");
    assert_eq!(second, alone - given, "the second host's GETs of ranges, alone {alone}");
    assert_eq!(first, first_alone, "the first host fetched what it served");
    println!(
        "{EXTENTS} MiB read on a second host: {alone} ranged GETs alone, {second} with peers, \
         {given} parts given by the first host, which made {first} ranged GETs either way"
    );
}

/// The control API replaces a daemon's peers at any time (`SetPeers`): an address that is no
/// `HOST:PORT` is refused (EINVAL), and a daemon that serves no peers refuses them all (ENOTSUP).
#[test]
fn peers_are_set_at_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("bucket");
    let hosts = [dir.path().join("peered"), dir.path().join("alone")];
    for path in [&bucket, &hosts[0], &hosts[1]] {
        std::fs::create_dir(path).unwrap();
    }
    let [me, other] = addresses();
    let peered = daemon(&hosts[0], &bucket, Some(&[me.clone()]));
    peered.control.set_peers(&[&me, &other]).unwrap();
    let refused = peered.control.set_peers(&[&me, "nowhere"]).unwrap_err();
    assert_eq!(refused.errno(), Some(fxdax::Errno::EINVAL));
    let alone = daemon(&hosts[1], &bucket, None);
    let refused = alone.control.set_peers(&[&me]).unwrap_err();
    assert_eq!(refused.errno(), Some(fxdax::Errno::ENOTSUP));
}

/// A part cache budget that is no positive number of bytes stops the daemon at its start, naming
/// the variable (TASK-406.1).
#[test]
fn a_cache_budget_the_daemon_cant_take_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fxdax-extentd"))
        .arg(dir.path().join("extentd.sock"))
        .env("FXDAX_EXTENTD_REMOTE", format!("file://{}/", dir.path().display()))
        .env("FXDAX_EXTENTD_TOKEN", "1")
        .env("FXDAX_EXTENTD_CACHE", dir.path().join("cache"))
        .env("FXDAX_EXTENTD_CACHE_BYTES", "1G")
        .output()
        .unwrap();
    assert!(!output.status.success(), "the daemon started");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("FXDAX_EXTENTD_CACHE_BYTES must be a positive number of bytes: \"1G\""),
        "{said}"
    );
}
