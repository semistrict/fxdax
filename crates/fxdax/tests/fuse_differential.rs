// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Random sequences of file system calls made on a FUSE mount of a new store (`fxdax::fuse`) and
//! on a directory of Linux's own tmpfs beside it, their results compared (TASK-149.37): Linux's
//! filesystem the oracle of the host's `Node` layer, which plays Linux's VFS for Fxfs, of
//! fxfs-posix under it, and of the FUSE server over it, as the guest's fsx differential holds the
//! guest's Fxfs to ext4 (TASK-149.12). Only Linux mounts FUSE.
//!
//! Each call is made on the reference (tmpfs) first, then on the mount, and their results compared:
//! its value, or its errno. After each, the two trees are compared whole: each node's type,
//! permissions, owner, group, links, size, device, which paths are links of one node, contents,
//! symlink targets and extended attributes; and which nodes' modification and change times the
//! call changed, the reference's coarse clock having ticked before it, so that a change it made
//! shows. At the end the mount is unmounted, its store checked (fsck) and mounted directly, and its
//! tree compared with the reference's, beyond what the kernel caches.
//!
//! What the two may differ in, and the calls left out as only their differences would show, are
//! listed in [`ALLOWED`], each with why: the one place they are.
//!
//! Run by a user, the mount is made through `fusermount3` and every call is the user's own. Run by
//! root (GCE; `sudo` in Lima), the mount is made with mount(2), the reference is a tmpfs of the
//! test's own, and the calls switch between root and two other users (the calling thread's
//! `setfsuid`, `setfsgid` and supplementary groups): permissions, sticky directories, setgid
//! directories' groups, chown's rules and setuid's killing, as Linux has them for each, which a
//! user's run cannot reach.
//!
//! Seeded and shrunk by proptest: `FXDAX_FUSE_DIFFERENTIAL_CASES` cases (256 by default), from
//! `PROPTEST_RNG_SEED`'s seed if set.

#![cfg(target_os = "linux")]

use futures::executor::block_on;
use fxdax::fuse::{Mount, MountOptions};
use fxdax::{FileType, Filesystem, Node, SetAttr, Storage};
use proptest::prelude::*;
use proptest::strategy::Union;
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fmt;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

/// What the mount and Linux's tmpfs may differ in, each with why. The comparison leaves out what
/// they may differ in, and the generator the calls whose results would only show a difference
/// listed here: [`allowed`] says which, and each place that leaves something out names its entry.
const ALLOWED: &[(Allowed, &str)] = &[
    (
        Allowed::InodeNumbers,
        "Each filesystem numbers its inodes its own way: compared as which paths link one node.",
    ),
    (
        Allowed::Blocks,
        "st_blocks counts what each filesystem allocates, its own way: tmpfs its pages, Fxfs its \
         blocks.",
    ),
    (
        Allowed::DirectorySizes,
        "A directory's st_size is its filesystem's own measure: tmpfs counts its entries, Fxfs its \
         own way.",
    ),
    (
        Allowed::AccessTimes,
        "Reads change atime as each filesystem's rules have it: tmpfs's relatime; the server never \
         on a read, as the kernel sends none. Only the atimes utimensat sets are compared.",
    ),
    (
        Allowed::SpecialFileTypes,
        "Fxfs's directory entries tell directories and symlinks from the rest, which list as \
         regular files (DT_REG): a FIFO's, socket's or device's d_type, not its stat's type.",
    ),
    (
        Allowed::DirectoryOrder,
        "readdir lists entries in its filesystem's own order: compared sorted.",
    ),
    (
        Allowed::NonUtf8Names,
        "Fxfs keeps names in UTF-8, refusing others with EINVAL, where Linux's filesystems take \
         any bytes: not generated.",
    ),
    (
        Allowed::TimesOutsideFxfs,
        "Fxfs keeps times as nanoseconds since the epoch, unsigned in 64 bits: before 1970 and \
         after 2554 refused (EINVAL), which tmpfs keeps: not generated.",
    ),
    (
        Allowed::RenameExchange,
        "Node::rename refuses RENAME_EXCHANGE (EINVAL), which fxfs-posix has (exchange) and tmpfs \
         serves (TASK-226): not generated. RENAME_WHITEOUT, which needs CAP_MKNOD and is \
         overlayfs's, neither.",
    ),
    (
        Allowed::TruncateToItsSize,
        "FUSE sends truncate(2), open(O_TRUNC) and ftruncate alike, a SETATTR of the size, the \
         kernel leaving their times to the server, which changes them as a truncation's: Linux's \
         filesystems leave them for truncate(2) to a file's own size alone. Its times are let \
         differ so.",
    ),
    (
        Allowed::PipeTimes,
        "A FIFO's reads and writes are the kernel's pipe's, which tells a FUSE server nothing, and \
         FUSE's inodes keep no times of their own (S_NOCMTIME): the times they change on tmpfs \
         stay on the mount. Not compared.",
    ),
    (
        Allowed::DirectorySeeks,
        "lseek on a directory is its filesystem's own: tmpfs's refuses SEEK_END, FUSE's takes \
         it. Not compared.",
    ),
    (
        Allowed::SetgidWithoutGroupExecute,
        "Linux kills a file's setgid without group-execute as one not of its group writes, \
         truncates or chowns it; FUSE's kernel, killing it for the server, kills it only with \
         group-execute (fuse_setattr), and tells a server nothing of the caller's groups: not \
         generated, as modes, nor through a umask taking group-execute.",
    ),
    (
        Allowed::FallocateModes,
        "The server allocates alone (fallocate's mode 0): KEEP_SIZE, PUNCH_HOLE, ZERO_RANGE and \
         the rest are EOPNOTSUPP, which tmpfs serves in part (TASK-226): not generated.",
    ),
    (
        Allowed::Holes,
        "The server serves no lseek, whose SEEK_DATA and SEEK_HOLE the kernel then answers as if \
         the file were all data, where tmpfs finds its holes (TASK-226): not generated.",
    ),
    (
        Allowed::PosixAcls,
        "The server serves no POSIX ACLs (system.posix_acl_*: EOPNOTSUPP), which tmpfs keeps \
         (TASK-226): not generated.",
    ),
    (
        Allowed::Tmpfile,
        "The server serves no O_TMPFILE (FUSE_TMPFILE: EOPNOTSUPP), which tmpfs serves \
         (TASK-226): not generated.",
    ),
];

/// A difference [`ALLOWED`] lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Allowed {
    InodeNumbers,
    Blocks,
    DirectorySizes,
    AccessTimes,
    SpecialFileTypes,
    DirectoryOrder,
    NonUtf8Names,
    TimesOutsideFxfs,
    TruncateToItsSize,
    PipeTimes,
    DirectorySeeks,
    SetgidWithoutGroupExecute,
    RenameExchange,
    FallocateModes,
    Holes,
    PosixAcls,
    Tmpfile,
}

/// Why `allowed` is, which it must be: what leaves something out names its entry so.
fn allowed(allowed: Allowed) -> &'static str {
    let found = ALLOWED.iter().find(|(listed, _)| *listed == allowed);
    found.map(|(_, why)| *why).unwrap_or_else(|| panic!("{allowed:?} is not in ALLOWED"))
}

/// A memory store's size: room for what a case writes.
const STORE_SIZE: u64 = 128 << 20;

/// The most files a case holds open at once: an open past them closes the oldest first.
const MAX_FDS: usize = 6;

/// An errno.
type Errno = i32;

/// What a call came to: its value, or its errno.
type Outcome = Result<Out, Errno>;

/// The errno of the call that just failed.
fn errno() -> Errno {
    std::io::Error::last_os_error().raw_os_error().unwrap()
}

/// The result of a call returning -1 on failure, and the errno then.
fn ret<T: Into<i64> + Copy>(value: T) -> Result<i64, Errno> {
    match value.into() {
        -1 => Err(errno()),
        value => Ok(value),
    }
}

fn cstring(bytes: &[u8]) -> CString {
    CString::new(bytes).unwrap()
}

/// Bytes, shown by their length and first bytes.
#[derive(Clone, PartialEq, Eq)]
struct Bytes(Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown = &self.0[..self.0.len().min(16)];
        write!(f, "{} bytes {:?}", self.0.len(), String::from_utf8_lossy(shown))
    }
}

/// A path in the tree, relative to its root: `.` is the root.
#[derive(Clone, PartialEq, Eq)]
struct P(String);

impl fmt::Debug for P {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.len() > 40 {
            // A long name, by its length.
            let parts: Vec<String> = self
                .0
                .split('/')
                .map(|part| match part.len() > 8 {
                    true => format!("{}×{}", &part[..1], part.len()),
                    false => part.to_owned(),
                })
                .collect();
            return write!(f, "{}", parts.join("/"));
        }
        write!(f, "{}", self.0)
    }
}

/// Bytes a write writes, made of a length and a seed, so that a case shows them shortly.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Data {
    len: usize,
    seed: u8,
}

impl Data {
    fn bytes(self) -> Vec<u8> {
        (0..self.len).map(|i| (i / 251 + i * 7 + self.seed as usize) as u8).collect()
    }
}

impl fmt::Debug for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}×{}", self.len, self.seed)
    }
}

/// open(2)'s flags.
#[derive(Clone, Copy, PartialEq, Eq)]
struct OpenFlags {
    access: i32,
    creat: bool,
    excl: bool,
    trunc: bool,
    append: bool,
    directory: bool,
    nofollow: bool,
}

impl OpenFlags {
    /// The flags, non-blocking always, so that a FIFO's open returns at once.
    fn bits(self) -> i32 {
        let mut bits = self.access | libc::O_NONBLOCK | libc::O_CLOEXEC;
        for (set, flag) in [
            (self.creat, libc::O_CREAT),
            (self.excl, libc::O_EXCL),
            (self.trunc, libc::O_TRUNC),
            (self.append, libc::O_APPEND),
            (self.directory, libc::O_DIRECTORY),
            (self.nofollow, libc::O_NOFOLLOW),
        ] {
            if set {
                bits |= flag;
            }
        }
        bits
    }
}

impl fmt::Debug for OpenFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let access = match self.access {
            libc::O_RDONLY => "RDONLY",
            libc::O_WRONLY => "WRONLY",
            _ => "RDWR",
        };
        write!(f, "{access}")?;
        for (set, name) in [
            (self.creat, "CREAT"),
            (self.excl, "EXCL"),
            (self.trunc, "TRUNC"),
            (self.append, "APPEND"),
            (self.directory, "DIRECTORY"),
            (self.nofollow, "NOFOLLOW"),
        ] {
            if set {
                write!(f, "|{name}")?;
            }
        }
        Ok(())
    }
}

/// A time utimensat sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Time {
    Now,
    Omit,
    At(i64, i64),
}

impl Time {
    fn timespec(self) -> libc::timespec {
        let (tv_sec, tv_nsec) = match self {
            Time::Now => (0, libc::UTIME_NOW),
            Time::Omit => (0, libc::UTIME_OMIT),
            Time::At(secs, nsecs) => (secs, nsecs),
        };
        libc::timespec { tv_sec, tv_nsec }
    }
}

/// A special file mknod makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Special {
    Fifo,
    Socket,
    Char(u32, u32),
    Block(u32, u32),
}

impl Special {
    fn mode_and_dev(self) -> (u32, libc::dev_t) {
        match self {
            Special::Fifo => (libc::S_IFIFO, 0),
            Special::Socket => (libc::S_IFSOCK, 0),
            Special::Char(major, minor) => (libc::S_IFCHR, libc::makedev(major, minor)),
            Special::Block(major, minor) => (libc::S_IFBLK, libc::makedev(major, minor)),
        }
    }
}

/// How large a buffer getxattr or listxattr is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XSize {
    /// None: the call says how large one it needs.
    Zero,
    Small,
    Large,
}

impl XSize {
    fn len(self) -> usize {
        match self {
            XSize::Zero => 0,
            XSize::Small => 4,
            XSize::Large => 1 << 16,
        }
    }
}

/// A call, made alike on both filesystems. `fd` is an index of the files a case holds open, modulo
/// their number.
#[derive(Clone, Debug)]
enum Op {
    Open {
        path: P,
        flags: OpenFlags,
        mode: u32,
    },
    Close {
        fd: usize,
    },
    Read {
        fd: usize,
        len: usize,
    },
    Write {
        fd: usize,
        data: Data,
    },
    Pread {
        fd: usize,
        at: u64,
        len: usize,
    },
    Pwrite {
        fd: usize,
        at: u64,
        data: Data,
    },
    Seek {
        fd: usize,
        at: i64,
        whence: i32,
    },
    Ftruncate {
        fd: usize,
        len: u64,
    },
    Truncate {
        path: P,
        len: u64,
    },
    Fallocate {
        fd: usize,
        at: u64,
        len: u64,
    },
    Fsync {
        fd: usize,
        data_only: bool,
    },
    Fstat {
        fd: usize,
    },
    Fchmod {
        fd: usize,
        mode: u32,
    },
    Mkdir {
        path: P,
        mode: u32,
    },
    Mknod {
        path: P,
        special: Special,
        mode: u32,
    },
    Symlink {
        target: P,
        path: P,
    },
    Link {
        from: P,
        to: P,
        follow: bool,
    },
    Unlink {
        path: P,
    },
    Rmdir {
        path: P,
    },
    Rename {
        from: P,
        to: P,
        noreplace: bool,
    },
    Chmod {
        path: P,
        mode: u32,
    },
    Chown {
        path: P,
        uid: Option<u32>,
        gid: Option<u32>,
        follow: bool,
    },
    Utimens {
        path: P,
        atime: Time,
        mtime: Time,
        follow: bool,
    },
    Stat {
        path: P,
        follow: bool,
    },
    Readlink {
        path: P,
    },
    Readdir {
        path: P,
    },
    Access {
        path: P,
        mode: i32,
    },
    Setxattr {
        path: P,
        name: &'static str,
        value: Data,
        flags: i32,
        follow: bool,
    },
    Getxattr {
        path: P,
        name: &'static str,
        size: XSize,
        follow: bool,
    },
    Listxattr {
        path: P,
        size: XSize,
        follow: bool,
    },
    Removexattr {
        path: P,
        name: &'static str,
        follow: bool,
    },
    /// Sets the process's umask: made once, for both.
    Umask {
        mask: u32,
    },
    /// Makes the calls from here on as the run's `who`th identity: made once, for both.
    As {
        who: usize,
    },
}

/// A node's attributes, as compared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Stat {
    /// `S_IFMT`'s bits.
    kind: u32,
    perm: u32,
    nlink: u64,
    uid: u32,
    gid: u32,
    /// The size, but a directory's ([`Allowed::DirectorySizes`]).
    size: Option<u64>,
    /// A device's major and minor numbers.
    rdev: Option<(u32, u32)>,
}

impl Stat {
    fn new(
        kind: u32,
        perm: u32,
        nlink: u64,
        (uid, gid): (u32, u32),
        size: u64,
        rdev: (u32, u32),
    ) -> Self {
        let is = |kinds: &[u32]| kinds.contains(&kind);
        Self {
            kind,
            perm,
            nlink,
            uid,
            gid,
            size: (!is(&[libc::S_IFDIR])).then_some(size),
            rdev: is(&[libc::S_IFCHR, libc::S_IFBLK]).then_some(rdev),
        }
    }

    fn of(st: &libc::stat) -> Self {
        let rdev = (libc::major(st.st_rdev), libc::minor(st.st_rdev));
        let owner = (st.st_uid, st.st_gid);
        let (kind, perm) = (st.st_mode & libc::S_IFMT, st.st_mode & 0o7777);
        Self::new(kind, perm, st.st_nlink as u64, owner, st.st_size as u64, rdev)
    }
}

/// What a call returned, as compared.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Out {
    Done,
    Size(i64),
    Bytes(Bytes),
    Stat(Stat),
    /// readdir's names and their `d_type`s, sorted ([`Allowed::DirectoryOrder`]).
    Entries(Vec<(String, u8)>),
    /// listxattr's names, sorted.
    Names(Vec<Bytes>),
    /// The atime and mtime utimensat set, of those it set to a time given.
    Times(Option<(i64, i64)>, Option<(i64, i64)>),
}

/// A time, in nanoseconds since the epoch.
type Nanos = i128;

/// What a tree's walk found of a node.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    stat: Stat,
    /// Which node of the tree it is: the first path's index, in order, of those linking it
    /// ([`Allowed::InodeNumbers`]).
    node: usize,
    data: Contents,
    xattrs: Result<BTreeMap<Bytes, Bytes>, Errno>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Contents {
    File(Result<Bytes, Errno>),
    Target(Bytes),
    /// A directory, and whether its listing failed, which leaves what it holds out of the tree.
    Directory(Result<(), Errno>),
    Special,
    /// A node listed whose attributes the walk could not read: a directory's entry whose
    /// directory it may list but not search.
    Unstatted(Errno),
}

impl PartialOrd for Bytes {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Bytes {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

/// A tree's nodes by path, and their modification and change times.
#[derive(Clone, Debug, Default)]
struct Tree {
    entries: BTreeMap<String, Entry>,
    times: BTreeMap<String, (Nanos, Nanos)>,
}

impl Tree {
    /// The tree of `found`'s nodes, by path: each one's inode number, and the rest.
    fn of(
        found: BTreeMap<String, (u64, Stat, Contents, Result<BTreeMap<Bytes, Bytes>, Errno>)>,
    ) -> Self {
        let mut first: BTreeMap<u64, usize> = BTreeMap::new();
        let entries = found
            .into_iter()
            .enumerate()
            .map(|(index, (path, (ino, stat, data, xattrs)))| {
                let node = *first.entry(ino).or_insert(index);
                (path, Entry { stat, node, data, xattrs })
            })
            .collect();
        Self { entries, times: BTreeMap::new() }
    }

    /// Whether all of it was read: every directory listed, every file read, every node's
    /// attributes listed.
    fn whole(&self) -> bool {
        self.entries.values().all(|entry| {
            entry.xattrs.is_ok()
                && !matches!(
                    entry.data,
                    Contents::File(Err(_)) | Contents::Directory(Err(_)) | Contents::Unstatted(_)
                )
        })
    }
}

/// `name` in the directory `dir`, a path of the tree.
fn join(dir: &str, name: &str) -> String {
    match dir {
        "." => name.to_owned(),
        dir => format!("{dir}/{name}"),
    }
}

/// Which way the trees differ, if they do: a path one has and the other hasn't, or the first
/// whose entries differ.
fn tree_difference(reference: &Tree, subject: &Tree) -> Option<String> {
    for (path, theirs) in &reference.entries {
        match subject.entries.get(path) {
            None => return Some(format!("{path} is on the reference alone: {theirs:?}")),
            Some(ours) if ours != theirs => {
                return Some(format!(
                    "{path} differs:\n  reference: {theirs:?}\n  mount:     {ours:?}"
                ));
            }
            Some(_) => {}
        }
    }
    let extra = subject.entries.iter().find(|(path, _)| !reference.entries.contains_key(*path));
    extra.map(|(path, ours)| format!("{path} is on the mount alone: {ours:?}"))
}

/// How the times of the nodes both trees `before` and `after` hold changed: which modification and
/// change times.
fn times_changed(before: &Tree, after: &Tree) -> BTreeMap<String, (bool, bool)> {
    let changed = after.times.iter().filter_map(|(path, (mtime, ctime))| {
        let (was_mtime, was_ctime) = before.times.get(path)?;
        Some((path.clone(), (mtime != was_mtime, ctime != was_ctime)))
    });
    changed.collect()
}

/// Who makes the calls: a user and its groups.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    uid: u32,
    gid: u32,
    groups: Vec<u32>,
}

impl Identity {
    /// Makes this thread's calls to filesystems as this identity's: its fsuid, fsgid and
    /// supplementary groups, the thread's alone (not glibc's setgroups, which sets every
    /// thread's). Root's own, of a run by root.
    fn assume(&self) {
        // SAFETY: setgroups(2) of the calling thread alone, from a vector that outlives the call;
        // setfsgid(2) and setfsuid(2), the thread's own.
        unsafe {
            let set = libc::syscall(libc::SYS_setgroups, self.groups.len(), self.groups.as_ptr());
            assert_eq!(set, 0, "setgroups: {}", std::io::Error::last_os_error());
            libc::setfsgid(self.gid);
            libc::setfsuid(self.uid);
            // An ID that isn't one changes nothing, and says what is.
            assert_eq!(libc::setfsgid(u32::MAX) as u32, self.gid, "setfsgid");
            assert_eq!(libc::setfsuid(u32::MAX) as u32, self.uid, "setfsuid");
        }
    }
}

/// What a run is: root's or a user's, and the identities, users and groups its calls use.
#[derive(Clone, Debug)]
struct Run {
    root: bool,
    /// Those calls are made as, the first, the run's own, to begin with.
    identities: Vec<Identity>,
    /// Those chown gives.
    uids: Vec<u32>,
    gids: Vec<u32>,
}

impl Run {
    fn new() -> Self {
        // SAFETY: Plain calls returning the process's IDs, and its groups into a buffer of their
        // number.
        let (uid, gid, groups) = unsafe {
            let (uid, gid) = (libc::geteuid(), libc::getegid());
            let mut groups = vec![0; libc::getgroups(0, std::ptr::null_mut()) as usize];
            let len = libc::getgroups(groups.len() as i32, groups.as_mut_ptr());
            groups.truncate(len as usize);
            (uid, gid, groups)
        };
        if uid == 0 {
            let user =
                |id: u32, groups: &[u32]| Identity { uid: id, gid: id, groups: groups.to_vec() };
            return Self {
                root: true,
                identities: vec![user(0, &[]), user(2001, &[2001, 3000]), user(2002, &[2002])],
                uids: vec![0, 2001, 2002, 65534],
                gids: vec![0, 2001, 2002, 3000],
            };
        }
        // The user's group, another of its groups, and one not its own.
        let mut gids = vec![gid];
        gids.extend(groups.iter().find(|&&group| group != gid));
        gids.extend((3000..).find(|group| !groups.contains(group) && *group != gid));
        Self {
            root: false,
            identities: vec![Identity { uid, gid, groups }],
            uids: vec![uid, uid + 1],
            gids,
        }
    }
}

/// A filesystem calls are made on: its root directory, and the files a case holds open in it.
struct Side {
    root: PathBuf,
    /// The root, which calls name paths relative to: until [`close`](Self::close), as it holds a
    /// mount busy.
    dir: Option<OwnedFd>,
    fds: Vec<OwnedFd>,
}

impl Side {
    fn new(root: &Path) -> Self {
        let path = cstring(root.as_os_str().as_bytes());
        // SAFETY: open(2) of a NUL-terminated path, its descriptor owned at once.
        let dir = unsafe {
            let fd = libc::open(path.as_ptr(), libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC);
            assert!(fd >= 0, "{}: {}", root.display(), std::io::Error::last_os_error());
            OwnedFd::from_raw_fd(fd)
        };
        Self { root: root.to_owned(), dir: Some(dir), fds: Vec::new() }
    }

    /// Closes the files held, and the root's descriptor: a mount can then unmount.
    fn close(&mut self) {
        self.fds.clear();
        self.dir = None;
    }

    /// `path`, relative to the root's descriptor.
    fn at(&self, path: &P) -> CString {
        cstring(path.0.as_bytes())
    }

    /// `path`, whole.
    fn whole(&self, path: &P) -> CString {
        cstring(self.root.join(&path.0).as_os_str().as_bytes())
    }

    /// The open file `fd`, of those held: EBADF if none are.
    fn fd(&self, fd: usize) -> Result<i32, Errno> {
        match self.fds.len() {
            0 => Err(libc::EBADF),
            held => Ok(self.fds[fd % held].as_raw_fd()),
        }
    }

    /// Makes `op`'s call, and returns what it came to.
    fn call(&mut self, op: &Op) -> Outcome {
        let dir = self.dir.as_ref().expect("closed").as_raw_fd();
        let nofollow = |follow: bool| if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
        // SAFETY: Calls of the C library's, each with NUL-terminated strings and buffers that
        // outlive it, and descriptors this side holds.
        unsafe {
            match op {
                Op::Open { path, flags, mode } => {
                    let fd = ret(libc::openat(dir, self.at(path).as_ptr(), flags.bits(), *mode))?;
                    if self.fds.len() == MAX_FDS {
                        self.fds.remove(0);
                    }
                    self.fds.push(OwnedFd::from_raw_fd(fd as i32));
                    Ok(Out::Done)
                }
                Op::Close { fd } => {
                    self.fd(*fd)?;
                    let held = self.fds.len();
                    drop(self.fds.remove(fd % held));
                    Ok(Out::Done)
                }
                Op::Read { fd, len } => {
                    let mut buf = vec![0u8; *len];
                    let read =
                        ret(libc::read(self.fd(*fd)?, buf.as_mut_ptr().cast(), *len) as i64)?;
                    buf.truncate(read as usize);
                    Ok(Out::Bytes(Bytes(buf)))
                }
                Op::Write { fd, data } => {
                    let data = data.bytes();
                    let written = libc::write(self.fd(*fd)?, data.as_ptr().cast(), data.len());
                    Ok(Out::Size(ret(written as i64)?))
                }
                Op::Pread { fd, at, len } => {
                    let mut buf = vec![0u8; *len];
                    let read =
                        libc::pread(self.fd(*fd)?, buf.as_mut_ptr().cast(), *len, *at as i64);
                    buf.truncate(ret(read as i64)? as usize);
                    Ok(Out::Bytes(Bytes(buf)))
                }
                Op::Pwrite { fd, at, data } => {
                    let data = data.bytes();
                    let written =
                        libc::pwrite(self.fd(*fd)?, data.as_ptr().cast(), data.len(), *at as i64);
                    Ok(Out::Size(ret(written as i64)?))
                }
                Op::Seek { fd, at, whence } => {
                    let fd = self.fd(*fd)?;
                    let mut st: libc::stat = std::mem::zeroed();
                    ret(libc::fstat(fd, &mut st))?;
                    let seeked = ret(libc::lseek(fd, *at, *whence));
                    if st.st_mode & libc::S_IFMT == libc::S_IFDIR {
                        allowed(Allowed::DirectorySeeks);
                        return Ok(Out::Done);
                    }
                    Ok(Out::Size(seeked?))
                }
                Op::Ftruncate { fd, len } => {
                    ret(libc::ftruncate(self.fd(*fd)?, *len as i64))?;
                    Ok(Out::Done)
                }
                Op::Truncate { path, len } => {
                    ret(libc::truncate(self.whole(path).as_ptr(), *len as i64))?;
                    Ok(Out::Done)
                }
                Op::Fallocate { fd, at, len } => {
                    ret(libc::fallocate(self.fd(*fd)?, 0, *at as i64, *len as i64))?;
                    Ok(Out::Done)
                }
                Op::Fsync { fd, data_only } => {
                    let fd = self.fd(*fd)?;
                    ret(if *data_only { libc::fdatasync(fd) } else { libc::fsync(fd) })?;
                    Ok(Out::Done)
                }
                Op::Fstat { fd } => {
                    let mut st: libc::stat = std::mem::zeroed();
                    ret(libc::fstat(self.fd(*fd)?, &mut st))?;
                    Ok(Out::Stat(Stat::of(&st)))
                }
                Op::Fchmod { fd, mode } => {
                    ret(libc::fchmod(self.fd(*fd)?, *mode))?;
                    Ok(Out::Done)
                }
                Op::Mkdir { path, mode } => {
                    ret(libc::mkdirat(dir, self.at(path).as_ptr(), *mode))?;
                    Ok(Out::Done)
                }
                Op::Mknod { path, special, mode } => {
                    let (kind, dev) = special.mode_and_dev();
                    ret(libc::mknodat(dir, self.at(path).as_ptr(), kind | mode, dev))?;
                    Ok(Out::Done)
                }
                Op::Symlink { target, path } => {
                    ret(libc::symlinkat(self.at(target).as_ptr(), dir, self.at(path).as_ptr()))?;
                    Ok(Out::Done)
                }
                Op::Link { from, to, follow } => {
                    let flags = if *follow { libc::AT_SYMLINK_FOLLOW } else { 0 };
                    let (from, to) = (self.at(from), self.at(to));
                    ret(libc::linkat(dir, from.as_ptr(), dir, to.as_ptr(), flags))?;
                    Ok(Out::Done)
                }
                Op::Unlink { path } => {
                    ret(libc::unlinkat(dir, self.at(path).as_ptr(), 0))?;
                    Ok(Out::Done)
                }
                Op::Rmdir { path } => {
                    ret(libc::unlinkat(dir, self.at(path).as_ptr(), libc::AT_REMOVEDIR))?;
                    Ok(Out::Done)
                }
                Op::Rename { from, to, noreplace } => {
                    let flags = if *noreplace { libc::RENAME_NOREPLACE } else { 0 };
                    let (from, to) = (self.at(from), self.at(to));
                    ret(libc::renameat2(dir, from.as_ptr(), dir, to.as_ptr(), flags))?;
                    Ok(Out::Done)
                }
                Op::Chmod { path, mode } => {
                    ret(libc::fchmodat(dir, self.at(path).as_ptr(), *mode, 0))?;
                    Ok(Out::Done)
                }
                Op::Chown { path, uid, gid, follow } => {
                    let (uid, gid) = (uid.unwrap_or(u32::MAX), gid.unwrap_or(u32::MAX));
                    ret(libc::fchownat(dir, self.at(path).as_ptr(), uid, gid, nofollow(*follow)))?;
                    Ok(Out::Done)
                }
                Op::Utimens { path, atime, mtime, follow } => {
                    let times = [atime.timespec(), mtime.timespec()];
                    let path = self.at(path);
                    ret(libc::utimensat(dir, path.as_ptr(), times.as_ptr(), nofollow(*follow)))?;
                    let mut st: libc::stat = std::mem::zeroed();
                    ret(libc::fstatat(dir, path.as_ptr(), &mut st, nofollow(*follow)))?;
                    // Times set to now differ, as the two calls' nows do; those omitted are what
                    // they were, compared as the trees' are.
                    let set = |time: &Time, value| matches!(time, Time::At(..)).then_some(value);
                    Ok(Out::Times(
                        set(atime, (st.st_atime, st.st_atime_nsec)),
                        set(mtime, (st.st_mtime, st.st_mtime_nsec)),
                    ))
                }
                Op::Stat { path, follow } => {
                    let mut st: libc::stat = std::mem::zeroed();
                    ret(libc::fstatat(dir, self.at(path).as_ptr(), &mut st, nofollow(*follow)))?;
                    Ok(Out::Stat(Stat::of(&st)))
                }
                Op::Readlink { path } => {
                    let mut buf = vec![0u8; 4096];
                    let len = libc::readlinkat(
                        dir,
                        self.at(path).as_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                    );
                    buf.truncate(ret(len as i64)? as usize);
                    Ok(Out::Bytes(Bytes(buf)))
                }
                Op::Readdir { path } => {
                    let entries = std::fs::read_dir(self.root.join(&path.0))
                        .map_err(|error| error.raw_os_error().unwrap())?;
                    let mut listed = Vec::new();
                    for entry in entries {
                        let entry = entry.map_err(|error| error.raw_os_error().unwrap())?;
                        let file_type =
                            entry.file_type().map_err(|error| error.raw_os_error().unwrap())?;
                        listed.push((entry.file_name().into_string().unwrap(), d_type(file_type)));
                    }
                    listed.sort();
                    Ok(Out::Entries(listed))
                }
                Op::Access { path, mode } => {
                    // faccessat2's AT_EACCESS checks as the thread's fsuid and groups, as every
                    // other call here does; without it, as its real IDs, root's.
                    let path = self.at(path);
                    let checked = libc::syscall(
                        libc::SYS_faccessat2,
                        dir,
                        path.as_ptr(),
                        *mode,
                        libc::AT_EACCESS,
                    );
                    ret(checked)?;
                    Ok(Out::Done)
                }
                Op::Setxattr { path, name, value, flags, follow } => {
                    let (path, name, value) =
                        (self.whole(path), cstring(name.as_bytes()), value.bytes());
                    let set = match follow {
                        true => libc::setxattr,
                        false => libc::lsetxattr,
                    };
                    ret(set(
                        path.as_ptr(),
                        name.as_ptr(),
                        value.as_ptr().cast(),
                        value.len(),
                        *flags,
                    ))?;
                    Ok(Out::Done)
                }
                Op::Getxattr { path, name, size, follow } => {
                    let (path, name) = (self.whole(path), cstring(name.as_bytes()));
                    let mut buf = vec![0u8; size.len()];
                    let get = match follow {
                        true => libc::getxattr,
                        false => libc::lgetxattr,
                    };
                    let len =
                        ret(get(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len())
                            as i64)?;
                    if *size == XSize::Zero {
                        return Ok(Out::Size(len));
                    }
                    buf.truncate(len as usize);
                    Ok(Out::Bytes(Bytes(buf)))
                }
                Op::Listxattr { path, size, follow } => {
                    let path = self.whole(path);
                    let mut buf = vec![0u8; size.len()];
                    let list = match follow {
                        true => libc::listxattr,
                        false => libc::llistxattr,
                    };
                    let len = ret(list(path.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) as i64)?;
                    if *size == XSize::Zero {
                        return Ok(Out::Size(len));
                    }
                    buf.truncate(len as usize);
                    Ok(Out::Names(xattr_names(&buf)))
                }
                Op::Removexattr { path, name, follow } => {
                    let (path, name) = (self.whole(path), cstring(name.as_bytes()));
                    let remove = match follow {
                        true => libc::removexattr,
                        false => libc::lremovexattr,
                    };
                    ret(remove(path.as_ptr(), name.as_ptr()))?;
                    Ok(Out::Done)
                }
                Op::Umask { .. } | Op::As { .. } => unreachable!("made once, for both"),
            }
        }
    }

    /// The tree as it is, read through the kernel.
    fn tree(&self) -> Tree {
        let mut found = BTreeMap::new();
        let mut times = BTreeMap::new();
        let mut stack = vec![".".to_owned()];
        while let Some(path) = stack.pop() {
            // The root by its own path, which its parent's search permission resolves, not by
            // `.` in it, which its own would.
            let whole = match path.as_str() {
                "." => self.root.clone(),
                path => self.root.join(path),
            };
            let raw = |error: std::io::Error| error.raw_os_error().unwrap();
            let metadata = match std::fs::symlink_metadata(&whole) {
                Ok(metadata) => metadata,
                Err(error) => {
                    let unstatted = Contents::Unstatted(raw(error));
                    found.insert(path, (0, Stat::default(), unstatted, Ok(BTreeMap::new())));
                    continue;
                }
            };
            let stat = Stat::new(
                metadata.mode() & libc::S_IFMT,
                metadata.mode() & 0o7777,
                metadata.nlink(),
                (metadata.uid(), metadata.gid()),
                metadata.size(),
                (libc::major(metadata.rdev()), libc::minor(metadata.rdev())),
            );
            let nanos = |secs: i64, nsecs: i64| secs as Nanos * 1_000_000_000 + nsecs as Nanos;
            let mtime = nanos(metadata.mtime(), metadata.mtime_nsec());
            let ctime = nanos(metadata.ctime(), metadata.ctime_nsec());
            times.insert(path.clone(), (mtime, ctime));
            let file_type = metadata.file_type();
            let data = if file_type.is_file() {
                Contents::File(std::fs::read(&whole).map(Bytes).map_err(raw))
            } else if file_type.is_symlink() {
                Contents::Target(Bytes(
                    std::fs::read_link(&whole).unwrap().as_os_str().as_bytes().to_vec(),
                ))
            } else if file_type.is_dir() {
                Contents::Directory(std::fs::read_dir(&whole).map_err(raw).and_then(|entries| {
                    for entry in entries {
                        let name = entry.map_err(raw)?.file_name().into_string().unwrap();
                        stack.push(join(&path, &name));
                    }
                    Ok(())
                }))
            } else {
                Contents::Special
            };
            found.insert(path, (metadata.ino(), stat, data, xattrs(&whole)));
        }
        Tree { times, ..Tree::of(found) }
    }
}

/// The `d_type` of a directory entry of `file_type`: a regular file's for special files, as the
/// mount lists them ([`Allowed::SpecialFileTypes`]).
fn d_type(file_type: std::fs::FileType) -> u8 {
    allowed(Allowed::SpecialFileTypes);
    if file_type.is_dir() {
        libc::DT_DIR
    } else if file_type.is_symlink() {
        libc::DT_LNK
    } else {
        libc::DT_REG
    }
}

/// The names in a list listxattr returned, sorted.
fn xattr_names(list: &[u8]) -> Vec<Bytes> {
    let mut names: Vec<Bytes> = list
        .split(|&byte| byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| Bytes(name.to_vec()))
        .collect();
    names.sort();
    names
}

/// The extended attributes of the node at `path`, not following a symlink.
fn xattrs(path: &Path) -> Result<BTreeMap<Bytes, Bytes>, Errno> {
    let path = cstring(path.as_os_str().as_bytes());
    let mut list = vec![0u8; 1 << 16];
    // SAFETY: llistxattr(2) and lgetxattr(2), with NUL-terminated strings and buffers that outlive
    // the calls.
    unsafe {
        let len =
            ret(libc::llistxattr(path.as_ptr(), list.as_mut_ptr().cast(), list.len()) as i64)?;
        list.truncate(len as usize);
        let mut values = BTreeMap::new();
        for name in xattr_names(&list) {
            let mut value = vec![0u8; 1 << 16];
            let named = cstring(&name.0);
            let len = libc::lgetxattr(
                path.as_ptr(),
                named.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            );
            value.truncate(ret(len as i64)? as usize);
            values.insert(name, Bytes(value));
        }
        Ok(values)
    }
}

/// The tree of the filesystem `fs`, read through its nodes, as the store holds it.
async fn store_tree(fs: &Filesystem) -> Tree {
    let mut found = BTreeMap::new();
    let root = fs.root().clone();
    let attr = root.getattr().await.unwrap();
    let mut stack: Vec<(String, Node, fxdax::Attr)> = vec![(".".to_owned(), root, attr)];
    while let Some((path, node, attr)) = stack.pop() {
        // The device numbers the kernel encodes in FUSE's 32 bits (new_encode_dev).
        let rdev = ((attr.rdev & 0xfff00) >> 8, (attr.rdev & 0xff) | ((attr.rdev >> 12) & 0xfff00));
        let owner = (attr.uid, attr.gid);
        let kind = attr.file_type.mode_bits();
        let stat = Stat::new(kind, attr.perm as u32, attr.nlink as u64, owner, attr.size, rdev);
        let data = match attr.file_type {
            FileType::File => {
                let mut contents = vec![0; attr.size as usize];
                let read = node.read(0, &mut contents).await.unwrap();
                assert_eq!(read, contents.len(), "{path} read short");
                Contents::File(Ok(Bytes(contents)))
            }
            FileType::Symlink => Contents::Target(Bytes(node.readlink().await.unwrap())),
            FileType::Directory => {
                for entry in node.entries().await.unwrap() {
                    let (child, attr) = node.lookup(&entry.name).await.unwrap();
                    stack.push((
                        join(&path, std::str::from_utf8(&entry.name).unwrap()),
                        child,
                        attr,
                    ));
                }
                Contents::Directory(Ok(()))
            }
            _ => Contents::Special,
        };
        let xattrs = async {
            let mut xattrs = BTreeMap::new();
            for name in node.listxattr().await? {
                let value = node.getxattr(&name).await?;
                xattrs.insert(Bytes(name), Bytes(value));
            }
            Ok(xattrs)
        };
        let xattrs = xattrs.await.map_err(|errno: fxdax::Errno| errno.raw());
        found.insert(path, (attr.inode.0, stat, data, xattrs));
    }
    Tree::of(found)
}

/// The store's tree differs from the reference's, if it does: wholly, if the reference's was all
/// read; else in what the reference's walk could read, as a user's may not read all its run made
/// (a file its mode keeps from it, a directory it can't list).
fn store_difference(reference: &Tree, store: &Tree) -> Option<String> {
    if reference.whole() {
        return tree_difference(reference, store);
    }
    for (path, theirs) in &reference.entries {
        let Some(ours) = store.entries.get(path) else {
            return Some(format!("{path} is on the reference alone: {theirs:?}"));
        };
        if matches!(theirs.data, Contents::Unstatted(_)) {
            continue;
        }
        let read = matches!(theirs.data, Contents::File(Ok(_)) | Contents::Target(_));
        let differs = ours.stat != theirs.stat
            || (read && ours.data != theirs.data)
            || (theirs.xattrs.is_ok() && ours.xattrs != theirs.xattrs);
        if differs {
            return Some(format!(
                "{path} differs:\n  reference: {theirs:?}\n  store:     {ours:?}"
            ));
        }
    }
    // What the store has beyond is in a directory the reference's walk couldn't list, or read.
    let unlisted: Vec<&String> = (reference.entries.iter())
        .filter(|(_, entry)| {
            matches!(entry.data, Contents::Directory(Err(_)) | Contents::Unstatted(_))
        })
        .map(|(path, _)| path)
        .collect();
    let extra = store.entries.keys().find(|path| {
        !reference.entries.contains_key(*path)
            && !unlisted
                .iter()
                .any(|dir| path.starts_with(&format!("{dir}/")) || dir.as_str() == ".")
    });
    extra.map(|path| format!("{path} is in the store alone"))
}

/// Waits until the coarse clock, which tmpfs's times are taken from, has ticked: a time a call
/// sets from here on differs from every one set before.
fn tick() {
    let coarse = || {
        let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: clock_gettime(2) into a timespec this function owns.
        assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_REALTIME_COARSE, &mut now) }, 0);
        (now.tv_sec, now.tv_nsec)
    };
    let then = coarse();
    while coarse() == then {
        std::hint::spin_loop();
    }
}

/// Where a run makes its references: a tmpfs, `nodev` so that a device node made in it never
/// opens a device of the host's. Root's run mounts one of its own; a user's has the host's
/// `/dev/shm`.
struct References {
    dir: PathBuf,
    /// Root's tmpfs, unmounted as it goes.
    mounted: Option<tempfile::TempDir>,
}

impl References {
    fn new(run: &Run) -> Self {
        let (dir, mounted) = if run.root {
            let at = tempfile::tempdir().unwrap();
            let path = cstring(at.path().as_os_str().as_bytes());
            // SAFETY: mount(2), with NUL-terminated strings that outlive the call.
            let mounted = unsafe {
                libc::mount(
                    c"fxdax-reference".as_ptr(),
                    path.as_ptr(),
                    c"tmpfs".as_ptr(),
                    libc::MS_NODEV | libc::MS_NOSUID,
                    c"size=1g,mode=0755".as_ptr().cast(),
                )
            };
            assert_eq!(mounted, 0, "mounting a tmpfs: {}", std::io::Error::last_os_error());
            (at.path().to_owned(), Some(at))
        } else {
            (PathBuf::from("/dev/shm"), None)
        };
        let path = cstring(dir.as_os_str().as_bytes());
        // SAFETY: statfs(2) and statvfs(3) into structs this function owns.
        let (statfs, statvfs) = unsafe {
            let (mut statfs, mut statvfs) =
                (std::mem::zeroed::<libc::statfs>(), std::mem::zeroed::<libc::statvfs>());
            assert_eq!(libc::statfs(path.as_ptr(), &mut statfs), 0);
            assert_eq!(libc::statvfs(path.as_ptr(), &mut statvfs), 0);
            (statfs, statvfs)
        };
        assert_eq!(statfs.f_type, libc::TMPFS_MAGIC, "{} is not a tmpfs", dir.display());
        assert_ne!(statvfs.f_flag & libc::ST_NODEV, 0, "{} is not nodev", dir.display());
        Self { dir, mounted }
    }
}

impl Drop for References {
    fn drop(&mut self) {
        if let Some(at) = &self.mounted {
            let path = cstring(at.path().as_os_str().as_bytes());
            // SAFETY: umount2(2), with a NUL-terminated string that outlives the call.
            let unmounted = unsafe { libc::umount2(path.as_ptr(), libc::MNT_DETACH) };
            if unmounted != 0 {
                eprintln!("unmounting the references' tmpfs: {}", std::io::Error::last_os_error());
            }
        }
    }
}

/// What a case is made on: the reference, and the subject, a FUSE mount of a new store, or, to
/// test the test, another directory of the reference's tmpfs.
struct Case<'a> {
    run: &'a Run,
    reference: Side,
    subject: Side,
    mount: Option<Mount>,
    /// The subject's mount point, or directory.
    _subject_dir: tempfile::TempDir,
    reference_dir: tempfile::TempDir,
    /// What the case did, for its failure to say.
    history: Vec<String>,
}

/// The two kinds of subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Subject {
    Mount,
    Tmpfs,
}

impl<'a> Case<'a> {
    fn new(run: &'a Run, references: &References, subject: Subject) -> Self {
        let me = &run.identities[0];
        let reference_dir =
            tempfile::Builder::new().prefix("reference").tempdir_in(&references.dir).unwrap();
        let subject_dir = match subject {
            Subject::Mount => tempfile::tempdir().unwrap(),
            Subject::Tmpfs => {
                tempfile::Builder::new().prefix("subject").tempdir_in(&references.dir).unwrap()
            }
        };
        // SAFETY: Plain calls setting the process's umask.
        unsafe { libc::umask(0o022) };
        let mount = (subject == Subject::Mount).then(|| {
            let fs = block_on(Filesystem::format(Storage::memory(STORE_SIZE))).unwrap();
            // A user's run mounts a root of its own, as a store made for it would be; root's keeps
            // a new store's, which has no POSIX attributes of its own (0755, root's).
            if !run.root {
                let owner = SetAttr { uid: Some(me.uid), gid: Some(me.gid), ..Default::default() };
                block_on(fs.root().setattr(&owner)).unwrap();
            }
            MountOptions::new().mount(fs, subject_dir.path()).expect("mounting failed")
        });
        // The other roots alike: 0755, the run's own.
        let roots = match subject {
            Subject::Mount => vec![reference_dir.path()],
            Subject::Tmpfs => vec![reference_dir.path(), subject_dir.path()],
        };
        for dir in roots {
            let path = cstring(dir.as_os_str().as_bytes());
            // SAFETY: chown(2) and chmod(2) of NUL-terminated paths.
            unsafe {
                assert_eq!(libc::chown(path.as_ptr(), me.uid, me.gid), 0);
                assert_eq!(libc::chmod(path.as_ptr(), 0o755), 0);
            }
        }
        Self {
            run,
            reference: Side::new(reference_dir.path()),
            subject: Side::new(subject_dir.path()),
            mount,
            _subject_dir: subject_dir,
            reference_dir,
            history: Vec::new(),
        }
    }

    /// The trees of both, read as the run's own identity, which may read all of root's.
    fn trees(&self, now: &Identity) -> (Tree, Tree) {
        let me = &self.run.identities[0];
        if self.run.root {
            me.assume();
        }
        let trees = (self.reference.tree(), self.subject.tree());
        if self.run.root {
            now.assume();
        }
        trees
    }

    /// Fails with what differed at the `index`th call, and what the case did.
    fn failed(&self, index: usize, what: String) -> String {
        format!("call {index}: {what}\n\nthe case:\n{}", self.history.join("\n"))
    }

    /// Makes `ops`' calls on both, comparing as it goes; then, with a mount, unmounts it, checks
    /// its store, and compares the store's tree with the reference's.
    fn run(&mut self, ops: &[Op]) -> Result<(), String> {
        let mut now = self.run.identities[0].clone();
        let (mut theirs, mut ours) = self.trees(&now);
        if let Some(difference) = tree_difference(&theirs, &ours) {
            return Err(self.failed(0, format!("the roots differ: {difference}")));
        }
        for (index, op) in ops.iter().enumerate() {
            tick();
            let outcomes = match op {
                Op::Umask { mask } => {
                    // SAFETY: Sets the process's umask.
                    unsafe { libc::umask(*mask) };
                    (Ok(Out::Done), Ok(Out::Done))
                }
                Op::As { who } => {
                    now = self.run.identities[who % self.run.identities.len()].clone();
                    now.assume();
                    (Ok(Out::Done), Ok(Out::Done))
                }
                op => (self.reference.call(op), self.subject.call(op)),
            };
            self.history.push(format!("{index}: {op:?} -> {:?}", outcomes.0));
            if outcomes.0 != outcomes.1 {
                let (reference, subject) = outcomes;
                return Err(self.failed(
                    index,
                    format!("{op:?}\n  reference: {reference:?}\n  mount:     {subject:?}"),
                ));
            }
            let (reference, subject) = self.trees(&now);
            if let Some(difference) = tree_difference(&reference, &subject) {
                return Err(self.failed(index, format!("{op:?}: after it, {difference}")));
            }
            let (changed, ours_changed) =
                (times_changed(&theirs, &reference), times_changed(&ours, &subject));
            let allowed_times = |path: &String, their_flags: &(bool, bool), ours: &(bool, bool)| {
                // A truncate(2) to the file's own size ([`Allowed::TruncateToItsSize`]): of the
                // file its path names, a symlink's target maybe, the one file it changes.
                let size = |tree: &Tree| {
                    tree.entries
                        .get(path)
                        .filter(|entry| entry.stat.kind == libc::S_IFREG)
                        .map(|entry| entry.stat.size)
                };
                let truncated_to_its_size = matches!(op, Op::Truncate { .. })
                    && size(&reference).is_some()
                    && size(&reference) == size(&theirs)
                    && *their_flags == (false, false)
                    && *ours == (true, true);
                // A FIFO's reads and writes ([`Allowed::PipeTimes`]).
                let piped = matches!(op, Op::Read { .. } | Op::Write { .. })
                    && reference
                        .entries
                        .get(path)
                        .is_some_and(|entry| entry.stat.kind == libc::S_IFIFO)
                    && *ours == (false, false);
                match (truncated_to_its_size, piped) {
                    (true, _) => allowed(Allowed::TruncateToItsSize) != "",
                    (_, true) => allowed(Allowed::PipeTimes) != "",
                    _ => false,
                }
            };
            if let Some((path, flags)) = changed.iter().find(|(path, flags)| {
                let ours = ours_changed.get(*path);
                ours != Some(flags) && !ours.is_some_and(|ours| allowed_times(path, flags, ours))
            }) {
                let says = |(mtime, ctime): (bool, bool)| {
                    format!("mtime {}, ctime {}", changes(mtime), changes(ctime))
                };
                let ours = ours_changed.get(path).copied().unwrap();
                return Err(self.failed(
                    index,
                    format!(
                        "{op:?}: of {path}, on the reference it {}; on the mount, {}",
                        says(*flags),
                        says(ours)
                    ),
                ));
            }
            (theirs, ours) = (reference, subject);
        }
        if self.run.root {
            self.run.identities[0].assume();
        }
        // SAFETY: Sets the process's umask back.
        unsafe { libc::umask(0o022) };
        self.reference.close();
        self.subject.close();
        let Some(mount) = self.mount.take() else { return Ok(()) };
        mount.unmount(false).map_err(|error| format!("unmounting: {error:#}"))?;
        let storage = mount.join().map_err(|error| format!("closing: {error:#}"))?;
        let storage = block_on(storage.check())
            .map_err(|error| format!("the store is not whole: {error:#}"))?;
        let store = block_on(async {
            let fs = Filesystem::mount(storage).await.unwrap();
            let tree = store_tree(&fs).await;
            fs.close().await.unwrap();
            tree
        });
        match store_difference(&theirs, &store) {
            Some(difference) => {
                Err(self.failed(ops.len(), format!("after unmounting, {difference}")))
            }
            None => Ok(()),
        }
    }
}

/// Whether a time changed, as a failure says it.
fn changes(changed: bool) -> &'static str {
    if changed { "changed" } else { "stayed" }
}

impl Drop for Case<'_> {
    fn drop(&mut self) {
        if self.run.root {
            self.run.identities[0].assume();
        }
        self.reference.close();
        self.subject.close();
        // A mount not unmounted yet, as the case failed: unmounted lazily, and closed.
        drop(self.mount.take());
        // Removable: root's run as root, which removes anything; a user's with its directories
        // its own to write and search, as all it made is its own.
        if !self.run.root {
            for dir in [self.reference_dir.path(), self.subject.root.as_path()] {
                make_removable(dir);
            }
        }
    }
}

/// Lets the user remove what is under `dir`: each directory its own to read, write and search.
fn make_removable(dir: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(dir) else { return };
    if !metadata.is_dir() {
        return;
    }
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            make_removable(&entry.path());
        }
    }
}

// The generator: calls on a few names, nested a few deep, so that they find what others made.

fn name() -> BoxedStrategy<String> {
    prop_oneof![
        24 => prop::sample::select(&["a", "b", "c", "d"][..]).prop_map(str::to_owned),
        // The longest name, and one longer.
        1 => Just("l".repeat(255)),
        1 => Just("m".repeat(256)),
    ]
    .boxed()
}

fn path() -> BoxedStrategy<P> {
    prop_oneof![
        1 => Just(P(".".to_owned())),
        10 => name().prop_map(P),
        6 => (name(), name()).prop_map(|(a, b)| P(format!("{a}/{b}"))),
        2 => (name(), name(), name()).prop_map(|(a, b, c)| P(format!("{a}/{b}/{c}"))),
    ]
    .boxed()
}

/// A symlink's target: a path of the tree's, relative to the symlink, never `..`, which would
/// leave the tree from its root, for the two roots' different parents.
fn target() -> BoxedStrategy<P> {
    prop_oneof![
        8 => path().prop_filter("not the root", |path| path.0 != "."),
        // A name no call makes: dangling.
        1 => Just(P("x".to_owned())),
    ]
    .boxed()
}

fn mode() -> BoxedStrategy<u32> {
    allowed(Allowed::SetgidWithoutGroupExecute);
    prop_oneof![
        4 => prop::sample::select(
            &[
                0o755, 0o644, 0o777, 0o700, 0o600, 0o000, 0o2775, 0o2755, 0o2710, 0o1777, 0o1770,
                0o4755, 0o6755, 0o4644, 0o640, 0o711, 0o070, 0o007, 0o4000, 0o2010,
            ][..]
        ),
        1 => (0u32..0o10000).prop_filter("setgid without group-execute", |mode| {
            mode & 0o2000 == 0 || mode & 0o010 != 0
        }),
    ]
    .boxed()
}

/// A new directory's mode: setgid often, whose new nodes take its group, and new directories its
/// setgid, which the server gives them.
fn dir_mode() -> BoxedStrategy<u32> {
    prop_oneof![
        2 => mode(),
        1 => prop::sample::select(&[0o2775, 0o2777, 0o3777, 0o2755][..]),
    ]
    .boxed()
}

fn data() -> BoxedStrategy<Data> {
    let len = prop_oneof![
        30 => prop::sample::select(&[0, 1, 7, 100, 4095, 4096, 4097, 5000, 65536, 70000][..]),
        30 => 0usize..10_000,
        // Past the most the kernel writes at once.
        1 => Just((1 << 20) + 4097),
    ];
    (len, any::<u8>()).prop_map(|(len, seed)| Data { len, seed }).boxed()
}

fn offset() -> BoxedStrategy<u64> {
    prop_oneof![
        3 => prop::sample::select(&[0, 1, 4095, 4096, 4097, 65535, 65536, (1 << 20) + 1][..]),
        2 => 0u64..150_000,
    ]
    .boxed()
}

fn size() -> BoxedStrategy<usize> {
    prop::sample::select(&[0, 1, 100, 4096, 5000, 70000][..]).boxed()
}

fn flags() -> BoxedStrategy<OpenFlags> {
    let access = prop::sample::select(&[libc::O_RDONLY, libc::O_WRONLY, libc::O_RDWR][..]);
    let bit = |percent: u32| (0u32..100).prop_map(move |roll| roll < percent);
    (access, bit(60), bit(20), bit(20), bit(15), bit(5), bit(10))
        .prop_map(|(access, creat, excl, trunc, append, directory, nofollow)| OpenFlags {
            access,
            creat,
            excl,
            trunc,
            append,
            directory,
            nofollow,
        })
        .boxed()
}

fn time() -> BoxedStrategy<Time> {
    allowed(Allowed::TimesOutsideFxfs);
    // Between the epoch and 2554, which Fxfs keeps; a second's nanoseconds, invalid.
    let secs = prop::sample::select(&[0i64, 1, 1_000_000_000, 1_700_000_000, 4_102_444_800][..]);
    let nsecs = prop::sample::select(&[0i64, 1, 123_456_789, 999_999_999, 1_000_000_000][..]);
    prop_oneof![
        1 => Just(Time::Now),
        1 => Just(Time::Omit),
        3 => (secs, nsecs).prop_map(|(secs, nsecs)| Time::At(secs, nsecs)),
    ]
    .boxed()
}

fn special() -> BoxedStrategy<Special> {
    let dev = prop::sample::select(&[(1u32, 3u32), (8, 0), (4095, 0xfffff), (0, 0)][..]);
    prop_oneof![
        3 => Just(Special::Fifo),
        2 => Just(Special::Socket),
        1 => dev.clone().prop_map(|(major, minor)| Special::Char(major, minor)),
        1 => dev.prop_map(|(major, minor)| Special::Block(major, minor)),
    ]
    .boxed()
}

/// Extended attributes' names: of the user namespace, one long; of the others root may set, or
/// not; and of none.
fn xattr_name() -> BoxedStrategy<&'static str> {
    allowed(Allowed::PosixAcls);
    static LONG: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| format!("user.{}", "x".repeat(200)));
    let long: &'static str = LONG.as_str();
    prop_oneof![
        8 => prop::sample::select(&["user.a", "user.b"][..]),
        1 => Just(long),
        2 => prop::sample::select(&["trusted.t", "security.s"][..]),
        1 => prop::sample::select(&["user.", "foo.bar"][..]),
    ]
    .boxed()
}

fn xattr_value() -> BoxedStrategy<Data> {
    let len = prop_oneof![10 => 0usize..300, 1 => Just(5000usize)];
    (len, any::<u8>()).prop_map(|(len, seed)| Data { len, seed }).boxed()
}

fn xsize() -> BoxedStrategy<XSize> {
    prop::sample::select(&[XSize::Zero, XSize::Small, XSize::Large, XSize::Large][..]).boxed()
}

/// Calls as `run` makes them.
fn op(run: &Run) -> BoxedStrategy<Op> {
    for left_out in [
        Allowed::NonUtf8Names,
        Allowed::RenameExchange,
        Allowed::FallocateModes,
        Allowed::Holes,
        Allowed::Tmpfile,
    ] {
        allowed(left_out);
    }
    let fd = || 0usize..MAX_FDS;
    let uid = prop::option::weighted(0.8, prop::sample::select(run.uids.clone()));
    let gid = prop::option::weighted(0.8, prop::sample::select(run.gids.clone()));
    let mut ops: Vec<(u32, BoxedStrategy<Op>)> = vec![
        (
            12,
            (path(), flags(), mode())
                .prop_map(|(path, flags, mode)| Op::Open { path, flags, mode })
                .boxed(),
        ),
        (3, fd().prop_map(|fd| Op::Close { fd }).boxed()),
        (4, (fd(), size()).prop_map(|(fd, len)| Op::Read { fd, len }).boxed()),
        (6, (fd(), data()).prop_map(|(fd, data)| Op::Write { fd, data }).boxed()),
        (3, (fd(), offset(), size()).prop_map(|(fd, at, len)| Op::Pread { fd, at, len }).boxed()),
        (
            6,
            (fd(), offset(), data()).prop_map(|(fd, at, data)| Op::Pwrite { fd, at, data }).boxed(),
        ),
        (
            2,
            (
                fd(),
                -10i64..100_000,
                prop::sample::select(&[libc::SEEK_SET, libc::SEEK_CUR, libc::SEEK_END][..]),
            )
                .prop_map(|(fd, at, whence)| Op::Seek { fd, at, whence })
                .boxed(),
        ),
        (3, (fd(), offset()).prop_map(|(fd, len)| Op::Ftruncate { fd, len }).boxed()),
        (3, (path(), offset()).prop_map(|(path, len)| Op::Truncate { path, len }).boxed()),
        (
            2,
            (fd(), offset(), 0u64..70_000)
                .prop_map(|(fd, at, len)| Op::Fallocate { fd, at, len })
                .boxed(),
        ),
        (2, (fd(), any::<bool>()).prop_map(|(fd, data_only)| Op::Fsync { fd, data_only }).boxed()),
        (3, fd().prop_map(|fd| Op::Fstat { fd }).boxed()),
        (2, (fd(), mode()).prop_map(|(fd, mode)| Op::Fchmod { fd, mode }).boxed()),
        (8, (path(), dir_mode()).prop_map(|(path, mode)| Op::Mkdir { path, mode }).boxed()),
        (
            3,
            (path(), special(), mode())
                .prop_map(|(path, special, mode)| Op::Mknod { path, special, mode })
                .boxed(),
        ),
        (4, (target(), path()).prop_map(|(target, path)| Op::Symlink { target, path }).boxed()),
        (
            4,
            (path(), path(), any::<bool>())
                .prop_map(|(from, to, follow)| Op::Link { from, to, follow })
                .boxed(),
        ),
        (5, path().prop_map(|path| Op::Unlink { path }).boxed()),
        (3, path().prop_map(|path| Op::Rmdir { path }).boxed()),
        (
            6,
            (path(), path(), any::<bool>())
                .prop_map(|(from, to, noreplace)| Op::Rename { from, to, noreplace })
                .boxed(),
        ),
        (5, (path(), mode()).prop_map(|(path, mode)| Op::Chmod { path, mode }).boxed()),
        (
            4,
            (path(), uid, gid, any::<bool>())
                .prop_map(|(path, uid, gid, follow)| Op::Chown { path, uid, gid, follow })
                .boxed(),
        ),
        (
            3,
            (path(), time(), time(), any::<bool>())
                .prop_map(|(path, atime, mtime, follow)| Op::Utimens { path, atime, mtime, follow })
                .boxed(),
        ),
        (3, (path(), any::<bool>()).prop_map(|(path, follow)| Op::Stat { path, follow }).boxed()),
        (1, path().prop_map(|path| Op::Readlink { path }).boxed()),
        (3, path().prop_map(|path| Op::Readdir { path }).boxed()),
        (
            3,
            (
                path(),
                prop::sample::select(
                    &[libc::F_OK, libc::R_OK, libc::W_OK, libc::X_OK, libc::R_OK | libc::W_OK][..],
                ),
            )
                .prop_map(|(path, mode)| Op::Access { path, mode })
                .boxed(),
        ),
        (
            4,
            (
                path(),
                xattr_name(),
                xattr_value(),
                prop::sample::select(&[0, libc::XATTR_CREATE, libc::XATTR_REPLACE][..]),
                any::<bool>(),
            )
                .prop_map(|(path, name, value, flags, follow)| Op::Setxattr {
                    path,
                    name,
                    value,
                    flags,
                    follow,
                })
                .boxed(),
        ),
        (
            3,
            (path(), xattr_name(), xsize(), any::<bool>())
                .prop_map(|(path, name, size, follow)| Op::Getxattr { path, name, size, follow })
                .boxed(),
        ),
        (
            2,
            (path(), xsize(), any::<bool>())
                .prop_map(|(path, size, follow)| Op::Listxattr { path, size, follow })
                .boxed(),
        ),
        (
            2,
            (path(), xattr_name(), any::<bool>())
                .prop_map(|(path, name, follow)| Op::Removexattr { path, name, follow })
                .boxed(),
        ),
        (
            1,
            prop::sample::select(&[0o022, 0o000, 0o002, 0o027, 0o066][..])
                .prop_map(|mask| Op::Umask { mask })
                .boxed(),
        ),
    ];
    if run.identities.len() > 1 {
        ops.push((4, (0..run.identities.len()).prop_map(|who| Op::As { who }).boxed()));
    }
    Union::new_weighted(ops).boxed()
}

/// Runs `cases` random cases on `subject`.
fn differential(subject: Subject, cases: u32) {
    let run = Run::new();
    let references = References::new(&run);
    let config =
        Config { cases, failure_persistence: None, max_shrink_time: 600_000, ..Config::default() };
    let ops = proptest::collection::vec(op(&run), 1..64);
    let result = TestRunner::new(config).run(&ops, |ops| {
        let mut case = Case::new(&run, &references, subject);
        case.run(&ops).map_err(TestCaseError::fail)
    });
    if let Err(error) = result {
        panic!("{error}");
    }
}

/// TASK-149.37: random calls on a FUSE mount of a new store come to what they come to on Linux's
/// tmpfs, as do its tree and times after each, and the store's tree once unmounted: see the
/// module's docs.
#[test]
fn random_calls_on_a_mount_come_to_what_they_do_on_tmpfs() {
    let cases =
        std::env::var("FXDAX_FUSE_DIFFERENTIAL_CASES").map_or(256, |cases| cases.parse().unwrap());
    differential(Subject::Mount, cases);
}

/// The test's own test: two directories of one tmpfs agree, however the calls go, their times
/// too. A difference here would be the test's.
#[test]
fn two_tmpfs_directories_agree() {
    differential(Subject::Tmpfs, 16);
}
