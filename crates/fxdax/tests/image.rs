//! Tests of images and their volumes: filesystems of their own on one image.

use futures::executor::block_on;
use fxdax::fuse::Server;
use fxdax::{Errno, Filesystem, Image, Inode, Owner, Storage, VolatileSlice, VolatileSliceMut};

const OWNER: Owner = Owner { uid: 1000, gid: 100 };

/// The size of the images the tests format.
const STORAGE_SIZE: u64 = 32 << 20;

async fn format() -> Image {
    Image::format(Storage::memory(STORAGE_SIZE)).await.expect("format failed")
}

/// Creates `name` in the root of `fs`, holding `contents`.
async fn write_file(fs: &Filesystem, name: &[u8], contents: &[u8]) {
    let inode = fs.create(Inode::ROOT, name, 0o644, OWNER).await.unwrap().inode;
    assert_eq!(fs.write(inode, 0, contents).await, Ok(contents.len()));
}

/// The contents of `name` in the root of `fs`.
async fn read_file(fs: &Filesystem, name: &[u8]) -> Vec<u8> {
    let attr = fs.lookup(Inode::ROOT, name).await.unwrap();
    let mut buf = vec![0; attr.size as usize];
    assert_eq!(fs.read(attr.inode, 0, &mut buf).await, Ok(buf.len()));
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
        assert_eq!(b.lookup(Inode::ROOT, b"file").await.map(|_| ()), Err(Errno::ENOENT));
        write_file(&b, b"file", b"in b").await;
        assert_eq!(read_file(&a, b"file").await, b"in a");
        assert_eq!(read_file(&b, b"file").await, b"in b");
        b.unlink(Inode::ROOT, b"file").await.unwrap();
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
        assert_eq!(a.getattr(Inode::ROOT).await.map(|_| ()), Err(Errno::ENOTCONN));
        write_file(&b, b"file", b"still served").await;
        // A volume's storage is its image's.
        assert!(a.close().await.is_err());
        let a = image.open_volume("a").await.unwrap();
        assert_eq!(read_file(&a, b"file").await, b"kept");
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
        assert_eq!(a.getattr(Inode::ROOT).await.map(|_| ()), Err(Errno::ENOTCONN));
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

/// Two volumes of one image serve two devices: each its own FUSE server.
#[test]
fn a_fuse_server_per_volume() {
    block_on(async {
        let image = format().await;
        let servers = [
            Server::new(image.create_volume("a").await.unwrap()),
            Server::new(image.create_volume("b").await.unwrap()),
        ];
        write_file(servers[0].filesystem(), b"only-in-a", b"").await;
        // FUSE_LOOKUP of the file in the root of each.
        let mut request = [0u8; 50];
        request[0..4].copy_from_slice(&50u32.to_le_bytes());
        request[4..8].copy_from_slice(&1u32.to_le_bytes());
        request[16..24].copy_from_slice(&1u64.to_le_bytes());
        request[40..50].copy_from_slice(b"only-in-a\0");
        let mut errors = Vec::new();
        for server in &servers {
            let mut reply = [0u8; 256];
            let len = server
                .handle(
                    &[VolatileSlice::from(&request[..])],
                    &mut [VolatileSliceMut::from(&mut reply[..])],
                )
                .await
                .unwrap();
            assert!(len >= 16);
            errors.push(i32::from_le_bytes(reply[4..8].try_into().unwrap()));
        }
        assert_eq!(errors, [0, -2]);
        drop(servers);
        image.close().await.unwrap().check().await.unwrap();
    })
}
