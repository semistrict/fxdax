//! Tests of the FUSE server, driven as a virtio-fs device drives it: with requests encoded as a
//! guest's FUSE client encodes them, split across buffers as descriptor chains split them.
//!
//! The client here encodes and decodes messages by hand, from Linux's `include/uapi/linux/fuse.h`,
//! independently of the server's definitions.

use futures::executor::block_on;
use fxdax::fuse::Server;
use fxdax::{Filesystem, Storage, VolatileSlice, VolatileSliceMut, kernel};

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
const GETXATTR: u32 = 22;
const FLUSH: u32 = 25;
const INIT: u32 = 26;
const OPENDIR: u32 = 27;
const READDIR: u32 = 28;
const RELEASEDIR: u32 = 29;
const CREATE: u32 = 35;
const INTERRUPT: u32 = 36;
const DESTROY: u32 = 38;
const BATCH_FORGET: u32 = 42;
const FALLOCATE: u32 = 43;
const READDIRPLUS: u32 = 44;
const RENAME2: u32 = 45;
const SETUPMAPPING: u32 = 48;
const REMOVEMAPPING: u32 = 49;

// Linux errno values.
const ENOENT: i32 = 2;
const EEXIST: i32 = 17;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const ENAMETOOLONG: i32 = 36;
const ENOSYS: i32 = 38;
const ENOTEMPTY: i32 = 39;
const EPROTO: i32 = 71;
const ENOTSUP: i32 = 95;
const ENOTCONN: i32 = 107;

const ENTRY_OUT_LEN: usize = 128;
const DIRENT_LEN: usize = 24;

/// A request's body, built field by field, little-endian as the guest's are.
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

/// The parts of an `fuse_attr` the tests check.
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

/// A `fuse_entry_out`'s node id and attributes.
fn entry(payload: &[u8]) -> (u64, Attr) {
    // The name and attributes stay valid as long as the guest likes.
    assert_eq!(u64_at(payload, 16), 24 * 60 * 60);
    (u64_at(payload, 0), Attr::decode(&payload[40..ENTRY_OUT_LEN]))
}

/// A `fuse_attr_out`'s attributes.
fn attr_out(payload: &[u8]) -> Attr {
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

/// The entries of a `READDIR` reply, or of a `READDIRPLUS` reply with the node ids of their
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

/// A guest's FUSE client of a server.
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

    /// Sends a request, split across two buffers, with room for a reply of `reply_len` bytes,
    /// split across three, and returns the reply's length and buffer.
    async fn send(
        &mut self,
        opcode: u32,
        nodeid: u64,
        body: &Body,
        reply_len: usize,
    ) -> (usize, Vec<u8>) {
        self.unique += 1;
        let mut request = Body::default()
            .u32((40 + body.0.len()) as u32)
            .u32(opcode)
            .u64(self.unique)
            .u64(nodeid)
            .u32(self.uid)
            .u32(self.gid)
            .u32(1234)
            .u32(0)
            .bytes(&body.0)
            .0;
        let split = request.len() / 2;
        let (first, second) = request.split_at_mut(split);
        let request = [VolatileSlice::from(&*first), VolatileSlice::from(&*second)];
        let mut reply = vec![0xa5u8; reply_len];
        let len = {
            let (a, rest) = reply.split_at_mut(reply_len.min(7));
            let (b, c) = rest.split_at_mut(rest.len() / 2);
            let mut reply =
                [VolatileSliceMut::from(a), VolatileSliceMut::from(b), VolatileSliceMut::from(c)];
            self.server.handle(&request, &mut reply).await.expect("handle failed")
        };
        (len, reply)
    }

    /// Calls `opcode` on `nodeid` with `body`, and returns its reply.
    async fn call(&mut self, opcode: u32, nodeid: u64, body: Body) -> Reply {
        self.call_with_room(opcode, nodeid, body, 1 << 16).await
    }

    async fn call_with_room(&mut self, opcode: u32, nodeid: u64, body: Body, room: usize) -> Reply {
        let (len, reply) = self.send(opcode, nodeid, &body, room).await;
        assert!(len >= 16, "a {len}-byte reply");
        assert_eq!(u32_at(&reply, 0) as usize, len, "the header's length is not the reply's");
        assert_eq!(u64_at(&reply, 8), self.unique, "the reply is not to the request");
        let error = u32_at(&reply, 4) as i32;
        if error != 0 {
            assert_eq!(len, 16, "an error with a payload");
        }
        Reply { error: -error, payload: reply[16..len].to_vec() }
    }

    async fn init(&mut self) -> Vec<u8> {
        // 7.31, with the flags a Linux guest sends, extended.
        let flags: u64 = (1 << 0)
            | (1 << 3)
            | (1 << 13)
            | (1 << 14)
            | (1 << 22)
            | (1 << 28)
            | (1 << 30)
            | (1 << 33);
        let body = Body::default()
            .u32(7)
            .u32(31)
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
        // An open file, without a handle, whose cache stays valid.
        assert_eq!(payload.len(), ENTRY_OUT_LEN + 16);
        assert_eq!(u64_at(&payload, ENTRY_OUT_LEN), 0);
        assert_eq!(u32_at(&payload, ENTRY_OUT_LEN + 8), 2);
        entry(&payload)
    }

    async fn mkdir(&mut self, parent: u64, name: &[u8]) -> (u64, Attr) {
        let body = Body::default().u32(0o777).u32(0o022).name(name);
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
        let body = read_in(0, offset, size);
        self.call(READ, nodeid, body).await.ok()
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
        u64_at(&payload, 0)
    }
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

/// INIT negotiates the protocol version and the features both sides support.
#[test]
fn init_negotiates() {
    block_on(async {
        let mut client = Client::new().await;
        let out = client.init().await;
        assert_eq!(out.len(), 64);
        assert_eq!((u32_at(&out, 0), u32_at(&out, 4)), (7, 27));
        assert_eq!(u32_at(&out, 8), 1 << 17, "max_readahead");
        // Of what was offered: async reads, atomic O_TRUNC, readdirplus (auto), max pages,
        // KILLPRIV_V2 and the extension, but not per-inode DAX.
        let flags = u32_at(&out, 12) as u64 | (u32_at(&out, 52) as u64) << 32;
        assert_eq!(
            flags,
            (1 << 0) | (1 << 3) | (1 << 13) | (1 << 14) | (1 << 22) | (1 << 28) | (1 << 30)
        );
        assert_eq!(u32_at(&out, 20), 1 << 20, "max_write");
        let max_pages = u16::from_le_bytes(out[28..30].try_into().unwrap());
        assert_eq!(max_pages as u64, (1 << 20) / kernel::page_size());

        // A newer major version is answered with the server's, for the guest to retry.
        let body = Body::default().u32(8).u32(0).u32(0).u32(0);
        let out = client.call(INIT, 0, body).await.ok();
        assert_eq!((u32_at(&out, 0), u32_at(&out, 4), u32_at(&out, 12)), (7, 27, 0));
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
        let mut client = Client::new().await;
        client.init().await;
        let (dir, attr) = client.mkdir(ROOT, b"dir").await;
        assert_eq!((attr.mode, attr.uid, attr.gid, attr.nlink), (0o040755, 1000, 100, 2));
        let (file, attr) = client.create(dir, b"file", 0o666).await;
        assert_eq!((attr.mode, attr.size), (0o100644, 0));
        assert_eq!(attr.ino, file);
        assert_eq!(entry(&client.lookup(dir, b"file").await.ok()), (file, attr));
        assert_eq!(client.lookup(dir, b"missing").await.error, ENOENT);
        let body = Body::default().u32(0).u32(0o644).u32(0).u32(0).name(b"file");
        assert_eq!(client.call(CREATE, dir, body).await.error, EEXIST);
        assert_eq!(attr_out(&client.getattr(dir).await.ok()).ino, dir);

        assert_eq!(client.call(RMDIR, ROOT, Body::default().name(b"dir")).await.error, ENOTEMPTY);
        assert_eq!(client.call(UNLINK, dir, Body::default().name(b"file")).await.ok(), b"");
        assert_eq!(client.lookup(dir, b"file").await.error, ENOENT);
        // FORGET has no reply; the file is gone once the guest forgets its two lookups.
        let (len, _) = client.send(FORGET, file, &Body::default().u64(2), 64).await;
        assert_eq!(len, 0);
        assert_eq!(client.getattr(file).await.error, ENOENT);
        assert_eq!(client.call(RMDIR, ROOT, Body::default().name(b"dir")).await.ok(), b"");
    })
}

#[test]
fn file_io() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let contents: Vec<u8> =
            (0..3 * kernel::page_size() as usize).map(|i| (i % 253) as u8).collect();
        let written = client.write(file, 5, &contents, 0).await.ok();
        assert_eq!(u32_at(&written, 0) as usize, contents.len());
        assert_eq!(attr_out(&client.getattr(file).await.ok()).size, 5 + contents.len() as u64);

        let read = client.read(file, 5, contents.len() as u32).await;
        assert_eq!(read, contents);
        // Reads stop at the end of the file.
        assert_eq!(
            client.read(file, 5 + contents.len() as u64 - 3, 100).await,
            &contents[contents.len() - 3..]
        );
        assert_eq!(client.read(file, 1 << 30, 100).await, b"");
        // A read is cut to the reply's room.
        let reply = client.call_with_room(READ, file, read_in(0, 5, 100), 16 + 10).await.ok();
        assert_eq!(reply, &contents[..10]);
        // A write's data must all be there.
        let short =
            Body::default().u64(0).u64(0).u32(100).u32(0).u64(0).u32(0).u32(0).bytes(b"short");
        assert_eq!(client.call(WRITE, file, short).await.error, EINVAL);
        assert_eq!(client.call(READ, ROOT, read_in(0, 0, 10)).await.error, EISDIR);
    })
}

#[test]
fn truncating_and_killing_privileges() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        client.write(file, 0, b"contents", 0).await.ok();
        let setuid = SetattrFields { mode: 0o6755, ..Default::default() };
        assert_eq!(client.setattr(file, 1, setuid).await.mode, 0o106755);

        // An open with O_TRUNC truncates, and clears setuid and setgid if asked to.
        let open = Body::default().u32(0o1000 | 2).u32(1);
        let out = client.call(OPEN, file, open).await.ok();
        assert_eq!((u64_at(&out, 0), u32_at(&out, 8)), (0, 2));
        let attr = attr_out(&client.getattr(file).await.ok());
        assert_eq!((attr.size, attr.mode), (0, 0o100755));
        // An open without it leaves the file be.
        client.write(file, 0, b"again", 0).await.ok();
        client.call(OPEN, file, Body::default().u32(2).u32(0)).await.ok();
        assert_eq!(attr_out(&client.getattr(file).await.ok()).size, 5);

        // A write clears them if asked to.
        let setuid = SetattrFields { mode: 0o4755, ..Default::default() };
        client.setattr(file, 1, setuid).await;
        client.write(file, 0, b"x", 1 << 2).await.ok();
        assert_eq!(attr_out(&client.getattr(file).await.ok()).mode, 0o100755);
        // As do chowns.
        client.setattr(file, 1, SetattrFields { mode: 0o4755, ..Default::default() }).await;
        let chown = SetattrFields { uid: 7, gid: 8, ..Default::default() };
        let attr = client.setattr(file, 2 | 4, chown).await;
        assert_eq!((attr.uid, attr.gid, attr.mode), (7, 8, 0o100755));
    })
}

#[test]
fn setattr_sizes_and_times() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
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

/// Times the guest can send but Fxfs can't store fail with EINVAL: before the epoch (the guest's
/// seconds are signed), too late to represent, or with a second's nanoseconds or more.
#[test]
fn setattr_rejects_invalid_times() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
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
        let mut client = Client::new().await;
        client.init().await;
        let (dir, _) = client.mkdir(ROOT, b"dir").await;
        let (file, _) = client.create(dir, b"a-file", 0o644).await;
        let link = entry(
            &client.call(SYMLINK, dir, Body::default().name(b"link").name(b"a-file")).await.ok(),
        )
        .0;
        assert_eq!(client.call(READLINK, link, Body::default()).await.ok(), b"a-file");
        let fifo = Body::default().u32(0o010644).u32(0).u32(0).u32(0).name(b"fifo");
        let (fifo, attr) = entry(&client.call(MKNOD, dir, fifo).await.ok());
        assert_eq!(attr.mode, 0o010644);

        let handle = client.opendir(dir).await;
        let listed =
            dirents(&client.call(READDIR, dir, read_in(handle, 0, 4096)).await.ok(), false);
        let names: Vec<(&[u8], u64, u32)> =
            listed.iter().map(|(d, _)| (d.name.as_slice(), d.off, d.type_)).collect();
        assert_eq!(
            names,
            [
                (&b"."[..], 1, 4),
                (b"..", 2, 4),
                (b"a-file", 3, 8),
                (b"fifo", 4, 8),
                (b"link", 5, 10)
            ]
        );
        assert_eq!(listed[0].0.ino, dir);
        assert_eq!(listed[1].0.ino, ROOT);
        // An entry that does not fit waits for the next request, from its offset.
        let first = dirents(&client.call(READDIR, dir, read_in(handle, 0, 64)).await.ok(), false);
        assert_eq!(first.len(), 2);
        let rest = dirents(&client.call(READDIR, dir, read_in(handle, 2, 4096)).await.ok(), false);
        assert_eq!(
            rest.iter().map(|(d, _)| d.name.as_slice()).collect::<Vec<_>>(),
            [&b"a-file"[..], b"fifo", b"link"]
        );
        assert_eq!(
            client
                .call(RELEASEDIR, dir, Body::default().u64(handle).u32(0).u32(0).u64(0))
                .await
                .ok(),
            b""
        );

        // READDIRPLUS looks each entry up but `.` and `..`.
        let handle = client.opendir(dir).await;
        let listed =
            dirents(&client.call(READDIRPLUS, dir, read_in(handle, 0, 4096)).await.ok(), true);
        let nodeids: Vec<Option<u64>> = listed.iter().map(|(_, nodeid)| *nodeid).collect();
        assert_eq!(nodeids, [Some(dir), Some(ROOT), Some(file), Some(fifo), Some(link)]);
        // The guest holds two lookups on each now, which it forgets in a batch.
        let batch = Body::default().u32(2).u32(0).u64(file).u64(2).u64(fifo).u64(2);
        assert_eq!(client.send(BATCH_FORGET, 0, &batch, 64).await.0, 0);
        assert_eq!(client.getattr(file).await.error, ENOENT);
        assert_eq!(client.getattr(link).await.error, 0);
    })
}

/// READDIRPLUS leaves out entries removed since the directory was opened, and lists the rest.
#[test]
fn readdirplus_skips_removed_entries() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
        let (dir, _) = client.mkdir(ROOT, b"dir").await;
        let (a, _) = client.create(dir, b"a", 0o644).await;
        client.create(dir, b"b", 0o644).await;
        let (c, _) = client.create(dir, b"c", 0o644).await;
        let handle = client.opendir(dir).await;
        assert_eq!(client.call(UNLINK, dir, Body::default().name(b"b")).await.ok(), b"");
        let listed =
            dirents(&client.call(READDIRPLUS, dir, read_in(handle, 0, 4096)).await.ok(), true);
        let names: Vec<(&[u8], Option<u64>)> =
            listed.iter().map(|(dirent, nodeid)| (dirent.name.as_slice(), *nodeid)).collect();
        assert_eq!(
            names,
            [(&b"."[..], Some(dir)), (b"..", Some(ROOT)), (b"a", Some(a)), (b"c", Some(c))]
        );
    })
}

/// Names of directory entries must be names: not empty, `.` or `..`, without `/`, and at most 255
/// bytes; and a request's body ends with its last name.
#[test]
fn names_are_validated() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
        for name in [&b""[..], b".", b"..", b"a/b"] {
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
        let mut client = Client::new().await;
        client.init().await;
        let (a, _) = client.mkdir(ROOT, b"a").await;
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

#[test]
fn statfs_fsync_flush_release_and_fallocate() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
        let statfs = client.call(STATFS, ROOT, Body::default()).await.ok();
        assert_eq!(statfs.len(), 80);
        assert_eq!(u32_at(&statfs, 40), 4096, "bsize");
        assert_eq!(u64_at(&statfs, 0), STORAGE_SIZE / 4096, "blocks");
        assert_eq!(u32_at(&statfs, 44), 255, "namelen");

        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let fsync = Body::default().u64(0).u32(0).u32(0);
        assert_eq!(client.call(FSYNC, file, fsync).await.ok(), b"");
        assert_eq!(
            client.call(FLUSH, file, Body::default().u64(0).u32(0).u32(0).u64(0)).await.ok(),
            b""
        );
        assert_eq!(
            client.call(RELEASE, file, Body::default().u64(0).u32(0).u32(0).u64(0)).await.ok(),
            b""
        );

        let fallocate = |mode| Body::default().u64(0).u64(0).u64(8192).u32(mode).u32(0);
        assert_eq!(client.call(FALLOCATE, file, fallocate(0)).await.ok(), b"");
        assert_eq!(attr_out(&client.getattr(file).await.ok()).size, 8192);
        assert_eq!(client.call(FALLOCATE, file, fallocate(1)).await.error, ENOTSUP);
    })
}

#[test]
fn dax_mappings() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
        let ps = kernel::page_size();
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        client.write(file, 0, b"mapped", 0).await.ok();
        let setup = |moffset| Body::default().u64(0).u64(0).u64(ps).u64(1 | 2).u64(moffset);
        let remove = |moffset| Body::default().u32(1).u64(moffset).u64(ps);
        assert_eq!(client.call(SETUPMAPPING, file, setup(0)).await.error, ENOSYS);

        let window = client.server.filesystem().create_dax_window(2 * ps, None).unwrap();
        assert_eq!(client.call(SETUPMAPPING, file, setup(ps)).await.ok(), b"");
        assert!(window.is_mapped(ps));
        assert_eq!(client.call(SETUPMAPPING, file, setup(ps + 1)).await.error, EINVAL);
        assert_eq!(client.call(REMOVEMAPPING, ROOT, remove(ps)).await.ok(), b"");
        assert!(!window.is_mapped(ps));
    })
}

#[test]
fn malformed_and_unsupported_requests() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
        // A request shorter than its header cannot be replied to.
        let short = [0u8; 20];
        let mut reply = [0u8; 64];
        let result = client
            .server
            .handle(&[VolatileSlice::from(&short[..])], &mut [VolatileSliceMut::from(&mut reply[..])])
            .await;
        assert_eq!(
            result.expect_err("handled").to_string(),
            "the request is shorter than its header"
        );
        // Nor one whose reply does not fit.
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let body = Body::default().u32(0).u32(0).u64(0);
        let mut request = Body::default()
            .u32(56)
            .u32(GETATTR)
            .u64(99)
            .u64(file)
            .u32(0)
            .u32(0)
            .u32(0)
            .u32(0)
            .bytes(&body.0)
            .0;
        let mut reply = [0u8; 50];
        let result = client
            .server
            .handle(&[VolatileSlice::from(&request[..])], &mut [VolatileSliceMut::from(&mut reply[..])])
            .await;
        assert_eq!(result.expect_err("handled").to_string(), "the reply does not fit its buffers");
        // A header claiming more than the request holds, and a body too short for its request,
        // are invalid.
        request[0] = 200;
        let mut reply = [0u8; 64];
        let len = client
            .server
            .handle(&[VolatileSlice::from(&request[..])], &mut [VolatileSliceMut::from(&mut reply[..])])
            .await
            .unwrap();
        assert_eq!((len, u32_at(&reply, 4) as i32), (16, -EINVAL));
        assert_eq!(client.call(MKDIR, ROOT, Body::default().u32(0o755)).await.error, EINVAL);
        // A name must be NUL-terminated.
        assert_eq!(client.call(LOOKUP, ROOT, Body::default().bytes(b"file")).await.error, EINVAL);
        // Requests without a use here.
        assert_eq!(
            client.call(GETXATTR, file, Body::default().u32(0).u32(0).name(b"user.x")).await.error,
            ENOSYS
        );
        assert_eq!(client.call(1000, file, Body::default()).await.error, ENOSYS);
        // INTERRUPT has no reply.
        assert_eq!(client.send(INTERRUPT, 0, &Body::default().u64(1), 64).await.0, 0);
    })
}

/// DESTROY unmounts the filesystem, after which requests fail.
#[test]
fn destroy_unmounts() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        client.write(file, 0, b"kept", 0).await.ok();
        assert_eq!(client.send(DESTROY, 0, &Body::default(), 64).await.0, 0);
        assert_eq!(client.getattr(ROOT).await.error, ENOTCONN);

        // The filesystem gives its storage back once closed, with what was written.
        let storage = client.server.into_filesystem().close().await.unwrap();
        let fs = Filesystem::mount(storage.check().await.unwrap()).await.unwrap();
        let file = fs.lookup(fxdax::Inode::ROOT, b"file").await.unwrap().inode;
        let mut buf = [0u8; 4];
        assert_eq!(fs.read(file, 0, &mut buf).await, Ok(4));
        assert_eq!(&buf, b"kept");
        fs.close().await.unwrap();
    })
}

/// Requests are handled concurrently, on one thread.
#[test]
fn concurrent_requests() {
    block_on(async {
        let mut client = Client::new().await;
        client.init().await;
        let (file, _) = client.create(ROOT, b"file", 0o644).await;
        let contents = vec![7u8; 4 * kernel::page_size() as usize];
        client.write(file, 0, &contents, 0).await.ok();

        let server = &client.server;
        let requests: Vec<Vec<u8>> = (0..16u64)
            .map(|i| {
                Body::default()
                    .u32(80)
                    .u32(READ)
                    .u64(100 + i)
                    .u64(file)
                    .u32(0)
                    .u32(0)
                    .u32(0)
                    .u32(0)
                    .bytes(&read_in(0, i * 1000, 1000).0)
                    .0
            })
            .collect();
        let mut replies = vec![vec![0u8; 16 + 1000]; 16];
        let lens = futures::future::join_all(requests.iter().zip(replies.iter_mut()).map(
            |(request, reply)| async move {
                server
                    .handle(
                        &[VolatileSlice::from(&request[..])],
                        &mut [VolatileSliceMut::from(&mut reply[..])],
                    )
                    .await
                    .unwrap()
            },
        ))
        .await;
        assert!(lens.iter().all(|&len| len == 16 + 1000));
        for (i, reply) in replies.iter().enumerate() {
            assert_eq!(u64_at(reply, 8), 100 + i as u64);
            assert!(reply[16..].iter().all(|&byte| byte == 7));
        }
    })
}
