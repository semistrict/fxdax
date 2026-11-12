// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! SQLite on a volume on a store an extent daemon serves, in this process, as a host's would
//! serve it from its own (TASK-300): pages read in place in the daemon's window.

#![cfg(target_os = "linux")]

mod common;

use futures::executor::block_on;
use rusqlite::{Connection, OpenFlags, params};
use sqlite_extents::{Options, Registration, Volume, daemon, register};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage_device::DeviceHolder;

/// A daemon on a thread of this process, as a host of the multi-node system runs it: a bucket in
/// its memory, placement 1. Its socket is in a directory of its own in the system's temporary
/// directory, which lives as long as the one returned: a socket's path is short (SUN_LEN), and one
/// under the target directory of a checkout deep in a host's scratch disk was not (TASK-344).
fn serve() -> (tempfile::TempDir, PathBuf) {
    serve_from("memory:///sqlite-extents")
}

/// A daemon as [`serve`] runs it, of the bucket at `url`.
fn serve_from(url: &str) -> (tempfile::TempDir, PathBuf) {
    // SAFETY: before this process starts any other thread.
    unsafe {
        std::env::set_var("FXDAX_EXTENTD_REMOTE", url);
        std::env::set_var("FXDAX_EXTENTD_TOKEN", "1");
    }
    let sockets = tempfile::tempdir().unwrap();
    let socket = sockets.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || fxdax::extentd::serve(listener));
    (sockets, socket)
}

#[test]
fn a_database_on_the_daemon_reads_in_place_and_survives_reopening() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (_sockets, socket) = serve();
    let store = dir.path().join("store");

    for round in 0..2i64 {
        let volume = daemon::open(&socket, &store, 1 << 30, 1 << 30, Options::default()).unwrap();
        let vfs = format!("fxdax-daemon-{round}");
        let registration = register(&vfs, volume.clone()).unwrap();
        let connection =
            Connection::open_with_flags_and_vfs("app.db", OpenFlags::default(), vfs.as_str())
                .unwrap();
        connection.execute_batch("PRAGMA mmap_size = 1073741824; CREATE TABLE IF NOT EXISTS t (k INTEGER PRIMARY KEY, v BLOB)").unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        for k in 0..2000 {
            transaction
                .execute(
                    "INSERT OR REPLACE INTO t VALUES (?1, ?2)",
                    params![k, vec![round as u8; 300]],
                )
                .unwrap();
        }
        transaction.commit().unwrap();
        for k in (0..2000).step_by(7) {
            connection
                .execute("UPDATE t SET v = ?2 WHERE k = ?1", params![k, vec![9u8; 300]])
                .unwrap();
        }
        let before = volume.fetched();
        let check: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0)).unwrap();
        assert_eq!(check, "ok");
        let (count, nines): (i64, i64) = connection
            .query_row("SELECT count(*), sum(v = ?1) FROM t", params![vec![9u8; 300]], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!((count, nines), (2000, 286));
        assert!(volume.fetched() > before + 100, "{} pages fetched", volume.fetched() - before);
        drop(connection);
        drop(registration);
        volume.close().unwrap();
    }
}

/// A volume on the store at `store`, mapped, registered as the VFS `vfs`, a connection to its
/// database reading it in place, and the store, to fork.
struct Mounted {
    store: Arc<fxdax::RemoteExtentStore>,
    volume: Arc<Volume>,
    _registration: Registration,
    connection: Connection,
}

impl Mounted {
    fn new(socket: &Path, store: &Path, vfs: &str) -> Self {
        let store = Arc::new(fxdax::RemoteExtentStore::connect(socket, store, 1 << 30).unwrap());
        let window = daemon::DaemonWindow::new(store.clone(), 1 << 30).unwrap();
        let device = DeviceHolder::new(fxdax::RemoteDevice::new(store.clone()).unwrap());
        let volume = Volume::open_mapped(device, Box::new(window), Options::default()).unwrap();
        let registration = register(vfs, volume.clone()).unwrap();
        let connection =
            Connection::open_with_flags_and_vfs("app.db", OpenFlags::default(), vfs).unwrap();
        connection.execute_batch("PRAGMA mmap_size = 1073741824").unwrap();
        Self { store, volume, _registration: registration, connection }
    }

    /// Checks the device holds every extent the volume names, those it references and those it
    /// has yet to free: so that none it frees, or failed to, is one the device never held, which
    /// it would try to free again at each commit; and closes the volume.
    fn close(self) {
        let Self { store, volume, _registration, connection } = self;
        drop(connection);
        let device = volume.device();
        let mut buffer = block_on(device.allocate_buffer(device.block_size() as usize));
        for extent in volume.extents().unwrap() {
            block_on(device.read(extent, 0, buffer.as_mut()))
                .unwrap_or_else(|error| panic!("{extent:?} the volume names: {error:#}"));
        }
        drop((store, _registration));
        volume.close().unwrap();
    }
}

/// A fork of a store on the daemon, as of its last commit, holds the database as it was, inherited
/// with the store's extents and pages, while the volume on the store is open, frees in its root
/// still to make; the two then change apart, the fork rewriting and so freeing all it inherited,
/// and each reads its own, in place, before and after opening again.
#[test]
fn a_fork_of_a_store_and_the_store_go_their_own_ways() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (_sockets, socket) = serve();
    let (path, forked) = (dir.path().join("store"), dir.path().join("fork"));
    let parent = Mounted::new(&socket, &path, "fxdax-daemon-parent");
    let mut parent_rows = common::fill(&parent.connection);
    parent.store.fork(&forked).unwrap();
    let fork = Mounted::new(&socket, &forked, "fxdax-daemon-fork");
    let mut fork_rows = parent_rows.clone();
    common::check(&fork.connection, &fork_rows);

    common::change_parent(&parent.connection, &mut parent_rows);
    common::change_fork(&fork.connection, &mut fork_rows);
    common::check(&parent.connection, &parent_rows);
    common::check(&fork.connection, &fork_rows);
    fork.close();
    parent.close();

    for (round, (path, rows)) in [(&path, parent_rows), (&forked, fork_rows)].iter().enumerate() {
        let mounted = Mounted::new(&socket, path, &format!("fxdax-daemon-reopened-{round}"));
        common::check(&mounted.connection, rows);
        mounted.close();
    }
}

/// TASK-364: a SQLite volume is a volume of its cell, fenced with it. Another host, of a later
/// placement, takes the cell, and the volume's branch with it: the first host's commit, durable on
/// its disk alone, is not in the bucket, its wait for it failing; its commits after it fail.
#[test]
fn a_database_is_fenced_with_its_cell() {
    use extent_remote::cell;
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let bucket = dir.path().join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    let url = format!("file://{}", bucket.display());
    let (sockets, socket) = serve_from(&url);
    let control_socket = sockets.path().join("control.sock");
    let listener = UnixListener::bind(&control_socket).unwrap();
    let tenants = sockets.path().join("tenants");
    std::fs::create_dir(&tenants).unwrap();
    std::thread::spawn(move || fxdax::extentd::control::serve_control(listener, tenants));
    let control = fxdax::extentd::control::Client::connect(&control_socket).unwrap();

    let store = dir.path().join("store");
    let volume = daemon::open(&socket, &store, 1 << 30, 1 << 30, Options::default()).unwrap();
    let _registration = register("fxdax-fenced", volume.clone()).unwrap();
    let connection =
        Connection::open_with_flags_and_vfs("app.db", OpenFlags::default(), "fxdax-fenced")
            .unwrap();
    connection.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY, v TEXT)").unwrap();
    connection.execute("INSERT INTO t VALUES (1, 'ours')", []).unwrap();
    control.sync_store(&store).unwrap();
    let ours = control.branch(&store).unwrap();
    let objects = extent_remote::store::Store::from_url(&url).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let held = runtime.block_on(cell::read(&objects, ours.cell)).unwrap().unwrap();
    let named = cell::Volume { branch: ours.id, kind: cell::Kind::Sqlite };
    assert_eq!(held.volumes, std::collections::BTreeMap::from([("store".to_owned(), named)]));
    assert_eq!((held.epoch, held.token), (1, 1));

    // Another host's, placed later.
    let objects = std::sync::Arc::new(objects);
    runtime.block_on(async {
        let (held, _) = cell::hold(&*objects, ours.cell, (2, 2), false, None).await.unwrap();
        // Its writer fences the branch (TASK-371).
        drop(extent_remote::Writer::open(objects.clone(), ours, 0x22, held.epoch).await.unwrap());
    });
    connection.execute("INSERT INTO t VALUES (2, 'cut off')", []).unwrap();
    let fenced = control.sync_store(&store).unwrap_err();
    assert_eq!(fenced.errno(), Some(fxdax::Errno::EROFS), "{fenced:#}");
    let refused = connection.execute("INSERT INTO t VALUES (3, 'refused')", []).unwrap_err();
    assert_eq!(
        refused.sqlite_error_code(),
        Some(rusqlite::ErrorCode::SystemIoFailure),
        "{refused}"
    );
}
