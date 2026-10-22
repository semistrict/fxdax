// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The types the operations of a filesystem's nodes ([`Node`](crate::Node)) take and give: inode
//! numbers, attributes, and directory entries.

use crate::error::{Errno, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// Linux's `st_mode` file types and permission bits.
const S_IFMT: u32 = 0o170000;
const S_IFSOCK: u32 = 0o140000;
const S_IFLNK: u32 = 0o120000;
const S_IFREG: u32 = 0o100000;
const S_IFBLK: u32 = 0o060000;
const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFIFO: u32 = 0o010000;
pub(crate) const S_ISUID: u32 = 0o4000;
pub(crate) const S_ISGID: u32 = 0o2000;
pub(crate) const S_IXGRP: u32 = 0o0010;
pub(crate) const PERM_MASK: u32 = 0o7777;

/// A node's inode number: its Fxfs object ID, which a volume allocates from small numbers up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Inode(pub u64);

impl From<u64> for Inode {
    fn from(inode: u64) -> Self {
        Self(inode)
    }
}

impl From<Inode> for u64 {
    fn from(inode: Inode) -> Self {
        inode.0
    }
}

/// The type of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileType {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// A named pipe.
    Fifo,
    /// A character device node.
    CharDevice,
    /// A block device node.
    BlockDevice,
    /// A Unix domain socket node.
    Socket,
}

impl FileType {
    /// The type whose `S_IFMT` bits `mode` has, if any.
    pub fn from_mode(mode: u32) -> Option<Self> {
        Some(match mode & S_IFMT {
            S_IFREG => Self::File,
            S_IFDIR => Self::Directory,
            S_IFLNK => Self::Symlink,
            S_IFIFO => Self::Fifo,
            S_IFCHR => Self::CharDevice,
            S_IFBLK => Self::BlockDevice,
            S_IFSOCK => Self::Socket,
            _ => return None,
        })
    }

    /// The type's `S_IFMT` bits of `st_mode`.
    pub fn mode_bits(self) -> u32 {
        match self {
            Self::File => S_IFREG,
            Self::Directory => S_IFDIR,
            Self::Symlink => S_IFLNK,
            Self::Fifo => S_IFIFO,
            Self::CharDevice => S_IFCHR,
            Self::BlockDevice => S_IFBLK,
            Self::Socket => S_IFSOCK,
        }
    }

    /// The type's `d_type` of a directory entry.
    pub fn dirent_type(self) -> u8 {
        (self.mode_bits() >> 12) as u8
    }

    /// The permissions of a node that has none of its own, such as the root directory of a new
    /// volume.
    pub(crate) fn default_perm(self) -> u16 {
        match self {
            Self::Directory => 0o755,
            Self::Symlink => 0o777,
            _ => 0o644,
        }
    }
}

/// A node's attributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attr {
    /// The node's inode number.
    pub inode: Inode,
    /// The size in bytes: of a file's contents, or of a symlink's target.
    pub size: u64,
    /// The storage allocated, in 512-byte units.
    pub blocks: u64,
    /// When the contents were last read.
    pub atime: SystemTime,
    /// When the contents were last changed.
    pub mtime: SystemTime,
    /// When the attributes or contents were last changed.
    pub ctime: SystemTime,
    /// The node's type.
    pub file_type: FileType,
    /// Permission bits, including setuid, setgid and sticky.
    pub perm: u16,
    /// The number of hard links.
    pub nlink: u32,
    /// The owner's user id.
    pub uid: u32,
    /// The owner's group id.
    pub gid: u32,
    /// The device a character or block device node stands for.
    pub rdev: u32,
    /// The preferred I/O size.
    pub blksize: u32,
}

impl Attr {
    /// `st_mode`: the type and permission bits.
    pub fn mode(&self) -> u32 {
        self.file_type.mode_bits() | self.perm as u32
    }
}

/// The new value of a timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetTime {
    /// The time of the change.
    Now,
    /// A given time, which must not be before the Unix epoch.
    At(SystemTime),
}

impl SetTime {
    pub(crate) fn resolve(self) -> SystemTime {
        match self {
            Self::Now => SystemTime::now(),
            Self::At(time) => time,
        }
    }
}

/// Changes to a node's attributes. `None` leaves an attribute unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetAttr {
    /// New permission bits; the type cannot change.
    pub perm: Option<u16>,
    /// A new owner's user id.
    pub uid: Option<u32>,
    /// A new owner's group id.
    pub gid: Option<u32>,
    /// A file's new size, which truncates or extends it.
    pub size: Option<u64>,
    /// A new access time.
    pub atime: Option<SetTime>,
    /// A new modification time.
    pub mtime: Option<SetTime>,
    /// Clear setuid, and setgid if group-executable, as a write, truncation or chown by an
    /// unprivileged process does. Applied before `perm`.
    pub kill_suid_sgid: bool,
}

/// How [`setxattr`](crate::Node::setxattr) sets an extended attribute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum XattrMode {
    /// Creates the attribute, or replaces its value.
    #[default]
    Set,
    /// Creates the attribute, failing with `EEXIST` if it exists (`XATTR_CREATE`).
    Create,
    /// Replaces the attribute's value, failing with `ENODATA` if it doesn't exist
    /// (`XATTR_REPLACE`).
    Replace,
}

/// The owner of a node being created: the credentials of the process creating it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Owner {
    /// The user id.
    pub uid: u32,
    /// The group id.
    pub gid: u32,
}

bitflags::bitflags! {
    /// `renameat2` flags.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct RenameFlags: u32 {
        /// Fail with `EEXIST` if the target exists.
        const NOREPLACE = 1 << 0;
        /// Exchange the source and target. Unsupported: fails with `EINVAL`.
        const EXCHANGE = 1 << 1;
    }
}

/// An entry of a directory ([`entries`](crate::Node::entries)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// The inode number of the entry's node.
    pub inode: Inode,
    /// The type of the entry's node.
    pub file_type: FileType,
    /// The entry's name.
    pub name: Vec<u8>,
}

/// A filesystem's usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatFs {
    /// The block size.
    pub block_size: u32,
    /// The number of blocks.
    pub blocks: u64,
    /// The number of free blocks.
    pub blocks_free: u64,
    /// The longest name allowed.
    pub name_max: u32,
}

/// The time `nanos` after the Unix epoch.
pub(crate) fn time_from_nanos(nanos: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(nanos)
}

/// Nanoseconds since the Unix epoch, which `time` must not be before.
pub(crate) fn nanos_from_time(time: SystemTime) -> Result<u64> {
    let since_epoch = time.duration_since(UNIX_EPOCH).map_err(|_| Errno::EINVAL)?;
    u64::try_from(since_epoch.as_nanos()).map_err(|_| Errno::EINVAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inodes are their numbers.
    #[test]
    fn inodes_are_their_numbers() {
        assert_eq!(u64::from(Inode::from(7)), 7);
    }

    /// Nodes without permissions of their own get these.
    #[test]
    fn default_permissions() {
        let perms = [FileType::Directory, FileType::Symlink, FileType::File, FileType::Fifo]
            .map(FileType::default_perm);
        assert_eq!(perms, [0o755, 0o777, 0o644, 0o644]);
    }
}
