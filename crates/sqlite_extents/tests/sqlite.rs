// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! SQLite on volumes on fake devices (TASK-296).

use rusqlite::{Connection, OpenFlags, params};
use sqlite_extents::{Options, Registration, Volume, register};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use storage_device::DeviceHolder;
use storage_device::fake_device::{Crashes, FakeDevice, Op};

const BLOCK: u32 = 4096;

/// A device of 1 GiB at most, as awkward as a store.
fn device() -> FakeDevice {
    FakeDevice::new(1 << 18, BLOCK)
}

/// A volume on `device`, registered as a VFS of a name of its own.
struct Mounted {
    volume: Arc<Volume>,
    vfs: String,
    _registration: Registration,
}

impl Mounted {
    fn new(device: DeviceHolder) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let vfs = format!("fxdax-test-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let volume = Volume::open(device, Options::default()).expect("opening the volume");
        let registration = register(&vfs, volume.clone()).expect("registering");
        Self { volume, vfs, _registration: registration }
    }

    fn connect(&self, name: &str) -> Connection {
        Connection::open_with_flags_and_vfs(name, OpenFlags::default(), self.vfs.as_str())
            .expect("opening the database")
    }

    /// The device as a crash that loses nothing leaves it.
    fn snapshot(&self) -> DeviceHolder {
        self.volume.device().snapshot().expect("a snapshot")
    }

    /// The bytes the extents the device holds take.
    fn held_bytes(&self) -> u64 {
        let device = self.volume.device();
        device.extents().iter().map(|extent| device.extent_len(*extent).unwrap()).sum()
    }

    /// Checks that the device holds just what the volume's files reference.
    fn check_extents(&self) {
        assert_eq!(self.volume.device().extents(), self.volume.extents().unwrap());
    }
}

fn integrity(connection: &Connection) -> String {
    connection.query_row("PRAGMA integrity_check", [], |row| row.get(0)).unwrap()
}

fn rows(connection: &Connection) -> BTreeMap<i64, Vec<u8>> {
    let mut statement = connection.prepare("SELECT k, v FROM t ORDER BY k").unwrap();
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    rows.map(Result::unwrap).collect()
}

#[test]
fn a_database_reads_back_once_opened_again() {
    let mounted = Mounted::new(DeviceHolder::new(device()));
    let connection = mounted.connect("main.db");
    connection.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)").unwrap();
    for round in 0..20i64 {
        let transaction = connection.unchecked_transaction().unwrap();
        for k in 0..50 {
            let v = vec![(round + k) as u8; 300 + k as usize];
            transaction
                .execute("INSERT OR REPLACE INTO t VALUES (?1, ?2)", params![k + round * 7, v])
                .unwrap();
        }
        transaction.commit().unwrap();
    }
    let expected = rows(&connection);
    assert_eq!(expected.len(), 183);
    drop(connection);
    mounted.check_extents();

    let reopened = Mounted::new(mounted.snapshot());
    assert_eq!(reopened.volume.files().unwrap(), vec!["main.db".to_owned()]);
    let connection = reopened.connect("main.db");
    assert_eq!(integrity(&connection), "ok");
    assert_eq!(rows(&connection), expected);
    reopened.check_extents();
}

#[test]
fn several_databases_share_a_volume() {
    let mounted = Mounted::new(DeviceHolder::new(device()));
    for (name, count) in [("a.db", 10), ("b.db", 400), ("c.db", 0)] {
        let connection = mounted.connect(name);
        connection.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)").unwrap();
        for k in 0..count {
            connection
                .execute("INSERT INTO t VALUES (?1, ?2)", params![k, name.as_bytes()])
                .unwrap();
        }
    }
    // One deleted.
    let connection = mounted.connect("a.db");
    connection.execute_batch("ATTACH 'c.db' AS c; DETACH c").unwrap();
    drop(connection);
    mounted.volume.delete("c.db").unwrap();
    mounted.check_extents();

    let reopened = Mounted::new(mounted.snapshot());
    assert_eq!(reopened.volume.files().unwrap(), vec!["a.db".to_owned(), "b.db".to_owned()]);
    for (name, count) in [("a.db", 10), ("b.db", 400)] {
        let connection = reopened.connect(name);
        assert_eq!(rows(&connection).len(), count);
        assert_eq!(rows(&connection)[&3], name.as_bytes());
    }
    let missing = Connection::open_with_flags_and_vfs(
        "c.db",
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        reopened.vfs.as_str(),
    );
    assert_eq!(missing.unwrap_err().sqlite_error_code(), Some(rusqlite::ErrorCode::CannotOpen));
}

/// A transaction committed is seen by the other connections, and one in progress isn't.
#[test]
fn connections_see_committed_transactions() {
    let mounted = Mounted::new(DeviceHolder::new(device()));
    let writer = mounted.connect("main.db");
    let reader = mounted.connect("main.db");
    writer
        .execute_batch(
            "CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB); INSERT INTO t VALUES (1, x'01')",
        )
        .unwrap();
    writer.execute_batch("BEGIN; INSERT INTO t VALUES (2, x'02')").unwrap();
    assert_eq!(rows(&reader).len(), 1);
    writer.execute_batch("COMMIT").unwrap();
    assert_eq!(rows(&reader).len(), 2);
}

/// A crash at every barrier of a run of transactions leaves the database as of the
/// transactions committed before it, whole, and the device holding what it references alone.
#[test]
fn a_crash_leaves_the_transactions_committed() {
    for seed in 0..4 {
        let crashes = Crashes::new(seed, 1);
        let mounted = Mounted::new(DeviceHolder::new(device().record_crashes(crashes.clone())));
        let connection = mounted.connect("main.db");
        connection
            .execute_batch("PRAGMA cache_size = 20; CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)")
            .unwrap();
        let mut model = BTreeMap::new();
        let mut committed = vec![model.clone()];
        crashes.mark(0);
        for round in 0..40i64 {
            let transaction = connection.unchecked_transaction().unwrap();
            // Inserts, updates and deletes, over enough rows for trees of two levels and more
            // than SQLite's cache holds.
            for k in 0..60 {
                let key = (k * 37 + round * 11) % 700;
                let v = vec![round as u8; 200 + (k as usize * 13) % 900];
                match (k + round) % 5 {
                    0 => {
                        transaction.execute("DELETE FROM t WHERE k = ?1", params![key]).unwrap();
                        model.remove(&key);
                    }
                    _ => {
                        transaction
                            .execute("INSERT OR REPLACE INTO t VALUES (?1, ?2)", params![key, v])
                            .unwrap();
                        model.insert(key, v);
                    }
                }
            }
            transaction.commit().unwrap();
            committed.push(model.clone());
            crashes.mark(round as u64 + 1);
        }
        drop(connection);

        let mut recovered = 0;
        for (mark, device) in crashes.take() {
            let reopened = Mounted::new(DeviceHolder::new(device));
            reopened.check_extents();
            let connection = reopened.connect("main.db");
            assert_eq!(integrity(&connection), "ok", "seed {seed}, mark {mark}");
            assert_eq!(rows(&connection), committed[mark as usize], "seed {seed}, mark {mark}");
            recovered += 1;
        }
        assert_eq!(recovered, 40, "seed {seed}");
    }
}

/// What the device holds stays within a bound of the database, however often its pages are
/// written again.
#[test]
fn space_stays_bounded_under_updates() {
    let mounted = Mounted::new(DeviceHolder::new(FakeDevice::new(1 << 18, BLOCK).calm()));
    let connection = mounted.connect("main.db");
    connection.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)").unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    for k in 0..2000i64 {
        transaction.execute("INSERT INTO t VALUES (?1, ?2)", params![k, vec![1u8; 1000]]).unwrap();
    }
    transaction.commit().unwrap();
    for round in 0..3000i64 {
        let key = (round * 7919) % 2000;
        connection
            .execute("UPDATE t SET v = ?2 WHERE k = ?1", params![key, vec![round as u8; 1000]])
            .unwrap();
    }
    let size: i64 = connection
        .query_row(
            "SELECT page_count * page_size FROM pragma_page_count, pragma_page_size",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let (size, held) = (size as u64, mounted.held_bytes());
    println!("database {size} bytes, device {held} bytes");
    assert!(held <= size * 3, "the device holds {held} bytes for a database of {size}");
    mounted.check_extents();
}

/// A commit whose root fails fails the volume; opened again, it holds the database as of the
/// commit before, and frees what the failed one wrote.
#[test]
fn a_failed_commit_leaves_nothing_behind() {
    let mut device = device();
    let failing = Arc::new(AtomicBool::new(false));
    let fail = failing.clone();
    device.set_op_callback(move |op| match op {
        Op::Flush if fail.load(Ordering::Relaxed) => Err(anyhow::anyhow!("the device fails")),
        _ => Ok(()),
    });
    let mounted = Mounted::new(DeviceHolder::new(device));
    let connection = mounted.connect("main.db");
    connection
        .execute_batch(
            "CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB); INSERT INTO t VALUES (1, x'01')",
        )
        .unwrap();
    failing.store(true, Ordering::Relaxed);
    let failed = connection.execute("INSERT INTO t VALUES (2, zeroblob(100000))", []);
    assert_eq!(failed.unwrap_err().sqlite_error_code(), Some(rusqlite::ErrorCode::SystemIoFailure));
    assert_eq!(
        mounted.volume.check().unwrap_err().to_string(),
        "the volume failed: committing the volume's root: the device fails"
    );
    let device = mounted.snapshot();
    drop(connection);

    let reopened = Mounted::new(device);
    reopened.check_extents();
    let connection = reopened.connect("main.db");
    assert_eq!(rows(&connection), BTreeMap::from([(1, vec![1])]));
}

/// WAL mode would keep its log in memory, lost in a crash: SQLite keeps the rollback journal.
#[test]
fn wal_mode_is_refused() {
    let mounted = Mounted::new(DeviceHolder::new(device()));
    let connection = mounted.connect("main.db");
    connection.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY)").unwrap();
    let mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)).unwrap();
    assert_eq!(mode, "delete");

    // In exclusive locking mode SQLite needs no shared memory for WAL mode, and takes it, but
    // fails to open the log at the next transaction.
    connection.execute_batch("PRAGMA locking_mode = EXCLUSIVE").unwrap();
    let mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)).unwrap();
    assert_eq!(mode, "wal");
    let inserted = connection.execute("INSERT INTO t VALUES (1)", []);
    assert_eq!(inserted.unwrap_err().sqlite_error_code(), Some(rusqlite::ErrorCode::CannotOpen));
    assert_eq!(mounted.volume.files().unwrap(), vec!["main.db".to_owned()]);
}
