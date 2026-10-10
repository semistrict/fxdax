//! Tests that the files of a volume and its snapshots share the memory of what they share on the
//! image: a page of memory per block read, until written.
//!
//! Each test counts the pages backing files, which nextest's process per test keeps to its own.

use futures::executor::block_on;
use fxdax::{Access, DaxWindow, Filesystem, Image, Inode, Owner, Storage, kernel};

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

fn window_bytes(window: &DaxWindow, offset: u64, len: usize) -> &mut [u8] {
    // SAFETY: The caller faulted the pages in, and nothing else accesses them while the test does.
    unsafe { std::slice::from_raw_parts_mut(window.host_base().add(offset as usize), len) }
}

#[test]
fn dax_maps_shared_pages_and_stores_copy_them() {
    block_on(async {
        let image = snapshot_image().await;
        let source = image.open_volume("source").await.unwrap();
        let snapshot = image.open_volume("snapshot").await.unwrap();
        let (source_file, snapshot_file) = (lookup(&source).await, lookup(&snapshot).await);
        assert_eq!(read_file(&source, source_file).await, contents());
        let before = file_pages();

        let ps = kernel::page_size();
        let window = snapshot.create_dax_window(2 * ps, None).unwrap();
        snapshot.setup_mapping(snapshot_file, 0, 2 * ps, 0, true).await.unwrap();
        window.fault(0, Access::Read).await.unwrap();
        window.fault(ps, Access::Read).await.unwrap();
        assert_eq!(window_bytes(&window, 0, 2 * ps as usize), &contents()[..2 * ps as usize]);
        assert_eq!(file_pages(), before, "mapping the snapshot's file gave it pages of its own");

        window.fault(ps, Access::Write).await.unwrap();
        window_bytes(&window, ps + 10, 6).copy_from_slice(b"stored");
        assert_eq!(file_pages(), before + 1);
        let mut expected = contents();
        expected[ps as usize + 10..ps as usize + 16].copy_from_slice(b"stored");
        assert_eq!(read_file(&snapshot, snapshot_file).await, expected);
        assert_eq!(read_file(&source, source_file).await, contents());
        // The other page is still shared, through the window too.
        assert_eq!(window_bytes(&window, 0, ps as usize), &contents()[..ps as usize]);

        snapshot.fsync(snapshot_file).await.unwrap();
        snapshot.remove_mapping(0, 2 * ps).unwrap();
        drop((source, snapshot));
        let image =
            Image::mount(image.close().await.unwrap().check().await.unwrap()).await.unwrap();
        let snapshot = image.open_volume("snapshot").await.unwrap();
        assert_eq!(read_file(&snapshot, lookup(&snapshot).await).await, expected);
        drop(snapshot);
        image.close().await.unwrap().check().await.unwrap();
    })
}
