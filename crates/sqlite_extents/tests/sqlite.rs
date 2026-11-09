// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! SQLite on volumes on fake devices (TASK-296).

use futures::executor::block_on;
use rusqlite::{Connection, OpenFlags, params};
use sqlite_extents::{Contents, Options, Registration, Run, Volume, Window, register};
use std::alloc::Layout;
use std::collections::BTreeMap;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use storage_device::fake_device::{Crashes, FakeDevice, Op};
use storage_device::{Device, DeviceHolder};

const BLOCK: u32 = 4096;

/// A device of 1 GiB at most, as awkward as a store.
fn device() -> FakeDevice {
    FakeDevice::new(1 << 18, BLOCK)
}

/// A window that copies the blocks it maps into memory of its own, as a daemon's maps its cache's
/// pages: what it unmaps reads as 0xab, which no page SQLite reads holds whole.
struct CopyingWindow {
    memory: NonNull<u8>,
    size: u64,
}

// SAFETY: written only as the volume maps, under its lock.
unsafe impl Send for CopyingWindow {}
unsafe impl Sync for CopyingWindow {}

impl CopyingWindow {
    /// Four regions of 16 MiB.
    const SIZE: u64 = 64 << 20;

    fn new() -> Self {
        let layout = Layout::from_size_align(Self::SIZE as usize, BLOCK as usize).unwrap();
        // SAFETY: a layout of some bytes.
        let memory = NonNull::new(unsafe { std::alloc::alloc(layout) }).unwrap();
        // SAFETY: the memory allocated.
        unsafe { memory.as_ptr().write_bytes(0xab, Self::SIZE as usize) };
        Self { memory, size: Self::SIZE }
    }

    #[allow(clippy::mut_from_ref)]
    fn bytes(&self, at: u64, len: u64) -> &mut [u8] {
        assert!(at + len <= self.size);
        // SAFETY: within the memory, which the volume changes under its lock alone.
        unsafe {
            std::slice::from_raw_parts_mut(self.memory.as_ptr().add(at as usize), len as usize)
        }
    }
}

impl Drop for CopyingWindow {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.size as usize, BLOCK as usize).unwrap();
        // SAFETY: allocated so.
        unsafe { std::alloc::dealloc(self.memory.as_ptr(), layout) };
    }
}

impl Window for CopyingWindow {
    fn memory(&self) -> (NonNull<u8>, u64) {
        (self.memory, self.size)
    }

    fn map(&self, device: &dyn Device, runs: &[Run]) -> anyhow::Result<()> {
        for run in runs {
            match run.contents {
                Contents::Extent { extent, offset } => {
                    let mut buffer = block_on(device.allocate_buffer(run.len as usize));
                    block_on(device.read(extent, offset, buffer.as_mut()))?;
                    buffer.as_ref().copy_to_slice(self.bytes(run.at, run.len));
                }
                Contents::Zeros => self.bytes(run.at, run.len).fill(0),
            }
        }
        Ok(())
    }

    fn unmap(&self, at: u64, len: u64) -> anyhow::Result<()> {
        self.bytes(at, len).fill(0xab);
        Ok(())
    }
}

/// How a test's volumes read: from the device, or from a window, SQLite's memory-mapped reads
/// reading it in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Device,
    Window,
}

/// Runs `test` in each mode.
fn both(test: impl Fn(Mode)) {
    test(Mode::Device);
    test(Mode::Window);
}

/// A volume on `device`, registered as a VFS of a name of its own.
struct Mounted {
    volume: Arc<Volume>,
    vfs: String,
    mode: Mode,
    _registration: Registration,
}

impl Mounted {
    fn new(device: DeviceHolder, mode: Mode) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let vfs = format!("fxdax-test-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let options = Options::default();
        let volume = match mode {
            Mode::Device => Volume::open(device, options),
            Mode::Window => Volume::open_mapped(device, Box::new(CopyingWindow::new()), options),
        };
        let volume = volume.expect("opening the volume");
        let registration = register(&vfs, volume.clone()).expect("registering");
        Self { volume, vfs, mode, _registration: registration }
    }

    fn connect(&self, name: &str) -> Connection {
        let connection =
            Connection::open_with_flags_and_vfs(name, OpenFlags::default(), self.vfs.as_str())
                .expect("opening the database");
        if self.mode == Mode::Window {
            connection.execute_batch("PRAGMA mmap_size = 1073741824").unwrap();
        }
        connection
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
    both(|mode| {
        let mounted = Mounted::new(DeviceHolder::new(device()), mode);
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

        let reopened = Mounted::new(mounted.snapshot(), mode);
        assert_eq!(reopened.volume.files().unwrap(), vec!["main.db".to_owned()]);
        let connection = reopened.connect("main.db");
        assert_eq!(integrity(&connection), "ok");
        assert_eq!(rows(&connection), expected);
        reopened.check_extents();
    });
}

#[test]
fn several_databases_share_a_volume() {
    both(|mode| {
        let mounted = Mounted::new(DeviceHolder::new(device()), mode);
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

        let reopened = Mounted::new(mounted.snapshot(), mode);
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
    });
}

/// A transaction committed is seen by the other connections, and one in progress isn't.
#[test]
fn connections_see_committed_transactions() {
    both(|mode| {
        let mounted = Mounted::new(DeviceHolder::new(device()), mode);
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
    });
}

/// A crash at every barrier of a run of transactions leaves the database as of the
/// transactions committed before it, whole, and the device holding what it references alone.
#[test]
fn a_crash_leaves_the_transactions_committed() {
    both(|mode| {
        for seed in 0..4 {
            let crashes = Crashes::new(seed, 1);
            let mounted =
                Mounted::new(DeviceHolder::new(device().record_crashes(crashes.clone())), mode);
            let connection = mounted.connect("main.db");
            connection
                .execute_batch(
                    "PRAGMA cache_size = 20; CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)",
                )
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
                            transaction
                                .execute("DELETE FROM t WHERE k = ?1", params![key])
                                .unwrap();
                            model.remove(&key);
                        }
                        _ => {
                            transaction
                                .execute(
                                    "INSERT OR REPLACE INTO t VALUES (?1, ?2)",
                                    params![key, v],
                                )
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
                let reopened = Mounted::new(DeviceHolder::new(device), mode);
                reopened.check_extents();
                let connection = reopened.connect("main.db");
                assert_eq!(integrity(&connection), "ok", "seed {seed}, mark {mark}");
                assert_eq!(rows(&connection), committed[mark as usize], "seed {seed}, mark {mark}");
                recovered += 1;
            }
            assert_eq!(recovered, 40, "seed {seed}");
        }
    });
}

/// What the device holds stays within a bound of the database, however often its pages are
/// written again.
#[test]
fn space_stays_bounded_under_updates() {
    both(|mode| {
        let mounted = Mounted::new(DeviceHolder::new(FakeDevice::new(1 << 18, BLOCK).calm()), mode);
        let connection = mounted.connect("main.db");
        connection.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)").unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        for k in 0..2000i64 {
            transaction
                .execute("INSERT INTO t VALUES (?1, ?2)", params![k, vec![1u8; 1000]])
                .unwrap();
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
    });
}

/// A commit whose root fails fails the volume; opened again, it holds the database as of the
/// commit before, and frees what the failed one wrote.
#[test]
fn a_failed_commit_leaves_nothing_behind() {
    both(|mode| {
        let mut device = device();
        let failing = Arc::new(AtomicBool::new(false));
        let fail = failing.clone();
        device.set_op_callback(move |op| match op {
            Op::Flush if fail.load(Ordering::Relaxed) => Err(anyhow::anyhow!("the device fails")),
            _ => Ok(()),
        });
        let mounted = Mounted::new(DeviceHolder::new(device), mode);
        let connection = mounted.connect("main.db");
        connection
            .execute_batch(
                "CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB); INSERT INTO t VALUES (1, x'01')",
            )
            .unwrap();
        failing.store(true, Ordering::Relaxed);
        let failed = connection.execute("INSERT INTO t VALUES (2, zeroblob(100000))", []);
        assert_eq!(
            failed.unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::SystemIoFailure)
        );
        assert_eq!(
            mounted.volume.check().unwrap_err().to_string(),
            "the volume failed: committing the volume's root: the device fails"
        );
        let device = mounted.snapshot();
        drop(connection);

        let reopened = Mounted::new(device, mode);
        reopened.check_extents();
        let connection = reopened.connect("main.db");
        assert_eq!(rows(&connection), BTreeMap::from([(1, vec![1])]));
    });
}

/// WAL mode would keep its log in memory, lost in a crash: SQLite keeps the rollback journal.
#[test]
fn wal_mode_is_refused() {
    both(|mode| {
        let mounted = Mounted::new(DeviceHolder::new(device()), mode);
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
        assert_eq!(
            inserted.unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::CannotOpen)
        );
        assert_eq!(mounted.volume.files().unwrap(), vec!["main.db".to_owned()]);
    });
}

/// Through a window, SQLite reads pages in place, and what a transaction changes reads back as it
/// left it, from the window, once committed, whether or not SQLite held pages as it committed.
#[test]
fn mapped_reads_follow_commits() {
    let mounted = Mounted::new(DeviceHolder::new(device()), Mode::Window);
    let writer = mounted.connect("main.db");
    let reader = mounted.connect("main.db");
    writer.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)").unwrap();
    for k in 0..300i64 {
        writer.execute("INSERT INTO t VALUES (?1, ?2)", params![k, vec![1u8; 500]]).unwrap();
    }
    let before = mounted.volume.fetched();
    assert_eq!(rows(&reader).len(), 300);
    assert!(
        mounted.volume.fetched() > before + 20,
        "{} pages fetched",
        mounted.volume.fetched() - before
    );
    // A reader holding a statement open, its pages fetched, while the writer commits.
    let mut statement = reader.prepare("SELECT v FROM t ORDER BY k").unwrap();
    let mut held = statement.query([]).unwrap();
    let first: Vec<u8> = held.next().unwrap().unwrap().get(0).unwrap();
    assert_eq!(first, vec![1u8; 500]);
    let busy = writer.execute("UPDATE t SET v = ?1", params![vec![2u8; 500]]);
    assert_eq!(busy.unwrap_err().sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy));
    drop(held);
    drop(statement);
    writer.execute("UPDATE t SET v = ?1", params![vec![2u8; 500]]).unwrap();
    let read = rows(&reader);
    assert_eq!(read.len(), 300);
    assert!(read.values().all(|v| *v == vec![2u8; 500]));
    writer.execute_batch("DELETE FROM t WHERE k >= 10; VACUUM").unwrap();
    assert_eq!(rows(&reader), (0..10).map(|k| (k, vec![2u8; 500])).collect());
    assert_eq!(integrity(&reader), "ok");
}

/// A commit made while pages are fetched maps nothing again until the last is given back: the
/// pages fetched read as before meanwhile, and the file reads as committed, from the device.
#[test]
fn a_commit_waits_for_fetched_pages_to_remap() {
    let mounted = Mounted::new(DeviceHolder::new(device()), Mode::Window);
    let volume = &mounted.volume;
    volume.open_file("f", true).unwrap();
    volume.write("f", 0, &[1; 8192]).unwrap();
    volume.sync("f").unwrap();
    let fetched = volume.fetch("f", 4096, 4096).unwrap().expect("a page in place");
    // SAFETY: fetched, until given back.
    let page = || unsafe { std::slice::from_raw_parts(fetched, 4096) }.to_vec();
    assert_eq!(page(), vec![1; 4096]);

    volume.write("f", 4096, &[2; 4096]).unwrap();
    assert_eq!(volume.fetch("f", 4096, 4096).unwrap(), None, "changed since the last commit");
    volume.sync("f").unwrap();
    assert_eq!(page(), vec![1; 4096], "held as fetched");
    assert_eq!(volume.fetch("f", 0, 4096).unwrap(), None, "the window is behind");
    let mut read = vec![0; 8192];
    assert_eq!(volume.read("f", 0, &mut read).unwrap(), 8192);
    assert_eq!(read[4096..], [2; 4096]);

    volume.unfetch("f");
    let fetched = volume.fetch("f", 4096, 4096).unwrap().expect("mapped again");
    // SAFETY: fetched, until given back.
    assert_eq!(unsafe { std::slice::from_raw_parts(fetched, 4096) }, [2; 4096]);
    volume.unfetch("f");
    volume.close_file("f");
}
