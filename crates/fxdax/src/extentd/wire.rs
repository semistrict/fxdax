// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The messages between a VM process and the extent daemon, and their framing on a Unix stream.
//!
//! Each message is a [`Header`], followed by `len` bytes of payload. A message may carry up to
//! [`MAX_FDS`] file descriptors, as `SCM_RIGHTS`, with its first bytes: the daemon's reply to
//! [`OPEN`] carries its kernel's physical memory, the VM process's tenant's arena of it, `args[1]`,
//! the default tenant's if it is of none, and the default arena, read-only (TASK-43, TASK-79.17).

use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// A message's header: what it is, the request it is for, and its arguments.
#[repr(C)]
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
pub(crate) struct Header {
    /// One of the kinds below.
    pub kind: u32,
    /// The kind's flags.
    pub flags: u32,
    /// The request a reply answers, or the stage operation an acknowledgement does.
    pub id: u64,
    /// The kind's arguments.
    pub args: [u64; 5],
    /// A reply's status: 0, or a -errno.
    pub status: i32,
    /// fxdax: how many descriptors the message carries, which come with its first bytes
    /// ([`MAX_FDS`] at most): what a receiver that reads many messages at once gives it of those
    /// that came with them, in order (TASK-91.16). The senders set it.
    pub fds: u32,
    /// The bytes of payload after the header.
    pub len: u64,
}

impl Header {
    pub fn new(kind: u32, id: u64) -> Self {
        Self { kind, id, ..Default::default() }
    }

    pub fn with_args(mut self, args: &[u64]) -> Self {
        self.args[..args.len()].copy_from_slice(args);
        self
    }

    pub fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }
}

// Requests, from a VM process. A reply ([`REPLY`]) answers each, but for [`ACK`].

/// Opens the store at the path the payload holds, creating it with `args[0]` bytes of capacity if
/// [`OPEN_CREATE`] and it doesn't exist; or, with [`OPEN_FORK_FROM`], forking it from the store
/// at the path after the payload's first NUL byte if it doesn't exist (TASK-39.1: a VM restored
/// from a snapshot, whose store point that is), or is a fork that failed, whose deletion it
/// finishes first (TASK-79.84). That point, if it doesn't exist either, is taken
/// from the daemon's bucket: a store of branch `args[1]`, of `args[0]` bytes, as on another host
/// than the snapshot's. The reply's `args[0]` is its branch, and it carries the kernel's physical
/// memory. fxdax: with [`OPEN_READ_ONLY`], a follower of the store (backlog doc-8), as of its
/// latest commit, whose epoch is the reply's `args[2]`.
pub(crate) const OPEN: u32 = 1;
/// Reads `args[3]` bytes at `args[2]` in extent `args[0..2]` (`READ`): the reply's payload; or,
/// with [`READ_TRANSFER`], into the connection's transfer area at `args[4]` ([`TRANSFER`]).
pub(crate) const READ: u32 = 2;
/// Writes the payload at `args[2]` in extent `args[0..2]` (`WRITE`), after a barrier if
/// [`WRITE_BARRIER`], and followed by a commit if [`WRITE_COMMIT`]; or, with [`WRITE_TRANSFER`],
/// the `args[4]` bytes at `args[3]` in the connection's transfer area ([`TRANSFER`]).
pub(crate) const WRITE: u32 = 3;
/// Frees extent `args[0..2]` (`FREE`).
pub(crate) const FREE: u32 = 4;
/// Maps `args[3]` bytes at `args[2]` in extent `args[0..2]` into the window at `args[4]`
/// (`MAP`), writably if [`MAP_WRITABLE`]; or zeros there, read-only, with [`MAP_ZEROS`], as a
/// snapshot's mappings may ([`MAPPINGS`]).
pub(crate) const MAP: u32 = 5;
/// Unmaps `args[1]` bytes of the window at `args[0]` (`UNMAP`).
pub(crate) const UNMAP: u32 = 6;
/// Commits (`COMMIT`), with the payload as the root if [`COMMIT_ROOT`].
pub(crate) const COMMIT: u32 = 7;
/// Reads the root (`ROOT`): the reply's payload.
pub(crate) const ROOT: u32 = 8;
/// What the store holds (`INFO`): the reply's `args`, as [`ExtentInfo`](crate::ExtentInfo)'s
/// fields in order.
pub(crate) const INFO: u32 = 9;
/// Creates the store's window, of `args[0]` bytes, which the VM process maps in its own memory as
/// the daemon's [stage operations](STAGE_MAP) say.
pub(crate) const WINDOW: u32 = 10;
/// Resolves the guest's fault at `args[0]` in the window: a store if [`FAULT_WRITE`], on that page
/// alone if [`FAULT_PAGE`]. The stage operations it takes come before the reply.
pub(crate) const FAULT: u32 = 11;
/// Acknowledges stage operation `id`, which the VM process has made; and says, as
/// [`STAGE_COUNTS`] does, what its stage operations have cost it so far.
pub(crate) const ACK: u32 = 12;
/// Forks the store, as of its last commit made durable, to a new branch `args[0]`, whose handle
/// is a new directory at the path the payload holds, nothing there (`EEXIST`)
/// ([`ExtentStore::fork`](crate::ExtentStore)):
/// a fork the daemon serves, sharing its extents' pages with the store's. With
/// [`FORK_QUIESCENT`], it fails with `EBUSY` unless nothing is open
/// ([`ExtentStore::is_quiescent`](crate::ExtentStore::is_quiescent)): the point a snapshot of
/// the VM is taken at.
pub(crate) const FORK: u32 = 13;
/// The guest's answer to a [`CAPACITY`] that asked it to shrink to `args[0]` bytes: whether it
/// took that size, `args[1]` 1, or not, 0.
pub(crate) const RESIZED: u32 = 16;
/// The window's mappings ([`ExtentStore::window_mappings`](crate::ExtentStore::window_mappings)):
/// the reply's payload, each six little-endian words ([`MAPPING_BYTES`]): its offset in the
/// window, its length, the extent's branch and sequence and the offset in it, and
/// [`MAPPING_WRITABLE`] or [`MAPPING_ZEROS`]. As many as a window of its size holds, a mapping a
/// page at most, which may be more than [`MAX_PAYLOAD`] holds (TASK-79.43).
pub(crate) const MAPPINGS: u32 = 17;
/// The bytes of a mapping in a [`MAPPINGS`] reply.
pub(crate) const MAPPING_BYTES: u64 = 6 * 8;
// 14, 15 and 18 to 20 were requests the host's tools made, of the daemon's stats, and to resize,
// fork, delete and sync stores by path: the control API's now (TASK-43.5), not reused.
/// fxdax: moves a follower's view to the writer's latest commit, whose epoch is the reply's
/// `args[0]`, holding what the commits since the one it was as of held, until [`LET_GO`]
/// ([`ExtentStore::advance`](crate::ExtentStore::advance)).
pub(crate) const ADVANCE: u32 = 21;
/// fxdax: lets go of what a follower's view held of the commits before the commit of epoch
/// `args[0]`, which an [`ADVANCE`] returned ([`ExtentStore::let_go`](crate::ExtentStore::let_go)):
/// the epoch named, a let-go served after the next advance lets go of no more.
pub(crate) const LET_GO: u32 = 22;
/// fxdax: keeps extent `args[0..2]`, which a follower's view holds, whatever commit it advances
/// to, until [`UNPIN`] ([`ExtentStore::pin`](crate::ExtentStore::pin)).
pub(crate) const PIN: u32 = 23;
/// fxdax: lets go of extent `args[0..2]`, pinned.
pub(crate) const UNPIN: u32 = 24;
/// The connection's transfer area, the shared memory it carries, of `args[0]` bytes: what
/// `READ`s and `WRITE`s with [`READ_TRANSFER`] and [`WRITE_TRANSFER`] move their data through,
/// rather than the stream, whose sending and receiving copy it twice more (TASK-26.18.6). The VM
/// process says which part of it each uses, which it has no other use for meanwhile.
pub(crate) const TRANSFER: u32 = 25;
/// fxdax: frees extent `args[0..2]`, and those the payload names after it ([`extent_ids`]), each
/// as [`FREE`] frees it, in order: the first that fails fails the request, those before it freed,
/// as a guest's `FREE` with doc-4's `FREE_MANY` does, a flush's dead extents in one round trip, not
/// one each (TASK-92.13). A request of its own, not a flag of `FREE`'s: a daemon older than it,
/// which ignores flags it doesn't know, would free the first alone and succeed, the rest leaked,
/// where it refuses a request it doesn't know (`EINVAL`).
pub(crate) const FREE_MANY: u32 = 26;
/// The most bytes a transfer area holds.
pub(crate) const MAX_TRANSFER_AREA: u64 = 64 << 20;
/// fxdax: the stage operations the VM process has made of the connection's, `args[0]`, the
/// nanoseconds they took it, `args[1]`, and those of them that took access away from pages it had
/// installed, by how, `args[2..5]` (`Stats::stage_overwrites`), so far, which the daemon counts as
/// its own (`Stats::stage_ops`, TASK-91.16): said with each [`ACK`]; otherwise once [`STAGE_COUNTS_EVERY`]
/// has passed since it last said, as it makes them; and as it closes the connection. Nothing
/// replies.
pub(crate) const STAGE_COUNTS: u32 = 27;
/// fxdax: how long a VM process's stage operations go uncounted at most while it makes them
/// ([`STAGE_COUNTS`]): within a sample of the daemon's stats file (`FXDAX_EXTENTD_STATS`).
pub(crate) const STAGE_COUNTS_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

pub(crate) const OPEN_CREATE: u32 = 1 << 0;
pub(crate) const OPEN_FORK_FROM: u32 = 1 << 1;
pub(crate) const OPEN_READ_ONLY: u32 = 1 << 2;
pub(crate) const MAP_WRITABLE: u32 = 1 << 0;
pub(crate) const MAP_ZEROS: u32 = 1 << 1;
pub(crate) const FORK_QUIESCENT: u32 = 1 << 0;
pub(crate) const MAPPING_WRITABLE: u64 = 1 << 0;
pub(crate) const MAPPING_ZEROS: u64 = 1 << 1;
pub(crate) const COMMIT_ROOT: u32 = 1 << 0;
/// fxdax: a `COMMIT` that completes once durable, not once ordered.
pub(crate) const COMMIT_DURABLE: u32 = 1 << 1;
pub(crate) const WRITE_BARRIER: u32 = 1 << 0;
pub(crate) const WRITE_COMMIT: u32 = 1 << 1;
/// A `WRITE`'s commit completes once durable.
pub(crate) const WRITE_DURABLE: u32 = 1 << 2;
pub(crate) const WRITE_TRANSFER: u32 = 1 << 3;
pub(crate) const READ_TRANSFER: u32 = 1 << 0;
pub(crate) const FAULT_WRITE: u32 = 1 << 0;
pub(crate) const FAULT_PAGE: u32 = 1 << 1;
/// fxdax: the bytes an extent's ID takes in a [`FREE_MANY`]'s payload: its branch and its
/// sequence, little-endian words.
pub(crate) const EXTENT_ID_BYTES: usize = 16;

/// fxdax: `extents`, as a [`FREE_MANY`]'s payload names them.
pub(crate) fn extent_ids_payload(extents: &[crate::ExtentId]) -> Vec<u8> {
    (extents.iter())
        .flat_map(|extent| [extent.branch, extent.sequence])
        .flat_map(u64::to_le_bytes)
        .collect()
}

/// fxdax: the extents a [`FREE_MANY`]'s payload names: `EINVAL` unless it is whole IDs.
pub(crate) fn extent_ids(
    payload: &[u8],
) -> Result<impl Iterator<Item = crate::ExtentId> + '_, crate::Errno> {
    let (ids, []) = payload.as_chunks::<EXTENT_ID_BYTES>() else {
        return Err(crate::Errno::EINVAL);
    };
    let word = |bytes: &[u8]| u64::from_le_bytes(bytes.try_into().expect("a word's bytes"));
    Ok(ids
        .iter()
        .map(move |id| crate::ExtentId { branch: word(&id[..8]), sequence: word(&id[8..]) }))
}

// From the daemon.

/// The reply to request `id`.
pub(crate) const REPLY: u32 = 100;
/// Maps `args[2]` bytes of the kernel's physical memory at `args[1]` in arena `args[4]`'s object
/// into the window at `args[0]`, with protection `args[3]` ([`Protection`](crate::Protection) as
/// [`protection_code`]).
pub(crate) const STAGE_MAP: u32 = 101;
/// Changes the protection of `args[1]` bytes of the window at `args[0]` to `args[2]`.
pub(crate) const STAGE_PROTECT: u32 = 102;
/// Unmaps `args[1]` bytes of the window at `args[0]`.
pub(crate) const STAGE_UNMAP: u32 = 103;
/// fxdax: the window maps `args[1]` bytes at `args[0]` anew (`SecondStage::window_mapped`).
pub(crate) const STAGE_MAPPED: u32 = 106;
/// The store's capacity is `args[0]` bytes now, which the guest takes (the control API's `ResizeStore`); or, with
/// [`CAPACITY_ASK`], the guest is asked to shrink to `args[0]` bytes, and answers ([`RESIZED`]).
pub(crate) const CAPACITY: u32 = 104;
pub(crate) const CAPACITY_ASK: u32 = 1 << 0;
/// fxdax: the writer of a follower's store committed: its latest commit's epoch is `args[0]`,
/// which the follower may [`ADVANCE`] to. Sent once for each run of commits.
pub(crate) const COMMITTED: u32 = 105;
/// A stage operation the daemon waits for the VM process to acknowledge ([`ACK`]), as it must for
/// one that takes access to a page away, before it frees the page or shares it.
pub(crate) const STAGE_ACK: u32 = 1 << 0;

/// `protection` on the wire.
pub(crate) fn protection_code(protection: crate::Protection) -> u64 {
    match protection {
        crate::Protection::None => 0,
        crate::Protection::Read => 1,
        crate::Protection::ReadWrite => 2,
    }
}

/// The protection `code` stands for.
pub(crate) fn protection(code: u64) -> io::Result<crate::Protection> {
    match code {
        0 => Ok(crate::Protection::None),
        1 => Ok(crate::Protection::Read),
        2 => Ok(crate::Protection::ReadWrite),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "an unknown protection")),
    }
}

/// The most bytes of payload a message carries: a `READ` or `WRITE` of a whole extent, or less.
pub(crate) const MAX_PAYLOAD: u64 = crate::ExtentStore::MAX_EXTENT;

/// fxdax: the most requests a VM process keeps sent and not answered, its replies not yet received
/// (TASK-79.16): many times what its device's queues and its window's faults have at once. A VM
/// process past it the daemon lets go.
pub(crate) const MAX_REQUESTS: usize = 4096;

/// fxdax: the most bytes of data a VM process's requests not answered move through the stream
/// ([`stream_bytes`], TASK-79.16): `WRITE`s' payloads and `READ`s' replies, but those through the
/// transfer area ([`TRANSFER`]); a few times its largest. A VM process past it, as the daemon
/// finds what it holds of it, and what it has queued for it, unread, the daemon lets go.
pub(crate) const MAX_STREAM_BYTES: u64 = 4 * MAX_PAYLOAD;

/// fxdax: the bytes of data the request `header` is of moves through the stream: its payload, and,
/// a `READ`'s, its reply's: [`MAX_PAYLOAD`] at most, as one asking for more is refused (`EINVAL`),
/// its reply's payload empty, whatever the VM process asks for: so a word that would overflow the
/// bytes counted is counted as no more (TASK-79.16).
pub(crate) fn stream_bytes(header: &Header) -> u64 {
    match header.kind {
        READ if header.flags & READ_TRANSFER == 0 => header.len + header.args[3].min(MAX_PAYLOAD),
        _ => header.len,
    }
}

/// The most file descriptors a message carries.
pub(crate) const MAX_FDS: usize = 2;

/// A message received.
pub(crate) struct Message {
    pub header: Header,
    pub payload: Vec<u8>,
    pub fds: Vec<OwnedFd>,
}

/// The sending half of a VM process's connection, which any of its threads sends whole messages
/// through, each waiting for room as the socket fills: the daemon's is an [`AsyncSender`].
pub(crate) struct Sender(Mutex<UnixStream>);

impl Sender {
    pub fn new(stream: UnixStream) -> Self {
        Self(Mutex::new(stream))
    }

    /// Sends `header`, with `payload` as its payload, and `fds`, at most [`MAX_FDS`].
    pub fn send(
        &self,
        mut header: Header,
        payload: &[u8],
        fds: &[BorrowedFd<'_>],
    ) -> io::Result<()> {
        assert!(fds.len() <= MAX_FDS, "too many descriptors for a message");
        header.len = payload.len() as u64;
        header.fds = fds.len() as u32;
        let stream = self.0.lock().unwrap();
        let header = header.as_bytes();
        // The daemon's connections are non-blocking, for their receivers (TASK-34): a full one is
        // waited for.
        let sent = loop {
            match send_with_fds(&stream, header, fds) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => wait_writable(&stream)?,
                sent => break sent?,
            }
        };
        write_all(&stream, &header[sent..])?;
        write_all(&stream, payload)
    }

    /// Shuts the connection down, which ends its receiver's messages.
    pub fn shutdown(&self) {
        let _ = self.0.lock().unwrap().shutdown(std::net::Shutdown::Both);
    }
}

/// fxdax: the sending half of one of the daemon's connections, which any thread sends whole
/// messages through, in order, none waiting for the VM process to take them (TASK-79.9): the
/// kernel sends its stage operations with its locks held, and a VM process that stops reading
/// would hold them for good. What the socket has no room for waits in a queue, which a task on the
/// runtime it was made on writes as the socket takes it; what is sent meanwhile queues behind.
pub(crate) struct AsyncSender {
    stream: tokio::io::unix::AsyncFd<UnixStream>,
    queue: Mutex<Queue>,
    /// How many bytes the queue holds, which the socket has not taken (TASK-79.16).
    queued: std::sync::atomic::AtomicU64,
    runtime: tokio::runtime::Handle,
}

/// What an [`AsyncSender`]'s socket has not taken yet.
#[derive(Default)]
struct Queue {
    /// The messages, and their descriptors, which they carry with their first bytes.
    messages: std::collections::VecDeque<(Queued, Vec<OwnedFd>)>,
    /// Whether a task writes them.
    draining: bool,
    /// Why the socket takes no more, once it doesn't.
    failed: Option<io::ErrorKind>,
}

/// A message, as much of it as the socket has not taken.
struct Queued {
    header: [u8; size_of::<Header>()],
    payload: Vec<u8>,
    /// How many of its bytes, its header's then its payload's, were sent.
    sent: usize,
    /// fxdax: its kind, if it says the latest of what its kind says, which one of its kind sent
    /// after it replaces, unless its sending began ([`AsyncSender::send_latest`]).
    latest: Option<u32>,
}

impl Queued {
    /// How many of its bytes are not sent.
    fn left(&self) -> u64 {
        (self.header.len() + self.payload.len() - self.sent) as u64
    }

    /// What of its header, and of its payload, is not sent.
    fn unsent(&self) -> (&[u8], &[u8]) {
        match self.sent.checked_sub(self.header.len()) {
            None => (&self.header[self.sent..], &self.payload[..]),
            Some(at) => (&[][..], &self.payload[at..]),
        }
    }
}

thread_local! {
    /// fxdax: the sender this thread holds back what it sends through, and how many corks deep
    /// ([`AsyncSender::cork`]).
    static CORKED: std::cell::RefCell<Option<(usize, usize)>> =
        const { std::cell::RefCell::new(None) };
}

impl AsyncSender {
    /// Sends through `stream`, which the caller made non-blocking, on the runtime it is made on.
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        let stream =
            tokio::io::unix::AsyncFd::with_interest(stream, tokio::io::Interest::WRITABLE)?;
        let runtime = tokio::runtime::Handle::current();
        Ok(Self { stream, queue: Mutex::default(), queued: Default::default(), runtime })
    }

    /// How many bytes of what was sent through it its socket has not taken yet.
    pub fn queued(&self) -> u64 {
        self.queued.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether the other end hung up, as a VM process that went does, though the receiving end
    /// may not have read so yet.
    pub fn peer_gone(&self) -> bool {
        hung_up(self.stream.get_ref().as_fd())
    }

    /// Sends `header`, with `payload` as its payload, and `fds`, at most [`MAX_FDS`], after what
    /// was sent before: at once, what the socket takes, and the rest as it takes it. Fails once
    /// the socket failed, as the other end gone.
    pub fn send(
        self: &std::sync::Arc<Self>,
        header: Header,
        payload: Vec<u8>,
        fds: &[BorrowedFd<'_>],
    ) -> io::Result<()> {
        self.queue(header, payload, fds, false)
    }

    /// fxdax: sends `header`, a message of no payload that says the latest of what its kind says,
    /// as [`AsyncSender::send`] does: or, one of its kind waiting in the queue, its sending not
    /// begun, in its place (TASK-79.9). So what the other end, not reading, is told of again and
    /// again takes one message of the queue, the latest, not one each time.
    pub fn send_latest(self: &std::sync::Arc<Self>, header: Header) -> io::Result<()> {
        self.queue(header, Vec::new(), &[], true)
    }

    /// fxdax: holds back what this thread sends through it, until [`AsyncSender::uncork`], to send
    /// it all at once, in as few writes as the socket takes: the messages of one change to a
    /// window, which its VM process then reads, and makes, together (TASK-91.16). Others' sends
    /// meanwhile send what it held back first, in order, so that it delays none of theirs. Corks
    /// nest; this thread mustn't wait for what it holds back, as for a stage operation's
    /// acknowledgement, until it uncorks.
    pub fn cork(self: &std::sync::Arc<Self>) {
        CORKED.with_borrow_mut(|corked| match corked {
            Some((sender, depth)) => {
                assert_eq!(*sender, self.identity(), "a thread corks one sender at a time");
                *depth += 1;
            }
            None => *corked = Some((self.identity(), 1)),
        });
    }

    /// fxdax: ends a [`AsyncSender::cork`]: the last one sends what this thread held back.
    pub fn uncork(self: &std::sync::Arc<Self>) {
        let last = CORKED.with_borrow_mut(|corked| {
            let (sender, depth) = corked.as_mut().expect("uncorked a sender not corked");
            assert_eq!(*sender, self.identity(), "uncorked another sender than corked");
            *depth -= 1;
            let last = *depth == 0;
            if last {
                *corked = None;
            }
            last
        });
        if last {
            let mut queue = self.queue.lock().unwrap();
            if !queue.draining && queue.failed.is_none() {
                self.write_queued(&mut queue);
            }
        }
    }

    /// fxdax: whether this thread holds back what it sends through any sender
    /// ([`AsyncSender::cork`]).
    pub fn corked() -> bool {
        CORKED.with_borrow(Option::is_some)
    }

    /// What tells this sender from another, for [`CORKED`].
    fn identity(&self) -> usize {
        self as *const Self as usize
    }

    /// Sends `header`, `payload` and `fds`, as [`AsyncSender::send`] does, in place of a message
    /// of its kind waiting if it is the `latest` of what its kind says.
    fn queue(
        self: &std::sync::Arc<Self>,
        mut header: Header,
        payload: Vec<u8>,
        fds: &[BorrowedFd<'_>],
        latest: bool,
    ) -> io::Result<()> {
        assert!(fds.len() <= MAX_FDS, "too many descriptors for a message");
        header.len = payload.len() as u64;
        header.fds = fds.len() as u32;
        let latest = latest.then_some(header.kind);
        let header = header.as_bytes().try_into().unwrap();
        let message = Queued { header, payload, sent: 0, latest };
        let mut queue = self.queue.lock().unwrap();
        if let Some(failed) = queue.failed {
            return Err(failed.into());
        }
        if latest.is_some()
            && let Some((queued, _)) = (queue.messages.iter_mut().rev())
                .find(|(queued, _)| queued.latest == latest && queued.sent == 0)
        {
            queued.header = message.header;
            return Ok(());
        }
        // The descriptors are the queue's until it sends them, with the message's first bytes.
        let fds = fds.iter().map(|fd| fd.try_clone_to_owned()).collect::<io::Result<_>>()?;
        self.queued.fetch_add(message.left(), std::sync::atomic::Ordering::Relaxed);
        queue.messages.push_back((message, fds));
        // Held back by this thread, or written by the task draining the queue, if one does.
        let corked = CORKED
            .with_borrow(|corked| corked.is_some_and(|(sender, _)| sender == self.identity()));
        if !corked && !queue.draining {
            self.write_queued(&mut queue);
        }
        queue.failed.map_or(Ok(()), |failed| Err(failed.into()))
    }

    /// Writes what of the messages in `queue` the socket takes now, and has a task write the rest
    /// as it takes it; or, the socket failed, fails the queue, which the sends say from now on, as
    /// a VM process gone makes them fail.
    fn write_queued(self: &std::sync::Arc<Self>, queue: &mut Queue) {
        match self.send_queued(queue) {
            Ok(true) => {}
            Ok(false) => {
                queue.draining = true;
                self.runtime.spawn(self.clone().drain());
            }
            Err(err) => {
                queue.failed = Some(err.kind());
                queue.messages.clear();
                self.queued.store(0, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// Sends what of the messages in `queue` the socket takes, many in each write: whether it took
    /// them all. A message's descriptors go with its first bytes, which begin a write.
    fn send_queued(&self, queue: &mut Queue) -> io::Result<bool> {
        /// The most messages a write sends: their headers and payloads, each an `iovec`.
        const MOST: usize = 64;
        while !queue.messages.is_empty() {
            let fds: Vec<BorrowedFd<'_>> = match queue.messages.front() {
                Some((message, fds)) if message.sent == 0 => fds.iter().map(AsFd::as_fd).collect(),
                _ => Vec::new(),
            };
            let mut parts: Vec<&[u8]> = Vec::with_capacity(2 * MOST);
            for (at, (message, more_fds)) in queue.messages.iter().take(MOST).enumerate() {
                // Another message's descriptors begin a write of their own.
                if at > 0 && !more_fds.is_empty() {
                    break;
                }
                let (header, payload) = message.unsent();
                parts.extend([header, payload]);
            }
            let sent = match send_parts(self.stream.get_ref(), &parts, &fds) {
                Ok(sent) => sent,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(err) => return Err(err),
            };
            drop(parts);
            drop(fds);
            self.queued.fetch_sub(sent as u64, std::sync::atomic::Ordering::Relaxed);
            let mut left = sent;
            while left > 0 {
                let (message, _) = queue.messages.front_mut().expect("bytes sent of a message");
                let taken = (message.left() as usize).min(left);
                message.sent += taken;
                left -= taken;
                if message.left() == 0 {
                    queue.messages.pop_front();
                }
            }
        }
        Ok(true)
    }

    /// Writes the messages queued as the socket takes them, until none is left, or it fails.
    async fn drain(self: std::sync::Arc<Self>) {
        loop {
            let mut ready = match self.stream.writable().await {
                Ok(ready) => ready,
                Err(err) => return self.fail(err),
            };
            let mut queue = self.queue.lock().unwrap();
            match self.send_queued(&mut queue) {
                Ok(true) => {
                    queue.draining = false;
                    return;
                }
                Ok(false) => {}
                Err(err) => {
                    drop(queue);
                    return self.fail(err);
                }
            }
            ready.clear_ready();
        }
    }

    /// The socket failed with `err`: what is queued goes, and nothing is sent from now on.
    fn fail(&self, err: io::Error) {
        log::warn!("extentd: sending to a VM process: {err}");
        let mut queue = self.queue.lock().unwrap();
        queue.failed = Some(err.kind());
        queue.draining = false;
        let dropped = std::mem::take(&mut queue.messages);
        self.queued.store(0, std::sync::atomic::Ordering::Relaxed);
        drop(queue);
        drop(dropped);
    }

    /// Shuts the connection down, which ends its receiver's messages: a failure logged, but for
    /// the connection's being gone already (`ENOTCONN`).
    pub fn shutdown(&self) {
        if let Err(err) = self.stream.get_ref().shutdown(std::net::Shutdown::Both)
            && err.kind() != io::ErrorKind::NotConnected
        {
            log::warn!("extentd: shutting a VM process's connection down: {err}");
        }
    }
}

/// Whether the other end of the stream `fd` hung up, though its receiving end may not have read so
/// yet.
pub(crate) fn hung_up(fd: BorrowedFd<'_>) -> bool {
    let mut poll = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLRDHUP, revents: 0 };
    // SAFETY: Polls a descriptor the caller keeps open, without waiting.
    let ready = unsafe { libc::poll(&mut poll, 1, 0) };
    ready > 0 && poll.revents & (libc::POLLRDHUP | libc::POLLHUP | libc::POLLERR) != 0
}

/// Waits until `stream` takes more bytes.
fn wait_writable(stream: &UnixStream) -> io::Result<()> {
    let mut poll = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
    loop {
        // SAFETY: Polls the stream's descriptor, which it keeps open.
        if unsafe { libc::poll(&mut poll, 1, -1) } >= 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// Writes all of `bytes` to `stream`, blocking or not, waiting for room as it fills.
fn write_all(stream: &UnixStream, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match (&*stream).write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => bytes = &bytes[written..],
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => wait_writable(stream)?,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Sends what of `bytes` one `sendmsg` does, with `fds`: how many bytes it sent, at least one.
fn send_with_fds(stream: &UnixStream, bytes: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<usize> {
    send_parts(stream, &[bytes], fds)
}

/// Sends what of `parts`, in order, one `sendmsg` does, with `fds`: how many bytes it sent, at
/// least one.
fn send_parts(stream: &UnixStream, parts: &[&[u8]], fds: &[BorrowedFd<'_>]) -> io::Result<usize> {
    let mut iov: Vec<libc::iovec> = (parts.iter())
        .map(|part| libc::iovec { iov_base: part.as_ptr() as *mut _, iov_len: part.len() })
        .collect();
    let len = (fds.len() * size_of::<RawFd>()) as u32;
    // SAFETY: CMSG_SPACE only computes a size.
    let space = unsafe { libc::CMSG_SPACE(len) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: A zeroed msghdr is a valid empty one.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = iov.as_mut_ptr();
    msg.msg_iovlen = iov.len() as _;
    if !fds.is_empty() {
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        // SAFETY: `msg`'s control buffer has room for one header and the descriptors.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(len) as _;
            let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
            for (index, fd) in fds.iter().enumerate() {
                std::ptr::write_unaligned(data.add(index), fd.as_raw_fd());
            }
        }
    }
    loop {
        // SAFETY: `msg` points at live buffers for the call.
        let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) };
        if sent >= 0 {
            return Ok(sent as usize);
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// The receiving half of a connection, which one thread receives messages through. fxdax: it
/// reads what the socket holds, many messages at once, rather than a message's header and payload
/// each a read (TASK-91.16): a VM process's stage operations come in runs, a window change's
/// together. The descriptors that come with what it reads it keeps, in order, for the messages
/// that say they carry them ([`Header::fds`]).
pub(crate) struct Receiver {
    stream: UnixStream,
    /// What it read and has not received yet: `buffer[start..end]`.
    buffer: Box<[u8]>,
    start: usize,
    end: usize,
    /// The descriptors that came with what it read, for the messages not received yet.
    fds: std::collections::VecDeque<OwnedFd>,
}

/// fxdax: the bytes a [`Receiver`] reads at most at once.
const RECEIVE_BYTES: usize = 64 << 10;

impl Receiver {
    pub fn new(stream: UnixStream) -> Self {
        let buffer = vec![0; RECEIVE_BYTES].into_boxed_slice();
        Self { stream, buffer, start: 0, end: 0, fds: Default::default() }
    }

    /// fxdax: takes what follows the stage unmap `header`, already waiting whole in the socket, of
    /// the window's change it begins (TASK-91.16): first the acknowledged unmaps adjacent to it or
    /// overlapping it, which it joins, then the window's mapping anew of the range they unmap
    /// ([`STAGE_MAPPED`]) and its pages ([`STAGE_MAP`]), each within it, which it returns, in
    /// order, for the VM process to make with the unmap at once. Up to 63 messages, without
    /// crossing another, a gap, or partial input. The unmap's ID becomes the largest of those it
    /// joined: never a later map's, as operations taking access away that haven't reached the
    /// wire yet may own the IDs between, whose pages kept must stay quarantined.
    pub fn take_change(&mut self, header: &mut Header) -> io::Result<Vec<Header>> {
        let is_unmap = |h: &Header| {
            h.kind == STAGE_UNMAP
                && h.flags == STAGE_ACK
                && h.status == 0
                && h.fds == 0
                && h.args[1] != 0
        };
        if !is_unmap(header) || header.len != 0 {
            return Ok(Vec::new());
        }
        let (mut start, Some(mut end)) =
            (header.args[0], header.args[0].checked_add(header.args[1]))
        else {
            return Ok(Vec::new());
        };
        let mut id = header.id;
        let mut anew = false;
        let taken = self.take_waiting(|next| {
            let len = match next.kind {
                STAGE_UNMAP if !anew && is_unmap(next) => {
                    let Some(next_end) = next.args[0].checked_add(next.args[1]) else {
                        return false;
                    };
                    if next.args[0] > end || next_end < start {
                        return false;
                    }
                    (start, end) = (start.min(next.args[0]), end.max(next_end));
                    id = id.max(next.id);
                    return true;
                }
                STAGE_MAPPED => next.args[1],
                STAGE_MAP => next.args[2],
                _ => return false,
            };
            anew = true;
            next.flags == 0
                && next.status == 0
                && next.fds == 0
                && len != 0
                && next.args[0] >= start
                && next.args[0].checked_add(len).is_some_and(|next_end| next_end <= end)
        })?;
        header.id = id;
        header.args[0] = start;
        header.args[1] = end - start;
        Ok(taken.into_iter().filter(|taken| taken.kind != STAGE_UNMAP).collect())
    }

    /// fxdax: receives the messages already read or waiting whole in the socket, up to 63, as long
    /// as `take` takes each, in order, none with a payload or descriptors: a read of the socket's,
    /// at most, that waits for nothing. Bounded, so that what is made of them together, and
    /// acknowledged once all is, waits for no more.
    fn take_waiting(&mut self, mut take: impl FnMut(&Header) -> bool) -> io::Result<Vec<Header>> {
        const MOST: usize = 63;
        self.read(libc::MSG_DONTWAIT)?;
        let mut taken = Vec::new();
        while taken.len() < MOST
            && let Some(next) = self.buffered_header()
        {
            if next.len != 0 || next.fds != 0 || !take(&next) {
                break;
            }
            self.start += size_of::<Header>();
            taken.push(next);
        }
        Ok(taken)
    }

    /// The next message, or `None` once the other end has closed the connection: one whose payload
    /// is past [`MAX_PAYLOAD`] only if `most`, asked then, allows that many bytes, as a reply to
    /// [`MAPPINGS`] of a large window may hold (TASK-79.43).
    pub fn receive(&mut self, most: impl FnOnce() -> u64) -> io::Result<Option<Message>> {
        let header = loop {
            if let Some(header) = self.buffered_header() {
                break header;
            }
            if self.read(0)? == 0 {
                return match self.start == self.end {
                    true => Ok(None),
                    false => Err(io::ErrorKind::UnexpectedEof.into()),
                };
            }
        };
        self.start += size_of::<Header>();
        if header.len > MAX_PAYLOAD + size_of::<Header>() as u64 && header.len > most() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a message too large"));
        }
        // Its descriptors came with its first bytes, which were read.
        let carried = header.fds as usize;
        if carried > MAX_FDS || carried > self.fds.len() {
            let what = format!("a message of {carried} descriptors, {} received", self.fds.len());
            return Err(io::Error::new(io::ErrorKind::InvalidData, what));
        }
        let fds = self.fds.drain(..carried).collect();
        let mut payload = vec![0; header.len as usize];
        let buffered = payload.len().min(self.end - self.start);
        payload[..buffered].copy_from_slice(&self.buffer[self.start..self.start + buffered]);
        self.start += buffered;
        // The rest straight from the socket: no more than the payload, which carries no
        // descriptors, as they come with a message's first bytes.
        self.stream.read_exact(&mut payload[buffered..])?;
        Ok(Some(Message { header, payload, fds }))
    }

    /// The next header, if what was read holds it whole.
    fn buffered_header(&self) -> Option<Header> {
        let bytes = self.buffer.get(self.start..self.start + size_of::<Header>())?;
        (self.start + size_of::<Header>() <= self.end)
            .then(|| Header::read_from_bytes(bytes).unwrap())
    }

    /// Reads what the socket holds into the buffer, after what it holds already, moved to its
    /// start, with the descriptors that come with it, with `flags` (`MSG_DONTWAIT`, or none to wait
    /// for some): how many bytes, 0 at the end of the stream, or with nothing waiting.
    fn read(&mut self, flags: libc::c_int) -> io::Result<usize> {
        self.buffer.copy_within(self.start..self.end, 0);
        (self.start, self.end) = (0, self.end - self.start);
        if self.end == self.buffer.len() {
            return Ok(0);
        }
        match receive_with_fds(&self.stream, &mut self.buffer[self.end..], flags) {
            Ok((read, fds)) => {
                self.end += read;
                self.fds.extend(fds);
                Ok(read)
            }
            Err(err)
                if err.kind() == io::ErrorKind::WouldBlock && flags & libc::MSG_DONTWAIT != 0 =>
            {
                Ok(0)
            }
            Err(err) => Err(err),
        }
    }
}

/// fxdax: the receiving half of one of the daemon's connections, on a runtime, which receives every
/// connection's messages on a few threads rather than a thread each (TASK-34): its socket
/// non-blocking, read as it is ready.
pub(crate) struct AsyncReceiver(tokio::io::unix::AsyncFd<UnixStream>);

impl AsyncReceiver {
    /// Receives from `stream`, which it makes non-blocking, as the runtime it is made on is ready
    /// to.
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self(tokio::io::unix::AsyncFd::new(stream)?))
    }

    /// The next message, or `None` once the other end has closed the connection, as
    /// [`Receiver::receive`] gives it, a payload of [`MAX_PAYLOAD`] at most: if `admit`, given its
    /// header, admits it, before its payload is read (TASK-79.16).
    pub async fn receive(
        &mut self,
        admit: impl FnOnce(&Header) -> io::Result<()>,
    ) -> io::Result<Option<Message>> {
        let mut header = Header::default();
        let mut fds = Vec::new();
        {
            let bytes = header.as_mut_bytes();
            let mut at = 0;
            while at < bytes.len() {
                // The descriptors come with the message's first bytes.
                let (received, mut more) = self.read(&mut bytes[at..], at == 0).await?;
                if received == 0 {
                    if at == 0 {
                        return Ok(None);
                    }
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                fds.append(&mut more);
                at += received;
            }
        }
        if header.len > MAX_PAYLOAD + size_of::<Header>() as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a message too large"));
        }
        admit(&header)?;
        let mut payload = vec![0; header.len as usize];
        let mut at = 0;
        while at < payload.len() {
            match self.read(&mut payload[at..], false).await? {
                (0, _) => return Err(io::ErrorKind::UnexpectedEof.into()),
                (received, _) => at += received,
            }
        }
        Ok(Some(Message { header, payload, fds }))
    }

    /// Reads what of `bytes` the socket has, once it has any: with the descriptors that came with
    /// them, if `with_fds`.
    async fn read(&self, bytes: &mut [u8], with_fds: bool) -> io::Result<(usize, Vec<OwnedFd>)> {
        loop {
            let mut ready = self.0.readable().await?;
            let read = ready.try_io(|stream| match with_fds {
                true => receive_with_fds(stream.get_ref(), bytes, 0),
                false => (&*stream.get_ref()).read(bytes).map(|read| (read, Vec::new())),
            });
            match read {
                Ok(read) => return read,
                // Not ready after all: waited for again.
                Err(_) => continue,
            }
        }
    }
}

/// Receives what of `bytes` one `recvmsg` does, and the descriptors that came with them.
fn receive_with_fds(
    stream: &UnixStream,
    bytes: &mut [u8],
    flags: libc::c_int,
) -> io::Result<(usize, Vec<OwnedFd>)> {
    let mut iov = libc::iovec { iov_base: bytes.as_mut_ptr().cast(), iov_len: bytes.len() };
    // SAFETY: CMSG_SPACE only computes a size.
    let space = unsafe { libc::CMSG_SPACE((MAX_FDS * size_of::<RawFd>()) as u32) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: A zeroed msghdr is a valid empty one.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    let received = loop {
        // SAFETY: `msg` points at live buffers for the call.
        let received =
            unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC | flags) };
        if received >= 0 {
            break received as usize;
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    };
    let mut fds = Vec::new();
    // SAFETY: The kernel filled in `msg`'s control buffer, which the macros walk.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let header = libc::CMSG_LEN(0) as usize;
                let count = ((*cmsg).cmsg_len as usize - header) / size_of::<RawFd>();
                let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                for index in 0..count {
                    let raw = std::ptr::read_unaligned(data.add(index));
                    fds.push(OwnedFd::from_raw_fd(raw));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    Ok((received, fds))
}

/// A connected pair of a sender and a receiver of each end, for tests.
#[cfg(test)]
pub(crate) fn pair() -> ((Sender, Receiver), (Sender, Receiver)) {
    let (a, b) = UnixStream::pair().unwrap();
    let ends =
        |stream: UnixStream| (Sender::new(stream.try_clone().unwrap()), Receiver::new(stream));
    (ends(a), ends(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    fn unmap(id: u64, offset: u64, len: u64) -> Header {
        Header::new(STAGE_UNMAP, id).with_flags(STAGE_ACK).with_args(&[offset, len])
    }

    #[test]
    fn unmaps_join_adjacent_ranges_and_ack_only_consumed_taking_ids() {
        let ((sender, _), (_, mut receiver)) = pair();
        // Includes reversed address order, overlap, and IDs allocated in a different order.
        for h in [
            unmap(7, 8192, 4096),
            unmap(3, 4096, 4096),
            unmap(5, 4096, 8192),
            unmap(6, 12288, 4096),
        ] {
            sender.send(h, &[], &[]).unwrap();
        }
        let mapped = Header::new(STAGE_MAPPED, 99).with_args(&[4096, 12288]);
        sender.send(mapped, &[], &[]).unwrap();
        let reply = Header::new(REPLY, 100);
        let file = tempfile::tempfile().unwrap();
        sender.send(reply, &[], &[file.as_fd()]).unwrap();
        let mut h = receiver.receive(|| 0).unwrap().unwrap().header;
        assert_eq!(receiver.take_change(&mut h).unwrap(), [mapped]);
        assert_eq!(h, unmap(7, 4096, 12288));
        let message = receiver.receive(|| 0).unwrap().unwrap();
        assert_eq!(message.header, Header { fds: 1, ..reply }, "the header says its descriptor");
        assert_eq!(message.fds.len(), 1);
    }

    /// fxdax: an unmap takes the window's mapping anew of what it unmaps, and the maps of its
    /// pages, each within it, in order, up to the first that isn't (TASK-91.16): one outside it,
    /// one of another kind, or an unmap after them, which begins the next change.
    #[test]
    fn unmaps_take_the_maps_within_them() {
        let map =
            |id, offset, len| Header::new(STAGE_MAP, id).with_args(&[offset, 1 << 20, len, 1, 0]);
        let mapped = Header::new(STAGE_MAPPED, 2).with_args(&[4096, 8192]);
        for (after, taken) in [
            (map(5, 8192, 8192), 3),
            (unmap(5, 4096, 4096), 3),
            (Header::new(STAGE_PROTECT, 5).with_args(&[4096, 4096, 2]), 3),
            (map(5, 4096, 8192), 4),
        ] {
            let ((sender, _), (_, mut receiver)) = pair();
            let sent = [mapped, map(3, 4096, 4096), map(4, 8192, 4096), after];
            for h in sent {
                sender.send(h, &[], &[]).unwrap();
            }
            let mut first = unmap(1, 4096, 8192);
            assert_eq!(receiver.take_change(&mut first).unwrap(), sent[..taken]);
            assert_eq!(first, unmap(1, 4096, 8192), "the unmap's ID is never a map's");
            if taken < sent.len() {
                assert_eq!(receiver.receive(|| 0).unwrap().unwrap().header, after);
            }
        }
    }

    #[test]
    fn unmap_batches_leave_gaps_other_operations_and_partial_headers_untouched() {
        for variation in 0..6 {
            let ((sender, _), (_, mut receiver)) = pair();
            let mut first = unmap(1, 4096, 4096);
            assert_eq!(receiver.take_change(&mut first).unwrap(), Vec::<Header>::new());
            let mut next = unmap(2, 8192, 4096);
            match variation {
                0 => next.args[0] += 4096,
                1 => {
                    next.kind = STAGE_PROTECT;
                    next.args[2] = 1;
                }
                2 => next.flags = 0,
                3 => next.args = [u64::MAX - 1024, 4096, 0, 0, 0],
                4 => {}
                5 => next.len = 3,
                _ => unreachable!(),
            }
            if variation == 4 {
                write_all(&sender.0.lock().unwrap(), &next.as_bytes()[..17]).unwrap();
                assert_eq!(receiver.take_change(&mut first).unwrap(), Vec::<Header>::new());
                assert_eq!(first, unmap(1, 4096, 4096));
                write_all(&sender.0.lock().unwrap(), &next.as_bytes()[17..]).unwrap();
                assert_eq!(receiver.take_change(&mut first).unwrap(), Vec::<Header>::new());
                assert_eq!(first, unmap(2, 4096, 8192));
            } else {
                let payload = vec![9; next.len as usize];
                sender.send(next, &payload, &[]).unwrap();
                assert_eq!(receiver.take_change(&mut first).unwrap(), Vec::<Header>::new());
                assert_eq!(first, unmap(1, 4096, 4096));
                let received = receiver.receive(|| 0).unwrap().unwrap();
                assert_eq!((received.header, received.payload), (next, payload));
            }
        }
    }

    #[test]
    fn unmap_batches_bound_the_work_taken_from_the_socket() {
        let ((sender, _), (_, mut receiver)) = pair();
        for id in 1..=128 {
            sender.send(unmap(id, id * 4096, 4096), &[], &[]).unwrap();
        }
        let mut next_id = 1;
        let mut total_joined = 0;
        while next_id <= 128 {
            let mut header = receiver.receive(|| 0).unwrap().unwrap().header;
            assert_eq!(header.id, next_id);
            assert_eq!(receiver.take_change(&mut header).unwrap(), Vec::<Header>::new());
            let count = header.args[1] / 4096;
            assert!(count <= 64, "{count} unmaps joined");
            assert_eq!(header, unmap(next_id + count - 1, next_id * 4096, count * 4096));
            total_joined += count - 1;
            next_id += count;
        }
        assert!(total_joined > 0);
    }

    /// Messages arrive whole and in order, with their payloads, and descriptors sent with one
    /// arrive with it.
    #[test]
    fn messages_carry_payloads_and_descriptors() {
        let ((sender, _), (_, mut receiver)) = pair();
        let (file, other) = (tempfile::tempfile().unwrap(), tempfile::tempfile().unwrap());
        let big = vec![7u8; 3 << 20];
        let sent = std::thread::spawn(move || {
            sender.send(Header::new(READ, 1).with_args(&[1, 2, 3]), b"", &[]).unwrap();
            let fds = [file.as_fd(), other.as_fd()];
            sender.send(Header::new(WRITE, 2).with_flags(5), &big, &fds).unwrap();
        });
        let first = receiver.receive(|| 0).unwrap().unwrap();
        assert_eq!(
            first.header,
            Header { kind: READ, id: 1, args: [1, 2, 3, 0, 0], ..Default::default() }
        );
        assert!(first.payload.is_empty() && first.fds.is_empty());
        let second = receiver.receive(|| 0).unwrap().unwrap();
        assert_eq!((second.header.kind, second.header.id, second.header.flags), (WRITE, 2, 5));
        assert_eq!(second.payload, vec![7u8; 3 << 20]);
        assert_eq!(second.fds.len(), 2, "the descriptors didn't arrive");
        sent.join().unwrap();
    }

    /// fxdax: the daemon's sends wait for no room (TASK-79.9): what its socket has none for
    /// queues, and arrives as the other end reads, whole and in order, with its descriptors; once
    /// the other end is gone, sends fail.
    #[test]
    fn async_sends_queue_what_the_socket_has_no_room_for() {
        let runtime =
            tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build();
        let runtime = runtime.unwrap();
        let (ours, theirs) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        let sender = std::sync::Arc::new({
            let _entered = runtime.enter();
            AsyncSender::new(ours).unwrap()
        });
        let file = tempfile::tempfile().unwrap();
        // Many times what the socket holds, none read meanwhile.
        let big = vec![7u8; 4 << 20];
        for id in 0..8 {
            let fds = if id == 5 { vec![file.as_fd()] } else { Vec::new() };
            sender.send(Header::new(READ, id), big.clone(), &fds).unwrap();
        }
        let mut receiver = Receiver::new(theirs);
        for id in 0..8 {
            let message = receiver.receive(|| 0).unwrap().unwrap();
            assert_eq!((message.header.id, message.payload.len()), (id, big.len()));
            assert!(message.payload == big, "message {id} holds other bytes");
            assert_eq!(message.fds.len(), usize::from(id == 5), "message {id}'s descriptors");
        }
        drop(receiver);
        let gone = sender.send(Header::new(READ, 8), Vec::new(), &[]);
        assert_eq!(gone.map_err(|err| err.kind()), Err(io::ErrorKind::BrokenPipe));
    }

    /// fxdax: what a thread sends corked waits until it uncorks, then arrives, in order
    /// (TASK-91.16); another thread's send meanwhile sends what waits first, then its own, as
    /// corking delays none of its sends; a message's descriptors arrive with it, whatever was
    /// sent with it.
    #[test]
    fn corked_sends_wait_for_uncorking_or_another_send() {
        let runtime =
            tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build();
        let runtime = runtime.unwrap();
        let (ours, theirs) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        let sender = std::sync::Arc::new({
            let _entered = runtime.enter();
            AsyncSender::new(ours).unwrap()
        });
        let waiting = |stream: &UnixStream| {
            let mut byte = [0u8; 1];
            // SAFETY: Peeks a byte into `byte`, without waiting.
            let got = unsafe {
                libc::recv(
                    stream.as_raw_fd(),
                    byte.as_mut_ptr().cast(),
                    1,
                    libc::MSG_PEEK | libc::MSG_DONTWAIT,
                )
            };
            got > 0
        };
        let mut receiver = Receiver::new(theirs.try_clone().unwrap());
        let mut ids = |count: usize| -> Vec<(u64, usize)> {
            (0..count)
                .map(|_| {
                    let message = receiver.receive(|| 0).unwrap().unwrap();
                    (message.header.id, message.fds.len())
                })
                .collect()
        };
        sender.cork();
        sender.send(Header::new(STAGE_UNMAP, 1), Vec::new(), &[]).unwrap();
        sender.send(Header::new(STAGE_MAP, 2), Vec::new(), &[]).unwrap();
        assert!(!waiting(&theirs), "a corked send arrived");
        let other = sender.clone();
        std::thread::spawn(move || other.send(Header::new(REPLY, 3), Vec::new(), &[]).unwrap())
            .join()
            .unwrap();
        assert_eq!(ids(3), [(1, 0), (2, 0), (3, 0)]);
        sender.cork();
        sender.send(Header::new(STAGE_MAP, 4), Vec::new(), &[]).unwrap();
        let file = tempfile::tempfile().unwrap();
        sender.send(Header::new(REPLY, 5), Vec::new(), &[file.as_fd()]).unwrap();
        sender.send(Header::new(STAGE_MAP, 6), Vec::new(), &[]).unwrap();
        sender.uncork();
        assert!(!waiting(&theirs), "a send arrived while corked");
        sender.uncork();
        assert_eq!(ids(3), [(4, 0), (5, 1), (6, 0)]);
        assert!(!AsyncSender::corked());
    }

    /// fxdax: the extents a `FREE_MANY`'s payload names are those put in it, in order; one that
    /// holds part of an ID is refused (TASK-92.13).
    #[test]
    fn free_many_payloads_name_their_extents() {
        let extents = [
            crate::ExtentId { branch: 1, sequence: 2 },
            crate::ExtentId { branch: u64::MAX, sequence: 1 << 40 },
        ];
        let payload = extent_ids_payload(&extents);
        assert_eq!(payload.len(), 2 * EXTENT_ID_BYTES);
        assert_eq!(extent_ids(&payload).unwrap().collect::<Vec<_>>(), extents);
        assert_eq!(extent_ids(&payload[..EXTENT_ID_BYTES + 8]).err(), Some(crate::Errno::EINVAL));
    }
}
