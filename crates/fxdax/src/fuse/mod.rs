//! A FUSE server for virtio-fs devices, serving a [`Filesystem`].
//!
//! A device hands the server each request on its queue as the request's buffers, which the server
//! copies out of, and the reply's, which it copies into: [`Server::handle`]. For a virtio-fs
//! device these are a descriptor chain's readable and writable descriptors, in guest memory, as
//! [`VolatileSlice`]s and [`VolatileSliceMut`]s. File contents are copied straight between them and
//! the files' pages.
//!
//! Ported from libkrun's `vendor/libkrun/src/devices/src/virtio/fs/server.rs`, from crosvm's
//! `fuse` crate, and its fxdax backend, which adapted it to [`Filesystem`].
//!
//! Deviations:
//! - The server serves a [`Filesystem`], not any `FileSystem`, and its handlers are futures.
//! - Requests and replies are volatile slices rather than `Reader`s and `Writer`s of a descriptor
//!   chain, and replies are built whole before they are copied out, except for `READ`'s data.
//! - A request whose body is short or malformed is replied to with `EINVAL`, rather than dropped.
//! - Requests a [`Filesystem`] has no use for are replied to with `ENOSYS`: extended attributes,
//!   locks, `ACCESS`, `BMAP`, `IOCTL`, `POLL`, `LSEEK` and `COPY_FILE_RANGE`. INIT does not offer
//!   security contexts, submounts or idmapped mounts, whose requests carry what it would ignore.
//! - No snapshots, macOS sender or exit codes, which are libkrun's.

mod abi;

use crate::error::{Errno, Error, Result};
use crate::kernel::page_size;
use crate::memory::{VolatileSlice, VolatileSliceMut};
use crate::node::{Attr, DirHandle, FileType, Inode, Owner, RenameFlags, SetAttr, SetTime};
use crate::{DirEntry, Filesystem};
use abi::*;
use std::mem::size_of;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zerocopy::{FromBytes, IntoBytes};

/// The most a request may write, or a reply read.
const MAX_BUFFER_SIZE: u32 = 1 << 20;

/// How long the guest may cache names, the absence of names, and attributes. Only the guest
/// changes the filesystem, so they stay valid.
const TIMEOUT_SECS: u64 = 24 * 60 * 60;

/// A FUSE server for a virtio-fs device, serving `fs`. See the module docs.
#[derive(Debug)]
pub struct Server {
    fs: Filesystem,
}

impl Server {
    /// A server of `fs`.
    pub fn new(fs: Filesystem) -> Self {
        Self { fs }
    }

    /// The filesystem served, such as to create its DAX window.
    pub fn filesystem(&self) -> &Filesystem {
        &self.fs
    }

    /// The filesystem served, such as to close it once the device is done.
    pub fn into_filesystem(self) -> Filesystem {
        self.fs
    }

    /// Handles the request whose bytes are `request`, in turn, and copies its reply, if it has one,
    /// into `reply`, in turn. Returns the reply's length, which is 0 for requests that have none
    /// (`FORGET`, `BATCH_FORGET`, `INTERRUPT` and `DESTROY`).
    ///
    /// Requests may be handled concurrently, as the guest sends them. Fails if `request` is too
    /// short for a request's header, or `reply` too short for its reply, as then there is nothing
    /// to reply to the guest; a device should then fail the request as it fails a malformed one.
    pub async fn handle(
        &self,
        request: &[VolatileSlice<'_>],
        reply: &mut [VolatileSliceMut<'_>],
    ) -> Result<usize, Error> {
        let mut request = Reader::new(request);
        let header: InHeader =
            request.read_obj().ok_or(Error::msg("the request is shorter than its header"))?;
        let mut reply = Writer { bufs: reply, unique: header.unique };
        let body_len = (header.len as usize).checked_sub(size_of::<InHeader>());
        let Some(body_len) = body_len.filter(|&len| len <= request.remaining()) else {
            return reply.error(Errno::EINVAL);
        };
        request.limit(body_len);
        let reply_payload = match header.opcode {
            opcode::INIT => self.init(&mut request),
            opcode::DESTROY => {
                self.fs.unmount().await?;
                return Ok(0);
            }
            opcode::FORGET => {
                if let Some(ForgetIn { nlookup }) = request.read_obj() {
                    self.fs.forget(Inode(header.nodeid), nlookup);
                }
                return Ok(0);
            }
            opcode::BATCH_FORGET => {
                self.batch_forget(&mut request);
                return Ok(0);
            }
            // Requests are not interruptible: each completes, as Fxfs completes it.
            opcode::INTERRUPT => return Ok(0),
            opcode::READ => return self.read(&header, &mut request, &mut reply).await,
            opcode::LOOKUP => self.lookup(&header, &mut request).await,
            opcode::GETATTR => self.getattr(&header).await,
            opcode::SETATTR => self.setattr(&header, &mut request).await,
            opcode::READLINK => self.fs.readlink(Inode(header.nodeid)).await,
            opcode::SYMLINK => self.symlink(&header, &mut request).await,
            opcode::MKNOD => self.mknod(&header, &mut request).await,
            opcode::MKDIR => self.mkdir(&header, &mut request).await,
            opcode::UNLINK => self.unlink(&header, &mut request, false).await,
            opcode::RMDIR => self.unlink(&header, &mut request, true).await,
            opcode::RENAME => self.rename(&header, &mut request, false).await,
            opcode::RENAME2 => self.rename(&header, &mut request, true).await,
            opcode::LINK => self.link(&header, &mut request).await,
            opcode::CREATE => self.create(&header, &mut request).await,
            opcode::WRITE => self.write(&header, &mut request).await,
            opcode::STATFS => self.statfs().await,
            // Files have no handles: there is nothing to open, flush or release. Refused, OPEN and
            // FLUSH are not sent again, and nor is RELEASE once OPEN is not (see `init`).
            opcode::OPEN | opcode::FLUSH => Err(Errno::ENOSYS),
            opcode::RELEASE => Ok(Vec::new()),
            opcode::FSYNC | opcode::FSYNCDIR => {
                self.fs.fsync(Inode(header.nodeid)).await.map(|()| Vec::new())
            }
            opcode::OPENDIR => self.opendir(&header).await,
            opcode::READDIR => self.readdir(&mut request, reply.payload_capacity(), false).await,
            opcode::READDIRPLUS => self.readdir(&mut request, reply.payload_capacity(), true).await,
            opcode::RELEASEDIR => self.releasedir(&mut request),
            opcode::FALLOCATE => self.fallocate(&header, &mut request).await,
            opcode::SETUPMAPPING => self.setup_mapping(&header, &mut request).await,
            opcode::REMOVEMAPPING => self.remove_mapping(&mut request),
            _ => Err(Errno::ENOSYS),
        };
        match reply_payload {
            Ok(payload) => reply.ok(&payload),
            Err(errno) => reply.error(errno),
        }
    }

    fn init(&self, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let init: InitIn = request.read_obj().ok_or(Errno::EINVAL)?;
        let flags2 = if init.flags as u64 & INIT_EXT != 0 {
            request.read_obj::<InitInExt>().ok_or(Errno::EINVAL)?.flags2
        } else {
            0
        };
        if init.major < KERNEL_VERSION {
            log::error!("unsupported FUSE protocol version {}.{}", init.major, init.minor);
            return Err(Errno::EPROTO);
        }
        if init.major > KERNEL_VERSION {
            // The guest replies with a version of ours.
            let out = InitOut {
                major: KERNEL_VERSION,
                minor: KERNEL_MINOR_VERSION,
                ..Default::default()
            };
            return Ok(out.as_bytes().to_vec());
        }
        if init.minor < KERNEL_MINOR_VERSION {
            log::error!("unsupported FUSE protocol version {}.{}", init.major, init.minor);
            return Err(Errno::EPROTO);
        }
        // Only the guest changes the filesystem, so it may cache all it reads: not AUTO_INVAL_DATA,
        // which drops a file's cached pages when its modification time changes. Opens are not
        // sent (`OPEN` is refused), so not ATOMIC_O_TRUNC either: the guest truncates with
        // `SETATTR`.
        let supported = ASYNC_READ
            | PARALLEL_DIROPS
            | BIG_WRITES
            | ASYNC_DIO
            | DO_READDIRPLUS
            | READDIRPLUS_AUTO
            | MAX_PAGES
            | CACHE_SYMLINKS
            | HANDLE_KILLPRIV_V2
            | INIT_EXT;
        let capable = ((flags2 as u64) << 32) | init.flags as u64;
        let enabled = capable & supported;
        let max_pages = (MAX_BUFFER_SIZE as u64 - 1) / page_size() + 1;
        let out = InitOut {
            major: KERNEL_VERSION,
            minor: KERNEL_MINOR_VERSION,
            max_readahead: init.max_readahead,
            flags: enabled as u32,
            max_background: u16::MAX,
            congestion_threshold: (u16::MAX / 4) * 3,
            max_write: MAX_BUFFER_SIZE,
            // Nanoseconds.
            time_gran: 1,
            max_pages: max_pages as u16,
            map_alignment: 0,
            flags2: (enabled >> 32) as u32,
            unused: [0; 7],
        };
        Ok(out.as_bytes().to_vec())
    }

    fn batch_forget(&self, request: &mut Reader<'_, '_>) {
        let Some(BatchForgetIn { count, .. }) = request.read_obj() else {
            return;
        };
        for _ in 0..count {
            let Some(ForgetOne { nodeid, nlookup }) = request.read_obj() else {
                return;
            };
            self.fs.forget(Inode(nodeid), nlookup);
        }
    }

    async fn lookup(&self, header: &InHeader, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let body = request.read_to_end();
        let [name] = names(&body)?;
        match self.fs.lookup(Inode(header.nodeid), name).await {
            Ok(attr) => Ok(entry_out(&attr).as_bytes().to_vec()),
            // A missing name is an entry without an inode, which the guest caches.
            Err(Errno::ENOENT) => Ok(negative_entry_out().as_bytes().to_vec()),
            Err(errno) => Err(errno),
        }
    }

    async fn getattr(&self, header: &InHeader) -> Result<Vec<u8>> {
        let attr = self.fs.getattr(Inode(header.nodeid)).await?;
        Ok(attr_out(&attr).as_bytes().to_vec())
    }

    async fn setattr(&self, header: &InHeader, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let set: SetattrIn = request.read_obj().ok_or(Errno::EINVAL)?;
        let valid = |flag| set.valid & flag != 0;
        let time = |flag, now_flag, secs, nsecs| {
            Ok(if valid(now_flag) {
                Some(SetTime::Now)
            } else if valid(flag) {
                Some(SetTime::At(time(secs, nsecs)?))
            } else {
                None
            })
        };
        let attr = self
            .fs
            .setattr(
                Inode(header.nodeid),
                &SetAttr {
                    perm: valid(FATTR_MODE).then_some((set.mode & 0o7777) as u16),
                    uid: valid(FATTR_UID).then_some(set.uid),
                    gid: valid(FATTR_GID).then_some(set.gid),
                    size: valid(FATTR_SIZE).then_some(set.size),
                    atime: time(FATTR_ATIME, FATTR_ATIME_NOW, set.atime, set.atimensec)?,
                    mtime: time(FATTR_MTIME, FATTR_MTIME_NOW, set.mtime, set.mtimensec)?,
                    // FUSE_HANDLE_KILLPRIV_V2: truncations the guest asks it of, and chowns.
                    kill_suid_sgid: (valid(FATTR_SIZE) && valid(FATTR_KILL_SUIDGID))
                        || valid(FATTR_UID)
                        || valid(FATTR_GID),
                },
            )
            .await?;
        Ok(attr_out(&attr).as_bytes().to_vec())
    }

    async fn symlink(&self, header: &InHeader, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let body = request.read_to_end();
        // The target is a path, not a name.
        let [name, target] = strings(&body)?;
        entry_name(name)?;
        let attr = self.fs.symlink(Inode(header.nodeid), name, target).await?;
        Ok(entry_out(&attr).as_bytes().to_vec())
    }

    async fn mknod(&self, header: &InHeader, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let MknodIn { mode, rdev, umask, .. } = request.read_obj().ok_or(Errno::EINVAL)?;
        let body = request.read_to_end();
        let [name] = names(&body)?;
        let mode = mode & !umask;
        // A mode without a type is a regular file's.
        let file_type = FileType::from_mode(mode).unwrap_or(FileType::File);
        let attr = self
            .fs
            .mknod(Inode(header.nodeid), name, file_type, perm(mode), rdev, owner(header))
            .await?;
        Ok(entry_out(&attr).as_bytes().to_vec())
    }

    async fn mkdir(&self, header: &InHeader, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let MkdirIn { mode, umask } = request.read_obj().ok_or(Errno::EINVAL)?;
        let body = request.read_to_end();
        let [name] = names(&body)?;
        let attr =
            self.fs.mkdir(Inode(header.nodeid), name, perm(mode & !umask), owner(header)).await?;
        Ok(entry_out(&attr).as_bytes().to_vec())
    }

    async fn unlink(
        &self,
        header: &InHeader,
        request: &mut Reader<'_, '_>,
        directory: bool,
    ) -> Result<Vec<u8>> {
        let body = request.read_to_end();
        let [name] = names(&body)?;
        let parent = Inode(header.nodeid);
        if directory {
            self.fs.rmdir(parent, name).await
        } else {
            self.fs.unlink(parent, name).await
        }?;
        Ok(Vec::new())
    }

    async fn rename(
        &self,
        header: &InHeader,
        request: &mut Reader<'_, '_>,
        with_flags: bool,
    ) -> Result<Vec<u8>> {
        let (new_parent, flags) = if with_flags {
            let Rename2In { newdir, flags, .. } = request.read_obj().ok_or(Errno::EINVAL)?;
            (newdir, flags)
        } else {
            let RenameIn { newdir } = request.read_obj().ok_or(Errno::EINVAL)?;
            (newdir, 0)
        };
        let body = request.read_to_end();
        let [name, new_name] = names(&body)?;
        let flags = RenameFlags::from_bits_retain(flags);
        self.fs.rename(Inode(header.nodeid), name, Inode(new_parent), new_name, flags).await?;
        Ok(Vec::new())
    }

    async fn link(&self, header: &InHeader, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let LinkIn { oldnodeid } = request.read_obj().ok_or(Errno::EINVAL)?;
        let body = request.read_to_end();
        let [new_name] = names(&body)?;
        let attr = self.fs.link(Inode(oldnodeid), Inode(header.nodeid), new_name).await?;
        Ok(entry_out(&attr).as_bytes().to_vec())
    }

    async fn create(&self, header: &InHeader, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let CreateIn { mode, umask, .. } = request.read_obj().ok_or(Errno::EINVAL)?;
        let body = request.read_to_end();
        let [name] = names(&body)?;
        let attr =
            self.fs.create(Inode(header.nodeid), name, perm(mode & !umask), owner(header)).await?;
        let mut reply = entry_out(&attr).as_bytes().to_vec();
        reply.extend_from_slice(open_out().as_bytes());
        Ok(reply)
    }

    /// Reads into the reply, after its header, which is written once the read is done.
    async fn read(
        &self,
        header: &InHeader,
        request: &mut Reader<'_, '_>,
        reply: &mut Writer<'_, '_>,
    ) -> Result<usize, Error> {
        let Some(ReadIn { offset, size, .. }) = request.read_obj() else {
            return reply.error(Errno::EINVAL);
        };
        if size > MAX_BUFFER_SIZE {
            return reply.error(Errno::ENOMEM);
        }
        let len = (size as usize).min(reply.payload_capacity());
        let read =
            self.fs.read_vectored(Inode(header.nodeid), offset, &mut reply.payload(len)).await;
        match read {
            Ok(read) => reply.header(read),
            Err(errno) => reply.error(errno),
        }
    }

    async fn write(&self, header: &InHeader, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let WriteIn { offset, size, write_flags, .. } = request.read_obj().ok_or(Errno::EINVAL)?;
        if size > MAX_BUFFER_SIZE || size as usize > request.remaining() {
            return Err(Errno::EINVAL);
        }
        let inode = Inode(header.nodeid);
        let written = self.fs.write_vectored(inode, offset, &request.slices(size as usize)).await?;
        if write_flags & WRITE_KILL_SUIDGID != 0 {
            let kill = SetAttr { kill_suid_sgid: true, ..Default::default() };
            self.fs.setattr(inode, &kill).await?;
        }
        Ok(WriteOut { size: written as u32, padding: 0 }.as_bytes().to_vec())
    }

    async fn statfs(&self) -> Result<Vec<u8>> {
        let statfs = self.fs.statfs().await?;
        let out = Kstatfs {
            blocks: statfs.blocks,
            bfree: statfs.blocks_free,
            bavail: statfs.blocks_free,
            bsize: statfs.block_size,
            namelen: statfs.name_max,
            frsize: statfs.block_size,
            ..Default::default()
        };
        Ok(out.as_bytes().to_vec())
    }

    async fn opendir(&self, header: &InHeader) -> Result<Vec<u8>> {
        let handle = self.fs.opendir(Inode(header.nodeid)).await?;
        // Only the guest changes directories, so it may cache their entries.
        let open_flags = FOPEN_KEEP_CACHE | FOPEN_CACHE_DIR;
        Ok(OpenOut { fh: handle.0, open_flags, padding: 0 }.as_bytes().to_vec())
    }

    /// Lists the open directory's entries, as many as fit in the reply, each with its offset, the
    /// offset to list the next from. With `plus`, each has its attributes, and a lookup on it,
    /// except `.` and `..`, which the guest takes no lookups on; entries removed since the
    /// directory was opened are left out.
    async fn readdir(
        &self,
        request: &mut Reader<'_, '_>,
        capacity: usize,
        plus: bool,
    ) -> Result<Vec<u8>> {
        let ReadIn { fh, offset, size, .. } = request.read_obj().ok_or(Errno::EINVAL)?;
        if size > MAX_BUFFER_SIZE || size as usize > capacity {
            return Err(Errno::ENOMEM);
        }
        let entries = self.fs.readdir(DirHandle(fh), offset)?;
        let mut reply = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            let next_offset = offset + i as u64 + 1;
            let dirent_len = (size_of::<Dirent>() + entry.name.len()).next_multiple_of(8);
            let len = dirent_len + if plus { size_of::<EntryOut>() } else { 0 };
            if reply.len() + len > size as usize {
                break;
            }
            if plus {
                let attr = if is_dot(&entry.name) {
                    self.fs.getattr(entry.inode).await
                } else {
                    self.fs.lookup(entries.directory(), &entry.name).await
                };
                let attr = match attr {
                    Ok(attr) => attr,
                    // Removed since the directory was opened.
                    Err(Errno::ENOENT) => continue,
                    // The entries so far took lookups, which the guest forgets only if it gets
                    // them.
                    Err(_) if !reply.is_empty() => break,
                    Err(error) => return Err(error),
                };
                reply.extend_from_slice(entry_out(&attr).as_bytes());
            }
            push_dirent(&mut reply, entry, next_offset, dirent_len);
        }
        Ok(reply)
    }

    fn releasedir(&self, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let ReleaseIn { fh, .. } = request.read_obj().ok_or(Errno::EINVAL)?;
        self.fs.releasedir(DirHandle(fh));
        Ok(Vec::new())
    }

    async fn fallocate(&self, header: &InHeader, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let FallocateIn { offset, length, mode, .. } = request.read_obj().ok_or(Errno::EINVAL)?;
        // Only the default mode, 0, is supported.
        if mode != 0 {
            return Err(Errno::ENOTSUP);
        }
        self.fs.allocate(Inode(header.nodeid), offset, length).await?;
        Ok(Vec::new())
    }

    async fn setup_mapping(
        &self,
        header: &InHeader,
        request: &mut Reader<'_, '_>,
    ) -> Result<Vec<u8>> {
        let SetupmappingIn { foffset, len, flags, moffset, .. } =
            request.read_obj().ok_or(Errno::EINVAL)?;
        let writable = flags & SETUPMAPPING_FLAG_WRITE != 0;
        self.fs.setup_mapping(Inode(header.nodeid), foffset, len, moffset, writable).await?;
        Ok(Vec::new())
    }

    fn remove_mapping(&self, request: &mut Reader<'_, '_>) -> Result<Vec<u8>> {
        let RemovemappingIn { count } = request.read_obj().ok_or(Errno::EINVAL)?;
        for _ in 0..count {
            let RemovemappingOne { moffset, len } = request.read_obj().ok_or(Errno::EINVAL)?;
            self.fs.remove_mapping(moffset, len)?;
        }
        Ok(Vec::new())
    }
}

/// The NUL-terminated strings that make up a request's body, which has no more.
fn strings<const N: usize>(body: &[u8]) -> Result<[&[u8]; N]> {
    let mut strings = body.split_inclusive(|&byte| byte == 0);
    let found: [&[u8]; N] = std::array::from_fn(|_| strings.next().unwrap_or_default());
    (found.iter().all(|string| string.last() == Some(&0)) && strings.next().is_none())
        .then(|| found.map(|string| &string[..string.len() - 1]))
        .ok_or(Errno::EINVAL)
}

/// The names of directory entries that make up a request's body, which has no more.
fn names<const N: usize>(body: &[u8]) -> Result<[&[u8]; N]> {
    let names = strings::<N>(body)?;
    for name in names {
        entry_name(name)?;
    }
    Ok(names)
}

/// The longest name of a directory entry, Fxfs's.
const NAME_MAX: usize = 255;

/// `name`, if it can name a directory entry: not empty, `.` or `..`, without `/`, and not too long.
fn entry_name(name: &[u8]) -> Result<&[u8]> {
    if name.is_empty() || is_dot(name) || name.contains(&b'/') {
        Err(Errno::EINVAL)
    } else if name.len() > NAME_MAX {
        Err(Errno::ENAMETOOLONG)
    } else {
        Ok(name)
    }
}

fn is_dot(name: &[u8]) -> bool {
    name == b"." || name == b".."
}

/// The owner of nodes the request creates: its caller.
fn owner(header: &InHeader) -> Owner {
    Owner { uid: header.uid, gid: header.gid }
}

/// The permission bits of `mode`.
fn perm(mode: u32) -> u16 {
    (mode & 0o7777) as u16
}

/// The time `secs` and `nsecs` after the epoch. The guest's seconds are signed, and times before
/// the epoch, which Fxfs cannot store, are invalid, as are nanoseconds of a second or more.
fn time(secs: u64, nsecs: u32) -> Result<SystemTime> {
    if (secs as i64) < 0 || nsecs >= 1_000_000_000 {
        return Err(Errno::EINVAL);
    }
    UNIX_EPOCH.checked_add(Duration::new(secs, nsecs)).ok_or(Errno::EINVAL)
}

/// The seconds and nanoseconds of `time` since the epoch, which fxdax's times are not before.
fn timespec(time: SystemTime) -> (u64, u32) {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    (since_epoch.as_secs(), since_epoch.subsec_nanos())
}

fn fuse_attr(attr: &Attr) -> abi::Attr {
    let (atime, atimensec) = timespec(attr.atime);
    let (mtime, mtimensec) = timespec(attr.mtime);
    let (ctime, ctimensec) = timespec(attr.ctime);
    abi::Attr {
        ino: attr.inode.0,
        size: attr.size,
        blocks: attr.blocks,
        atime,
        mtime,
        ctime,
        atimensec,
        mtimensec,
        ctimensec,
        mode: attr.mode(),
        nlink: attr.nlink,
        uid: attr.uid,
        gid: attr.gid,
        rdev: attr.rdev,
        blksize: attr.blksize,
        flags: 0,
    }
}

fn entry_out(attr: &Attr) -> EntryOut {
    EntryOut {
        nodeid: attr.inode.0,
        generation: 0,
        entry_valid: TIMEOUT_SECS,
        attr_valid: TIMEOUT_SECS,
        entry_valid_nsec: 0,
        attr_valid_nsec: 0,
        attr: fuse_attr(attr),
    }
}

/// The entry of a name that does not exist, which the guest caches as long as other entries.
fn negative_entry_out() -> EntryOut {
    EntryOut {
        nodeid: 0,
        generation: 0,
        entry_valid: TIMEOUT_SECS,
        attr_valid: 0,
        entry_valid_nsec: 0,
        attr_valid_nsec: 0,
        attr: Default::default(),
    }
}

fn attr_out(attr: &Attr) -> AttrOut {
    AttrOut { attr_valid: TIMEOUT_SECS, attr_valid_nsec: 0, dummy: 0, attr: fuse_attr(attr) }
}

/// A created file's open: it needs no handle, nor flushing when closed. Only the guest changes
/// files, so its page cache stays valid.
fn open_out() -> OpenOut {
    OpenOut { fh: 0, open_flags: FOPEN_KEEP_CACHE | FOPEN_NOFLUSH, padding: 0 }
}

/// Appends `entry`'s dirent, whose next entry is at `next_offset`, padded to `dirent_len`.
fn push_dirent(reply: &mut Vec<u8>, entry: &DirEntry, next_offset: u64, dirent_len: usize) {
    let start = reply.len();
    let dirent = Dirent {
        ino: entry.inode.0,
        off: next_offset,
        namelen: entry.name.len() as u32,
        type_: entry.file_type.dirent_type().into(),
    };
    reply.extend_from_slice(dirent.as_bytes());
    reply.extend_from_slice(&entry.name);
    reply.resize(start + dirent_len, 0);
}

/// Copies out of a request's buffers in turn.
struct Reader<'r, 'a> {
    bufs: &'r [VolatileSlice<'a>],
    /// The buffer next read from, and the offset into it.
    index: usize,
    offset: usize,
    /// How much more may be read.
    remaining: usize,
}

impl<'r, 'a> Reader<'r, 'a> {
    fn new(bufs: &'r [VolatileSlice<'a>]) -> Self {
        let remaining = bufs.iter().map(VolatileSlice::len).sum();
        Self { bufs, index: 0, offset: 0, remaining }
    }

    fn remaining(&self) -> usize {
        self.remaining
    }

    /// Limits what may be read to `len` more bytes.
    fn limit(&mut self, len: usize) {
        self.remaining = self.remaining.min(len);
    }

    /// The next `len` bytes, which must remain, as slices of the buffers, which are then read.
    fn slices(&mut self, mut len: usize) -> Vec<VolatileSlice<'a>> {
        assert!(len <= self.remaining);
        let mut slices = Vec::new();
        while len > 0 {
            let buf = &self.bufs[self.index];
            let take = (buf.len() - self.offset).min(len);
            if take > 0 {
                slices.push(buf.subslice(self.offset, take).unwrap());
            }
            self.advance(take);
            len -= take;
        }
        slices
    }

    /// Advances over `len` bytes of the current buffer, which it must have.
    fn advance(&mut self, len: usize) {
        self.offset += len;
        self.remaining -= len;
        if self.index < self.bufs.len() && self.offset == self.bufs[self.index].len() {
            self.index += 1;
            self.offset = 0;
        }
    }

    /// Copies the next `dst.len()` bytes into `dst`, unless fewer remain.
    fn read_exact(&mut self, dst: &mut [u8]) -> Option<()> {
        if dst.len() > self.remaining {
            return None;
        }
        let mut copied = 0;
        while copied < dst.len() {
            let buf = &self.bufs[self.index];
            let take = (buf.len() - self.offset).min(dst.len() - copied);
            buf.subslice(self.offset, take).unwrap().copy_to(&mut dst[copied..copied + take]);
            self.advance(take);
            copied += take;
        }
        Some(())
    }

    fn read_obj<T: FromBytes + IntoBytes>(&mut self) -> Option<T> {
        let mut obj = T::new_zeroed();
        self.read_exact(obj.as_mut_bytes())?;
        Some(obj)
    }

    fn read_to_end(&mut self) -> Vec<u8> {
        let mut rest = vec![0; self.remaining];
        self.read_exact(&mut rest).expect("the rest remains");
        rest
    }
}

/// Copies into a reply's buffers in turn.
struct Writer<'w, 'a> {
    bufs: &'w mut [VolatileSliceMut<'a>],
    /// The request's id, which its reply's header carries.
    unique: u64,
}

impl Writer<'_, '_> {
    fn capacity(&self) -> usize {
        self.bufs.iter().map(VolatileSliceMut::len).sum()
    }

    /// How much payload fits after the header.
    fn payload_capacity(&self) -> usize {
        self.capacity().saturating_sub(size_of::<OutHeader>())
    }

    /// Replies with `payload`.
    fn ok(&mut self, payload: &[u8]) -> Result<usize, Error> {
        let header = self.header_bytes(payload.len(), 0);
        let reply = [header.as_slice(), payload].concat();
        self.copy_at(0, &reply)?;
        Ok(reply.len())
    }

    /// Replies with `errno`.
    fn error(&mut self, errno: Errno) -> Result<usize, Error> {
        let header = self.header_bytes(0, -errno.raw());
        self.copy_at(0, &header)?;
        Ok(header.len())
    }

    /// Writes the header of a reply whose payload of `len` bytes is already in place.
    fn header(&mut self, len: usize) -> Result<usize, Error> {
        let header = self.header_bytes(len, 0);
        self.copy_at(0, &header)?;
        Ok(header.len() + len)
    }

    fn header_bytes(&self, payload_len: usize, error: i32) -> Vec<u8> {
        let len = (size_of::<OutHeader>() + payload_len) as u32;
        OutHeader { len, error, unique: self.unique }.as_bytes().to_vec()
    }

    /// Copies `data` into the buffers at `offset`, which must fit it.
    fn copy_at(&mut self, offset: usize, data: &[u8]) -> Result<(), Error> {
        if offset + data.len() > self.capacity() {
            return Err(Error::msg("the reply does not fit its buffers"));
        }
        let mut copied = 0;
        for mut slice in self.slices(offset, data.len()) {
            copied += slice.copy_from(&data[copied..]);
        }
        Ok(())
    }

    /// The `len` bytes of payload after the header, as slices of the buffers.
    fn payload(&mut self, len: usize) -> Vec<VolatileSliceMut<'_>> {
        self.slices(size_of::<OutHeader>(), len)
    }

    /// The `len` bytes at `offset`, which must be within the buffers, as slices of them.
    fn slices(&mut self, mut offset: usize, mut len: usize) -> Vec<VolatileSliceMut<'_>> {
        let mut slices = Vec::new();
        for buf in self.bufs.iter_mut() {
            if len == 0 {
                break;
            }
            if offset >= buf.len() {
                offset -= buf.len();
                continue;
            }
            let take = (buf.len() - offset).min(len);
            // SAFETY: The slice is within `buf`, whose memory is writable for its lifetime, which
            // outlives the returned slices, which borrow the writer mutably.
            slices.push(unsafe {
                VolatileSliceMut::from_raw_parts(buf.addr_at(offset) as *mut u8, take)
            });
            offset = 0;
            len -= take;
        }
        slices
    }
}
