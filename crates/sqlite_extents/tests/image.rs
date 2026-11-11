// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! SQLite on a volume on an fxdax image: an extent store on disk, its branch kept in a bucket
//! beside it (TASK-296).

mod common;

use fxdax::Storage;
use rusqlite::{Connection, OpenFlags, params};
use sqlite_extents::{Options, Registration, Volume, register};
use std::path::Path;
use std::sync::Arc;

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

/// A volume on `storage`, registered as the VFS `vfs`, and a connection to its database.
fn mount(storage: Storage, vfs: &str) -> (Arc<Volume>, Registration, Connection) {
    let volume = Volume::open(storage.into_device(), Options::default()).unwrap();
    let registration = register(vfs, volume.clone()).unwrap();
    let connection =
        Connection::open_with_flags_and_vfs("app.db", OpenFlags::default(), vfs).unwrap();
    (volume, registration, connection)
}

fn unmount((volume, registration, connection): (Arc<Volume>, Registration, Connection)) {
    drop(connection);
    held(&volume);
    drop(registration);
    volume.close().unwrap();
}

/// Checks the device holds every extent the volume names, those it references and those it has yet
/// to free: so that none it frees, or failed to, is one the device never held, which it would try
/// to free again at each commit.
fn held(volume: &Volume) {
    let device = volume.device();
    let mut buffer =
        futures::executor::block_on(device.allocate_buffer(device.block_size() as usize));
    for extent in volume.extents().unwrap() {
        futures::executor::block_on(device.read(extent, 0, buffer.as_mut()))
            .unwrap_or_else(|error| panic!("{extent:?} the volume names: {error:#}"));
    }
}

fn open(path: &Path) -> Storage {
    Storage::open(path).unwrap()
}

/// A clone of an image, a fork of its branch in its bucket, holds the image's database as it was,
/// inheriting its extents; the two then change apart, the clone rewriting and so freeing all it
/// inherited, and each reads its own, before and after opening again.
#[test]
fn a_clone_and_its_image_go_their_own_ways() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (path, clone) = (dir.path().join("image"), dir.path().join("clone"));
    let mounted = mount(Storage::create(&path, 256 << 20).unwrap(), "fxdax-clone-fill");
    let mut parent_rows = common::fill(&mounted.2);
    unmount(mounted);

    let cloned = futures::executor::block_on(open(&path).clone_to(&clone)).unwrap();
    let parent = mount(open(&path), "fxdax-clone-parent");
    let fork = mount(cloned, "fxdax-clone-fork");
    let mut fork_rows = parent_rows.clone();
    common::check(&fork.2, &fork_rows);
    common::change_parent(&parent.2, &mut parent_rows);
    common::change_fork(&fork.2, &mut fork_rows);
    common::check(&parent.2, &parent_rows);
    common::check(&fork.2, &fork_rows);
    unmount(fork);
    unmount(parent);

    for (path, rows) in [(&path, parent_rows), (&clone, fork_rows)] {
        let mounted = mount(open(path), "fxdax-clone-reopened");
        common::check(&mounted.2, &rows);
        unmount(mounted);
    }
}
