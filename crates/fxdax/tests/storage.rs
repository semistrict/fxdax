// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of image storage: direct I/O, and clones of closed and mounted filesystems.
//!
//! Images live under the build's temporary directory, which is on the repository's filesystem,
//! where direct I/O works, rather than on a `/tmp` that may be a tmpfs.

use futures::executor::block_on;
use fxdax::{Filesystem, Inode, Owner, Storage};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const OWNER: Owner = Owner { uid: 1000, gid: 100 };
const IMAGE_SIZE: u64 = 64 << 20;

/// A new directory for a test's images.
fn images() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

/// A formatted image at `path`, holding `file` with `contents`, closed.
async fn image_with(path: &Path, file: &[u8], contents: &[u8]) -> Storage {
    let fs = Filesystem::format(Storage::create(path, IMAGE_SIZE).unwrap()).await.unwrap();
    let inode = fs.create(Inode::ROOT, file, 0o644, OWNER).await.unwrap().inode;
    fs.write(inode, 0, contents).await.unwrap();
    fs.close().await.unwrap()
}

/// The contents of `file` in the root of `fs`.
async fn read(fs: &Filesystem, file: &[u8]) -> Vec<u8> {
    let attr = fs.lookup(Inode::ROOT, file).await.unwrap();
    let mut buf = vec![0; attr.size as usize];
    assert_eq!(fs.read(attr.inode, 0, &mut buf).await, Ok(buf.len()));
    buf
}

/// The bytes of the host's storage the files under `path` take.
fn allocated(path: &Path) -> u64 {
    let metadata = std::fs::symlink_metadata(path).unwrap();
    if !metadata.is_dir() {
        return metadata.blocks() * 512;
    }
    std::fs::read_dir(path).unwrap().map(|entry| allocated(&entry.unwrap().path())).sum()
}

/// Image I/O bypasses the host's page cache, which the in-process kernel stands in for.
#[test]
fn images_bypass_the_page_cache() {
    let dir = images();
    let storage = Storage::create(dir.path().join("image"), IMAGE_SIZE).unwrap();
    assert!(storage.bypasses_page_cache());
    assert!(!Storage::memory(IMAGE_SIZE).bypasses_page_cache());
}

/// A clone of closed storage is a separate image with the same contents, which shares the image's
/// data rather than copies it.
#[test]
fn clone_closed_storage() {
    block_on(async {
        let dir = images();
        let (path, clone_path): (PathBuf, PathBuf) =
            (dir.path().join("image"), dir.path().join("clone"));
        let storage = image_with(&path, b"file", b"original").await;
        let before = allocated(&path);
        let clone = storage.clone_to(&clone_path).await.expect("clone failed");
        // A handle naming the image, and a record of the fork in it.
        assert!(allocated(&clone_path) <= 16 << 10, "{} bytes", allocated(&clone_path));
        assert!(allocated(&path) <= before + (64 << 10), "{} > {before}", allocated(&path));
        assert!(storage.clone_to(&clone_path).await.is_err(), "cloned over an image");

        // Each changes independently of the other.
        let fs = Filesystem::mount(clone.check().await.unwrap()).await.unwrap();
        assert_eq!(read(&fs, b"file").await, b"original");
        let file = fs.lookup(Inode::ROOT, b"file").await.unwrap().inode;
        fs.write(file, 0, b"modified").await.unwrap();
        fs.close().await.unwrap().check().await.unwrap();

        let fs = Filesystem::mount(storage.check().await.unwrap()).await.unwrap();
        assert_eq!(read(&fs, b"file").await, b"original");
        fs.close().await.unwrap();
    })
}

/// A clone of a mounted filesystem has what was written before it, even if never synced, and not
/// what is written after.
#[test]
fn clone_mounted_filesystem() {
    block_on(async {
        let dir = images();
        let (path, clone_path) = (dir.path().join("image"), dir.path().join("clone"));
        let storage = image_with(&path, b"before", b"synced").await;
        let fs = Filesystem::mount(storage).await.unwrap();
        let file = fs.create(Inode::ROOT, b"unsynced", 0o644, OWNER).await.unwrap().inode;
        fs.write(file, 0, b"dirty pages").await.unwrap();

        let clone = fs.clone_storage_to(&clone_path).await.expect("clone failed");
        fs.write(file, 0, b"after clone").await.unwrap();
        fs.close().await.unwrap().check().await.unwrap();

        let fs = Filesystem::mount(clone.check().await.unwrap()).await.unwrap();
        assert_eq!(read(&fs, b"before").await, b"synced");
        assert_eq!(read(&fs, b"unsynced").await, b"dirty pages");
        fs.close().await.unwrap();
    })
}

/// An image deleted while a clone of it lives keeps its directory, which holds what the clone
/// shares of it, and no longer mounts; the clone mounts as before, and its deletion takes the
/// image's directory too (TASK-39.4).
#[test]
fn delete_images_and_their_clones() {
    block_on(async {
        let dir = images();
        let (path, clone_path) = (dir.path().join("image"), dir.path().join("clone"));
        let storage = image_with(&path, b"file", b"original").await;
        let clone = storage.clone_to(&clone_path).await.unwrap();
        assert!(Storage::delete(&clone_path).is_err(), "deleted an open clone");
        assert!(Storage::delete(&path).is_err(), "deleted an open image");
        drop((clone, storage));
        Storage::delete(&path).unwrap();
        assert!(path.exists());
        let error = Storage::open(&path).expect_err("opened a deleted image");
        assert_eq!(
            format!("{error:#}"),
            format!(
                "opening {}: the store was deleted: its directory holds its forks' data alone",
                path.display()
            )
        );
        let fs = Filesystem::mount(Storage::open(&clone_path).unwrap()).await.unwrap();
        assert_eq!(read(&fs, b"file").await, b"original");
        fs.close().await.unwrap();
        Storage::delete(&clone_path).unwrap();
        assert!(!clone_path.exists());
        assert!(!path.exists());
    })
}

#[test]
fn memory_cannot_be_cloned() {
    block_on(async {
        let dir = images();
        let storage = Storage::memory(IMAGE_SIZE);
        let error = storage.clone_to(dir.path().join("clone")).await.expect_err("cloned memory");
        assert_eq!(error.to_string(), "memory cannot be cloned to an image");
        let fs = Filesystem::format(storage).await.unwrap();
        let error = fs.clone_storage_to(dir.path().join("clone")).await.expect_err("cloned memory");
        assert_eq!(error.to_string(), "memory cannot be cloned to an image");
        fs.close().await.unwrap();
    })
}
