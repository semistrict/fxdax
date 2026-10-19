// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of importing container images into filesystems (TASK-39.5).

use futures::executor::block_on;
use fxdax::import::testing::{Entry, Kind, MTIME, gzip, layer, oci_layout, opaque, whiteout};
use fxdax::import::{Importer, OciImage};
use fxdax::{Attr, Errno, FileType, Filesystem, Inode, Storage};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The size of the filesystems the tests format.
const STORAGE_SIZE: u64 = 64 << 20;

fn time(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// A new filesystem, with the layers `layers` applied in turn.
async fn imported(layers: &[Vec<u8>]) -> Filesystem {
    let fs = Filesystem::format(Storage::memory(STORAGE_SIZE)).await.unwrap();
    let mut importer = Importer::new(&fs);
    for layer in layers {
        importer.apply(layer.as_slice()).await.expect("applying a layer failed");
    }
    importer.finish().await.expect("finishing failed");
    fs
}

/// Closes and checks `fs`.
async fn close(fs: Filesystem) {
    fs.close().await.unwrap().check().await.expect("check failed");
}

/// The node at `path`.
async fn at(fs: &Filesystem, path: &str) -> fxdax::Result<Attr> {
    let mut attr = fs.getattr(Inode::ROOT).await?;
    for name in path.split('/').filter(|name| !name.is_empty()) {
        attr = fs.lookup(attr.inode, name.as_bytes()).await?;
    }
    Ok(attr)
}

async fn contents(fs: &Filesystem, inode: Inode) -> Vec<u8> {
    let mut buf = vec![0; fs.getattr(inode).await.unwrap().size as usize];
    let mut read = 0;
    while read < buf.len() {
        let len = fs.read(inode, read as u64, &mut buf[read..]).await.unwrap();
        assert_ne!(len, 0, "the file ends early");
        read += len;
    }
    buf
}

/// The names in the directory at `path`, sorted.
async fn names(fs: &Filesystem, path: &str) -> Vec<String> {
    let dir = at(fs, path).await.unwrap().inode;
    let handle = fs.opendir(dir).await.unwrap();
    let entries = fs.readdir(handle, 0).unwrap();
    fs.releasedir(handle);
    let mut names: Vec<String> =
        entries.iter().skip(2).map(|e| String::from_utf8(e.name.clone()).unwrap()).collect();
    names.sort();
    names
}

#[test]
fn a_layer_makes_files_with_their_owners_modes_links_and_xattrs() {
    let holey = [&b"head"[..], &[0; 128 << 10], b"tail", &[0; 128 << 10]].concat();
    let capability = [1, 0, 0, 2, 0x20, 0, 0, 0];
    let layer = layer(&[
        Entry::new("etc", Kind::Dir).mtime(MTIME + 1),
        Entry::new("etc/passwd", Kind::File(b"root:x:0:0::/root:/bin/sh\n")).mtime(MTIME + 2),
        Entry::new("tmp", Kind::Dir).perm(0o1777),
        Entry::new("usr/bin/sudo", Kind::File(b"#!/bin/sudo")).perm(0o4755),
        Entry::new("bin/ping", Kind::File(b"ping"))
            .perm(0o755)
            .xattrs(&[("security.capability", &capability), ("user.origin", b"test")]),
        Entry::new("home/user", Kind::Dir).perm(0o700).owner(1000, 1000),
        Entry::new("home/user/notes", Kind::File(b"mine")).perm(0o600).owner(1000, 100),
        Entry::new("home/user/link", Kind::Symlink("notes")).owner(1000, 100),
        Entry::new("home/user/hard", Kind::HardLink("home/user/notes")),
        Entry::new("dev/null", Kind::CharDevice(1, 3)).perm(0o666),
        Entry::new("dev/sda1", Kind::BlockDevice(8, 1)).perm(0o660).owner(0, 6),
        Entry::new("run/fifo", Kind::Fifo).perm(0o620),
        Entry::new("var/holey", Kind::File(&holey)),
    ])
    .unwrap();
    block_on(async {
        let fs = imported(&[layer]).await;
        let etc = at(&fs, "etc").await.unwrap();
        assert_eq!(
            (etc.file_type, etc.perm, etc.mtime),
            (FileType::Directory, 0o755, time(MTIME + 1))
        );
        let passwd = at(&fs, "etc/passwd").await.unwrap();
        assert_eq!((passwd.perm, passwd.uid, passwd.gid), (0o644, 0, 0));
        assert_eq!((passwd.mtime, passwd.atime), (time(MTIME + 2), time(MTIME + 2)));
        assert_eq!(contents(&fs, passwd.inode).await, b"root:x:0:0::/root:/bin/sh\n");
        assert_eq!(at(&fs, "tmp").await.unwrap().perm, 0o1777);
        assert_eq!(at(&fs, "usr/bin/sudo").await.unwrap().perm, 0o4755);
        // Directories no entry made: root's, 0755, at the epoch.
        let usr = at(&fs, "usr").await.unwrap();
        assert_eq!((usr.perm, usr.uid, usr.mtime), (0o755, 0, UNIX_EPOCH));

        let ping = at(&fs, "bin/ping").await.unwrap().inode;
        let mut xattrs = fs.listxattr(ping).await.unwrap();
        xattrs.sort();
        assert_eq!(xattrs, [b"security.capability".to_vec(), b"user.origin".to_vec()]);
        assert_eq!(fs.getxattr(ping, b"security.capability").await.unwrap(), capability);
        assert_eq!(fs.getxattr(ping, b"user.origin").await.unwrap(), b"test");

        let home = at(&fs, "home/user").await.unwrap();
        assert_eq!((home.perm, home.uid, home.gid), (0o700, 1000, 1000));
        let notes = at(&fs, "home/user/notes").await.unwrap();
        assert_eq!((notes.perm, notes.uid, notes.gid, notes.nlink), (0o600, 1000, 100, 2));
        let hard = at(&fs, "home/user/hard").await.unwrap();
        assert_eq!(hard.inode, notes.inode);
        let link = at(&fs, "home/user/link").await.unwrap();
        assert_eq!((link.file_type, link.uid, link.gid), (FileType::Symlink, 1000, 100));
        assert_eq!(link.mtime, time(MTIME));
        assert_eq!(fs.readlink(link.inode).await.unwrap(), b"notes");

        let null = at(&fs, "dev/null").await.unwrap();
        assert_eq!((null.file_type, null.perm, null.rdev), (FileType::CharDevice, 0o666, 0x103));
        let sda1 = at(&fs, "dev/sda1").await.unwrap();
        assert_eq!((sda1.file_type, sda1.rdev, sda1.gid), (FileType::BlockDevice, 0x801, 6));
        let fifo = at(&fs, "run/fifo").await.unwrap();
        assert_eq!((fifo.file_type, fifo.perm), (FileType::Fifo, 0o620));

        // The zeros are holes: two pages hold data, the head's and the tail's.
        let holey_attr = at(&fs, "var/holey").await.unwrap();
        assert_eq!(holey_attr.size, holey.len() as u64);
        assert_eq!(holey_attr.blocks * 512, 2 * fxdax::kernel::page_size());
        assert_eq!(contents(&fs, holey_attr.inode).await, holey);
        close(fs).await;
    })
}

#[test]
fn layers_apply_in_turn_with_whiteouts() {
    let base = layer(&[
        Entry::new("app", Kind::Dir),
        Entry::new("app/config", Kind::File(b"base")),
        Entry::new("app/data", Kind::Dir),
        Entry::new("app/data/a", Kind::File(b"a")),
        Entry::new("app/data/b", Kind::File(b"b")),
        Entry::new("app/cache/old", Kind::File(b"old")),
        Entry::new("opaque", Kind::Dir),
        Entry::new("opaque/below", Kind::File(b"below")),
        Entry::new("becomes-dir", Kind::File(b"file")),
        Entry::new("becomes-file/inside", Kind::File(b"inside")),
        Entry::new("kept", Kind::Dir).perm(0o750),
        Entry::new("kept/child", Kind::File(b"child")),
    ])
    .unwrap();
    let top = layer(&[
        Entry::new("app/config", Kind::File(b"top")),
        Entry::new(&whiteout("app/data/a"), Kind::File(b"")),
        Entry::new(&whiteout("app/cache"), Kind::File(b"")),
        Entry::new(&whiteout("never/there"), Kind::File(b"")),
        Entry::new("opaque/above", Kind::File(b"above")),
        Entry::new(&opaque("opaque"), Kind::File(b"")),
        Entry::new("becomes-dir", Kind::Dir),
        Entry::new("becomes-file", Kind::File(b"now a file")),
        // A directory over a directory keeps its entries, and takes the new attributes.
        Entry::new("kept", Kind::Dir).perm(0o711).owner(5, 5).mtime(MTIME + 9),
    ])
    .unwrap();
    block_on(async {
        let fs = imported(&[base, top]).await;
        assert_eq!(contents(&fs, at(&fs, "app/config").await.unwrap().inode).await, b"top");
        assert_eq!(names(&fs, "app").await, ["config", "data"]);
        assert_eq!(names(&fs, "app/data").await, ["b"]);
        // An opaque whiteout keeps what its own layer made.
        assert_eq!(names(&fs, "opaque").await, ["above"]);
        assert_eq!(at(&fs, "becomes-dir").await.unwrap().file_type, FileType::Directory);
        let file = at(&fs, "becomes-file").await.unwrap();
        assert_eq!(contents(&fs, file.inode).await, b"now a file");
        let kept = at(&fs, "kept").await.unwrap();
        assert_eq!((kept.perm, kept.uid, kept.mtime), (0o711, 5, time(MTIME + 9)));
        assert_eq!(names(&fs, "kept").await, ["child"]);
        assert_eq!(names(&fs, "").await, ["app", "becomes-dir", "becomes-file", "kept", "opaque"]);
        assert_eq!(at(&fs, "never").await.unwrap_err(), Errno::ENOENT);
        close(fs).await;
    })
}

/// Everything about the nodes in `fs` that an image gives them, by path: all but their inode
/// numbers and change times.
async fn manifest(fs: &Filesystem) -> Vec<String> {
    let mut manifest = Vec::new();
    let mut dirs = vec![(String::new(), Inode::ROOT)];
    while let Some((path, dir)) = dirs.pop() {
        for name in names(fs, &path).await {
            let path = format!("{path}/{name}");
            let attr = fs.lookup(dir, name.as_bytes()).await.unwrap();
            let mut xattrs = fs.listxattr(attr.inode).await.unwrap();
            xattrs.sort();
            let mut line = format!(
                "{path} {:?} {:o} {}:{} size={} nlink={} rdev={:x} mtime={:?} atime={:?}",
                attr.file_type,
                attr.perm,
                attr.uid,
                attr.gid,
                attr.size,
                attr.nlink,
                attr.rdev,
                attr.mtime,
                attr.atime,
            );
            for xattr in xattrs {
                let value = fs.getxattr(attr.inode, &xattr).await.unwrap();
                line += &format!(" {}={value:?}", String::from_utf8_lossy(&xattr));
            }
            match attr.file_type {
                FileType::Directory => dirs.push((path, attr.inode)),
                FileType::Symlink => line += &format!(" -> {:?}", fs.readlink(attr.inode).await),
                FileType::File => line += &format!(" {:?}", contents(fs, attr.inode).await),
                _ => {}
            }
            manifest.push(line);
        }
    }
    manifest.sort();
    manifest
}

/// Imports the image in `layout` into a new filesystem, and returns its manifest.
fn import_layout(layout: &Path, reference: Option<&str>) -> Vec<String> {
    block_on(async {
        let image = OciImage::open(layout, reference).expect("opening the image failed");
        let fs = Filesystem::format(Storage::memory(STORAGE_SIZE)).await.unwrap();
        let mut importer = Importer::new(&fs);
        image.import(&mut importer).await.expect("importing failed");
        importer.finish().await.unwrap();
        let manifest = manifest(&fs).await;
        close(fs).await;
        manifest
    })
}

fn sample_layers() -> Vec<Vec<u8>> {
    let base = layer(&[
        Entry::new("bin/sh", Kind::File(b"\x7fELF")).perm(0o755),
        Entry::new("bin/ping", Kind::File(b"ping")).xattrs(&[("user.a", b"1"), ("user.b", b"2")]),
        Entry::new("etc/hosts", Kind::File(b"127.0.0.1 localhost\n")).mtime(MTIME + 5),
        Entry::new("etc/alias", Kind::Symlink("hosts")).owner(3, 4),
        Entry::new("etc/again", Kind::HardLink("etc/hosts")),
        Entry::new("dev/zero", Kind::CharDevice(1, 5)),
        Entry::new("gone", Kind::File(b"gone")),
    ])
    .unwrap();
    let top = layer(&[
        Entry::new(&whiteout("gone"), Kind::File(b"")),
        Entry::new("etc/motd", Kind::File(b"hello\n")).owner(1, 2),
    ])
    .unwrap();
    vec![gzip(&base).unwrap(), top]
}

#[test]
fn the_same_image_gives_the_same_files() {
    let dir = tempfile::tempdir().unwrap();
    let layout = dir.path().join("image");
    oci_layout(&layout, "sample:1", &sample_layers()).unwrap();

    let first = import_layout(&layout, Some("sample:1"));
    let second = import_layout(&layout, None);
    assert_eq!(first, second);
    let line = |path: &str| first.iter().find(|line| line.starts_with(path)).cloned();
    assert_eq!(
        line("/etc/hosts").unwrap(),
        format!(
            "/etc/hosts File 644 0:0 size=20 nlink=2 rdev=0 mtime={:?} atime={:?} {:?}",
            time(MTIME + 5),
            time(MTIME + 5),
            b"127.0.0.1 localhost\n"
        )
    );
    assert!(line("/etc/motd").unwrap().starts_with("/etc/motd File 644 1:2 size=6"));
    assert!(line("/bin/ping").unwrap().contains(" user.a=[49] user.b=[50]"));
    assert_eq!(line("/gone"), None);
    assert_eq!(first.len(), 10, "{first:#?}");
}

#[test]
fn an_image_must_be_named_and_its_blobs_whole() {
    let dir = tempfile::tempdir().unwrap();
    let layout = dir.path().join("image");
    oci_layout(&layout, "sample:1", &sample_layers()).unwrap();
    let error = OciImage::open(&layout, Some("other:2")).err().expect("opened");
    assert_eq!(format!("{error:#}").rsplit(": ").next().unwrap(), "no image is named \"other:2\"");

    // A layer whose contents are not its digest's: the gzipped one.
    let gzipped: Vec<_> = std::fs::read_dir(layout.join("blobs/sha256"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| std::fs::read(path).unwrap().starts_with(&[0x1f, 0x8b]))
        .collect();
    let [base_digest] = gzipped.as_slice() else { panic!("gzipped layers: {gzipped:?}") };
    let mut corrupt = std::fs::read(base_digest).unwrap();
    *corrupt.last_mut().unwrap() ^= 1;
    std::fs::write(base_digest, corrupt).unwrap();
    block_on(async {
        let image = OciImage::open(&layout, None).unwrap();
        let fs = Filesystem::format(Storage::memory(STORAGE_SIZE)).await.unwrap();
        let mut importer = Importer::new(&fs);
        let error = image.import(&mut importer).await.expect_err("imported a corrupt layer");
        let hex = base_digest.file_name().unwrap().to_str().unwrap();
        assert!(
            format!("{error:#}").starts_with(&format!("the blob sha256:{hex} holds sha256:")),
            "{error:#}"
        );
        drop(importer);
        close(fs).await;
    })
}

/// A layer compressed with zstd is refused, saying so.
#[test]
fn zstd_layers_are_refused() {
    block_on(async {
        let fs = Filesystem::format(Storage::memory(STORAGE_SIZE)).await.unwrap();
        let mut importer = Importer::new(&fs);
        let layer = [0x28, 0xb5, 0x2f, 0xfd, 0, 0, 0, 0];
        let error = importer.apply(&layer[..]).await.expect_err("applied");
        assert_eq!(format!("{error:#}"), "zstd-compressed layers are not supported");
        drop(importer);
        close(fs).await;
    })
}

#[test]
fn paths_stay_in_the_image() {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(3);
    header.set_mode(0o644);
    // `append_data` refuses `..`, as importing does; the name is written as is.
    header.as_gnu_mut().unwrap().name[..11].copy_from_slice(b"../escaped\0");
    header.set_cksum();
    let mut builder = tar::Builder::new(Vec::new());
    builder.append(&header, &b"out"[..]).unwrap();
    let layer = builder.into_inner().unwrap();
    block_on(async {
        let fs = Filesystem::format(Storage::memory(STORAGE_SIZE)).await.unwrap();
        let mut importer = Importer::new(&fs);
        let error = importer.apply(layer.as_slice()).await.expect_err("applied");
        assert_eq!(
            format!("{error:#}"),
            "applying \"../escaped\": the path \"../escaped\" goes through .."
        );
        drop(importer);
        close(fs).await;
    })
}

/// Runs fxdaxctl with `args`, and returns whether it succeeded, and what it printed to stderr.
fn fxdaxctl(args: &[&str]) -> (bool, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_fxdaxctl")).args(args).output();
    let output = output.expect("running fxdaxctl failed");
    (output.status.success(), String::from_utf8_lossy(&output.stderr).into_owned())
}

/// The manifest of the filesystem in the store at `dir`.
fn store_manifest(dir: &Path) -> Vec<String> {
    block_on(async {
        let fs = Filesystem::mount(Storage::open(dir).unwrap()).await.unwrap();
        let manifest = manifest(&fs).await;
        close(fs).await;
        manifest
    })
}

#[test]
fn fxdaxctl_makes_stores_of_images() {
    let dir = tempfile::tempdir().unwrap();
    let layout = dir.path().join("image");
    let layers = sample_layers();
    oci_layout(&layout, "sample:1", &layers).unwrap();
    let path = |name: &str| dir.path().join(name).to_str().unwrap().to_owned();

    let (ok, stderr) = fxdaxctl(&["import", &path("oci"), "67108864", &path("image"), "sample:1"]);
    assert!(ok, "{stderr}");
    assert_eq!(store_manifest(Path::new(&path("oci"))), import_layout(&layout, None));

    // A `docker export` tarball: one layer.
    std::fs::write(path("export.tar"), &layers[0]).unwrap();
    let (ok, stderr) = fxdaxctl(&["import", &path("export"), "67108864", &path("export.tar")]);
    assert!(ok, "{stderr}");
    let manifest = store_manifest(Path::new(&path("export")));
    assert_eq!(manifest.iter().filter(|line| line.starts_with("/gone File")).count(), 1);
    assert_eq!(manifest.len(), 10, "{manifest:#?}");

    // An image that cannot be imported leaves no store behind.
    let (ok, stderr) = fxdaxctl(&["import", &path("missing"), "67108864", &path("image"), "x:1"]);
    assert!(!ok);
    assert!(stderr.contains("no image is named \"x:1\""), "{stderr}");
    assert!(!dir.path().join("missing").exists());
    // Nor does it touch what is there.
    let (ok, stderr) = fxdaxctl(&["import", &path("oci"), "67108864", &path("image")]);
    assert!(!ok);
    assert!(stderr.contains("File exists"), "{stderr}");
    assert_eq!(store_manifest(Path::new(&path("oci"))), import_layout(&layout, None));
}
