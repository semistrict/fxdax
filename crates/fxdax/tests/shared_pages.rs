// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests that the files of a volume and its snapshots share the memory of what they share on the
//! image: a page of memory per block read, until written.
//!
//! Each test counts the pages backing files, which nextest's process per test keeps to its own.

use futures::executor::block_on;
use fxdax::{Filesystem, Image, Inode, Owner, Storage, kernel};

const OWNER: Owner = Owner { uid: 1000, gid: 100 };

/// The size of the files the tests read.
const FILE_SIZE: u64 = 1 << 20;

/// The pages the kernel holds for files, which the pager supplied.
fn file_pages() -> u64 {
    let counts = zircon::vm::pmm::node().page_queues().queue_counts();
    (counts.reclaim.iter().sum::<usize>()
        + counts.reclaim_isolate
        + counts.pager_backed_dirty
        + counts.high_priority) as u64
}

fn contents() -> Vec<u8> {
    (0..FILE_SIZE).map(|i| (i % 251) as u8).collect()
}

async fn read_file(fs: &Filesystem, inode: Inode) -> Vec<u8> {
    let mut buf = vec![0; FILE_SIZE as usize];
    assert_eq!(fs.read(inode, 0, &mut buf).await, Ok(buf.len()));
    buf
}

/// An image with a volume `source` holding `file`, and its snapshot `snapshot`, mounted afresh so
/// that neither's file has pages yet.
async fn snapshot_image() -> Image {
    let image = Image::format(Storage::memory(32 << 20)).await.unwrap();
    let source = image.create_volume("source").await.unwrap();
    let file = source.create(Inode::ROOT, b"file", 0o644, OWNER).await.unwrap().inode;
    assert_eq!(source.write(file, 0, &contents()).await, Ok(FILE_SIZE as usize));
    image.snapshot_volume("source", "snapshot").await.unwrap();
    drop(source);
    Image::mount(image.close().await.unwrap()).await.unwrap()
}

async fn lookup(fs: &Filesystem) -> Inode {
    fs.lookup(Inode::ROOT, b"file").await.unwrap().inode
}

#[test]
fn a_volume_and_its_snapshot_share_pages() {
    block_on(async {
        let image = snapshot_image().await;
        let source = image.open_volume("source").await.unwrap();
        let snapshot = image.open_volume("snapshot").await.unwrap();
        let (source_file, snapshot_file) = (lookup(&source).await, lookup(&snapshot).await);
        let before = file_pages();

        assert_eq!(read_file(&source, source_file).await, contents());
        let pages = FILE_SIZE / kernel::page_size();
        assert_eq!(file_pages() - before, pages);
        assert_eq!(read_file(&snapshot, snapshot_file).await, contents());
        assert_eq!(file_pages() - before, pages, "the snapshot's file has pages of its own");

        // A write gives the file a page of its own, which the other doesn't see.
        assert_eq!(snapshot.write(snapshot_file, 1, b"written").await, Ok(7));
        assert_eq!(file_pages() - before, pages + 1);
        let mut expected = contents();
        expected[1..8].copy_from_slice(b"written");
        assert_eq!(read_file(&snapshot, snapshot_file).await, expected);
        assert_eq!(read_file(&source, source_file).await, contents());

        drop((source, snapshot));
        image.close().await.unwrap().check().await.unwrap();
    })
}
