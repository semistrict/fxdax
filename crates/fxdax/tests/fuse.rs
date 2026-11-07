// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests of the FUSE server (`fxdax::fuse::Server`), driven as the kernel drives it: with requests
//! encoded as Linux's FUSE client encodes them, and their replies decoded.
//!
//! The client here encodes and decodes messages by hand, from Linux's `include/uapi/linux/fuse.h`,
//! independently of the server's definitions.

use futures::executor::block_on;
use fxdax::fuse::Server;
use fxdax::{Filesystem, Storage, pages};

const STORAGE_SIZE: u64 = 16 << 20;
const ROOT: u64 = 1;

// Opcodes.
const LOOKUP: u32 = 1;
const FORGET: u32 = 2;
const GETATTR: u32 = 3;
const SETATTR: u32 = 4;
const READLINK: u32 = 5;
const SYMLINK: u32 = 6;
const MKNOD: u32 = 8;
const MKDIR: u32 = 9;
const UNLINK: u32 = 10;
const RMDIR: u32 = 11;
const RENAME: u32 = 12;
const LINK: u32 = 13;
const OPEN: u32 = 14;
const READ: u32 = 15;
const WRITE: u32 = 16;
const STATFS: u32 = 17;
const RELEASE: u32 = 18;
const FSYNC: u32 = 20;
const SETXATTR: u32 = 21;
const GETXATTR: u32 = 22;
const LISTXATTR: u32 = 23;
const REMOVEXATTR: u32 = 24;
const FLUSH: u32 = 25;
const INIT: u32 = 26;
const OPENDIR: u32 = 27;
const READDIR: u32 = 28;
const RELEASEDIR: u32 = 29;
const GETLK: u32 = 31;
const CREATE: u32 = 35;
const INTERRUPT: u32 = 36;
const DESTROY: u32 = 38;
const BATCH_FORGET: u32 = 42;
const FALLOCATE: u32 = 43;
const READDIRPLUS: u32 = 44;
const RENAME2: u32 = 45;
const SETUPMAPPING: u32 = 48;

// Linux errno values.
const EBADF: i32 = 9;
const EEXIST: i32 = 17;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const ERANGE: i32 = 34;
const ENAMETOOLONG: i32 = 36;
const ENOSYS: i32 = 38;
const ENOTEMPTY: i32 = 39;
const ENODATA: i32 = 61;
const EPROTO: i32 = 71;
const ENOTSUP: i32 = 95;
const ENOTCONN: i32 = 107;
const ESTALE: i32 = 116;

const ENTRY_OUT_LEN: usize = 128;
const DIRENT_LEN: usize = 24;
const DAY: u64 = 24 * 60 * 60;

/// A request's body, built field by field, little-endian as the kernel's are.
#[derive(Default)]
struct Body(Vec<u8>);

impl Body {
    fn u32(mut self, value: u32) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn u64(mut self, value: u64) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn name(mut self, name: &[u8]) -> Self {
        self.0.extend_from_slice(name);
        self.0.push(0);
        self
    }

    fn bytes(mut self, bytes: &[u8]) -> Self {
        self.0.extend_from_slice(bytes);
        self
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// A reply: its error, and its payload.
#[derive(Debug, PartialEq, Eq)]
struct Reply {
    error: i32,
    payload: Vec<u8>,
}

impl Reply {
    fn ok(self) -> Vec<u8> {
        assert_eq!(self.error, 0, "the request failed");
        self.payload
    }
}

/// The parts of a `fuse_attr` the tests check.
#[derive(Debug, PartialEq, Eq)]
struct Attr {
    ino: u64,
    size: u64,
    mode: u32,
    nlink: u32,
    uid: u32,
    gid: u32,
    rdev: u32,
}

impl Attr {
    fn decode(attr: &[u8]) -> Self {
        Self {
            ino: u64_at(attr, 0),
            size: u64_at(attr, 8),
            mode: u32_at(attr, 60),
            nlink: u32_at(attr, 64),
            uid: u32_at(attr, 68),
            gid: u32_at(attr, 72),
            rdev: u32_at(attr, 76),
        }
    }
}

/// A `fuse_entry_out`'s node ID and attributes.
fn entry(payload: &[u8]) -> (u64, Attr) {
    // The name and attributes stay valid as long as the kernel likes.
    assert_eq!((u64_at(payload, 16), u64_at(payload, 24)), (DAY, DAY));
    (u64_at(payload, 0), Attr::decode(&payload[40..ENTRY_OUT_LEN]))
}

/// A `fuse_attr_out`'s attributes.
fn attr_out(payload: &[u8]) -> Attr {
    assert_eq!(u64_at(payload, 0), DAY);
    Attr::decode(&payload[16..])
}

/// A directory entry of a `READDIR` reply: its inode, next offset, type and name.
#[derive(Debug, PartialEq, Eq)]
struct Dirent {
    ino: u64,
    off: u64,
    type_: u32,
    name: Vec<u8>,
}

/// The entries of a `READDIR` reply, or of a `READDIRPLUS` reply with the node IDs of their
/// entries.
fn dirents(mut payload: &[u8], plus: bool) -> Vec<(Dirent, Option<u64>)> {
    let mut entries = Vec::new();
    while !payload.is_empty() {
        let nodeid = plus.then(|| u64_at(payload, 0));
        if plus {
            payload = &payload[ENTRY_OUT_LEN..];
        }
        let namelen = u32_at(payload, 16) as usize;
        let dirent = Dirent {
            ino: u64_at(payload, 0),
            off: u64_at(payload, 8),
            type_: u32_at(payload, 20),
            name: payload[DIRENT_LEN..DIRENT_LEN + namelen].to_vec(),
        };
        let len = (DIRENT_LEN + namelen).next_multiple_of(8);
        assert!(payload[DIRENT_LEN + namelen..len].iter().all(|&byte| byte == 0));
        payload = &payload[len..];
        entries.push((dirent, nodeid));
    }
    entries
}

/// The kernel's FUSE client of a server.
struct Client {
    server: Server,
    unique: u64,
    uid: u32,
    gid: u32,
}

impl Client {
    async fn new() -> Self {
        let fs = Filesystem::format(Storage::memory(STORAGE_SIZE)).await.unwrap();
        Self { server: Server::new(fs), unique: 0, uid: 1000, gid: 100 }
    }

    /// A client of a new server, initialized.
    async fn init_new() -> Self {
        let mut client = Self::new().await;
        client.init().await;
        client
    }

    /// The bytes of a request.
    fn request(&mut self, opcode: u32, nodeid: u64, body: &Body) -> Vec<u8> {
        self.unique += 1;
        Body::default()
            .u32((40 + body.0.len()) as u32)
            .u32(opcode)
            .u64(self.unique)
            .u64(nodeid)
            .u32(self.uid)
            .u32(self.gid)
            .u32(1234)
            .u32(0)
            .bytes(&body.0)
            .0
    }

    /// Sends a request, and returns its reply's bytes, if it has one.
    async fn send(&mut self, opcode: u32, nodeid: u64, body: &Body) -> Option<Vec<u8>> {
        let request = self.request(opcode, nodeid, body);
        self.server.handle(&request).await.expect("handle failed")
    }

    /// Calls `opcode` on `nodeid` with `body`, and returns its reply.
    async fn call(&mut self, opcode: u32, nodeid: u64, body: Body) -> Reply {
        let reply = self.send(opcode, nodeid, &body).await.expect("no reply");
        decode(&reply, self.unique)
    }

    async fn init(&mut self) -> Vec<u8> {
        // 7.39, with flags a Linux kernel sends, extended: ASYNC_READ, POSIX_LOCKS,
        // ATOMIC_O_TRUNC, BIG_WRITES, AUTO_INVAL_DATA, DO_READDIRPLUS, READDIRPLUS_AUTO,
        // ASYNC_DIO, WRITEBACK_CACHE, PARALLEL_DIROPS, POSIX_ACL, MAX_PAGES, CACHE_SYMLINKS,
        // HANDLE_KILLPRIV_V2, SETXATTR_EXT, INIT_EXT and HAS_INODE_DAX.
        let flags: u64 = [0, 1, 3, 5, 12, 13, 14, 15, 16, 18, 20, 22, 23, 28, 29, 30, 33]
            .iter()
            .map(|bit| 1 << bit)
            .sum();
        let body = Body::default()
            .u32(7)
            .u32(39)
            .u32(1 << 17)
            .u32(flags as u32)
            .u32((flags >> 32) as u32)
            .bytes(&[0; 44]);
        self.call(INIT, 0, body).await.ok()
    }

    async fn lookup(&mut self, parent: u64, name: &[u8]) -> Reply {
        self.call(LOOKUP, parent, Body::default().name(name)).await
    }

    async fn create(&mut self, parent: u64, name: &[u8], mode: u32) -> (u64, Attr) {
        let body = Body::default().u32(0o100002).u32(mode).u32(0o022).u32(0).name(name);
        let payload = self.call(CREATE, parent, body).await.ok();
        // An open file, without a handle, whose cache stays valid, and that needs no flushing.
        assert_eq!(payload.len(), ENTRY_OUT_LEN + 16);
        assert_eq!(u64_at(&payload, ENTRY_OUT_LEN), 0);
        assert_eq!(u32_at(&payload, ENTRY_OUT_LEN + 8), (1 << 1) | (1 << 5));
        entry(&payload)
    }

    async fn mkdir(&mut self, parent: u64, name: &[u8], mode: u32) -> (u64, Attr) {
        let body = Body::default().u32(mode).u32(0o022).name(name);
        entry(&self.call(MKDIR, parent, body).await.ok())
    }

    async fn write(&mut self, nodeid: u64, offset: u64, data: &[u8], write_flags: u32) -> Reply {
        let body = Body::default()
            .u64(0)
            .u64(offset)
            .u32(data.len() as u32)
            .u32(write_flags)
            .u64(0)
            .u32(0)
            .u32(0)
            .bytes(data);
        self.call(WRITE, nodeid, body).await
    }

    async fn read(&mut self, nodeid: u64, offset: u64, size: u32) -> Vec<u8> {
        self.call(READ, nodeid, read_in(0, offset, size)).await.ok()
    }

    async fn getattr(&mut self, nodeid: u64) -> Reply {
        self.call(GETATTR, nodeid, Body::default().u32(0).u32(0).u64(0)).await
    }

    async fn setattr(&mut self, nodeid: u64, valid: u32, fields: SetattrFields) -> Attr {
        attr_out(&self.setattr_reply(nodeid, valid, fields).await.ok())
    }

    async fn setattr_reply(&mut self, nodeid: u64, valid: u32, fields: SetattrFields) -> Reply {
        let body = Body::default()
            .u32(valid)
            .u32(0)
            .u64(0)
            .u64(fields.size)
            .u64(0)
            .u64(fields.atime.0)
            .u64(fields.mtime.0)
            .u64(0)
            .u32(fields.atime.1)
            .u32(fields.mtime.1)
            .u32(0)
            .u32(fields.mode)
            .u32(0)
            .u32(fields.uid)
            .u32(fields.gid)
            .u32(0);
        self.call(SETATTR, nodeid, body).await
    }

    async fn opendir(&mut self, nodeid: u64) -> u64 {
        let payload = self.call(OPENDIR, nodeid, Body::default().u32(0).u32(0)).await.ok();
        // Its entries, and data, may be cached.
        assert_eq!(u32_at(&payload, 8), (1 << 1) | (1 << 3));
        u64_at(&payload, 0)
    }

    async fn readdir(&mut self, nodeid: u64, handle: u64, offset: u64, size: u32) -> Vec<Dirent> {
        let payload = self.call(READDIR, nodeid, read_in(handle, offset, size)).await.ok();
        dirents(&payload, false).into_iter().map(|(dirent, _)| dirent).collect()
    }

    async fn forget(&mut self, nodeid: u64, nlookup: u64) {
        assert_eq!(self.send(FORGET, nodeid, &Body::default().u64(nlookup)).await, None);
    }
}

/// The reply to the request `unique`, decoded from `reply`.
fn decode(reply: &[u8], unique: u64) -> Reply {
    assert!(reply.len() >= 16, "a {}-byte reply", reply.len());
    assert_eq!(u32_at(reply, 0) as usize, reply.len(), "the header's length is not the reply's");
    assert_eq!(u64_at(reply, 8), unique, "the reply is not to the request");
    let error = u32_at(reply, 4) as i32;
    if error != 0 {
        assert_eq!(reply.len(), 16, "an error with a payload");
    }
    Reply { error: -error, payload: reply[16..].to_vec() }
}

#[derive(Default)]
struct SetattrFields {
    size: u64,
    atime: (u64, u32),
    mtime: (u64, u32),
    mode: u32,
    uid: u32,
    gid: u32,
}

fn read_in(fh: u64, offset: u64, size: u32) -> Body {
    Body::default().u64(fh).u64(offset).u32(size).u32(0).u64(0).u32(0).u32(0)
}

fn release_in(fh: u64) -> Body {
    Body::default().u64(fh).u32(0).u32(0).u64(0)
}

/// INIT negotiates the protocol version and the features both sides support.
#[test]
fn init_negotiates() {
    block_on(async {
        let mut client = Client::new().await;
        let out = client.init().await;
        assert_eq!(out.len(), 64);
        assert_eq!((u32_at(&out, 0), u32_at(&out, 4)), (7, 36));
        assert_eq!(u32_at(&out, 8), 1 << 17, "max_readahead");
        // Of what was offered: async reads and direct I/O, big writes, readdirplus (auto),
        // parallel directory operations, max pages, cached symlinks and the extension; not
        // locks, ACLs or extended setxattr, which the server doesn't serve, atomic O_TRUNC (opens
        // are refused), automatic invalidation of cached data (the mount is the only writer), the
        // writeback cache (dead-ends/fuse-writeback-cache.md), KILLPRIV_V2 (the kernel kills
        // setuid and setgid itself, knowing who may keep them), or DAX.
        let flags = u32_at(&out, 12) as u64 | (u32_at(&out, 52) as u64) << 32;
        let expected: u64 = [0, 5, 13, 14, 15, 18, 22, 23, 30].iter().map(|bit| 1 << bit).sum();
        assert_eq!(flags, expected, "{flags:#x}");
        assert_eq!(u32_at(&out, 20), 1 << 20, "max_write");
        let max_pages = u16::from_le_bytes(out[28..30].try_into().unwrap());
        assert_eq!(max_pages as u64, (1 << 20) / pages::page_size());

        // A newer major version is answered with the server's, for the kernel to ask again.
        let body = Body::default().u32(8).u32(0).u32(0).u32(0);
        let out = client.call(INIT, 0, body).await.ok();
        assert_eq!((u32_at(&out, 0), u32_at(&out, 4), u32_at(&out, 12)), (7, 36, 0));
        // Older versions are refused.
        for (major, minor) in [(6, 40), (7, 26)] {
            let body = Body::default().u32(major).u32(minor).u32(0).u32(0);
            assert_eq!(client.call(INIT, 0, body).await.error, EPROTO);
        }
    })
}

#[test]
fn entries() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (dir, attr) = client.mkdir(ROOT, b"dir", 0o777).await;
        assert_eq!((attr.mode, attr.uid, attr.gid, attr.nlink), (0o040755, 1000, 100, 2));
        let (file, attr) = client.create(dir, b"file", 0o666).await;
        assert_eq!((attr.mode, attr.size), (0o100644, 0));
        assert_eq!(attr.ino, file);
        assert_eq!(entry(&client.lookup(dir, b"file").await.ok()), (file, attr));
        // A missing name is an entry without a node, cached as long as others.
        let missing = client.lookup(dir, b"missing").await.ok();
        assert_eq!(missing.len(), ENTRY_OUT_LEN);
        assert_eq!((u64_at(&missing, 0), u64_at(&missing, 16)), (0, DAY));
        let body = Body::default().u32(0).u32(0o644).u32(0).u32(0).name(b"file");
        assert_eq!(client.call(CREATE, dir, body).await.error, EEXIST);
        assert_eq!(attr_out(&client.getattr(dir).await.ok()).ino, dir);

        assert_eq!(client.call(RMDIR, ROOT, Body::default().name(b"dir")).await.error, ENOTEMPTY);
        assert_eq!(client.call(UNLINK, dir, Body::default().name(b"file")).await.ok(), b"");
        assert_eq!(u64_at(&client.lookup(dir, b"file").await.ok(), 0), 0);
        // FORGET has no reply; the file is gone once the kernel forgets its two lookups, and its
        // node ID is stale.
        client.forget(file, 1).await;
        assert_eq!(attr_out(&client.getattr(file).await.ok()).nlink, 0);
        client.forget(file, 1).await;
        assert_eq!(client.getattr(file).await.error, ESTALE);
        assert_eq!(client.call(RMDIR, ROOT, Body::default().name(b"dir")).await.ok(), b"");
    })
}

/// A file unlinked while the kernel holds it, as a process holds it open, is read and written
/// until the kernel forgets it.
#[test]
fn unlinked_files_stay_while_held() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        client.write(file, 0, b"kept", 0).await.ok();
        assert_eq!(client.call(UNLINK, ROOT, Body::default().name(b"file")).await.ok(), b"");
        client.write(file, 4, b" on", 0).await.ok();
        assert_eq!(client.read(file, 0, 100).await, b"kept on");
        let attr = attr_out(&client.getattr(file).await.ok());
        assert_eq!((attr.nlink, attr.size), (0, 7));
        client.forget(file, 1).await;
        assert_eq!(client.getattr(file).await.error, ESTALE);
    })
}

#[test]
fn file_io() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let contents: Vec<u8> =
            (0..3 * pages::page_size() as usize).map(|i| (i % 253) as u8).collect();
        let written = client.write(file, 5, &contents, 0).await.ok();
        assert_eq!(u32_at(&written, 0) as usize, contents.len());
        assert_eq!(attr_out(&client.getattr(file).await.ok()).size, 5 + contents.len() as u64);

        assert_eq!(client.read(file, 5, contents.len() as u32).await, contents);
        // Reads stop at the end of the file.
        assert_eq!(
            client.read(file, 5 + contents.len() as u64 - 3, 100).await,
            &contents[contents.len() - 3..]
        );
        assert_eq!(client.read(file, 1 << 30, 100).await, b"");
        // No more than max_write is read or written at once.
        assert_eq!(client.call(READ, file, read_in(0, 0, (1 << 20) + 1)).await.error, EINVAL);
        // A write's data must all be there.
        let short =
            Body::default().u64(0).u64(0).u32(100).u32(0).u64(0).u32(0).u32(0).bytes(b"short");
        assert_eq!(client.call(WRITE, file, short).await.error, EINVAL);
        assert_eq!(client.call(READ, ROOT, read_in(0, 0, 10)).await.error, EISDIR);
    })
}

/// Opens are refused, so the kernel truncates an open with O_TRUNC itself: a SETATTR of the
/// size. Setuid and setgid are the kernel's to kill (KILLPRIV_V2 is not offered), with a SETATTR
/// of the mode it computes, alone or with what kills them: a truncation, a write, a chown keep
/// them, even asked to kill them as KILLPRIV_V2's flags would, and a mode given with a chown is
/// the file's; a directory's setgid survives a chown, as Linux's.
#[test]
fn truncating_and_killing_privileges() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        client.write(file, 0, b"contents", 0).await.ok();
        let setuid = SetattrFields { mode: 0o6755, ..Default::default() };
        assert_eq!(client.setattr(file, 1, setuid).await.mode, 0o106755);

        let open = Body::default().u32(0o1000 | 2).u32(1);
        assert_eq!(client.call(OPEN, file, open).await.error, ENOSYS);
        // FATTR_SIZE, with FATTR_KILL_SUIDGID.
        let attr = client.setattr(file, 8 | 2048, SetattrFields::default()).await;
        assert_eq!((attr.size, attr.mode), (0, 0o106755));
        // FUSE_WRITE_KILL_SUIDGID.
        client.write(file, 0, b"x", 1 << 2).await.ok();
        assert_eq!(attr_out(&client.getattr(file).await.ok()).mode, 0o106755);
        let chown = SetattrFields { uid: 7, gid: 8, ..Default::default() };
        let attr = client.setattr(file, 2 | 4, chown).await;
        assert_eq!((attr.uid, attr.gid, attr.mode), (7, 8, 0o106755));
        // The kernel's chown of a setuid file: the owner and the mode it computed, in one.
        let chown = SetattrFields { uid: 9, mode: 0o755, ..Default::default() };
        let attr = client.setattr(file, 1 | 2, chown).await;
        assert_eq!((attr.uid, attr.gid, attr.mode), (9, 8, 0o100755));
        let (dir, _) = client.mkdir(ROOT, b"dir", 0o2775).await;
        let attr = client.setattr(dir, 4, SetattrFields { gid: 8, ..Default::default() }).await;
        assert_eq!((attr.gid, attr.mode), (8, 0o042755));
    })
}

#[test]
fn setattr_sizes_and_times() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let attr =
            client.setattr(file, 8, SetattrFields { size: 4096, ..Default::default() }).await;
        assert_eq!(attr.size, 4096);
        let times = SetattrFields { atime: (1000, 5), mtime: (2000, 6), ..Default::default() };
        client.setattr(file, 16 | 32, times).await;
        let payload = client.getattr(file).await.ok();
        let attr = &payload[16..];
        assert_eq!((u64_at(attr, 24), u32_at(attr, 48)), (1000, 5), "atime");
        assert_eq!((u64_at(attr, 32), u32_at(attr, 52)), (2000, 6), "mtime");
    })
}

/// Times the kernel can send but Fxfs can't store fail with EINVAL: before the epoch (the
/// kernel's seconds are signed), too late to represent, or with a second's nanoseconds or more.
#[test]
fn setattr_rejects_invalid_times() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        for time in [(-1i64 as u64, 0), (u64::MAX >> 1, 999_999_999), (1000, 1_000_000_000)] {
            let fields = SetattrFields { atime: time, mtime: time, ..Default::default() };
            assert_eq!(client.setattr_reply(file, 16 | 32, fields).await.error, EINVAL, "{time:?}");
        }
    })
}

#[test]
fn directories() {
    block_on(async {
        let mut client = Client::init_new().await;
        let root_ino = attr_out(&client.getattr(ROOT).await.ok()).ino;
        let (dir, _) = client.mkdir(ROOT, b"dir", 0o755).await;
        let (file, _) = client.create(dir, b"a-file", 0o644).await;
        let link = Body::default().name(b"link").name(b"a-file");
        let (link, _) = entry(&client.call(SYMLINK, dir, link).await.ok());
        assert_eq!(client.call(READLINK, link, Body::default()).await.ok(), b"a-file");
        let fifo = Body::default().u32(0o010644).u32(0).u32(0).u32(0).name(b"fifo");
        let (fifo, attr) = entry(&client.call(MKNOD, dir, fifo).await.ok());
        assert_eq!(attr.mode, 0o010644);

        // `.`, `..`, then the entries, each with the offset of the next; Fxfs's entries tell
        // directories, symlinks and files apart, and no other type.
        let handle = client.opendir(dir).await;
        let listed: Vec<_> = client
            .readdir(dir, handle, 0, 4096)
            .await
            .into_iter()
            .map(|dirent| (dirent.name, dirent.ino, dirent.off, dirent.type_))
            .collect();
        let expected: [(&[u8], u64, u64, u32); 5] = [
            (b".", dir, 1, 4),
            (b"..", root_ino, 2, 4),
            (b"a-file", file, 3, 8),
            (b"fifo", fifo, 4, 8),
            (b"link", link, 5, 10),
        ];
        assert_eq!(
            listed,
            expected.map(|(name, ino, off, type_)| (name.to_vec(), ino, off, type_))
        );
        // An entry that does not fit waits for the next read, from its offset.
        assert_eq!(client.readdir(dir, handle, 0, 64).await.len(), 2);
        let rest = client.readdir(dir, handle, 2, 4096).await;
        assert_eq!(
            rest.iter().map(|dirent| dirent.name.as_slice()).collect::<Vec<_>>(),
            [&b"a-file"[..], b"fifo", b"link"]
        );
        assert_eq!(client.readdir(dir, handle, 5, 4096).await, []);
        assert_eq!(client.call(RELEASEDIR, dir, release_in(handle)).await.ok(), b"");
        // A handle released, or never given, is bad.
        assert_eq!(client.call(READDIR, dir, read_in(handle, 0, 4096)).await.error, EBADF);

        // The root's `..` is itself.
        let handle = client.opendir(ROOT).await;
        let dots = client.readdir(ROOT, handle, 0, 4096).await;
        assert_eq!((dots[0].ino, dots[1].ino), (root_ino, root_ino));

        // READDIRPLUS looks each entry up but `.` and `..`, whose entries are empty.
        let handle = client.opendir(dir).await;
        let payload = client.call(READDIRPLUS, dir, read_in(handle, 0, 4096)).await.ok();
        let nodeids: Vec<Option<u64>> =
            dirents(&payload, true).iter().map(|(_, nodeid)| *nodeid).collect();
        assert_eq!(nodeids, [Some(0), Some(0), Some(file), Some(fifo), Some(link)]);
        // The kernel holds two lookups of each now, which it forgets in a batch.
        let batch = Body::default().u32(2).u32(0).u64(file).u64(2).u64(fifo).u64(2);
        assert_eq!(client.send(BATCH_FORGET, 0, &batch).await, None);
        assert_eq!(client.getattr(file).await.error, ESTALE);
        assert_eq!(client.getattr(fifo).await.error, ESTALE);
        assert_eq!(client.getattr(link).await.error, 0);
    })
}

/// READDIRPLUS leaves out entries removed since the directory was opened, and lists the rest.
#[test]
fn readdirplus_skips_removed_entries() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (dir, _) = client.mkdir(ROOT, b"dir", 0o755).await;
        let (a, _) = client.create(dir, b"a", 0o644).await;
        client.create(dir, b"b", 0o644).await;
        let (c, _) = client.create(dir, b"c", 0o644).await;
        let handle = client.opendir(dir).await;
        assert_eq!(client.call(UNLINK, dir, Body::default().name(b"b")).await.ok(), b"");
        let payload = client.call(READDIRPLUS, dir, read_in(handle, 0, 4096)).await.ok();
        let names: Vec<(Vec<u8>, Option<u64>)> = dirents(&payload, true)
            .into_iter()
            .map(|(dirent, nodeid)| (dirent.name, nodeid))
            .collect();
        let expected: [(&[u8], Option<u64>); 4] =
            [(b".", Some(0)), (b"..", Some(0)), (b"a", Some(a)), (b"c", Some(c))];
        assert_eq!(names, expected.map(|(name, nodeid)| (name.to_vec(), nodeid)));
    })
}

/// Names of directory entries must be names: not empty, `.` or `..`, without `/`, in UTF-8, as
/// Fxfs keeps them, and at most 255 bytes; and a request's body ends with its last name.
#[test]
fn names_are_validated() {
    block_on(async {
        let mut client = Client::init_new().await;
        for name in [&b""[..], b".", b"..", b"a/b", b"\xff"] {
            assert_eq!(client.lookup(ROOT, name).await.error, EINVAL, "{name:?}");
        }
        assert_eq!(client.lookup(ROOT, &[b'x'; 256]).await.error, ENAMETOOLONG);
        let trailing = Body::default().u32(0o755).u32(0o022).name(b"dir").bytes(b"junk");
        assert_eq!(client.call(MKDIR, ROOT, trailing).await.error, EINVAL);
        // A symlink's target is a path.
        let symlink = Body::default().name(b"link").name(b"../a/./b");
        let (link, _) = entry(&client.call(SYMLINK, ROOT, symlink).await.ok());
        assert_eq!(client.call(READLINK, link, Body::default()).await.ok(), b"../a/./b");
    })
}

#[test]
fn renames_and_links() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (a, _) = client.mkdir(ROOT, b"a", 0o755).await;
        let (file, _) = client.create(a, b"file", 0o644).await;
        let rename = Body::default().u64(ROOT).name(b"file").name(b"moved");
        assert_eq!(client.call(RENAME, a, rename).await.ok(), b"");
        assert_eq!(entry(&client.lookup(ROOT, b"moved").await.ok()).0, file);
        client.create(ROOT, b"other", 0o644).await;
        // RENAME_NOREPLACE.
        let noreplace = Body::default().u64(ROOT).u32(1).u32(0).name(b"moved").name(b"other");
        assert_eq!(client.call(RENAME2, ROOT, noreplace).await.error, EEXIST);
        // RENAME_EXCHANGE is unsupported.
        let exchange = Body::default().u64(ROOT).u32(2).u32(0).name(b"moved").name(b"other");
        assert_eq!(client.call(RENAME2, ROOT, exchange).await.error, EINVAL);

        let linked = client.call(LINK, a, Body::default().u64(file).name(b"link")).await.ok();
        let (nodeid, attr) = entry(&linked);
        assert_eq!((nodeid, attr.nlink), (file, 2));
    })
}

/// New nodes are their caller's, but in a directory whose setgid is set, whose group they take,
/// and a directory its setgid too, as Linux's VFS gives them.
#[test]
fn setgid_directories_give_their_group() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (shared, attr) = client.mkdir(ROOT, b"shared", 0o2775).await;
        assert_eq!((attr.gid, attr.mode), (100, 0o042755));
        client.gid = 200;
        let (_, attr) = client.create(shared, b"file", 0o644).await;
        assert_eq!((attr.uid, attr.gid, attr.mode), (1000, 100, 0o100644));
        let (_, attr) = client.mkdir(shared, b"sub", 0o755).await;
        assert_eq!((attr.gid, attr.mode), (100, 0o042755));
        let symlink = Body::default().name(b"link").name(b"file");
        let (_, attr) = entry(&client.call(SYMLINK, shared, symlink).await.ok());
        assert_eq!(attr.gid, 100);
        let fifo = Body::default().u32(0o010644).u32(0).u32(0).u32(0).name(b"fifo");
        let (_, attr) = entry(&client.call(MKNOD, shared, fifo).await.ok());
        assert_eq!(attr.gid, 100);
        // Elsewhere, the caller's.
        let (_, attr) = client.create(ROOT, b"mine", 0o644).await;
        assert_eq!(attr.gid, 200);
        let (_, attr) = client.mkdir(ROOT, b"plain", 0o755).await;
        assert_eq!((attr.gid, attr.mode), (200, 0o040755));
    })
}

/// Extended attributes are set, read, listed and removed as setxattr(2) and its kin have them: a
/// size of 0 asks for the value's size, a size short of it fails with ERANGE.
#[test]
fn extended_attributes() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let set = |name: &[u8], value: &[u8], flags: u32| {
            Body::default().u32(value.len() as u32).u32(flags).name(name).bytes(value)
        };
        let get = |name: &[u8], size: u32| Body::default().u32(size).u32(0).name(name);
        let list = |size: u32| Body::default().u32(size).u32(0);

        assert_eq!(client.call(SETXATTR, file, set(b"user.a", b"value", 0)).await.ok(), b"");
        assert_eq!(client.call(SETXATTR, file, set(b"user.b", b"", 0)).await.ok(), b"");
        let size = client.call(GETXATTR, file, get(b"user.a", 0)).await.ok();
        assert_eq!((size.len(), u32_at(&size, 0)), (8, 5));
        assert_eq!(client.call(GETXATTR, file, get(b"user.a", 5)).await.ok(), b"value");
        assert_eq!(client.call(GETXATTR, file, get(b"user.a", 4)).await.error, ERANGE);
        assert_eq!(client.call(GETXATTR, file, get(b"user.b", 64)).await.ok(), b"");
        assert_eq!(client.call(GETXATTR, file, get(b"user.c", 64)).await.error, ENODATA);

        let names = client.call(LISTXATTR, file, list(0)).await.ok();
        assert_eq!(u32_at(&names, 0), 14);
        let mut names: Vec<Vec<u8>> = client
            .call(LISTXATTR, file, list(14))
            .await
            .ok()
            .split_inclusive(|&byte| byte == 0)
            .map(<[u8]>::to_vec)
            .collect();
        names.sort();
        assert_eq!(names, [b"user.a\0".to_vec(), b"user.b\0".to_vec()]);
        assert_eq!(client.call(LISTXATTR, file, list(13)).await.error, ERANGE);

        // XATTR_CREATE and XATTR_REPLACE, not both.
        assert_eq!(client.call(SETXATTR, file, set(b"user.a", b"x", 1)).await.error, EEXIST);
        assert_eq!(client.call(SETXATTR, file, set(b"user.c", b"x", 2)).await.error, ENODATA);
        assert_eq!(client.call(SETXATTR, file, set(b"user.a", b"x", 3)).await.error, EINVAL);
        assert_eq!(client.call(SETXATTR, file, set(b"user.a", b"new", 2)).await.ok(), b"");
        assert_eq!(client.call(GETXATTR, file, get(b"user.a", 64)).await.ok(), b"new");
        // The value is all that follows the name, which its size must say.
        let short = Body::default().u32(10).u32(0).name(b"user.d").bytes(b"short");
        assert_eq!(client.call(SETXATTR, file, short).await.error, EINVAL);

        // Names are of the namespaces Linux's filesystems serve, as its VFS resolves them: of none,
        // EOPNOTSUPP; a namespace alone, EINVAL; of the others, as the user's.
        let refused = [(&b"foo.bar"[..], ENOTSUP), (b"user.", EINVAL), (b"trusted.", EINVAL)];
        for (name, error) in refused {
            let set = client.call(SETXATTR, file, set(name, b"x", 0)).await;
            assert_eq!(set.error, error, "{name:?}");
            assert_eq!(client.call(GETXATTR, file, get(name, 64)).await.error, error, "{name:?}");
            let remove = client.call(REMOVEXATTR, file, Body::default().name(name)).await;
            assert_eq!(remove.error, error, "{name:?}");
        }
        assert_eq!(client.call(SETXATTR, file, set(b"security.s", b"x", 0)).await.ok(), b"");
        assert_eq!(client.call(GETXATTR, file, get(b"security.s", 64)).await.ok(), b"x");

        assert_eq!(client.call(REMOVEXATTR, file, Body::default().name(b"user.a")).await.ok(), b"");
        assert_eq!(client.call(GETXATTR, file, get(b"user.a", 64)).await.error, ENODATA);
        let remove_again = client.call(REMOVEXATTR, file, Body::default().name(b"user.a")).await;
        assert_eq!(remove_again.error, ENODATA);
    })
}

#[test]
fn statfs_fsync_flush_release_and_fallocate() {
    block_on(async {
        let mut client = Client::init_new().await;
        let statfs = client.call(STATFS, ROOT, Body::default()).await.ok();
        assert_eq!(statfs.len(), 80);
        assert_eq!(u32_at(&statfs, 40), 4096, "bsize");
        assert_eq!(u64_at(&statfs, 0), STORAGE_SIZE / 4096, "blocks");
        assert_eq!(u32_at(&statfs, 44), 255, "namelen");

        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let fsync = Body::default().u64(0).u32(0).u32(0);
        assert_eq!(client.call(FSYNC, file, fsync).await.ok(), b"");
        // Files have nothing to flush: refused, FLUSH is not sent again.
        let flush = Body::default().u64(0).u32(0).u32(0).u64(0);
        assert_eq!(client.call(FLUSH, file, flush).await.error, ENOSYS);
        assert_eq!(client.call(RELEASE, file, release_in(0)).await.ok(), b"");

        let fallocate = |mode| Body::default().u64(0).u64(0).u64(8192).u32(mode).u32(0);
        assert_eq!(client.call(FALLOCATE, file, fallocate(0)).await.ok(), b"");
        assert_eq!(attr_out(&client.getattr(file).await.ok()).size, 8192);
        assert_eq!(client.call(FALLOCATE, file, fallocate(1)).await.error, ENOTSUP);
    })
}

#[test]
fn malformed_unsupported_and_stale_requests() {
    block_on(async {
        let mut client = Client::init_new().await;
        // A request shorter than its header cannot be replied to.
        let error = client.server.handle(&[0; 20]).await.expect_err("handled");
        assert_eq!(error.to_string(), "the request is shorter than its header");
        // A header claiming more than the request holds, and a body too short for its request,
        // are invalid.
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let mut request = client.request(GETATTR, file, &Body::default().u32(0).u32(0).u64(0));
        request[0] = 200;
        let reply = client.server.handle(&request).await.unwrap().unwrap();
        assert_eq!(decode(&reply, client.unique).error, EINVAL);
        assert_eq!(client.call(MKDIR, ROOT, Body::default().u32(0o755)).await.error, EINVAL);
        // A name must be NUL-terminated.
        assert_eq!(client.call(LOOKUP, ROOT, Body::default().bytes(b"file")).await.error, EINVAL);
        // Requests the server has no use for.
        let getlk = Body::default().u64(0).u64(0).bytes(&[0; 24]).u32(0).u32(0);
        assert_eq!(client.call(GETLK, file, getlk).await.error, ENOSYS);
        let setupmapping = Body::default().u64(0).u64(0).u64(4096).u64(1).u64(0);
        assert_eq!(client.call(SETUPMAPPING, file, setupmapping).await.error, ENOSYS);
        assert_eq!(client.call(1000, file, Body::default()).await.error, ENOSYS);
        // INTERRUPT has no reply.
        assert_eq!(client.send(INTERRUPT, 0, &Body::default().u64(1)).await, None);
        // A node the kernel never looked up, or forgot, is stale.
        assert_eq!(client.getattr(file + 1000).await.error, ESTALE);
        assert_eq!(client.lookup(file + 1000, b"name").await.error, ESTALE);
    })
}

/// DESTROY unmounts the filesystem, after which requests fail.
#[test]
fn destroy_unmounts() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        client.write(file, 0, b"kept", 0).await.ok();
        assert_eq!(client.send(DESTROY, 0, &Body::default()).await, None);
        assert_eq!(client.getattr(ROOT).await.error, ENOTCONN);

        // The filesystem gives its storage back once closed, with what was written.
        let storage = client.server.into_filesystem().close().await.unwrap();
        let fs = Filesystem::mount(storage.check().await.unwrap()).await.unwrap();
        let (file, _) = fs.root().lookup(b"file").await.unwrap();
        let mut buf = [0u8; 4];
        assert_eq!(file.read(0, &mut buf).await, Ok(4));
        assert_eq!(&buf, b"kept");
        drop(file);
        fs.close().await.unwrap();
    })
}

/// Requests are handled concurrently, on one thread.
#[test]
fn concurrent_requests() {
    block_on(async {
        let mut client = Client::init_new().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let contents = vec![7u8; 4 * pages::page_size() as usize];
        client.write(file, 0, &contents, 0).await.ok();

        let requests: Vec<(u64, Vec<u8>)> = (0..16u64)
            .map(|i| {
                let request = client.request(READ, file, &read_in(0, i * 1000, 1000));
                (client.unique, request)
            })
            .collect();
        let server = &client.server;
        let replies = futures::future::join_all(requests.iter().map(|(unique, request)| async {
            (*unique, server.handle(request).await.unwrap().unwrap())
        }))
        .await;
        for (unique, reply) in replies {
            let read = decode(&reply, unique).ok();
            assert_eq!(read, vec![7; 1000]);
        }
    })
}
