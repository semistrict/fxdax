// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of a server of many stores (TASK-302): stores attached by name in its root, their nodes'
//! IDs apart, nothing made or moved in the root or between stores, and stores detached once the
//! kernel forgets their nodes, or at once by force; driven by requests as the kernel sends them.

use super::*;
use crate::Storage;
use futures::executor::block_on;
use std::sync::mpsc;

const STORAGE_SIZE: u64 = 16 << 20;
const OWNER: Owner = Owner { uid: 1000, gid: 100 };

/// A server of many stores, none attached, and the notifications it sends.
fn stores() -> (Server, mpsc::Receiver<Vec<u8>>) {
    let (sent, notified) = mpsc::channel();
    let sent = Mutex::new(sent);
    let notify = move |message: &[u8]| {
        sent.lock().unwrap().send(message.to_vec()).unwrap();
        Ok(())
    };
    (Server::stores(OWNER, Box::new(notify)), notified)
}

/// A new filesystem, empty, in memory, and its root's object ID.
async fn filesystem() -> Result<Filesystem, Error> {
    Filesystem::format(Storage::memory(STORAGE_SIZE)).await
}

/// The object ID of a new filesystem's root, as every one has.
fn root_object() -> u64 {
    let fs = block_on(filesystem()).unwrap();
    let root = fs.root().inode().0;
    block_on(fs.close()).unwrap();
    root
}

/// A request of `opcode` of the node `nodeid`, whose body is `body`.
fn request(opcode: u32, nodeid: u64, body: &[u8]) -> Vec<u8> {
    let len = (size_of::<InHeader>() + body.len()) as u32;
    let header = InHeader {
        len,
        opcode,
        unique: 7,
        nodeid,
        uid: OWNER.uid,
        gid: OWNER.gid,
        pid: 1,
        padding: 0,
    };
    [header.as_bytes(), body].concat()
}

/// The reply `server` gives the request, failing with its errno if it failed.
fn send(server: &Server, request: &[u8]) -> std::result::Result<Vec<u8>, Errno> {
    let reply = block_on(server.handle(request)).unwrap().expect("a reply");
    let (header, payload) = OutHeader::read_from_prefix(&reply).unwrap();
    assert_eq!(header.len as usize, reply.len());
    match header.error {
        0 => Ok(payload.to_vec()),
        error => Err(Errno::from_raw(-error)),
    }
}

/// A name, NUL-terminated, as requests carry names.
fn named(name: &str) -> Vec<u8> {
    [name.as_bytes(), b"\0"].concat()
}

fn lookup(server: &Server, dir: u64, name: &str) -> std::result::Result<EntryOut, Errno> {
    let reply = send(server, &request(opcode::LOOKUP, dir, &named(name)))?;
    Ok(EntryOut::read_from_bytes(&reply).unwrap())
}

fn mkdir(server: &Server, dir: u64, name: &str) -> std::result::Result<EntryOut, Errno> {
    let body = [MkdirIn { mode: 0o755, umask: 0 }.as_bytes(), &named(name)].concat();
    let reply = send(server, &request(opcode::MKDIR, dir, &body))?;
    Ok(EntryOut::read_from_bytes(&reply).unwrap())
}

fn rename(server: &Server, (dir, name): (u64, &str), (to, new): (u64, &str)) -> Result<()> {
    let body = [RenameIn { newdir: to }.as_bytes(), &named(name), &named(new)].concat();
    send(server, &request(opcode::RENAME, dir, &body)).map(drop)
}

fn getattr(server: &Server, nodeid: u64) -> std::result::Result<AttrOut, Errno> {
    let reply = send(server, &request(opcode::GETATTR, nodeid, &[]))?;
    Ok(AttrOut::read_from_bytes(&reply).unwrap())
}

fn forget(server: &Server, nodeid: u64, nlookup: u64) {
    let forgot =
        block_on(server.handle(&request(opcode::FORGET, nodeid, ForgetIn { nlookup }.as_bytes())));
    assert_eq!(forgot.unwrap(), None);
}

/// The names the directory `dir` lists, `.` and `..` first, and their inode numbers.
fn readdir(server: &Server, dir: u64) -> Vec<(String, u64)> {
    let opened = send(server, &request(opcode::OPENDIR, dir, &[])).unwrap();
    let OpenOut { fh, .. } = OpenOut::read_from_bytes(&opened).unwrap();
    let read = ReadIn { fh, offset: 0, size: 4096, ..Default::default() };
    let reply = send(server, &request(opcode::READDIR, dir, read.as_bytes())).unwrap();
    let release = ReleaseIn { fh, ..Default::default() };
    send(server, &request(opcode::RELEASEDIR, dir, release.as_bytes())).unwrap();
    let mut listed = Vec::new();
    let mut rest = &reply[..];
    while !rest.is_empty() {
        let (dirent, after) = Dirent::read_from_prefix(rest).unwrap();
        let name = String::from_utf8(after[..dirent.namelen as usize].to_vec()).unwrap();
        listed.push((name, dirent.ino));
        rest = &rest[(size_of::<Dirent>() + dirent.namelen as usize).next_multiple_of(8)..];
    }
    listed
}

/// The node ID of the object `object` of the store in `slot`.
fn id(slot: u64, object: u64) -> u64 {
    slot << SLOT_SHIFT | object
}

/// Stores attached are their names in the root, which lists them; their nodes' IDs, and inode
/// numbers, are their slots' and their object IDs, apart though the stores number objects alike;
/// a name attached at nowhere is absent for no time; and the root is the server's, `OWNER`'s,
/// 0755, nothing made in it nor moved in or out of it, or between stores.
#[test]
fn stores_attached_are_in_the_root_and_their_nodes_are_apart() {
    let (server, notified) = stores();
    let root = root_object();
    block_on(server.attach("a", filesystem())).unwrap();
    block_on(server.attach("b", filesystem())).unwrap();
    assert_eq!(
        server.attached(),
        [
            Attached { name: "a".into(), detaching: false },
            Attached { name: "b".into(), detaching: false }
        ]
    );

    let a = lookup(&server, ROOT_ID, "a").unwrap();
    let b = lookup(&server, ROOT_ID, "b").unwrap();
    assert_eq!(
        (a.nodeid, a.attr.ino, b.nodeid, b.attr.ino),
        (id(1, root), id(1, root), id(2, root), id(2, root))
    );
    let (x_in_a, x_in_b) =
        (mkdir(&server, a.nodeid, "x").unwrap(), mkdir(&server, b.nodeid, "x").unwrap());
    let object = x_in_a.nodeid & ((1 << SLOT_SHIFT) - 1);
    assert_eq!((x_in_a.nodeid, x_in_b.nodeid), (id(1, object), id(2, object)));
    assert_eq!(x_in_b.attr.ino, x_in_b.nodeid);

    let absent = lookup(&server, ROOT_ID, "c").unwrap();
    assert_eq!((absent.nodeid, absent.entry_valid), (0, 0));
    let in_a = lookup(&server, a.nodeid, "absent").unwrap();
    assert_eq!((in_a.nodeid, in_a.entry_valid), (0, TIMEOUT_SECS));

    assert_eq!(
        readdir(&server, ROOT_ID),
        [
            (".".into(), ROOT_ID),
            ("..".into(), ROOT_ID),
            ("a".into(), id(1, root)),
            ("b".into(), id(2, root))
        ]
    );
    assert_eq!(
        readdir(&server, a.nodeid),
        [(".".into(), id(1, root)), ("..".into(), ROOT_ID), ("x".into(), x_in_a.nodeid)]
    );
    let attr = getattr(&server, ROOT_ID).unwrap().attr;
    assert_eq!(
        (attr.ino, attr.mode, attr.uid, attr.gid),
        (ROOT_ID, FileType::Directory.mode_bits() | 0o755, OWNER.uid, OWNER.gid)
    );

    assert_eq!(mkdir(&server, ROOT_ID, "c").unwrap_err(), Errno::EPERM);
    assert_eq!(rename(&server, (a.nodeid, "x"), (b.nodeid, "y")), Err(Errno::EXDEV));
    assert_eq!(rename(&server, (a.nodeid, "x"), (ROOT_ID, "y")), Err(Errno::EXDEV));
    assert_eq!(rename(&server, (ROOT_ID, "a"), (ROOT_ID, "c")), Err(Errno::EPERM));
    let unlinked = send(&server, &request(opcode::RMDIR, ROOT_ID, &named("a")));
    assert_eq!(unlinked, Err(Errno::EPERM));

    // Each store's usage is its own; the root holds nothing.
    let statfs = |nodeid| {
        let reply = send(&server, &request(opcode::STATFS, nodeid, &[])).unwrap();
        Kstatfs::read_from_bytes(&reply).unwrap()
    };
    assert_eq!(statfs(a.nodeid).blocks * statfs(a.nodeid).bsize as u64, STORAGE_SIZE);
    assert_eq!((statfs(ROOT_ID).blocks, statfs(ROOT_ID).namelen), (0, 255));

    block_on(server.unmount()).unwrap();
    assert!(notified.try_recv().is_err(), "nothing was detached");
}

/// A store detached goes from the root at once, the kernel told to drop every name it was given a
/// node of the store under, as they are after renames and removals, so that it forgets them, those
/// in use once let go of; names it is given meanwhile are given for no time. The store is unmounted
/// once the kernel forgets the last of its nodes, and the detach returns then; its slot, and its
/// name, are free again after. The other store is untouched.
#[test]
fn a_store_detached_is_unmounted_once_the_kernel_forgets_its_nodes() {
    let (server, notified) = stores();
    block_on(server.attach("a", filesystem())).unwrap();
    block_on(server.attach("b", filesystem())).unwrap();
    let a = lookup(&server, ROOT_ID, "a").unwrap();
    let x = mkdir(&server, a.nodeid, "x").unwrap();
    let y = mkdir(&server, x.nodeid, "y").unwrap();
    rename(&server, (x.nodeid, "y"), (a.nodeid, "moved")).unwrap();
    let gone = mkdir(&server, a.nodeid, "gone").unwrap();
    send(&server, &request(opcode::RMDIR, a.nodeid, &named("gone"))).unwrap();
    let b = lookup(&server, ROOT_ID, "b").unwrap();
    let in_b = mkdir(&server, b.nodeid, "x").unwrap();

    std::thread::scope(|scope| {
        let detached = scope.spawn(|| block_on(server.detach("a", false)));
        // The kernel is told before the detach waits for it: each name once, as it is now.
        let mut told: Vec<Vec<u8>> = (0..3).map(|_| notified.recv().unwrap()).collect();
        told.sort();
        let mut names = vec![
            inval_entry(ROOT_ID, b"a"),
            inval_entry(a.nodeid, b"x"),
            inval_entry(a.nodeid, b"moved"),
        ];
        names.sort();
        assert_eq!(told, names);
        assert_eq!(
            server.attached(),
            [
                Attached { name: "a".into(), detaching: true },
                Attached { name: "b".into(), detaching: false }
            ]
        );
        assert_eq!(lookup(&server, ROOT_ID, "a").unwrap().nodeid, 0);
        assert_eq!(
            block_on(server.attach("a", filesystem())).unwrap_err().errno(),
            Some(Errno::EBUSY)
        );
        // The nodes the kernel holds are served until it forgets them, and what it finds through
        // them is given for no time; the other store's, as long as it likes.
        assert_eq!(getattr(&server, x.nodeid).unwrap().attr.ino, x.nodeid);
        let found = lookup(&server, a.nodeid, "moved").unwrap();
        assert_eq!((found.nodeid, found.entry_valid), (y.nodeid, 0));
        assert_eq!(lookup(&server, b.nodeid, "x").unwrap().entry_valid, TIMEOUT_SECS);
        for (nodeid, lookups) in [(gone.nodeid, 1), (y.nodeid, 2), (x.nodeid, 1)] {
            forget(&server, nodeid, lookups);
        }
        assert!(!detached.is_finished());
        forget(&server, a.nodeid, 1);
        detached.join().unwrap().unwrap();
    });
    assert!(notified.try_recv().is_err(), "told more");

    assert_eq!(server.attached(), [Attached { name: "b".into(), detaching: false }]);
    assert_eq!(getattr(&server, in_b.nodeid).unwrap().attr.ino, in_b.nodeid);
    // Slot 1 and the name are free again, its names given for as long as the kernel likes.
    block_on(server.attach("a", filesystem())).unwrap();
    let again = lookup(&server, ROOT_ID, "a").unwrap();
    assert_eq!(again.nodeid >> SLOT_SHIFT, 1);
    assert_eq!(mkdir(&server, again.nodeid, "x").unwrap().entry_valid, TIMEOUT_SECS);
    block_on(server.unmount()).unwrap();
}

/// A store detached by force is unmounted at once: the nodes the kernel holds of it fail with
/// `ENOTCONN`, and its slot is used again only once the kernel has forgotten them.
#[test]
fn a_store_detached_by_force_is_unmounted_at_once() {
    let (server, notified) = stores();
    block_on(server.attach("a", filesystem())).unwrap();
    let a = lookup(&server, ROOT_ID, "a").unwrap();
    let x = mkdir(&server, a.nodeid, "x").unwrap();

    block_on(server.detach("a", true)).unwrap();
    let mut told: Vec<Vec<u8>> = notified.try_iter().collect();
    told.sort();
    let mut names = vec![inval_entry(ROOT_ID, b"a"), inval_entry(a.nodeid, b"x")];
    names.sort();
    assert_eq!(told, names);
    assert_eq!(server.attached(), []);
    assert_eq!(getattr(&server, x.nodeid).unwrap_err(), Errno::ENOTCONN);
    let statfs = send(&server, &request(opcode::STATFS, x.nodeid, &[]));
    assert_eq!(statfs, Err(Errno::ENOTCONN));

    // Slot 1 is still the kernel's.
    block_on(server.attach("b", filesystem())).unwrap();
    assert_eq!(lookup(&server, ROOT_ID, "b").unwrap().nodeid >> SLOT_SHIFT, 2);
    forget(&server, x.nodeid, 1);
    forget(&server, a.nodeid, 1);
    block_on(server.attach("c", filesystem())).unwrap();
    assert_eq!(lookup(&server, ROOT_ID, "c").unwrap().nodeid >> SLOT_SHIFT, 1);
    block_on(server.unmount()).unwrap();
}

/// What attaching and detaching refuse: a name taken, a name no directory holds, a name not
/// attached; a store that fails to open leaves its name free; a server of one filesystem attaches
/// none; and once the kernel destroys the mount, none is attached.
#[test]
fn attaching_and_detaching_refuse_what_they_cant_do() {
    let (server, _notified) = stores();
    let errno = |attached: Result<(), Error>| attached.unwrap_err().errno();
    block_on(server.attach("a", filesystem())).unwrap();
    assert_eq!(errno(block_on(server.attach("a", filesystem()))), Some(Errno::EEXIST));
    for name in ["", ".", "..", "a/b"] {
        assert_eq!(
            errno(block_on(server.attach(name, filesystem()))),
            Some(Errno::EINVAL),
            "{name:?}"
        );
    }
    let failing = async { Err(Error::from(Errno::ESTALE)) };
    assert_eq!(errno(block_on(server.attach("b", failing))), Some(Errno::ESTALE));
    assert_eq!(server.attached(), [Attached { name: "a".into(), detaching: false }]);
    block_on(server.attach("b", filesystem())).unwrap();
    assert_eq!(errno(block_on(server.detach("c", false))), Some(Errno::ENOENT));

    let destroy = block_on(server.handle(&request(opcode::DESTROY, 0, &[])));
    assert_eq!(destroy.unwrap(), None);
    assert_eq!(errno(block_on(server.attach("c", filesystem()))), Some(Errno::ENOTCONN));
    // The stores are unmounted.
    assert_eq!(lookup(&server, ROOT_ID, "a").unwrap_err(), Errno::ENOTCONN);

    let one = Server::new(block_on(filesystem()).unwrap());
    assert_eq!(errno(block_on(one.attach("a", filesystem()))), Some(Errno::ENOTSUP));
    assert_eq!(errno(block_on(one.detach("a", false))), Some(Errno::ENOTSUP));
    block_on(one.into_filesystem().close()).unwrap();
}

/// Attaching and detaching are futures a runtime may move between threads, as the control API's
/// tasks are, on Linux alone (`fuse::control`): so checked on every host.
#[test]
fn attaching_and_detaching_are_send() {
    fn send<T: Send>(_: T) {}
    let (server, _notified) = stores();
    send(server.attach("a", filesystem()));
    send(server.detach("a", false));
}
