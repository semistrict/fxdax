// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Where a SQLite transaction of one row's time goes on a volume (TASK-301): the device's
//! requests and the window's, each timed as the volume makes them; the store's flushes, phase by
//! phase (`extent_store::timings`); and, for scale, what an fsync of a block appended to a file
//! takes on the same disk, and a commit of SQLite's own WAL with `synchronous=FULL`.
//!
//! Configurations: `image`, the volume on an in-process image (`fxdax::Storage`); on Linux,
//! `daemon`, on a store an extent daemon on a thread of this process serves, its files mapped
//! into its window.
//!
//! Usage: sqlite-commit-profile [COMMITS], 1,000 by default.

use anyhow::Error;
use async_trait::async_trait;
use fxdax::Storage;
use rusqlite::{Connection, OpenFlags, params};
use sqlite_extents::{Options, Volume, register};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::{Device, DeviceHolder, ExtentId, ReadOptions, WriteOptions};

/// The kinds of request timed.
const KINDS: [&str; 7] = ["read", "write", "free", "commit_root", "flush", "map", "unmap"];

/// How many of each kind, and their nanoseconds.
static COUNTS: [AtomicU64; KINDS.len()] = [const { AtomicU64::new(0) }; KINDS.len()];
static NANOS: [AtomicU64; KINDS.len()] = [const { AtomicU64::new(0) }; KINDS.len()];

/// Counts a request of kind `kind` that began at `start`.
fn count(kind: usize, start: Instant) {
    COUNTS[kind].fetch_add(1, Ordering::Relaxed);
    NANOS[kind].fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// Times `f`, a request of kind `kind`.
async fn timed<T>(kind: usize, f: impl Future<Output = T>) -> T {
    let start = Instant::now();
    let result = f.await;
    count(kind, start);
    result
}

fn reset() {
    for (count, nanos) in COUNTS.iter().zip(&NANOS) {
        count.store(0, Ordering::Relaxed);
        nanos.store(0, Ordering::Relaxed);
    }
}

/// A device whose requests are timed.
struct Timing(DeviceHolder);

#[async_trait]
impl Device for Timing {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.0.allocate_buffer(size)
    }
    fn block_size(&self) -> u32 {
        self.0.block_size()
    }
    fn block_count(&self) -> u64 {
        self.0.block_count()
    }
    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: MutableBufferRef<'_>,
        opts: ReadOptions,
    ) -> Result<(), Error> {
        timed(0, self.0.read_with_opts(extent, offset, buffer, opts)).await
    }
    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        opts: WriteOptions<'_>,
    ) -> Result<(), Error> {
        timed(1, self.0.write_with_opts(extent, offset, buffer, opts)).await
    }
    fn branch(&self) -> u64 {
        self.0.branch()
    }
    async fn next_sequence(&self) -> Result<u64, Error> {
        self.0.next_sequence().await
    }
    async fn sequence_floor(&self) -> Result<u64, Error> {
        self.0.sequence_floor().await
    }
    async fn free(&self, extent: ExtentId) -> Result<(), Error> {
        timed(2, self.0.free(extent)).await
    }
    async fn free_extents(&self, extents: &[ExtentId]) -> Vec<Result<(), Error>> {
        timed(2, self.0.free_extents(extents)).await
    }
    async fn root(&self) -> Result<Vec<u8>, Error> {
        self.0.root().await
    }
    async fn commit_root(&self, root: &[u8]) -> Result<(), Error> {
        timed(3, self.0.commit_root(root)).await
    }
    async fn close(&self) -> Result<(), Error> {
        self.0.close().await
    }
    async fn flush(&self) -> Result<(), Error> {
        timed(4, self.0.flush()).await
    }
    fn is_read_only(&self) -> bool {
        self.0.is_read_only()
    }
}

/// A window whose requests are timed.
#[cfg(target_os = "linux")]
struct TimingWindow(sqlite_extents::daemon::DaemonWindow);

#[cfg(target_os = "linux")]
impl sqlite_extents::Window for TimingWindow {
    fn memory(&self) -> (std::ptr::NonNull<u8>, u64) {
        self.0.memory()
    }
    fn map(&self, device: &dyn Device, runs: &[sqlite_extents::Run]) -> Result<(), Error> {
        let start = Instant::now();
        let mapped = self.0.map(device, runs);
        count(5, start);
        mapped
    }
    fn unmap(&self, at: u64, len: u64) -> Result<(), Error> {
        let start = Instant::now();
        let unmapped = self.0.unmap(at, len);
        count(6, start);
        unmapped
    }
}

/// Commits `commits` transactions of one row each on `connection`, after a table of 10,000 rows
/// as the benchmark's: the time each took.
fn commits(connection: &Connection, commits: usize) -> Vec<Duration> {
    connection.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)").unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    for k in 0..10_000i64 {
        transaction.execute("INSERT INTO t VALUES (?1, ?2)", params![k, vec![1u8; 100]]).unwrap();
    }
    transaction.commit().unwrap();
    reset();
    let flushes = extent_store::timings().flushes;
    let times = (0..commits)
        .map(|index| {
            let start = Instant::now();
            let key = (index as i64 * 7919) % 10_000;
            connection
                .execute("UPDATE t SET v = ?2 WHERE k = ?1", params![key, vec![2u8; 100]])
                .unwrap();
            start.elapsed()
        })
        .collect();
    report_requests(commits);
    report_flushes(flushes, commits);
    times
}

fn report_requests(commits: usize) {
    for (kind, name) in KINDS.iter().enumerate() {
        let (count, nanos) =
            (COUNTS[kind].load(Ordering::Relaxed), NANOS[kind].load(Ordering::Relaxed));
        if count > 0 {
            println!(
                "  {name:<12} {:>6.2} a commit, {:>8.1} µs each, {:>8.1} µs a commit",
                count as f64 / commits as f64,
                nanos as f64 / count as f64 / 1e3,
                nanos as f64 / commits as f64 / 1e3,
            );
        }
    }
}

/// The store's flushes since `before`, phase by phase, a commit's share.
fn report_flushes(before: extent_store::Timed<6>, commits: usize) {
    let after = extent_store::timings().flushes;
    let count = after.count - before.count;
    let phases: Vec<String> = (extent_store::FLUSH_PHASES.iter().enumerate())
        .map(|(phase, name)| {
            let us = after.sum.us[phase] - before.sum.us[phase];
            format!("{name} {:.1}", us as f64 / commits as f64)
        })
        .collect();
    println!(
        "  store flushes {:.2} a commit; µs a commit: {}",
        count as f64 / commits as f64,
        phases.join(", ")
    );
}

fn summary(config: &str, times: &mut [Duration]) {
    times.sort();
    let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize].as_secs_f64() * 1e6;
    let total: Duration = times.iter().sum();
    println!(
        "{config}: {} commits, {:.0}/s, p50 {:.0} µs, p99 {:.0} µs",
        times.len(),
        times.len() as f64 / total.as_secs_f64(),
        at(0.5),
        at(0.99)
    );
}

fn on_volume(config: &str, volume: Arc<Volume>, count: usize) {
    let registration = register(config, volume.clone()).unwrap();
    let connection =
        Connection::open_with_flags_and_vfs("p.db", OpenFlags::default(), config).unwrap();
    println!("{config}:");
    let mut times = commits(&connection, count);
    summary(config, &mut times);
    drop(connection);
    drop(registration);
    volume.close().unwrap();
}

/// What appending `blocks` blocks to a file and syncing them takes on the disk under `dir`.
fn fsyncs(dir: &Path, count: usize, blocks: usize) {
    use std::io::Write as _;
    let mut file = std::fs::File::create(dir.join(format!("fsyncs-{blocks}"))).unwrap();
    let data = vec![7u8; 4096 * blocks];
    let mut times: Vec<Duration> = (0..count)
        .map(|_| {
            let start = Instant::now();
            file.write_all(&data).unwrap();
            file.sync_data().unwrap();
            start.elapsed()
        })
        .collect();
    summary(&format!("append and fdatasync {blocks} blocks"), &mut times);
}

/// What overwriting `blocks` blocks of a file written whole before, in turn, and syncing them
/// takes: what a log whose file was made ahead of its records costs, its size and blocks unchanged.
fn overwrites(dir: &Path, count: usize, blocks: usize) {
    use std::os::unix::fs::FileExt as _;
    let file = std::fs::File::create(dir.join(format!("overwrites-{blocks}"))).unwrap();
    let len = 4096 * blocks;
    file.write_all_at(&vec![0u8; len * count], 0).unwrap();
    file.sync_all().unwrap();
    let data = vec![7u8; len];
    let mut times: Vec<Duration> = (0..count)
        .map(|at| {
            let start = Instant::now();
            file.write_all_at(&data, (at * len) as u64).unwrap();
            file.sync_data().unwrap();
            start.elapsed()
        })
        .collect();
    summary(&format!("overwrite and fdatasync {blocks} blocks"), &mut times);
}

fn main() {
    let count: usize = std::env::args().nth(1).map_or(1000, |count| count.parse().unwrap());
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/sqlite-commit-profile");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    // A host of the multi-node system, the daemon's and the image's branches kept in a file bucket
    // here, placement 1.
    // SAFETY: before this process starts any other thread.
    unsafe {
        std::env::set_var(
            "FXDAX_EXTENTD_REMOTE",
            format!("file://{}/", root.join("bucket").display()),
        );
        std::env::set_var("FXDAX_EXTENTD_TOKEN", "1");
    }
    println!("Under {}.", root.display());
    for blocks in [1, 5] {
        fsyncs(&root, count, blocks);
        overwrites(&root, count, blocks);
    }
    {
        let dir = root.join("wal");
        std::fs::create_dir_all(&dir).unwrap();
        let connection = Connection::open(dir.join("p.db")).unwrap();
        connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL").unwrap();
        let mut times = commits(&connection, count);
        summary("wal-full", &mut times);
    }
    {
        let device = Storage::create(root.join("image"), 64 << 30).unwrap().into_device();
        let volume = Volume::open(DeviceHolder::new(Timing(device)), Options::default()).unwrap();
        on_volume("image", volume, count);
    }
    #[cfg(target_os = "linux")]
    {
        let socket = root.join("extentd.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || fxdax::extentd::serve(listener));
        let store =
            fxdax::RemoteExtentStore::connect(&socket, root.join("daemon"), 64 << 30).unwrap();
        let store = Arc::new(store);
        let window = sqlite_extents::daemon::DaemonWindow::new(store.clone(), 1 << 30).unwrap();
        let device =
            DeviceHolder::new(Timing(DeviceHolder::new(fxdax::RemoteDevice::new(store).unwrap())));
        let volume =
            Volume::open_mapped(device, Box::new(TimingWindow(window)), Options::default())
                .unwrap();
        on_volume("daemon", volume, count);
    }
}
