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
    pub reserved: u32,
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
/// from a snapshot, whose store point that is). That point, if it doesn't exist either, is taken
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
/// Acknowledges stage operation `id`, which the VM process has made.
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
/// the reply's payload, each six little-endian words: its offset in the window, its length, the
/// extent's branch and sequence and the offset in it, and [`MAPPING_WRITABLE`] or
/// [`MAPPING_ZEROS`].
pub(crate) const MAPPINGS: u32 = 17;
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
/// The most bytes a transfer area holds.
pub(crate) const MAX_TRANSFER_AREA: u64 = 64 << 20;

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
}

impl Queued {
    /// Sends what of it `stream` takes, `fds` with its first bytes: whether it is all sent.
    fn send(&mut self, stream: &UnixStream, fds: &[BorrowedFd<'_>]) -> io::Result<bool> {
        let len = self.header.len() + self.payload.len();
        while self.sent < len {
            let (header, payload) = match self.sent.checked_sub(self.header.len()) {
                None => (&self.header[self.sent..], &self.payload[..]),
                Some(at) => (&[][..], &self.payload[at..]),
            };
            let fds = if self.sent == 0 { fds } else { &[] };
            match send_parts(stream, [header, payload], fds) {
                Ok(sent) => self.sent += sent,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(err) => return Err(err),
            }
        }
        Ok(true)
    }
}

impl AsyncSender {
    /// Sends through `stream`, which the caller made non-blocking, on the runtime it is made on.
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        let stream =
            tokio::io::unix::AsyncFd::with_interest(stream, tokio::io::Interest::WRITABLE)?;
        let runtime = tokio::runtime::Handle::current();
        Ok(Self { stream, queue: Mutex::default(), runtime })
    }

    /// Whether the other end hung up, as a VM process that went does, though the receiving end
    /// may not have read so yet.
    pub fn peer_gone(&self) -> bool {
        let fd = self.stream.get_ref().as_raw_fd();
        let mut poll = libc::pollfd { fd, events: libc::POLLRDHUP, revents: 0 };
        // SAFETY: Polls the stream's descriptor, which it keeps open, without waiting.
        let ready = unsafe { libc::poll(&mut poll, 1, 0) };
        ready > 0 && poll.revents & (libc::POLLRDHUP | libc::POLLHUP | libc::POLLERR) != 0
    }

    /// Sends `header`, with `payload` as its payload, and `fds`, at most [`MAX_FDS`], after what
    /// was sent before: at once, what the socket takes, and the rest as it takes it. Fails once
    /// the socket failed, as the other end gone.
    pub fn send(
        self: &std::sync::Arc<Self>,
        mut header: Header,
        payload: Vec<u8>,
        fds: &[BorrowedFd<'_>],
    ) -> io::Result<()> {
        assert!(fds.len() <= MAX_FDS, "too many descriptors for a message");
        header.len = payload.len() as u64;
        let header = header.as_bytes().try_into().unwrap();
        let mut message = Queued { header, payload, sent: 0 };
        let mut queue = self.queue.lock().unwrap();
        if let Some(failed) = queue.failed {
            return Err(failed.into());
        }
        if queue.messages.is_empty() {
            match message.send(self.stream.get_ref(), fds) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(err) => {
                    queue.failed = Some(err.kind());
                    return Err(err);
                }
            }
        }
        // The descriptors, unless sent with its first bytes, are the queue's until it sends them.
        let fds = match message.sent {
            0 => fds.iter().map(|fd| fd.try_clone_to_owned()).collect::<io::Result<_>>()?,
            _ => Vec::new(),
        };
        queue.messages.push_back((message, fds));
        if !queue.draining {
            queue.draining = true;
            self.runtime.spawn(self.clone().drain());
        }
        Ok(())
    }

    /// Writes the messages queued as the socket takes them, until none is left, or it fails.
    async fn drain(self: std::sync::Arc<Self>) {
        loop {
            let mut ready = match self.stream.writable().await {
                Ok(ready) => ready,
                Err(err) => return self.fail(err),
            };
            let mut queue = self.queue.lock().unwrap();
            while let Some((message, fds)) = queue.messages.front_mut() {
                let fds: Vec<BorrowedFd<'_>> = fds.iter().map(AsFd::as_fd).collect();
                match message.send(self.stream.get_ref(), &fds) {
                    Ok(true) => drop(queue.messages.pop_front()),
                    Ok(false) => break,
                    Err(err) => {
                        drop(queue);
                        return self.fail(err);
                    }
                }
            }
            if queue.messages.is_empty() {
                queue.draining = false;
                return;
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
        drop(queue);
        drop(dropped);
    }

    /// Shuts the connection down, which ends its receiver's messages.
    pub fn shutdown(&self) {
        let _ = self.stream.get_ref().shutdown(std::net::Shutdown::Both);
    }
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
    send_parts(stream, [bytes, &[]], fds)
}

/// Sends what of `parts`, in order, one `sendmsg` does, with `fds`: how many bytes it sent, at
/// least one.
fn send_parts(stream: &UnixStream, parts: [&[u8]; 2], fds: &[BorrowedFd<'_>]) -> io::Result<usize> {
    let mut iov =
        parts.map(|part| libc::iovec { iov_base: part.as_ptr() as *mut _, iov_len: part.len() });
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

/// The receiving half of a connection, which one thread receives messages through.
pub(crate) struct Receiver(UnixStream);

impl Receiver {
    pub fn new(stream: UnixStream) -> Self {
        Self(stream)
    }

    /// The next message, or `None` once the other end has closed the connection.
    pub fn receive(&mut self) -> io::Result<Option<Message>> {
        let mut header = Header::default();
        let bytes = header.as_mut_bytes();
        let (received, fds) = receive_with_fds(&self.0, bytes)?;
        if received == 0 {
            return Ok(None);
        }
        self.0.read_exact(&mut bytes[received..])?;
        if header.len > MAX_PAYLOAD + size_of::<Header>() as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a message too large"));
        }
        let mut payload = vec![0; header.len as usize];
        self.0.read_exact(&mut payload)?;
        Ok(Some(Message { header, payload, fds }))
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
    /// [`Receiver::receive`] gives it.
    pub async fn receive(&mut self) -> io::Result<Option<Message>> {
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
                true => receive_with_fds(stream.get_ref(), bytes),
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
fn receive_with_fds(stream: &UnixStream, bytes: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
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
            unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
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
        let first = receiver.receive().unwrap().unwrap();
        assert_eq!(
            first.header,
            Header { kind: READ, id: 1, args: [1, 2, 3, 0, 0], ..Default::default() }
        );
        assert!(first.payload.is_empty() && first.fds.is_empty());
        let second = receiver.receive().unwrap().unwrap();
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
            let message = receiver.receive().unwrap().unwrap();
            assert_eq!((message.header.id, message.payload.len()), (id, big.len()));
            assert!(message.payload == big, "message {id} holds other bytes");
            assert_eq!(message.fds.len(), usize::from(id == 5), "message {id}'s descriptors");
        }
        drop(receiver);
        let gone = sender.send(Header::new(READ, 8), Vec::new(), &[]);
        assert_eq!(gone.map_err(|err| err.kind()), Err(io::ErrorKind::BrokenPipe));
    }
}
