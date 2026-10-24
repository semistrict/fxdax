// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Real SQLite transactions, locking and process recovery, shared by guest benchmarks and
//! harness tests. Timings use one live connection; opening SQLite and generating SQL are outside
//! the measured interval. A process kill tests recovery, not power-loss durability.

use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

const ROWS: usize = 4096;
const MARKER: &str = "sqlite-request-complete";

struct Connection {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Connection {
    fn open(db: &Path) -> Self {
        let mut child = Command::new("sqlite3")
            .args(["-batch", "-bail"])
            .arg(db)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("starting sqlite3");
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self { child, input, output }
    }

    fn request(&mut self, sql: &str) -> Vec<String> {
        writeln!(self.input, "{sql}\nSELECT '{MARKER}';").expect("writing SQLite input");
        self.input.flush().unwrap();
        let mut rows = Vec::new();
        loop {
            let mut line = String::new();
            assert_ne!(self.output.read_line(&mut line).unwrap(), 0, "SQLite exited before reply");
            let line = line.trim_end();
            if line == MARKER {
                return rows;
            }
            rows.push(line.to_owned());
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Also reap a connection abandoned by an assertion. Killing after the last reply
        // deliberately avoids SQLite's clean-close checkpoint in the recovery checks.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Mode {
    DeleteFull,
    WalFull,
    WalNormal,
}

impl Mode {
    pub(crate) const ALL: [Self; 3] = [Self::DeleteFull, Self::WalFull, Self::WalNormal];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::DeleteFull => "delete-full",
            Self::WalFull => "wal-full",
            Self::WalNormal => "wal-normal",
        }
    }

    fn initialize(self, db: &Path) -> Connection {
        let mut connection = Connection::open(db);
        let journal = match self {
            Self::DeleteFull => "delete",
            Self::WalFull | Self::WalNormal => "wal",
        };
        let sync = match self {
            Self::DeleteFull | Self::WalFull => "FULL",
            Self::WalNormal => "NORMAL",
        };
        assert_eq!(
            connection.request(&format!(
                "PRAGMA page_size=4096; PRAGMA journal_mode={journal};
                 PRAGMA synchronous={sync}; PRAGMA wal_autocheckpoint=0;
                 PRAGMA cache_size=-8192; PRAGMA mmap_size=0;
                 CREATE TABLE t(id INTEGER PRIMARY KEY, version INTEGER NOT NULL, payload BLOB);
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<{ROWS})
                 INSERT INTO t SELECT i, 0, randomblob(1000) FROM n;"
            )),
            [journal, "0", "0"]
        );
        if journal == "wal" {
            assert_eq!(connection.request("PRAGMA wal_checkpoint(TRUNCATE);"), ["0|0|0"]);
        }
        connection
    }
}

pub(crate) struct Measurement {
    pub(crate) commit_seconds: f64,
    pub(crate) checkpoint_seconds: Option<f64>,
}

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Commits,
    Checkpoint,
}

/// Fingerprint every payload through the live connection's cache before killing it, then
/// compare with a fresh connection's reads. Integrity checks alone do not detect stale payloads.
fn contents(connection: &mut Connection) -> u64 {
    let mut hash = DefaultHasher::new();
    connection.request("SELECT id,version,hex(payload) FROM t ORDER BY id;").hash(&mut hash);
    hash.finish()
}

/// Check committed data and the entire database after reopening it, outside the timing.
fn verify(db: &Path, updates: usize, expected: u64) {
    let mut connection = Connection::open(db);
    assert_eq!(
        connection.request(
            "PRAGMA integrity_check; SELECT count(*),sum(version),sum(length(payload)) FROM t;"
        ),
        ["ok".to_owned(), format!("{ROWS}|{updates}|{}", ROWS * 1000)]
    );
    assert_eq!(contents(&mut connection), expected, "reopened SQLite payloads changed");
}

/// Measure small random updates, either individually committed or in batches. WAL's explicit
/// checkpoint is separate: NORMAL omits each commit's sync and must not masquerade as FULL.
pub(crate) fn transactions(
    root: &Path,
    mode: Mode,
    count: usize,
    batch: usize,
    mut observe: impl FnMut(Phase, &mut dyn FnMut() -> f64) -> f64,
) -> Measurement {
    assert!(count > 0 && batch > 0);
    let db = root.join(format!("sqlite-{}-{batch}.db", mode.name()));
    let mut connection = mode.initialize(&db);
    let mut sql = String::new();
    for transaction in 0..count {
        sql.push_str("BEGIN IMMEDIATE;\n");
        for update in 0..batch {
            let id = (transaction * batch + update) * 17 % ROWS + 1;
            writeln!(sql, "UPDATE t SET version=version+1,payload=randomblob(1000) WHERE id={id};")
                .unwrap();
        }
        sql.push_str("COMMIT;\n");
    }
    let commit_seconds = observe(Phase::Commits, &mut || {
        let start = Instant::now();
        assert!(connection.request(&sql).is_empty());
        start.elapsed().as_secs_f64()
    });
    let checkpoint_seconds = match mode {
        Mode::DeleteFull => None,
        Mode::WalFull | Mode::WalNormal => Some(observe(Phase::Checkpoint, &mut || {
            let start = Instant::now();
            assert_eq!(connection.request("PRAGMA wal_checkpoint(TRUNCATE);"), ["0|0|0"]);
            start.elapsed().as_secs_f64()
        })),
    };
    let expected = contents(&mut connection);
    drop(connection);
    verify(&db, count * batch, expected);
    std::fs::remove_file(db).unwrap();
    Measurement { commit_seconds, checkpoint_seconds }
}

/// A killed writer must preserve committed rows and discard an uncommitted transaction, even
/// with a cache small enough to spill dirty pages into the rollback journal or WAL.
pub(crate) fn recovery(root: &Path, mode: Mode) {
    let db = root.join(format!("sqlite-recovery-{}.db", mode.name()));
    let mut connection = mode.initialize(&db);
    assert!(
        connection
            .request("BEGIN IMMEDIATE; UPDATE t SET version=1 WHERE id=1; COMMIT;")
            .is_empty()
    );
    let expected = contents(&mut connection);
    assert!(
        connection
            .request("PRAGMA cache_size=8; BEGIN IMMEDIATE; UPDATE t SET version=version+100;")
            .is_empty()
    );
    drop(connection);
    verify(&db, 1, expected);
    std::fs::remove_file(db).unwrap();
}

/// A WAL reader keeps its snapshot while another connection commits. Releasing the reader
/// permits a truncating checkpoint, and a new read sees the committed update.
pub(crate) fn snapshot(root: &Path) {
    let db = root.join("sqlite-snapshot.db");
    let mut writer = Mode::WalFull.initialize(&db);
    let mut reader = Connection::open(&db);
    assert_eq!(reader.request("BEGIN; SELECT sum(version) FROM t;"), ["0"]);
    assert!(
        writer.request("BEGIN IMMEDIATE; UPDATE t SET version=1 WHERE id=1; COMMIT;").is_empty()
    );
    let expected = contents(&mut writer);
    assert_eq!(
        reader.request("SELECT sum(version) FROM t; COMMIT; SELECT sum(version) FROM t;"),
        ["0", "1"]
    );
    assert_eq!(writer.request("PRAGMA wal_checkpoint(TRUNCATE);"), ["0|0|0"]);
    drop(reader);
    drop(writer);
    verify(&db, 1, expected);
    std::fs::remove_file(db).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_updates_survive_reopen() {
        let root = tempdir::TempDir::new("sqlite-transactions").unwrap();
        for mode in Mode::ALL {
            for batch in [1, 100] {
                transactions(root.path(), mode, 3, batch, |_, run| run());
            }
        }
    }

    #[test]
    fn killed_writer_recovers_only_committed_updates() {
        let root = tempdir::TempDir::new("sqlite-recovery").unwrap();
        for mode in Mode::ALL {
            recovery(root.path(), mode);
        }
    }

    #[test]
    fn wal_reader_keeps_snapshot_during_writer_commit() {
        let root = tempdir::TempDir::new("sqlite-snapshot").unwrap();
        snapshot(root.path());
    }
}
