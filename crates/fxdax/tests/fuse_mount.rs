// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of stores mounted with FUSE through Linux's kernel (`fxdax::fuse::MountOptions`,
//! `fxdax-mount`, TASK-222), used with ordinary file system calls, then read back from the store.
//!
//! A test run by root mounts with mount(2); any other user's, through `fusermount3`, which the
//! hosts the tests run on have (libfuse 3's `fuse3`: .github/workflows/ci.yml,
//! scripts/lib/linux-host-setup.sh, scripts/lima-setup.sh).

#![cfg(target_os = "linux")]

use futures::executor::block_on;
use fxdax::fuse::MountOptions;
use fxdax::{Filesystem, SetAttr, Storage};
use std::ffi::CString;
use std::io::{BufRead as _, ErrorKind, Read as _, Seek as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const STORE_SIZE: u64 = 256 << 20;

/// A new store at `path`, formatted, its root directory the test's user's, as a store made for
/// them would be: mounted by them, the kernel checks their permissions.
fn format(path: &Path) -> Filesystem {
    let fs = block_on(Filesystem::format(Storage::create(path, STORE_SIZE).unwrap())).unwrap();
    // SAFETY: Plain calls returning the process's IDs.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let owner = SetAttr { uid: Some(uid), gid: Some(gid), ..Default::default() };
    block_on(fs.root().setattr(&owner)).unwrap();
    fs
}

/// The store and mount point of a test, in `dir`.
fn paths(dir: &Path) -> (PathBuf, PathBuf) {
    let mountpoint = dir.join("mnt");
    std::fs::create_dir(&mountpoint).unwrap();
    (dir.join("store"), mountpoint)
}

/// The names in the directory `path`, sorted.
fn names(path: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn cstring(path: &Path) -> CString {
    CString::new(path.as_os_str().as_bytes()).unwrap()
}

/// The contents of `name`, a file in `dir`, read through the store's filesystem.
async fn contents(dir: &fxdax::Node, name: &[u8]) -> Vec<u8> {
    let (file, attr) = dir.lookup(name).await.unwrap();
    let mut contents = vec![0; attr.size as usize];
    assert_eq!(file.read(0, &mut contents).await, Ok(contents.len()));
    contents
}

/// A pattern of `len` bytes that no shift of it repeats within a page.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// A store mounted through the kernel is used as any filesystem is: directories, files read and
/// written, large ones too, permissions, symlinks, hard links, renames, listings, a file unlinked
/// while open, truncations, extended attributes, statvfs, FIFOs, exclusive creates, fsync. Once
/// unmounted and closed, the store is whole, and holds what was done.
#[test]
fn a_store_mounted_through_the_kernel_is_used_as_any_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let (store, m) = paths(dir.path());
    let mount = MountOptions::new().mount(format(&store), &m).expect("mount failed");
    assert_eq!(mount.mountpoint(), m);

    std::fs::create_dir_all(m.join("a/b")).unwrap();
    std::fs::write(m.join("a/b/file"), "hello").unwrap();
    assert_eq!(std::fs::read_to_string(m.join("a/b/file")).unwrap(), "hello");
    let metadata = std::fs::metadata(m.join("a/b/file")).unwrap();
    assert_eq!((metadata.len(), metadata.is_file()), (5, true));
    // SAFETY: Plain calls returning the process's IDs.
    assert_eq!((metadata.uid(), metadata.gid()), unsafe { (libc::getuid(), libc::getgid()) });

    std::fs::set_permissions(m.join("a/b/file"), std::fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(std::fs::metadata(m.join("a/b/file")).unwrap().mode() & 0o7777, 0o640);

    std::os::unix::fs::symlink("b/file", m.join("a/link")).unwrap();
    assert_eq!(std::fs::read_link(m.join("a/link")).unwrap(), Path::new("b/file"));
    assert_eq!(std::fs::read_to_string(m.join("a/link")).unwrap(), "hello");
    std::fs::hard_link(m.join("a/b/file"), m.join("a/hard")).unwrap();
    assert_eq!(std::fs::metadata(m.join("a/b/file")).unwrap().nlink(), 2);
    std::fs::rename(m.join("a/hard"), m.join("renamed")).unwrap();
    assert_eq!(names(&m.join("a")), ["b", "link"]);
    assert_eq!(names(&m), ["a", "renamed"]);

    // Larger than a write or read the kernel sends at once.
    let large = pattern((3 << 20) + 123);
    std::fs::write(m.join("large"), &large).unwrap();
    assert_eq!(std::fs::read(m.join("large")).unwrap(), large);

    // A file unlinked while open is read and written through the open file.
    let mut open = std::fs::File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(m.join("temporary"))
        .unwrap();
    open.write_all(b"temporary").unwrap();
    std::fs::remove_file(m.join("temporary")).unwrap();
    assert_eq!(std::fs::metadata(m.join("temporary")).unwrap_err().kind(), ErrorKind::NotFound);
    open.rewind().unwrap();
    let mut read = String::new();
    open.read_to_string(&mut read).unwrap();
    assert_eq!((read.as_str(), open.metadata().unwrap().nlink()), ("temporary", 0));
    drop(open);

    // Truncations: by an open, and by a length set.
    std::fs::write(m.join("truncated"), "contents").unwrap();
    std::fs::File::create(m.join("truncated")).unwrap();
    assert_eq!(std::fs::metadata(m.join("truncated")).unwrap().len(), 0);
    let file = std::fs::File::options().write(true).open(m.join("truncated")).unwrap();
    file.set_len(10).unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert_eq!(std::fs::read(m.join("truncated")).unwrap(), [0; 10]);

    // Extended attributes.
    let renamed = cstring(&m.join("renamed"));
    // SAFETY: setxattr(2) and getxattr(2), with NUL-terminated strings and buffers that outlive
    // the calls.
    unsafe {
        let value = b"attribute";
        let set = libc::setxattr(
            renamed.as_ptr(),
            c"user.fxdax".as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        );
        assert_eq!(set, 0, "setxattr: {}", std::io::Error::last_os_error());
        let mut got = [0u8; 64];
        let len = libc::getxattr(
            renamed.as_ptr(),
            c"user.fxdax".as_ptr(),
            got.as_mut_ptr().cast(),
            got.len(),
        );
        assert_eq!(len, value.len() as isize, "getxattr: {}", std::io::Error::last_os_error());
        assert_eq!(&got[..value.len()], value);
    }

    // SAFETY: statvfs(2) into a struct this test owns, of a NUL-terminated path.
    let statvfs = unsafe {
        let mut statvfs: libc::statvfs = std::mem::zeroed();
        assert_eq!(libc::statvfs(cstring(&m).as_ptr(), &mut statvfs), 0);
        statvfs
    };
    assert_eq!((statvfs.f_bsize, statvfs.f_blocks), (4096, STORE_SIZE / 4096));
    assert!(statvfs.f_bfree < statvfs.f_blocks, "{} blocks free", statvfs.f_bfree);

    // SAFETY: mkfifo(3), with a NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(cstring(&m.join("fifo")).as_ptr(), 0o644) }, 0);
    assert!(std::fs::metadata(m.join("fifo")).unwrap().file_type().is_fifo());

    let exclusive = std::fs::File::options().write(true).create_new(true).open(m.join("renamed"));
    assert_eq!(exclusive.unwrap_err().kind(), ErrorKind::AlreadyExists);
    assert_eq!(std::fs::remove_dir(m.join("a")).unwrap_err().kind(), ErrorKind::DirectoryNotEmpty);
    std::fs::File::open(m.join("renamed")).unwrap().sync_all().unwrap();

    mount.unmount(false).expect("unmount failed");
    let storage = mount.join().expect("closing failed");
    let storage = block_on(storage.check()).expect("the store is not whole");

    block_on(async {
        let fs = Filesystem::mount(storage).await.unwrap();
        let root = fs.root();
        let (a, _) = root.lookup(b"a").await.unwrap();
        let (b, _) = a.lookup(b"b").await.unwrap();
        assert_eq!(contents(&b, b"file").await, b"hello");
        assert_eq!(contents(root, b"renamed").await, b"hello");
        assert_eq!(contents(root, b"large").await, large);
        assert_eq!(contents(root, b"truncated").await, [0; 10]);
        let (renamed, attr) = root.lookup(b"renamed").await.unwrap();
        assert_eq!((attr.nlink, attr.perm), (2, 0o640));
        assert_eq!(renamed.getxattr(b"user.fxdax").await.unwrap(), b"attribute");
        let (link, _) = a.lookup(b"link").await.unwrap();
        assert_eq!(link.readlink().await.unwrap(), b"b/file");
        let (_, fifo) = root.lookup(b"fifo").await.unwrap();
        assert_eq!(fifo.file_type, fxdax::FileType::Fifo);
        assert_eq!(root.lookup(b"temporary").await.unwrap_err(), fxdax::Errno::ENOENT);
        drop((a, b, renamed, link));
        fs.close().await.unwrap();
    });
}

/// A mount dropped before it is joined is unmounted, lazily, and closed: its store is whole, holds
/// what was written, and may be opened again.
#[test]
fn a_mount_dropped_unmounts_and_closes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, m) = paths(dir.path());
    let mount = MountOptions::new().mount(format(&store), &m).unwrap();
    std::fs::write(m.join("file"), "written").unwrap();
    drop(mount);
    assert_eq!(names(&m), Vec::<String>::new(), "still mounted");
    block_on(async {
        let storage = Storage::open(&store).unwrap().check().await.unwrap();
        let fs = Filesystem::mount(storage).await.unwrap();
        assert_eq!(contents(fs.root(), b"file").await, b"written");
        fs.close().await.unwrap();
    });
}

/// A child process, killed if the test ends while it runs.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `fxdax-mount [FLAGS] STORE MOUNTPOINT`, its output piped, once it has said it is mounted.
fn fxdax_mount(flags: &[&str], store: &Path, mountpoint: &Path) -> KillOnDrop {
    let mut child = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_fxdax-mount"))
            .args(flags)
            .arg(store)
            .arg(mountpoint)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut said = String::new();
    std::io::BufReader::new(child.0.stdout.take().unwrap()).read_line(&mut said).unwrap();
    if said.is_empty() {
        let mut stderr = String::new();
        child.0.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
        panic!("fxdax-mount ended unmounted, {}: {stderr}", child.0.wait().unwrap());
    }
    assert_eq!(said.trim_end(), mountpoint.to_str().unwrap());
    child
}

/// `fxdax-mount` serves a store until SIGTERM, which unmounts it; then it closes it and exits 0,
/// and the store holds what was written through the mount.
#[test]
fn fxdax_mount_serves_a_store_until_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let (store, m) = paths(dir.path());
    block_on(format(&store).close()).unwrap();
    let child = fxdax_mount(&[], &store, &m);
    std::fs::create_dir(m.join("dir")).unwrap();
    std::fs::write(m.join("dir/file"), "through fxdax-mount").unwrap();
    terminate(child, &m);
    block_on(async {
        let storage = Storage::open(&store).unwrap().check().await.unwrap();
        let fs = Filesystem::mount(storage).await.unwrap();
        let (dir, _) = fs.root().lookup(b"dir").await.unwrap();
        assert_eq!(contents(&dir, b"file").await, b"through fxdax-mount");
        drop(dir);
        fs.close().await.unwrap();
    });
}

/// Sends `fxdax-mount`'s `child` SIGTERM, and checks that it unmounted `mountpoint` and exited 0.
fn terminate(mut child: KillOnDrop, mountpoint: &Path) {
    // SAFETY: Signals a process this test started.
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
    let status = child.0.wait().unwrap();
    let mut stderr = String::new();
    child.0.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    assert!(status.success(), "{status}: {stderr}");
    assert_eq!(names(mountpoint), Vec::<String>::new(), "still mounted");
}

/// `fxdax-mount --new CAPACITY` makes a new store of CAPACITY bytes, its filesystem empty, its
/// root a new volume's (0755, root's), and mounts it; a store already there it refuses, as it does
/// a capacity that isn't a number.
#[test]
fn fxdax_mount_makes_a_new_store() {
    let dir = tempfile::tempdir().unwrap();
    let (store, m) = paths(dir.path());
    let child = fxdax_mount(&["--new", "67108864"], &store, &m);
    assert_eq!(names(&m), Vec::<String>::new());
    let root = std::fs::metadata(&m).unwrap();
    assert_eq!((root.mode() & 0o7777, root.uid(), root.gid()), (0o755, 0, 0));
    terminate(child, &m);
    block_on(async {
        let storage = Storage::open(&store).unwrap().check().await.unwrap();
        let fs = Filesystem::mount(storage).await.unwrap();
        assert_eq!(fs.statfs().await.unwrap().blocks, 67108864 / 4096);
        fs.close().await.unwrap();
    });

    for (args, said) in [
        (vec!["--new", "67108864"], "making"),
        (vec!["--new", "lots"], "--new CAPACITY, in bytes?"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_fxdax-mount"))
            .args(args)
            .arg(&store)
            .arg(&m)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "mounted");
        assert!(stderr.contains(said), "{stderr}");
    }
}

/// A store mounted is its mount's alone: `fxdax-mount` fails to open it, once it has waited for it
/// as long as the extent store waits for another process's (`TAKE_WAIT`), exits unsuccessfully,
/// and says why.
#[test]
fn a_store_mounted_is_not_mounted_again() {
    let dir = tempfile::tempdir().unwrap();
    let (store, m) = paths(dir.path());
    let mount = MountOptions::new().mount(format(&store), &m).unwrap();
    let other = dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fxdax-mount")).arg(&store).arg(&other).output();
    let output = output.unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "mounted twice");
    assert!(stderr.contains(&format!("opening {}", store.display())), "{stderr}");
    assert!(stderr.contains("another process has the store's branches open"), "{stderr}");
    assert_eq!(names(&other), Vec::<String>::new());
    mount.unmount(false).unwrap();
    block_on(mount.join().unwrap().check()).unwrap();
}

/// `fxdax-mount --control SOCKET --stores DIR --bucket URL MOUNTPOINT`, all in `dir`, once it has
/// said it is mounted (TASK-302): its child, its control socket and its mount point.
fn fxdax_mount_stores(dir: &Path) -> (KillOnDrop, PathBuf, PathBuf) {
    let (socket, stores, bucket) = (dir.join("control"), dir.join("stores"), dir.join("bucket"));
    std::fs::create_dir(&stores).unwrap();
    std::fs::create_dir(&bucket).unwrap();
    let bucket = url::Url::from_directory_path(&bucket).unwrap().to_string();
    let mountpoint = dir.join("mnt");
    std::fs::create_dir(&mountpoint).unwrap();
    let flags = ["--control", socket.to_str().unwrap(), "--stores", stores.to_str().unwrap()];
    let mut child = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_fxdax-mount"))
            .args(flags)
            .args(["--bucket", &bucket])
            .arg(&mountpoint)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut said = String::new();
    std::io::BufReader::new(child.0.stdout.take().unwrap()).read_line(&mut said).unwrap();
    if said.is_empty() {
        let mut stderr = String::new();
        child.0.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
        panic!("fxdax-mount ended unmounted, {}: {stderr}", child.0.wait().unwrap());
    }
    assert_eq!(said.trim_end(), mountpoint.to_str().unwrap());
    (child, socket, mountpoint)
}

/// `fxdax-mount COMMAND ARGS`: what it prints, once it succeeds.
fn fxdax_mount_command(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_fxdax-mount")).args(args).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "fxdax-mount {args:?}: {}: {stderr}", output.status);
    String::from_utf8(output.stdout).unwrap()
}

/// One mount of many stores (TASK-302), of tenants' cells (TASK-364): a cell attached through its
/// control socket shows each Fxfs volume of it at TENANT/CELL/VOLUME, a filesystem of its own
/// (nothing renamed between them, nothing made in the mount's directories of its own); its cell's
/// objects in the bucket under its tenant's prefix. A cell detached goes, written back, so that it
/// attaches again from its stores, or, those gone, from the bucket; a volume with a file open is
/// detached once the file is closed.
#[test]
fn one_mount_attaches_and_detaches_many_stores() {
    let dir = tempfile::tempdir().unwrap();
    let (child, socket, m) = fxdax_mount_stores(dir.path());
    let socket = socket.to_str().unwrap();
    assert_eq!(names(&m), Vec::<String>::new());
    let capacity = STORE_SIZE.to_string();
    let (one, two) = ("0000000000000001", "0000000000000002");
    // A new cell of each tenant's, of no volumes: nothing shown.
    let attach = |tenant: &str, cell: &str| {
        fxdax_mount_command(&["attach", socket, tenant, cell]).lines().map(str::to_owned).collect()
    };
    let new: Vec<String> = attach("1", "0");
    let a = new[0].clone();
    let new: Vec<String> = attach("2", "0");
    let b = new[0].clone();
    assert_eq!(names(&m), Vec::<String>::new());
    let volume = |tenant: &str, cell: &str, name: &str| {
        let made = ["create-volume", socket, tenant, cell, name, &capacity];
        fxdax_mount_command(&made).trim().to_owned()
    };
    let (root, data, other) =
        (volume("1", &a, "root"), volume("1", &a, "data"), volume("2", &b, "root"));
    assert_eq!(names(&m), [one, two]);
    assert_eq!(names(&m.join(one)), [a.as_str()]);
    assert_eq!(names(&m.join(one).join(&a)), ["data", "root"]);
    assert_eq!(names(&m.join(two).join(&b)), ["root"]);
    let (at_a, at_b) = (m.join(one).join(&a), m.join(two).join(&b));
    std::fs::write(at_a.join("root/file"), "in root").unwrap();
    std::fs::write(at_a.join("data/file"), "in data").unwrap();
    std::fs::write(at_b.join("root/file"), "in b").unwrap();
    assert_eq!(std::fs::read_to_string(at_a.join("root/file")).unwrap(), "in root");
    assert_eq!(std::fs::read_to_string(at_b.join("root/file")).unwrap(), "in b");
    for (from, to) in [
        (at_a.join("root/file"), at_a.join("data/moved")),
        (at_a.join("root/file"), at_b.join("root/moved")),
    ] {
        assert_eq!(std::fs::rename(&from, &to).unwrap_err().raw_os_error(), Some(libc::EXDEV));
    }
    for made in [m.join("c"), m.join(one).join("c"), at_a.join("c")] {
        let refused = std::fs::create_dir(&made).unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(libc::EPERM), "{}", made.display());
    }
    assert_ne!(
        std::fs::metadata(at_a.join("root/file")).unwrap().ino(),
        std::fs::metadata(at_a.join("data/file")).unwrap().ino()
    );
    let listed = format!("{one} {a}\n  data {data}\n  root {root}\n{two} {b}\n  root {other}\n");
    assert_eq!(fxdax_mount_command(&["list", socket]), listed);
    // The cells' objects are under their tenants' prefixes: their cells' objects, and their
    // volumes' branches.
    let bucket = dir.path().join("bucket");
    let cell_a = bucket.join(format!("tenants/{one}/cells/{a}"));
    assert!(cell_a.join("cell").exists(), "no cell object at {}", cell_a.display());
    let mut branches = vec![data.clone(), root.clone()];
    branches.sort();
    assert_eq!(names(&cell_a.join("branches")), branches);
    assert!(bucket.join(format!("tenants/{two}/cells/{b}/branches/{other}")).exists());

    // Detached, and attached again from its stores; the other cell detached, its stores gone,
    // then attached again from the bucket.
    fxdax_mount_command(&["detach", socket, "1", &a]);
    assert_eq!(names(&m), [two]);
    let again: Vec<String> = attach("1", &a);
    assert_eq!(again, [a.clone(), format!("data {data}"), format!("root {root}")]);
    assert_eq!(std::fs::read_to_string(at_a.join("data/file")).unwrap(), "in data");
    assert_eq!(std::fs::read_to_string(at_a.join("root/file")).unwrap(), "in root");
    fxdax_mount_command(&["detach", socket, "2", &b]);
    std::fs::remove_dir_all(dir.path().join("stores").join(two)).unwrap();
    let again: Vec<String> = attach("2", &b);
    assert_eq!(again, [b.clone(), format!("root {other}")]);
    assert_eq!(std::fs::read_to_string(at_b.join("root/file")).unwrap(), "in b");

    // A file open holds its volume, and its cell's detach, until it is closed.
    let open = std::fs::File::open(at_a.join("data/file")).unwrap();
    let mut detach = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_fxdax-mount"))
            .args(["detach", socket, "1", &a])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let detaching = format!("{one} {a}\n  data {data} detaching\n{two} {b}\n  root {other}\n");
    while fxdax_mount_command(&["list", socket]) != detaching {}
    assert_eq!(names(&m), [two]);
    assert!(detach.0.try_wait().unwrap().is_none(), "detached with a file open");
    drop(open);
    assert!(detach.0.wait().unwrap().success());
    assert_eq!(fxdax_mount_command(&["list", socket]), format!("{two} {b}\n  root {other}\n"));

    terminate(child, &m);
    // Each store is whole, and holds what was written.
    block_on(async {
        let stores = dir.path().join("stores");
        for (store, file) in [
            (stores.join(one).join(&a).join("root"), "in root"),
            (stores.join(one).join(&a).join("data"), "in data"),
            (stores.join(two).join(&b).join("root"), "in b"),
        ] {
            let storage = Storage::open(&store).unwrap().check().await.unwrap();
            let fs = Filesystem::mount(storage).await.unwrap();
            assert_eq!(contents(fs.root(), b"file").await, file.as_bytes());
            fs.close().await.unwrap();
        }
    });
}
