// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! SQLite on a volume on an fxdax image, against SQLite on the host's filesystem (TASK-298):
//! the same workloads, each on a database of its own, made under the workspace's target
//! directory, on the disk the host builds on.
//!
//! - `commit`: transactions of one INSERT each, as an application logging events does;
//! - `bulk`: one transaction loading the table the rest read and update;
//! - `update`: transactions of one UPDATE of a random row each;
//! - `batch`: transactions of 500 UPDATEs of random rows each;
//! - `read`: SELECTs of one random row each, outside transactions;
//! - `scan`: a sum over every row.
//!
//! Configurations: `extents`, this crate's VFS on an image (`fxdax::Storage`), every commit
//! durable, the image's I/O bypassing the page cache; and SQLite's own unix VFS on a file:
//! `delete-full` (rollback journal, `synchronous=FULL`: every commit durable), `wal-full` (WAL,
//! every commit durable) and `wal-normal` (WAL, durable at checkpoints only: no commit is).
//!
//! Usage: sqlite-bench [SCALE], SCALE multiplying the counts (default 1). It prints a table
//! in markdown, a row per configuration and workload.

use fxdax::Storage;
use rusqlite::{Connection, OpenFlags, params};
use sqlite_extents::{Options, Volume, register};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PAYLOAD: usize = 100;
const ROWS: i64 = 100_000;
const COMMITS: usize = 2_000;
const UPDATES: usize = 2_000;
const BATCHES: usize = 100;
const BATCH: usize = 500;
const READS: usize = 50_000;
const SCANS: usize = 3;

/// What a workload took: how many operations, how long, and each transaction's time if it timed
/// them.
struct Measured {
    ops: usize,
    total: Duration,
    latencies: Vec<Duration>,
}

impl Measured {
    fn row(&self, config: &str, workload: &str) -> String {
        let per_sec = self.ops as f64 / self.total.as_secs_f64();
        let (p50, p99) = match self.latencies.is_empty() {
            true => ("".to_owned(), "".to_owned()),
            false => {
                let mut sorted = self.latencies.clone();
                sorted.sort();
                let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
                (micros(at(0.5)), micros(at(0.99)))
            }
        };
        format!(
            "| {config} | {workload} | {} | {:.3} | {per_sec:.0} | {p50} | {p99} |",
            self.ops,
            self.total.as_secs_f64()
        )
    }
}

fn micros(duration: Duration) -> String {
    format!("{:.0}", duration.as_secs_f64() * 1e6)
}

/// A random number generator, the same for every configuration (xorshift).
struct Keys(u64);

impl Keys {
    fn next(&mut self, below: i64) -> i64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % below as u64) as i64
    }
}

/// Times `count` transactions, each what `one` does with its index.
fn timed(count: usize, mut one: impl FnMut(usize)) -> Measured {
    let mut latencies = Vec::with_capacity(count);
    let start = Instant::now();
    for index in 0..count {
        let began = Instant::now();
        one(index);
        latencies.push(began.elapsed());
    }
    Measured { ops: count, total: start.elapsed(), latencies }
}

/// Runs every workload on `connection`, printing a row each.
fn workloads(config: &str, connection: &Connection, scale: usize) {
    let mut keys = Keys(0x9e37_79b9_7f4a_7c15);
    let payload = vec![7u8; PAYLOAD];
    connection
        .execute_batch(
            "CREATE TABLE log (id INTEGER PRIMARY KEY, v BLOB);
             CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)",
        )
        .unwrap();

    let commits = timed(COMMITS * scale, |_| {
        connection.execute("INSERT INTO log (v) VALUES (?1)", params![payload]).unwrap();
    });
    println!("{}", commits.row(config, "commit"));

    let rows = ROWS * scale as i64;
    let start = Instant::now();
    let transaction = connection.unchecked_transaction().unwrap();
    {
        let mut insert = transaction.prepare("INSERT INTO t VALUES (?1, ?2)").unwrap();
        for k in 0..rows {
            insert.execute(params![k, payload]).unwrap();
        }
    }
    transaction.commit().unwrap();
    let bulk = Measured { ops: rows as usize, total: start.elapsed(), latencies: Vec::new() };
    println!("{}", bulk.row(config, "bulk"));

    let mut update = connection.prepare("UPDATE t SET v = ?2 WHERE k = ?1").unwrap();
    let updates = timed(UPDATES * scale, |index| {
        let v = vec![index as u8; PAYLOAD];
        update.execute(params![keys.next(rows), v]).unwrap();
    });
    println!("{}", updates.row(config, "update"));

    let batches = timed(BATCHES * scale, |index| {
        let transaction = connection.unchecked_transaction().unwrap();
        {
            let mut update =
                transaction.prepare_cached("UPDATE t SET v = ?2 WHERE k = ?1").unwrap();
            for _ in 0..BATCH {
                update.execute(params![keys.next(rows), vec![index as u8; PAYLOAD]]).unwrap();
            }
        }
        transaction.commit().unwrap();
    });
    println!("{}", batches.row(config, "batch"));

    let mut select = connection.prepare("SELECT length(v) FROM t WHERE k = ?1").unwrap();
    let start = Instant::now();
    for _ in 0..READS * scale {
        let len: i64 = select.query_row(params![keys.next(rows)], |row| row.get(0)).unwrap();
        assert_eq!(len, PAYLOAD as i64);
    }
    let reads = Measured { ops: READS * scale, total: start.elapsed(), latencies: Vec::new() };
    println!("{}", reads.row(config, "read"));

    let start = Instant::now();
    for _ in 0..SCANS {
        let sum: i64 =
            connection.query_row("SELECT sum(length(v)) FROM t", [], |row| row.get(0)).unwrap();
        assert_eq!(sum, rows * PAYLOAD as i64);
    }
    let scans =
        Measured { ops: SCANS * rows as usize, total: start.elapsed(), latencies: Vec::new() };
    println!("{}", scans.row(config, "scan (rows)"));
}

/// The bytes the files under `path` take.
fn disk_bytes(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).unwrap();
    if !metadata.is_dir() {
        return metadata.blocks() * 512;
    }
    std::fs::read_dir(path).unwrap().map(|entry| disk_bytes(&entry.unwrap().path())).sum()
}

fn main() {
    let scale: usize = std::env::args().nth(1).map_or(1, |scale| scale.parse().expect("a scale"));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/sqlite-bench");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    println!("Databases under {}, scale {scale}.", root.display());
    println!();
    println!("| config | workload | ops | seconds | ops/s | p50 µs | p99 µs |");
    println!("|---|---|---|---|---|---|---|");

    let mut sizes = Vec::new();
    {
        let image = root.join("extents");
        let volume = Volume::open(
            Storage::create(&image, 64 << 30).unwrap().into_device(),
            Options::default(),
        )
        .unwrap();
        let registration = register("sqlite-bench", volume.clone()).unwrap();
        let connection =
            Connection::open_with_flags_and_vfs("bench.db", OpenFlags::default(), "sqlite-bench")
                .unwrap();
        workloads("extents", &connection, scale);
        drop(connection);
        drop(registration);
        volume.close().unwrap();
        // The image, and the bucket beside it that keeps its branch (`Storage::create`).
        let bucket = root.join("extents.bucket");
        sizes.push(("extents", disk_bytes(&image) + disk_bytes(&bucket)));
    }
    for (config, pragmas) in [
        ("delete-full", "PRAGMA journal_mode = DELETE; PRAGMA synchronous = FULL"),
        ("wal-full", "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL"),
        ("wal-normal", "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL"),
    ] {
        let dir = root.join(config);
        std::fs::create_dir_all(&dir).unwrap();
        let connection = Connection::open(dir.join("bench.db")).unwrap();
        connection.execute_batch(pragmas).unwrap();
        workloads(config, &connection, scale);
        drop(connection);
        sizes.push((config, disk_bytes(&dir)));
    }
    println!();
    println!("| config | bytes on disk |");
    println!("|---|---|");
    for (config, bytes) in sizes {
        println!("| {config} | {bytes} |");
    }
}
