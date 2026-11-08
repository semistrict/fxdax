// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the messages of the wire ([`super::wire`]) as types, each encoded and decoded here alone,
//! so that both ends read a field at the index they write it at (TASK-160.9): the requests a VM
//! process makes ([`Request`]), what it says of the stage operations it made ([`Made`]), what the
//! daemon tells it unasked ([`Told`]), its stage operations among that ([`Stage`]), and what
//! replies say besides their status ([`Opened`], [`Info`], [`Advanced`], [`mappings`]). The bytes
//! are those each end wrote by hand before.
//!
//! An [`OPEN`] and its reply say their sender's wire [`VERSION`] ([`version`], TASK-141.3): one of
//! another is refused (`EPROTONOSUPPORT`) before anything else of it, as nothing else of it may
//! mean what this version has it mean.
//!
//! Decoding refuses (`EINVAL`) what a message's kind gives no meaning: a flag it doesn't know, or
//! flags it knows together that it doesn't, an argument out of its range, a payload or descriptors
//! it has no use for, a status, which replies alone have, and a path that is empty or holds a NUL.
//! The words a kind doesn't use it ignores. What a request names is checked as far as the message
//! goes: whether its tenant may name it, and its store holds it, is the daemon's to say.

use super::wire::*;
use crate::pages::{ArenaId, PhysLocation};
use crate::{Access, Commit, Errno, ExtentId, ExtentInfo, MappingContents};
use crate::{WindowMapping, WriteBarriers};
use std::borrow::Cow;
use std::num::NonZeroU64;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// A request, of a VM process to the daemon, which a reply ([`REPLY`]) answers.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) enum Request<'a> {
    /// [`OPEN`]: the store at `path`, opened as `opening` says. Its paths are owned: the daemon
    /// opens a store to write holding no thread, beyond the message.
    Open { path: PathBuf, opening: Opening },
    /// [`READ`]: `len` bytes at `offset` in `extent`, into its reply's payload, [`MAX_PAYLOAD`]
    /// at most, or the connection's transfer area.
    Read { extent: ExtentId, offset: u64, len: u64, into: ReadInto },
    /// [`WRITE`]: `data` at `offset` in `extent`, ordered with `barriers`.
    Write { extent: ExtentId, offset: u64, data: Data<'a>, barriers: WriteBarriers<'a> },
    /// [`FREE`].
    Free(ExtentId),
    /// [`FREE_MANY`]: `first`, then `rest`, in order.
    FreeMany { first: ExtentId, rest: Cow<'a, [ExtentId]> },
    /// [`MAP`]: the mapping into the window.
    Map(WindowMapping),
    /// fxdax: [`MAP_MANY`]: the mappings into the window, in order, one at least.
    MapMany(Cow<'a, [WindowMapping]>),
    /// [`UNMAP`].
    Unmap { window_offset: u64, len: u64 },
    /// [`COMMIT`], completing as `commit` says, setting the root to `root` if given.
    Commit { commit: Commit, root: Option<&'a [u8]> },
    /// [`ROOT`].
    Root,
    /// [`INFO`].
    Info,
    /// [`WINDOW`]: of `size` bytes, [`MAX_WINDOW`] at most; fxdax: copying on write, of
    /// `private_max` private pages at most, as many as it has pages at most, if given, a writer's;
    /// or not, a follower's, its `args[1]` 0. Its VM process keeps `budget` runs of pages
    /// installed at most, 0 if it doesn't say (TASK-111.5).
    Window { size: u64, private_max: Option<u64>, budget: u64 },
    /// [`FAULT`].
    Fault(Fault),
    /// [`FORK`]: to `branch`, a new directory at `path`; a point if `quiescent`.
    Fork { path: PathBuf, branch: u64, quiescent: bool },
    /// [`RESIZED`]: the guest's answer to a shrink to `capacity` bytes.
    Resized { capacity: u64, took: bool },
    /// [`MAPPINGS`].
    Mappings,
    /// [`ADVANCE`].
    Advance,
    /// [`LET_GO`].
    LetGo { epoch: u64 },
    /// [`PIN`].
    Pin(ExtentId),
    /// [`UNPIN`].
    Unpin(ExtentId),
    /// [`TRANSFER`]: the area, of `len` bytes.
    Transfer { len: u64, area: Area<'a> },
    /// [`BUILD`]: the commit the VM process was built of, a label's characters, [`MAX_BUILD`] at
    /// most.
    Build(&'a str),
    /// fxdax: [`PRIVATE`].
    Private { window_offset: u64, len: u64 },
    /// fxdax: [`STATS`]: the answer to ask `ask`, with its guest's stats if it has a guest that
    /// answers, [`MAX_STATS`] bytes at most.
    Stats { ask: u64, guest: Option<&'a str> },
}

/// How an [`OPEN`] opens its store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Opening {
    /// To write, if it exists (no flags).
    Existing,
    /// To write, created with this many bytes of capacity if it doesn't exist ([`OPEN_CREATE`]).
    Create(u64),
    /// To write, forked from the store at `from` if it doesn't exist, of branch `branch` and
    /// `capacity` bytes, which the daemon takes from its bucket if it doesn't exist either
    /// ([`OPEN_FORK_FROM`]).
    ForkFrom { from: PathBuf, branch: u64, capacity: u64 },
    /// fxdax: followed, read-only (backlog doc-8, [`OPEN_READ_ONLY`]).
    ReadOnly,
}

/// Where a [`READ`]'s bytes go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadInto {
    /// The reply's payload.
    Reply,
    /// The connection's transfer area, at `at` ([`READ_TRANSFER`]).
    Transfer { at: u64 },
}

/// The bytes a [`WRITE`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Data<'a> {
    /// In its payload.
    Inline(&'a [u8]),
    /// `len` bytes of the connection's transfer area at `at` ([`WRITE_TRANSFER`]).
    Transfer { at: u64, len: u64 },
}

/// A guest's fault on the window ([`FAULT`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fault {
    /// Where in the window.
    pub offset: u64,
    /// A store ([`FAULT_WRITE`]), or not.
    pub access: Access,
    /// On that page alone ([`FAULT_PAGE`]).
    pub page: bool,
    /// fxdax: the VM process's probe of the page, to read, not the guest's access
    /// ([`FAULT_PROBE`]): on that page alone.
    pub probe: bool,
}

/// The shared memory a [`TRANSFER`] carries, its descriptor.
#[derive(Debug)]
pub(crate) struct Area<'a>(pub BorrowedFd<'a>);

/// fxdax: areas are one if their descriptors, numbers of their processes', not of the wire's,
/// name one file.
#[cfg(test)]
impl PartialEq for Area<'_> {
    fn eq(&self, other: &Self) -> bool {
        use std::os::fd::AsRawFd;
        let file = |fd: &BorrowedFd<'_>| {
            // SAFETY: A zeroed stat is a valid one to fill in.
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: Fills `stat` in, of a descriptor the area borrows.
            assert_eq!(unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) }, 0);
            (stat.st_dev, stat.st_ino)
        };
        file(&self.0) == file(&other.0)
    }
}

/// A message encoded: its header, but for a request's ID, which its sender gives it, and the
/// lengths, which the sending sets; its payload, the parts sent one after another, as a write's
/// data and the root after it, which no copy joins; and the descriptor it carries, if any.
pub(crate) struct Encoded<'a> {
    pub header: Header,
    pub payload: [Cow<'a, [u8]>; 2],
    pub fd: Option<BorrowedFd<'a>>,
}

impl<'a> Encoded<'a> {
    fn new(kind: u32, flags: u32, args: [u64; 5]) -> Self {
        let header = Header { kind, flags, args, ..Default::default() };
        Self { header, payload: Default::default(), fd: None }
    }

    /// With `part` as its payload.
    fn carrying(mut self, part: impl Into<Cow<'a, [u8]>>) -> Self {
        self.payload[0] = part.into();
        self
    }

    /// With `part` after what it carries.
    fn followed_by(mut self, part: impl Into<Cow<'a, [u8]>>) -> Self {
        self.payload[1] = part.into();
        self
    }
}

/// The flags requests of `kind` know: `EINVAL` for a kind no request is of.
fn known_flags(kind: u32) -> Result<u32, Errno> {
    Ok(match kind {
        OPEN => OPEN_CREATE | OPEN_FORK_FROM | OPEN_READ_ONLY,
        READ => READ_TRANSFER,
        WRITE => WRITE_BARRIER | WRITE_COMMIT | WRITE_DURABLE | WRITE_TRANSFER | WRITE_ROOT,
        MAP => MAP_ZEROS,
        COMMIT => COMMIT_ROOT | COMMIT_DURABLE,
        FAULT => FAULT_WRITE | FAULT_PAGE | FAULT_PROBE,
        FORK => FORK_QUIESCENT,
        STATS => STATS_GUEST,
        FREE | FREE_MANY | UNMAP | ROOT | INFO | WINDOW | RESIZED | MAPPINGS | ADVANCE | LET_GO
        | PIN | UNPIN | TRANSFER | BUILD | PRIVATE | MAP_MANY => 0,
        _ => return Err(Errno::EINVAL),
    })
}

impl<'a> Request<'a> {
    /// Its kind.
    pub fn kind(&self) -> u32 {
        match self {
            Request::Open { .. } => OPEN,
            Request::Read { .. } => READ,
            Request::Write { .. } => WRITE,
            Request::Free(_) => FREE,
            Request::FreeMany { .. } => FREE_MANY,
            Request::Map(_) => MAP,
            Request::MapMany(_) => MAP_MANY,
            Request::Unmap { .. } => UNMAP,
            Request::Commit { .. } => COMMIT,
            Request::Root => ROOT,
            Request::Info => INFO,
            Request::Window { .. } => WINDOW,
            Request::Fault(_) => FAULT,
            Request::Fork { .. } => FORK,
            Request::Resized { .. } => RESIZED,
            Request::Mappings => MAPPINGS,
            Request::Advance => ADVANCE,
            Request::LetGo { .. } => LET_GO,
            Request::Pin(_) => PIN,
            Request::Unpin(_) => UNPIN,
            Request::Transfer { .. } => TRANSFER,
            Request::Build(_) => BUILD,
            Request::Private { .. } => PRIVATE,
            Request::Stats { .. } => STATS,
        }
    }

    /// The paths it names, of stores: which the daemon authorizes, all of them, before it serves
    /// it.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        let (path, from) = match self {
            Request::Open { path, opening: Opening::ForkFrom { from, .. } } => {
                (Some(path), Some(from))
            }
            Request::Open { path, .. } | Request::Fork { path, .. } => (Some(path), None),
            _ => (None, None),
        };
        path.into_iter().chain(from).map(PathBuf::as_path)
    }

    /// It, as the wire has it.
    pub fn encode(&self) -> Encoded<'_> {
        let message = |flags, args| Encoded::new(self.kind(), flags, args);
        let flag = |set: bool, bit: u32| if set { bit } else { 0 };
        match self {
            Request::Open { path, opening } => {
                let path = path.as_os_str().as_bytes();
                let mut open = match opening {
                    Opening::Existing => message(0, [0; 5]).carrying(path),
                    Opening::Create(capacity) => {
                        message(OPEN_CREATE, [*capacity, 0, 0, 0, 0]).carrying(path)
                    }
                    Opening::ForkFrom { from, branch, capacity } => {
                        let from = [&[0][..], from.as_os_str().as_bytes()].concat();
                        let args = [*capacity, *branch, 0, 0, 0];
                        message(OPEN_FORK_FROM, args).carrying(path).followed_by(from)
                    }
                    Opening::ReadOnly => message(OPEN_READ_ONLY, [0; 5]).carrying(path),
                };
                open.header = with_version(open.header);
                open
            }
            Request::Read { extent, offset, len, into } => match into {
                ReadInto::Reply => message(0, [extent.branch, extent.sequence, *offset, *len, 0]),
                ReadInto::Transfer { at } => {
                    let args = [extent.branch, extent.sequence, *offset, *len, *at];
                    message(READ_TRANSFER, args)
                }
            },
            Request::Write { extent, offset, data, barriers } => {
                let mut flags = flag(barriers.before, WRITE_BARRIER);
                flags |= match barriers.after {
                    None => 0,
                    Some(Commit::Ordered) => WRITE_COMMIT,
                    Some(Commit::Durable) => WRITE_COMMIT | WRITE_DURABLE,
                };
                // fxdax: the root the commit sets, after the data in the payload (TASK-153.13).
                flags |= flag(barriers.root.is_some(), WRITE_ROOT);
                let root = barriers.root.unwrap_or_default();
                let (branch, sequence) = (extent.branch, extent.sequence);
                match data {
                    Data::Inline(data) => {
                        let args = [branch, sequence, *offset, 0, data.len() as u64];
                        message(flags, args).carrying(*data).followed_by(root)
                    }
                    Data::Transfer { at, len } => {
                        let args = [branch, sequence, *offset, *at, *len];
                        message(flags | WRITE_TRANSFER, args).carrying(root)
                    }
                }
            }
            Request::Free(extent) | Request::Pin(extent) | Request::Unpin(extent) => {
                message(0, [extent.branch, extent.sequence, 0, 0, 0])
            }
            Request::FreeMany { first, rest } => {
                let args = [first.branch, first.sequence, 0, 0, 0];
                message(0, args).carrying(extent_ids_payload(rest))
            }
            Request::Map(WindowMapping { window_offset, len, contents }) => match *contents {
                MappingContents::Extent { extent, offset } => {
                    message(0, [extent.branch, extent.sequence, offset, *len, *window_offset])
                }
                MappingContents::Zeros => message(MAP_ZEROS, [0, 0, 0, *len, *window_offset]),
            },
            Request::MapMany(mappings) => message(0, [0; 5]).carrying(mappings_payload(mappings)),
            Request::Unmap { window_offset, len } => message(0, [*window_offset, *len, 0, 0, 0]),
            Request::Commit { commit, root } => {
                let flags = flag(root.is_some(), COMMIT_ROOT)
                    | flag(*commit == Commit::Durable, COMMIT_DURABLE);
                message(flags, [0; 5]).carrying(root.unwrap_or_default())
            }
            Request::Root | Request::Info | Request::Mappings | Request::Advance => {
                message(0, [0; 5])
            }
            Request::Window { size, private_max, budget } => {
                message(0, [*size, private_max.unwrap_or(0), *budget, 0, 0])
            }
            Request::Fault(Fault { offset, access, page, probe }) => {
                let flags = flag(*access == Access::Write, FAULT_WRITE)
                    | flag(*page, FAULT_PAGE)
                    | flag(*probe, FAULT_PROBE);
                message(flags, [*offset, 0, 0, 0, 0])
            }
            Request::Fork { path, branch, quiescent } => {
                let flags = flag(*quiescent, FORK_QUIESCENT);
                message(flags, [*branch, 0, 0, 0, 0]).carrying(path.as_os_str().as_bytes())
            }
            Request::Resized { capacity, took } => {
                message(0, [*capacity, u64::from(*took), 0, 0, 0])
            }
            Request::LetGo { epoch } => message(0, [*epoch, 0, 0, 0, 0]),
            Request::Transfer { len, area } => {
                Encoded { fd: Some(area.0), ..message(0, [*len, 0, 0, 0, 0]) }
            }
            Request::Build(commit) => message(0, [0; 5]).carrying(commit.as_bytes()),
            Request::Private { window_offset, len } => message(0, [*window_offset, *len, 0, 0, 0]),
            Request::Stats { ask, guest } => {
                let args = [*ask, 0, 0, 0, 0];
                message(flag(guest.is_some(), STATS_GUEST), args)
                    .carrying(guest.unwrap_or_default().as_bytes())
            }
        }
    }

    /// The request `message` is: `EINVAL` unless it is one its kind allows; fxdax:
    /// `EPROTONOSUPPORT` for an [`OPEN`] of another wire version, whatever else it holds
    /// (TASK-141.3).
    pub fn decode(message: &'a Message) -> Result<Self, Errno> {
        let Message { header, fds, .. } = message;
        let Header { kind, flags, args, status, .. } = *header;
        if kind == OPEN && version(header) != VERSION {
            return Err(Errno::EPROTONOSUPPORT);
        }
        if status != 0 || flags & !(known_flags(kind)? | REQUEST) != 0 {
            return Err(Errno::EINVAL);
        }
        // fxdax: what its kind has, after the guest request it says, if any (TASK-260).
        let (_, payload) = guest_request(message)?;
        let flags = flags & !REQUEST;
        // A request's descriptors: a TRANSFER's one, and none of any other.
        let fd = match (kind, &fds[..]) {
            (TRANSFER, [fd]) => Some(fd.as_fd()),
            (_, []) if kind != TRANSFER => None,
            _ => return Err(Errno::EINVAL),
        };
        let has = |flag: u32| flags & flag != 0;
        // The payload of a request that carries none.
        let nothing = || if payload.is_empty() { Ok(()) } else { Err(Errno::EINVAL) };
        let extent = ExtentId { branch: args[0], sequence: args[1] };
        Ok(match kind {
            OPEN => {
                let (named, opening) = match flags {
                    0 => (&payload[..], Opening::Existing),
                    OPEN_CREATE => (&payload[..], Opening::Create(args[0])),
                    OPEN_READ_ONLY => (&payload[..], Opening::ReadOnly),
                    OPEN_FORK_FROM => {
                        let nul = payload.iter().position(|byte| *byte == 0);
                        let nul = nul.ok_or(Errno::EINVAL)?;
                        let from = path(&payload[nul + 1..])?;
                        let (branch, capacity) = (args[1], args[0]);
                        (&payload[..nul], Opening::ForkFrom { from, branch, capacity })
                    }
                    // Two ways of opening at once.
                    _ => return Err(Errno::EINVAL),
                };
                Request::Open { path: path(named)?, opening }
            }
            READ => {
                nothing()?;
                let len = args[3];
                let into = match has(READ_TRANSFER) {
                    true => ReadInto::Transfer { at: args[4] },
                    // fxdax: a reply holds no more (TASK-79.16).
                    false if len > MAX_PAYLOAD => return Err(Errno::EINVAL),
                    false => ReadInto::Reply,
                };
                Request::Read { extent, offset: args[2], len, into }
            }
            WRITE => {
                // A commit's flags, without a commit.
                if !has(WRITE_COMMIT) && has(WRITE_DURABLE | WRITE_ROOT) {
                    return Err(Errno::EINVAL);
                }
                // The data, and the root after it in the payload, if any.
                let (data, root) = match has(WRITE_TRANSFER) {
                    true => (Data::Transfer { at: args[3], len: args[4] }, &payload[..]),
                    false if has(WRITE_ROOT) => {
                        let len = usize::try_from(args[4]).map_err(|_| Errno::EINVAL)?;
                        let data = payload.get(..len).ok_or(Errno::EINVAL)?;
                        (Data::Inline(data), &payload[len..])
                    }
                    false => (Data::Inline(&payload[..]), &[][..]),
                };
                if (!has(WRITE_ROOT) && !root.is_empty()) || root.len() > MAX_ROOT {
                    return Err(Errno::EINVAL);
                }
                let after = has(WRITE_COMMIT).then_some(if has(WRITE_DURABLE) {
                    Commit::Durable
                } else {
                    Commit::Ordered
                });
                let root = has(WRITE_ROOT).then_some(root);
                let barriers = WriteBarriers { before: has(WRITE_BARRIER), after, root };
                Request::Write { extent, offset: args[2], data, barriers }
            }
            FREE => {
                nothing()?;
                Request::Free(extent)
            }
            FREE_MANY => {
                let rest: Cow<'a, [ExtentId]> = Cow::Owned(extent_ids(payload)?.collect());
                Request::FreeMany { first: extent, rest }
            }
            MAP => {
                nothing()?;
                let contents = match has(MAP_ZEROS) {
                    false => MappingContents::Extent { extent, offset: args[2] },
                    true => MappingContents::Zeros,
                };
                Request::Map(WindowMapping { window_offset: args[4], len: args[3], contents })
            }
            // fxdax: one mapping at least (TASK-92.14).
            MAP_MANY => match mappings(payload)? {
                mappings if mappings.is_empty() => return Err(Errno::EINVAL),
                mappings => Request::MapMany(Cow::Owned(mappings)),
            },
            UNMAP => {
                nothing()?;
                Request::Unmap { window_offset: args[0], len: args[1] }
            }
            COMMIT => {
                let root = has(COMMIT_ROOT).then_some(&payload[..]);
                if root.is_none() {
                    nothing()?;
                }
                // fxdax: no more than a root holds (TASK-149.17).
                if root.is_some_and(|root| root.len() > MAX_ROOT) {
                    return Err(Errno::EINVAL);
                }
                let commit = if has(COMMIT_DURABLE) { Commit::Durable } else { Commit::Ordered };
                Request::Commit { commit, root }
            }
            ROOT | INFO | MAPPINGS | ADVANCE => {
                nothing()?;
                match kind {
                    ROOT => Request::Root,
                    INFO => Request::Info,
                    MAPPINGS => Request::Mappings,
                    _ => Request::Advance,
                }
            }
            WINDOW => {
                nothing()?;
                // fxdax: past it, the daemon's room for its VM process's replies (TASK-150.3).
                if args[0] > MAX_WINDOW {
                    return Err(Errno::EINVAL);
                }
                // fxdax: some private pages, no more than the window's, or none (backlog doc-13).
                let private_max = (args[1] != 0).then_some(args[1]);
                if args[1] > args[0] / crate::pages::page_size() {
                    return Err(Errno::EINVAL);
                }
                Request::Window { size: args[0], private_max, budget: args[2] }
            }
            FAULT => {
                nothing()?;
                let access = if has(FAULT_WRITE) { Access::Write } else { Access::Read };
                // fxdax: a probe is of a page alone, to read.
                let probe = has(FAULT_PROBE);
                if probe && (access == Access::Write || !has(FAULT_PAGE)) {
                    return Err(Errno::EINVAL);
                }
                Request::Fault(Fault { offset: args[0], access, page: has(FAULT_PAGE), probe })
            }
            FORK => {
                let quiescent = has(FORK_QUIESCENT);
                Request::Fork { path: path(payload)?, branch: args[0], quiescent }
            }
            RESIZED => {
                nothing()?;
                let took = match args[1] {
                    0 => false,
                    1 => true,
                    _ => return Err(Errno::EINVAL),
                };
                Request::Resized { capacity: args[0], took }
            }
            LET_GO => {
                nothing()?;
                Request::LetGo { epoch: args[0] }
            }
            PIN | UNPIN => {
                nothing()?;
                if kind == PIN { Request::Pin(extent) } else { Request::Unpin(extent) }
            }
            TRANSFER => {
                nothing()?;
                Request::Transfer { len: args[0], area: Area(fd.ok_or(Errno::EINVAL)?) }
            }
            BUILD => Request::Build(build(payload)?),
            PRIVATE => {
                nothing()?;
                Request::Private { window_offset: args[0], len: args[1] }
            }
            STATS => {
                let guest = match has(STATS_GUEST) {
                    true => Some(stats_text(payload)?),
                    false => {
                        nothing()?;
                        None
                    }
                };
                Request::Stats { ask: args[0], guest }
            }
            _ => return Err(Errno::EINVAL),
        })
    }
}

/// fxdax: the bytes of data the request `header` is of moves through the stream: its payload, and,
/// a `READ`'s, its reply's: [`MAX_PAYLOAD`] at most, as one asking for more is refused (`EINVAL`),
/// its reply's payload empty, whatever the VM process asks for: so a word that would overflow the
/// bytes counted is counted as no more (TASK-79.16). Of the header alone, as the daemon admits a
/// request before it reads its payload, and decodes it.
pub(crate) fn stream_bytes(header: &Header) -> u64 {
    match header.kind {
        READ if header.flags & READ_TRANSFER == 0 => header.len + header.args[3].min(MAX_PAYLOAD),
        _ => header.len,
    }
}

/// fxdax: the guest request the request `message` is a call for, if it says one ([`REQUEST`]),
/// and the rest of its payload, its kind's: `EINVAL` if it says one it doesn't hold, or 0
/// (TASK-260).
pub(crate) fn guest_request(message: &Message) -> Result<(Option<NonZeroU64>, &[u8]), Errno> {
    if message.header.flags & REQUEST == 0 {
        return Ok((None, &message.payload));
    }
    let (guest, rest) =
        message.payload.split_first_chunk::<REQUEST_BYTES>().ok_or(Errno::EINVAL)?;
    let guest = NonZeroU64::new(u64::from_le_bytes(*guest)).ok_or(Errno::EINVAL)?;
    Ok((Some(guest), rest))
}

/// fxdax: the bytes a [`REQUEST`] says the guest request `guest` in (TASK-260).
pub(crate) fn request_bytes(guest: NonZeroU64) -> [u8; REQUEST_BYTES] {
    guest.get().to_le_bytes()
}

/// The path `bytes` hold: `EINVAL` if empty, or holding a NUL, which no path does.
fn path(bytes: &[u8]) -> Result<PathBuf, Errno> {
    if bytes.is_empty() || bytes.contains(&0) {
        return Err(Errno::EINVAL);
    }
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

/// fxdax: the commit a [`BUILD`]'s payload says: `EINVAL` unless a label's characters, as many as
/// [`MAX_BUILD`] at most (TASK-111.7).
fn build(payload: &[u8]) -> Result<&str, Errno> {
    let commit = std::str::from_utf8(payload).map_err(|_| Errno::EINVAL)?;
    let label = |byte: u8| byte.is_ascii_alphanumeric() || b"._-".contains(&byte);
    if commit.is_empty() || commit.len() > MAX_BUILD || !commit.bytes().all(label) {
        return Err(Errno::EINVAL);
    }
    Ok(commit)
}

/// fxdax: the guest's stats a [`STATS`]'s payload holds: `EINVAL` unless UTF-8, [`MAX_STATS`]
/// bytes at most (TASK-111.4).
fn stats_text(payload: &[u8]) -> Result<&str, Errno> {
    if payload.len() > MAX_STATS {
        return Err(Errno::EINVAL);
    }
    std::str::from_utf8(payload).map_err(|_| Errno::EINVAL)
}

/// fxdax: `extents`, as a [`FREE_MANY`]'s payload names them.
fn extent_ids_payload(extents: &[ExtentId]) -> Vec<u8> {
    (extents.iter())
        .flat_map(|extent| [extent.branch, extent.sequence])
        .flat_map(u64::to_le_bytes)
        .collect()
}

/// fxdax: the extents a [`FREE_MANY`]'s payload names: `EINVAL` unless it is whole IDs.
fn extent_ids(payload: &[u8]) -> Result<impl Iterator<Item = ExtentId> + '_, Errno> {
    let (ids, []) = payload.as_chunks::<EXTENT_ID_BYTES>() else {
        return Err(Errno::EINVAL);
    };
    Ok(ids.iter().map(|id| ExtentId { branch: word(&id[..8]), sequence: word(&id[8..]) }))
}

/// The little-endian word `bytes`, eight of them, hold.
fn word(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("a word's bytes"))
}

/// What a VM process says of the stage operations it made, which nothing replies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Made {
    /// [`ACK`]: it made stage operation `id`, and those before it; and its second stage is as
    /// `counts` says, as [`STAGE_COUNTS`] says it.
    Ack { id: u64, counts: StageCounts },
    /// fxdax: [`STAGE_COUNTS`] (TASK-91.16).
    Counts(StageCounts),
    /// fxdax: [`STAGE_PRIVATE_COUNTS`] (TASK-164.10).
    PrivateCounts(PrivateCosts),
}

/// fxdax: what a VM process says of its window's second stage, as [`STAGE_COUNTS`] says it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StageCounts {
    /// What its stage operations cost it so far, counts that only grow: how many, their
    /// nanoseconds, and those that took access away from pages it had installed, by how: installed
    /// over, left out (TASK-91.16).
    pub costs: [u64; 4],
    /// The runs of pages it has installed now, each a mapping of its own (TASK-111.5).
    pub installed: u64,
}

impl StageCounts {
    fn args(&self) -> [u64; 5] {
        let [ops, ns, overwrites, leave_outs] = self.costs;
        [ops, ns, overwrites, leave_outs, self.installed]
    }

    fn from_args(args: [u64; 5]) -> Self {
        let [ops, ns, overwrites, leave_outs, installed] = args;
        Self { costs: [ops, ns, overwrites, leave_outs], installed }
    }
}

/// fxdax: what a VM process's runs of private pages cost it so far, as [`STAGE_PRIVATE_COUNTS`]
/// says it: how many, their pages, their nanoseconds; and its `PRIVATE` requests, and the
/// nanoseconds it waited for their replies.
pub(crate) type PrivateCosts = [u64; 5];

impl Made {
    pub fn encode(&self) -> Header {
        match *self {
            Made::Ack { id, counts } => Header::new(ACK, id).with_args(&counts.args()),
            Made::Counts(counts) => Header::new(STAGE_COUNTS, 0).with_args(&counts.args()),
            Made::PrivateCounts(counts) => Header::new(STAGE_PRIVATE_COUNTS, 0).with_args(&counts),
        }
    }

    /// What `message` says: `EINVAL` unless it is an [`ACK`], a [`STAGE_COUNTS`] or a
    /// [`STAGE_PRIVATE_COUNTS`], of no flags, payload or descriptors.
    pub fn decode(message: &Message) -> Result<Self, Errno> {
        let Header { kind, flags, id, args, status, .. } = message.header;
        if flags != 0 || status != 0 || !message.payload.is_empty() || !message.fds.is_empty() {
            return Err(Errno::EINVAL);
        }
        let counts = StageCounts::from_args(args);
        match kind {
            ACK => Ok(Made::Ack { id, counts }),
            STAGE_COUNTS => Ok(Made::Counts(counts)),
            STAGE_PRIVATE_COUNTS => Ok(Made::PrivateCounts(args)),
            _ => Err(Errno::EINVAL),
        }
    }
}

/// What the daemon tells a VM process unasked: none of it carries a payload or descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Told {
    /// A stage operation, which the VM process makes of its window.
    Stage(Stage),
    /// [`CAPACITY`]: the store's capacity is `capacity` bytes now; or, if `ask`, the guest is asked
    /// to shrink to it ([`CAPACITY_ASK`]).
    Capacity { capacity: u64, ask: bool },
    /// fxdax: [`COMMITTED`].
    Committed { epoch: u64 },
    /// fxdax: [`STATS_ASK`].
    Stats { ask: u64 },
}

impl Told {
    pub fn encode(&self) -> Header {
        match *self {
            Told::Stage(stage) => stage.encode(),
            Told::Capacity { capacity, ask } => (Header::new(CAPACITY, 0).with_args(&[capacity]))
                .with_flags(if ask { CAPACITY_ASK } else { 0 }),
            Told::Committed { epoch } => Header::new(COMMITTED, 0).with_args(&[epoch]),
            Told::Stats { ask } => Header::new(STATS_ASK, 0).with_args(&[ask]),
        }
    }

    /// What the message of `header` says: `EINVAL` unless one of these, as its kind allows.
    pub fn decode(header: &Header) -> Result<Self, Errno> {
        let Header { kind, flags, args, status, fds, len, .. } = *header;
        if status != 0 || fds != 0 || len != 0 {
            return Err(Errno::EINVAL);
        }
        let known = |known: u32| if flags & !known == 0 { Ok(()) } else { Err(Errno::EINVAL) };
        match kind {
            CAPACITY => {
                known(CAPACITY_ASK)?;
                Ok(Told::Capacity { capacity: args[0], ask: flags & CAPACITY_ASK != 0 })
            }
            COMMITTED => known(0).map(|()| Told::Committed { epoch: args[0] }),
            STATS_ASK => known(0).map(|()| Told::Stats { ask: args[0] }),
            _ => Stage::decode(header).map(Told::Stage),
        }
    }
}

/// A stage operation, of the daemon's window's second stage in a VM process's memory, which the
/// VM process makes, and acknowledges ([`ACK`]) if it takes access to pages away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stage {
    /// Its ID, which an acknowledgement names.
    pub id: u64,
    pub op: StageOp,
}

/// What a [`Stage`] operation does, to the window's bytes at `offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StageOp {
    /// [`STAGE_MAP`]: maps `len` bytes of physical memory at `phys`, read-only.
    Map { offset: u64, phys: PhysLocation, len: u64 },
    /// [`STAGE_UNMAP`].
    Unmap { offset: u64, len: u64 },
    /// fxdax: [`STAGE_MAPPED`].
    Mapped { offset: u64, len: u64 },
    /// fxdax: [`STAGE_PRIVATE`]: the page at `offset` private, holding the page's at `from`, or
    /// zeros.
    Private { offset: u64, from: Option<PhysLocation> },
}

impl StageOp {
    /// Whether it takes access to pages away, which its VM process acknowledges ([`STAGE_ACK`]).
    pub fn taking(&self) -> bool {
        match self {
            // fxdax: a private page takes the page mapped there away, and copies the page `from`,
            // which the daemon keeps until it is made.
            StageOp::Unmap { .. } | StageOp::Private { .. } => true,
            StageOp::Map { .. } | StageOp::Mapped { .. } => false,
        }
    }
}

impl Stage {
    pub fn encode(&self) -> Header {
        let (kind, args) = match self.op {
            StageOp::Map { offset, phys, len } => {
                (STAGE_MAP, [offset, phys.offset, len, 0, phys.arena.0.into()])
            }
            StageOp::Unmap { offset, len } => (STAGE_UNMAP, [offset, len, 0, 0, 0]),
            StageOp::Mapped { offset, len } => (STAGE_MAPPED, [offset, len, 0, 0, 0]),
            StageOp::Private { offset, from: None } => (STAGE_PRIVATE, [offset, 0, 0, 0, 0]),
            StageOp::Private { offset, from: Some(from) } => {
                (STAGE_PRIVATE, [offset, from.offset, 1, 0, from.arena.0.into()])
            }
        };
        let flags = if self.op.taking() { STAGE_ACK } else { 0 };
        Header { kind, flags, id: self.id, args, ..Default::default() }
    }

    /// The stage operation the message of `header` is: `EINVAL` unless one, acknowledged as it
    /// takes access away or not, of no payload or descriptors.
    pub fn decode(header: &Header) -> Result<Self, Errno> {
        let Header { kind, flags, id, args, status, fds, len } = *header;
        if status != 0 || fds != 0 || len != 0 {
            return Err(Errno::EINVAL);
        }
        let op = match kind {
            STAGE_MAP => {
                let arena = ArenaId(u32::try_from(args[4]).map_err(|_| Errno::EINVAL)?);
                let phys = PhysLocation { arena, offset: args[1] };
                StageOp::Map { offset: args[0], phys, len: args[2] }
            }
            STAGE_UNMAP => StageOp::Unmap { offset: args[0], len: args[1] },
            STAGE_MAPPED => StageOp::Mapped { offset: args[0], len: args[1] },
            STAGE_PRIVATE => {
                let from = match args[2] {
                    0 => None,
                    1 => {
                        let arena = ArenaId(u32::try_from(args[4]).map_err(|_| Errno::EINVAL)?);
                        Some(PhysLocation { arena, offset: args[1] })
                    }
                    _ => return Err(Errno::EINVAL),
                };
                StageOp::Private { offset: args[0], from }
            }
            _ => return Err(Errno::EINVAL),
        };
        let acknowledged = if op.taking() { STAGE_ACK } else { 0 };
        if flags != acknowledged {
            return Err(Errno::EINVAL);
        }
        Ok(Stage { id, op })
    }
}

/// fxdax: where an [`OPEN`] and its reply say their sender's wire [`VERSION`] (TASK-141.3).
const VERSION_ARG: usize = 4;

/// fxdax: the wire version the [`OPEN`], or the reply to one, of `header` says: its sender's
/// (TASK-141.3), whatever the rest of it holds, as no version moves where it is said.
pub(crate) fn version(header: &Header) -> u64 {
    header.args[VERSION_ARG]
}

/// fxdax: `header`, an [`OPEN`] or a reply to one, saying this end's wire [`VERSION`]: as a
/// daemon's reply refusing an `OPEN` of another version does too (TASK-141.3).
pub(crate) fn with_version(mut header: Header) -> Header {
    header.args[VERSION_ARG] = VERSION;
    header
}

/// What a reply to an [`OPEN`] says, with the physical memory it carries (TASK-43, TASK-79.17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Opened {
    /// The store's branch.
    pub branch: u64,
    /// The arena of the memory it carries first: the VM process's tenant's.
    pub arena: ArenaId,
    /// fxdax: a follower's view's commit's epoch; 0 of a store opened to write.
    pub epoch: u64,
    /// fxdax: how many mappings the host lets a process make, if it limits them
    /// (`physmem::max_map_count`), which the VM process's window budgets its own by: the daemon
    /// reads it for the VM process, which can't in a jail without /proc (TASK-172). 0 if it
    /// doesn't limit them.
    pub max_map_count: Option<u64>,
}

impl Opened {
    /// Its arguments, with this daemon's wire [`VERSION`], last ([`VERSION_ARG`]).
    pub fn args(&self) -> [u64; 5] {
        [self.branch, self.arena.0.into(), self.epoch, self.max_map_count.unwrap_or(0), VERSION]
    }

    /// What a reply of `args` says: `EINVAL` of an arena that is none; fxdax: `EPROTONOSUPPORT`
    /// from a daemon of another wire version, before anything else of it (TASK-141.3).
    pub fn from_args(args: [u64; 5]) -> Result<Self, Errno> {
        if args[VERSION_ARG] != VERSION {
            return Err(Errno::EPROTONOSUPPORT);
        }
        let arena = ArenaId(u32::try_from(args[1]).map_err(|_| Errno::EINVAL)?);
        let max_map_count = (args[3] != 0).then_some(args[3]);
        Ok(Self { branch: args[0], arena, epoch: args[2], max_map_count })
    }
}

/// What a reply to an [`INFO`] says: the store's [`ExtentInfo`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Info(pub ExtentInfo);

impl Info {
    pub fn args(&self) -> [u64; 5] {
        let ExtentInfo { capacity, used, available, next_sequence, sequence_floor } = self.0;
        [capacity, used, available, next_sequence, sequence_floor]
    }

    pub fn from_args(args: [u64; 5]) -> Self {
        let [capacity, used, available, next_sequence, sequence_floor] = args;
        Self(ExtentInfo { capacity, used, available, next_sequence, sequence_floor })
    }
}

/// fxdax: what a reply to an [`ADVANCE`] says: the epoch of the commit the view moved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Advanced {
    pub epoch: u64,
}

impl Advanced {
    pub fn args(&self) -> [u64; 5] {
        [self.epoch, 0, 0, 0, 0]
    }

    pub fn from_args(args: [u64; 5]) -> Self {
        Self { epoch: args[0] }
    }
}

/// `mappings`, as a reply to [`MAPPINGS`], or a [`MAP_MANY`], carries them: each
/// [`MAPPING_BYTES`].
pub(crate) fn mappings_payload(mappings: &[WindowMapping]) -> Vec<u8> {
    (mappings.iter())
        .flat_map(|mapping| {
            let words = match mapping.contents {
                MappingContents::Extent { extent, offset } => {
                    [extent.branch, extent.sequence, offset, 0]
                }
                MappingContents::Zeros => [0, 0, 0, MAPPING_ZEROS],
            };
            [mapping.window_offset, mapping.len].into_iter().chain(words)
        })
        .flat_map(u64::to_le_bytes)
        .collect()
}

/// The mappings a reply to [`MAPPINGS`], or a [`MAP_MANY`], carries, `payload`: `EINVAL` unless
/// whole ones, of flags known.
pub(crate) fn mappings(payload: &[u8]) -> Result<Vec<WindowMapping>, Errno> {
    let (mappings, []) = payload.as_chunks::<{ MAPPING_BYTES as usize }>() else {
        return Err(Errno::EINVAL);
    };
    (mappings.iter())
        .map(|mapping| {
            let [window_offset, len, branch, sequence, offset, flags] =
                std::array::from_fn(|at| word(&mapping[8 * at..8 * (at + 1)]));
            let contents = match flags {
                0 => MappingContents::Extent { extent: ExtentId { branch, sequence }, offset },
                MAPPING_ZEROS => MappingContents::Zeros,
                _ => return Err(Errno::EINVAL),
            };
            Ok(WindowMapping { window_offset, len, contents })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::os::fd::OwnedFd;

    /// What one of each shape of request is made of.
    #[derive(Clone, Debug)]
    struct Fields {
        words: [u64; 5],
        flags: [bool; 3],
        data: Vec<u8>,
        root: Vec<u8>,
        paths: [PathBuf; 2],
        extents: Vec<ExtentId>,
        build: String,
        /// fxdax: a guest's stats (TASK-111.4).
        stats: String,
        commit: Commit,
        access: Access,
    }

    fn fields() -> impl Strategy<Value = Fields> {
        let extent = (any::<u64>(), any::<u64>())
            .prop_map(|(branch, sequence)| ExtentId { branch, sequence });
        let path = || "[a-z0-9/._-]{1,24}".prop_map(PathBuf::from);
        let bytes = || proptest::collection::vec(any::<u8>(), 0..64);
        (
            (any::<[u64; 5]>(), any::<[bool; 3]>(), bytes(), bytes()),
            ([path(), path()], proptest::collection::vec(extent, 0..8)),
            ("[a-zA-Z0-9._-]{1,64}", "(\\PC{0,16}\n){0,8}"),
            prop_oneof![Just(Commit::Ordered), Just(Commit::Durable)],
            prop_oneof![Just(Access::Read), Just(Access::Write)],
        )
            .prop_map(
                |((words, flags, data, root), (paths, extents), (build, stats), commit, access)| {
                    Fields {
                        words,
                        flags,
                        data,
                        root,
                        paths,
                        extents,
                        build,
                        stats,
                        commit,
                        access,
                    }
                },
            )
    }

    /// A request of every shape there is, of `fields`, with `area` as a transfer area: each kind,
    /// each of its ways.
    fn requests<'a>(fields: &'a Fields, area: BorrowedFd<'a>) -> Vec<Request<'a>> {
        let Fields { words: [a, b, c, d, e], flags: [one, two, three], .. } = *fields;
        let extent = ExtentId { branch: a, sequence: b };
        let other = ExtentId { branch: c, sequence: d };
        let [path, from] = fields.paths.clone();
        let commit = fields.commit;
        let root = (one && two).then_some(&fields.root[..]);
        let barriers = WriteBarriers { before: three, after: two.then_some(commit), root };
        let read = |len, into| Request::Read { extent, offset: c, len, into };
        let write = |data| Request::Write { extent, offset: c, data, barriers };
        let map = |contents| Request::Map(WindowMapping { window_offset: d, len: e, contents });
        let open = |opening| Request::Open { path: path.clone(), opening };
        vec![
            open(Opening::Existing),
            open(Opening::Create(a)),
            open(Opening::ForkFrom { from, branch: b, capacity: a }),
            open(Opening::ReadOnly),
            read(d % (MAX_PAYLOAD + 1), ReadInto::Reply),
            read(d, ReadInto::Transfer { at: e }),
            write(Data::Inline(&fields.data[..])),
            write(Data::Transfer { at: d, len: e }),
            Request::Free(extent),
            Request::FreeMany { first: extent, rest: Cow::Borrowed(&fields.extents[..]) },
            map(MappingContents::Extent { extent: other, offset: a }),
            map(MappingContents::Zeros),
            Request::MapMany(Cow::Owned(vec![
                WindowMapping {
                    window_offset: d,
                    len: e,
                    contents: MappingContents::Extent { extent: other, offset: a },
                },
                WindowMapping { window_offset: a, len: b, contents: MappingContents::Zeros },
            ])),
            Request::Unmap { window_offset: a, len: b },
            Request::Commit { commit, root: one.then_some(&fields.data[..]) },
            Request::Root,
            Request::Info,
            Request::Window { size: a % (MAX_WINDOW + 1), private_max: None, budget: c },
            Request::Window {
                size: MAX_WINDOW,
                private_max: Some(1 + b % (MAX_WINDOW / crate::pages::page_size())),
                budget: d,
            },
            Request::Fault(Fault { offset: a, access: fields.access, page: one, probe: false }),
            Request::Fault(Fault { offset: a, access: Access::Read, page: true, probe: true }),
            Request::Fork { path: path.clone(), branch: b, quiescent: one },
            Request::Resized { capacity: a, took: one },
            Request::Mappings,
            Request::Advance,
            Request::LetGo { epoch: a },
            Request::Pin(extent),
            Request::Unpin(other),
            Request::Transfer { len: a, area: Area(area) },
            Request::Build(&fields.build),
            Request::Private { window_offset: a, len: b },
            Request::Stats { ask: a, guest: None },
            Request::Stats { ask: b, guest: Some(&fields.stats) },
        ]
    }

    /// Every kind of request.
    const KINDS: [u32; 24] = [
        OPEN, READ, WRITE, FREE, MAP, UNMAP, COMMIT, ROOT, INFO, WINDOW, FAULT, FORK, RESIZED,
        MAPPINGS, ADVANCE, LET_GO, PIN, UNPIN, TRANSFER, FREE_MANY, BUILD, PRIVATE, STATS,
        MAP_MANY,
    ];

    /// `request`, as its receiver receives it: of ID `id`.
    fn sent(request: &Request<'_>, id: u64) -> Message {
        let Encoded { mut header, payload, fd } = request.encode();
        let payload: Vec<u8> = payload.concat();
        header.id = id;
        header.len = payload.len() as u64;
        header.fds = u32::from(fd.is_some());
        let fds = fd.map(|fd| fd.try_clone_to_owned().unwrap()).into_iter().collect();
        Message { header, payload, fds }
    }

    /// fxdax TASK-260: every request decodes as it is whether or not it says its guest request,
    /// which [`guest_request`] reads; one that says it and doesn't hold it, or says 0, is refused.
    #[test]
    fn requests_say_their_guest_request() {
        let fields = Fields {
            words: [3, 5, 4096, 8192, 1 << 20],
            flags: [true, false, false],
            data: (0..64).collect(),
            root: b"root".to_vec(),
            paths: [PathBuf::from("store"), PathBuf::from("from")],
            extents: vec![ExtentId { branch: 7, sequence: 9 }],
            build: "build".into(),
            stats: "stats".into(),
            commit: Commit::Ordered,
            access: Access::Read,
        };
        let area = area();
        let guest = NonZeroU64::new(0x1234_5678_9abc).unwrap();
        for request in requests(&fields, area.as_fd()) {
            let plain = sent(&request, 1);
            assert_eq!(guest_request(&plain).map(|(guest, _)| guest), Ok(None));
            let mut saying = sent(&request, 1);
            saying.header.flags |= REQUEST;
            saying.payload.splice(0..0, request_bytes(guest));
            saying.header.len += REQUEST_BYTES as u64;
            assert_eq!(guest_request(&saying).map(|(guest, _)| guest), Ok(Some(guest)));
            assert_eq!(Request::decode(&saying).as_ref(), Ok(&request), "{request:?}");
            let mut zero = sent(&request, 1);
            zero.header.flags |= REQUEST;
            zero.payload.splice(0..0, [0; REQUEST_BYTES]);
            assert_eq!(Request::decode(&zero).err(), Some(Errno::EINVAL), "{request:?}");
        }
        let mut short = sent(&Request::Info, 1);
        short.header.flags |= REQUEST;
        short.payload = vec![1; REQUEST_BYTES - 1];
        assert_eq!(Request::decode(&short).err(), Some(Errno::EINVAL));
    }

    /// A transfer area's descriptor, for requests to carry.
    fn area() -> OwnedFd {
        tempfile::tempfile().unwrap().into()
    }

    /// TASK-160.9: each kind of request, each of its ways, decodes as what was encoded, whatever
    /// its fields; and nothing but those kinds are requests.
    #[test]
    fn requests_decode_as_encoded() {
        let area = area();
        proptest!(|(drawn in fields(), id in any::<u64>())| {
            let requests = requests(&drawn, area.as_fd());
            let kinds: std::collections::BTreeSet<u32> =
                requests.iter().map(Request::kind).collect();
            let all: std::collections::BTreeSet<u32> = KINDS.into_iter().collect();
            prop_assert_eq!(kinds, all);
            for request in requests {
                let message = sent(&request, id);
                prop_assert_eq!(message.header.kind, request.kind());
                prop_assert_eq!(Request::decode(&message), Ok(request));
            }
        });
        for kind in (0..256).filter(|kind| !KINDS.contains(kind)) {
            let message = Message { header: Header::new(kind, 1), payload: vec![], fds: vec![] };
            assert_eq!(Request::decode(&message).err(), Some(Errno::EINVAL), "kind {kind}");
        }
    }

    /// TASK-160.9: a request with a flag its kind doesn't know, any of them, is refused, as is one
    /// with flags it knows that mean nothing together. Before, the daemon ignored them: a `WRITE`
    /// of a flag it didn't know was written as though it had none.
    #[test]
    fn requests_with_flags_their_kinds_dont_know_are_refused() {
        // What each kind knows, as doc-4 and the wire's docs have it.
        let known = |kind: u32| -> u32 {
            match kind {
                OPEN => 0b111,
                READ => 0b1,
                WRITE => 0b1_1111,
                // fxdax: zeros; bit 0, writably, writable DAX's, is gone (backlog doc-13).
                MAP => 0b10,
                COMMIT => 0b11,
                FAULT => 0b11,
                FORK => 0b1,
                // fxdax: the guest's (TASK-111.4).
                STATS => 0b1,
                _ => 0,
            }
        };
        let fields = Fields {
            words: [1, 2, 4096, 4096, 8192],
            flags: [false; 3],
            data: vec![7; 4096],
            root: Vec::new(),
            paths: [PathBuf::from("store"), PathBuf::from("from")],
            extents: vec![ExtentId { branch: 1, sequence: 3 }],
            build: "abc".into(),
            stats: "seals 1\n".into(),
            commit: Commit::Ordered,
            access: Access::Read,
        };
        let area = area();
        let requests = requests(&fields, area.as_fd());
        for kind in KINDS {
            let request = requests.iter().find(|request| request.kind() == kind).unwrap();
            let mut message = sent(request, 1);
            assert!(Request::decode(&message).is_ok(), "{request:?}");
            // fxdax: REQUEST, which any request may have, aside (TASK-260).
            let unknown = |bit: &u32| (known(kind) | REQUEST) & *bit == 0;
            for bit in (0..32).map(|bit| 1u32 << bit).filter(unknown) {
                message.header.flags = bit;
                let decoded = Request::decode(&message);
                assert_eq!(decoded.err(), Some(Errno::EINVAL), "kind {kind}, flag {bit:#x}");
            }
        }
        // Flags known, that mean nothing together.
        let refused = |kind, flags, payload: &[u8]| {
            let header = with_version(Header::new(kind, 1).with_flags(flags));
            let message = Message { header, payload: payload.to_vec(), fds: vec![] };
            assert_eq!(Request::decode(&message).err(), Some(Errno::EINVAL), "{kind}, {flags:#x}");
        };
        refused(OPEN, OPEN_CREATE | OPEN_FORK_FROM, b"store\0from");
        refused(OPEN, OPEN_CREATE | OPEN_READ_ONLY, b"store");
        refused(OPEN, OPEN_FORK_FROM | OPEN_READ_ONLY, b"store\0from");
        refused(WRITE, WRITE_DURABLE, b"data");
        refused(WRITE, WRITE_ROOT, b"data");
        // fxdax: a probe is of a page alone, to read.
        refused(FAULT, FAULT_PROBE, b"");
        refused(FAULT, FAULT_PROBE | FAULT_PAGE | FAULT_WRITE, b"");
        // What the daemon tells, and VM processes say of their stage operations, the same: a
        // stage operation is acknowledged if, and only if, it takes access away.
        let phys = PhysLocation { arena: ArenaId::DEFAULT, offset: 0 };
        let ops = [
            StageOp::Map { offset: 0, phys, len: 4096 },
            StageOp::Unmap { offset: 0, len: 4096 },
            StageOp::Mapped { offset: 0, len: 4096 },
            StageOp::Private { offset: 0, from: Some(phys) },
            StageOp::Private { offset: 0, from: None },
        ];
        let told = ops.map(|op| Told::Stage(Stage { id: 1, op })).into_iter().chain([
            Told::Capacity { capacity: 4096, ask: false },
            Told::Committed { epoch: 1 },
            Told::Stats { ask: 1 },
        ]);
        for told in told {
            let header = told.encode();
            assert_eq!(Told::decode(&header), Ok(told));
            for bit in (0..32).map(|bit| 1u32 << bit) {
                let flipped = Header { flags: header.flags ^ bit, ..header };
                let known = matches!(told, Told::Capacity { .. }) && bit == CAPACITY_ASK;
                assert_eq!(Told::decode(&flipped).is_ok(), known, "{told:?}, flag {bit:#x}");
            }
        }
        for bit in (0..32).map(|bit| 1u32 << bit) {
            let header = Header::new(ACK, 1).with_flags(bit);
            let made = Message { header, payload: vec![], fds: vec![] };
            assert_eq!(Made::decode(&made).err(), Some(Errno::EINVAL), "{bit:#x}");
        }
    }

    /// TASK-160.9: what is refused besides: a payload, or descriptors, a request has no use for,
    /// arguments out of range, a status, and paths that name nothing.
    #[test]
    fn requests_malformed_otherwise_are_refused() {
        let refused = |header: Header, payload: &[u8], fds: Vec<OwnedFd>| {
            let message = Message { header, payload: payload.to_vec(), fds };
            assert_eq!(Request::decode(&message).err(), Some(Errno::EINVAL), "{header:?}");
        };
        refused(Header::new(READ, 1).with_args(&[1, 1, 0, 4096]), b"extra", vec![]);
        refused(Header::new(READ, 1).with_args(&[1, 1, 0, MAX_PAYLOAD + 1]), b"", vec![]);
        refused(Header::new(INFO, 1), b"", vec![area()]);
        refused(Header::new(TRANSFER, 1).with_args(&[4096]), b"", vec![]);
        refused(Header::new(TRANSFER, 1).with_args(&[4096]), b"", vec![area(), area()]);
        refused(Header::new(WINDOW, 1).with_args(&[MAX_WINDOW + 1]), b"", vec![]);
        // fxdax: a window copying on write of more private pages than it has pages.
        let pages = (1 << 20) / crate::pages::page_size();
        refused(Header::new(WINDOW, 1).with_args(&[1 << 20, pages + 1]), b"", vec![]);
        refused(Header::new(PRIVATE, 1).with_args(&[0, 4096]), b"extra", vec![]);
        refused(Header::new(RESIZED, 1).with_args(&[4096, 2]), b"", vec![]);
        refused(Header { status: -1, ..Header::new(INFO, 1) }, b"", vec![]);
        refused(Header::new(FREE_MANY, 1).with_args(&[1, 1]), &[0; EXTENT_ID_BYTES + 8], vec![]);
        // fxdax: no mapping, part of one, or one of flags unknown (TASK-92.14).
        refused(Header::new(MAP_MANY, 1), b"", vec![]);
        refused(Header::new(MAP_MANY, 1), &[0; MAPPING_BYTES as usize - 8], vec![]);
        let unknown = [0u64, 4096, 1, 1, 0, 1].map(u64::to_le_bytes).concat();
        refused(Header::new(MAP_MANY, 1), &unknown, vec![]);
        let write = Header::new(WRITE, 1).with_args(&[1, 1, 0, 0, 8]);
        refused(write.with_flags(WRITE_COMMIT | WRITE_ROOT), b"data", vec![]);
        let transfer = Header::new(WRITE, 1).with_args(&[1, 1, 0, 0, 8]);
        refused(transfer.with_flags(WRITE_TRANSFER), b"no root", vec![]);
        refused(Header::new(COMMIT, 1), b"no root", vec![]);
        let open = |flags| with_version(Header::new(OPEN, 1).with_flags(flags));
        for path in [&b""[..], &b"a\0b"[..]] {
            refused(Header::new(FORK, 1), path, vec![]);
            refused(open(OPEN_CREATE), path, vec![]);
        }
        refused(open(OPEN_FORK_FROM), b"no from", vec![]);
        refused(open(OPEN_FORK_FROM), b"store\0", vec![]);
        refused(Header::new(BUILD, 1), b"a\"b", vec![]);
        refused(Header::new(BUILD, 1), &[b'a'; MAX_BUILD + 1], vec![]);
        // fxdax: a guest's stats, without the flag that says it carries them; past their most;
        // not text (TASK-111.4).
        refused(Header::new(STATS, 1), b"seals 1\n", vec![]);
        let guest = Header::new(STATS, 1).with_flags(STATS_GUEST);
        refused(guest, &[b'\n'; MAX_STATS + 1], vec![]);
        refused(guest, b"seals \xff\n", vec![]);
    }

    /// TASK-160.9: the bytes are those the VM process and the daemon wrote by hand before.
    #[test]
    fn requests_are_the_bytes_they_were() {
        let extent = ExtentId { branch: 7, sequence: 9 };
        let area = area();
        let root =
            WriteBarriers { before: true, after: Some(Commit::Durable), root: Some(&b"r"[..]) };
        let header =
            |kind, flags, args: &[u64]| Header::new(kind, 0).with_args(args).with_flags(flags);
        let cases: Vec<(Request<'_>, Header, Vec<u8>)> = vec![
            (
                Request::Open { path: "s".into(), opening: Opening::Create(4096) },
                header(OPEN, OPEN_CREATE, &[4096, 0, 0, 0, VERSION]),
                b"s".to_vec(),
            ),
            (
                Request::Open {
                    path: "s".into(),
                    opening: Opening::ForkFrom { from: "f".into(), branch: 3, capacity: 4096 },
                },
                header(OPEN, OPEN_FORK_FROM, &[4096, 3, 0, 0, VERSION]),
                b"s\0f".to_vec(),
            ),
            (
                Request::Open { path: "s".into(), opening: Opening::ReadOnly },
                header(OPEN, OPEN_READ_ONLY, &[0, 0, 0, 0, VERSION]),
                b"s".to_vec(),
            ),
            (
                Request::Read { extent, offset: 1, len: 2, into: ReadInto::Reply },
                header(READ, 0, &[7, 9, 1, 2]),
                vec![],
            ),
            (
                Request::Read { extent, offset: 1, len: 2, into: ReadInto::Transfer { at: 3 } },
                header(READ, READ_TRANSFER, &[7, 9, 1, 2, 3]),
                vec![],
            ),
            (
                Request::Write {
                    extent,
                    offset: 1,
                    data: Data::Inline(&b"dd"[..]),
                    barriers: root,
                },
                header(WRITE, 0b1_0111, &[7, 9, 1, 0, 2]),
                b"ddr".to_vec(),
            ),
            (
                Request::Write {
                    extent,
                    offset: 1,
                    data: Data::Transfer { at: 3, len: 2 },
                    barriers: WriteBarriers::default(),
                },
                header(WRITE, WRITE_TRANSFER, &[7, 9, 1, 3, 2]),
                vec![],
            ),
            (
                Request::FreeMany { first: extent, rest: Cow::Owned(vec![extent]) },
                header(FREE_MANY, 0, &[7, 9]),
                [7u64.to_le_bytes(), 9u64.to_le_bytes()].concat(),
            ),
            (
                Request::Map(WindowMapping {
                    window_offset: 5,
                    len: 4,
                    contents: MappingContents::Extent { extent, offset: 3 },
                }),
                header(MAP, 0, &[7, 9, 3, 4, 5]),
                vec![],
            ),
            (
                Request::Map(WindowMapping {
                    window_offset: 5,
                    len: 4,
                    contents: MappingContents::Zeros,
                }),
                header(MAP, MAP_ZEROS, &[0, 0, 0, 4, 5]),
                vec![],
            ),
            (
                Request::MapMany(Cow::Owned(vec![
                    WindowMapping {
                        window_offset: 5,
                        len: 4,
                        contents: MappingContents::Extent { extent, offset: 3 },
                    },
                    WindowMapping { window_offset: 6, len: 4, contents: MappingContents::Zeros },
                ])),
                header(MAP_MANY, 0, &[]),
                [5u64, 4, 7, 9, 3, 0, 6, 4, 0, 0, 0, MAPPING_ZEROS].map(u64::to_le_bytes).concat(),
            ),
            (
                Request::Commit { commit: Commit::Durable, root: Some(&b"r"[..]) },
                header(COMMIT, COMMIT_ROOT | COMMIT_DURABLE, &[]),
                b"r".to_vec(),
            ),
            (
                Request::Fault(Fault {
                    offset: 5,
                    access: Access::Write,
                    page: true,
                    probe: false,
                }),
                header(FAULT, FAULT_WRITE | FAULT_PAGE, &[5]),
                vec![],
            ),
            (
                Request::Fault(Fault { offset: 5, access: Access::Read, page: true, probe: true }),
                header(FAULT, FAULT_PAGE | FAULT_PROBE, &[5]),
                vec![],
            ),
            (
                Request::Fork { path: "p".into(), branch: 3, quiescent: true },
                header(FORK, FORK_QUIESCENT, &[3]),
                b"p".to_vec(),
            ),
            (
                Request::Resized { capacity: 4096, took: true },
                header(RESIZED, 0, &[4096, 1]),
                vec![],
            ),
            (
                Request::Transfer { len: 4096, area: Area(area.as_fd()) },
                header(TRANSFER, 0, &[4096]),
                vec![],
            ),
            (
                Request::Window { size: 8192, private_max: None, budget: 0 },
                header(WINDOW, 0, &[8192]),
                vec![],
            ),
            (
                Request::Window { size: 8192, private_max: Some(2), budget: 3 },
                header(WINDOW, 0, &[8192, 2, 3]),
                vec![],
            ),
            (
                Request::Stats { ask: 5, guest: Some("seals 1\n") },
                header(STATS, STATS_GUEST, &[5]),
                b"seals 1\n".to_vec(),
            ),
            (Request::Stats { ask: 5, guest: None }, header(STATS, 0, &[5]), vec![]),
            (
                Request::Private { window_offset: 4096, len: 8192 },
                header(PRIVATE, 0, &[4096, 8192]),
                vec![],
            ),
        ];
        for (request, header, payload) in cases {
            let encoded = request.encode();
            let bytes = (encoded.header, encoded.payload.concat());
            assert_eq!(bytes, (header, payload), "{request:?}");
            assert_eq!(encoded.fd.is_some(), request.kind() == TRANSFER);
        }
        let map = Stage {
            id: 4,
            op: StageOp::Map {
                offset: 1,
                phys: PhysLocation { arena: ArenaId(6), offset: 2 },
                len: 3,
            },
        };
        assert_eq!(map.encode(), Header::new(STAGE_MAP, 4).with_args(&[1, 2, 3, 0, 6]));
        let unmap = Stage { id: 4, op: StageOp::Unmap { offset: 1, len: 2 } };
        let acked = Header::new(STAGE_UNMAP, 4).with_args(&[1, 2]).with_flags(STAGE_ACK);
        assert_eq!(unmap.encode(), acked);
        // fxdax: a private page, a copy of a page or zeros, acknowledged (backlog doc-13).
        let from = Some(PhysLocation { arena: ArenaId(6), offset: 2 });
        let private = Stage { id: 4, op: StageOp::Private { offset: 1, from } }.encode();
        let copied = Header::new(STAGE_PRIVATE, 4).with_args(&[1, 2, 1, 0, 6]);
        assert_eq!(private, copied.with_flags(STAGE_ACK));
        let zeros = Stage { id: 4, op: StageOp::Private { offset: 1, from: None } }.encode();
        assert_eq!(zeros, Header::new(STAGE_PRIVATE, 4).with_args(&[1]).with_flags(STAGE_ACK));
        let ask = Told::Capacity { capacity: 4096, ask: true }.encode();
        assert_eq!(ask, Header::new(CAPACITY, 0).with_args(&[4096]).with_flags(CAPACITY_ASK));
        let counts = StageCounts { costs: [1, 2, 3, 4], installed: 5 };
        let ack = Made::Ack { id: 4, counts }.encode();
        assert_eq!(ack, Header::new(ACK, 4).with_args(&[1, 2, 3, 4, 5]));
        let asked = Told::Stats { ask: 6 }.encode();
        assert_eq!(asked, Header::new(STATS_ASK, 0).with_args(&[6]));
        let private = Made::PrivateCounts([1, 64, 5, 2, 9]).encode();
        assert_eq!(private, Header::new(STAGE_PRIVATE_COUNTS, 0).with_args(&[1, 64, 5, 2, 9]));
    }

    proptest! {
        /// TASK-160.9: what the daemon tells, each stage operation among it, and what VM processes
        /// say of them, decode as encoded.
        #[test]
        fn told_and_made_decode_as_encoded(
            id in any::<u64>(),
            [a, b, c, d, e] in any::<[u64; 5]>(),
            arena in any::<u32>(),
            ask in any::<bool>(),
        ) {
            let phys = PhysLocation { arena: ArenaId(arena), offset: b };
            let stages = [
                StageOp::Map { offset: a, phys, len: c },
                StageOp::Unmap { offset: a, len: b },
                StageOp::Mapped { offset: a, len: b },
                StageOp::Private { offset: a, from: Some(phys) },
                StageOp::Private { offset: a, from: None },
            ];
            let told = stages.map(|op| Told::Stage(Stage { id, op })).into_iter().chain([
                Told::Capacity { capacity: a, ask },
                Told::Committed { epoch: a },
                Told::Stats { ask: b },
            ]);
            for told in told {
                prop_assert_eq!(Told::decode(&told.encode()), Ok(told));
            }
            let counts = StageCounts { costs: [a, b, c, d], installed: e };
            let other = StageCounts { costs: [b, c, d, e], installed: a };
            let made = [
                Made::Ack { id, counts },
                Made::Counts(other),
                Made::PrivateCounts([a, b, c, d, e]),
            ];
            for made in made {
                let message = Message { header: made.encode(), payload: vec![], fds: vec![] };
                prop_assert_eq!(Made::decode(&message), Ok(made));
            }
        }

        /// TASK-160.9: what replies say besides their status decodes as encoded.
        #[test]
        fn replies_decode_as_encoded(
            [a, b, c, d, e] in any::<[u64; 5]>(),
            arena in any::<u32>(),
            mappings_of in proptest::collection::vec((any::<[u64; 5]>(), any::<bool>()), 0..16),
        ) {
            let max_map_count = (c != 0).then_some(c);
            let opened = Opened { branch: a, arena: ArenaId(arena), epoch: b, max_map_count };
            prop_assert_eq!(Opened::from_args(opened.args()), Ok(opened));
            let info = Info(ExtentInfo {
                capacity: a,
                used: b,
                available: c,
                next_sequence: d,
                sequence_floor: e,
            });
            prop_assert_eq!(Info::from_args(info.args()), info);
            let advanced = Advanced { epoch: a };
            prop_assert_eq!(Advanced::from_args(advanced.args()), advanced);
            let mapped: Vec<WindowMapping> = (mappings_of.into_iter())
                .map(|([window_offset, len, branch, sequence, offset], zeros)| WindowMapping {
                    window_offset,
                    len,
                    contents: match zeros {
                        true => MappingContents::Zeros,
                        false => {
                            let extent = ExtentId { branch, sequence };
                            MappingContents::Extent { extent, offset }
                        }
                    },
                })
                .collect();
            let payload = mappings_payload(&mapped);
            prop_assert_eq!(payload.len() as u64, mapped.len() as u64 * MAPPING_BYTES);
            prop_assert_eq!(mappings(&payload), Ok(mapped));
        }
    }

    /// Replies malformed: an arena that is none, mappings of part of one, or of flags unknown or
    /// meaning nothing together.
    #[test]
    fn replies_malformed_are_refused() {
        assert_eq!(Opened::from_args([1, 1 << 32, 0, 0, VERSION]).err(), Some(Errno::EINVAL));
        let mapping = |flags: u64| [0u64, 4096, 1, 1, 0, flags].map(u64::to_le_bytes).concat();
        assert_eq!(mappings(&mapping(0)[..40]).err(), Some(Errno::EINVAL));
        // fxdax: bit 0, writable, writable DAX's (backlog doc-13).
        for flags in [1, MAPPING_ZEROS | 1, 1 << 2, 1 << 63] {
            assert_eq!(mappings(&mapping(flags)).err(), Some(Errno::EINVAL), "{flags:#x}");
        }
    }

    /// TASK-141.3: an `OPEN` of another wire version is refused as such, whatever else it holds,
    /// as is a reply to one from a daemon of another, an older one's, saying none, among them.
    #[test]
    fn opens_of_other_wire_versions_are_refused() {
        let area = area();
        proptest!(|(drawn in fields(), id in any::<u64>(), flags in any::<u32>())| {
            let opens = requests(&drawn, area.as_fd()).into_iter().filter(|r| r.kind() == OPEN);
            for open in opens {
                let mut message = sent(&open, id);
                prop_assert_eq!(version(&message.header), VERSION);
                for other in [0, VERSION + 1, u64::MAX] {
                    message.header.args[4] = other;
                    let refused = Some(Errno::EPROTONOSUPPORT);
                    prop_assert_eq!(Request::decode(&message).err(), refused);
                    message.header.flags = flags;
                    prop_assert_eq!(Request::decode(&message).err(), refused);
                }
            }
        });
        let opened = Opened { branch: 1, arena: ArenaId::DEFAULT, epoch: 0, max_map_count: None };
        assert_eq!(version(&Header::new(REPLY, 1).with_args(&opened.args())), VERSION);
        for other in [0, VERSION + 1] {
            let [branch, arena, epoch, max_map_count, _] = opened.args();
            let args = [branch, arena, epoch, max_map_count, other];
            assert_eq!(Opened::from_args(args).err(), Some(Errno::EPROTONOSUPPORT), "{other}");
            // Before its arena, which another version may say otherwise.
            let args = [branch, u64::MAX, epoch, max_map_count, other];
            assert_eq!(Opened::from_args(args).err(), Some(Errno::EPROTONOSUPPORT), "{other}");
        }
    }

    /// TASK-141.3: the wire as this [`VERSION`] has it, as a fingerprint of everything its two ends
    /// must agree on: what each kind of message encodes to, of fixed fields, and decodes from (the
    /// flags each kind knows), the replies' arguments, the [`Header`]'s layout, and the bounds.
    /// A change to any of it without a new `VERSION` fails here: a VM process and a daemon of the
    /// two would meet as of one version, and serve each other with what neither means.
    #[test]
    fn the_wire_is_its_version() {
        /// The version, and the fingerprint of the wire it is. A change to the wire is a new
        /// version, both changed together, never the fingerprint alone.
        const WIRE: (u64, u64) = (4, 0x2461_c9e8_dfa2_d6a6);
        let fingerprint = fingerprint();
        assert_eq!(
            (VERSION, fingerprint),
            WIRE,
            "the wire changed, its fingerprint {fingerprint:#x}: wire::VERSION goes up with it, to \
             {}, and WIRE here to ({}, {fingerprint:#x}) (AGENTS.md: no migrations)",
            WIRE.0 + 1,
            WIRE.0 + 1,
        );
    }

    /// The wire's fingerprint ([`the_wire_is_its_version`]): FNV-1a of what it is made of.
    fn fingerprint() -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut hashed = |bytes: &[u8]| {
            for byte in bytes {
                hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
            }
            // Where each part ends.
            hash = (hash ^ 0xff).wrapping_mul(0x0100_0000_01b3);
        };
        let header = |hashed: &mut dyn FnMut(&[u8]), header: Header| {
            hashed(zerocopy::IntoBytes::as_bytes(&header))
        };
        hashed(&std::mem::size_of::<Header>().to_le_bytes());
        let fields = Fields {
            words: [3, 5, 4096, 8192, 1 << 20],
            flags: [true, true, false],
            data: (0..64).collect(),
            root: b"root".to_vec(),
            paths: [PathBuf::from("store"), PathBuf::from("from")],
            extents: vec![
                ExtentId { branch: 7, sequence: 9 },
                ExtentId { branch: 7, sequence: 11 },
            ],
            build: "build".into(),
            stats: "stats".into(),
            commit: Commit::Durable,
            access: Access::Write,
        };
        let area = area();
        for request in requests(&fields, area.as_fd()) {
            let message = sent(&request, 1);
            header(&mut hashed, message.header);
            hashed(&message.payload);
        }
        for kind in 0..256 {
            hashed(&known_flags(kind).map_or(u64::MAX, u64::from).to_le_bytes());
        }
        // fxdax: what any request says of its guest request (TASK-260).
        hashed(&REQUEST.to_le_bytes());
        hashed(&request_bytes(NonZeroU64::new(5).unwrap()));
        let phys = PhysLocation { arena: ArenaId(6), offset: 1 << 21 };
        let told = [
            StageOp::Map { offset: 4096, phys, len: 8192 },
            StageOp::Unmap { offset: 4096, len: 8192 },
            StageOp::Mapped { offset: 4096, len: 8192 },
            StageOp::Private { offset: 4096, from: Some(phys) },
            StageOp::Private { offset: 4096, from: None },
        ]
        .map(|op| Told::Stage(Stage { id: 2, op }))
        .into_iter()
        .chain([
            Told::Capacity { capacity: 1 << 30, ask: true },
            Told::Committed { epoch: 12 },
            Told::Stats { ask: 6 },
        ]);
        for told in told {
            header(&mut hashed, told.encode());
        }
        let (counts, other) = (
            StageCounts { costs: [1, 2, 3, 4], installed: 5 },
            StageCounts { costs: [5, 6, 7, 8], installed: 9 },
        );
        let private = Made::PrivateCounts([1, 64, 5, 2, 9]);
        for made in [Made::Ack { id: 2, counts }, Made::Counts(other), private] {
            header(&mut hashed, made.encode());
        }
        let opened = Opened { branch: 3, arena: ArenaId(6), epoch: 12, max_map_count: Some(65530) };
        let info = ExtentInfo {
            capacity: 1 << 30,
            used: 4096,
            available: (1 << 30) - 4096,
            next_sequence: 12,
            sequence_floor: 1 << 40,
        };
        let replies = [opened.args(), Info(info).args(), Advanced { epoch: 12 }.args()];
        hashed(
            &replies.as_flattened().iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>(),
        );
        let extent =
            MappingContents::Extent { extent: ExtentId { branch: 3, sequence: 4 }, offset: 8192 };
        hashed(&mappings_payload(&[
            WindowMapping { window_offset: 0, len: 4096, contents: extent },
            WindowMapping { window_offset: 4096, len: 8192, contents: MappingContents::Zeros },
        ]));
        let bounds = [
            MAX_PAYLOAD,
            MAX_TRANSFER_AREA,
            MAX_WINDOW,
            MAX_REQUESTS as u64,
            MAX_STREAM_BYTES,
            MAX_FDS as u64,
            MAX_BUILD as u64,
            MAX_ROOT as u64,
            MAX_STATS as u64,
            MAPPING_BYTES,
            EXTENT_ID_BYTES as u64,
            STAGE_COUNTS_EVERY.as_nanos() as u64,
        ];
        hashed(&bounds.iter().flat_map(|bound| bound.to_le_bytes()).collect::<Vec<_>>());
        hash
    }

    /// fxdax: the extents a `FREE_MANY`'s payload names are those put in it, in order; one that
    /// holds part of an ID is refused (TASK-92.13).
    #[test]
    fn free_many_payloads_name_their_extents() {
        let extents =
            [ExtentId { branch: 1, sequence: 2 }, ExtentId { branch: u64::MAX, sequence: 1 << 40 }];
        let payload = extent_ids_payload(&extents);
        assert_eq!(payload.len(), 2 * EXTENT_ID_BYTES);
        assert_eq!(extent_ids(&payload).unwrap().collect::<Vec<_>>(), extents);
        assert_eq!(extent_ids(&payload[..EXTENT_ID_BYTES + 8]).err(), Some(Errno::EINVAL));
    }
}
