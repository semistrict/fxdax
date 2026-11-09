// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! SQLite on a volume on a store an extent daemon serves, in this process, as a host's would
//! serve it from its own (TASK-300): pages read in place in the daemon's window.

#![cfg(target_os = "linux")]

use rusqlite::{Connection, OpenFlags, params};
use sqlite_extents::{Options, daemon, register};
use std::os::unix::net::UnixListener;

#[test]
fn a_database_on_the_daemon_reads_in_place_and_survives_reopening() {
    // A host of the multi-node system, as the daemon is: a bucket in its memory, placement 1.
    // SAFETY: before this process starts any other thread.
    unsafe {
        std::env::set_var("FXDAX_EXTENTD_REMOTE", "memory:///sqlite-extents");
        std::env::set_var("FXDAX_EXTENTD_TOKEN", "1");
    }
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || fxdax::extentd::serve(listener));
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
