// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The messages between a VM process and the extent daemon, and their framing on a Unix stream.
//!
//! Each message is a [`Header`], followed by `len` bytes of payload. A message may carry one file
//! descriptor, as `SCM_RIGHTS`, with its first bytes: the daemon's reply to [`OPEN`] carries its
//! kernel's physical memory.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
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
/// memory.
pub(crate) const OPEN: u32 = 1;
/// Reads `args[3]` bytes at `args[2]` in extent `args[0..2]` (`READ`): the reply's payload.
pub(crate) const READ: u32 = 2;
/// Writes the payload at `args[2]` in extent `args[0..2]` (`WRITE`), after a barrier if
/// [`WRITE_BARRIER`], and followed by a commit if [`WRITE_COMMIT`].
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
/// is a new directory at the path the payload holds ([`ExtentStore::fork`](crate::ExtentStore)):
/// a fork the daemon serves, sharing its extents' pages with the store's. With
/// [`FORK_QUIESCENT`], it fails with `EBUSY` unless nothing is open
/// ([`ExtentStore::is_quiescent`](crate::ExtentStore::is_quiescent)): the point a snapshot of
/// the VM is taken at.
pub(crate) const FORK: u32 = 13;
/// The daemon's counters ([`Stats`](crate::stats::Stats)): the reply's payload, as little-endian
/// words in their fields' order. It needs no store open.
pub(crate) const STATS: u32 = 14;
/// Sets the capacity of the store at the path the payload holds to `args[0]` bytes
/// (`fxdaxctl resize`). It needs no store open. A store a VM process has open grows at once,
/// and its VM process is told ([`CAPACITY`]); it shrinks once its guest, asked, takes the smaller
/// size ([`RESIZED`]). A store no VM process has open grows; it shrinks only with its guest.
pub(crate) const RESIZE: u32 = 15;
/// The guest's answer to a [`CAPACITY`] that asked it to shrink to `args[0]` bytes: whether it
/// took that size, `args[1]` 1, or not, 0.
pub(crate) const RESIZED: u32 = 16;
/// The window's mappings ([`ExtentStore::window_mappings`](crate::ExtentStore::window_mappings)):
/// the reply's payload, each six little-endian words: its offset in the window, its length, the
/// extent's branch and sequence and the offset in it, and [`MAPPING_WRITABLE`] or
/// [`MAPPING_ZEROS`].
pub(crate) const MAPPINGS: u32 = 17;

pub(crate) const OPEN_CREATE: u32 = 1 << 0;
pub(crate) const OPEN_FORK_FROM: u32 = 1 << 1;
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
pub(crate) const FAULT_WRITE: u32 = 1 << 0;
pub(crate) const FAULT_PAGE: u32 = 1 << 1;

// From the daemon.

/// The reply to request `id`.
pub(crate) const REPLY: u32 = 100;
/// Maps `args[2]` bytes of the kernel's physical memory at `args[1]` into the window at `args[0]`,
/// with protection `args[3]` ([`Protection`](crate::Protection) as [`protection_code`]).
pub(crate) const STAGE_MAP: u32 = 101;
/// Changes the protection of `args[1]` bytes of the window at `args[0]` to `args[2]`.
pub(crate) const STAGE_PROTECT: u32 = 102;
/// Unmaps `args[1]` bytes of the window at `args[0]`.
pub(crate) const STAGE_UNMAP: u32 = 103;
/// The store's capacity is `args[0]` bytes now, which the guest takes ([`RESIZE`]); or, with
/// [`CAPACITY_ASK`], the guest is asked to shrink to `args[0]` bytes, and answers ([`RESIZED`]).
pub(crate) const CAPACITY: u32 = 104;
pub(crate) const CAPACITY_ASK: u32 = 1 << 0;
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

/// A message received.
pub(crate) struct Message {
    pub header: Header,
    pub payload: Vec<u8>,
    pub fd: Option<OwnedFd>,
}

/// The sending half of a connection, which any thread sends whole messages through.
pub(crate) struct Sender(Mutex<UnixStream>);

impl Sender {
    pub fn new(stream: UnixStream) -> Self {
        Self(Mutex::new(stream))
    }

    /// Whether the other end hung up, as a VM process that went does, though the receiving end
    /// may not have read so yet.
    pub fn peer_gone(&self) -> bool {
        use std::os::fd::AsRawFd;
        let stream = self.0.lock().unwrap();
        let mut poll = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLRDHUP, revents: 0 };
        // SAFETY: Polls the stream's descriptor, which it keeps open, without waiting.
        let ready = unsafe { libc::poll(&mut poll, 1, 0) };
        ready > 0 && poll.revents & (libc::POLLRDHUP | libc::POLLHUP | libc::POLLERR) != 0
    }

    /// Sends `header`, with `payload` as its payload, and `fd` if any.
    pub fn send(
        &self,
        mut header: Header,
        payload: &[u8],
        fd: Option<BorrowedFd<'_>>,
    ) -> io::Result<()> {
        header.len = payload.len() as u64;
        let mut stream = self.0.lock().unwrap();
        let header = header.as_bytes();
        let sent = send_with_fd(&stream, header, fd)?;
        stream.write_all(&header[sent..])?;
        stream.write_all(payload)
    }

    /// Shuts the connection down, which ends its receiver's messages.
    pub fn shutdown(&self) {
        let _ = self.0.lock().unwrap().shutdown(std::net::Shutdown::Both);
    }
}

/// Sends what of `bytes` one `sendmsg` does, with `fd` if any: how many bytes it sent, at least
/// one.
fn send_with_fd(
    stream: &UnixStream,
    bytes: &[u8],
    fd: Option<BorrowedFd<'_>>,
) -> io::Result<usize> {
    let mut iov = libc::iovec { iov_base: bytes.as_ptr() as *mut _, iov_len: bytes.len() };
    // SAFETY: CMSG_SPACE only computes a size.
    let space = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: A zeroed msghdr is a valid empty one.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if let Some(fd) = fd {
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        // SAFETY: `msg`'s control buffer has room for one header and one descriptor.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd.as_raw_fd());
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
        let (received, fd) = receive_with_fd(&self.0, bytes)?;
        if received == 0 {
            return Ok(None);
        }
        self.0.read_exact(&mut bytes[received..])?;
        if header.len > MAX_PAYLOAD + size_of::<Header>() as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a message too large"));
        }
        let mut payload = vec![0; header.len as usize];
        self.0.read_exact(&mut payload)?;
        Ok(Some(Message { header, payload, fd }))
    }
}

/// Receives what of `bytes` one `recvmsg` does, and the descriptor that came with them, if any.
fn receive_with_fd(stream: &UnixStream, bytes: &mut [u8]) -> io::Result<(usize, Option<OwnedFd>)> {
    let mut iov = libc::iovec { iov_base: bytes.as_mut_ptr().cast(), iov_len: bytes.len() };
    // SAFETY: CMSG_SPACE only computes a size.
    let space = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as usize;
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
    let mut fd = None;
    // SAFETY: The kernel filled in `msg`'s control buffer, which the macros walk.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let raw = std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>());
                fd = Some(OwnedFd::from_raw_fd(raw));
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    Ok((received, fd))
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

    /// Messages arrive whole and in order, with their payloads, and a descriptor sent with one
    /// arrives with it.
    #[test]
    fn messages_carry_payloads_and_descriptors() {
        let ((sender, _), (_, mut receiver)) = pair();
        let file = tempfile::tempfile().unwrap();
        let big = vec![7u8; 3 << 20];
        let sent = std::thread::spawn(move || {
            sender.send(Header::new(READ, 1).with_args(&[1, 2, 3]), b"", None).unwrap();
            sender.send(Header::new(WRITE, 2).with_flags(5), &big, Some(file.as_fd())).unwrap();
        });
        let first = receiver.receive().unwrap().unwrap();
        assert_eq!(
            first.header,
            Header { kind: READ, id: 1, args: [1, 2, 3, 0, 0], ..Default::default() }
        );
        assert!(first.payload.is_empty() && first.fd.is_none());
        let second = receiver.receive().unwrap().unwrap();
        assert_eq!((second.header.kind, second.header.id, second.header.flags), (WRITE, 2, 5));
        assert_eq!(second.payload, vec![7u8; 3 << 20]);
        assert!(second.fd.is_some(), "the descriptor didn't arrive");
        sent.join().unwrap();
    }
}
