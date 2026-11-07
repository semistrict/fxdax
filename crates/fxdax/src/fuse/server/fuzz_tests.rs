// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The server fuzzed (TASK-149.39): random sequences of requests to one [`Server`], valid and
//! malformed, as no kernel would send them, the server checked after each against a model of what
//! the kernel holds.
//!
//! Valid requests are of every opcode the server serves, on names of a few, the root, nodes the
//! kernel holds, nodes it forgot and nodes it never had, directory handles open, released and never
//! given, with offsets, sizes, modes and times of every kind, forgets of more lookups than are held.
//! Malformed ones are valid ones made wrong: their header's length short of a header or past the
//! request, their body cut short of its message, a name without its NUL, bytes after the last name,
//! a write's or an extended attribute's size not its data's; or bodies of random bytes, and opcodes
//! the protocol hasn't.
//!
//! Each request is checked:
//!
//! - The server never panics, and handles each request (a request too short for its header alone
//!   has nothing to reply to, which is never sent).
//! - It replies to each request that has a reply, and only those (not `FORGET`, `BATCH_FORGET`,
//!   `INTERRUPT`): the reply's header says its length and the request's unique; an error is a
//!   known errno, with no payload; a payload is of the opcode's reply's shape, and each entry it
//!   gives has the node ID its inode number's is (the root's and FUSE's root ID swapped).
//! - A malformed request is refused with `EINVAL`, an unknown opcode with `ENOSYS`, a request on a
//!   node the kernel doesn't hold with `ESTALE`, a directory handle not open with `EBADF`.
//! - The server's tables are the model's: the nodes the kernel holds, with as many lookups as the
//!   replies gave it less those it forgot, each node the one its ID names; the directories open, by
//!   handle.
//!
//! At the end, `DESTROY` unmounts the filesystem, and its store is checked (fsck).
//!
//! Seeded and shrunk by proptest: `FXDAX_FUSE_FUZZ_CASES` cases (256 by default), from
//! `PROPTEST_RNG_SEED`'s seed if set.

use super::*;
use futures::executor::block_on;
use proptest::prelude::*;
use proptest::strategy::Union;
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use std::collections::{BTreeMap, BTreeSet};
use zerocopy::Immutable;

const STORAGE_SIZE: u64 = 16 << 20;

/// A node a request names, resolved against the model as it is sent.
#[derive(Clone, Copy, Debug)]
enum Target {
    Root,
    /// The `n`th node the kernel holds, modulo their number: the root if it holds none.
    Held(usize),
    /// The `n`th node the kernel was given and has forgotten: one never given if none.
    Forgotten(usize),
    /// A node ID never given: 0, or one past those Fxfs numbers.
    Never(u64),
}

/// A directory handle a request names.
#[derive(Clone, Copy, Debug)]
enum Handle {
    /// The `n`th open, modulo their number: one never given if none is.
    Open(usize),
    /// The `n`th released: one never given if none was.
    Released(usize),
    Never(u64),
}

/// A request, valid as the protocol has it, though no kernel would send it so.
#[derive(Clone, Debug)]
enum Request {
    Init { major: u32, minor: u32, flags: u64 },
    Lookup { dir: Target, name: Name },
    Getattr { node: Target },
    Setattr { node: Target, valid: u32, mode: u32, uid: u32, gid: u32, size: u64, time: (u64, u32) },
    Readlink { node: Target },
    Symlink { dir: Target, name: Name, target: Name },
    Mknod { dir: Target, name: Name, mode: u32, rdev: u32 },
    Mkdir { dir: Target, name: Name, mode: u32 },
    Unlink { dir: Target, name: Name },
    Rmdir { dir: Target, name: Name },
    Rename { dir: Target, name: Name, new_dir: Target, new_name: Name, flags: Option<u32> },
    Link { node: Target, dir: Target, name: Name },
    Create { dir: Target, name: Name, mode: u32 },
    Open { node: Target },
    Read { node: Target, offset: u64, size: u32 },
    Write { node: Target, offset: u64, data: Vec<u8>, flags: u32 },
    Statfs,
    Release { node: Target },
    Fsync { node: Target, dir: bool },
    Flush { node: Target },
    Setxattr { node: Target, name: Name, value: Vec<u8>, flags: u32 },
    Getxattr { node: Target, name: Name, size: u32 },
    Listxattr { node: Target, size: u32 },
    Removexattr { node: Target, name: Name },
    Opendir { node: Target },
    Readdir { handle: Handle, offset: u64, size: u32, plus: bool },
    Releasedir { handle: Handle },
    Fallocate { node: Target, offset: u64, length: u64, mode: u32 },
    Forget { node: Target, nlookup: u64 },
    BatchForget { forgets: Vec<(Target, u64)> },
    Interrupt,
    Unknown { opcode: u32 },
}

/// A name a request carries: one of a few, or one no directory may hold.
#[derive(Clone, Debug)]
struct Name(Vec<u8>);

/// How a valid request is made wrong.
#[derive(Clone, Copy, Debug)]
enum Mangling {
    /// The header's length short of a header.
    ShortHeader(u32),
    /// The header's length past the request's.
    LongHeader(u32),
    /// The body cut short of its message, by this many bytes more than its names.
    Cut(usize),
    /// The last name's NUL dropped.
    NoNul,
    /// Bytes after the last name.
    Trailing,
    /// A write's or an extended attribute's size past its data.
    Oversized,
}

/// What is sent.
#[derive(Clone, Debug)]
enum Sent {
    Valid(Request),
    Mangled(Request, Mangling),
    /// An opcode and a body of random bytes, on a node the kernel holds.
    Random {
        opcode: u32,
        node: Target,
        body: Vec<u8>,
    },
}

/// What the kernel holds, as the replies it got say: the model the server's tables are held to.
#[derive(Debug, Default)]
struct Model {
    /// The root's object ID.
    root: u64,
    /// The lookups held, by node ID.
    lookups: BTreeMap<u64, u64>,
    /// Every node ID a reply gave.
    given: BTreeSet<u64>,
    /// The directory handles open, and those released.
    open: BTreeSet<u64>,
    released: BTreeSet<u64>,
    unique: u64,
}

impl Model {
    fn node(&self, target: Target) -> u64 {
        match target {
            Target::Root => ROOT_ID,
            Target::Held(n) => match self.lookups.len() {
                0 => ROOT_ID,
                held => *self.lookups.keys().nth(n % held).unwrap(),
            },
            Target::Forgotten(n) => {
                let forgotten: Vec<u64> = self
                    .given
                    .iter()
                    .copied()
                    .filter(|id| !self.lookups.contains_key(id))
                    .collect();
                match forgotten.len() {
                    0 => self.never(n as u64),
                    len => forgotten[n % len],
                }
            }
            Target::Never(n) => self.never(n),
        }
    }

    /// A node ID never given: 0, or past those Fxfs numbers (but the root's, whose node ID is
    /// FUSE's root ID).
    fn never(&self, n: u64) -> u64 {
        match n % 3 {
            0 => 0,
            _ => (1 << 40) + n % (1 << 20),
        }
    }

    fn handle(&self, handle: Handle) -> u64 {
        let never = |n: u64| (1 << 50) + n % (1 << 20);
        let nth = |set: &BTreeSet<u64>, n: usize| match set.len() {
            0 => never(n as u64),
            len => *set.iter().nth(n % len).unwrap(),
        };
        match handle {
            Handle::Open(n) => nth(&self.open, n),
            Handle::Released(n) => nth(&self.released, n),
            Handle::Never(n) => never(n),
        }
    }

    /// Whether the kernel holds the node `nodeid`: the root, or one it has a lookup of.
    fn holds(&self, nodeid: u64) -> bool {
        nodeid == ROOT_ID || self.lookups.contains_key(&nodeid)
    }

    /// The node ID of the node whose object ID is `ino`, and the reverse.
    fn swap(&self, id: u64) -> u64 {
        match id {
            ROOT_ID => self.root,
            id if id == self.root => ROOT_ID,
            id => id,
        }
    }

    /// The kernel takes a lookup of the node an entry gives.
    fn looked_up(&mut self, nodeid: u64) {
        if nodeid != ROOT_ID {
            *self.lookups.entry(nodeid).or_default() += 1;
            self.given.insert(nodeid);
        }
    }

    /// The kernel forgets `nlookup` lookups of `nodeid`: as many as it holds at most.
    fn forget(&mut self, nodeid: u64, nlookup: u64) {
        if let Some(held) = self.lookups.get_mut(&nodeid) {
            *held = held.saturating_sub(nlookup);
            if *held == 0 {
                self.lookups.remove(&nodeid);
            }
        }
    }
}

/// A request's body, built field by field.
#[derive(Default)]
struct Body {
    bytes: Vec<u8>,
    /// Where its names begin: its messages are before.
    names: Option<usize>,
}

impl Body {
    fn message(mut self, message: &(impl IntoBytes + Immutable)) -> Self {
        self.bytes.extend_from_slice(message.as_bytes());
        self
    }

    fn name(mut self, name: &Name) -> Self {
        self.names.get_or_insert(self.bytes.len());
        self.bytes.extend_from_slice(&name.0);
        self.bytes.push(0);
        self
    }

    fn bytes(mut self, bytes: &[u8]) -> Self {
        self.bytes.extend_from_slice(bytes);
        self
    }
}

/// What a request's reply must be, besides well-formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// Any reply of its opcode's.
    Reply,
    /// One of these errors.
    Error(&'static [Errno]),
    /// None.
    Nothing,
}

const INVALID: &[Errno] = &[Errno::EINVAL];
const UNKNOWN: &[Errno] = &[Errno::ENOSYS];
/// A node the kernel doesn't hold; or, as the server checks some of a request's arguments first,
/// a time, size, mode or flags it refuses.
const STALE: &[Errno] = &[Errno::ESTALE, Errno::EINVAL, Errno::ENOTSUP];
/// A directory handle not open; or a size too large, which the server checks first.
const BAD_HANDLE: &[Errno] = &[Errno::EBADF, Errno::EINVAL];

/// A request encoded: its opcode, node ID and body, the node IDs it names (the header's among
/// them), and the handle it names.
struct Encoded {
    opcode: u32,
    nodeid: u64,
    body: Body,
    nodes: Vec<u64>,
    handle: Option<u64>,
}

impl Request {
    fn encode(&self, model: &Model) -> Encoded {
        let node = |target: &Target| model.node(*target);
        let (opcode, nodeid, body, mut nodes, mut handle) = match self {
            Request::Init { major, minor, flags } => {
                let init = InitIn {
                    major: *major,
                    minor: *minor,
                    max_readahead: 1 << 17,
                    flags: *flags as u32,
                };
                let ext = InitInExt { flags2: (*flags >> 32) as u32, unused: [0; 11] };
                (opcode::INIT, 0, Body::default().message(&init).message(&ext), vec![], None)
            }
            Request::Lookup { dir, name } => {
                (opcode::LOOKUP, node(dir), Body::default().name(name), vec![], None)
            }
            Request::Getattr { node: target } => {
                (opcode::GETATTR, node(target), Body::default().bytes(&[0; 16]), vec![], None)
            }
            Request::Setattr { node: target, valid, mode, uid, gid, size, time } => {
                let set = SetattrIn {
                    valid: *valid,
                    size: *size,
                    atime: time.0,
                    mtime: time.0,
                    atimensec: time.1,
                    mtimensec: time.1,
                    mode: *mode,
                    uid: *uid,
                    gid: *gid,
                    ..Default::default()
                };
                (opcode::SETATTR, node(target), Body::default().message(&set), vec![], None)
            }
            Request::Readlink { node: target } => {
                (opcode::READLINK, node(target), Body::default(), vec![], None)
            }
            Request::Symlink { dir, name, target } => {
                (opcode::SYMLINK, node(dir), Body::default().name(name).name(target), vec![], None)
            }
            Request::Mknod { dir, name, mode, rdev } => {
                let mknod = MknodIn { mode: *mode, rdev: *rdev, umask: 0o022, padding: 0 };
                (opcode::MKNOD, node(dir), Body::default().message(&mknod).name(name), vec![], None)
            }
            Request::Mkdir { dir, name, mode } => {
                let mkdir = MkdirIn { mode: *mode, umask: 0o022 };
                (opcode::MKDIR, node(dir), Body::default().message(&mkdir).name(name), vec![], None)
            }
            Request::Unlink { dir, name } => {
                (opcode::UNLINK, node(dir), Body::default().name(name), vec![], None)
            }
            Request::Rmdir { dir, name } => {
                (opcode::RMDIR, node(dir), Body::default().name(name), vec![], None)
            }
            Request::Rename { dir, name, new_dir, new_name, flags } => {
                let newdir = node(new_dir);
                let (opcode, body) = match flags {
                    None => (opcode::RENAME, Body::default().message(&RenameIn { newdir })),
                    Some(flags) => (
                        opcode::RENAME2,
                        Body::default().message(&Rename2In { newdir, flags: *flags, padding: 0 }),
                    ),
                };
                (opcode, node(dir), body.name(name).name(new_name), vec![newdir], None)
            }
            Request::Link { node: target, dir, name } => {
                let oldnodeid = node(target);
                let body = Body::default().message(&LinkIn { oldnodeid }).name(name);
                (opcode::LINK, node(dir), body, vec![oldnodeid], None)
            }
            Request::Create { dir, name, mode } => {
                let create = CreateIn { flags: 0o100002, mode: *mode, umask: 0o022, open_flags: 0 };
                (
                    opcode::CREATE,
                    node(dir),
                    Body::default().message(&create).name(name),
                    vec![],
                    None,
                )
            }
            Request::Open { node: target } => (
                opcode::OPEN,
                node(target),
                Body::default().bytes(&[2, 0, 0, 0, 0, 0, 0, 0]),
                vec![],
                None,
            ),
            Request::Read { node: target, offset, size } => {
                let read = ReadIn { offset: *offset, size: *size, ..Default::default() };
                (opcode::READ, node(target), Body::default().message(&read), vec![], None)
            }
            Request::Write { node: target, offset, data, flags } => {
                let write = WriteIn {
                    offset: *offset,
                    size: data.len() as u32,
                    write_flags: *flags,
                    ..Default::default()
                };
                (
                    opcode::WRITE,
                    node(target),
                    Body::default().message(&write).bytes(data),
                    vec![],
                    None,
                )
            }
            Request::Statfs => (opcode::STATFS, ROOT_ID, Body::default(), vec![], None),
            Request::Release { node: target } => (
                opcode::RELEASE,
                node(target),
                Body::default().message(&ReleaseIn::default()),
                vec![],
                None,
            ),
            Request::Fsync { node: target, dir } => {
                let opcode = if *dir { opcode::FSYNCDIR } else { opcode::FSYNC };
                (opcode, node(target), Body::default().bytes(&[0; 16]), vec![], None)
            }
            Request::Flush { node: target } => {
                (opcode::FLUSH, node(target), Body::default().bytes(&[0; 24]), vec![], None)
            }
            Request::Setxattr { node: target, name, value, flags } => {
                let set = SetxattrIn { size: value.len() as u32, flags: *flags };
                (
                    opcode::SETXATTR,
                    node(target),
                    Body::default().message(&set).name(name).bytes(value),
                    vec![],
                    None,
                )
            }
            Request::Getxattr { node: target, name, size } => {
                let get = GetxattrIn { size: *size, padding: 0 };
                (
                    opcode::GETXATTR,
                    node(target),
                    Body::default().message(&get).name(name),
                    vec![],
                    None,
                )
            }
            Request::Listxattr { node: target, size } => {
                let list = GetxattrIn { size: *size, padding: 0 };
                (opcode::LISTXATTR, node(target), Body::default().message(&list), vec![], None)
            }
            Request::Removexattr { node: target, name } => {
                (opcode::REMOVEXATTR, node(target), Body::default().name(name), vec![], None)
            }
            Request::Opendir { node: target } => {
                (opcode::OPENDIR, node(target), Body::default().bytes(&[0; 8]), vec![], None)
            }
            Request::Readdir { handle, offset, size, plus } => {
                let fh = model.handle(*handle);
                let read = ReadIn { fh, offset: *offset, size: *size, ..Default::default() };
                let opcode = if *plus { opcode::READDIRPLUS } else { opcode::READDIR };
                // The node ID is the directory's, which the server takes from the handle.
                (opcode, ROOT_ID, Body::default().message(&read), vec![], Some(fh))
            }
            Request::Releasedir { handle } => {
                let fh = model.handle(*handle);
                let release = ReleaseIn { fh, ..Default::default() };
                (opcode::RELEASEDIR, ROOT_ID, Body::default().message(&release), vec![], Some(fh))
            }
            Request::Fallocate { node: target, offset, length, mode } => {
                let fallocate = FallocateIn {
                    fh: 0,
                    offset: *offset,
                    length: *length,
                    mode: *mode,
                    padding: 0,
                };
                (opcode::FALLOCATE, node(target), Body::default().message(&fallocate), vec![], None)
            }
            Request::Forget { node: target, nlookup } => {
                let forget = ForgetIn { nlookup: *nlookup };
                (opcode::FORGET, node(target), Body::default().message(&forget), vec![], None)
            }
            Request::BatchForget { forgets } => {
                let mut body = Body::default()
                    .message(&BatchForgetIn { count: forgets.len() as u32, dummy: 0 });
                for (target, nlookup) in forgets {
                    body = body.message(&ForgetOne { nodeid: node(target), nlookup: *nlookup });
                }
                (opcode::BATCH_FORGET, 0, body, vec![], None)
            }
            Request::Interrupt => (
                opcode::INTERRUPT,
                0,
                Body::default().bytes(&[1, 0, 0, 0, 0, 0, 0, 0]),
                vec![],
                None,
            ),
            Request::Unknown { opcode } => {
                (*opcode, ROOT_ID, Body::default().bytes(&[0; 16]), vec![], None)
            }
        };
        // The requests that name a node by the header's node ID, which must be held.
        let by_header = !matches!(
            opcode,
            opcode::INIT
                | opcode::STATFS
                | opcode::FORGET
                | opcode::BATCH_FORGET
                | opcode::INTERRUPT
                | opcode::OPEN
                | opcode::FLUSH
                | opcode::RELEASE
                | opcode::READDIR
                | opcode::READDIRPLUS
                | opcode::RELEASEDIR
        ) && is_served(opcode);
        if by_header {
            nodes.push(nodeid);
        }
        // A handle read must be open; releasing one that isn't releases nothing, as the kernel
        // takes no reply of a release's but that it was done.
        if !matches!(opcode, opcode::READDIR | opcode::READDIRPLUS) {
            handle = None;
        }
        Encoded { opcode, nodeid, body, nodes, handle }
    }
}

/// Whether the server serves `opcode`.
fn is_served(opcode: u32) -> bool {
    matches!(
        opcode,
        opcode::INIT
            | opcode::DESTROY
            | opcode::FORGET
            | opcode::BATCH_FORGET
            | opcode::INTERRUPT
            | opcode::LOOKUP
            | opcode::GETATTR
            | opcode::SETATTR
            | opcode::READLINK
            | opcode::SYMLINK
            | opcode::MKNOD
            | opcode::MKDIR
            | opcode::UNLINK
            | opcode::RMDIR
            | opcode::RENAME
            | opcode::RENAME2
            | opcode::LINK
            | opcode::CREATE
            | opcode::READ
            | opcode::WRITE
            | opcode::STATFS
            | opcode::OPEN
            | opcode::FLUSH
            | opcode::RELEASE
            | opcode::FSYNC
            | opcode::FSYNCDIR
            | opcode::SETXATTR
            | opcode::GETXATTR
            | opcode::LISTXATTR
            | opcode::REMOVEXATTR
            | opcode::OPENDIR
            | opcode::READDIR
            | opcode::READDIRPLUS
            | opcode::RELEASEDIR
            | opcode::FALLOCATE
    )
}

/// How much of the body of a request of `opcode` is the message the server must read, before its
/// names if it has any: none if the server reads none.
fn message_len(opcode: u32, names: Option<usize>, len: usize) -> Option<usize> {
    let read = match opcode {
        // InitIn, the extension after it read only if its flags say so.
        opcode::INIT => return Some(size_of::<InitIn>()),
        opcode::SETATTR
        | opcode::MKNOD
        | opcode::MKDIR
        | opcode::RENAME
        | opcode::RENAME2
        | opcode::LINK
        | opcode::CREATE
        | opcode::READ
        | opcode::WRITE
        | opcode::SETXATTR
        | opcode::GETXATTR
        | opcode::LISTXATTR
        | opcode::READDIR
        | opcode::READDIRPLUS
        | opcode::RELEASEDIR
        | opcode::FALLOCATE
        | opcode::FORGET
        | opcode::BATCH_FORGET => true,
        _ => false,
    };
    let len = names.unwrap_or(len);
    (read && len > 0).then_some(len)
}

/// Whether `opcode`'s request has a reply.
fn has_reply(opcode: u32) -> bool {
    !matches!(opcode, opcode::FORGET | opcode::BATCH_FORGET | opcode::INTERRUPT | opcode::DESTROY)
}

/// The bytes of the request `unique`, and what its reply must be.
fn bytes(sent: &Sent, model: &Model, unique: u64) -> (Vec<u8>, u32, Expect) {
    let (encoded, mangling) = match sent {
        Sent::Valid(request) => (request.encode(model), None),
        Sent::Mangled(request, mangling) => (request.encode(model), Some(*mangling)),
        Sent::Random { opcode, node, body } => {
            let encoded = Encoded {
                opcode: *opcode,
                nodeid: model.node(*node),
                body: Body::default().bytes(body),
                nodes: vec![],
                handle: None,
            };
            (encoded, None)
        }
    };
    let Encoded { opcode, nodeid, body: Body { mut bytes, names }, nodes, handle } = encoded;
    let mut len = None;
    let mut expect = if !is_served(opcode) {
        Expect::Error(UNKNOWN)
    } else if !has_reply(opcode) {
        Expect::Nothing
    } else if opcode == opcode::OPEN || opcode == opcode::FLUSH {
        Expect::Error(UNKNOWN)
    } else if nodes.iter().any(|&nodeid| !model.holds(nodeid)) {
        Expect::Error(STALE)
    } else if handle.is_some_and(|fh| !model.open.contains(&fh)) {
        Expect::Error(BAD_HANDLE)
    } else {
        Expect::Reply
    };
    if let Sent::Random { .. } = sent {
        // What random bytes come to is the server's to say: a reply as its opcode has, or none.
        expect = match expect {
            Expect::Nothing | Expect::Error(UNKNOWN) => expect,
            _ => Expect::Reply,
        };
    }
    if let Some(mangling) = mangling {
        let invalid = Expect::Error(INVALID);
        let cut = message_len(opcode, names, bytes.len());
        match mangling {
            // The header is checked first, of every request, before its opcode.
            Mangling::ShortHeader(short) => {
                len = Some(short % size_of::<InHeader>() as u32);
                expect = invalid;
            }
            Mangling::LongHeader(more) => {
                len = Some((size_of::<InHeader>() + bytes.len()) as u32 + 1 + more % 4096);
                expect = invalid;
            }
            // Of a request whose message the server reads: a forget's, which has no reply, is
            // dropped.
            Mangling::Cut(more) if cut.is_some() => {
                let message_len = cut.unwrap();
                bytes.truncate(message_len - 1 - more % message_len);
                if has_reply(opcode) {
                    expect = invalid;
                }
            }
            Mangling::NoNul | Mangling::Trailing
                if names.is_some() && opcode != opcode::SETXATTR =>
            {
                match mangling {
                    Mangling::NoNul => {
                        bytes.pop();
                    }
                    _ => bytes.extend_from_slice(b"junk\0"),
                }
                expect = invalid;
            }
            Mangling::Oversized if matches!(opcode, opcode::WRITE | opcode::SETXATTR) => {
                // Both sizes are a body's first u32 past its offset or nothing.
                let at = if opcode == opcode::WRITE { 16 } else { 0 };
                let size = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                bytes[at..at + 4].copy_from_slice(&(size + 1).to_le_bytes());
                expect = invalid;
            }
            // A mangling the request has nothing for leaves it valid.
            _ => {}
        }
    }
    let header = InHeader {
        len: len.unwrap_or((size_of::<InHeader>() + bytes.len()) as u32),
        opcode,
        unique,
        nodeid,
        uid: 1000,
        gid: 100,
        pid: 1234,
        padding: 0,
    };
    let mut request = header.as_bytes().to_vec();
    request.extend_from_slice(&bytes);
    (request, opcode, expect)
}

/// `attr`, a reply's `fuse_attr`, of the node `nodeid`: its inode number is the node's object ID.
fn check_attr(model: &Model, nodeid: u64, attr: &super::super::abi::Attr) -> Result<(), String> {
    if attr.ino != model.swap(nodeid) {
        return Err(format!(
            "node {nodeid}'s inode number is {}, not {}",
            attr.ino,
            model.swap(nodeid)
        ));
    }
    if FileType::from_mode(attr.mode).is_none() {
        return Err(format!("node {nodeid}'s mode {:o} has no type", attr.mode));
    }
    Ok(())
}

/// A reply's entry: if it gives a node, the kernel takes a lookup of it.
fn check_entry(model: &mut Model, entry: &EntryOut, positive: bool) -> Result<(), String> {
    if entry.nodeid == 0 {
        return match positive {
            true => Err("an entry made has no node".to_owned()),
            false => Ok(()),
        };
    }
    check_attr(model, entry.nodeid, &entry.attr)?;
    model.looked_up(entry.nodeid);
    Ok(())
}

/// Checks the reply to a request of `opcode`, and takes what it gives into the model.
fn check_reply(
    model: &mut Model,
    request: &[u8],
    opcode: u32,
    expect: Expect,
    reply: Option<Vec<u8>>,
) -> Result<(), String> {
    let header = InHeader::read_from_prefix(request).unwrap().0;
    let Some(reply) = reply else {
        return match expect {
            Expect::Nothing => Ok(()),
            _ => Err(format!("no reply, where {expect:?}")),
        };
    };
    if expect == Expect::Nothing {
        return Err(format!("a reply, where none: {reply:?}"));
    }
    let (out, payload) =
        OutHeader::read_from_prefix(&reply).map_err(|_| format!("a {}-byte reply", reply.len()))?;
    if out.len as usize != reply.len() || out.unique != header.unique {
        return Err(format!(
            "the reply's header {out:?} is not of the {}-byte reply to {}",
            reply.len(),
            header.unique
        ));
    }
    if out.error > 0 {
        return Err(format!("a positive error {}", out.error));
    }
    if out.error < 0 {
        let errno = Errno::from_raw(-out.error);
        if errno.name().is_none() || !payload.is_empty() {
            return Err(format!("an error {errno:?}, with a {}-byte payload", payload.len()));
        }
        return match expect {
            Expect::Error(expected) if !expected.contains(&errno) => {
                Err(format!("{errno:?}, where {expected:?}"))
            }
            _ => Ok(()),
        };
    }
    if let Expect::Error(expected) = expect {
        return Err(format!("a {}-byte payload, where {expected:?}", payload.len()));
    }
    let exactly = |len: usize| match payload.len() == len {
        true => Ok(()),
        false => Err(format!("a {}-byte payload, where {len}", payload.len())),
    };
    let at_most = |len: u64| match payload.len() as u64 <= len {
        true => Ok(()),
        false => Err(format!("a {}-byte payload, where at most {len}", payload.len())),
    };
    let body = &request[size_of::<InHeader>()..];
    match opcode {
        opcode::INIT => exactly(size_of::<InitOut>()),
        opcode::LOOKUP => {
            exactly(size_of::<EntryOut>())?;
            check_entry(model, &EntryOut::read_from_bytes(payload).unwrap(), false)
        }
        opcode::SYMLINK | opcode::MKNOD | opcode::MKDIR | opcode::LINK => {
            exactly(size_of::<EntryOut>())?;
            check_entry(model, &EntryOut::read_from_bytes(payload).unwrap(), true)
        }
        opcode::CREATE => {
            exactly(size_of::<EntryOut>() + size_of::<OpenOut>())?;
            check_entry(model, &EntryOut::read_from_prefix(payload).unwrap().0, true)
        }
        opcode::GETATTR | opcode::SETATTR => {
            exactly(size_of::<AttrOut>())?;
            check_attr(model, header.nodeid, &AttrOut::read_from_bytes(payload).unwrap().attr)
        }
        opcode::READLINK => match payload.is_empty() {
            true => Err("an empty target".to_owned()),
            false => Ok(()),
        },
        opcode::READ => at_most(ReadIn::read_from_prefix(body).unwrap().0.size as u64),
        opcode::WRITE => {
            exactly(size_of::<WriteOut>())?;
            let written = WriteOut::read_from_bytes(payload).unwrap().size;
            match written <= WriteIn::read_from_prefix(body).unwrap().0.size {
                true => Ok(()),
                false => Err(format!("{written} bytes written, more than sent")),
            }
        }
        opcode::STATFS => exactly(size_of::<Kstatfs>()),
        opcode::GETXATTR | opcode::LISTXATTR => {
            match GetxattrIn::read_from_prefix(body).unwrap().0.size {
                0 => exactly(size_of::<GetxattrOut>()),
                size => {
                    at_most(size as u64)?;
                    match opcode == opcode::LISTXATTR
                        && payload.last().is_some_and(|&last| last != 0)
                    {
                        true => Err("names not NUL-terminated".to_owned()),
                        false => Ok(()),
                    }
                }
            }
        }
        opcode::OPENDIR => {
            exactly(size_of::<OpenOut>())?;
            let fh = OpenOut::read_from_bytes(payload).unwrap().fh;
            match model.open.insert(fh) && !model.released.contains(&fh) {
                true => Ok(()),
                false => Err(format!("handle {fh} given again")),
            }
        }
        opcode::RELEASEDIR => {
            exactly(0)?;
            let fh = ReleaseIn::read_from_prefix(body).unwrap().0.fh;
            model.open.remove(&fh);
            model.released.insert(fh);
            Ok(())
        }
        opcode::READDIR | opcode::READDIRPLUS => {
            let read = ReadIn::read_from_prefix(body).unwrap().0;
            at_most(read.size as u64)?;
            check_dirents(model, payload, read.offset, opcode == opcode::READDIRPLUS)
        }
        _ => exactly(0),
    }
}

/// A `READDIR` or `READDIRPLUS` reply's entries, read from `offset`: each whole, padded with
/// zeros, of a valid name, its offset the next's, past `offset`; with `plus`, each but `.` and
/// `..` an entry the kernel takes a lookup of.
fn check_dirents(
    model: &mut Model,
    mut payload: &[u8],
    mut offset: u64,
    plus: bool,
) -> Result<(), String> {
    while !payload.is_empty() {
        let entry = match plus {
            true => {
                let (entry, rest) =
                    EntryOut::read_from_prefix(payload).map_err(|_| "a cut entry".to_owned())?;
                payload = rest;
                Some(entry)
            }
            false => None,
        };
        let (dirent, rest) =
            Dirent::read_from_prefix(payload).map_err(|_| "a cut dirent".to_owned())?;
        let len = (size_of::<Dirent>() + dirent.namelen as usize).next_multiple_of(8);
        let name = rest.get(..dirent.namelen as usize).ok_or("a cut name")?;
        let padding = payload.get(size_of::<Dirent>() + name.len()..len).ok_or("a cut dirent")?;
        if padding.iter().any(|&byte| byte != 0) || dirent.off <= offset {
            return Err(format!("a bad dirent {dirent:?} at offset {offset}"));
        }
        let dots = matches!(name, b"." | b"..");
        if !dots && crate::filesystem::valid_name(name).is_err() {
            return Err(format!("a name {name:?} no directory holds"));
        }
        if let Some(entry) = entry {
            check_entry(model, &entry, !dots)?;
            if dots != (entry.nodeid == 0) {
                return Err(format!("{name:?}'s entry {entry:?}"));
            }
        }
        offset = dirent.off;
        payload = &payload[len..];
    }
    Ok(())
}

/// The server's tables are the model's: the nodes the kernel holds, with their lookups, each the
/// node its ID names; and the directories open.
fn check_tables(server: &Server, model: &Model) -> Result<(), String> {
    let nodes = server.nodes.lock().unwrap();
    let lookups: BTreeMap<u64, u64> =
        nodes.iter().map(|(&id, looked)| (id, looked.lookups)).collect();
    if lookups != model.lookups {
        return Err(format!(
            "the server holds lookups {lookups:?}, where the kernel holds {:?}",
            model.lookups
        ));
    }
    if let Some((id, looked)) =
        nodes.iter().find(|(id, looked)| looked.node.inode().0 != model.swap(**id))
    {
        return Err(format!("node {id} holds object {}", looked.node.inode().0));
    }
    drop(nodes);
    let open: BTreeSet<u64> = server.dirs.lock().unwrap().open.keys().copied().collect();
    if open != model.open {
        return Err(format!("the server has directories {open:?} open, where {:?}", model.open));
    }
    Ok(())
}

/// Sends `sent` in turn to a new server, checking each reply and the tables after it; then
/// DESTROY, and checks the store.
fn fuzz(sent: &[Sent]) -> Result<(), String> {
    block_on(async {
        let fs = Filesystem::format(crate::Storage::memory(STORAGE_SIZE)).await.unwrap();
        let server = Server::new(fs);
        let mut model = Model { root: server.root, ..Model::default() };
        let mut history = Vec::new();
        for (index, sent) in sent.iter().enumerate() {
            model.unique += 1;
            let (request, opcode, expect) = bytes(sent, &model, model.unique);
            let reply = server
                .handle(&request)
                .await
                .map_err(|error| format!("{index}: {sent:?}: {error:#}"))?;
            history.push(format!(
                "{index}: {sent:?}: {:?}",
                reply.as_deref().map(|reply| &reply[..reply.len().min(16)])
            ));
            let failed = |what: String| {
                format!("{index}: {sent:?}: {what}\n\nsent:\n{}", history.join("\n"))
            };
            check_reply(&mut model, &request, opcode, expect, reply).map_err(failed)?;
            // The forgets the kernel sends, which have no reply.
            if expect == Expect::Nothing {
                let body = &request[size_of::<InHeader>()..];
                match opcode {
                    opcode::FORGET => {
                        if let Ok((ForgetIn { nlookup }, _)) = ForgetIn::read_from_prefix(body) {
                            model.forget(
                                InHeader::read_from_prefix(&request).unwrap().0.nodeid,
                                nlookup,
                            );
                        }
                    }
                    opcode::BATCH_FORGET => {
                        if let Ok((batch, mut rest)) = BatchForgetIn::read_from_prefix(body) {
                            for _ in 0..batch.count {
                                let Ok((one, after)) = ForgetOne::read_from_prefix(rest) else {
                                    break;
                                };
                                model.forget(one.nodeid, one.nlookup);
                                rest = after;
                            }
                        }
                    }
                    _ => {}
                }
            }
            check_tables(&server, &model).map_err(failed)?;
        }
        let destroy = InHeader {
            len: size_of::<InHeader>() as u32,
            opcode: opcode::DESTROY,
            unique: model.unique + 1,
            ..Default::default()
        };
        match server.handle(destroy.as_bytes()).await {
            Ok(None) => {}
            other => return Err(format!("DESTROY: {other:?}")),
        }
        let storage = server
            .into_filesystem()
            .close()
            .await
            .map_err(|error| format!("closing: {error:#}"))?;
        storage.check().await.map_err(|error| format!("the store is not whole: {error:#}"))?;
        Ok(())
    })
}

// The generator.

fn target() -> BoxedStrategy<Target> {
    prop_oneof![
        3 => Just(Target::Root),
        8 => any::<usize>().prop_map(Target::Held),
        1 => any::<usize>().prop_map(Target::Forgotten),
        1 => any::<u64>().prop_map(Target::Never),
    ]
    .boxed()
}

fn handle() -> BoxedStrategy<Handle> {
    prop_oneof![
        8 => any::<usize>().prop_map(Handle::Open),
        1 => any::<usize>().prop_map(Handle::Released),
        1 => any::<u64>().prop_map(Handle::Never),
    ]
    .boxed()
}

fn name() -> BoxedStrategy<Name> {
    const NAMES: [&[u8]; 4] = [b"a", b"b", b"c", b"d"];
    const INVALID_NAMES: [&[u8]; 6] = [b"", b".", b"..", b"a/b", b"\xff", &[b'n'; 256]];
    prop_oneof![
        12 => prop::sample::select(&NAMES[..]).prop_map(|name| Name(name.to_vec())),
        1 => prop::sample::select(&INVALID_NAMES[..]).prop_map(|name| Name(name.to_vec())),
        1 => prop::collection::vec(1u8..=255, 1..300).prop_map(Name),
    ]
    .boxed()
}

fn mode() -> BoxedStrategy<u32> {
    prop_oneof![
        3 => prop::sample::select(&[0o100644, 0o040755, 0o010644, 0o140755, 0o020600, 0o060600, 0o2755, 0o644][..]),
        1 => any::<u32>(),
    ]
    .boxed()
}

fn offset() -> BoxedStrategy<u64> {
    prop_oneof![
        3 => prop::sample::select(&[0, 1, 4095, 4096, 1 << 20, 1 << 40, u64::MAX - 1, u64::MAX, i64::MAX as u64][..]),
        3 => 0u64..100_000,
        1 => any::<u64>(),
    ]
    .boxed()
}

fn size() -> BoxedStrategy<u32> {
    prop_oneof![
        3 => prop::sample::select(&[0, 1, 4, 64, 4096, MAX_WRITE, MAX_WRITE + 1, u32::MAX][..]),
        2 => 0u32..10_000,
    ]
    .boxed()
}

fn data() -> BoxedStrategy<Vec<u8>> {
    prop_oneof![
        8 => prop::collection::vec(any::<u8>(), 0..300),
        1 => (0usize..70_000, any::<u8>()).prop_map(|(len, byte)| vec![byte; len]),
    ]
    .boxed()
}

fn time() -> BoxedStrategy<(u64, u32)> {
    prop::sample::select(
        &[
            (0, 0),
            (1, 5),
            (1_700_000_000, 999_999_999),
            (u64::MAX >> 1, 999_999_999),
            (-1i64 as u64, 0),
            (5, 1_000_000_000),
        ][..],
    )
    .boxed()
}

fn request() -> BoxedStrategy<Request> {
    let requests: Vec<(u32, BoxedStrategy<Request>)> = vec![
        (
            1,
            (
                prop::sample::select(&[6u32, 7, 8][..]),
                prop::sample::select(&[0u32, 26, 27, 36, 40][..]),
                any::<u64>(),
            )
                .prop_map(|(major, minor, flags)| Request::Init { major, minor, flags })
                .boxed(),
        ),
        (8, (target(), name()).prop_map(|(dir, name)| Request::Lookup { dir, name }).boxed()),
        (4, target().prop_map(|node| Request::Getattr { node }).boxed()),
        (
            4,
            (
                target(),
                any::<u32>(),
                mode(),
                prop::sample::select(&[0u32, 7, 1000, u32::MAX][..]),
                any::<u32>(),
                offset(),
                time(),
            )
                .prop_map(|(node, valid, mode, uid, gid, size, time)| Request::Setattr {
                    node,
                    // Of the bits the server reads, and some it doesn't.
                    valid: valid & 0x1fff,
                    mode,
                    uid,
                    gid,
                    size: size % (1 << 41),
                    time,
                })
                .boxed(),
        ),
        (2, target().prop_map(|node| Request::Readlink { node }).boxed()),
        (
            3,
            (target(), name(), name())
                .prop_map(|(dir, name, target)| Request::Symlink { dir, name, target })
                .boxed(),
        ),
        (
            3,
            (target(), name(), mode(), any::<u32>())
                .prop_map(|(dir, name, mode, rdev)| Request::Mknod { dir, name, mode, rdev })
                .boxed(),
        ),
        (
            6,
            (target(), name(), mode())
                .prop_map(|(dir, name, mode)| Request::Mkdir { dir, name, mode })
                .boxed(),
        ),
        (3, (target(), name()).prop_map(|(dir, name)| Request::Unlink { dir, name }).boxed()),
        (2, (target(), name()).prop_map(|(dir, name)| Request::Rmdir { dir, name }).boxed()),
        (
            3,
            (
                target(),
                name(),
                target(),
                name(),
                prop::option::of(prop::sample::select(&[0u32, 1, 2, 4, 7][..])),
            )
                .prop_map(|(dir, name, new_dir, new_name, flags)| Request::Rename {
                    dir,
                    name,
                    new_dir,
                    new_name,
                    flags,
                })
                .boxed(),
        ),
        (
            3,
            (target(), target(), name())
                .prop_map(|(node, dir, name)| Request::Link { node, dir, name })
                .boxed(),
        ),
        (
            6,
            (target(), name(), mode())
                .prop_map(|(dir, name, mode)| Request::Create { dir, name, mode })
                .boxed(),
        ),
        (1, target().prop_map(|node| Request::Open { node }).boxed()),
        (
            4,
            (target(), offset(), size())
                .prop_map(|(node, offset, size)| Request::Read { node, offset, size })
                .boxed(),
        ),
        (
            6,
            (target(), offset(), data(), prop::sample::select(&[0u32, 1, 4, 7][..]))
                .prop_map(|(node, offset, data, flags)| Request::Write {
                    node,
                    offset,
                    data,
                    flags,
                })
                .boxed(),
        ),
        (1, Just(Request::Statfs).boxed()),
        (1, target().prop_map(|node| Request::Release { node }).boxed()),
        (2, (target(), any::<bool>()).prop_map(|(node, dir)| Request::Fsync { node, dir }).boxed()),
        (1, target().prop_map(|node| Request::Flush { node }).boxed()),
        (
            4,
            (target(), name(), data(), prop::sample::select(&[0u32, 1, 2, 3, 4][..]))
                .prop_map(|(node, name, value, flags)| Request::Setxattr {
                    node,
                    name,
                    value,
                    flags,
                })
                .boxed(),
        ),
        (
            3,
            (target(), name(), size())
                .prop_map(|(node, name, size)| Request::Getxattr { node, name, size })
                .boxed(),
        ),
        (2, (target(), size()).prop_map(|(node, size)| Request::Listxattr { node, size }).boxed()),
        (
            2,
            (target(), name()).prop_map(|(node, name)| Request::Removexattr { node, name }).boxed(),
        ),
        (4, target().prop_map(|node| Request::Opendir { node }).boxed()),
        (
            5,
            (handle(), prop_oneof![3 => 0u64..8, 1 => any::<u64>()], size(), any::<bool>())
                .prop_map(|(handle, offset, size, plus)| Request::Readdir {
                    handle,
                    offset,
                    size,
                    plus,
                })
                .boxed(),
        ),
        (2, handle().prop_map(|handle| Request::Releasedir { handle }).boxed()),
        (
            2,
            (target(), offset(), offset(), prop::sample::select(&[0u32, 1, 2, 3][..]))
                .prop_map(|(node, offset, length, mode)| Request::Fallocate {
                    node,
                    offset,
                    length,
                    mode,
                })
                .boxed(),
        ),
        (
            5,
            (target(), prop::sample::select(&[1u64, 2, 3, u64::MAX][..]))
                .prop_map(|(node, nlookup)| Request::Forget { node, nlookup })
                .boxed(),
        ),
        (
            2,
            prop::collection::vec((target(), prop::sample::select(&[1u64, 2, u64::MAX][..])), 0..4)
                .prop_map(|forgets| Request::BatchForget { forgets })
                .boxed(),
        ),
        (1, Just(Request::Interrupt).boxed()),
        (
            1,
            prop::sample::select(
                &[
                    0u32,
                    7,
                    31,
                    32,
                    33,
                    34,
                    37,
                    39,
                    40,
                    41,
                    46,
                    47,
                    48,
                    49,
                    50,
                    51,
                    52,
                    53,
                    4096,
                    u32::MAX,
                ][..],
            )
            .prop_map(|opcode| Request::Unknown { opcode })
            .boxed(),
        ),
    ];
    Union::new_weighted(requests).boxed()
}

fn sent() -> BoxedStrategy<Sent> {
    let mangling = prop_oneof![
        any::<u32>().prop_map(Mangling::ShortHeader),
        any::<u32>().prop_map(Mangling::LongHeader),
        any::<usize>().prop_map(Mangling::Cut),
        Just(Mangling::NoNul),
        Just(Mangling::Trailing),
        Just(Mangling::Oversized),
    ];
    let opcode = prop_oneof![
        4 => prop::sample::select(&[1u32, 3, 4, 6, 8, 9, 10, 11, 12, 13, 15, 16, 21, 22, 23, 24, 26, 28, 29, 35, 42, 43, 44, 45][..]),
        1 => any::<u32>(),
    ];
    prop_oneof![
        16 => request().prop_map(Sent::Valid),
        3 => (request(), mangling).prop_map(|(request, mangling)| Sent::Mangled(request, mangling)),
        1 => (opcode, target(), prop::collection::vec(any::<u8>(), 0..200))
            .prop_map(|(opcode, node, body)| Sent::Random { opcode, node, body }),
    ]
    .boxed()
}

/// TASK-149.39: random requests, valid and malformed, to one server: see the module's docs.
#[test]
fn random_requests_are_answered_as_the_protocol_says() {
    let cases = std::env::var("FXDAX_FUSE_FUZZ_CASES").map_or(256, |cases| cases.parse().unwrap());
    let config =
        Config { cases, failure_persistence: None, max_shrink_time: 300_000, ..Config::default() };
    // INIT first, as the kernel sends it, then the rest.
    let init = Sent::Valid(Request::Init { major: 7, minor: 36, flags: u64::MAX });
    let strategy = proptest::collection::vec(sent(), 1..120);
    let result = TestRunner::new(config).run(&strategy, |sent| {
        let sent: Vec<Sent> = std::iter::once(init.clone()).chain(sent).collect();
        fuzz(&sent).map_err(TestCaseError::fail)
    });
    if let Err(error) = result {
        panic!("{error}");
    }
}
