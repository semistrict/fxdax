// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of images and their volumes: filesystems of their own on one image.

use futures::executor::block_on;
use fxdax::{Errno, Filesystem, Image, Owner, RenameFlags, Storage};

const OWNER: Owner = Owner { uid: 1000, gid: 100 };

/// The size of the images the tests format.
const STORAGE_SIZE: u64 = 32 << 20;

async fn format() -> Image {
    Image::format(Storage::memory(STORAGE_SIZE)).await.expect("format failed")
}

/// Creates `name` in the root of `fs`, holding `contents`.
async fn write_file(fs: &Filesystem, name: &[u8], contents: &[u8]) {
    let (file, _) = fs.root().create(name, 0o644, OWNER).await.unwrap();
    assert_eq!(file.write(0, contents).await, Ok(contents.len()));
}

/// The contents of `name` in the root of `fs`.
async fn read_file(fs: &Filesystem, name: &[u8]) -> Vec<u8> {
    let (file, attr) = fs.root().lookup(name).await.unwrap();
    let mut buf = vec![0; attr.size as usize];
    assert_eq!(file.read(0, &mut buf).await, Ok(buf.len()));
    buf
}

/// A new image has no volumes; created ones are listed, by name.
#[test]
fn create_and_list_volumes() {
    block_on(async {
        let image = format().await;
        assert_eq!(image.volumes().await.unwrap(), Vec::<String>::new());
        let b = image.create_volume("b").await.unwrap();
        let a = image.create_volume("a").await.unwrap();
        assert_eq!(image.volumes().await.unwrap(), ["a", "b"]);
        let exists = image.create_volume("a").await.expect_err("created a volume twice");
        assert_eq!(exists.errno(), Some(Errno::EEXIST));
        let missing = image.open_volume("c").await.expect_err("opened a missing volume");
        assert_eq!(missing.errno(), Some(Errno::ENOENT));
        let mounted = image.open_volume("a").await.expect_err("mounted a volume twice");
        assert_eq!(mounted.errno(), Some(Errno::EBUSY));
        a.unmount().await.unwrap();
        b.unmount().await.unwrap();
        image.close().await.unwrap().check().await.unwrap();
    })
}

/// What one volume holds, another doesn't.
#[test]
fn volumes_are_isolated() {
    block_on(async {
        let image = format().await;
        let a = image.create_volume("a").await.unwrap();
        let b = image.create_volume("b").await.unwrap();
        write_file(&a, b"file", b"in a").await;
        assert_eq!(b.root().lookup(b"file").await.map(|_| ()), Err(Errno::ENOENT));
        write_file(&b, b"file", b"in b").await;
        assert_eq!(read_file(&a, b"file").await, b"in a");
        assert_eq!(read_file(&b, b"file").await, b"in b");
        b.root().unlink(b"file").await.unwrap();
        assert_eq!(read_file(&a, b"file").await, b"in a");
        image.close().await.unwrap().check().await.unwrap();
    })
}

/// Unmounting one volume leaves the others served; its filesystem then fails with `ENOTCONN`,
/// and it can be mounted again, with what was written to it.
#[test]
fn unmounting_a_volume_leaves_the_others() {
    block_on(async {
        let image = format().await;
        let a = image.create_volume("a").await.unwrap();
        let b = image.create_volume("b").await.unwrap();
        write_file(&a, b"file", b"kept").await;
        a.unmount().await.unwrap();
        assert_eq!(a.root().getattr().await.map(|_| ()), Err(Errno::ENOTCONN));
        write_file(&b, b"file", b"still served").await;
        // A volume's storage is its image's.
        assert!(a.close().await.is_err());
        let a = image.open_volume("a").await.unwrap();
        assert_eq!(read_file(&a, b"file").await, b"kept");
        drop(a);
        image.close().await.unwrap().check().await.unwrap();
    })
}

/// Dropping a volume's filesystem unmounts it, writing back what it held: it can be mounted again.
#[test]
fn dropping_a_volume_unmounts_it() {
    block_on(async {
        let image = format().await;
        let a = image.create_volume("a").await.unwrap();
        write_file(&a, b"file", b"dropped").await;
        drop(a);
        let a = image.open_volume("a").await.unwrap();
        assert_eq!(read_file(&a, b"file").await, b"dropped");
        drop(a);
        image.close().await.unwrap().check().await.unwrap();
    })
}

/// Closing an image unmounts the volumes still mounted, and what they held survives.
#[test]
fn closing_the_image_unmounts_its_volumes() {
    block_on(async {
        let image = format().await;
        let a = image.create_volume("a").await.unwrap();
        write_file(&a, b"file", b"written before closing").await;
        let storage = image.close().await.unwrap();
        assert_eq!(a.root().getattr().await.map(|_| ()), Err(Errno::ENOTCONN));
        drop(a);

        let image = Image::mount(storage.check().await.unwrap()).await.unwrap();
        assert_eq!(image.volumes().await.unwrap(), ["a"]);
        let a = image.open_volume("a").await.unwrap();
        assert_eq!(read_file(&a, b"file").await, b"written before closing");
        drop(a);
        image.close().await.unwrap();
    })
}

/// A mounted volume can't be deleted; an unmounted one can, with everything in it.
#[test]
fn delete_volumes() {
    block_on(async {
        let image = format().await;
        let a = image.create_volume("a").await.unwrap();
        write_file(&a, b"file", &[7; 1 << 20]).await;
        let busy = image.delete_volume("a").await.expect_err("deleted a mounted volume");
        assert_eq!(busy.errno(), Some(Errno::EBUSY));
        a.unmount().await.unwrap();
        image.delete_volume("a").await.unwrap();
        assert_eq!(image.volumes().await.unwrap(), Vec::<String>::new());
        let missing = image.delete_volume("a").await.expect_err("deleted a volume twice");
        assert_eq!(missing.errno(), Some(Errno::ENOENT));
        image.close().await.unwrap().check().await.unwrap();
    })
}

/// A filesystem mounted from storage is the image's `default` volume, which an image sees.
#[test]
fn a_filesystem_is_the_default_volume() {
    block_on(async {
        let fs = Filesystem::format(Storage::memory(STORAGE_SIZE)).await.unwrap();
        write_file(&fs, b"file", b"single").await;
        let image = Image::mount(fs.close().await.unwrap()).await.unwrap();
        assert_eq!(image.volumes().await.unwrap(), ["default"]);
        let fs = image.open_volume("default").await.unwrap();
        assert_eq!(read_file(&fs, b"file").await, b"single");
        drop(fs);
        let fs = Filesystem::mount(image.close().await.unwrap()).await.unwrap();
        assert_eq!(read_file(&fs, b"file").await, b"single");
        fs.close().await.unwrap().check().await.unwrap();
    })
}

/// A snapshot of a mounted volume has everything written to it, synced or not, and from then on
/// each changes on its own.
#[test]
fn snapshot_a_mounted_volume() {
    block_on(async {
        let image = format().await;
        let source = image.create_volume("source").await.unwrap();
        write_file(&source, b"file", b"before the snapshot").await;
        image.snapshot_volume("source", "snapshot").await.unwrap();
        let snapshot = image.open_volume("snapshot").await.unwrap();
        assert_eq!(read_file(&snapshot, b"file").await, b"before the snapshot");

        write_file(&source, b"only-in-source", b"after").await;
        let (file, _) = snapshot.root().lookup(b"file").await.unwrap();
        assert_eq!(file.write(0, b"BEFORE").await, Ok(6));
        assert_eq!(read_file(&source, b"file").await, b"before the snapshot");
        assert_eq!(read_file(&snapshot, b"file").await, b"BEFORE the snapshot");
        let missing = snapshot.root().lookup(b"only-in-source").await.map(|_| ());
        assert_eq!(missing, Err(Errno::ENOENT));

        let exists = image.snapshot_volume("source", "snapshot").await.expect_err("snapshot");
        assert_eq!(exists.errno(), Some(Errno::EEXIST));
        let missing = image.snapshot_volume("missing", "other").await.expect_err("snapshot");
        assert_eq!(missing.errno(), Some(Errno::ENOENT));

        let image =
            Image::mount(image.close().await.unwrap().check().await.unwrap()).await.unwrap();
        assert_eq!(image.volumes().await.unwrap(), ["snapshot", "source"]);
        let source = image.open_volume("source").await.unwrap();
        let snapshot = image.open_volume("snapshot").await.unwrap();
        assert_eq!(read_file(&source, b"file").await, b"before the snapshot");
        assert_eq!(read_file(&snapshot, b"file").await, b"BEFORE the snapshot");
        drop((source, snapshot));
        image.close().await.unwrap().check().await.unwrap();
    })
}

/// A snapshot of a volume mounted again, while the filesystem it was unmounted from lives, has
/// what its new mount wrote, unsynced, to a file still open: the snapshot writes back the mounted
/// volume's dirty data, not the unmounted one's.
#[test]
fn snapshot_a_volume_mounted_again() {
    block_on(async {
        let image = format().await;
        let unmounted = image.create_volume("source").await.unwrap();
        unmounted.unmount().await.unwrap();
        let source = image.open_volume("source").await.unwrap();
        let (file, _) = source.root().create(b"file", 0o644, OWNER).await.unwrap();
        assert_eq!(file.write(0, b"after the remount").await, Ok(17));
        image.snapshot_volume("source", "snapshot").await.unwrap();
        let snapshot = image.open_volume("snapshot").await.unwrap();
        assert_eq!(read_file(&snapshot, b"file").await, b"after the remount");
        drop((file, unmounted, source, snapshot));
        image.close().await.unwrap().check().await.unwrap();
    })
}

/// A preallocated file, whose blocks are written in place, is written to new blocks where a
/// snapshot shares them, through the pager's writeback.
#[test]
fn snapshot_a_preallocated_file() {
    block_on(async {
        let image = format().await;
        let source = image.create_volume("source").await.unwrap();
        let (file, _) = source.root().create(b"file", 0o644, OWNER).await.unwrap();
        file.allocate(0, 1 << 20).await.unwrap();
        assert_eq!(file.write(0, &[1; 1 << 16]).await, Ok(1 << 16));
        image.snapshot_volume("source", "snapshot").await.unwrap();
        let snapshot = image.open_volume("snapshot").await.unwrap();

        // The source's file is open across the snapshot.
        assert_eq!(file.write(0, &[2; 1 << 15]).await, Ok(1 << 15));
        assert_eq!(file.write(1 << 19, &[3; 1 << 15]).await, Ok(1 << 15));
        file.fsync().await.unwrap();
        let (snapshot_file, _) = snapshot.root().lookup(b"file").await.unwrap();
        assert_eq!(snapshot_file.write(1 << 15, &[4; 1 << 15]).await, Ok(1 << 15));
        snapshot_file.fsync().await.unwrap();

        let mut expected_source = vec![0; 1 << 20];
        expected_source[..1 << 16].fill(1);
        let mut expected_snapshot = expected_source.clone();
        expected_source[..1 << 15].fill(2);
        expected_source[1 << 19..(1 << 19) + (1 << 15)].fill(3);
        expected_snapshot[1 << 15..1 << 16].fill(4);
        assert_eq!(read_file(&source, b"file").await, expected_source);
        assert_eq!(read_file(&snapshot, b"file").await, expected_snapshot);

        drop((source, snapshot));
        let image =
            Image::mount(image.close().await.unwrap().check().await.unwrap()).await.unwrap();
        let source = image.open_volume("source").await.unwrap();
        let snapshot = image.open_volume("snapshot").await.unwrap();
        assert_eq!(read_file(&source, b"file").await, expected_source);
        assert_eq!(read_file(&snapshot, b"file").await, expected_snapshot);
        drop((source, snapshot));
        image.close().await.unwrap().check().await.unwrap();
    })
}

/// A node is its own filesystem's: renames and links into another volume's directories fail with
/// `EXDEV`, and change nothing.
#[test]
fn nodes_of_two_volumes_do_not_mix() {
    block_on(async {
        let image = format().await;
        let a = image.create_volume("a").await.unwrap();
        let b = image.create_volume("b").await.unwrap();
        let (file, _) = a.root().create(b"file", 0o644, OWNER).await.unwrap();
        let renamed = a.root().rename(b"file", b.root(), b"moved", RenameFlags::empty()).await;
        assert_eq!(renamed, Err(Errno::EXDEV));
        assert_eq!(file.link(b.root(), b"link").await.map(|_| ()), Err(Errno::EXDEV));
        assert_eq!(b.root().entries().await.unwrap(), []);
        assert_eq!(a.root().lookup(b"file").await.unwrap().1.inode, file.inode());
        image.close().await.unwrap().check().await.unwrap();
    })
}
