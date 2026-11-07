// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The FUSE server: a request's bytes in, its reply's out, whatever carries them.

use super::abi::*;
use crate::error::{Errno, Error, Result};
use crate::filesystem::{Filesystem, Node};
use crate::node::XattrMode;
use crate::node::{Attr, DirEntry, FileType, Inode, Owner, RenameFlags, S_ISGID, SetAttr, SetTime};
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zerocopy::{FromBytes, IntoBytes};

/// The most a write carries, and a read asks for: the `max_write` INIT gives the kernel, which
/// sizes its reads by it too (`max_pages`).
pub(crate) const MAX_WRITE: u32 = 1 << 20;

/// How long the kernel may cache names, their absence, and attributes: as long as it likes, as
/// the mount is the filesystem's only writer, whose store its process alone has open.
const TIMEOUT_SECS: u64 = 24 * 60 * 60;

/// A FUSE server of a [`Filesystem`]: [`handle`](Self::handle) takes a request as the kernel sent
/// it, and gives its reply, for whatever carries them, as `Mount` does `/dev/fuse`'s. See the
/// module docs.
pub struct Server {
    fs: Filesystem,
    /// The object ID of the filesystem's root directory, whose node ID is [`ROOT_ID`]: what is
    /// the root's node ID to the kernel and the root's object ID to Fxfs swap places
    /// ([`Self::swap`]).
    root: u64,
    /// The nodes the kernel has looked up, by node ID, and how many lookups it holds of each,
    /// which it forgets as it evicts their inodes: a node it holds stays, as its handle holds it,
    /// unlinked or not, until it is forgotten. The root is not among them: it stays while the
    /// filesystem does.
    nodes: Mutex<HashMap<u64, Looked>>,
    /// The directories open, by handle.
    dirs: Mutex<OpenDirs>,
}

/// A node the kernel has looked up, and how many lookups of it it holds.
struct Looked {
    node: Node,
    lookups: u64,
}

#[derive(Default)]
struct OpenDirs {
    /// The handle the next directory opened takes.
    next: u64,
    open: HashMap<u64, Arc<OpenDir>>,
}

/// An open directory: its entries as it held them as it was opened, which its reads list in
/// turn, an offset an entry's index past `.` and `..`, so that they list each entry once, however
/// the directory changes meanwhile.
struct OpenDir {
    dir: Node,
    /// `..`'s inode.
    parent: Inode,
    entries: Vec<DirEntry>,
}

impl Server {
    /// A server of `fs`.
    pub fn new(fs: Filesystem) -> Self {
        let root = fs.root().inode().0;
        Self { fs, root, nodes: Mutex::default(), dirs: Mutex::default() }
    }

    /// The filesystem served.
    pub fn filesystem(&self) -> &Filesystem {
        &self.fs
    }

    /// The filesystem served, such as to close it once the kernel is done: the nodes the kernel
    /// held go, and the directories it had open.
    pub fn into_filesystem(self) -> Filesystem {
        self.fs
    }

    /// Handles the request whose bytes are `request`, as the kernel sent it, and returns its
    /// reply, unless it has none (`FORGET`, `BATCH_FORGET`, `INTERRUPT` and `DESTROY`).
    ///
    /// Requests may be handled concurrently, any number at once, as the kernel sends them. Fails if
    /// `request` is too short for a request's header, as then there is nothing to reply to, or if
    /// `DESTROY`'s unmount fails.
    pub async fn handle(&self, request: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let (header, rest) = InHeader::read_from_prefix(request)
            .map_err(|_| Error::msg("the request is shorter than its header"))?;
        let reply = Reply { unique: header.unique };
        let body_len = (header.len as usize).checked_sub(size_of::<InHeader>());
        let Some(body) = body_len.and_then(|len| rest.get(..len)) else {
            return Ok(Some(reply.error(Errno::EINVAL)));
        };
        let mut body = Body(body);
        let payload = match header.opcode {
            opcode::INIT => self.init(&mut body),
            opcode::DESTROY => {
                self.fs.unmount().await?;
                return Ok(None);
            }
            opcode::FORGET => {
                if let Ok(ForgetIn { nlookup }) = body.read() {
                    self.forget(header.nodeid, nlookup);
                }
                return Ok(None);
            }
            opcode::BATCH_FORGET => {
                self.batch_forget(&mut body);
                return Ok(None);
            }
            // Requests are not interruptible: each completes, as Fxfs completes it.
            opcode::INTERRUPT => return Ok(None),
            opcode::LOOKUP => self.lookup(&header, &mut body).await,
            opcode::GETATTR => self.getattr(&header).await,
            opcode::SETATTR => self.setattr(&header, &mut body).await,
            opcode::READLINK => self.readlink(&header).await,
            opcode::SYMLINK => self.symlink(&header, &mut body).await,
            opcode::MKNOD => self.mknod(&header, &mut body).await,
            opcode::MKDIR => self.mkdir(&header, &mut body).await,
            opcode::UNLINK => self.remove(&header, &mut body, false).await,
            opcode::RMDIR => self.remove(&header, &mut body, true).await,
            opcode::RENAME => self.rename(&header, &mut body, false).await,
            opcode::RENAME2 => self.rename(&header, &mut body, true).await,
            opcode::LINK => self.link(&header, &mut body).await,
            opcode::CREATE => self.create(&header, &mut body).await,
            opcode::READ => self.read(&header, &mut body).await,
            opcode::WRITE => self.write(&header, &mut body).await,
            opcode::STATFS => self.statfs().await,
            // Files have no handles: there is nothing to open, flush or release. Refused, OPEN and
            // FLUSH are not sent again, and nor is RELEASE once OPEN is not.
            opcode::OPEN | opcode::FLUSH => Err(Errno::ENOSYS),
            opcode::RELEASE => Ok(Vec::new()),
            opcode::FSYNC | opcode::FSYNCDIR => self.fsync(&header).await,
            opcode::SETXATTR => self.setxattr(&header, &mut body).await,
            opcode::GETXATTR => self.getxattr(&header, &mut body).await,
            opcode::LISTXATTR => self.listxattr(&header, &mut body).await,
            opcode::REMOVEXATTR => self.removexattr(&header, &mut body).await,
            opcode::OPENDIR => self.opendir(&header).await,
            opcode::READDIR => self.readdir(&mut body, false).await,
            opcode::READDIRPLUS => self.readdir(&mut body, true).await,
            opcode::RELEASEDIR => self.releasedir(&mut body),
            opcode::FALLOCATE => self.fallocate(&header, &mut body).await,
            _ => Err(Errno::ENOSYS),
        };
        Ok(Some(match payload {
            Ok(payload) => reply.ok(&payload),
            Err(errno) => reply.error(errno),
        }))
    }

    /// The node ID of the node `id` (and the object ID of the node whose node ID is `id`): its
    /// object ID, but for the root's, which is [`ROOT_ID`], and [`ROOT_ID`]'s, which is the
    /// root's: the two swap places.
    fn swap(&self, id: u64) -> u64 {
        match id {
            ROOT_ID => self.root,
            id if id == self.root => ROOT_ID,
            id => id,
        }
    }

    /// The node whose node ID is `nodeid`: the root, or one the kernel holds a lookup of. Any other
    /// is one the kernel forgot, or never had: `ESTALE`.
    fn node(&self, nodeid: u64) -> Result<Node> {
        if nodeid == ROOT_ID {
            return Ok(self.fs.root().clone());
        }
        let nodes = self.nodes.lock().unwrap();
        nodes.get(&nodeid).map(|looked| looked.node.clone()).ok_or(Errno::ESTALE)
    }

    /// The entry of a node found or made, which the kernel holds a lookup of from the reply on.
    fn entry(&self, (node, attr): (Node, Attr)) -> EntryOut {
        let nodeid = self.swap(attr.inode.0);
        if nodeid != ROOT_ID {
            let mut nodes = self.nodes.lock().unwrap();
            nodes.entry(nodeid).or_insert(Looked { node, lookups: 0 }).lookups += 1;
        }
        entry_out(nodeid, &attr)
    }

    /// The kernel forgets `nlookup` of its lookups of the node `nodeid`: the last lets it go.
    fn forget(&self, nodeid: u64, nlookup: u64) {
        let mut nodes = self.nodes.lock().unwrap();
        let Some(looked) = nodes.get_mut(&nodeid) else { return };
        looked.lookups = looked.lookups.saturating_sub(nlookup);
        if looked.lookups == 0 {
            let gone = nodes.remove(&nodeid);
            drop(nodes);
            // Letting a node go may free it: not under the table's lock.
            drop(gone);
        }
    }

    fn batch_forget(&self, body: &mut Body<'_>) {
        let Ok(BatchForgetIn { count, .. }) = body.read() else { return };
        for _ in 0..count {
            let Ok(ForgetOne { nodeid, nlookup }) = body.read() else { return };
            self.forget(nodeid, nlookup);
        }
    }

    fn init(&self, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let init: InitIn = body.read()?;
        let flags2 = match init.flags as u64 & INIT_EXT {
            0 => 0,
            _ => body.read::<InitInExt>()?.flags2,
        };
        if init.major < KERNEL_VERSION {
            log::error!("fuse: unsupported protocol version {}.{}", init.major, init.minor);
            return Err(Errno::EPROTO);
        }
        if init.major > KERNEL_VERSION {
            // The kernel asks again with a version of the server's.
            let out = InitOut {
                major: KERNEL_VERSION,
                minor: KERNEL_MINOR_VERSION,
                ..Default::default()
            };
            return Ok(out.as_bytes().to_vec());
        }
        if init.minor < OLDEST_MINOR_VERSION {
            log::error!("fuse: unsupported protocol version {}.{}", init.major, init.minor);
            return Err(Errno::EPROTO);
        }
        // The mount is the filesystem's only writer, so the kernel may cache all it reads: not
        // AUTO_INVAL_DATA, which drops a file's cached pages when its modification time changes,
        // nor WRITEBACK_CACHE, which cost more than it saved (dead-ends/fuse-writeback-cache.md).
        // Opens are not sent (`OPEN` is refused), so not ATOMIC_O_TRUNC either: the kernel
        // truncates with `SETATTR`.
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
        let enabled = (((flags2 as u64) << 32) | init.flags as u64) & supported;
        let max_pages = (MAX_WRITE as u64).div_ceil(crate::pages::page_size());
        let out = InitOut {
            major: KERNEL_VERSION,
            minor: KERNEL_MINOR_VERSION,
            max_readahead: init.max_readahead,
            flags: enabled as u32,
            max_background: u16::MAX,
            congestion_threshold: (u16::MAX / 4) * 3,
            max_write: MAX_WRITE,
            // Nanoseconds.
            time_gran: 1,
            max_pages: max_pages as u16,
            map_alignment: 0,
            flags2: (enabled >> 32) as u32,
            unused: [0; 7],
        };
        Ok(out.as_bytes().to_vec())
    }

    async fn lookup(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let [name] = strings(body.rest())?;
        match self.node(header.nodeid)?.lookup(name).await {
            Ok(found) => Ok(self.entry(found).as_bytes().to_vec()),
            // A missing name is an entry without a node, which the kernel caches.
            Err(Errno::ENOENT) => Ok(negative_entry_out().as_bytes().to_vec()),
            Err(errno) => Err(errno),
        }
    }

    async fn getattr(&self, header: &InHeader) -> Result<Vec<u8>> {
        let attr = self.node(header.nodeid)?.getattr().await?;
        Ok(attr_out(&attr).as_bytes().to_vec())
    }

    async fn setattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let set: SetattrIn = body.read()?;
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
        let set = SetAttr {
            perm: valid(FATTR_MODE).then_some(perm(set.mode)),
            uid: valid(FATTR_UID).then_some(set.uid),
            gid: valid(FATTR_GID).then_some(set.gid),
            size: valid(FATTR_SIZE).then_some(set.size),
            atime: time(FATTR_ATIME, FATTR_ATIME_NOW, set.atime, set.atimensec)?,
            mtime: time(FATTR_MTIME, FATTR_MTIME_NOW, set.mtime, set.mtimensec)?,
            // HANDLE_KILLPRIV_V2: truncations the kernel asks it of, and chowns.
            kill_suid_sgid: (valid(FATTR_SIZE) && valid(FATTR_KILL_SUIDGID))
                || valid(FATTR_UID)
                || valid(FATTR_GID),
        };
        let attr = self.node(header.nodeid)?.setattr(&set).await?;
        Ok(attr_out(&attr).as_bytes().to_vec())
    }

    async fn readlink(&self, header: &InHeader) -> Result<Vec<u8>> {
        self.node(header.nodeid)?.readlink().await
    }

    async fn fsync(&self, header: &InHeader) -> Result<Vec<u8>> {
        self.node(header.nodeid)?.fsync().await?;
        Ok(Vec::new())
    }

    /// The owner of a node the caller makes in the directory `dir`, and whether `dir`'s setgid is
    /// set, which a directory made in it takes too. Linux's VFS gives new inodes their owners
    /// (`inode_init_owner`), which FUSE leaves to the server: the caller's, but the group of a
    /// directory whose setgid is set. (Linux clears a new file's own setgid, if its caller isn't
    /// of its group, as it can't tell here, and leaves it.)
    async fn owner(&self, dir: &Node, header: &InHeader) -> Result<(Owner, bool)> {
        let attr = dir.getattr().await?;
        let setgid = attr.perm as u32 & S_ISGID != 0;
        let gid = if setgid { attr.gid } else { header.gid };
        Ok((Owner { uid: header.uid, gid }, setgid))
    }

    async fn symlink(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        // The target is a path, not a name.
        let [name, target] = strings(body.rest())?;
        let dir = self.node(header.nodeid)?;
        let (owner, _) = self.owner(&dir, header).await?;
        let made = dir.symlink(name, target, owner).await?;
        Ok(self.entry(made).as_bytes().to_vec())
    }

    async fn mknod(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let MknodIn { mode, rdev, umask, .. } = body.read()?;
        let [name] = strings(body.rest())?;
        // A mode without a type is a regular file's.
        let file_type = FileType::from_mode(mode).unwrap_or(FileType::File);
        let dir = self.node(header.nodeid)?;
        let (owner, _) = self.owner(&dir, header).await?;
        let made = dir.mknod(name, file_type, perm(mode & !umask), rdev, owner).await?;
        Ok(self.entry(made).as_bytes().to_vec())
    }

    async fn mkdir(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let MkdirIn { mode, umask } = body.read()?;
        let [name] = strings(body.rest())?;
        let dir = self.node(header.nodeid)?;
        let (owner, setgid) = self.owner(&dir, header).await?;
        let perm = perm(mode & !umask) | if setgid { S_ISGID as u16 } else { 0 };
        let made = dir.mkdir(name, perm, owner).await?;
        Ok(self.entry(made).as_bytes().to_vec())
    }

    async fn create(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let CreateIn { mode, umask, .. } = body.read()?;
        let [name] = strings(body.rest())?;
        let dir = self.node(header.nodeid)?;
        let (owner, _) = self.owner(&dir, header).await?;
        let made = dir.create(name, perm(mode & !umask), owner).await?;
        let mut reply = self.entry(made).as_bytes().to_vec();
        reply.extend_from_slice(open_out().as_bytes());
        Ok(reply)
    }

    /// Removes a name, as `rmdir` does if `directory`, and as `unlink` does otherwise.
    async fn remove(
        &self,
        header: &InHeader,
        body: &mut Body<'_>,
        directory: bool,
    ) -> Result<Vec<u8>> {
        let [name] = strings(body.rest())?;
        let dir = self.node(header.nodeid)?;
        match directory {
            true => dir.rmdir(name).await?,
            false => dir.unlink(name).await?,
        }
        Ok(Vec::new())
    }

    async fn rename(
        &self,
        header: &InHeader,
        body: &mut Body<'_>,
        with_flags: bool,
    ) -> Result<Vec<u8>> {
        let (new_dir, flags) = match with_flags {
            true => {
                let Rename2In { newdir, flags, .. } = body.read()?;
                (newdir, flags)
            }
            false => (body.read::<RenameIn>()?.newdir, 0),
        };
        let [name, new_name] = strings(body.rest())?;
        let (dir, new_dir) = (self.node(header.nodeid)?, self.node(new_dir)?);
        let flags = RenameFlags::from_bits_retain(flags);
        dir.rename(name, &new_dir, new_name, flags).await?;
        Ok(Vec::new())
    }

    async fn link(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let LinkIn { oldnodeid } = body.read()?;
        let [new_name] = strings(body.rest())?;
        let (node, dir) = (self.node(oldnodeid)?, self.node(header.nodeid)?);
        let attr = node.link(&dir, new_name).await?;
        Ok(self.entry((node, attr)).as_bytes().to_vec())
    }

    async fn read(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let ReadIn { offset, size, .. } = body.read()?;
        if size > MAX_WRITE {
            return Err(Errno::EINVAL);
        }
        let mut data = vec![0; size as usize];
        let read = self.node(header.nodeid)?.read(offset, &mut data).await?;
        data.truncate(read);
        Ok(data)
    }

    async fn write(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let WriteIn { offset, size, write_flags, .. } = body.read()?;
        if size > MAX_WRITE {
            return Err(Errno::EINVAL);
        }
        let data = body.take(size as usize)?;
        let file = self.node(header.nodeid)?;
        let written = file.write(offset, data).await?;
        if write_flags & WRITE_KILL_SUIDGID != 0 {
            file.setattr(&SetAttr { kill_suid_sgid: true, ..Default::default() }).await?;
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

    async fn setxattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let SetxattrIn { size, flags } = body.read()?;
        let mode = match flags {
            0 => XattrMode::Set,
            XATTR_CREATE => XattrMode::Create,
            XATTR_REPLACE => XattrMode::Replace,
            _ => return Err(Errno::EINVAL),
        };
        // The name, then the value, which ends the body.
        let rest = body.rest();
        let end = rest.iter().position(|&byte| byte == 0).ok_or(Errno::EINVAL)?;
        let (name, value) = (&rest[..end], &rest[end + 1..]);
        if value.len() != size as usize {
            return Err(Errno::EINVAL);
        }
        let name = xattr_name(name)?;
        self.node(header.nodeid)?.setxattr(name, value, mode).await?;
        Ok(Vec::new())
    }

    async fn getxattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let GetxattrIn { size, .. } = body.read()?;
        let [name] = strings(body.rest())?;
        let value = self.node(header.nodeid)?.getxattr(xattr_name(name)?).await?;
        sized(value, size)
    }

    async fn listxattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let GetxattrIn { size, .. } = body.read()?;
        let names = self.node(header.nodeid)?.listxattr().await?;
        // Each name NUL-terminated, as listxattr(2) lists them.
        let list = names.into_iter().flat_map(|name| name.into_iter().chain([0])).collect();
        sized(list, size)
    }

    async fn removexattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let [name] = strings(body.rest())?;
        self.node(header.nodeid)?.removexattr(xattr_name(name)?).await?;
        Ok(Vec::new())
    }

    async fn opendir(&self, header: &InHeader) -> Result<Vec<u8>> {
        let dir = self.node(header.nodeid)?;
        let entries = dir.entries().await?;
        let parent = dir.parent()?;
        let mut dirs = self.dirs.lock().unwrap();
        let fh = dirs.next;
        dirs.next += 1;
        dirs.open.insert(fh, Arc::new(OpenDir { dir, parent, entries }));
        // The mount is the directory's only writer, so the kernel may cache its entries.
        let open_flags = FOPEN_KEEP_CACHE | FOPEN_CACHE_DIR;
        Ok(OpenOut { fh, open_flags, padding: 0 }.as_bytes().to_vec())
    }

    /// Lists the open directory's entries from the offset asked, as many as fit in the size asked,
    /// each with its offset, the offset to list the next from: `.`, `..`, then those the
    /// directory held as it was opened. With `plus`, each has its node's entry, and a lookup of
    /// it, but `.` and `..`, which the kernel takes no lookups of, and entries removed since the
    /// directory was opened are left out.
    async fn readdir(&self, body: &mut Body<'_>, plus: bool) -> Result<Vec<u8>> {
        let ReadIn { fh, offset, size, .. } = body.read()?;
        if size > MAX_WRITE {
            return Err(Errno::EINVAL);
        }
        let open = self.dirs.lock().unwrap().open.get(&fh).cloned().ok_or(Errno::EBADF)?;
        let dots = [(&b"."[..], open.dir.inode()), (b"..", open.parent)].map(|(name, inode)| {
            DirEntry { inode, file_type: FileType::Directory, name: name.to_vec() }
        });
        let listed = dots.iter().chain(&open.entries).enumerate().skip(offset as usize);
        let mut reply = Vec::new();
        for (index, entry) in listed {
            let dirent_len = (size_of::<Dirent>() + entry.name.len()).next_multiple_of(8);
            let len = dirent_len + if plus { size_of::<EntryOut>() } else { 0 };
            if reply.len() + len > size as usize {
                break;
            }
            let mut entry = entry.clone();
            if plus {
                let found = match index {
                    // The kernel takes these from its own dentries.
                    0 | 1 => EntryOut::default(),
                    _ => match open.dir.lookup(&entry.name).await {
                        Ok(found) => {
                            entry.inode = found.1.inode;
                            self.entry(found)
                        }
                        // Removed since the directory was opened.
                        Err(Errno::ENOENT) => continue,
                        // The entries so far took lookups, which the kernel forgets only if it
                        // gets them.
                        Err(_) if !reply.is_empty() => break,
                        Err(errno) => return Err(errno),
                    },
                };
                reply.extend_from_slice(found.as_bytes());
            }
            push_dirent(&mut reply, &entry, index as u64 + 1, dirent_len);
        }
        Ok(reply)
    }

    fn releasedir(&self, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let ReleaseIn { fh, .. } = body.read()?;
        let closed = self.dirs.lock().unwrap().open.remove(&fh);
        // Its handle on the directory goes: not under the table's lock.
        drop(closed);
        Ok(Vec::new())
    }

    async fn fallocate(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let FallocateIn { offset, length, mode, .. } = body.read()?;
        // Only the default mode, 0, which allocates.
        if mode != 0 {
            return Err(Errno::ENOTSUP);
        }
        self.node(header.nodeid)?.allocate(offset, length).await?;
        Ok(Vec::new())
    }
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").field("fs", &self.fs).finish_non_exhaustive()
    }
}

/// What remains of a request's body, read from its front.
struct Body<'a>(&'a [u8]);

impl<'a> Body<'a> {
    /// The message at the front: `EINVAL` if too little remains.
    fn read<T: FromBytes>(&mut self) -> Result<T> {
        let (message, rest) = T::read_from_prefix(self.0).map_err(|_| Errno::EINVAL)?;
        self.0 = rest;
        Ok(message)
    }

    /// The `len` bytes at the front: `EINVAL` if fewer remain.
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(len).ok_or(Errno::EINVAL)?;
        self.0 = rest;
        Ok(taken)
    }

    /// What remains.
    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.0)
    }
}

/// A reply to the request `unique`.
struct Reply {
    unique: u64,
}

impl Reply {
    fn ok(&self, payload: &[u8]) -> Vec<u8> {
        let mut reply = self.header(payload.len(), 0);
        reply.extend_from_slice(payload);
        reply
    }

    fn error(&self, errno: Errno) -> Vec<u8> {
        self.header(0, -errno.raw())
    }

    fn header(&self, payload_len: usize, error: i32) -> Vec<u8> {
        let len = (size_of::<OutHeader>() + payload_len) as u32;
        OutHeader { len, error, unique: self.unique }.as_bytes().to_vec()
    }
}

/// The `N` NUL-terminated strings that make up `body`, which holds no more: `EINVAL` otherwise.
fn strings<const N: usize>(body: &[u8]) -> Result<[&[u8]; N]> {
    let mut strings = body.split_inclusive(|&byte| byte == 0);
    let found: [&[u8]; N] = std::array::from_fn(|_| strings.next().unwrap_or_default());
    (found.iter().all(|string| string.last() == Some(&0)) && strings.next().is_none())
        .then(|| found.map(|string| &string[..string.len() - 1]))
        .ok_or(Errno::EINVAL)
}

/// The namespaces of extended attributes served: those the guest's Fxfs has handlers of. POSIX
/// ACLs' (`system.posix_acl_*`) the kernel refuses itself, as INIT doesn't offer `POSIX_ACL`.
const XATTR_NAMESPACES: [&[u8]; 3] = [b"user.", b"trusted.", b"security."];

/// `name`, an extended attribute's, of a namespace served, as Linux's VFS resolves a name to its
/// filesystem's handler (`xattr_resolve_name`), which FUSE leaves to its servers: `EOPNOTSUPP` of
/// another namespace, `EINVAL` for a namespace's prefix alone.
fn xattr_name(name: &[u8]) -> Result<&[u8]> {
    match XATTR_NAMESPACES.iter().find(|prefix| name.starts_with(prefix)) {
        Some(prefix) if name.len() == prefix.len() => Err(Errno::EINVAL),
        Some(_) => Ok(name),
        None => Err(Errno::ENOTSUP),
    }
}

/// `value` for a request that asked for at most `size` bytes of it, or, asking for 0, its size:
/// `ERANGE` if it is longer, as getxattr(2) and listxattr(2) have it.
fn sized(value: Vec<u8>, size: u32) -> Result<Vec<u8>> {
    match size {
        0 => Ok(GetxattrOut { size: value.len() as u32, padding: 0 }.as_bytes().to_vec()),
        size if value.len() > size as usize => Err(Errno::ERANGE),
        _ => Ok(value),
    }
}

/// The permission bits of `mode`.
fn perm(mode: u32) -> u16 {
    (mode & 0o7777) as u16
}

/// The time `secs` and `nsecs` after the epoch. The kernel's seconds are signed, and times before
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

fn fuse_attr(attr: &Attr) -> super::abi::Attr {
    let (atime, atimensec) = timespec(attr.atime);
    let (mtime, mtimensec) = timespec(attr.mtime);
    let (ctime, ctimensec) = timespec(attr.ctime);
    super::abi::Attr {
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

/// The entry of the node `nodeid`, whose attributes are `attr`: its inode number is its object
/// ID, whatever its node ID.
fn entry_out(nodeid: u64, attr: &Attr) -> EntryOut {
    EntryOut {
        nodeid,
        generation: 0,
        entry_valid: TIMEOUT_SECS,
        attr_valid: TIMEOUT_SECS,
        entry_valid_nsec: 0,
        attr_valid_nsec: 0,
        attr: fuse_attr(attr),
    }
}

/// The entry of a name that does not exist, which the kernel caches as long as other entries.
fn negative_entry_out() -> EntryOut {
    EntryOut { entry_valid: TIMEOUT_SECS, ..Default::default() }
}

fn attr_out(attr: &Attr) -> AttrOut {
    AttrOut { attr_valid: TIMEOUT_SECS, attr_valid_nsec: 0, dummy: 0, attr: fuse_attr(attr) }
}

/// A created file's open: it needs no handle, nor flushing as it is closed, and its cached pages
/// stay valid, as the mount is the file's only writer.
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    /// The root's object ID and FUSE's root ID swap places as node IDs, and each other object ID
    /// is its own: whatever Fxfs numbers 1 has the root's object ID for its node ID, so no two
    /// nodes share one.
    #[test]
    fn the_root_and_fuses_root_id_swap_places() {
        let fs = block_on(Filesystem::format(crate::Storage::memory(8 << 20))).unwrap();
        let server = Server::new(fs);
        let root = server.root;
        assert_eq!((server.swap(root), server.swap(ROOT_ID)), (ROOT_ID, root));
        for id in [ROOT_ID, root, root + 1, 1 << 40] {
            assert_eq!(server.swap(server.swap(id)), id);
        }
        block_on(server.into_filesystem().close()).unwrap();
    }
}
