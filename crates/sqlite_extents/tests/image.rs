// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! SQLite on a volume on an fxdax image: an extent store on disk, its branch kept in a bucket
//! beside it (TASK-296).

use fxdax::Storage;
use rusqlite::{Connection, OpenFlags, params};
use sqlite_extents::{Options, Volume, register};

#[test]
fn a_database_on_an_image_survives_closing() {
    // Under the build's temporary directory, where direct I/O works, as fxdax's image tests have.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let path = dir.path().join("image");
    {
        let volume = Volume::open(
            Storage::create(&path, 256 << 20).unwrap().into_device(),
            Options::default(),
        )
        .unwrap();
        let registration = register("fxdax-image-write", volume.clone()).unwrap();
        let connection = Connection::open_with_flags_and_vfs(
            "app.db",
            OpenFlags::default(),
            "fxdax-image-write",
        )
        .unwrap();
        connection.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)").unwrap();
        for round in 0..10i64 {
            let transaction = connection.unchecked_transaction().unwrap();
            for k in 0..300 {
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO t VALUES (?1, ?2)",
                        params![k, vec![round as u8; 700]],
                    )
                    .unwrap();
            }
            transaction.commit().unwrap();
        }
        drop(connection);
        drop(registration);
        volume.close().unwrap();
    }

    let volume =
        Volume::open(Storage::open(&path).unwrap().into_device(), Options::default()).unwrap();
    assert_eq!(volume.files().unwrap(), vec!["app.db".to_owned()]);
    let registration = register("fxdax-image-read", volume.clone()).unwrap();
    let connection =
        Connection::open_with_flags_and_vfs("app.db", OpenFlags::default(), "fxdax-image-read")
            .unwrap();
    let check: String =
        connection.query_row("PRAGMA integrity_check", [], |row| row.get(0)).unwrap();
    assert_eq!(check, "ok");
    let (count, sum): (i64, i64) = connection
        .query_row("SELECT count(*), sum(unicode(substr(v, 1, 1))) FROM t", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!((count, sum), (300, 300 * 9));
    drop(connection);
    drop(registration);
    volume.close().unwrap();
}
