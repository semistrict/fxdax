// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Random sequences of what a VFS asks of Fxfs, checked against a plain model of files as pages:
//! no VM, no kernel, seconds. Offsets lean on the awkward ones, a block's edges, a chunk's, 4 GiB
//! and the largest a file may reach, and sequences remount and compact between their operations,
//! so that what holds until a file is reopened is found: xfstests found those in a guest, in
//! minutes, one at a time (generic/130, 303, 423, 525). Each step reads back what it changed; each
//! sequence ends with every file compared, across a remount, and fsck on both sides of it.
//!
//! Some sequences have a device too small for what they write, where anything that grows may be
//! refused and leaves things as they were, and what shrinks mustn't be. Some write and clone
//! thousands of fragments at once, for the transactions that makes, which the journal must hold:
//! none may take more of it than one may, whatever it borrows (`Setting::builder`).
//!
//! Power cuts: the device records itself crashing at random barriers, with any of what it wrote
//! since the barrier before lost (`storage_device::fake_device::Crashes`). What each holds, replayed,
//! must pass fsck, be what the model was after one of the operations since the last sync, show none
//! of what was cut off past a file's end, and go on with the sequence. The power is cut during that
//! replay too, which must leave the same.
//!
//! Seeded by proptest, which says the sequence if one fails, shrunk.
//!
//! FXDAX_MODEL_CASES asks for more sequences, to hunt, and FXDAX_MODEL_OPS for longer ones.
//! FXDAX_MODEL_FSCK does the hunt with fsck after every transaction, slowly: corruption is found at
//! the transaction that made it, not operations later. FXDAX_MODEL_WORKERS has tasks apply queued
//! unlinks as a guest's do, by the clock, which a seed then doesn't replay.
//!
//! Random cuts took minutes to reach some bugs. Short sequences are also cut at every barrier, with
//! every subset of what it may lose, up to a bound (`every_cut_of_a_short_sequence`), which
//! FXDAX_MODEL_EVERY_CUT asks for more of.
//!
//! Devices fail too: some sequences' requests do, as a seed says, until an operation doesn't survive
//! it (`a_device_that_fails_loses_nothing_durable`, FXDAX_MODEL_FAILURES): which a filesystem must
//! say, and leave what was durable as it was, without panicking, in any task.
//!
//! Names are in directories that move, and go, as files do, across them and for each other, which
//! are transactions of many objects. Files may be open as they lose their names, or be made with
//! none, as what Fxfs keeps in the graveyard meanwhile: they stay read and written until they close.
//! A cut closes them, and leaves nothing of them: no object, no entry in the graveyard, no extent.
//! Only what a VFS would ask is asked: `Model::may_rename` and `Op::Link` say what it refuses, and
//! files unlink as a guest's do, queued (`Op::UnlinkLater`), which a cut may find not yet done.
//!
//! Every node's attributes are the model's too, as Linux's VFS sees them (`Node`): its type and
//! mode, owner, group, device number, times and extended attributes, the root's from the defaults
//! Fxfs gives a node it made none for. Times are checked as far as Fxfs keeps them: what a setattr
//! sets reads back, and what Fxfs takes from the clock is no earlier than the operation that took
//! it began; what the VFS keeps, a file's data's times, moves no earlier (`Time`). Names are as
//! long as a name may be, of many bytes a character, in directories that fold case; a readdir
//! goes on after the name it left off at while names come and go. Symlinks say targets of every
//! length a VFS asks for, up to `MAX_LONG_SYMLINK`, which Fxfs keeps outside their records when
//! long, in extended attributes listing leaves out. Files grow by fallocate as well as by writes,
//! which sets nothing aside on an extent store, and by the page cache's writeback, paid for by the
//! room reserved as its pages were dirtied, which a full filesystem refuses instead (`write_dirty`);
//! nodes are FIFOs, sockets and devices too. `statfs` counts at least what the model must take.
//!
//! A quarter of the sequences run on the guest's platform, whose VFS keeps directories' times, a
//! create of a file leaving them to it (`Seeded`). The volume is snapshotted too, each snapshot a
//! volume of its own that holds what the volume did then, whatever it does after, across remounts
//! and cuts, the graveyard tombstoning what it has queued meanwhile (TASK-241).

use super::*;
use fxfs::filesystem::{FlushReason, ForceMajor, FxFilesystemBuilder};
use fxfs::fsck::{FsckOptions, fsck_with_options};
use fxfs::hooks::Hooks;
use fxfs::lsm_tree::Query;
use fxfs::lsm_tree::types::{ItemRef, LayerIterator};
use fxfs::object_store::directory::{MAX_INLINE_SYMLINK, MAX_LONG_SYMLINK};
use fxfs::object_store::graveyard::Graveyard;
use fxfs::object_store::journal::JournalOptions;
use fxfs::object_store::object_record::ObjectKeyData;
use fxfs::object_store::transaction::TRANSACTION_MAX_JOURNAL_USAGE;
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{Extent, ExtentMode, ExtentValue, NewChildStoreOptions, StoreOptions};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed, TestRunner};
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::LazyLock;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc;
use std::time::Duration;
use storage_device::DeviceHolder;
use storage_device::fake_device::{Crashes, FakeDevice, Op as DeviceOp};

const PAGE: u64 = 4096;
const FILE: u32 = S_IFREG | 0o644;
const DIR: u32 = S_IFDIR | 0o755;
const SYMLINK: u32 = S_IFLNK | 0o777;
/// What of a mode isn't its type: its permissions, and fxdax's attribute flags (chattr), which it
/// keeps above the kernel's 16 bits (`Attr::mode`).
const PERMISSIONS: u32 = 0o7777 | 0xf << 16;
/// What a setattr may change but a file's size: what the kernel writes back of its inode.
const SET_ANY: u32 = SET_MODE | SET_UID | SET_GID | SET_ATIME | SET_MTIME | SET_CTIME;
/// The largest size a file may have.
const LARGEST: u64 = i64::MAX as u64;
/// The whole pages a file may have.
const PAGES_AT_MOST: u64 = LARGEST / PAGE;
/// The names a sequence uses in a directory, of each spelling: few, so that they meet.
const NAMES: usize = 6;
/// The directories a name may be in, of those there are: it's `name / NAMES`th, the root first, so
/// that a name of less than `NAMES` is in the root whatever there is, and sequences shrink to it.
const DIRS: usize = 4;
/// The spellings of names (`SPELLINGS`): letters, as `name / (NAMES * DIRS)` says, and more.
const ALPHABETS: usize = 2;
/// The extended attributes' names a sequence uses (`xattr_of`).
const XATTR_NAMES: u8 = 12;
/// The readdirs a sequence keeps going on with (`Cursor`).
const CURSORS: usize = 4;
/// What `Op::BadName` asks for: names Fxfs's, which are UTF-8, can't be.
const BAD_NAMES: [&[u8]; 4] = [b"\xff", b"a\xc0\xafb", b"\xed\xa0\x80", b"\xf4\x90\x80\x80"];
/// The objects of a volume with nothing in it: its root directory and its graveyard.
const BARE_OBJECTS: u64 = 2;
/// What the journal may take during what crosses extents, for each, and whatever else is going on:
/// what it crosses between them costs nothing. A commit a run of bytes, counting holes, makes a
/// clone across exabytes of them run until a device filled, which it may say: only this says that
/// it shouldn't have.
const JOURNALED_PER_EXTENT: u64 = 1 << 10;
const JOURNALED_ANYWAY: u64 = 128 << 10;
/// The most unlinks a sequence leaves queued: every subset of them is what a cut may find undone.
const QUEUED_AT_MOST: usize = 4;
/// The room a sequence has: enough, or less than a few of its fills want.
const ROOMY: u64 = 32768;
const TIGHT: u64 = 2048;
/// More blocks than a transaction of a fill holds, all of which it leaves free as it's refused.
const BLOCKS_A_FILL_MAY_LEAVE: u64 = 1024;
/// How much of a file is read whole, holes and all: a file may be as large as a file may be.
const WHOLLY_READ: u64 = 64 << 20;
/// The volume the sequences' files are in.
const VOLUME: &str = "test";
/// The snapshots a sequence takes of it, at most.
const SNAPSHOTS: usize = 2;
/// A power cut is recorded at one barrier in this many.
const CUTS_ONE_IN: u32 = 3;
/// A power cut of a replay is recorded at one barrier in this many.
const SECOND_CUTS_ONE_IN: u32 = 2;
/// How long a batched graveyard waits between batches: for as long as nothing asks it to go on, so
/// that a seed says when it does.
const BATCHES_APART: Duration = Duration::from_secs(3600);
/// How long a sequence may take. A filesystem that spins forever is a failure proptest can't see,
/// waiting for the sequence to end: a graveyard walking every attribute ID was found only by the
/// time a run took.
const SEQUENCE_LIMIT: Duration = Duration::from_secs(120);
/// How long a failing case shrinks, at most, in milliseconds, unless PROPTEST_MAX_SHRINK_TIME says:
/// then it says what it has. Unbounded, a case each run of which stalled 60 s before it panicked
/// (debug_assert_not_too_long), and so fed the watchdog, shrank past CI's 90 minutes, its report
/// lost with the job (TASK-233).
const SHRINK_FOR_AT_MOST: u32 = 600_000;

/// The runner's configuration of `cases` cases, each failure shrinking for [`SHRINK_FOR_AT_MOST`].
fn config(cases: u32) -> Config {
    let default = Config::default();
    let max_shrink_time = match default.max_shrink_time {
        0 => SHRINK_FOR_AT_MOST,
        set => set,
    };
    Config { cases, failure_persistence: None, max_shrink_time, ..default }
}
/// The operations a filesystem that survived a power cut goes on with.
const GOES_ON_FOR: usize = 4;
/// The most a barrier may lose, a free or an extent's writes each, for every subset of it to be
/// tried: past that, losing nothing, all of it, each alone, or all but each, of the barriers the
/// test counts. Each more than doubles what a barrier takes.
const EVERY_SUBSET_OF: usize = 6;
/// A request fails, in a sequence with failures, one in as many as one of these: of one kind, or
/// another, as most are reads, which end a sequence before a flush can fail.
const FAILS_ONE_IN: [u32; 3] = [4, 32, 256];
/// How long a short sequence may be, for every cut of it.
const SHORT: usize = 6;
/// How much of the journal a short sequence fills before compactions start, on their own, among
/// its operations, and each extension of its file: a few transactions.
const SHORT_RECLAIM: u64 = 16 << 10;

/// What a node's time is known to be: what a setattr set it to, or one Fxfs took from the clock,
/// or the VFS may have, no earlier than this, and no later than now.
#[derive(Clone, Copy, Debug)]
enum Time {
    Is(u64),
    Since(u64),
}

impl Time {
    /// What it is once something may have moved it to now, or not: no earlier.
    fn no_earlier(self) -> Self {
        match self {
            Time::Is(time) | Time::Since(time) => Time::Since(time),
        }
    }

    /// Whether `got` is what it is, `now` being now.
    fn holds(self, got: u64, now: u64) -> bool {
        match self {
            Time::Is(time) => got == time,
            Time::Since(time) => time <= got && got <= now,
        }
    }
}

/// Now, as Fxfs takes it for a node's times.
fn clock() -> u64 {
    Timestamp::now().as_nanos()
}

/// A node's model but for a file's data: its attributes, as Linux's VFS sees them, and its
/// extended attributes.
#[derive(Clone, Debug)]
struct Node {
    /// Its type, permissions and attribute flags.
    mode: u32,
    uid: u32,
    gid: u32,
    rdev: u64,
    atime: Time,
    mtime: Time,
    ctime: Time,
    xattrs: BTreeMap<u8, Vec<u8>>,
    /// Of those, the ones Fxfs keeps outside its record, in an attribute of their own: once one is,
    /// it stays there, however short (`set_extended_attribute_impl`).
    outside: BTreeSet<u8>,
    /// A symlink's target; nothing, any other node's.
    target: Vec<u8>,
}

impl Node {
    /// One made no earlier than `made`, its times Fxfs's.
    fn new(mode: u32, uid: u32, gid: u32, rdev: u64, made: u64) -> Self {
        let made = Time::Since(made);
        let (atime, mtime, ctime) = (made, made, made);
        let (xattrs, outside, target) = (BTreeMap::new(), BTreeSet::new(), Vec::new());
        Self { mode, uid, gid, rdev, atime, mtime, ctime, xattrs, outside, target }
    }

    /// A symlink to `target`, made no earlier than `made`.
    fn symlink(target: &Target, made: u64) -> Self {
        Self { target: target.bytes(), ..Self::new(SYMLINK, 0, 0, 0, made) }
    }

    /// Its extended attribute `name` set to `value`, or removed, if there's none: its status
    /// changes, which the VFS times.
    fn set_xattr(&mut self, name: u8, value: Option<Vec<u8>>) {
        match value {
            Some(value) => {
                if value.len() > MAX_INLINE_XATTR_SIZE {
                    self.outside.insert(name);
                }
                self.xattrs.insert(name, value);
            }
            None => {
                self.xattrs.remove(&name);
                self.outside.remove(&name);
            }
        }
        self.ctime = self.ctime.no_earlier();
    }

    fn kind(&self) -> u32 {
        self.mode & S_IFMT
    }

    /// The blocks its extended attributes take: those Fxfs keeps outside its record take theirs.
    fn xattr_blocks(&self) -> u64 {
        let outside = self.outside.iter().map(|name| self.xattrs[name].len() as u64);
        outside.map(|len| len.div_ceil(PAGE)).sum()
    }

    /// Its status changed, no earlier than `now`: a link to it came or went, or it moved.
    fn changed(&mut self, now: u64) {
        self.ctime = Time::Since(now);
    }

    /// A directory whose names changed no earlier than `now`, which Fxfs times, as the host's VFS
    /// leaves it to (`Platform::KEEPS_DIRECTORY_TIMES`).
    fn renamed(&mut self, now: u64) {
        (self.mtime, self.ctime) = (Time::Since(now), Time::Since(now));
    }

    /// Its data changed, or may have, whose times the VFS keeps, by a setattr: no earlier.
    fn written(&mut self) {
        (self.mtime, self.ctime) = (self.mtime.no_earlier(), self.ctime.no_earlier());
    }

    /// What a setattr of `set` no earlier than `now` makes of it: its status changes with any of
    /// it, at the time `set` says, if it says one.
    fn set(&mut self, set: &SetAttr, now: u64) {
        let given = |bit| set.valid & bit != 0;
        if given(SET_MODE) {
            self.mode = set.mode;
        }
        if given(SET_UID) {
            self.uid = set.uid;
        }
        if given(SET_GID) {
            self.gid = set.gid;
        }
        if given(SET_ATIME) {
            self.atime = Time::Is(set.atime_ns);
        }
        if given(SET_MTIME) {
            self.mtime = Time::Is(set.mtime_ns);
        }
        if given(SET_ANY) {
            self.ctime = if given(SET_CTIME) { Time::Is(set.ctime_ns) } else { Time::Since(now) };
        }
    }

    /// Checks `attr`, Fxfs's attributes of this node, `now` being now.
    fn holds(&self, attr: &Attr, now: u64) -> Result<(), String> {
        let ino = attr.ino;
        if (attr.mode, attr.uid, attr.gid) != (self.mode, self.uid, self.gid) {
            return Err(format!(
                "{ino} is mode {:o}, owner {}, group {}, not {:o}, {}, {}",
                attr.mode, attr.uid, attr.gid, self.mode, self.uid, self.gid
            ));
        }
        if attr.rdev != self.rdev {
            return Err(format!("{ino} is device {}, not {}", attr.rdev, self.rdev));
        }
        let times = [
            ("atime", self.atime, attr.atime_ns),
            ("mtime", self.mtime, attr.mtime_ns),
            ("ctime", self.ctime, attr.ctime_ns),
        ];
        for (name, time, got) in times {
            if !time.holds(got, now) {
                return Err(format!("{ino}'s {name} is {got}, not {time:?}, now {now}"));
            }
        }
        Ok(())
    }
}

/// A file's model: its size, and the pages written, which holes aren't.
#[derive(Clone, Debug, Default)]
struct Data {
    size: u64,
    pages: BTreeMap<u64, Arc<Vec<u8>>>,
}

impl Data {
    fn write(&mut self, at: u64, bytes: &[u8]) {
        let mut done = 0;
        while done < bytes.len() {
            let here = at + done as u64;
            let within = (here % PAGE) as usize;
            let len = (PAGE as usize - within).min(bytes.len() - done);
            let page = self.pages.entry(here / PAGE).or_default();
            let page = Arc::make_mut(page);
            page.resize(PAGE as usize, 0);
            page[within..within + len].copy_from_slice(&bytes[done..done + len]);
            done += len;
        }
        self.size = self.size.max(at + bytes.len() as u64);
    }

    fn truncate(&mut self, size: u64) {
        if size < self.size {
            self.pages.retain(|page, _| page * PAGE < size);
            if let Some(last) = self.pages.get_mut(&(size / PAGE)) {
                Arc::make_mut(last)[(size % PAGE) as usize..].fill(0);
            }
        }
        self.size = size;
    }

    /// What an allocation of `range` makes of this (fallocate): the file reaches its end, and
    /// nothing is set aside, which an extent store's copy-on-write can't, as the guest's doesn't.
    fn allocate(&mut self, range: Range<u64>) {
        self.size = self.size.max(range.end);
    }

    /// Makes `run` what a plan says, the file reaching its end.
    fn put(&mut self, run: &Run) {
        match &run.bytes {
            Some(bytes) => {
                self.pages.extend(run.pages.clone().map(|page| (page, bytes.clone())));
            }
            None => self.punch(run.pages.clone()),
        }
        self.size = self.size.max(run.pages.end * PAGE);
    }

    /// Makes this what `plan` says.
    fn follow(&mut self, plan: &Plan) {
        plan.runs.iter().for_each(|run| self.put(run));
        self.size = plan.size.unwrap_or(self.size);
    }

    fn punch(&mut self, pages: Range<u64>) {
        let punched: Vec<u64> = self.pages.range(pages).map(|(page, _)| *page).collect();
        punched.iter().for_each(|page| drop(self.pages.remove(page)));
    }

    /// How many extents `pages` holds at most: nothing of its holes counts.
    fn extents_in(&self, pages: Range<u64>) -> u64 {
        self.pages.range(pages).count() as u64
    }

    fn read(&self, at: u64, len: usize) -> Vec<u8> {
        let end = self.size.min(at.saturating_add(len as u64));
        if end <= at {
            return Vec::new();
        }
        let mut read = vec![0; (end - at) as usize];
        for (page, bytes) in self.pages.range(at / PAGE..=(end - 1) / PAGE) {
            let start = page * PAGE;
            let (from, to) = (start.max(at), (start + PAGE).min(end));
            read[(from - at) as usize..(to - at) as usize]
                .copy_from_slice(&bytes[(from - start) as usize..(to - start) as usize]);
        }
        read
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Entry {
    File(u64),
    Symlink(u64),
    Dir(u64),
    /// A FIFO, a socket or a device, which has no data.
    Special(u64),
}

impl Entry {
    fn ino(self) -> u64 {
        match self {
            Entry::File(ino) | Entry::Symlink(ino) | Entry::Dir(ino) | Entry::Special(ino) => ino,
        }
    }
}

/// Pages of a file a plan makes `bytes`, or a hole, which may be as long as a file may.
struct Run {
    pages: Range<u64>,
    bytes: Option<Arc<Vec<u8>>>,
}

/// What an operation of many transactions makes of a file's pages, each run in turn: so one
/// stopped short, by a full filesystem or a power cut, has made its first, and no others.
struct Plan {
    ino: u64,
    runs: Vec<Run>,
    /// The file's size if it's done, if that's not what its pages say: a clone to a source's end
    /// takes its last block whole, and its size.
    size: Option<u64>,
}

impl Plan {
    /// The page after its last.
    fn end(&self) -> u64 {
        self.runs.last().map_or(0, |run| run.pages.end)
    }
}

#[derive(Clone, Debug)]
enum Op {
    Create(usize),
    /// A symlink of the name, to the target.
    Symlink(usize, Target),
    Mkdir(usize),
    /// A file's or symlink's new name, or a tmpfile's first, as `linkat` makes.
    Link(usize, usize),
    /// A name gone now, as a directory's goes.
    Unlink(usize),
    /// A name gone in a batch with others, as a guest's file's goes: when something waits for it.
    /// A directory's goes now, as a guest's do.
    UnlinkLater(usize),
    Rename(usize, usize),
    /// Two names, each linking what the other did.
    Exchange(usize, usize),
    /// A file with no name, open, made for the directory a name is in.
    Tmpfile(usize),
    /// Holds a file or a directory open: it stays until `Close`, whatever happens to its names.
    Open(usize),
    /// Lets go of the `index`th of what's open, as the kernel evicts it if it can.
    Close(usize),
    Write {
        file: usize,
        at: u64,
        len: usize,
        byte: u8,
    },
    /// Whole pages, every `stride`th of `pages`, in as few transactions as hold them, as writeback
    /// writes them.
    Fill {
        file: usize,
        page: u64,
        pages: u64,
        stride: u64,
        byte: u8,
    },
    /// Whole pages, a transaction each, as a compaction runs, which commits racing a super-block
    /// fall on either side of, and if `flushing`, with the volume's store flushed after each, as
    /// an encrypted store flushes itself when it must: as can its end.
    FillWhileCompacting {
        file: usize,
        page: u64,
        pages: u64,
        byte: u8,
        flushing: bool,
    },
    /// Whole pages the page cache dirties, reserving their room first (`reserve_dirty`), which a
    /// full filesystem refuses, as write(2) then fails; written back by `write_dirty`, which their
    /// room pays for, so that a filesystem filled since doesn't refuse it; or, if `dropped`, let
    /// go of unwritten, as a truncate drops them, their room released.
    WriteDirty {
        file: usize,
        page: u64,
        pages: u64,
        byte: u8,
        dropped: bool,
    },
    Read {
        file: usize,
        at: u64,
        len: usize,
    },
    Truncate {
        file: usize,
        size: u64,
    },
    Punch {
        file: usize,
        page: u64,
        pages: u64,
    },
    Clone {
        from: usize,
        to: usize,
        page: u64,
        pages: u64,
        to_page: u64,
    },
    /// A clone to the source's end, wherever in a block that is, as the VFS lets one: the block's
    /// taken whole, with the size where it ends. It may not end before the destination's size.
    CloneToEnd {
        from: usize,
        to: usize,
        page: u64,
        to_page: u64,
    },
    SetXattr {
        file: usize,
        name: u8,
        len: usize,
        byte: u8,
    },
    RemoveXattr {
        file: usize,
        name: u8,
    },
    /// What the attributes of the `dir`th directory are, which it takes through the graveyard.
    SetDirXattr {
        dir: usize,
        name: u8,
        len: usize,
        byte: u8,
    },
    /// An extended attribute of the `node`th node (`Model::node`), as `change` says.
    NodeXattr {
        node: usize,
        name: u8,
        change: XattrChange,
    },
    /// A node as mknod, mkdir or open's O_CREAT makes it, as `make` says.
    Make {
        name: usize,
        make: Make,
    },
    /// What a VFS's setattr changes of the `node`th node (`Model::node`), as `change` says.
    SetAttr {
        node: usize,
        change: Change,
    },
    /// fallocate, with no flags: the blocks of `len` bytes at `at`, the file growing to their end.
    Allocate {
        file: usize,
        at: u64,
        len: u64,
    },
    /// A readdir of the `dir`th directory from its start, `count` names of it, which it goes on
    /// with (`Op::ReadOn`).
    ReadSome {
        dir: usize,
        count: usize,
    },
    /// A readdir going on, the `cursor`th, after the last name it read, for `count` more.
    ReadOn {
        cursor: usize,
        count: usize,
    },
    /// The `dir`th directory made to fold case, or not to, if it has no names, as `chattr +F` may
    /// make an ext4 directory: Fxfs keeps the flag in its record.
    Casefold {
        dir: usize,
        folds: bool,
    },
    /// Names that can't be Fxfs's, which are UTF-8, asked of the `dir`th directory: `BAD_NAMES`'s
    /// `name`th. Nothing changes.
    BadName {
        dir: usize,
        name: usize,
    },
    /// statfs, which counts at least what the model takes.
    Statfs,
    /// A snapshot of the volume, a volume of its own, unless the sequence has taken `SNAPSHOTS`.
    Snapshot,
    Sync,
    Fsync,
    Compact,
    Remount,
    /// A crash just after a sync: what's on the device holds all there was, replayed.
    Crash,
}

/// What `Op::NodeXattr` does: sets a value, makes it only where there's none, replaces only one
/// there is, or removes one there is. Refused otherwise, it changes nothing.
#[derive(Clone, Copy, Debug)]
enum XattrChange {
    Set { len: usize, byte: u8 },
    Create { len: usize, byte: u8 },
    Replace { len: usize, byte: u8 },
    Remove,
}

impl XattrChange {
    /// The value it sets, if it sets one.
    fn value(self) -> Option<Vec<u8>> {
        match self {
            XattrChange::Set { len, byte }
            | XattrChange::Create { len, byte }
            | XattrChange::Replace { len, byte } => Some(vec![byte; len]),
            XattrChange::Remove => None,
        }
    }
}

/// What `Op::Make` makes: a node of the type `kind` names (`KINDS`), with the permissions and
/// attribute flags `permissions`, owned by `uid` and `gid`, and if it's a device, `rdev`.
#[derive(Clone, Copy, Debug)]
struct Make {
    kind: usize,
    permissions: u32,
    uid: u32,
    gid: u32,
    rdev: u32,
}

/// The types of node `Op::Make` makes.
const KINDS: [u32; 6] = [S_IFREG, S_IFDIR, S_IFIFO, S_IFSOCK, S_IFCHR, S_IFBLK];

impl Make {
    /// A file, as `Op::Create` makes it.
    const FILE: Make = Make { kind: 0, permissions: FILE & PERMISSIONS, uid: 0, gid: 0, rdev: 0 };
    /// A directory, as `Op::Mkdir` makes it.
    const DIR: Make = Make { kind: 1, permissions: DIR & PERMISSIONS, uid: 0, gid: 0, rdev: 0 };

    /// Its mode.
    fn mode(&self) -> u32 {
        KINDS[self.kind % KINDS.len()] | self.permissions
    }

    /// Its device number: a device's alone has one.
    fn rdev(&self) -> u64 {
        match self.mode() & S_IFMT {
            S_IFCHR | S_IFBLK => self.rdev.into(),
            _ => 0,
        }
    }

    /// Its entry, as `ino`.
    fn entry(&self, ino: u64) -> Entry {
        match self.mode() & S_IFMT {
            S_IFREG => Entry::File(ino),
            S_IFDIR => Entry::Dir(ino),
            _ => Entry::Special(ino),
        }
    }
}

/// What `Op::Symlink` says: `len` bytes, as `byte` begins them, or the short target the pinned
/// sequences' symlinks say. A VFS asks for none longer than `MAX_LONG_SYMLINK`, PATH_MAX's less
/// one: Fxfs keeps one longer than `MAX_INLINE_SYMLINK` outside its record.
#[derive(Clone, Copy, Debug)]
struct Target {
    len: usize,
    byte: u8,
}

impl Target {
    /// What the pinned sequences' symlinks say: `target`.
    const SHORT: Target = Target { len: 0, byte: 0 };

    fn bytes(&self) -> Vec<u8> {
        if self.len == 0 {
            return b"target".to_vec();
        }
        let letter = |at: usize| b'a' + ((self.byte as usize + at) % 26) as u8;
        (0..self.len).map(|at| if at % 64 == 63 { b'/' } else { letter(at) }).collect()
    }
}

/// What `Op::SetAttr` changes, of what `valid` says (`SET_*`): `permissions` with the node's type.
#[derive(Clone, Copy, Debug)]
struct Change {
    valid: u32,
    permissions: u32,
    uid: u32,
    gid: u32,
    atime: u64,
    mtime: u64,
    ctime: u64,
}

impl Change {
    /// What a VFS asks of Fxfs for it, of a node of type `kind`: no change of a symlink's mode,
    /// which is always the same.
    fn set_attr(&self, kind: u32) -> SetAttr {
        let modes = if kind == S_IFLNK { 0 } else { SET_MODE };
        SetAttr {
            valid: self.valid & (SET_ANY & !SET_MODE | modes),
            mode: kind | self.permissions,
            uid: self.uid,
            gid: self.gid,
            size: 0,
            atime_ns: self.atime,
            mtime_ns: self.mtime,
            ctime_ns: self.ctime,
        }
    }
}

impl Op {
    /// What this makes of a file's pages, if it does so in many transactions and can be done of
    /// what `model` has.
    fn plan(&self, model: &Model) -> Option<Plan> {
        match *self {
            Op::Fill { file, page, pages, stride, byte } => {
                // A page each, as each takes a block of its own (`Model::blocks_at_least`).
                let bytes = || Some(Arc::new(vec![byte; PAGE as usize]));
                let pages = (page..page + pages).step_by(stride as usize);
                let runs =
                    pages.map(|page| Run { pages: page..page + 1, bytes: bytes() }).collect();
                Some(Plan { ino: model.file(file)?, runs, size: None })
            }
            Op::FillWhileCompacting { file, page, pages, byte, .. }
            | Op::WriteDirty { file, page, pages, byte, dropped: false } => {
                Op::Fill { file, page, pages, stride: 1, byte }.plan(model)
            }
            Op::Punch { file, page, pages } => {
                let ino = model.file(file)?;
                let end = (page + pages).min(model.files[&ino].size / PAGE);
                let runs = vec![Run { pages: page..end, bytes: None }];
                (page < end).then_some(Plan { ino, runs, size: None })
            }
            Op::Clone { from, to, page, pages, to_page } => {
                let (src, dst) = (model.file(from)?, model.file(to)?);
                let end = page.saturating_add(pages).min(model.files[&src].size / PAGE);
                let plan = model.plan_clone(src, page..end, dst, to_page)?;
                (plan.end() <= PAGES_AT_MOST).then_some(plan)
            }
            Op::CloneToEnd { from, to, page, to_page } => {
                let (src, dst) = (model.file(from)?, model.file(to)?);
                let end = model.files[&src].size;
                let size = (to_page * PAGE).checked_add(end.checked_sub(page * PAGE)?)?;
                let plan = model.plan_clone(src, page..end.div_ceil(PAGE), dst, to_page)?;
                let fits = size >= model.files[&dst].size && size <= LARGEST;
                fits.then_some(Plan { size: Some(size), ..plan })
            }
            _ => None,
        }
    }
}

/// Offsets that lean on the edges of things.
fn offset() -> impl Strategy<Value = u64> {
    prop_oneof![
        3 => (0u64..32).prop_map(|page| page * PAGE),
        3 => 0u64..(64 << 10),
        1 => (0u64..8192).prop_map(|near| (2 << 20) + near - 4096),
        1 => (0u64..8192).prop_map(|near| (1 << 32) + near - 4096),
        2 => (1u64..16384).prop_map(|back| LARGEST - back),
    ]
}

/// The pages of a fill: a few, or more than a transaction holds.
fn fill() -> impl Strategy<Value = u64> {
    prop_oneof![8u64..64, 512u64..4096]
}

/// Where a clone starts or lands, a page leaning on the edges of things, as `offset`.
fn clone_page() -> impl Strategy<Value = u64> {
    let around = |at: u64| (0u64..16).prop_map(move |near| at / PAGE + near - 8);
    prop_oneof![
        6 => 0u64..32,
        1 => around(2 << 20),
        1 => around(1 << 32),
        1 => around(1 << 50),
        1 => (1u64..16).prop_map(|back| PAGES_AT_MOST - back),
    ]
}

/// The pages of a clone, which stops at its source's end: a few, more than a transaction holds, or
/// across as much as a hole may be: nothing of that but what extents there are may cost anything.
fn clone_pages() -> impl Strategy<Value = u64> {
    prop_oneof![
        6 => 1u64..8,
        3 => 64u64..4096,
        2 => (1u64 << 20)..(1 << 21),
        1 => (1u64 << 38)..(1 << 40),
        1 => Just(PAGES_AT_MOST),
    ]
}

/// A name, mostly the root's, of those there are (`DIRS`), mostly a letter (`SPELLINGS`).
fn name() -> impl Strategy<Value = usize> {
    prop_oneof![3 => 0..NAMES, 1 => 0..NAMES * DIRS, 1 => 0..NAMES * DIRS * ALPHABETS]
}

/// A node, by `Model::node`: the root, as often as any other.
fn node() -> impl Strategy<Value = usize> {
    prop_oneof![1 => Just(0usize), 4 => 1usize..32]
}

/// An owner or a group: root's, a user's, nobody's, or the last one may be.
fn id() -> impl Strategy<Value = u32> {
    prop_oneof![Just(0u32), 1u32..2000, Just(65534), Just(u32::MAX - 1)]
}

/// Permissions, setuid, setgid and sticky too, and fxdax's attribute flags (`PERMISSIONS`).
fn permissions() -> impl Strategy<Value = u32> {
    (0u32..0o10000, 0u32..16).prop_map(|(permissions, flags)| permissions | flags << 16)
}

/// A time a setattr sets, in the past, so that what the clock moves it to comes after: the epoch,
/// a second's edges, and this century's.
fn time() -> impl Strategy<Value = u64> {
    prop_oneof![Just(0u64), 0u64..3_000_000_000, (1u64 << 59)..(1 << 60)]
}

fn make() -> impl Strategy<Value = Make> {
    (0..KINDS.len(), permissions(), id(), id(), any::<u32>())
        .prop_map(|(kind, permissions, uid, gid, rdev)| Make { kind, permissions, uid, gid, rdev })
}

/// What a setattr changes: any of a node's attributes but its size, or none.
fn change() -> impl Strategy<Value = Change> {
    let bits = [SET_MODE, SET_UID, SET_GID, SET_ATIME, SET_MTIME, SET_CTIME];
    let valid = (0u32..1 << bits.len()).prop_map(move |chosen| {
        bits.iter().enumerate().filter(|(i, _)| chosen & 1 << i != 0).map(|(_, bit)| bit).sum()
    });
    (valid, permissions(), id(), id(), time(), time(), time()).prop_map(
        |(valid, permissions, uid, gid, atime, mtime, ctime)| Change {
            valid,
            permissions,
            uid,
            gid,
            atime,
            mtime,
            ctime,
        },
    )
}

/// How much an allocation asks for: a few pages, or more than a small device has.
fn allocation() -> impl Strategy<Value = u64> {
    prop_oneof![3 => 1u64..(64 * PAGE), 1 => (4096 * PAGE)..(8192 * PAGE)]
}

/// The length of an extended attribute's value: none, a few bytes, about what Fxfs keeps in its
/// record, kilobytes, or about the most it takes, or more, which it refuses.
fn xattr_len() -> impl Strategy<Value = usize> {
    let inline = MAX_INLINE_XATTR_SIZE;
    prop_oneof![
        1 => Just(0usize),
        3 => 1usize..64,
        1 => inline - 2..=inline + 2,
        2 => 1024usize..8192,
        1 => MAX_XATTR_VALUE_SIZE - 16..MAX_XATTR_VALUE_SIZE,
        1 => Just(MAX_XATTR_VALUE_SIZE),
        1 => Just(MAX_XATTR_VALUE_SIZE + 1),
    ]
}

/// A symlink's target: short, about as long as its record holds, kilobytes, or the most a VFS asks.
fn target() -> impl Strategy<Value = Target> {
    let len = prop_oneof![
        3 => 1usize..64,
        1 => MAX_INLINE_SYMLINK - 2..=MAX_INLINE_SYMLINK + 2,
        2 => 1024usize..MAX_LONG_SYMLINK,
        1 => MAX_LONG_SYMLINK - 2..=MAX_LONG_SYMLINK,
    ];
    (len, any::<u8>()).prop_map(|(len, byte)| Target { len, byte })
}

fn xattr_change() -> impl Strategy<Value = XattrChange> {
    prop_oneof![
        (xattr_len(), any::<u8>()).prop_map(|(len, byte)| XattrChange::Set { len, byte }),
        (xattr_len(), any::<u8>()).prop_map(|(len, byte)| XattrChange::Create { len, byte }),
        (xattr_len(), any::<u8>()).prop_map(|(len, byte)| XattrChange::Replace { len, byte }),
        Just(XattrChange::Remove),
    ]
}

/// What a short sequence does: few pages at a time, leaning on what ends in a barrier, so that a
/// barrier has little to lose and every subset of it can be tried.
fn short_op() -> impl Strategy<Value = Op> {
    let file = || 0usize..16;
    prop_oneof![
        3 => name().prop_map(Op::Create),
        2 => name().prop_map(Op::Mkdir),
        2 => name().prop_map(Op::Unlink),
        2 => name().prop_map(Op::UnlinkLater),
        2 => (name(), name()).prop_map(|(from, to)| Op::Rename(from, to)),
        2 => (name(), name()).prop_map(|(a, b)| Op::Exchange(a, b)),
        1 => (file(), name()).prop_map(|(file, name)| Op::Link(file, name)),
        2 => name().prop_map(Op::Tmpfile),
        2 => file().prop_map(Op::Open),
        1 => file().prop_map(Op::Close),
        4 => (file(), 0u64..(64 << 10), 1usize..(2 * PAGE as usize), any::<u8>())
            .prop_map(|(file, at, len, byte)| Op::Write { file, at, len, byte }),
        2 => (file(), 0u64..16, 2u64..8, 1u64..=2, any::<u8>()).prop_map(
            |(file, page, pages, stride, byte)| Op::Fill { file, page, pages, stride, byte }
        ),
        6 => (file(), 0u64..16, 2u64..16, any::<u8>(), any::<bool>()).prop_map(
            |(file, page, pages, byte, flushing)| {
                Op::FillWhileCompacting { file, page, pages, byte, flushing }
            }
        ),
        2 => (file(), 0u64..16, 1u64..8, any::<u8>(), prop::bool::weighted(0.25)).prop_map(
            |(file, page, pages, byte, dropped)| Op::WriteDirty { file, page, pages, byte, dropped }
        ),
        1 => (file(), file(), 0u64..8, 1u64..8, 0u64..16).prop_map(
            |(from, to, page, pages, to_page)| Op::Clone { from, to, page, pages, to_page }
        ),
        1 => (file(), 0u64..(64 << 10)).prop_map(|(file, size)| Op::Truncate { file, size }),
        1 => (file(), 0u64..16, 1u64..4)
            .prop_map(|(file, page, pages)| Op::Punch { file, page, pages }),
        1 => (file(), 0u8..3, 1usize..64, any::<u8>())
            .prop_map(|(file, name, len, byte)| Op::SetXattr { file, name, len, byte }),
        1 => (0usize..DIRS, 0u8..3, 1024usize..8192, any::<u8>())
            .prop_map(|(dir, name, len, byte)| Op::SetDirXattr { dir, name, len, byte }),
        1 => (node(), 0u8..XATTR_NAMES, xattr_change())
            .prop_map(|(node, name, change)| Op::NodeXattr { node, name, change }),
        2 => (name(), make()).prop_map(|(name, make)| Op::Make { name, make }),
        2 => (node(), change()).prop_map(|(node, change)| Op::SetAttr { node, change }),
        1 => (file(), 0u64..(64 << 10), 1u64..(16 * PAGE))
            .prop_map(|(file, at, len)| Op::Allocate { file, at, len }),
        1 => (0usize..DIRS, any::<bool>()).prop_map(|(dir, folds)| Op::Casefold { dir, folds }),
        2 => Just(Op::Sync),
        2 => Just(Op::Fsync),
        3 => Just(Op::Compact),
        2 => Just(Op::Remount),
        1 => Just(Op::Snapshot),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let file = || 0usize..16;
    let length = || prop_oneof![1usize..64, Just(PAGE as usize), 1usize..(3 * PAGE as usize)];
    prop_oneof![
        3 => name().prop_map(Op::Create),
        1 => (name(), target()).prop_map(|(name, target)| Op::Symlink(name, target)),
        2 => name().prop_map(Op::Mkdir),
        1 => (file(), name()).prop_map(|(file, name)| Op::Link(file, name)),
        1 => name().prop_map(Op::Unlink),
        2 => name().prop_map(Op::UnlinkLater),
        2 => (name(), name()).prop_map(|(from, to)| Op::Rename(from, to)),
        1 => (name(), name()).prop_map(|(a, b)| Op::Exchange(a, b)),
        1 => name().prop_map(Op::Tmpfile),
        2 => file().prop_map(Op::Open),
        1 => file().prop_map(Op::Close),
        6 => (file(), offset(), length(), any::<u8>())
            .prop_map(|(file, at, len, byte)| Op::Write { file, at, len, byte }),
        3 => (file(), 0u64..64, fill(), 1u64..=2, any::<u8>()).prop_map(
            |(file, page, pages, stride, byte)| Op::Fill { file, page, pages, stride, byte }
        ),
        2 => (file(), 0u64..64, 2u64..32, any::<u8>(), any::<bool>()).prop_map(
            |(file, page, pages, byte, flushing)| {
                Op::FillWhileCompacting { file, page, pages, byte, flushing }
            }
        ),
        2 => (file(), 0u64..64, fill(), any::<u8>(), prop::bool::weighted(0.25)).prop_map(
            |(file, page, pages, byte, dropped)| Op::WriteDirty { file, page, pages, byte, dropped }
        ),
        2 => (file(), offset(), length()).prop_map(|(file, at, len)| Op::Read { file, at, len }),
        3 => (file(), offset()).prop_map(|(file, size)| Op::Truncate { file, size }),
        2 => (file(), 0u64..32, 1u64..8)
            .prop_map(|(file, page, pages)| Op::Punch { file, page, pages }),
        1 => (file(), clone_page(), clone_pages())
            .prop_map(|(file, page, pages)| Op::Punch { file, page, pages }),
        2 => (file(), file(), clone_page(), clone_pages(), clone_page()).prop_map(
            |(from, to, page, pages, to_page)| Op::Clone { from, to, page, pages, to_page }
        ),
        1 => (file(), file(), clone_page(), clone_page())
            .prop_map(|(from, to, page, to_page)| Op::CloneToEnd { from, to, page, to_page }),
        1 => (file(), 0u8..XATTR_NAMES, xattr_len(), any::<u8>())
            .prop_map(|(file, name, len, byte)| Op::SetXattr { file, name, len, byte }),
        1 => (file(), 0u8..XATTR_NAMES).prop_map(|(file, name)| Op::RemoveXattr { file, name }),
        1 => (0usize..DIRS, 0u8..XATTR_NAMES, xattr_len(), any::<u8>())
            .prop_map(|(dir, name, len, byte)| Op::SetDirXattr { dir, name, len, byte }),
        1 => (node(), 0u8..XATTR_NAMES, xattr_change())
            .prop_map(|(node, name, change)| Op::NodeXattr { node, name, change }),
        2 => (name(), make()).prop_map(|(name, make)| Op::Make { name, make }),
        2 => (node(), change()).prop_map(|(node, change)| Op::SetAttr { node, change }),
        1 => (file(), offset(), allocation())
            .prop_map(|(file, at, len)| Op::Allocate { file, at, len }),
        1 => (0usize..DIRS, 0usize..8).prop_map(|(dir, count)| Op::ReadSome { dir, count }),
        2 => (0usize..CURSORS, 0usize..8).prop_map(|(cursor, count)| Op::ReadOn { cursor, count }),
        1 => (0usize..DIRS, any::<bool>()).prop_map(|(dir, folds)| Op::Casefold { dir, folds }),
        1 => (0usize..DIRS, 0..BAD_NAMES.len()).prop_map(|(dir, name)| Op::BadName { dir, name }),
        1 => Just(Op::Statfs),
        1 => Just(Op::Snapshot),
        1 => Just(Op::Sync),
        1 => Just(Op::Fsync),
        1 => Just(Op::Compact),
        1 => Just(Op::Remount),
        1 => Just(Op::Crash),
    ]
}

/// An unlink queued: which a cut may find not done, though the model has gone on from it.
#[derive(Clone, Debug)]
struct Queued {
    place: (u64, usize),
    entry: Entry,
}

/// What a sequence's files should be.
#[derive(Clone, Debug)]
struct Model {
    /// The directories there are, the root first.
    dirs: Vec<u64>,
    /// Of those, the ones that fold case.
    casefold: BTreeSet<u64>,
    /// What each directory's names link, by `SPELLINGS`.
    names: BTreeMap<(u64, usize), Entry>,
    files: BTreeMap<u64, Data>,
    /// The attributes of every node there is, and of some that went.
    nodes: BTreeMap<u64, Node>,
    /// What's held open: what no name links, of these, is in the graveyard.
    open: BTreeSet<u64>,
    /// Of those, the tmpfiles no name has linked yet, which alone a VFS lets a link name with none
    /// left (`I_LINKABLE`).
    linkable: BTreeSet<u64>,
    /// The unlinks queued that may not be done yet.
    queued: Vec<Queued>,
    /// The files those leave nothing of, as a cut before them would find them.
    gone: BTreeMap<u64, Data>,
    /// The snapshots of the volume, in the order they were taken.
    snapshots: Vec<Arc<Snapshot>>,
}

/// A snapshot of the volume (`Op::Snapshot`), a volume of its own: what it holds is what the
/// volume did as it was taken, as a volume opened anew has it, with any of the unlinks queued
/// then done or not, and what was open with no name gone (`Model::cuts`).
#[derive(Debug)]
struct Snapshot {
    name: String,
    held: Vec<Model>,
}

impl Model {
    /// A new volume's, whose root is `root`, made no earlier than `made`: Fxfs gives it no
    /// attributes of its own, and gives it defaults (`Fxfs::attr`).
    fn new(root: u64, made: u64) -> Self {
        Self {
            dirs: vec![root],
            casefold: BTreeSet::new(),
            names: BTreeMap::new(),
            files: BTreeMap::new(),
            nodes: BTreeMap::from([(root, Node::new(DIR, 0, 0, 0, made))]),
            open: BTreeSet::new(),
            linkable: BTreeSet::new(),
            queued: Vec::new(),
            gone: BTreeMap::new(),
            snapshots: Vec::new(),
        }
    }

    /// What a snapshot taken now, named `name`, holds.
    fn snapshot(&self, name: String) -> Snapshot {
        let model = Self { snapshots: Vec::new(), ..self.clone() };
        Snapshot { name, held: model.cuts() }
    }

    /// The ino of the `index`th file there is, if there is any: one open may have no names.
    fn file(&self, index: usize) -> Option<u64> {
        let files: Vec<u64> = self.files.keys().copied().collect();
        (!files.is_empty()).then(|| files[index % files.len()])
    }

    /// What the `index`th of the files, symlinks and special files there are is linked as, if
    /// there are any: a tmpfile, as what it is, by the time it's linked.
    fn linked_as(&self, index: usize) -> Option<Entry> {
        let others = |entry: &&Entry| matches!(entry, Entry::Symlink(_) | Entry::Special(_));
        let others = self.names.values().filter(others);
        let others: BTreeMap<u64, Entry> = others.map(|entry| (entry.ino(), *entry)).collect();
        let mut entries: Vec<Entry> = self.files.keys().map(|ino| Entry::File(*ino)).collect();
        entries.extend(others.values());
        entries.sort_by_key(|entry| entry.ino());
        (!entries.is_empty()).then(|| entries[index % entries.len()])
    }

    /// The ino of the `index`th node there is: the root first, then every other, in order, but
    /// directories gone and held open.
    fn node(&self, index: usize) -> u64 {
        let named = self.names.values().map(|entry| entry.ino());
        let others: BTreeSet<u64> = named.chain(self.files.keys().copied()).collect();
        match index {
            0 => self.dirs[0],
            _ if others.is_empty() => self.dirs[0],
            _ => *others.iter().nth((index - 1) % others.len()).expect("a node"),
        }
    }

    /// The type of `ino`, as Linux has it.
    fn kind(&self, ino: u64) -> u32 {
        self.nodes[&ino].kind()
    }

    /// Which directory `name` is in, and which of its names it is (`SPELLINGS`): its spelling, or
    /// in a directory that folds case, one it has that folds as it does.
    fn place(&self, name: usize) -> (u64, usize) {
        let dir = self.dirs[name / NAMES % DIRS % self.dirs.len()];
        let at = name / (NAMES * DIRS) % ALPHABETS * NAMES + name % NAMES;
        (dir, self.spelled(dir, at))
    }

    /// The name `dir` has that `at` names: `at`, or in a directory that folds case, one it has
    /// that folds as `at` does.
    fn spelled(&self, dir: u64, at: usize) -> usize {
        if !self.casefold.contains(&dir) {
            return at;
        }
        let mut names = self.names_in(dir).map(|(name, _)| name);
        names.find(|name| FOLDED[*name] == FOLDED[at]).unwrap_or(at)
    }

    fn entry(&self, name: usize) -> Option<Entry> {
        self.names.get(&self.place(name)).copied()
    }

    fn links(&self, ino: u64) -> u32 {
        self.names.values().filter(|entry| entry.ino() == ino).count() as u32
    }

    /// What's in the directory `dir`, in order.
    fn names_in(&self, dir: u64) -> impl Iterator<Item = (usize, Entry)> + use<'_> {
        self.names.range((dir, 0)..=(dir, usize::MAX)).map(|((_, name), entry)| (*name, *entry))
    }

    /// Whether `dir` is `other`, or anywhere under it.
    fn is_within(&self, dir: u64, other: u64) -> bool {
        let parent = |dir: u64| {
            let parent = self.names.iter().find(|(_, entry)| entry.ino() == dir);
            parent.map(|((parent, _), _)| *parent)
        };
        std::iter::successors(Some(dir), |dir| parent(*dir)).any(|dir| dir == other)
    }

    /// Whether a VFS asks a filesystem to move `from` to `to`: it does nothing of two names of an
    /// inode, moves no directory under itself, replaces a directory only with a directory and a
    /// file only with a file, and no directory a move comes from. That a directory replaced is
    /// empty only Fxfs knows.
    fn may_rename(&self, from: usize, to: usize) -> bool {
        let Some(entry) = self.entry(from) else { return false };
        let (src, dst) = (self.place(from), self.place(to));
        let same_inode = self.entry(to).is_some_and(|replaced| replaced.ino() == entry.ino());
        let under_itself = matches!(entry, Entry::Dir(ino) if self.is_within(dst.0, ino));
        let replaceable = match self.entry(to) {
            Some(Entry::Dir(ino)) => matches!(entry, Entry::Dir(_)) && !self.is_within(src.0, ino),
            Some(_) => !matches!(entry, Entry::Dir(_)),
            None => true,
        };
        src != dst && !same_inode && !under_itself && replaceable
    }

    /// Whether a VFS asks a filesystem to exchange `a` and `b`: two inodes, neither going under
    /// itself.
    fn may_exchange(&self, a: usize, b: usize) -> bool {
        let (Some(first), Some(second)) = (self.entry(a), self.entry(b)) else { return false };
        let under_itself =
            |entry, dir| matches!(entry, Entry::Dir(ino) if self.is_within(dir, ino));
        first.ino() != second.ino()
            && !under_itself(first, self.place(b).0)
            && !under_itself(second, self.place(a).0)
    }

    /// What a clone of the pages `pages` of `src` to `to_page` of `dst` makes of `dst`, if a VFS
    /// asks for it: not of nothing, nor over itself.
    fn plan_clone(&self, src: u64, pages: Range<u64>, dst: u64, to_page: u64) -> Option<Plan> {
        if pages.is_empty() {
            return None;
        }
        let at = |page: u64| to_page + page - pages.start;
        let to_end = to_page.checked_add(pages.end - pages.start)?;
        if src == dst && pages.start < to_end && to_page < pages.end {
            return None;
        }
        let mut runs = Vec::new();
        let mut hole_from = pages.start;
        let hole = |from: u64, to: u64| Run { pages: at(from)..at(to), bytes: None };
        for (page, bytes) in self.files[&src].pages.range(pages.clone()) {
            if hole_from < *page {
                runs.push(hole(hole_from, *page));
            }
            runs.push(Run { pages: at(*page)..at(*page) + 1, bytes: Some(bytes.clone()) });
            hole_from = page + 1;
        }
        if hole_from < pages.end {
            runs.push(hole(hole_from, pages.end));
        }
        Some(Plan { ino: dst, runs, size: None })
    }

    /// What's open that no name links.
    fn unlinked(&self) -> BTreeSet<u64> {
        self.open.iter().copied().filter(|ino| self.links(*ino) == 0).collect()
    }

    /// The nodes there are but the root: those a name links, and those held open.
    fn held(&self) -> BTreeSet<u64> {
        let inos = self.names.values().map(|entry| entry.ino()).chain(self.open.iter().copied());
        inos.chain(self.files.keys().copied()).collect()
    }

    /// How many objects a volume with this in it has.
    fn objects(&self) -> u64 {
        BARE_OBJECTS + self.held().len() as u64
    }

    /// How many blocks this takes at least: a page of a file each, but those a clone shares, which
    /// share it in the model too, and what extended attributes take (`Node::xattr_blocks`).
    fn blocks_at_least(&self) -> u64 {
        let pages = self.files.values().flat_map(|data| data.pages.values().map(Arc::as_ptr));
        let pages = pages.collect::<BTreeSet<_>>().len() as u64;
        let held = self.held();
        let nodes =
            self.nodes.iter().filter(|(ino, _)| held.contains(ino) || **ino == self.dirs[0]);
        pages + nodes.map(|(_, node)| node.xattr_blocks()).sum::<u64>()
    }

    /// The blocks the node `ino` counts as its own: the pages of a file written, and what its
    /// extended attributes take, of which a clone's are its own too.
    fn blocks_of(&self, ino: u64) -> u64 {
        let pages = self.files.get(&ino).map_or(0, |data| {
            data.pages.keys().filter(|page| *page * PAGE < data.size).count() as u64
        });
        pages + self.nodes[&ino].xattr_blocks()
    }

    /// The size of `ino`, a symlink or a special file: what a symlink says.
    fn size_of(&self, ino: u64) -> u64 {
        match self.kind(ino) {
            S_IFLNK => self.nodes[&ino].target.len() as u64,
            _ => 0,
        }
    }

    /// What a power cut leaves of this: what's open closes, and what no name links goes.
    fn cut(mut self) -> Self {
        let linked: BTreeSet<u64> = self.names.values().map(|entry| entry.ino()).collect();
        self.files.retain(|ino, _| linked.contains(ino));
        let root = self.dirs[0];
        self.nodes.retain(|ino, _| linked.contains(ino) || *ino == root);
        self.casefold.retain(|dir| self.dirs.contains(dir));
        self.open.clear();
        self.linkable.clear();
        self.queued.clear();
        self.gone.clear();
        self
    }

    /// What a power cut may leave of this: of the unlinks queued, any may not be done.
    fn cuts(&self) -> Vec<Self> {
        let undone = |undone: u32| {
            let mut model = self.clone();
            for (i, Queued { place, entry }) in self.queued.iter().enumerate() {
                if undone & 1 << i != 0 {
                    model.names.insert(*place, *entry);
                    if let Some(data) = self.gone.get(&entry.ino()) {
                        model.files.insert(entry.ino(), data.clone());
                    }
                }
            }
            model.cut()
        };
        (0..1 << self.queued.len()).map(undone).collect()
    }
}

/// What a run of sequences got to, which the test says it got to: one that never cut the power, or
/// never filled a device, would pass without having looked.
#[derive(Default)]
struct Tally {
    cuts: Cell<u64>,
    second_cuts: Cell<u64>,
    /// The cuts that left a file or a directory open that no name linked.
    cuts_of_the_unlinked: Cell<u64>,
    /// Extents replays of cuts during replays held that no mount sought (TASK-149.41.1).
    unsought_orphans: Cell<u64>,
    refused: Cell<u64>,
    /// The barriers with anything to lose, of sequences cut at every one.
    barriers: Cell<u64>,
    /// Those with more to lose than every subset of it was tried for.
    beyond_the_bound: Cell<u64>,
    /// The operations of sequences with failures that a device's failure failed.
    failed_ops: Cell<u64>,
    /// The fsyncs and syncs among them, which said so.
    failed_syncs: Cell<u64>,
    /// The device's requests that failed.
    failed_requests: Cell<u64>,
    /// Of those, its flushes.
    failed_flushes: Cell<u64>,
}

/// A device's requests failing, at random, as a seed says, while armed: which a filesystem must
/// answer with a clean error or by going read-only, never a panic, nor taking back what it said
/// was durable. A failure is of the request alone: the next may work.
struct Faults {
    rng: Mutex<StdRng>,
    /// Fails one request in this many, if any.
    one_in: AtomicU32,
    /// Those of this kind only, as `DeviceOp` orders them, if it's less than their number: with
    /// `PostBarrier`, a flush's.
    kind: usize,
    /// The requests failed so far, as `DeviceOp` orders them.
    failed: [AtomicU64; 5],
}

/// What `Faults::kind` is for requests of every kind.
const EVERY_KIND: usize = 5;

impl Faults {
    fn new(seed: u64) -> Arc<Self> {
        let (one_in, failed) = (AtomicU32::new(0), Default::default());
        let mut rng = StdRng::seed_from_u64(seed);
        let kind = rng.random_range(0..=EVERY_KIND);
        Arc::new(Self { rng: Mutex::new(rng), one_in, kind, failed })
    }

    /// As `new`, of a request of any kind.
    fn of_every_kind(seed: u64) -> Arc<Self> {
        let (one_in, failed) = (AtomicU32::new(0), Default::default());
        let rng = Mutex::new(StdRng::seed_from_u64(seed));
        Arc::new(Self { rng, one_in, kind: EVERY_KIND, failed })
    }

    /// Requests fail from now on, one in `one_in`; none, if that's 0.
    fn arm(&self, one_in: u32) {
        self.one_in.store(one_in, Ordering::Relaxed);
    }

    /// `kind`'s failure, if it's to fail.
    fn check(&self, kind: DeviceOp) -> Result<(), Error> {
        let one_in = self.one_in.load(Ordering::Relaxed);
        let kind = kind as usize;
        let chosen = self.kind == EVERY_KIND
            || self.kind == kind
            || self.kind == DeviceOp::Flush as usize && kind == DeviceOp::PostBarrier as usize;
        if one_in == 0 || !chosen || !self.rng.lock().unwrap().random_ratio(1, one_in) {
            return Ok(());
        }
        self.failed[kind].fetch_add(1, Ordering::Relaxed);
        bail!("the device failed a request")
    }

    fn any_failed(&self) -> bool {
        self.failed.iter().any(|failed| failed.load(Ordering::Relaxed) > 0)
    }

    fn requests_failed(&self) -> u64 {
        self.failed.iter().map(|failed| failed.load(Ordering::Relaxed)).sum()
    }

    fn flushes_failed(&self) -> u64 {
        let failed = |kind: DeviceOp| self.failed[kind as usize].load(Ordering::Relaxed);
        failed(DeviceOp::Flush) + failed(DeviceOp::PostBarrier)
    }
}

/// What a sequence runs on: what cuts the power, and what it has of a device.
#[derive(Clone, Copy, Debug)]
struct Setting {
    seed: u64,
    blocks: u64,
    cuts_one_in: u32,
    /// Whether the device is as awkward as a store, as `seed` says, or calm: answering at once and
    /// in order, as a sequence found on a calm one must to stay what was found.
    awkward: bool,
    /// Whether fsck runs after every transaction.
    fsck_each: bool,
    /// Whether every power cut is tried (`Crashes::every`), rather than some at random.
    every_cut: bool,
    /// Whether the device's requests fail, one in as many as `FAILS_ONE_IN` says, once the
    /// filesystem is formatted.
    failing: bool,
    /// Whether tasks apply queued unlinks, as they're timed to, or only what waits for them does,
    /// where a seed says.
    workers: bool,
    /// Whether the graveyard tombstones in batches, as a guest's does, until what waits for it
    /// says, rather than each as it comes.
    batched_graveyard: bool,
    /// Whether the platform is the guest's, whose VFS keeps directories' times (`Seeded`).
    keeps_directory_times: bool,
}

impl Setting {
    fn calm(seed: u64, blocks: u64) -> Self {
        let (awkward, fsck_each, every_cut, failing) = (false, false, false, false);
        let (cuts_one_in, workers, batched_graveyard) = (CUTS_ONE_IN, false, false);
        let keeps_directory_times = false;
        Self {
            seed,
            blocks,
            cuts_one_in,
            awkward,
            fsck_each,
            every_cut,
            failing,
            workers,
            batched_graveyard,
            keeps_directory_times,
        }
    }

    fn awkward(seed: u64, blocks: u64) -> Self {
        Self { awkward: true, ..Self::calm(seed, blocks) }
    }

    fn cutting_one_in(self, cuts_one_in: u32) -> Self {
        Self { cuts_one_in, ..self }
    }

    fn with_fsck_each(self, fsck_each: bool) -> Self {
        Self { fsck_each, ..self }
    }

    fn cutting_everywhere(self) -> Self {
        Self { every_cut: true, ..self }
    }

    fn failing(self) -> Self {
        Self { failing: true, ..self }
    }

    fn with_workers(self, workers: bool) -> Self {
        Self { workers, ..self }
    }

    /// A graveyard as a guest's, for half of a run's sequences, as its seed says.
    fn with_graveyard_of_seed(self) -> Self {
        Self { batched_graveyard: self.seed.is_multiple_of(2), ..self }
    }

    /// The guest's platform, whose VFS keeps directories' times.
    fn on_the_guests_platform(self) -> Self {
        Self { keeps_directory_times: true, ..self }
    }

    /// The guest's platform for a quarter of a run's sequences, as its seed says, apart from
    /// which graveyard they have.
    fn with_platform_of_seed(self) -> Self {
        Self { keeps_directory_times: self.seed / 2 % 4 == 0, ..self }
    }

    /// How often this sequence's device's requests fail, if they do.
    fn fails_one_in(&self) -> u32 {
        match self.failing {
            true => FAILS_ONE_IN[self.seed as usize % FAILS_ONE_IN.len()],
            false => 0,
        }
    }

    /// What cuts the power.
    fn crashes(&self) -> Arc<Crashes> {
        match self.every_cut {
            true => Crashes::every(self.seed, EVERY_SUBSET_OF),
            false => Crashes::new(self.seed, self.cuts_one_in),
        }
    }

    /// What cuts the replay of a power cut, whose seed this is: nothing, if every cut is tried,
    /// which is as many as can be tried.
    fn second_cuts(&self, seed: u64) -> Arc<Crashes> {
        Crashes::new(seed, if self.every_cut { u32::MAX } else { SECOND_CUTS_ONE_IN })
    }

    /// What opens a filesystem here, and what keeps each transaction it commits, for as long as
    /// it's kept, to what a transaction may take of the journal: its metadata reservation sees that
    /// of those that reserve, in a debug build, and nothing that of those that borrow, as a trim
    /// and an unlink do. A transaction past it took a guest's kernel with it: generic/161, 303.
    fn builder(&self) -> (FxFilesystemBuilder, Hooks<'static>) {
        let (mut hooks, handle) = Hooks::new();
        hooks.set_pre_commit(|transaction| {
            let bytes = transaction.journal_bytes();
            assert!(bytes <= TRANSACTION_MAX_JOURNAL_USAGE, "a transaction of {bytes} bytes");
            Ok(())
        });
        let builder = FxFilesystemBuilder::new().hooks(handle);
        let builder = builder.fsck_after_every_transaction(self.fsck_each);
        let builder = match self.batched_graveyard {
            true => builder.graveyard_batch_delay(BATCHES_APART),
            false => builder,
        };
        if !self.every_cut {
            return (builder, hooks);
        }
        let builder = builder.journal_options(JournalOptions {
            reclaim_size: SHORT_RECLAIM,
            chunk_size: SHORT_RECLAIM,
            ..Default::default()
        });
        (builder, hooks)
    }

    /// The seed of what cuts the replay of power cut `cut`, during operation `op`: of those of
    /// sequences found on a calm device, as it was then.
    fn seed_of_cut(&self, cut: usize, op: usize) -> u64 {
        let seed = op as u64 * 31 + cut as u64 + 1;
        if self.awkward { seed.wrapping_mul(self.seed | 1) } else { seed }
    }

    /// The device, recording `crashes`, failing as `faults` says.
    fn device(&self, crashes: &Arc<Crashes>, faults: &Arc<Faults>) -> FakeDevice {
        let device = FakeDevice::new(self.blocks, 4096);
        let device = if self.awkward { device.awkward(self.seed) } else { device.calm() };
        let mut device = device.record_crashes(crashes.clone());
        let faults = faults.clone();
        device.set_op_callback(move |op| faults.check(op));
        device
    }

    /// What `ops` come to here.
    fn run(self, ops: Vec<Op>, tally: Rc<Tally>) -> Result<(), String> {
        let mut executor = fasync::LocalExecutorBuilder::new().build();
        match self.keeps_directory_times {
            true => executor.run_singlethreaded(run::<true>(self, ops, tally)),
            false => executor.run_singlethreaded(run::<false>(self, ops, tally)),
        }
    }

    /// Checks that `ops` hold here.
    /// That `ops` hold, and that no replay left an extent no mount sought (TASK-149.41.1), which
    /// the random runs count rather than fail for: a pinned sequence leaves none, but the one that
    /// shows that task.
    fn holds(self, ops: Vec<Op>) {
        let tally = Rc::new(Tally::default());
        assert_eq!(self.run(ops, tally.clone()), Ok(()), "{self:?}");
        assert_eq!(tally.unsought_orphans.get(), 0, "extents no mount sought: {self:?}");
    }
}

/// As the host does, but for the tasks that apply queued unlinks: a clock wakes them, and a seed
/// wouldn't say what a sequence does. What waits for what's queued applies it, unless `workers`.
/// The guest's, if `KEEPS`: its VFS keeps directories' times, which a create then leaves be.
struct Seeded<const KEEPS: bool> {
    workers: bool,
}

impl<const KEEPS: bool> Platform for Seeded<KEEPS> {
    const KEEPS_DIRECTORY_TIMES: bool = KEEPS;

    type Durable = ();

    fn durable(&self) {}

    fn spawn_urgent(&self, future: impl Future<Output = ()> + Send + 'static) {
        if self.workers {
            fasync::Task::spawn(future).detach();
        }
    }
}

/// What a volume's Fxfs holds, checked against a model: the volume's, or a snapshot's.
struct View<'a, P: Platform>(&'a Fxfs<P>);

impl<P: Platform> View<'_, P> {
    /// Checks the names there are and each file against `model`.
    async fn holds(&self, model: &Model) -> Result<(), String> {
        for dir in &model.dirs {
            self.holds_names(model, *dir).await?;
        }
        for ino in model.files.keys() {
            self.check(model, *ino).await?;
        }
        Ok(())
    }

    /// Checks what the directory `dir` holds against `model`, and what its names link: their
    /// types, each by lookup too, as any spelling of it, and every other spelling found not to be
    /// there, its links, attributes, extended attributes, and what a symlink says.
    async fn holds_names(&self, model: &Model, dir: u64) -> Result<(), String> {
        let folds = Directory::open(&self.0.volume, dir).await;
        let folds = folds.map_err(|e| format!("opening {dir}: {e:#}"))?.dir_type().is_casefold();
        if folds != model.casefold.contains(&dir) {
            return Err(format!("{dir} folds case: {folds}"));
        }
        // A lookup finds no name queued to go, though its record is there until the readdir below
        // applies what is queued in the directory: a lookup applies only what its name's inode
        // waits for (TASK-92.26).
        for at in 0..SPELLINGS.len() {
            let expected = model.names.get(&(dir, model.spelled(dir, at))).map(|entry| entry.ino());
            let found = match self.0.lookup(dir, &name_of(at)).await {
                Ok(attr) => Some(attr.ino),
                Err(error) if errno(&error) == Errno::ENOENT => None,
                Err(error) => return Err(format!("lookup of {at} in {dir}: {error:#}")),
            };
            if found != expected {
                return Err(format!("{at} in {dir} is {found:?}, not {expected:?}"));
            }
        }
        let mut found = Vec::new();
        let listed = self.0.readdir(dir, ReaddirStart::Index(0), |name, ino, kind| {
            found.push((name.as_bytes().to_vec(), ino, kind));
            true
        });
        listed.await.map_err(|e| format!("readdir of {dir}: {e:#}"))?;
        found.sort();
        let mut names: Vec<(Vec<u8>, u64, u32)> = model
            .names_in(dir)
            .map(|(name, entry)| (name_of(name), entry.ino(), model.kind(entry.ino())))
            .collect();
        names.sort();
        if found != names {
            return Err(format!("{dir} holds {found:?}, not {names:?}"));
        }
        self.holds_xattrs(model, dir).await?;
        let mut sub_dirs = 0;
        for (name, entry) in model.names_in(dir) {
            let attr = self.0.lookup(dir, &name_of(name)).await;
            let attr = attr.map_err(|e| format!("lookup of {name} in {dir}: {e:#}"))?;
            if attr.ino != entry.ino() {
                return Err(format!("{name} in {dir} is {}, not {entry:?}", attr.ino));
            }
            self.holds_node(model, &attr)?;
            match entry {
                Entry::File(_) => {}
                Entry::Symlink(ino) => {
                    let target = self.0.readlink(ino).await;
                    let target = target.map_err(|e| format!("readlink of {ino}: {e:#}"))?;
                    let said = &model.nodes[&ino].target;
                    if target != *said {
                        let (got, len) = (String::from_utf8_lossy(&target), said.len());
                        return Err(format!(
                            "{ino} says {got:?}, not the {len} bytes it was made to"
                        ));
                    }
                }
                Entry::Dir(_) => sub_dirs += 1,
                Entry::Special(_) => {}
            }
            if let Entry::Symlink(ino) | Entry::Special(ino) = entry {
                if attr.nlink != model.links(ino) {
                    let links = model.links(ino);
                    return Err(format!("{ino} has {} links, not {links}", attr.nlink));
                }
                if attr.size != model.size_of(ino) {
                    return Err(format!("{ino} has {} bytes", attr.size));
                }
                self.holds_xattrs(model, ino).await?;
            }
        }
        let attr = self.0.attr(dir).await.map_err(|e| format!("attr of {dir}: {e:#}"))?;
        if attr.nlink != 2 + sub_dirs {
            return Err(format!("{dir} has {} links with {sub_dirs} subdirectories", attr.nlink));
        }
        self.holds_node(model, &attr)
    }

    /// Checks `attr`, the attributes Fxfs has of a node, against `model`'s, and the blocks it says
    /// the node takes.
    fn holds_node(&self, model: &Model, attr: &Attr) -> Result<(), String> {
        let node = model.nodes.get(&attr.ino).ok_or(format!("{} has no model", attr.ino))?;
        node.holds(attr, clock())?;
        let blocks = model.blocks_of(attr.ino) * (PAGE / 512);
        if attr.blocks != blocks {
            return Err(format!(
                "{} takes {} blocks of 512 bytes, not {blocks}",
                attr.ino, attr.blocks
            ));
        }
        Ok(())
    }

    /// Checks the extended attributes of the node `ino` against `model`'s: their names, and what
    /// each holds.
    async fn holds_xattrs(&self, model: &Model, ino: u64) -> Result<(), String> {
        let xattrs = &model.nodes.get(&ino).ok_or(format!("{ino} has no model"))?.xattrs;
        let mut names = self.0.list_xattrs(ino).await.map_err(|e| format!("xattrs: {e:#}"))?;
        names.sort();
        let mut expected: Vec<Vec<u8>> = xattrs.keys().map(|name| xattr_of(*name)).collect();
        expected.sort();
        if names != expected {
            return Err(format!("{ino} has the xattrs {names:?}, not {expected:?}"));
        }
        for (name, value) in xattrs {
            let got = self.0.get_xattr(ino, &xattr_of(*name)).await;
            let got = got.map_err(|e| format!("xattr {name} of {ino}: {e:#}"))?;
            if got != *value {
                return Err(format!("xattr {name} of {ino} is {} bytes, not those set", got.len()));
            }
        }
        Ok(())
    }

    /// Checks file `ino` against `model`: its size, its links, its extended attributes, and what
    /// it reads back: all of it, as far as WHOLLY_READ, then around its pages and at its end.
    async fn check(&self, model: &Model, ino: u64) -> Result<(), String> {
        let data = &model.files[&ino];
        // As a lookup does, for the links the kernel takes from it.
        self.0.drain(&[ino]).await.map_err(|e| format!("draining: {e:#}"))?;
        let attr = self.0.attr(ino).await.map_err(|e| format!("attr of {ino}: {e:#}"))?;
        if attr.size != data.size {
            return Err(format!("{ino} is {} bytes, not {}", attr.size, data.size));
        }
        // In the graveyard, its record counts one, which the kernel doesn't.
        let links = model.links(ino).max(1);
        if attr.nlink != links {
            return Err(format!("{ino} has {} links, not {links}", attr.nlink));
        }
        self.holds_node(model, &attr)?;
        self.holds_xattrs(model, ino).await?;
        let mut places: Vec<u64> = data.pages.keys().map(|page| page * PAGE).collect();
        places.push(data.size.saturating_sub(2 * PAGE));
        places.sort_unstable();
        // Holes too: a file's start, whole, for what should be a hole may hold what it did.
        let mut runs = vec![0..data.size.min(WHOLLY_READ)];
        for at in places {
            let end = at + 3 * PAGE;
            match runs.last_mut() {
                Some(run) if at <= run.end => run.end = run.end.max(end),
                _ => runs.push(at..end),
            }
        }
        for run in runs {
            self.compare(data, ino, run.start, (run.end - run.start) as usize).await?;
        }
        Ok(())
    }

    async fn compare(&self, data: &Data, ino: u64, at: u64, len: usize) -> Result<(), String> {
        let expected = data.read(at, len);
        let mut read = vec![0xa5; len];
        let got = self.0.read(ino, at, &mut read).await;
        let got = got.map_err(|e| format!("read of {len} at {at} of {ino}: {e:#}"))?;
        if read[..got] != expected[..] {
            let first = (0..got.min(expected.len())).find(|i| read[*i] != expected[*i]);
            return Err(format!(
                "read of {len} at {at} of {ino}: {got} bytes, expected {}, differing at {first:?}",
                expected.len()
            ));
        }
        Ok(())
    }
}

/// A filesystem and its model.
struct World<const KEEPS: bool> {
    setting: Setting,
    filesystem: fxfs::filesystem::OpenFxFilesystem,
    /// What checks its transactions (`Setting::builder`).
    _hooks: Hooks<'static>,
    // Boxed: the tasks applying queued unlinks refer to it as a world moves.
    fs: Box<Fxfs<Seeded<KEEPS>>>,
    root: u64,
    model: Model,
    tally: Rc<Tally>,
    /// Whether a power cut kept some of what its device was written since a barrier, and lost
    /// some: the journal's records of what a shared extent took, while what was written to it is
    /// there, which Fxfs can't count until the device says how far each extent reaches (TASK-213).
    cut: bool,
    /// The readdirs going on (`Op::ReadOn`), until the filesystem remounts.
    cursors: Vec<Cursor>,
}

/// A readdir of a directory going on, as the kernel's goes on after the name it left off at
/// (`ReaddirStart::After`): each name it lists is there as it lists it, and listed once, and once
/// it ends, it has listed every name that was there all along.
#[derive(Debug)]
struct Cursor {
    dir: u64,
    /// The last name it listed, if any.
    after: Option<Vec<u8>>,
    listed: BTreeSet<Vec<u8>>,
    /// The names there since it began.
    stayed: BTreeSet<Vec<u8>>,
}

/// Runs fsck, and says what it found.
pub(super) async fn checked(filesystem: &fxfs::filesystem::OpenFxFilesystem) -> Result<(), String> {
    let issues = Arc::new(std::sync::Mutex::new(Vec::new()));
    let found = issues.clone();
    let on_error = Box::new(move |issue: &fxfs::fsck::errors::FsckIssue| {
        found.lock().unwrap().push(format!("{issue:?}"));
    });
    let options = FsckOptions { on_error, fail_on_warning: true, ..FsckOptions::default() };
    let done = fsck_with_options((**filesystem).clone(), &options).await;
    done.map(drop).map_err(|e| format!("fsck: {e:#}: {:?}", issues.lock().unwrap()))
}

/// The names of a directory, by `Model::place`'s `at`: letters, then names in another case, or
/// that fold as another does (ß as ss, and a character Unicode ignores), and as long as a name may
/// be, of one byte a character or many.
static SPELLINGS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut names: Vec<String> = (b'a'..b'a' + NAMES as u8).map(|c| char::from(c).into()).collect();
    names.extend([
        "A".into(),
        "B\u{200c}".into(),
        "Straße".into(),
        "STRASSE".into(),
        "L".repeat(255),
        format!("{}{}end", "É".repeat(64), "🦀".repeat(31)),
    ]);
    assert_eq!(names.len(), NAMES * ALPHABETS);
    names
});

/// Of each of `SPELLINGS`, the first that folds as it does: what a directory that folds case
/// takes for one name.
static FOLDED: LazyLock<Vec<usize>> = LazyLock::new(|| {
    let folds = |a: &str, b: &str| fxfs_unicode::casefold_cmp(a, b) == std::cmp::Ordering::Equal;
    let first = |name: &String| SPELLINGS.iter().position(|other| folds(name, other));
    SPELLINGS.iter().map(|name| first(name).expect("itself")).collect()
});

fn name_of(at: usize) -> Vec<u8> {
    SPELLINGS[at].as_bytes().to_vec()
}

/// The bytes of `run`, a page a fill writes.
fn written(run: &Run) -> &[u8] {
    run.bytes.as_ref().expect("written")
}

/// The name of the extended attribute `name`: a few letters, or a namespace's, or as long as one
/// may be.
fn xattr_of(name: u8) -> Vec<u8> {
    match name {
        0..3 => vec![b'x', b'a' + name],
        11 => [b"user.".as_slice(), &[b'n'; MAX_XATTR_NAME_SIZE - 5]].concat(),
        _ => format!("user.{name}").into_bytes(),
    }
}

impl<const KEEPS: bool> World<KEEPS> {
    /// What its volume holds, to check.
    fn view(&self) -> View<'_, Seeded<KEEPS>> {
        View(&self.fs)
    }

    /// Checks that its volume, and each of its snapshots, hold what `model` says.
    async fn holds(&self, model: &Model) -> Result<(), String> {
        self.view().holds(model).await?;
        self.holds_snapshots(model).await
    }

    /// Checks that the filesystem's volumes are its volume and `model`'s snapshots, each of
    /// which, opened as a volume, holds what it did as it was taken.
    async fn holds_snapshots(&self, model: &Model) -> Result<(), String> {
        let volumes = root_volume((*self.filesystem).clone()).await;
        let volumes = volumes.map_err(|e| format!("the root volume: {e:#}"))?;
        let mut found = Vec::new();
        let layer_set = volumes.volume_directory().store().tree().layer_set();
        let mut merger = layer_set.merger();
        let listed = volumes.volume_directory().iter(&mut merger).await;
        let mut listed = listed.map_err(|e| format!("listing volumes: {e:#}"))?;
        while let Some((name, _, _)) = listed.get() {
            found.push(name.to_string());
            listed.advance().await.map_err(|e| format!("listing volumes: {e:#}"))?;
        }
        found.sort();
        let mut expected: Vec<String> =
            model.snapshots.iter().map(|snapshot| snapshot.name.clone()).collect();
        expected.push(VOLUME.to_string());
        expected.sort();
        if found != expected {
            return Err(format!("the volumes are {found:?}, not {expected:?}"));
        }
        for snapshot in &model.snapshots {
            let volume = volumes.volume(&snapshot.name, StoreOptions::default()).await;
            let volume = volume.map_err(|e| format!("{}: {e:#}", snapshot.name))?;
            let platform = Seeded::<KEEPS> { workers: false };
            let fs = Fxfs::new((*self.filesystem).clone(), volume, true, platform);
            let mut differs = Vec::new();
            for held in &snapshot.held {
                match View(&fs).holds(held).await {
                    Ok(()) => break,
                    Err(error) => differs.push(error),
                }
            }
            if differs.len() == snapshot.held.len() {
                return Err(format!("{} holds none of what it did: {differs:?}", snapshot.name));
            }
        }
        Ok(())
    }

    async fn new(
        setting: Setting,
        crashes: &Arc<Crashes>,
        faults: &Arc<Faults>,
        tally: Rc<Tally>,
    ) -> Self {
        let device = DeviceHolder::new(setting.device(crashes, faults));
        let (builder, _hooks) = setting.builder();
        let filesystem = builder.format(true).open(device).await.expect("formatting");
        let made = clock();
        let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
        let volume =
            volumes.new_volume(VOLUME, NewChildStoreOptions::default()).await.expect("the volume");
        let root = volume.root_directory_object_id();
        let platform = Seeded { workers: setting.workers };
        let fs = Box::new(Fxfs::new((*filesystem).clone(), volume, false, platform));
        let model = Model::new(root, made);
        let (cut, cursors) = (false, Vec::new());
        Self { setting, filesystem, _hooks, fs, root, model, tally, cut, cursors }
    }

    /// Mounts what `device` holds, or replays it: what `model` should say.
    async fn mount(
        setting: Setting,
        device: DeviceHolder,
        root: u64,
        model: Model,
        tally: Rc<Tally>,
    ) -> Result<Self, String> {
        let (builder, _hooks) = setting.builder();
        let filesystem = builder.open(device).await;
        let filesystem = filesystem.map_err(|e| format!("opening: {e:#}"))?;
        let volumes = root_volume((*filesystem).clone()).await.map_err(|e| format!("{e:#}"))?;
        let volume = volumes.volume(VOLUME, StoreOptions::default()).await;
        let volume = volume.map_err(|e| format!("the volume: {e:#}"))?;
        drop(volumes);
        let platform = Seeded { workers: setting.workers };
        let fs = Box::new(Fxfs::new((*filesystem).clone(), volume, false, platform));
        let (cut, cursors) = (false, Vec::new());
        Ok(Self { setting, filesystem, _hooks, fs, root, model, tally, cut, cursors })
    }

    /// Unmounts, as a guest's `umount` does, leaving the device.
    async fn unmount(self) -> Result<DeviceHolder, String> {
        self.unmount_borrowing().await.map(|(device, _)| device)
    }

    /// Unmounts, leaving the device, and the metadata space the filesystem borrowed as it closed.
    async fn unmount_borrowing(self) -> Result<(DeviceHolder, u64), String> {
        let Self { filesystem, fs, .. } = self;
        fs.stop_delayed().await.map_err(|e| format!("stopping: {e:#}"))?;
        drop(fs);
        filesystem.close().await.map_err(|e| format!("closing: {e:#}"))?;
        let borrowed = filesystem.object_manager().borrowed_metadata_space();
        Ok((filesystem.take_device().await, borrowed))
    }

    /// `done`, unless the filesystem had no room for it, which is `None`: what a full filesystem
    /// may answer of whatever grows anything.
    fn unless_full<T>(&self, done: Result<T, Error>, what: &str) -> Result<Option<T>, String> {
        match done {
            Ok(done) => Ok(Some(done)),
            Err(error) if FxfsError::NoSpace.matches(&error) => {
                self.tally.refused.set(self.tally.refused.get() + 1);
                Ok(None)
            }
            Err(error) => Err(format!("{what}: {error:#}")),
        }
    }

    /// What's left of `entry`, a name of it gone: what no name links goes as the kernel lets it
    /// go, which holds a directory's inode to write back its times, as overlayfs leaves them.
    async fn forget(&mut self, entry: Entry) -> Result<(), String> {
        let ino = entry.ino();
        if let Entry::Dir(_) = entry {
            self.model.dirs.retain(|dir| *dir != ino);
            self.model.casefold.remove(&ino);
            let times = SetAttr { valid: SET_MTIME, mtime_ns: 1, ..SetAttr::default() };
            let now = clock();
            let written = self.fs.setattr(ino, &times).await;
            if self.unless_full(written, "a directory's times")?.is_some() {
                self.model.nodes.get_mut(&ino).expect("a node").set(&times, now);
            }
        }
        if self.model.links(ino) == 0 && !self.model.open.contains(&ino) {
            self.evict_unlinked(ino);
        }
        Ok(())
    }

    /// Lets `ino`, which no name links and nothing holds, go.
    fn evict_unlinked(&mut self, ino: u64) {
        if let Some(data) = self.model.files.remove(&ino)
            && self.model.queued.iter().any(|queued| queued.entry.ino() == ino)
        {
            self.model.gone.insert(ino, data);
        }
        self.fs.evict(ino, true);
    }

    /// Lets go of `ino`: the kernel may evict what nothing holds, and find it again.
    fn close(&mut self, ino: u64) {
        self.model.open.remove(&ino);
        self.model.linkable.remove(&ino);
        match self.model.links(ino) {
            0 => self.evict_unlinked(ino),
            _ => self.fs.evict(ino, false),
        }
    }

    /// Forgets the unlinks queued, once none are.
    fn settle(&mut self) {
        if self.fs.delayed.queue.lock().unwrap().pending.is_empty() {
            self.model.queued.clear();
            self.model.gone.clear();
        }
    }

    /// `data`, with what `plan` got done before it stopped short.
    async fn stopped_short(&self, mut data: Data, plan: &Plan) -> Result<Data, String> {
        let ino = plan.ino;
        let size = self.fs.attr(ino).await.map_err(|e| format!("attr of {ino}: {e:#}"))?.size;
        let mut read = vec![0; PAGE as usize];
        let hole = read.clone();
        for run in &plan.runs {
            let expected = run.bytes.as_ref().map_or(&hole, |bytes| &**bytes);
            // All that a hole changes is what was there: it may be as long as a file may.
            let pages: Vec<u64> = match run.bytes {
                Some(_) => run.pages.clone().collect(),
                None => data.pages.range(run.pages.clone()).map(|(page, _)| *page).collect(),
            };
            for page in pages {
                let at = page * PAGE;
                let got = self.fs.read(ino, at, &mut read).await;
                let got = got.map_err(|e| format!("read at {at} of {ino}: {e:#}"))?;
                if at + PAGE > size || got != read.len() || read != *expected {
                    return Ok(data);
                }
                // A page of zeros reads as a hole does: which it is, whether a block holds it says.
                let mapped = || async { self.mapped(ino, page..page + 1).await };
                if *expected == hole && mapped().await?.is_empty() == run.bytes.is_some() {
                    return Ok(data);
                }
                data.put(&Run { pages: page..page + 1, bytes: run.bytes.clone() });
            }
            let reached = run.pages.end.min(size / PAGE);
            if run.bytes.is_none() {
                data.size = data.size.max(reached * PAGE);
            }
            if reached < run.pages.end {
                return Ok(data);
            }
        }
        Ok(data)
    }

    /// Checks that statfs counts what `model` takes at least, unless a cut left the device holding
    /// what Fxfs can't count (`cut`).
    async fn holds_statfs(&self, model: &Model) -> Result<(), String> {
        if self.cut {
            return Ok(());
        }
        let stat = self.fs.statfs().await.map_err(|e| format!("statfs: {e:#}"))?;
        if (stat.block_size, stat.blocks) != (PAGE, self.setting.blocks) {
            return Err(format!("statfs says {stat:?} of a device of {}", self.setting.blocks));
        }
        let used = stat.blocks.checked_sub(stat.free_blocks).ok_or(format!("{stat:?}"))?;
        let (least, objects) = (model.blocks_at_least(), model.objects());
        if used < least || stat.files < objects {
            return Err(format!(
                "statfs says {stat:?}, of at least {least} blocks, {objects} files"
            ));
        }
        Ok(())
    }

    /// The pages of file `ino` whose blocks Fxfs records as written in place, as its preallocation
    /// made them: none, since nothing of fxdax's preallocates, its writes copy-on-write.
    async fn written_in_place(&self, ino: u64) -> Result<BTreeSet<u64>, String> {
        let failed = |e: Error| format!("the extents of {ino}: {e:#}");
        let layer_set = self.fs.volume.tree().layer_set();
        let mut merger = layer_set.merger();
        let first = AttributeKey::Extent(Extent::search_key_from_offset(0));
        let first = ObjectKey::attribute(ino, AttributeId::DATA, first);
        let mut extents = merger.query(Query::FullRange(&first)).await.map_err(failed)?;
        let mut pages = BTreeSet::new();
        while let Some(ItemRef {
            key:
                ObjectKey {
                    object_id,
                    data: ObjectKeyData::Attribute(attribute, AttributeKey::Extent(extent)),
                },
            value: ObjectValue::Extent(value),
            ..
        }) = extents.get()
            && *object_id == ino
            && *attribute == AttributeId::DATA
        {
            if let ExtentValue::Some {
                mode: ExtentMode::Overwrite | ExtentMode::OverwritePartial(_),
                ..
            } = value
            {
                pages.extend(extent.start / PAGE..extent.end.div_ceil(PAGE));
            }
            extents.advance().await.map_err(failed)?;
        }
        Ok(pages)
    }

    /// Checks the extents of file `ino` against the model, after each operation, not only when
    /// fsck runs: none written in place, nor its data's record saying it has any, which an
    /// allocation made, fsck refusing the flag once copy-on-write writes replaced them all
    /// (TASK-149.39); and what a DAX window would map, which is every page written, and nothing
    /// else.
    async fn holds_extents(&self, ino: u64) -> Result<(), String> {
        let data = &self.model.files[&ino];
        let in_place = self.written_in_place(ino).await?;
        if let Some(page) = in_place.first() {
            return Err(format!(
                "{ino} has {} pages written in place, from {page}",
                in_place.len()
            ));
        }
        let record = self.fs.volume.tree().find(&data_key(ino)).await;
        let record =
            record.map_err(|e| format!("the data of {ino}: {e:#}"))?.map(|item| item.value);
        if let Some(ObjectValue::Attribute { has_overwrite_extents: true, .. }) = record {
            return Err(format!("{ino}'s data says it has extents written in place"));
        }
        let mapped = self.mapped(ino, 0..data.size.div_ceil(PAGE)).await?;
        let written = data.pages.keys().copied().filter(|page| page * PAGE < data.size).collect();
        if mapped != written {
            let (more, fewer) = (mapped.difference(&written), written.difference(&mapped));
            let (more, fewer): (Vec<_>, Vec<_>) = (more.take(8).collect(), fewer.take(8).collect());
            return Err(format!("{ino} has pages {more:?} mapped, and not {fewer:?}"));
        }
        Ok(())
    }

    /// The room an allocation of file `ino` has, as Fxfs counts it: what's free, and what the file
    /// takes already, which it writes anew (`Fxfs::allocate`).
    async fn room(&self, ino: u64) -> Result<u64, String> {
        let usage = self.fs.usage();
        let attr = self.fs.attr(ino).await.map_err(|e| format!("attr of {ino}: {e:#}"))?;
        Ok((usage.free_blocks * usage.block_size).saturating_add(attr.blocks * 512))
    }

    /// The pages of file `ino` in `pages` that a DAX window would map: those a block holds.
    async fn mapped(&self, ino: u64, pages: Range<u64>) -> Result<BTreeSet<u64>, String> {
        if pages.is_empty() {
            return Ok(BTreeSet::new());
        }
        let extents = self.fs.dax_extents(ino, pages.start * PAGE..pages.end * PAGE).await;
        let extents = extents.map_err(|e| format!("the DAX extents of {ino}: {e:#}"))?;
        Ok(extents
            .iter()
            .flat_map(|e| e.offset / PAGE..(e.offset + e.len).div_ceil(PAGE))
            .collect())
    }

    /// Reads `count` more names of the readdir `cursor`, and checks them: each is there, as the
    /// model has it, and listed once. Whether it got to the end, which then has listed every name
    /// that stayed.
    async fn read_on(&self, cursor: &mut Cursor, count: usize) -> Result<bool, String> {
        let (dir, mut got, mut more) = (cursor.dir, Vec::new(), false);
        let start = match &cursor.after {
            Some(name) => ReaddirStart::After(name),
            None => ReaddirStart::Index(0),
        };
        let read = self.fs.readdir(dir, start, |name, ino, kind| {
            more = got.len() == count;
            if !more {
                got.push((name.as_bytes().to_vec(), ino, kind));
            }
            !more
        });
        read.await.map_err(|e| format!("readdir of {dir} after {:?}: {e:#}", cursor.after))?;
        for (name, ino, kind) in got {
            let mut names = self.model.names_in(dir);
            let entry = names.find(|(at, _)| name_of(*at) == name).map(|(_, entry)| entry.ino());
            if entry != Some(ino) || kind != self.model.kind(ino) {
                return Err(format!("{dir} lists {name:?} as {ino}, of kind {kind:o}: {entry:?}"));
            }
            if !cursor.listed.insert(name.clone()) {
                return Err(format!("{dir} lists {name:?} again"));
            }
            cursor.after = Some(name);
        }
        if !more && let Some(missed) = cursor.stayed.difference(&cursor.listed).next() {
            return Err(format!("{dir}, listed to its end, never listed {missed:?}"));
        }
        Ok(!more)
    }

    /// Checks each file.
    async fn check_files(&self) -> Result<(), String> {
        for ino in self.model.files.keys() {
            self.view().check(&self.model, *ino).await?;
        }
        Ok(())
    }

    /// Checks everything, then fsck.
    async fn check_all(&self) -> Result<(), String> {
        self.holds(&self.model).await?;
        for ino in self.model.files.keys() {
            self.holds_extents(*ino).await?;
        }
        self.holds_statfs(&self.model).await?;
        self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
        self.holds_nothing_else(&self.model).await?;
        if !self.cut {
            self.counts_what_its_device_holds().await?;
        }
        checked(&self.filesystem).await
    }

    /// Checks that Fxfs counts each extent the device holds as far as the device holds it, those
    /// waiting to be freed too: what an allocation that dropped wrote of a shared extent, which a
    /// mount counted only as far as the allocations that committed in it, held more than Fxfs
    /// counted (TASK-213).
    async fn counts_what_its_device_holds(&self) -> Result<(), String> {
        let (device, allocator) = (self.filesystem.device(), self.filesystem.allocator());
        allocator.freed().await.map_err(|e| format!("freeing: {e:#}"))?;
        let longer: Vec<_> = (device.extents().into_iter())
            .filter(|extent| extent.branch == device.branch())
            .filter_map(|extent| {
                let held = device.extent_len(extent)?;
                let counted = allocator.extent_counted(extent);
                (held > counted).then_some((extent, held, counted))
            })
            .collect();
        match longer.len() {
            0 => Ok(()),
            count => Err(format!("{count} extents held further than counted: {longer:?}")),
        }
    }

    /// Checks that the volume holds no object but `model`'s, and only what's open and no name
    /// links in its graveyard, a sync having let go of what it can.
    async fn holds_nothing_else(&self, model: &Model) -> Result<(), String> {
        let volume = &self.fs.volume;
        let objects = volume.object_count();
        if objects != model.objects() {
            return Err(format!("{objects} objects, not {}: {model:?}", model.objects()));
        }
        let layer_set = volume.tree().layer_set();
        let mut merger = layer_set.merger();
        let graveyard = volume.graveyard_directory_object_id();
        let mut entries = Graveyard::iter(graveyard, &mut merger)
            .await
            .map_err(|e| format!("the graveyard: {e:#}"))?;
        let mut found = BTreeSet::new();
        while let Some(entry) = entries.get() {
            // What a trim short of room left, of a file that stays, is no object's end.
            if !matches!(entry.value(), ObjectValue::Trim) {
                found.insert(entry.object_id());
            }
            entries.advance().await.map_err(|e| format!("the graveyard: {e:#}"))?;
        }
        if found != model.unlinked() {
            return Err(format!("the graveyard has {found:?}, not {:?}", model.unlinked()));
        }
        Ok(())
    }

    /// Checks that nothing lies past the end of any file, which growing it would show: what a
    /// truncate cut off, had the power gone before the file's last page was zeroed. Grows them.
    async fn grow_to_pages(&mut self) -> Result<(), String> {
        for ino in self.model.files.keys().copied().collect::<Vec<_>>() {
            let size = self.model.files[&ino].size;
            if size % PAGE == 0 || size > LARGEST - PAGE {
                continue;
            }
            let size = size.next_multiple_of(PAGE);
            let set = SetAttr { valid: SET_SIZE, size, ..SetAttr::default() };
            let grown = self.fs.setattr(ino, &set).await;
            if self.unless_full(grown, "growing")?.is_some() {
                self.model.files.get_mut(&ino).expect("a file").truncate(size);
            }
            self.model.nodes.get_mut(&ino).expect("a node").written();
            self.view().check(&self.model, ino).await?;
        }
        Ok(())
    }

    /// Syncs until nothing waits to be freed: the graveyard's trims, of what a truncate a cut
    /// stopped short left, among it.
    async fn lets_go(&self) -> Result<(), String> {
        // A sync starts what it lets die being freed, and what dies meanwhile waits for the next.
        let allocator = self.filesystem.allocator();
        loop {
            self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
            allocator.freed().await.map_err(|e| format!("freeing: {e:#}"))?;
            if !allocator.has_dead_extents() {
                return Ok(());
            }
        }
    }

    /// Checks that the device holds no extent Fxfs doesn't know, a sync having freed what's dead,
    /// as what a mount reaped of a compaction cut short is: what a crash orphaned, freed by a
    /// mount, takes room Fxfs doesn't count otherwise.
    async fn holds_no_orphans(&self) -> Result<(), String> {
        self.lets_go().await?;
        let allocator = self.filesystem.allocator();
        let device = self.filesystem.device();
        let known = self.filesystem.allocator().extents();
        let (branch, held) = (device.branch(), device.extents());
        let held = held.iter().filter(|extent| extent.branch == branch);
        let orphans: Vec<_> = held.filter(|extent| !known.contains_key(extent)).collect();
        match orphans.len() {
            0 => Ok(()),
            count => Err(format!(
                "{count} extents unknown, {:?} of {} used, {} allocated: {orphans:?}",
                allocator.get_used_bytes(),
                allocator.get_disk_bytes(),
                allocator.get_allocated_bytes(),
            )),
        }
    }

    /// `holds_no_orphans`, but for extents outside every window of sequence numbers the mount sought
    /// orphans among, whose number it returns: those that transactions a replay discarded name, of a
    /// writer's range between two others, that a cut past the replay's recorded tail left before
    /// their frees (TASK-149.41.1). One in a window, which the mount sought, fails.
    async fn holds_no_orphans_but_unsought(&self) -> Result<u64, String> {
        let Err(error) = self.holds_no_orphans().await else { return Ok(0) };
        let device = self.filesystem.device();
        let known = self.filesystem.allocator().extents();
        let sought = self.filesystem.allocator().orphans_sought();
        let (branch, held) = (device.branch(), device.extents());
        let orphans: Vec<_> =
            held.iter().filter(|e| e.branch == branch && !known.contains_key(e)).collect();
        let in_sought =
            |e: &&storage_device::ExtentId| sought.iter().any(|w| w.contains(&e.sequence));
        match orphans.iter().any(in_sought) {
            true => Err(format!("{error}, sought {sought:?}")),
            false => Ok(orphans.len() as u64),
        }
    }

    /// Unmounts and mounts again, as a guest's `umount` and `mount` do: nothing stays open, or
    /// it wouldn't.
    async fn remount(mut self) -> Result<Self, String> {
        self.model.open.clone().into_iter().for_each(|ino| self.close(ino));
        let (setting, root, cut) = (self.setting, self.root, self.cut);
        let (model, tally) = (self.model.clone(), self.tally.clone());
        let (device, borrowed) = self.unmount_borrowing().await?;
        device.reopen(false);
        let mut world = Self::mount(setting, device, root, model, tally).await?;
        world.cut = cut;
        // Replay finds the metadata space borrowed as the filesystem had it: a mount that found
        // more borrowed it again, and a store whose many mounts each did ran out of room to
        // compact (TASK-218).
        let replayed = world.filesystem.object_manager().borrowed_as_replayed();
        if replayed != borrowed {
            return Err(format!("{replayed} bytes borrowed as replayed, {borrowed} as unmounted"));
        }
        world.settle();
        Ok(world)
    }

    /// Checks what a crash leaves now, this filesystem going on: a copy of the device, as of a
    /// sync, replayed, holds what the model does, and passes fsck.
    async fn crash(&self) -> Result<(), String> {
        self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
        let device = self.filesystem.device().snapshot().map_err(|e| format!("snapshot: {e:#}"))?;
        let (model, tally) = (self.model.clone().cut(), self.tally.clone());
        let mut replayed = Self::mount(self.setting, device, self.root, model, tally).await?;
        replayed.cut = self.cut;
        replayed.check_all().await?;
        replayed.unmount().await.map(drop)
    }

    /// Does `op`, if it can be done of what there is, and checks what it changed.
    async fn step(self, op: &Op) -> Result<Self, String> {
        let (world, done) = self.attempt(op).await;
        done.map(|()| world.expect("a world"))
    }

    /// `step`, leaving what's left of the filesystem if it fails, which a remount may not.
    async fn attempt(mut self, op: &Op) -> (Option<Self>, Result<(), String>) {
        if !matches!(op, Op::Remount) {
            // Before `op`, not after: what applied the unlinks may be `op`'s check, after what
            // `op` changed, which a cut may keep without them (a truncate, then a read back).
            self.settle();
            let (names, dirs) = (self.model.names.clone(), self.model.dirs.len());
            let done = self.act(op).await;
            self.keep_cursors();
            let done = done.and_then(|()| self.keeps_the_order_of(op, &names));
            let done = match done {
                Ok(()) => self.moved_whole(op, &names, dirs).await,
                failed => failed,
            };
            return (Some(self), done);
        }
        match self.remount().await {
            Ok(world) => {
                let checked = world.check_files().await;
                (Some(world), checked)
            }
            Err(error) => (None, Err(error)),
        }
    }

    /// Keeps the readdirs going on of the directories there are, which have kept the names they
    /// had all along.
    fn keep_cursors(&mut self) {
        let model = &self.model;
        self.cursors.retain_mut(|cursor| {
            let names: BTreeSet<Vec<u8>> =
                model.names_in(cursor.dir).map(|(at, _)| name_of(at)).collect();
            cursor.stayed.retain(|name| names.contains(name));
            model.dirs.contains(&cursor.dir)
        });
    }

    /// Checks that what `op` did, which found `names`, came after every unlink queued, if it had
    /// to: the journal has names change in the order the syscalls did, and what's durable holds
    /// them. A cut that a name's change had overtaken an unlink would find what the model takes
    /// for an unlink not done yet: only the rule says that it can't be.
    fn keeps_the_order_of(
        &self,
        op: &Op,
        names: &BTreeMap<(u64, usize), Entry>,
    ) -> Result<(), String> {
        let renamed = !matches!(op, Op::UnlinkLater(_)) && *names != self.model.names;
        let durable = matches!(op, Op::Sync | Op::Fsync | Op::Crash);
        match (renamed || durable) && !self.fs.delayed.queue.lock().unwrap().pending.is_empty() {
            true => Err(format!("{op:?} went ahead of the unlinks queued")),
            false => Ok(()),
        }
    }

    /// Checks the directories what `op` did changed, which found `names` in `dirs` directories, if
    /// it moved names or made or removed a directory: their counts of subdirectories change in the
    /// same transaction, which nothing else shows until a cut, or the end, if then.
    async fn moved_whole(
        &self,
        op: &Op,
        names: &BTreeMap<(u64, usize), Entry>,
        dirs: usize,
    ) -> Result<(), String> {
        let moved = matches!(op, Op::Rename(..) | Op::Exchange(..)) && *names != self.model.names;
        if moved || dirs != self.model.dirs.len() {
            for dir in &self.model.dirs {
                self.view().holds_names(&self.model, *dir).await?;
            }
        }
        Ok(())
    }

    /// Removes `name` now, a directory if it's empty.
    async fn unlink(&mut self, name: usize) -> Result<(), String> {
        let entry = self.model.entry(name).expect("looked up");
        let (dir, at) = self.model.place(name);
        let now = clock();
        let gone = self.fs.unlink(dir, &name_of(at), entry.ino()).await;
        if self.is_refused_as_not_empty(Some(entry), &gone)? {
            return Ok(());
        }
        gone.map_err(|e| format!("unlink: {e:#}"))?;
        self.model.names.remove(&(dir, at));
        self.renamed(dir, &[entry], now);
        self.forget(entry).await
    }

    /// What a change of the names of `dir`, no earlier than `now`, makes of the times of the
    /// directory, and of `changed`, the nodes it linked or unlinked: a directory's status alone
    /// changes with the name it's linked as.
    fn renamed(&mut self, dir: u64, changed: &[Entry], now: u64) {
        self.model.nodes.get_mut(&dir).expect("a directory").renamed(now);
        for entry in changed {
            if !matches!(entry, Entry::Dir(_)) || self.model.links(entry.ino()) > 0 {
                self.model.nodes.get_mut(&entry.ino()).expect("a node").changed(now);
            }
        }
    }

    /// Whether `done` was refused for replacing or removing `entry`, a directory with names in
    /// it, as it must be, unless the filesystem had no room to find out.
    fn is_refused_as_not_empty<T>(
        &self,
        entry: Option<Entry>,
        done: &Result<T, Error>,
    ) -> Result<bool, String> {
        let Some(Entry::Dir(ino)) = entry else { return Ok(false) };
        if self.model.names_in(ino).next().is_none() {
            return Ok(false);
        }
        match done {
            Err(error) if FxfsError::NotEmpty.matches(error) => Ok(true),
            Err(error) if FxfsError::NoSpace.matches(error) => Ok(true),
            Err(error) => Err(format!("a directory with names in it: {error:#}")),
            Ok(_) => Err(format!("{ino} went with names in it")),
        }
    }

    /// Does `op`, any but a remount, and checks what it changed.
    async fn act(&mut self, op: &Op) -> Result<(), String> {
        let now = clock();
        match *op {
            Op::Create(name) if self.model.entry(name).is_none() => {
                self.make(name, &Make::FILE, now).await?;
            }
            Op::Symlink(name, target) if self.model.entry(name).is_none() => {
                let (dir, at) = self.model.place(name);
                let made = self.fs.symlink(dir, &name_of(at), &target.bytes(), 0, 0).await;
                let Some([attr, _]) = self.unless_full(made, "symlink")? else { return Ok(()) };
                if attr.ctime_ns < attr.mtime_ns {
                    return Err(format!("a new symlink's ctime is before its mtime: {attr:?}"));
                }
                self.model.names.insert((dir, at), Entry::Symlink(attr.ino));
                self.model.nodes.insert(attr.ino, Node::symlink(&target, now));
                self.renamed(dir, &[], now);
                self.view().holds_node(&self.model, &attr)?;
            }
            Op::Mkdir(name) if self.model.entry(name).is_none() => {
                self.make(name, &Make::DIR, now).await?;
            }
            Op::Make { name, make } if self.model.entry(name).is_none() => {
                self.make(name, &make, now).await?;
            }
            Op::Link(file, name) if self.model.entry(name).is_none() => {
                let Some(entry) = self.model.linked_as(file) else { return Ok(()) };
                let ino = entry.ino();
                let unnamed = self.model.links(ino) == 0;
                if unnamed && !self.model.linkable.contains(&ino) {
                    return Ok(());
                }
                let (dir, at) = self.model.place(name);
                let linked = self.fs.link(ino, dir, &name_of(at), unnamed).await;
                let Some([attr, _]) = self.unless_full(linked, "link")? else { return Ok(()) };
                self.model.linkable.remove(&ino);
                self.model.names.insert((dir, at), entry);
                self.renamed(dir, &[entry], now);
                if attr.nlink != self.model.links(ino) {
                    let links = self.model.links(ino);
                    return Err(format!("{ino} has {} links, not {links}", attr.nlink));
                }
                self.view().holds_node(&self.model, &attr)?;
            }
            Op::Unlink(name) if self.model.entry(name).is_some() => self.unlink(name).await?,
            Op::UnlinkLater(name) if self.model.entry(name).is_some() => {
                let entry = self.model.entry(name).expect("looked up");
                if let Entry::Dir(_) = entry {
                    return self.unlink(name).await;
                }
                if self.model.queued.len() == QUEUED_AT_MOST {
                    self.fs.drain_all().await.map_err(|e| format!("draining: {e:#}"))?;
                    self.settle();
                }
                let (dir, at) = self.model.place(name);
                let queued = self.fs.unlink_later(dir, &name_of(at), entry.ino()).await;
                queued.map_err(|e| format!("unlink_later: {e:#}"))?;
                self.model.names.remove(&(dir, at));
                // Applied later, it leaves the times to the VFS, as the guest's keeps them.
                self.model.queued.push(Queued { place: (dir, at), entry });
                self.forget(entry).await?;
            }
            Op::Rename(from, to) if self.model.may_rename(from, to) => {
                let entry = self.model.entry(from).expect("looked up");
                let replaced = self.model.entry(to);
                let (src, dst) = (self.model.place(from), self.model.place(to));
                let (name, new_name) = (name_of(src.1), name_of(dst.1));
                let new_ino = replaced.map(Entry::ino);
                let renamed =
                    self.fs.rename(src.0, &name, entry.ino(), dst.0, &new_name, new_ino).await;
                if self.is_refused_as_not_empty(replaced, &renamed)?
                    || self.unless_full(renamed, "rename")?.is_none()
                {
                    return Ok(());
                }
                self.model.names.remove(&src);
                self.model.names.insert(dst, entry);
                self.renamed(src.0, &[entry], now);
                self.renamed(dst.0, replaced.as_slice(), now);
                if let Some(replaced) = replaced {
                    self.forget(replaced).await?;
                }
            }
            Op::Exchange(a, b) if self.model.may_exchange(a, b) => {
                let (first, second) = (self.model.place(a), self.model.place(b));
                let (name, new_name) = (name_of(first.1), name_of(second.1));
                let exchanged = self.fs.exchange(first.0, &name, second.0, &new_name).await;
                if self.unless_full(exchanged, "exchange")?.is_none() {
                    return Ok(());
                }
                let (a, b) = (self.model.entry(a).expect("looked up"), self.model.entry(b));
                let b = b.expect("looked up");
                self.model.names.insert(first, b);
                self.model.names.insert(second, a);
                self.renamed(first.0, &[a, b], now);
                self.renamed(second.0, &[], now);
            }
            Op::Tmpfile(name) => {
                let dir = self.model.place(name).0;
                let made = self.fs.tmpfile(dir, FILE, 0, 0).await;
                let Some(attr) = self.unless_full(made, "tmpfile")? else { return Ok(()) };
                self.model.files.insert(attr.ino, Data::default());
                self.model.nodes.insert(attr.ino, Node::new(FILE, 0, 0, 0, now));
                self.model.open.insert(attr.ino);
                self.model.linkable.insert(attr.ino);
                self.view().holds_node(&self.model, &attr)?;
            }
            Op::Open(index) => {
                let subdirs = self.model.dirs[1..].iter();
                let held: Vec<u64> = self.model.files.keys().chain(subdirs).copied().collect();
                if !held.is_empty() {
                    self.model.open.insert(held[index % held.len()]);
                }
            }
            Op::Close(index) => {
                let open: Vec<u64> = self.model.open.iter().copied().collect();
                if !open.is_empty() {
                    self.close(open[index % open.len()]);
                }
            }
            Op::Write { file, at, len, byte } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                let at = at.min(LARGEST - len as u64);
                let bytes = vec![byte; len];
                let written = self.fs.write(ino, at, &bytes).await;
                if self.unless_full(written, "write")?.is_some() {
                    self.model.files.get_mut(&ino).expect("a file").write(at, &bytes);
                }
                self.model.nodes.get_mut(&ino).expect("a node").written();
                self.view().check(&self.model, ino).await?;
                self.holds_extents(ino).await?;
            }
            Op::Fill { .. } | Op::Clone { .. } | Op::CloneToEnd { .. } => {
                let Some(plan) = op.plan(&self.model) else { return Ok(()) };
                let journaled = self.filesystem.journal().file_offset();
                let done = match *op {
                    Op::Clone { from, page, to_page, .. }
                    | Op::CloneToEnd { from, page, to_page, .. } => {
                        let src = self.model.file(from).expect("planned");
                        let end = plan.end();
                        let size = self.model.files[&plan.ino].size.max(end * PAGE);
                        let size = plan.size.unwrap_or(size);
                        let range = page * PAGE..(page + end - to_page) * PAGE;
                        let dst_offset = to_page * PAGE;
                        let cloned = self.fs.clone_range(src, range, plan.ino, dst_offset, size);
                        let cloned = cloned.await.map(drop);
                        let replaced = self.model.files[&plan.ino].extents_in(to_page..end);
                        self.journaled_for(plan.runs.len() as u64 + replaced, journaled)?;
                        cloned
                    }
                    _ => {
                        let mut writes: Vec<(u64, &[u8])> = (plan.runs.iter())
                            .map(|run| (run.pages.start * PAGE, written(run)))
                            .collect();
                        self.fs.write_many(plan.ino, &mut writes).await
                    }
                };
                self.planned(&plan, done).await?;
            }
            Op::FillWhileCompacting { flushing, .. } => {
                let Some(plan) = op.plan(&self.model) else { return Ok(()) };
                let writes = async {
                    for run in &plan.runs {
                        self.fs.write(plan.ino, run.pages.start * PAGE, written(run)).await?;
                        if flushing {
                            let reason = FlushReason::Journal(ForceMajor::False);
                            self.fs.volume.flush_with_reason(reason).await?;
                        }
                    }
                    Ok(())
                };
                let journal = self.filesystem.journal();
                let (compacted, done) = futures::join!(journal.force_compact(), writes);
                compacted.map_err(|e| format!("compacting: {e:#}"))?;
                self.planned(&plan, done).await?;
            }
            Op::WriteDirty { file, pages, dropped, .. } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                let dirty = pages * PAGE;
                let reserved = self.fs.reserve_dirty(ino, dirty);
                if self.unless_full(reserved, "reserve_dirty")?.is_none() {
                    return Ok(());
                }
                if dropped {
                    self.fs.release_dirty(ino, dirty);
                } else {
                    let plan = op.plan(&self.model).expect("a file's pages");
                    let mut writes: Vec<(u64, &[u8])> = (plan.runs.iter())
                        .map(|run| (run.pages.start * PAGE, written(run)))
                        .collect();
                    let done = self.fs.write_dirty(ino, &mut writes, dirty).await;
                    // Paid for with the room it reserved: a full filesystem doesn't refuse it.
                    if let Err(error) = &done {
                        return Err(format!("write_dirty of {dirty} bytes reserved: {error:#}"));
                    }
                    self.planned(&plan, done).await?;
                }
                let kept = self.fs.dirty.dirty_bytes(ino);
                if kept != 0 {
                    return Err(format!("{ino} keeps {kept} bytes of dirty room, none dirty"));
                }
            }
            Op::Read { file, at, len } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                self.view().compare(&self.model.files[&ino], ino, at, len).await?;
            }
            Op::Truncate { file, size } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                let set = SetAttr { valid: SET_SIZE, size, ..SetAttr::default() };
                let cut = self.fs.setattr(ino, &set).await;
                // A file can always be cut.
                let cut = if size < self.model.files[&ino].size {
                    cut.map(Some).map_err(|e| format!("truncate: {e:#}"))?
                } else {
                    self.unless_full(cut, "truncate")?
                };
                if cut.is_some() {
                    self.model.files.get_mut(&ino).expect("a file").truncate(size);
                }
                self.model.nodes.get_mut(&ino).expect("a node").written();
                self.view().check(&self.model, ino).await?;
                self.holds_extents(ino).await?;
            }
            Op::Punch { .. } => {
                let Some(plan) = op.plan(&self.model) else { return Ok(()) };
                let pages = plan.runs[0].pages.clone();
                let extents = self.model.files[&plan.ino].extents_in(pages.clone());
                let journaled = self.filesystem.journal().file_offset();
                let punched = self.fs.punch(plan.ino, pages.start * PAGE..pages.end * PAGE).await;
                self.journaled_for(extents, journaled)?;
                self.planned(&plan, punched).await?;
            }
            Op::Allocate { file, at, len } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                let range = at.min(LARGEST - len)..at.min(LARGEST - len) + len;
                let allocated = self.fs.allocate(ino, range.clone()).await;
                // Room only grows meanwhile, as what was freed is, but for a block the growth
                // takes, zeroing what the file's last held past its end anew.
                let room = self.room(ino).await? + PAGE;
                if self.unless_full(allocated, "allocate")?.is_some() {
                    if len > room {
                        return Err(format!("{ino} allocated {len} bytes, past the room, {room}"));
                    }
                    self.model.files.get_mut(&ino).expect("a file").allocate(range);
                }
                self.model.nodes.get_mut(&ino).expect("a node").written();
                self.view().check(&self.model, ino).await?;
                self.holds_extents(ino).await?;
            }
            // Each set made at once, or refused if the filesystem has no room for it, as the
            // model then has it unchanged; a removal, which frees, works on a full one (TASK-199).
            Op::SetXattr { file, name, len, byte } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                self.change_xattr(ino, name, XattrChange::Set { len, byte }).await?;
            }
            Op::RemoveXattr { file, name } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                if self.model.nodes[&ino].xattrs.contains_key(&name) {
                    self.change_xattr(ino, name, XattrChange::Remove).await?;
                }
            }
            Op::SetDirXattr { dir, name, len, byte } => {
                let dir = self.model.dirs[dir % self.model.dirs.len()];
                self.change_xattr(dir, name, XattrChange::Set { len, byte }).await?;
            }
            Op::NodeXattr { node, name, change } => {
                self.change_xattr(self.model.node(node), name, change).await?;
            }
            Op::SetAttr { node, change } => {
                let ino = self.model.node(node);
                let set = change.set_attr(self.model.kind(ino));
                self.fs.setattr(ino, &set).await.map_err(|e| format!("setattr: {e:#}"))?;
                self.model.nodes.get_mut(&ino).expect("a node").set(&set, now);
                let attr = self.fs.attr(ino).await.map_err(|e| format!("attr of {ino}: {e:#}"))?;
                self.view().holds_node(&self.model, &attr)?;
            }
            Op::ReadSome { dir, count } => {
                let dir = self.model.dirs[dir % self.model.dirs.len()];
                let names = self.model.names_in(dir).map(|(at, _)| name_of(at)).collect();
                let (after, listed) = (None, BTreeSet::new());
                let mut cursor = Cursor { dir, after, listed, stayed: names };
                if !self.read_on(&mut cursor, count).await? {
                    if self.cursors.len() == CURSORS {
                        self.cursors.remove(0);
                    }
                    self.cursors.push(cursor);
                }
            }
            Op::ReadOn { cursor, count } if !self.cursors.is_empty() => {
                let at = cursor % self.cursors.len();
                let mut cursor = self.cursors.remove(at);
                if !self.read_on(&mut cursor, count).await? {
                    self.cursors.insert(at, cursor);
                }
            }
            Op::Casefold { dir, folds } => {
                let dir = self.model.dirs[dir % self.model.dirs.len()];
                if self.model.names_in(dir).next().is_some() {
                    return Ok(());
                }
                // Its names queued to go, gone.
                self.fs.drain_all().await.map_err(|e| format!("draining: {e:#}"))?;
                let directory = self.fs.directory(dir).await.map_err(|e| format!("{e:#}"))?;
                let set = directory.set_casefold(folds).await;
                if self.unless_full(set, "casefold")?.is_some() {
                    match folds {
                        true => drop(self.model.casefold.insert(dir)),
                        false => drop(self.model.casefold.remove(&dir)),
                    }
                }
                self.view().holds_names(&self.model, dir).await?;
            }
            Op::BadName { dir, name } => {
                let dir = self.model.dirs[dir % self.model.dirs.len()];
                self.refuses_bad_name(dir, BAD_NAMES[name % BAD_NAMES.len()]).await?;
            }
            Op::Snapshot if self.model.snapshots.len() < SNAPSHOTS => {
                let name = format!("snapshot-{}", self.model.snapshots.len());
                let volumes = root_volume((*self.filesystem).clone()).await;
                let volumes = volumes.map_err(|e| format!("the root volume: {e:#}"))?;
                let made = volumes.snapshot_volume(VOLUME, &name).await.map(drop);
                if self.unless_full(made, "snapshot")?.is_none() {
                    return Ok(());
                }
                let snapshot = self.model.snapshot(name);
                self.model.snapshots.push(Arc::new(snapshot));
                self.holds_snapshots(&self.model).await?;
            }
            Op::Statfs => self.holds_statfs(&self.model).await?,
            Op::Sync => self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?,
            Op::Fsync => self.fs.fsync().await.map_err(|e| format!("fsync: {e:#}"))?,
            Op::Compact => {
                let journal = self.filesystem.journal();
                journal.force_compact().await.map_err(|e| format!("compacting: {e:#}"))?;
            }
            Op::Crash => self.crash().await.map_err(|e| format!("after a crash: {e}"))?,
            _ => {}
        }
        Ok(())
    }

    /// Makes `name` as `make` says, no earlier than `now`, and checks what it made, and what of
    /// its directory changed.
    async fn make(&mut self, name: usize, make: &Make, now: u64) -> Result<(), String> {
        let (dir, at) = self.model.place(name);
        let (mode, rdev) = (make.mode(), make.rdev());
        let made = self.fs.create(dir, &name_of(at), mode, rdev, make.uid, make.gid, &[]).await;
        let Some([attr, _]) = self.unless_full(made, "create")? else { return Ok(()) };
        if attr.ctime_ns < attr.mtime_ns {
            return Err(format!("a new node's ctime is before its mtime: {attr:?}"));
        }
        let entry = make.entry(attr.ino);
        self.model.names.insert((dir, at), entry);
        self.model.nodes.insert(attr.ino, Node::new(mode, make.uid, make.gid, rdev, now));
        match entry {
            Entry::File(ino) => drop(self.model.files.insert(ino, Data::default())),
            Entry::Dir(ino) => {
                self.model.dirs.push(ino);
                // A directory folds case as the one it's made in does.
                if self.model.casefold.contains(&dir) {
                    self.model.casefold.insert(ino);
                }
            }
            Entry::Symlink(_) | Entry::Special(_) => {}
        }
        // A directory's times are its VFS's on the guest's platform, which a create of a node but
        // a directory leaves to it (`create_child_node`'s `times`): Fxfs times only those of the
        // directories it makes in it.
        if !KEEPS || matches!(entry, Entry::Dir(_)) {
            self.renamed(dir, &[], now);
        }
        self.view().holds_node(&self.model, &attr)
    }

    /// Sets or removes the extended attribute `name` of `ino` as `change` says, and checks what it
    /// found: a value longer than Fxfs takes, or one a change needs there, or not, refused,
    /// changing nothing; and as anything that grows, a set refused on a full filesystem.
    async fn change_xattr(
        &mut self,
        ino: u64,
        name: u8,
        change: XattrChange,
    ) -> Result<(), String> {
        let key = xattr_of(name);
        let held = self.model.nodes[&ino].xattrs.contains_key(&name);
        let value = change.value();
        let done = match (change, &value) {
            (XattrChange::Remove, _) => self.fs.remove_xattr(ino, &key).await,
            (_, Some(value)) => {
                let mode = match change {
                    XattrChange::Create { .. } => SetExtendedAttributeMode::Create,
                    XattrChange::Replace { .. } => SetExtendedAttributeMode::Replace,
                    _ => SetExtendedAttributeMode::Set,
                };
                self.fs.set_xattr(ino, &key, value, mode).await
            }
            (_, None) => unreachable!("a value to set"),
        };
        let refusal = match (change, held) {
            _ if value.as_ref().is_some_and(|value| value.len() > MAX_XATTR_VALUE_SIZE) => {
                Some(FxfsError::TooBig)
            }
            (XattrChange::Create { .. }, true) => Some(FxfsError::AlreadyExists),
            (XattrChange::Replace { .. } | XattrChange::Remove, false) => Some(FxfsError::NotFound),
            _ => None,
        };
        let done = match (refusal, done) {
            (Some(refusal), Err(error)) if refusal.matches(&error) => None,
            // A full filesystem refuses the transaction before the change, which changes nothing
            // either way.
            (Some(_), Err(error)) if FxfsError::NoSpace.matches(&error) => None,
            (Some(refusal), done) => {
                return Err(format!("{change:?} of {name} of {ino}, not {refusal:?}: {done:?}"));
            }
            (None, done) if value.is_none() => Some(done.map_err(|e| format!("remove: {e:#}"))?),
            (None, done) => self.unless_full(done, "set_xattr")?,
        };
        if done.is_some() {
            self.model.nodes.get_mut(&ino).expect("a node").set_xattr(name, value);
        }
        self.view().holds_xattrs(&self.model, ino).await
    }

    /// Checks that `name`, which Fxfs's names can't be, is no name in `dir`, and can't be made
    /// one: nothing of it changes.
    async fn refuses_bad_name(&self, dir: u64, name: &[u8]) -> Result<(), String> {
        let invalid = |what: &str, done: Result<(), Error>| match done {
            Err(error) if errno(&error) == Errno::EINVAL => Ok(()),
            done => Err(format!("{what} of {name:?} in {dir}: {done:?}")),
        };
        match self.fs.lookup(dir, name).await {
            Err(error) if errno(&error) == Errno::ENOENT => {}
            found => return Err(format!("lookup of {name:?} in {dir}: {found:?}")),
        }
        invalid("create", self.fs.create(dir, name, FILE, 0, 0, 0, &[]).await.map(drop))?;
        invalid("mkdir", self.fs.create(dir, name, DIR, 0, 0, 0, &[]).await.map(drop))?;
        invalid(
            "symlink",
            self.fs.symlink(dir, name, &Target::SHORT.bytes(), 0, 0).await.map(drop),
        )?;
        if let Some((at, entry)) = self.model.names_in(dir).next() {
            if !matches!(entry, Entry::Dir(_)) {
                let linked = self.fs.link(entry.ino(), dir, name, false).await;
                invalid("link", linked.map(drop))?;
            }
            let ino = entry.ino();
            let renamed = self.fs.rename(dir, &name_of(at), ino, dir, name, None).await;
            invalid("rename", renamed.map(drop))?;
        }
        self.view().holds_names(&self.model, dir).await
    }

    /// `model`, had `op`, which failed, worked: this filesystem says what a name it made links.
    async fn had_it_worked(&self, model: &Model, op: &Op) -> Option<Model> {
        let mut model = model.clone();
        // What times it moved, it moved to no earlier.
        let touch = |model: &mut Model, ino: u64| model.nodes.get_mut(&ino).map(Node::written);
        if let Some(plan) = op.plan(&model) {
            model.files.get_mut(&plan.ino)?.follow(&plan);
            touch(&mut model, plan.ino);
            return Some(model);
        }
        match *op {
            Op::Create(name) | Op::Symlink(name, _) | Op::Mkdir(name) | Op::Make { name, .. } => {
                let (dir, at) = model.place(name);
                let ino = self.fs.lookup(dir, &name_of(at)).await.ok()?.ino;
                let make = match *op {
                    Op::Make { make, .. } => make,
                    Op::Mkdir(_) => Make::DIR,
                    _ => Make::FILE,
                };
                let (entry, node) = match op {
                    Op::Symlink(_, target) => (Entry::Symlink(ino), Node::symlink(target, 0)),
                    _ => (
                        make.entry(ino),
                        Node::new(make.mode(), make.uid, make.gid, make.rdev(), 0),
                    ),
                };
                model.names.insert((dir, at), entry);
                model.nodes.insert(ino, node);
                touch(&mut model, dir);
                match entry {
                    Entry::File(_) => drop(model.files.insert(ino, Data::default())),
                    Entry::Dir(_) => {
                        model.dirs.push(ino);
                        if model.casefold.contains(&dir) {
                            model.casefold.insert(ino);
                        }
                    }
                    Entry::Symlink(_) | Entry::Special(_) => {}
                }
            }
            Op::Link(file, name) => {
                let entry = model.linked_as(file)?;
                let place = model.place(name);
                model.names.insert(place, entry);
                touch(&mut model, place.0);
                touch(&mut model, entry.ino());
            }
            Op::Unlink(name) | Op::UnlinkLater(name) => {
                let place = model.place(name);
                let entry = model.names.remove(&place)?;
                model.dirs.retain(|dir| *dir != entry.ino());
                if model.links(entry.ino()) == 0 {
                    model.files.remove(&entry.ino());
                }
                touch(&mut model, place.0);
                touch(&mut model, entry.ino());
            }
            Op::Rename(from, to) => {
                let (src, dst) = (model.place(from), model.place(to));
                let entry = model.names.remove(&src)?;
                touch(&mut model, entry.ino());
                if let Some(replaced) = model.names.insert(dst, entry) {
                    model.dirs.retain(|dir| *dir != replaced.ino());
                    if model.links(replaced.ino()) == 0 {
                        model.files.remove(&replaced.ino());
                    }
                    touch(&mut model, replaced.ino());
                }
                touch(&mut model, src.0);
                touch(&mut model, dst.0);
            }
            Op::Exchange(a, b) => {
                let (first, second) = (model.entry(a)?, model.entry(b)?);
                let (a, b) = (model.place(a), model.place(b));
                model.names.insert(a, second);
                model.names.insert(b, first);
                for ino in [a.0, b.0, first.ino(), second.ino()] {
                    touch(&mut model, ino);
                }
            }
            Op::Write { file, at, len, byte } => {
                let ino = model.file(file)?;
                model.files.get_mut(&ino)?.write(at.min(LARGEST - len as u64), &vec![byte; len]);
                touch(&mut model, ino);
            }
            Op::Truncate { file, size } => {
                let ino = model.file(file)?;
                model.files.get_mut(&ino)?.truncate(size);
                touch(&mut model, ino);
            }
            Op::Allocate { file, at, len } => {
                let ino = model.file(file)?;
                let at = at.min(LARGEST - len);
                model.files.get_mut(&ino)?.allocate(at..at + len);
                touch(&mut model, ino);
            }
            Op::SetXattr { file, name, len, byte } => {
                let ino = model.file(file)?;
                model.nodes.get_mut(&ino)?.set_xattr(name, Some(vec![byte; len]));
            }
            Op::RemoveXattr { file, name } => {
                let ino = model.file(file)?;
                model.nodes.get_mut(&ino)?.set_xattr(name, None);
            }
            Op::SetDirXattr { dir, name, len, byte } => {
                let dir = model.dirs[dir % model.dirs.len()];
                model.nodes.get_mut(&dir)?.set_xattr(name, Some(vec![byte; len]));
            }
            Op::NodeXattr { node, name, change } => {
                let ino = model.node(node);
                model.nodes.get_mut(&ino)?.set_xattr(name, change.value());
            }
            Op::SetAttr { node, change } => {
                let node = model.nodes.get_mut(&model.node(node))?;
                node.set(&change.set_attr(node.kind()), 0);
            }
            Op::Casefold { dir, folds } => {
                let dir = model.dirs[dir % model.dirs.len()];
                match folds {
                    true => drop(model.casefold.insert(dir)),
                    false => drop(model.casefold.remove(&dir)),
                }
            }
            _ => return None,
        }
        Some(model)
    }

    /// Checks that what the journal took since it was at `since` is what `extents` extents cost.
    fn journaled_for(&self, extents: u64, since: u64) -> Result<(), String> {
        let journaled = self.filesystem.journal().file_offset() - since;
        let allowed = JOURNALED_ANYWAY + JOURNALED_PER_EXTENT * extents;
        if journaled > allowed {
            return Err(format!("{journaled} bytes of journal to cross {extents} extents"));
        }
        Ok(())
    }

    /// Takes `plan` for done, as `done` says, or for what got done before it stopped short.
    async fn planned(&mut self, plan: &Plan, done: Result<(), Error>) -> Result<(), String> {
        let mut data = self.model.files[&plan.ino].clone();
        self.model.nodes.get_mut(&plan.ino).expect("a node").written();
        if self.unless_full(done, "a plan")?.is_some() {
            data.follow(plan);
        } else {
            data = self.stopped_short(data, plan).await?;
        }
        self.model.files.insert(plan.ino, data);
        self.view().check(&self.model, plan.ino).await?;
        self.holds_extents(plan.ino).await
    }
}

/// What a sequence's model was as it went, which a power cut's device is checked against.
struct Past {
    setting: Setting,
    /// The model after each operation, the first before any.
    states: Vec<Model>,
    /// For each of those, the latest that was durable by then.
    durable: Vec<usize>,
    /// The operation a device's failure ended the sequence at, if one did. Its state is the one
    /// before it: it may have got as far as anything, which the filesystem says.
    failed: Option<usize>,
}

impl Past {
    /// The model had the operation that led to `state` stopped short, by what `world` holds of it.
    async fn stopped_short<const KEEPS: bool>(
        &self,
        world: &World<KEEPS>,
        state: usize,
        ops: &[Op],
    ) -> Option<Model> {
        let mut before = self.states[state.checked_sub(1)?].clone();
        let plan = ops.get(state - 1)?.plan(&before)?;
        let data = before.files.remove(&plan.ino)?;
        let short = world.stopped_short(data, &plan).await.ok()?;
        before.files.insert(plan.ino, short);
        before.nodes.get_mut(&plan.ino)?.written();
        Some(before)
    }

    /// Checks what power cut `cut` of a sequence, during operation `op` of `ops`, leaves on
    /// `device`: it replays, passes fsck, is the model after one of the operations since the last
    /// that was durable, and goes on with what the sequence did next.
    async fn check_cut<const KEEPS: bool>(
        &self,
        (cut, op): (usize, usize),
        ops: &[Op],
        device: FakeDevice,
        root: u64,
        tally: Rc<Tally>,
    ) -> Result<(), String> {
        tally.cuts.set(tally.cuts.get() + 1);
        let seed = self.setting.seed_of_cut(cut, op);
        let second_cuts = self.setting.second_cuts(seed);
        second_cuts.mark(0);
        let device = DeviceHolder::new(device.record_crashes(second_cuts.clone()));
        let setting = self.setting;
        let mut world =
            World::<KEEPS>::mount(setting, device, root, Model::new(root, 0), tally.clone())
                .await?;
        world.cut = true;
        second_cuts.stop();
        checked(&world.filesystem).await?;
        world.holds_no_orphans().await?;
        let (first, last) = (self.durable[op], (op + 1).min(self.states.len() - 1));
        let mut differs = Vec::new();
        let mut held = None;
        for state in (first..=last).rev() {
            let mut candidates = self.states[state].cuts();
            if let Some(short) = self.stopped_short(&world, state, ops).await {
                candidates.extend(short.cuts());
            }
            if self.failed == Some(state.wrapping_sub(1)) {
                let worked = world.had_it_worked(&self.states[state], &ops[state - 1]).await;
                candidates.extend(worked.map(Model::cut));
            }
            for candidate in candidates {
                if held.is_none() {
                    match world.holds(&candidate).await {
                        Ok(()) => held = Some(candidate),
                        Err(error) => differs.push(format!("{state}: {error}")),
                    }
                }
            }
        }
        let Some(held) = held else {
            return Err(format!("holds none of states {first} to {last}: {differs:?}"));
        };
        world.holds_nothing_else(&held).await?;
        world.model = held.clone();
        if self.states[first..=last].iter().any(|state| !state.unlinked().is_empty()) {
            tally.cuts_of_the_unlinked.set(tally.cuts_of_the_unlinked.get() + 1);
        }
        for (_, device) in second_cuts.take() {
            tally.second_cuts.set(tally.second_cuts.get() + 1);
            let device = DeviceHolder::new(device);
            let again = World::<KEEPS>::mount(setting, device, root, held.clone(), tally.clone());
            let again = again.await;
            let mut again = again.map_err(|e| format!("cut during its replay: {e}"))?;
            again.cut = true;
            // As the replay it cut was: what a truncate cut short left to trim, the graveyard
            // trims after the mount, the file counting its blocks until then; and it holds no
            // extent Fxfs doesn't know, the orphans the replay it cut found among them, which it
            // had minted past (TASK-149.41), but those of TASK-149.41.1.
            let unsought = again.holds_no_orphans_but_unsought().await;
            let unsought = unsought.map_err(|e| format!("cut during its replay: {e}"))?;
            tally.unsought_orphans.set(tally.unsought_orphans.get() + unsought);
            again.check_all().await.map_err(|e| format!("cut during its replay: {e}"))?;
            again.unmount().await?;
        }
        world.grow_to_pages().await.map_err(|e| format!("past a file's end: {e}"))?;
        for next in ops[op..].iter().take(GOES_ON_FOR) {
            world = world.step(next).await.map_err(|e| format!("going on with {next:?}: {e}"))?;
        }
        world.check_all().await.map_err(|e| format!("gone on: {e}"))?;
        world.unmount().await.map(drop)
    }
}

async fn run<const KEEPS: bool>(
    setting: Setting,
    ops: Vec<Op>,
    tally: Rc<Tally>,
) -> Result<(), String> {
    let crashes = setting.crashes();
    let faults = Faults::new(setting.seed);
    let world = World::<KEEPS>::new(setting, &crashes, &faults, tally.clone()).await;
    let (root, device) = (world.root, world.filesystem.device());
    let states = vec![world.model.clone()];
    let mut past = Past { setting, states, durable: vec![0], failed: None };
    world.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
    crashes.mark(0);
    faults.arm(setting.fails_one_in());
    let mut world = Some(world);
    for (i, op) in ops.iter().enumerate() {
        let flushes_failed = faults.flushes_failed();
        let (next, done) = world.take().expect("a world").attempt(op).await;
        world = next;
        if let Err(error) = done {
            if !faults.any_failed() {
                return Err(format!("op {i}, {op:?}: {error}"));
            }
            tally.failed_ops.set(tally.failed_ops.get() + 1);
            if matches!(op, Op::Sync | Op::Fsync) {
                tally.failed_syncs.set(tally.failed_syncs.get() + 1);
            }
            past.states.push(past.states[i].clone());
            past.durable.push(past.durable[i]);
            past.failed = Some(i);
            break;
        }
        past.states.push(world.as_ref().expect("a world that worked").model.clone());
        let durable = matches!(op, Op::Sync | Op::Fsync | Op::Crash | Op::Remount);
        past.durable.push(if durable { i + 1 } else { past.durable[i] });
        crashes.mark(i as u64 + 1);
        // A sync that says it's done with a flush failing beside it had better be: the next, which
        // works, would seal what it lost.
        if durable && faults.flushes_failed() > flushes_failed {
            break;
        }
    }
    faults.arm(0);
    if setting.failing {
        // What the device holds at last, as a crash leaves it, and as it stands.
        crashes.last();
        device.flush().await.map_err(|e| format!("flushing what's left: {e:#}"))?;
    }
    let finished = match (world, past.failed) {
        (Some(world), None) => finish(world).await,
        // What's left of it unmounts or it doesn't: that it doesn't panic is all that's asked.
        (Some(world), Some(_)) => world.unmount().await.map(drop).or(Ok(())),
        (None, _) => Ok(()),
    };
    if !faults.any_failed() {
        finished?;
    }
    tally.failed_requests.set(tally.failed_requests.get() + faults.requests_failed());
    tally.failed_flushes.set(tally.failed_flushes.get() + faults.flushes_failed());
    let counts = crashes.counts();
    tally.barriers.set(tally.barriers.get() + counts.barriers);
    tally.beyond_the_bound.set(tally.beyond_the_bound.get() + counts.beyond_the_bound);
    for (cut, (mark, device)) in crashes.take().enumerate() {
        let checked =
            past.check_cut::<KEEPS>((cut, mark as usize), &ops, device, root, tally.clone());
        let checked = checked.await;
        checked.map_err(|e| format!("cut {cut}, during op {mark}: {e}"))?;
    }
    Ok(())
}

/// Checks what a sequence left, across a remount, and unmounts it.
async fn finish<const KEEPS: bool>(world: World<KEEPS>) -> Result<(), String> {
    world.check_all().await.map_err(|e| format!("at the end: {e}"))?;
    let world = world.remount().await.map_err(|e| format!("at the end: {e}"))?;
    world.check_all().await.map_err(|e| format!("remounted at the end: {e}"))?;
    world.unmount().await.map(drop)
}

/// Aborts, saying the sequence, if one takes longer than `limit`: each is sent here as it starts,
/// and this ends with the sender.
pub(super) fn watch_sequences(limit: Duration) -> mpsc::Sender<String> {
    let (sender, sequences) = mpsc::channel();
    std::thread::spawn(move || {
        let mut sequence = String::new();
        loop {
            match sequences.recv_timeout(limit) {
                Ok(next) => sequence = next,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    eprintln!("this sequence never ended: {sequence}");
                    std::process::abort();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    sender
}

#[test]
fn whatever_a_vfs_asks_reads_back_as_a_model_says() {
    let cases = std::env::var("FXDAX_MODEL_CASES").map_or(96, |cases| cases.parse().unwrap());
    let fsck_each = std::env::var_os("FXDAX_MODEL_FSCK").is_some();
    let workers = std::env::var_os("FXDAX_MODEL_WORKERS").is_some();
    let config = config(cases);
    let most: u32 = std::env::var("FXDAX_MODEL_OPS").map_or(48, |most| most.parse().unwrap());
    let room = prop_oneof![Just(ROOMY), Just(TIGHT)];
    let sequences = (any::<u64>(), room, proptest::collection::vec(op(), 1..most as usize));
    let tally = Rc::new(Tally::default());
    let limit = SEQUENCE_LIMIT * (most / 48).max(1);
    let watched = watch_sequences(if fsck_each { 30 * limit } else { limit });
    TestRunner::new(config)
        .run(&sequences, |(seed, blocks, ops)| {
            watched.send(format!("{seed}, {blocks}, {ops:?}")).expect("the watchdog stopped");
            let setting = Setting::awkward(seed, blocks).with_graveyard_of_seed();
            let setting = setting.with_platform_of_seed();
            let setting = setting.with_fsck_each(fsck_each).with_workers(workers);
            prop_assert_eq!(setting.run(ops, tally.clone()), Ok(()));
            Ok(())
        })
        .unwrap();
    let (cuts, second_cuts) = (tally.cuts.get(), tally.second_cuts.get());
    let unlinked = tally.cuts_of_the_unlinked.get();
    println!(
        "{cuts} power cuts, {second_cuts} of replays, {unlinked} of files open with no name, {} \
         refused, {} extents left unsought (TASK-149.41.1)",
        tally.refused.get(),
        tally.unsought_orphans.get()
    );
    assert!(cuts > 0, "no power cut");
    assert!(second_cuts > 0, "no power cut of a replay");
    assert!(unlinked > 0, "no power cut of a file open with no name");
    assert!(tally.refused.get() > 0, "no device filled");
}

/// The proptest seed of the sequences `a_device_that_fails_loses_nothing_durable` runs unless a
/// hunt asks for others: of its 24, 12 fail an operation, 6 of them syncs.
const FAILING_SEQUENCES: u64 = 2;

/// A panic of any task since the last call, which a sequence can't see of one it doesn't wait for.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Sequences whose device's requests fail, each ended by the first its operation doesn't survive:
/// a filesystem that hit an error remounts, passes fsck, and holds what was durable, or what the
/// operation that failed got as far as: nothing panics, and no sync that said it was done isn't.
///
/// The same sequences every run, of `FAILING_SEQUENCES`, unless a hunt asks for others
/// (FXDAX_MODEL_FAILURES, for more, and PROPTEST_RNG_SEED): a sequence fails as its seed says,
/// requests of one kind, one in 4, 32 or 256, and about two in three fail no operation, most
/// because no request did. 24 sequences drawn afresh each run all did so once in about 10,000
/// runs, and the check that something failed failed with them (CI 42446163807: one request failed,
/// a flush, no operation).
#[test]
fn a_device_that_fails_loses_nothing_durable() {
    let hunt = std::env::var("FXDAX_MODEL_FAILURES").ok();
    let cases = hunt.as_ref().map_or(24, |cases| cases.parse().unwrap());
    let config = match (config(cases), hunt) {
        (config, None) if config.rng_seed == RngSeed::Random => {
            Config { rng_seed: RngSeed::Fixed(FAILING_SEQUENCES), ..config }
        }
        (config, _) => config,
    };
    let room = prop_oneof![Just(ROOMY), Just(TIGHT)];
    // Many syncs, for a flush to fail under.
    let op = || prop_oneof![4 => op(), 1 => Just(Op::Fsync), 1 => Just(Op::Sync)];
    let sequences = (any::<u64>(), room, proptest::collection::vec(op(), 1..48));
    let tally = Rc::new(Tally::default());
    let watched = watch_sequences(SEQUENCE_LIMIT);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        PANICS.lock().unwrap().push(panic.to_string());
        hook(panic);
    }));
    TestRunner::new(config)
        .run(&sequences, |(seed, blocks, ops)| {
            watched.send(format!("{seed}, {blocks}, {ops:?}")).expect("the watchdog stopped");
            let setting = Setting::awkward(seed, blocks).with_graveyard_of_seed().failing();
            let done = setting.run(ops, tally.clone());
            prop_assert_eq!(std::mem::take(&mut *PANICS.lock().unwrap()), Vec::<String>::new());
            prop_assert_eq!(done, Ok(()));
            Ok(())
        })
        .unwrap();
    let (requests, flushes) = (tally.failed_requests.get(), tally.failed_flushes.get());
    let (failed, syncs) = (tally.failed_ops.get(), tally.failed_syncs.get());
    println!(
        "{requests} requests failed, {flushes} of them flushes: {failed} operations failed, \
         {syncs} of them syncs, {} power cuts",
        tally.cuts.get()
    );
    assert!(failed > 0, "no operation failed");
}

/// A device that fails every request fails the fsync that asks it to, and what was synced before
/// stays.
#[test]
fn a_failed_fsync_is_reported_and_keeps_what_was_synced() {
    fasync::LocalExecutorBuilder::new().build().run_singlethreaded(async {
        let setting = Setting::awkward(0, ROOMY);
        let crashes = Crashes::new(0, u32::MAX);
        let faults = Faults::of_every_kind(0);
        let mut world = World::<false>::new(setting, &crashes, &faults, Rc::default()).await;
        let device = world.filesystem.device();
        let write = |byte| Op::Write { file: 0, at: 0, len: 1, byte };
        world.act(&Op::Create(0)).await.unwrap();
        world.act(&write(1)).await.unwrap();
        world.act(&Op::Fsync).await.unwrap();
        let (root, synced) = (world.root, world.model.clone());
        world.act(&write(2)).await.unwrap();
        crashes.mark(1);

        faults.arm(1);
        let fsynced = world.fs.fsync().await;
        faults.arm(0);
        assert!(fsynced.is_err(), "an fsync the device failed said it was done");
        assert!(faults.any_failed());

        crashes.last();
        device.flush().await.expect("flush failed");
        for (_, device) in crashes.take() {
            let tally = Rc::default();
            let device = DeviceHolder::new(device);
            let world = World::<false>::mount(setting, device, root, synced.clone(), tally);
            let world = world.await.unwrap();
            let ino = *synced.files.keys().next().expect("a file");
            let mut read = [0];
            world.fs.read(ino, 0, &mut read).await.expect("read failed");
            assert_eq!(read, [1], "what was synced changed, or what wasn't is half there");
            checked(&world.filesystem).await.unwrap();
            world.unmount().await.unwrap();
        }
    });
}

/// Short sequences, cut at every barrier with every subset of what it may lose (`EVERY_SUBSET_OF`),
/// which is complete up to a bound, as ALICE and CrashMonkey are: FXDAX_MODEL_EVERY_CUT asks for more
/// of them. The test says how far the bound went: the barriers past it were tried with less.
#[test]
fn every_cut_of_a_short_sequence() {
    let cases = std::env::var("FXDAX_MODEL_EVERY_CUT").map_or(6, |cases| cases.parse().unwrap());
    let config = config(cases);
    let sequences = (any::<u64>(), proptest::collection::vec(short_op(), 1..=SHORT));
    let tally = Rc::new(Tally::default());
    let watched = watch_sequences(30 * SEQUENCE_LIMIT);
    TestRunner::new(config)
        .run(&sequences, |(seed, ops)| {
            watched.send(format!("{seed}, {ops:?}")).expect("the watchdog stopped");
            let setting = Setting::awkward(seed, ROOMY).with_graveyard_of_seed();
            prop_assert_eq!(setting.cutting_everywhere().run(ops, tally.clone()), Ok(()));
            Ok(())
        })
        .unwrap();
    let (cuts, barriers) = (tally.cuts.get(), tally.barriers.get());
    let (beyond, unlinked) = (tally.beyond_the_bound.get(), tally.cuts_of_the_unlinked.get());
    println!(
        "{cuts} power cuts at {barriers} barriers, {beyond} past every subset of \
         {EVERY_SUBSET_OF}, {unlinked} of files open with no name"
    );
    assert!(cuts > 0, "no power cut");
}

/// A file unlinked while open reads and writes as it did until it closes, and a cut anywhere around
/// that finds it linked, or nothing of it: no object, no graveyard entry, no extent.
#[test]
fn a_file_open_outlives_its_names() {
    let ops = vec![
        Op::Create(0),
        Op::Write { file: 0, at: 8192, len: 8192, byte: 1 },
        Op::Open(0),
        Op::Sync,
        Op::UnlinkLater(0),
        Op::Write { file: 0, at: 1, len: 4096, byte: 2 },
        Op::Fill { file: 0, page: 2, pages: 8, stride: 1, byte: 3 },
        Op::Sync,
        Op::Truncate { file: 0, size: 4097 },
        Op::Compact,
        Op::Close(0),
        Op::Sync,
    ];
    let tally = Rc::new(Tally::default());
    let setting = Setting::awkward(0, ROOMY).cutting_everywhere();
    assert_eq!(setting.run(ops, tally.clone()), Ok(()));
    assert!(tally.cuts_of_the_unlinked.get() > 0, "no cut with the file open and unlinked");
}

/// A tmpfile a cut finds with no name is gone; one it finds linked is a file, which goes on.
#[test]
fn a_tmpfile_is_a_file_once_linked() {
    let ops = vec![
        Op::Tmpfile(0),
        Op::Write { file: 0, at: 0, len: 8192, byte: 1 },
        Op::Sync,
        Op::Link(0, 7),
        Op::Compact,
        Op::Unlink(7),
        Op::Link(0, 7),
        Op::Sync,
        Op::Close(0),
        Op::Write { file: 0, at: 4096, len: 1, byte: 2 },
    ];
    let tally = Rc::new(Tally::default());
    let setting = Setting::awkward(0, ROOMY).cutting_everywhere();
    assert_eq!(setting.run(ops, tally.clone()), Ok(()));
    assert!(tally.cuts_of_the_unlinked.get() > 0, "no cut with the tmpfile unnamed");
}

/// Names move across directories, which move, exchange and go themselves, a directory with names
/// in it refused: each a transaction of many objects, which a cut leaves whole.
#[test]
fn names_and_directories_move_as_one() {
    let in_a = |name| NAMES + name;
    let ops = vec![
        Op::Mkdir(0),
        Op::Mkdir(1),
        Op::Create(in_a(0)),
        Op::Write { file: 0, at: 0, len: 1, byte: 1 },
        Op::Link(0, 2),
        Op::Sync,
        Op::Rename(in_a(0), 2 * NAMES),
        Op::Unlink(1),
        Op::Exchange(0, 2),
        Op::Rename(0, 3),
        Op::Mkdir(in_a(4)),
        Op::Rename(in_a(4), 5),
        Op::Rename(5, in_a(4)),
        Op::Exchange(3, 1),
        Op::SetDirXattr { dir: 1, name: 0, len: 4096, byte: 1 },
        Op::Compact,
        Op::Rename(1, 4),
        Op::Symlink(5, Target::SHORT),
        Op::Link(1, in_a(5)),
        Op::Unlink(5),
    ];
    Setting::awkward(0, ROOMY).cutting_everywhere().holds(ops);
}

/// Names change, and syncs complete, after the unlinks queued before them, whatever comes between:
/// the journal keeps the order the syscalls had.
#[test]
fn what_comes_after_an_unlink_queued_stays_after_it() {
    let write = Op::Write { file: 0, at: 0, len: 1, byte: 1 };
    let ops = vec![
        Op::Create(0),
        Op::Create(1),
        Op::Create(2),
        Op::Create(3),
        Op::UnlinkLater(0),
        write.clone(),
        Op::Rename(1, 4),
        Op::UnlinkLater(2),
        write.clone(),
        Op::Link(0, 5),
        Op::UnlinkLater(3),
        write,
        Op::Fsync,
        Op::UnlinkLater(4),
        Op::Sync,
    ];
    Setting::awkward(0, ROOMY).cutting_everywhere().holds(ops);
}

/// A clone costs what extents it crosses, whatever lies between them: counted, a hole of exabytes
/// (generic/303) took commits for ever, which outran the journal.
#[test]
fn a_clone_crosses_holes_of_any_size() {
    let far = (1 << 50) / PAGE;
    let ops = vec![
        Op::Create(0),
        Op::Create(1),
        Op::Write { file: 0, at: 0, len: 4096, byte: 1 },
        Op::Write { file: 0, at: far * PAGE, len: 4096, byte: 2 },
        Op::Clone { from: 0, to: 1, page: 0, pages: PAGES_AT_MOST, to_page: 0 },
        Op::Sync,
        Op::Write { file: 0, at: LARGEST - 4097, len: 4096, byte: 3 },
        Op::CloneToEnd { from: 0, to: 1, page: 0, to_page: 0 },
    ];
    let watched = watch_sequences(SEQUENCE_LIMIT);
    watched.send(format!("{ops:?}")).expect("the watchdog stopped");
    Setting::awkward(0, ROOMY).holds(ops);
}

/// A punch costs what extents it frees, as a clone does: a transaction a MiB of its range spun for
/// as long as a file may be, a guest's inode locked.
#[test]
fn a_punch_crosses_holes_of_any_size() {
    let far = (1 << 50) / PAGE;
    let ops = vec![
        Op::Create(0),
        Op::Write { file: 0, at: 0, len: 8192, byte: 1 },
        Op::Write { file: 0, at: far * PAGE, len: 4096, byte: 2 },
        Op::Sync,
        Op::Punch { file: 0, page: 1, pages: PAGES_AT_MOST },
        Op::Sync,
    ];
    let watched = watch_sequences(SEQUENCE_LIMIT);
    watched.send(format!("{ops:?}")).expect("the watchdog stopped");
    Setting::awkward(0, ROOMY).holds(ops);
}

/// What a truncate cuts from a file's last block stays gone, however full the filesystem is as it
/// does: the tail's copy found no room, what the cut freed waiting for a sync, and a write past the
/// end brought those bytes back. Found by a hunt, whose seed this keeps, as how full the device gets
/// depends on it. With no room even after a sync they still come back, which a long hunt finds, and
/// TASK-149.8's notes say why. It no longer fills the device so, whose records changed: the test
/// after it finds this whatever they are.
#[test]
fn a_truncate_on_a_full_filesystem_leaves_nothing_past_the_end() {
    let ops = vec![
        Op::Compact,
        Op::Create(0),
        Op::Create(3),
        Op::Create(5),
        Op::Fill { file: 11, page: 2, pages: 2695, stride: 2, byte: 1 },
        Op::Create(1),
        Op::CloneToEnd { from: 6, to: 13, page: 0, to_page: 0 },
        Op::Symlink(3, Target::SHORT),
        Op::Fill { file: 1, page: 0, pages: 512, stride: 1, byte: 0 },
        Op::Unlink(0),
        Op::Rename(5, 1),
        Op::Read { file: 0, at: 0, len: 3 },
        Op::Truncate { file: 15, size: 41646 },
        Op::Crash,
        Op::Create(5),
        Op::Create(2),
        Op::Write { file: 9, at: 51996, len: 8, byte: 83 },
    ];
    Setting::awkward(0, TIGHT).with_graveyard_of_seed().holds(ops);
}

/// What a truncate cuts from a file's last block stays gone when no block is free even after a
/// sync, its blocks shared with a clone, so that cutting them freed nothing: a clone that grew the
/// file after brought the bytes back. Found by a hunt (TASK-149.8's long4/11, TASK-150.19).
#[test]
fn a_truncate_with_no_block_to_zero_its_tail_in_leaves_nothing_past_the_end() {
    let ops = vec![
        Op::Create(1),
        Op::Exchange(0, 0),
        Op::Read { file: 0, at: 0, len: 1 },
        Op::Open(0),
        Op::Write { file: 0, at: 0, len: 1, byte: 1 },
        Op::Remount,
        Op::Truncate { file: 15, size: 4294967297 },
        Op::Fill { file: 13, page: 58, pages: 2051, stride: 2, byte: 139 },
        Op::Fill { file: 12, page: 5, pages: 2519, stride: 2, byte: 254 },
        Op::Create(14),
        Op::Write { file: 1, at: 52953, len: 4096, byte: 55 },
        Op::Truncate { file: 4, size: 4294971308 },
        Op::Write { file: 2, at: 32768, len: 4096, byte: 7 },
        Op::CloneToEnd { from: 0, to: 9, page: 508, to_page: 17 },
        Op::Truncate { file: 15, size: 54584 },
        Op::UnlinkLater(7),
        Op::Clone { from: 2, to: 14, page: 5, pages: 5, to_page: 515 },
    ];
    Setting::awkward(0, TIGHT).holds(ops);
}

/// A clone of a file to its own end, past a last block it holds part of: zeroing that block's tail
/// in the clone's transaction freed the block the clone took for the file's, an inconsistency the
/// plan's fsck found (the Linux workspace on GCE, TASK-153.4).
#[test]
fn a_clone_to_its_own_end_past_a_partial_block_holds() {
    let ops = vec![
        Op::Create(0),
        Op::Write { file: 0, at: 77824, len: 1, byte: 0 },
        Op::CloneToEnd { from: 0, to: 0, page: 0, to_page: 20 },
    ];
    Setting::awkward(0, ROOMY).with_graveyard_of_seed().holds(ops);
}

/// What a truncate cuts from a file's last block stays gone on a device just found full, however
/// many transactions and records that took: the tail's copy finds no room, which a sync makes of
/// what is waiting to go, here a file's queued unlink. A sequence above found this, on a seed that
/// fills the device just so, which lost it as `write_many` came to split its transactions by their
/// room, and 4,000 cases of a hunt don't find it: this fills the device until a page is refused,
/// whatever that takes. A fill is refused a transaction at a time, which leaves its room free, so
/// pages take what it left.
#[test]
fn a_truncate_on_a_device_just_filled_leaves_nothing_past_the_end() {
    let mut ops = vec![
        Op::Create(0),
        Op::Create(1),
        Op::Create(2),
        Op::Fill { file: 0, page: 0, pages: 12, stride: 1, byte: 1 },
        Op::Fill { file: 2, page: 0, pages: 64, stride: 1, byte: 2 },
        Op::Sync,
        Op::Fill { file: 1, page: 0, pages: 2 * TIGHT, stride: 1, byte: 3 },
    ];
    let pages = 2 * TIGHT..2 * TIGHT + BLOCKS_A_FILL_MAY_LEAVE;
    ops.extend(pages.map(|page| Op::Write { file: 1, at: page * PAGE, len: 4096, byte: 4 }));
    ops.extend([
        Op::UnlinkLater(2),
        Op::Truncate { file: 0, size: 41646 },
        // The room the write past the end wants, which shows what the cut left.
        Op::Sync,
        Op::Write { file: 0, at: 51996, len: 8, byte: 83 },
    ]);
    Setting::calm(0, TIGHT).holds(ops);
}

/// A punch of many commits, cut, has punched what it got to, from its start: found as a false alarm
/// by a hunt, a punch taken for one transaction, whose seed this keeps, as what the cuts lose depends
/// on it.
#[test]
fn a_punch_cut_short_leaves_what_it_got_to() {
    let ops = vec![
        Op::Create(0),
        Op::Sync,
        Op::SetXattr { file: 0, name: 0, len: 4097, byte: 0 },
        Op::Fill { file: 0, page: 0, pages: 1211, stride: 2, byte: 1 },
        Op::SetDirXattr { dir: 0, name: 0, len: 4097, byte: 0 },
        Op::Crash,
        Op::Create(1),
        Op::Sync,
        Op::Punch { file: 2, page: 504, pages: 657 },
    ];
    Setting::awkward(8215753583942078673, ROOMY).with_graveyard_of_seed().holds(ops);
}

/// Commits don't stop for a super-block, whose offsets are of the journal it flushed: read after
/// that flush, they took a store's flush that ended meanwhile for done, past what a crash kept, and
/// what only the journal held was given up, an allocation among it. Every cut of a few operations
/// finds it in seconds; at random, it took 21 and a hunt of minutes. The seed keeps when the device
/// answers.
#[test]
fn every_cut_of_a_store_flushed_around_a_super_block() {
    let ops = vec![
        Op::Write { file: 10, at: 29721, len: 1698, byte: 234 },
        Op::Create(3),
        Op::Compact,
        Op::Fill { file: 5, page: 8, pages: 5, stride: 2, byte: 114 },
        Op::FillWhileCompacting { file: 7, page: 7, pages: 13, byte: 45, flushing: true },
    ];
    Setting::awkward(5713515047801504255, ROOMY).cutting_everywhere().holds(ops);
}

/// A compaction a caller forced ran with the flush task's, each writing super-blocks, of which a
/// crash could find neither readable. Found by cutting everywhere, whose seed this keeps, as when
/// the device answers depends on it.
#[test]
fn a_compaction_waits_for_another() {
    let ops = vec![
        Op::Compact,
        Op::Create(0),
        Op::FillWhileCompacting { file: 0, page: 0, pages: 11, byte: 0, flushing: false },
    ];
    Setting::awkward(1290169520417531043, ROOMY).cutting_everywhere().holds(ops);
}

/// Fsck after every transaction runs, around a sequence's compactions, crashes and remounts, and
/// passes: the hunt that does it, FXDAX_MODEL_FSCK, is too slow to run always.
#[test]
fn fsck_can_follow_every_transaction() {
    let ops = vec![
        Op::Create(0),
        Op::Write { file: 0, at: 0, len: 1, byte: 1 },
        Op::Clone { from: 0, to: 0, page: 0, pages: 1, to_page: 1 },
        Op::Compact,
        Op::Crash,
        Op::Unlink(0),
        Op::Remount,
    ];
    Setting::awkward(0, ROOMY).with_fsck_each(true).holds(ops);
}

/// A power cut around a compaction leaves a filesystem that mounts, whatever the size of its
/// device: the first a filesystem has, its journal still in the file it was formatted with, found
/// the metadata reservation holding less than what's left of that file, which every mount after a
/// crash renews.
#[test]
fn a_compaction_cut_short_mounts() {
    for blocks in [TIGHT, ROOMY] {
        for seed in 0..8 {
            Setting::calm(seed, blocks).cutting_one_in(1).holds(vec![Op::Compact]);
        }
    }
}

/// A clone of thousands of fragments, or over them, takes as many commits as hold it.
#[test]
fn a_clone_of_many_fragments_fits_in_transactions() {
    let fragments = |file| Op::Fill { file, page: 0, pages: 4096, stride: 2, byte: 1 };
    let ops = vec![
        Op::Create(0),
        Op::Create(1),
        Op::Create(2),
        fragments(0),
        Op::Fill { file: 1, page: 0, pages: 4096, stride: 1, byte: 2 },
        Op::Clone { from: 0, to: 1, page: 0, pages: 4096, to_page: 0 },
        Op::Write { file: 2, at: 0, len: 1, byte: 3 },
        fragments(2),
        Op::Clone { from: 1, to: 2, page: 0, pages: 4096, to_page: 0 },
        Op::Sync,
    ];
    Setting::calm(0, ROOMY).holds(ops);
    let ops = vec![
        Op::Create(0),
        Op::Create(1),
        Op::Fill { file: 8, page: 0, pages: 2945, stride: 1, byte: 0 },
        Op::Clone { from: 2, to: 13, page: 0, pages: 2937, to_page: 0 },
    ];
    Setting::calm(0, ROOMY).holds(ops);
}

/// A punch, a truncate and an unlink of thousands of fragments each take as many commits as hold
/// them: what stops them is what they take of the journal, which only the transactions' check sees
/// of those that borrow what they use of it.
#[test]
fn freeing_many_fragments_fits_in_transactions() {
    let fragments = |file| Op::Fill { file, page: 0, pages: 8192, stride: 2, byte: 1 };
    let ops = vec![
        Op::Create(0),
        Op::Create(1),
        Op::Create(2),
        fragments(0),
        fragments(1),
        fragments(2),
        Op::Punch { file: 0, page: 0, pages: 8192 },
        Op::Truncate { file: 1, size: 0 },
        Op::Unlink(2),
        Op::Sync,
    ];
    Setting::calm(0, ROOMY).holds(ops);
}

/// A file with more extents than its batch's transaction has room to free, unlinked and evicted in
/// it, is left to the graveyard, which frees the rest. Only such a file goes that way: what a
/// transaction holds decides which those are, and a hunt's fills came to reach it by chance once
/// it held more than 200 mutations.
#[test]
fn a_file_too_large_for_its_batch_goes_through_the_graveyard() {
    let ops = vec![
        Op::Create(0),
        Op::Fill { file: 0, page: 0, pages: 8192, stride: 2, byte: 1 },
        Op::UnlinkLater(0),
        Op::Sync,
    ];
    Setting::calm(0, ROOMY).holds(ops);
}

/// The journal's padding is paid for by a transaction of its own, which a replay that discarded
/// more of the journal than a transaction reserves must not panic: found by a hunt, whose seed this
/// keeps, as what the cuts lose depends on it.
#[test]
fn a_replay_discarding_much_of_the_journal_is_paid_for() {
    let ops = vec![
        Op::Create(1),
        Op::Crash,
        Op::Fill { file: 0, page: 0, pages: 42, stride: 1, byte: 0 },
        Op::Sync,
        Op::Fill { file: 0, page: 0, pages: 1025, stride: 1, byte: 0 },
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Unlink(1),
        Op::Crash,
        Op::Create(0),
        Op::Write { file: 0, at: 1, len: 4096, byte: 0 },
        Op::Fill { file: 0, page: 0, pages: 1011, stride: 2, byte: 0 },
        Op::Write { file: 0, at: 2093056, len: 8193, byte: 0 },
        Op::Write { file: 0, at: 7, len: 12282, byte: 0 },
        Op::Sync,
        Op::Truncate { file: 0, size: 4096 },
    ];
    Setting::calm(8774446482155464184, ROOMY).holds(ops);
}

/// A write the device lost, of a block a clone shared, must be found on replay though the clone's
/// reference has gone: replay took the block for dead, and the file read what the device held.
/// Found by a hunt, whose seed this keeps, as what the cuts lose depends on it.
#[test]
fn a_write_lost_of_a_block_a_clone_shared_is_rolled_back() {
    let ops = vec![
        Op::Create(0),
        Op::Sync,
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Sync,
        Op::Create(0),
        Op::Write { file: 0, at: 57344, len: 8193, byte: 0 },
        Op::Clone { from: 0, to: 0, page: 14, pages: 1, to_page: 6 },
        Op::Write { file: 0, at: 24576, len: 1, byte: 0 },
        Op::Write { file: 0, at: 1, len: 4096, byte: 0 },
        Op::Punch { file: 0, page: 15, pages: 1 },
        Op::Punch { file: 0, page: 0, pages: 1 },
        Op::RemoveXattr { file: 0, name: 0 },
        Op::Read { file: 0, at: 0, len: 1 },
    ];
    Setting::calm(3957436890551874953, ROOMY).holds(ops);
}

/// What a crash orphans takes room on the device Fxfs doesn't count, which it frees as it mounts,
/// whether or not the journal's tail needed renewing: found by a hunt, whose seed this keeps, as
/// what the cuts lose depends on it.
#[test]
fn what_a_crash_orphans_leaves_the_room_it_took() {
    let ops = vec![
        Op::Create(0),
        Op::Read { file: 0, at: 0, len: 1 },
        Op::Create(0),
        Op::Fill { file: 0, page: 6, pages: 1247, stride: 1, byte: 0 },
        Op::Create(0),
        Op::Fill { file: 0, page: 0, pages: 1408, stride: 1, byte: 0 },
    ];
    Setting::calm(3625385343287752248, TIGHT).holds(ops);
}

/// A super-block records how far objects are flushed as of the journal it flushed: read later, it
/// said they were past what a crash kept, and replay gave up what only the journal held, an
/// extension of the journal's file among it, its extents allocated but no part of it, which fsck
/// reports. Found by a hunt, whose seed this keeps, as what the cuts lose depends on it.
#[test]
fn a_super_block_records_only_what_its_journal_flushed() {
    let fill = |file, page, pages, stride| Op::Fill { file, page, pages, stride, byte: 0 };
    let ops = vec![
        Op::Compact,
        Op::Sync,
        Op::Create(2),
        Op::Create(0),
        Op::Write { file: 14, at: 4096, len: 1, byte: 0 },
        Op::Create(1),
        Op::Crash,
        fill(10, 9, 2233, 2),
        Op::Truncate { file: 9, size: 0 },
        Op::Clone { from: 1, to: 2, page: 9, pages: 2151, to_page: 0 },
        Op::Remount,
        fill(0, 0, 2230, 1),
        Op::Read { file: 0, at: 0, len: 1 },
        Op::Remount,
        Op::Unlink(0),
        Op::Create(1),
        Op::Create(0),
        Op::Read { file: 0, at: 0, len: 1 },
        fill(0, 1, 1858, 1),
        Op::Compact,
        Op::Create(0),
    ];
    Setting::calm(1738431799382197223, ROOMY).holds(ops);
}

/// The first transaction of a mount, renewing the journal's tail, may find nothing dirty, its own
/// checkpoint the earliest, ahead of what has been accounted for, nor may what a crash left of a
/// super-block's flush be more than it recorded. Found by a hunt, whose seed this keeps, as what
/// the cuts lose depends on it.
#[test]
fn a_renewal_of_a_journal_compacted_clean_accounts_for_nothing() {
    let fill = |page, pages, stride| Op::Fill { file: 0, page, pages, stride, byte: 0 };
    let ops = vec![
        Op::Create(0),
        fill(31, 8, 1),
        Op::Sync,
        fill(0, 15, 1),
        fill(31, 4057, 2),
        fill(31, 43, 2),
        Op::Sync,
        Op::Truncate { file: 0, size: 16384 },
        fill(1, 3976, 1),
        Op::Crash,
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Remount,
        Op::Compact,
        Op::Compact,
    ];
    Setting::calm(4949468108853898276, ROOMY).holds(ops);
}

/// A hole is read: one taken for what a crash left, a punch the journal never held, wasn't, and
/// a page of it was found holding what it did only after going on. Found by a hunt, whose seed this
/// keeps, as what the cuts lose depends on it.
#[test]
fn what_a_crash_kept_is_matched_with_its_holes() {
    let ops = vec![
        Op::Create(0),
        Op::Fill { file: 0, page: 8, pages: 15, stride: 2, byte: 0 },
        Op::Remount,
        Op::Fill { file: 0, page: 11, pages: 8, stride: 1, byte: 0 },
        Op::Rename(0, 0),
        Op::Write { file: 0, at: 32768, len: 1, byte: 1 },
        Op::Crash,
        Op::Punch { file: 0, page: 6, pages: 3 },
        Op::Write { file: 0, at: 1, len: 4096, byte: 0 },
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Fill { file: 0, page: 0, pages: 59, stride: 2, byte: 0 },
        Op::Sync,
        Op::Write { file: 0, at: 20481, len: 4096, byte: 0 },
    ];
    Setting::calm(10934300477646575156, TIGHT).holds(ops);
}

/// A mount that renews the journal's tail writes in extents only a super-block records. A flush
/// let what it found dead be freed before that super-block was written: a cut then left a replay,
/// from the last one, with no sign of what the mount discarded, verifying a write to an extent it
/// had freed: a filesystem that mounts no more. On a calm device, the frees wait for an executor's
/// turn the mount doesn't give them. Found by a hunt, whose seed this keeps, as what the cuts lose
/// and when the device answers depend on it.
#[test]
fn a_mount_frees_nothing_until_a_super_block_records_its_journal() {
    let ops = vec![
        Op::Create(0),
        Op::Write { file: 0, at: 12288, len: 4101, byte: 230 },
        Op::Create(0),
        Op::Write { file: 6, at: 0, len: 4096, byte: 128 },
        Op::Write { file: 12, at: 12288, len: 47, byte: 234 },
        Op::Crash,
        Op::Link(7, 4),
        Op::Fill { file: 10, page: 46, pages: 3287, stride: 2, byte: 76 },
        Op::Write { file: 4, at: 12288, len: 4096, byte: 32 },
        Op::Unlink(0),
        Op::Compact,
    ];
    Setting::awkward(8643161789968376626, ROOMY).holds(ops);
}

/// Numbers sought for orphans that a lease's move left the device never having held die in as many
/// records, in a compaction's room no reservation counts: it failed, and took the journal with it.
/// Found by a hunt, whose seed this keeps, as what the device's numbers do depends on it.
#[test]
fn what_a_mount_sought_leaves_a_compaction_its_room() {
    let ops = vec![
        Op::Create(0),
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Remount,
        Op::Compact,
    ];
    Setting::awkward(1312117120894968101, ROOMY).holds(ops);
}

/// A compaction moves old layers to the graveyard and tombstones them: one let run by a mount
/// before its initial reap put them on the reap's list too, and a file gone was tombstoned, which
/// panics, or does what it may to what's left. Found by a hunt, whose seed this keeps, as when the
/// device answers depends on it.
#[test]
fn a_mount_compacts_nothing_before_its_initial_reap() {
    let ops = vec![
        Op::Create(0),
        Op::Create(0),
        Op::Write { file: 0, at: 0, len: 1, byte: 209 },
        Op::Read { file: 11, at: 34507, len: 52 },
        Op::Fsync,
        Op::Rename(2, 5),
        Op::Read { file: 10, at: 9223372036854770658, len: 4096 },
        Op::Fsync,
        Op::Write { file: 5, at: 16384, len: 4096, byte: 178 },
        Op::Fill { file: 10, page: 24, pages: 31, stride: 1, byte: 142 },
        Op::SetXattr { file: 5, name: 2, len: 2365, byte: 169 },
        Op::Fill { file: 10, page: 19, pages: 2807, stride: 2, byte: 211 },
        Op::Create(3),
        Op::Fill { file: 6, page: 26, pages: 2849, stride: 2, byte: 127 },
        Op::Write { file: 9, at: 2094745, len: 27, byte: 192 },
        Op::Write { file: 4, at: 45056, len: 4096, byte: 70 },
        Op::Fill { file: 8, page: 35, pages: 62, stride: 1, byte: 167 },
        Op::Create(3),
        Op::Remount,
        Op::Clone { from: 5, to: 8, page: 7, pages: 406, to_page: 27 },
        Op::Fill { file: 7, page: 22, pages: 4041, stride: 1, byte: 176 },
    ];
    Setting::awkward(5760810855262097532, ROOMY).holds(ops);
}

/// The staging extent, emptied, took a small write a cut lost: its replay discarded the write's
/// transaction, so the extent died at the deallocation that emptied it, and was freed as the mount
/// began. A cut before the journal recorded the discard had the next replay read the write again,
/// and verify it in an extent the device no longer held: a filesystem that mounted no more. Found
/// by CI (run 41876989384), whose seed this keeps, as what the cuts lose depends on it.
#[test]
fn a_mount_frees_what_replay_found_dead_once_its_discard_is_recorded() {
    let ops = vec![
        Op::Remount,
        Op::Symlink(0, Target::SHORT),
        Op::Create(1),
        Op::Read { file: 0, at: 0, len: 1 },
        Op::Compact,
        Op::Open(0),
        Op::Write { file: 0, at: 2093056, len: 4096, byte: 0 },
        Op::Read { file: 0, at: 0, len: 1 },
        Op::RemoveXattr { file: 0, name: 0 },
        Op::Symlink(0, Target::SHORT),
        Op::Write { file: 0, at: 16384, len: 1, byte: 0 },
        Op::Write { file: 0, at: 2093057, len: 28, byte: 156 },
        Op::Create(4),
        Op::Rename(0, 20),
        Op::UnlinkLater(1),
        Op::Truncate { file: 0, size: 14044 },
        Op::Fill { file: 8, page: 42, pages: 24, stride: 1, byte: 215 },
        Op::Clone { from: 4, to: 2, page: 8, pages: 166, to_page: 20 },
        Op::Mkdir(2),
        Op::UnlinkLater(15),
        Op::Rename(1, 3),
        Op::Clone { from: 6, to: 14, page: 28, pages: 1, to_page: 1048577 },
        Op::Fill { file: 2, page: 10, pages: 1761, stride: 2, byte: 146 },
        Op::Write { file: 10, at: 38692, len: 4096, byte: 244 },
        Op::Mkdir(1),
        Op::Exchange(15, 3),
        Op::Truncate { file: 2, size: 63582 },
    ];
    Setting::awkward(16794538089474069855, ROOMY).with_graveyard_of_seed().holds(ops);
}

/// A truncate of a file with an unlink of another of its names queued, which the truncate needn't
/// wait for, but the read back after it applies: a cut may keep the truncate and lose the unlink.
/// The model forgot the unlink as done once the read back had applied it, after the truncate, and
/// found none of its states on the device. Found by a hunt, whose seed this keeps, as what the
/// cuts lose depends on it.
#[test]
fn an_unlink_applied_after_an_operation_may_be_cut_with_it_kept() {
    let ops = vec![
        Op::Create(0),
        Op::Write { file: 0, at: 53248, len: 8193, byte: 0 },
        Op::Write { file: 0, at: 77824, len: 1, byte: 0 },
        Op::Write { file: 0, at: 1, len: 4096, byte: 0 },
        Op::Write { file: 0, at: 49152, len: 4096, byte: 1 },
        Op::Sync,
        Op::Fill { file: 0, page: 44, pages: 2579, stride: 2, byte: 0 },
        Op::Write { file: 0, at: 0, len: 4096, byte: 0 },
        Op::Link(0, 3),
        Op::Write { file: 0, at: 4294963200, len: 1, byte: 0 },
        Op::Fill { file: 0, page: 43, pages: 38, stride: 1, byte: 0 },
        Op::Crash,
        Op::Fill { file: 0, page: 7, pages: 1979, stride: 2, byte: 0 },
        Op::Compact,
        Op::Create(1),
        Op::SetXattr { file: 1, name: 0, len: 1024, byte: 69 },
        Op::UnlinkLater(3),
        Op::Truncate { file: 0, size: 52592 },
    ];
    Setting::awkward(6784763639641074038, ROOMY).with_graveyard_of_seed().holds(ops);
}

/// The fsck hunt's sequence of TASK-182: a power cut, with fsck after every transaction, left a
/// journal that replay refused, as ending before where a barrier had found it written.
#[test]
fn a_cut_with_fsck_after_every_transaction_replays_as_far_as_its_barrier() {
    let ops = vec![
        Op::SetDirXattr { dir: 0, name: 0, len: 1, byte: 0 },
        Op::Create(1),
        Op::Tmpfile(0),
        Op::Tmpfile(0),
        Op::Symlink(0, Target::SHORT),
        Op::Write { file: 10, at: 8192, len: 1, byte: 0 },
        Op::Sync,
        Op::Write { file: 4, at: 40960, len: 4096, byte: 0 },
        Op::Sync,
        Op::Fill { file: 3, page: 0, pages: 3945, stride: 1, byte: 0 },
        Op::UnlinkLater(1),
        Op::Symlink(2, Target::SHORT),
        Op::RemoveXattr { file: 0, name: 0 },
        Op::Truncate { file: 1, size: 4096 },
        Op::Fill { file: 14, page: 3, pages: 3377, stride: 1, byte: 0 },
        Op::Write { file: 2, at: 16384, len: 8193, byte: 0 },
        Op::Write { file: 8, at: 1, len: 4096, byte: 0 },
        Op::Write { file: 1, at: 1, len: 4096, byte: 0 },
        Op::Crash,
        Op::Truncate { file: 8, size: 4294963200 },
        Op::Close(5),
        Op::Symlink(1, Target::SHORT),
        Op::Fill { file: 0, page: 0, pages: 2997, stride: 2, byte: 0 },
    ];
    let setting = Setting::awkward(12436301060589371142, ROOMY).with_graveyard_of_seed();
    setting.with_fsck_each(true).holds(ops);
}

/// The hunt's sequence of CI run 42196106048: a power cut during the replay of a power cut left a
/// journal whose replay read an extent the device doesn't hold, and failed the mount.
#[test]
fn a_cut_during_a_replay_leaves_a_journal_that_replays() {
    let ops = vec![
        Op::Symlink(0, Target::SHORT),
        Op::Unlink(0),
        Op::Open(0),
        Op::Compact,
        Op::Open(0),
        Op::Read { file: 0, at: 4294963247, len: 4096 },
        Op::Fill { file: 8, page: 18, pages: 745, stride: 1, byte: 119 },
        Op::UnlinkLater(1),
        Op::Unlink(1),
        Op::Read { file: 14, at: 118784, len: 7754 },
        Op::Open(8),
        Op::Write { file: 8, at: 61440, len: 50, byte: 40 },
        Op::Crash,
        Op::Clone { from: 7, to: 9, page: 1048576, pages: 7, to_page: 2251799813685238 },
        Op::Write { file: 13, at: 2096015, len: 4096, byte: 23 },
        Op::Create(0),
        Op::Create(16),
        Op::Symlink(4, Target::SHORT),
        Op::Read { file: 0, at: 9223372036854775522, len: 4096 },
        Op::SetXattr { file: 5, name: 0, len: 37, byte: 105 },
        Op::FillWhileCompacting { file: 5, page: 42, pages: 10, byte: 189, flushing: true },
    ];
    Setting::awkward(8160502985120782900, ROOMY).with_graveyard_of_seed().holds(ops);
}

/// A full device's mount renews the journal's tail before the flush that records what its replay
/// discarded, which needs the room of what the mount found dead: freeing none of it before that
/// flush, rather than only what the transactions discarded name, left a cut that mounted no more
/// ("reallocating the journal's tail ... No space"). Found by the hunt of TASK-210.
#[test]
fn a_full_device_mounts_with_the_room_of_what_it_found_dead() {
    let ops = vec![
        Op::Create(0),
        Op::Unlink(0),
        Op::Create(2),
        Op::Create(0),
        Op::Fill { file: 0, page: 38, pages: 807, stride: 2, byte: 0 },
        Op::Create(0),
        Op::Compact,
        Op::Create(0),
        Op::Create(0),
        Op::Create(0),
        Op::Create(0),
        Op::Create(0),
        Op::Fill { file: 1, page: 26, pages: 1272, stride: 1, byte: 0 },
        Op::Fill { file: 1, page: 13, pages: 44, stride: 1, byte: 0 },
        Op::Write { file: 6, at: 4294963241, len: 4072, byte: 121 },
        Op::Fill { file: 5, page: 51, pages: 1090, stride: 2, byte: 216 },
        Op::FillWhileCompacting { file: 2, page: 37, pages: 23, byte: 249, flushing: true },
        Op::Write { file: 7, at: 56047, len: 6514, byte: 3 },
        Op::SetXattr { file: 7, name: 0, len: 7198, byte: 121 },
        Op::Truncate { file: 3, size: 4294964062 },
        Op::Truncate { file: 3, size: 28672 },
    ];
    Setting::awkward(4758196049501471983, TIGHT).with_graveyard_of_seed().holds(ops);
}

/// A full device, unmounted cleanly, mounts again: replay's metadata reservation found less free
/// than it asked of the device the sequence had filled (TASK-211). Found by the hunt of TASK-210.
#[test]
fn a_full_device_unmounted_cleanly_mounts_again() {
    let ops = vec![
        Op::Create(2),
        Op::Create(0),
        Op::Fill { file: 5, page: 0, pages: 1329, stride: 2, byte: 0 },
        Op::Fill { file: 0, page: 0, pages: 1885, stride: 2, byte: 0 },
        Op::Fill { file: 0, page: 0, pages: 45, stride: 1, byte: 0 },
        Op::Punch { file: 0, page: 0, pages: 1 },
    ];
    Setting::awkward(0, TIGHT).with_graveyard_of_seed().holds(ops);
}

/// fallocate grows a file and sets nothing aside: what it covers reads as zeros and takes no
/// blocks, and what's written over it after, or cut, reads back, with nothing written in place.
/// Fxfs's preallocation made extents written in place, which copy-on-write writes replaced, its
/// record still saying it had some, which fsck refuses: the FUSE fuzzer's case (TASK-149.39), a
/// byte allocated at 28672, a write from 28469 over it. Before, growing a file allocated part of
/// a block failed EIO, its tail read from an extent nothing wrote (TASK-149.36), at once with a
/// power cut, which these seeds keep.
#[test]
fn an_allocation_grows_a_file_setting_nothing_aside() {
    let ops = vec![
        Op::Create(0),
        Op::Allocate { file: 0, at: 28672, len: 1 },
        Op::Write { file: 0, at: 28469, len: 204, byte: 1 },
        Op::Sync,
        Op::Allocate { file: 0, at: 0, len: 1 },
        Op::Truncate { file: 0, size: 4096 },
        Op::Allocate { file: 0, at: 8192, len: 4097 },
        Op::Write { file: 0, at: 16384, len: 1, byte: 1 },
        Op::Remount,
        Op::Truncate { file: 0, size: 3 * 4096 + 7 },
        Op::Truncate { file: 0, size: 4 * 4096 },
    ];
    Setting::calm(0, ROOMY).cutting_one_in(u32::MAX).holds(ops);
    let ops = vec![Op::Create(0), Op::Allocate { file: 0, at: 0, len: 1 }];
    Setting::awkward(54400744546958328, ROOMY).with_graveyard_of_seed().holds(ops);
}

/// FIFOs, sockets and devices read back as made, by lookup and by readdir, linked, moved and cut.
/// A readdir took a node's type from its entry, which says only that it is a file: a FIFO was
/// listed as a regular file, which `find -type p` and `ls` take a listing's word for. Found by
/// the model's special files (TASK-149.36).
#[test]
fn special_files_read_back_as_made() {
    let make = |kind, rdev| Make { kind, permissions: 0o640 | 1 << 16, uid: 7, gid: 8, rdev };
    let ops = vec![
        Op::Mkdir(0),
        Op::Make { name: 1, make: make(2, 0) },
        Op::Make { name: 2, make: make(3, 0) },
        Op::Make { name: 3, make: make(4, 0x1234_5678) },
        Op::Make { name: NAMES + 1, make: make(5, u32::MAX) },
        Op::Link(0, 4),
        Op::ReadSome { dir: 0, count: 2 },
        Op::Rename(1, NAMES + 2),
        Op::ReadOn { cursor: 0, count: 8 },
        Op::SetAttr { node: 2, change: Change { valid: SET_MODE | SET_GID, ..CHANGE } },
        Op::Sync,
        Op::Unlink(2),
        Op::Remount,
    ];
    Setting::awkward(0, ROOMY).cutting_everywhere().holds(ops);
}

/// Symlinks' targets of every length read back, those longer than a record holds kept in
/// extended attributes of their own, which listing a symlink's leaves out: beside those set, which
/// alone it lists, moved, unlinked and cut (TASK-149.40).
#[test]
fn long_symlinks_read_back_and_list_only_what_was_set() {
    let target = |len| Target { len, byte: len as u8 };
    let set = XattrChange::Set { len: 100, byte: 9 };
    let ops = vec![
        Op::Symlink(0, target(MAX_INLINE_SYMLINK + 1)),
        Op::Symlink(1, target(MAX_LONG_SYMLINK)),
        Op::Symlink(2, target(MAX_INLINE_SYMLINK)),
        Op::NodeXattr { node: 1, name: 3, change: set },
        Op::NodeXattr { node: 2, name: 4, change: set },
        Op::NodeXattr { node: 2, name: 4, change: XattrChange::Remove },
        Op::Sync,
        Op::Rename(1, NAMES + 1),
        Op::Unlink(2),
        Op::Remount,
    ];
    Setting::awkward(0, ROOMY).cutting_everywhere().holds(ops);
}

/// On the guest's platform, whose VFS keeps directories' times, a create of a file or a special
/// file leaves its directory's times as they were, set here, as the VFS writes them back; a mkdir,
/// Fxfs's own, times it (TASK-149.40).
#[test]
fn creates_on_the_guests_platform_leave_their_directorys_times() {
    let times = Change { valid: SET_MTIME | SET_CTIME, ..CHANGE };
    let ops = vec![
        Op::SetAttr { node: 0, change: times },
        Op::Create(1),
        Op::Make { name: 2, make: Make { kind: 2, ..Make::FILE } },
        Op::Sync,
        Op::Remount,
        Op::SetAttr { node: 0, change: times },
        Op::Mkdir(3),
        Op::Remount,
    ];
    Setting::awkward(0, ROOMY).on_the_guests_platform().cutting_everywhere().holds(ops);
}

/// The page cache's writeback on a full filesystem: the room its pages reserved as they were
/// dirtied pays for their writes, which nothing else has room for then. Of dirtyings halving in
/// size, the first the filesystem has room to reserve for has no room for its writes beside: but
/// for that room, its writeback would be refused. What the page cache drops unwritten gives its
/// room back, and a reservation the filesystem hasn't room for is refused, before any write
/// (TASK-149.40).
#[test]
fn writeback_on_a_full_filesystem_is_paid_for_by_the_room_it_reserved() {
    let dirty = |page, pages, dropped| Op::WriteDirty { file: 0, page, pages, byte: 7, dropped };
    let mut ops = vec![
        Op::Create(0),
        Op::Create(1),
        dirty(0, 256, true),
        dirty(0, 256, false),
        Op::Fill { file: 1, page: 0, pages: TIGHT, stride: 1, byte: 1 },
        Op::Truncate { file: 1, size: 3 * TIGHT / 4 * PAGE },
        Op::Sync,
    ];
    ops.extend((0..11).map(|half| dirty(1024 + 2048 * half, 1024 >> half, false)));
    ops.push(Op::Remount);
    Setting::awkward(0, TIGHT).holds(ops);
}

/// A snapshot of a volume whose graveyard holds a file closed, not yet tombstoned, as a guest's
/// batches its tombstones: remounted, the volume still syncs, and its snapshot holds nothing of
/// the file. The graveyard tombstoned it during the snapshot's flush, freeing its extent, which the
/// snapshot shared; remounted, the snapshot's graveyard freed it again, an inconsistency the
/// journal failed with (TASK-149.40's snapshots found it, TASK-241).
#[test]
fn a_snapshot_of_a_file_closed_but_not_yet_tombstoned() {
    let ops = vec![
        Op::Tmpfile(0),
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Close(0),
        Op::Snapshot,
    ];
    Setting::awkward(0, ROOMY).with_graveyard_of_seed().holds(ops);
}

/// Snapshots hold what the volume did as they were taken, as the volume goes on: its files
/// written over, truncated, unlinked and renamed, a directory made, across cuts and remounts. A
/// snapshot shares the volume's extents, which neither frees while both do (TASK-149.40).
#[test]
fn snapshots_hold_what_the_volume_did_as_it_goes_on() {
    let write = |file, byte| Op::Fill { file, page: 0, pages: 4, stride: 1, byte };
    let ops = vec![
        Op::Create(0),
        Op::Create(1),
        write(0, 1),
        write(1, 2),
        Op::Snapshot,
        write(0, 3),
        Op::Truncate { file: 1, size: PAGE / 2 },
        Op::Sync,
        Op::Snapshot,
        Op::Unlink(0),
        Op::Rename(1, 2),
        Op::Mkdir(3),
        Op::Remount,
    ];
    Setting::awkward(0, ROOMY).cutting_everywhere().holds(ops);
}

/// A snapshot, an unlink queued and a write, every cut of them: land-all11's macOS run of
/// every_cut_of_a_short_sequence hung on it, a commit waiting for its lock and another for room in
/// the journal, before the fixes of TASK-241 and TASK-245.
#[test]
fn every_cut_of_a_snapshot_an_unlink_queued_and_a_write() {
    let ops = vec![
        Op::Snapshot,
        Op::UnlinkLater(5),
        Op::Write { file: 13, at: 36085, len: 2686, byte: 15 },
    ];
    let setting = Setting::awkward(17604531016692502004, ROOMY).with_graveyard_of_seed();
    setting.cutting_everywhere().holds(ops);
}

/// A change of mode, owner, group and times, as `Op::SetAttr` asks.
const CHANGE: Change = Change {
    valid: SET_ANY,
    permissions: 0o4750,
    uid: 1000,
    gid: 100,
    atime: 1,
    mtime: 999_999_999,
    ctime: 1 << 59,
};

/// The root's attributes, which Fxfs gives no record until they change, change, one at a time and
/// all at once, and its times, and a directory's, move as its names do: TASK-222 found a change of
/// one of the root's mode, owner and group alone zero the others (TASK-149.36).
#[test]
fn the_roots_attributes_change_as_any_nodes() {
    let change = |valid| Op::SetAttr { node: 0, change: Change { valid, ..CHANGE } };
    let ops = vec![
        change(SET_UID),
        change(SET_GID),
        change(SET_MODE),
        Op::Create(0),
        change(SET_ATIME | SET_MTIME),
        Op::Mkdir(1),
        change(SET_CTIME),
        Op::Rename(0, NAMES),
        change(SET_ANY),
        Op::NodeXattr { node: 0, name: 11, change: XattrChange::Set { len: 300, byte: 1 } },
        Op::NodeXattr { node: 0, name: 11, change: XattrChange::Replace { len: 0, byte: 0 } },
        Op::SetDirXattr { dir: 0, name: 3, len: MAX_XATTR_VALUE_SIZE, byte: 2 },
        Op::SetDirXattr { dir: 0, name: 4, len: MAX_XATTR_VALUE_SIZE + 1, byte: 3 },
        Op::NodeXattr { node: 0, name: 3, change: XattrChange::Create { len: 1, byte: 4 } },
        Op::NodeXattr { node: 0, name: 5, change: XattrChange::Remove },
        Op::Exchange(1, NAMES),
    ];
    Setting::awkward(0, ROOMY).cutting_everywhere().holds(ops);
}

/// Names of every spelling (`SPELLINGS`), as long as a name may be and of many bytes a character,
/// in a directory that folds case, where names in another case are one, and readdirs going on
/// while names come and go.
#[test]
fn names_of_every_spelling_fold_and_list() {
    let in_a = |at: usize| (at / NAMES) * NAMES * DIRS + NAMES + at % NAMES;
    let mut ops = vec![Op::Mkdir(0), Op::Casefold { dir: 1, folds: true }];
    ops.extend((0..2 * NAMES).map(|at| Op::Create(in_a(at))));
    ops.extend([
        Op::ReadSome { dir: 1, count: 2 },
        Op::Unlink(in_a(8)),
        Op::Create(in_a(9)),
        Op::Rename(in_a(10), in_a(1)),
        Op::ReadOn { cursor: 0, count: 1 },
        Op::Mkdir(in_a(2)),
        Op::ReadOn { cursor: 0, count: 8 },
        Op::BadName { dir: 1, name: 0 },
        Op::BadName { dir: 0, name: 3 },
        Op::Create(2 * NAMES * DIRS - 2),
        Op::Rename(in_a(11), 2 * NAMES * DIRS - 1),
        Op::Remount,
    ]);
    Setting::awkward(0, ROOMY).holds(ops);
}

/// An allocation over pages a fill wrote changes nothing of them, and a truncate into the first
/// zeroes what it held past the new end, writing it anew. Found by the default run once
/// TASK-149.36 was merged, when allocations still made extents written in place, its seed kept.
#[test]
fn an_allocation_over_written_pages_changes_nothing() {
    let ops = vec![
        Op::Symlink(1, Target::SHORT),
        Op::Create(0),
        Op::FillWhileCompacting { file: 0, page: 0, pages: 13, byte: 1, flushing: false },
        Op::Allocate { file: 0, at: 0, len: 49153 },
        Op::Truncate { file: 0, size: 1 },
    ];
    Setting::awkward(0, ROOMY).with_graveyard_of_seed().holds(ops);
}

/// A name's unlink queued in a directory that folds case leaves the directory's times to the VFS,
/// as one in any other directory does: applied after a setattr of the directory's mtime, it timed
/// the directory as it applied, over what was set. Found by a hunt (TASK-149.36), its seed kept.
#[test]
fn an_unlink_queued_in_a_casefolded_directory_leaves_its_times() {
    let mtime = Op::SetAttr { node: 0, change: Change { valid: SET_MTIME, mtime: 0, ..CHANGE } };
    let ops = vec![
        Op::Casefold { dir: 0, folds: true },
        Op::Create(18),
        Op::UnlinkLater(0),
        mtime,
        Op::SetAttr { node: 0, change: Change { valid: 0, ..CHANGE } },
    ];
    Setting::awkward(0, ROOMY).with_graveyard_of_seed().holds(ops);
}

/// Allocations on a filesystem filling up are refused past its room, what's free and what the file
/// takes, and taken within it, and clones over what they grew share what they clone. Found by a
/// hunt (TASK-149.36) when an allocation's blocks couldn't be shared, its seed kept, as what fills
/// the device depends on it.
#[test]
fn allocations_on_a_full_filesystem_take_only_the_room() {
    let ops = vec![
        Op::Create(0),
        Op::Fill { file: 0, page: 4, pages: 39, stride: 1, byte: 0 },
        Op::Symlink(0, Target::SHORT),
        Op::Tmpfile(0),
        Op::SetXattr { file: 0, name: 0, len: 63984, byte: 0 },
        Op::Fill { file: 0, page: 43, pages: 2175, stride: 2, byte: 0 },
        Op::Write { file: 0, at: 0, len: 8193, byte: 0 },
        Op::Allocate { file: 1, at: 0, len: 249857 },
        Op::Fill { file: 0, page: 2, pages: 805, stride: 2, byte: 0 },
        Op::Crash,
        Op::Punch { file: 0, page: 0, pages: 1 },
        Op::Clone { from: 7, to: 14, page: 0, pages: 1, to_page: 0 },
        Op::Allocate { file: 0, at: 0, len: TIGHT * PAGE },
        Op::Allocate { file: 1, at: 1 << 40, len: 4096 },
    ];
    Setting::awkward(0, TIGHT).with_graveyard_of_seed().holds(ops);
}

/// A replay cut during the replay of a cut finishes the trim of a truncate the cut stopped short,
/// as the replay it cut does, before a node's blocks are counted: a truncate of a thousand extents
/// frees them through the graveyard, after the mount, and the blocks were counted before. Found
/// by the default run (TASK-149.36), its seed kept, as what fails and is cut depends on it.
#[test]
fn a_cut_during_a_replay_counts_blocks_once_trimmed() {
    let ops = vec![
        Op::Remount,
        Op::Create(0),
        Op::Fill { file: 2, page: 14, pages: 2084, stride: 2, byte: 130 },
        Op::Write { file: 9, at: 9223372036854760149, len: 4096, byte: 109 },
        Op::Sync,
        Op::Symlink(4, Target::SHORT),
        Op::Sync,
        Op::Clone { from: 5, to: 7, page: 513, pages: 1, to_page: 274877906946 },
        Op::Sync,
        Op::Compact,
        Op::Truncate { file: 4, size: 61582 },
    ];
    let setting = Setting::awkward(1273004606570409581, ROOMY).with_graveyard_of_seed().failing();
    assert_eq!(setting.run(ops, Rc::default()), Ok(()), "{setting:?}");
}

/// TASK-233: a clone's commit, on a device whose room dead extents took, made room in the journal,
/// whose allocation synced to free them, and the sync recorded what dropped allocations reached
/// (TASK-213) in a transaction of its own: which waited for the commit lock the clone's commit held,
/// for ever. Found by the model test, as CI's workspace job shrank it past its 90 minutes, each
/// shrink 60 s waiting (filesystem.rs's debug_assert_not_too_long); then this, its 42 operations
/// cut to 8. A sync as a commit makes room records nothing; the next records it.
#[test]
fn a_commit_making_room_in_the_journal_syncs_without_committing() {
    let ops = vec![
        Op::Make {
            name: 14,
            make: Make {
                kind: 4,
                permissions: 593185,
                uid: 4294967294,
                gid: 1187,
                rdev: 3649396020,
            },
        },
        Op::Create(1),
        Op::FillWhileCompacting { file: 3, page: 34, pages: 22, byte: 44, flushing: false },
        Op::Create(3),
        Op::SetAttr {
            node: 0,
            change: Change {
                valid: 6,
                permissions: 461441,
                uid: 65534,
                gid: 1320,
                atime: 0,
                mtime: 0,
                ctime: 0,
            },
        },
        Op::Fill { file: 15, page: 55, pages: 3247, stride: 2, byte: 160 },
        Op::FillWhileCompacting { file: 10, page: 5, pages: 18, byte: 168, flushing: true },
        Op::CloneToEnd { from: 13, to: 5, page: 25, to_page: 274877906946 },
    ];
    Setting::awkward(8580878931066154398, TIGHT).with_graveyard_of_seed().holds(ops);
}

/// A replay of a power cut made during the replay of a power cut holds no extent Fxfs doesn't know,
/// across three writers' ranges: a mount found orphans of the second, minted past its floor in the
/// third, and was cut before their frees; the next sought orphans below the device's next alone, in
/// the third. Found by a hunt once the replays of cuts during replays were held to
/// `holds_no_orphans` (TASK-149.41), its seed kept.
#[test]
fn orphans_a_cut_replay_found_are_freed_across_writers_ranges() {
    let ops = vec![
        Op::Mkdir(0),
        Op::SetAttr {
            node: 0,
            change: Change {
                valid: 22,
                permissions: 592740,
                uid: 0,
                gid: 4294967294,
                atime: 605274433419370234,
                mtime: 2525524574,
                ctime: 0,
            },
        },
        Op::Remount,
        Op::Fsync,
        Op::Unlink(2),
        Op::Punch { file: 8, page: 19, pages: 3 },
        Op::CloneToEnd { from: 12, to: 5, page: 2251799813685246, to_page: 2251799813685241 },
        Op::Rename(5, 19),
        Op::Sync,
        Op::Write { file: 0, at: 4294966230, len: 6018, byte: 31 },
        Op::Open(8),
        Op::Make {
            name: 21,
            make: Make {
                kind: 0,
                permissions: 591027,
                uid: 4294967294,
                gid: 65534,
                rdev: 1683133175,
            },
        },
        Op::Statfs,
        Op::Crash,
        Op::Close(13),
        Op::Make {
            name: 5,
            make: Make { kind: 1, permissions: 2813, uid: 65534, gid: 0, rdev: 2362451879 },
        },
        Op::Truncate { file: 8, size: 9223372036854774438 },
        Op::Write { file: 2, at: 9223372036854765833, len: 4096, byte: 200 },
        Op::Create(0),
        Op::Close(8),
        Op::Mkdir(3),
        Op::SetXattr { file: 8, name: 8, len: 257, byte: 204 },
        Op::Remount,
        Op::BadName { dir: 1, name: 0 },
        Op::CloneToEnd { from: 1, to: 2, page: 20, to_page: 3 },
        Op::Write { file: 12, at: 2100257, len: 57, byte: 244 },
        Op::Exchange(4, 18),
        Op::Write { file: 15, at: 39939, len: 4096, byte: 210 },
        Op::Fill { file: 4, page: 35, pages: 12, stride: 1, byte: 71 },
        Op::UnlinkLater(3),
        Op::CloneToEnd { from: 4, to: 9, page: 16, to_page: 21 },
        Op::Make {
            name: 0,
            make: Make { kind: 2, permissions: 132088, uid: 0, gid: 1899, rdev: 1988163531 },
        },
        Op::NodeXattr { node: 19, name: 4, change: XattrChange::Remove },
        Op::Casefold { dir: 0, folds: true },
        Op::SetXattr { file: 11, name: 11, len: 3242, byte: 84 },
    ];
    Setting::awkward(15527335009487930946, 32768).with_graveyard_of_seed().holds(ops);
}

/// A replay holds no extent Fxfs doesn't know after a cut that came once the replay before had
/// recorded its journal's tail, before it freed the orphans what it discarded names, which wait
/// for that record: the next sought below the device's next alone, past the floor. Found by a hunt
/// once the replays of cuts during replays were held to `holds_no_orphans` (TASK-149.41), its seed
/// kept.
#[test]
fn orphans_a_discard_names_are_freed_after_a_cut_past_the_super_block() {
    let ops = vec![
        Op::Open(0),
        Op::Create(44),
        Op::RemoveXattr { file: 0, name: 0 },
        Op::Fill { file: 0, page: 9, pages: 45, stride: 2, byte: 1 },
        Op::Remount,
        Op::Truncate { file: 0, size: 36865 },
        Op::Write { file: 0, at: 2093057, len: 4096, byte: 0 },
        Op::Create(0),
        Op::FillWhileCompacting { file: 0, page: 0, pages: 14, byte: 0, flushing: false },
    ];
    Setting::awkward(16256079810524344455, 32768).with_graveyard_of_seed().holds(ops);
}

/// As `orphans_a_discard_names_are_freed_after_a_cut_past_the_super_block`, on a device whose
/// requests fail. Found by the same hunt (TASK-149.41), its seed kept.
#[test]
fn orphans_a_cut_replay_found_are_freed_on_a_device_that_fails() {
    let ops = vec![
        Op::Create(4),
        Op::Truncate { file: 3, size: 8029 },
        Op::Crash,
        Op::Punch { file: 0, page: 26, pages: 6 },
        Op::Rename(0, 25),
        Op::Fsync,
        Op::Tmpfile(25),
        Op::Write { file: 9, at: 45923, len: 4607, byte: 122 },
        Op::Mkdir(41),
        Op::Unlink(3),
        Op::FillWhileCompacting { file: 9, page: 52, pages: 8, byte: 196, flushing: false },
        Op::Fsync,
        Op::Fsync,
        Op::Rename(2, 3),
        Op::Compact,
        Op::Fill { file: 4, page: 10, pages: 3108, stride: 1, byte: 222 },
        Op::Exchange(2, 1),
        Op::Clone { from: 4, to: 1, page: 8, pages: 2, to_page: 9 },
        Op::Fill { file: 0, page: 53, pages: 23, stride: 1, byte: 159 },
        Op::Clone { from: 0, to: 5, page: 274877906946, pages: 7, to_page: 20 },
        Op::Fsync,
        Op::Sync,
        Op::Make {
            name: 1,
            make: Make {
                kind: 1,
                permissions: 393787,
                uid: 481,
                gid: 4294967294,
                rdev: 3514334670,
            },
        },
        Op::Sync,
        Op::ReadOn { cursor: 0, count: 4 },
        Op::Fsync,
        Op::Truncate { file: 12, size: 9223372036854775337 },
        Op::Fsync,
        Op::Make {
            name: 1,
            make: Make {
                kind: 1,
                permissions: 658827,
                uid: 4294967294,
                gid: 65534,
                rdev: 665549476,
            },
        },
        Op::FillWhileCompacting { file: 1, page: 28, pages: 4, byte: 241, flushing: false },
        Op::Fsync,
        Op::FillWhileCompacting { file: 3, page: 51, pages: 21, byte: 11, flushing: false },
        Op::Punch { file: 12, page: 29, pages: 4 },
        Op::Statfs,
        Op::Exchange(3, 1),
        Op::RemoveXattr { file: 0, name: 7 },
        Op::NodeXattr { node: 9, name: 10, change: XattrChange::Set { len: 5652, byte: 95 } },
        Op::Sync,
        Op::ReadOn { cursor: 0, count: 3 },
        Op::Fsync,
        Op::Sync,
    ];
    Setting::awkward(11133504338176470938, 2048).with_graveyard_of_seed().failing().holds(ops);
}

/// Orphans that transactions a replay discarded name, of a writer's range between two others, stay
/// unsought after a cut once that replay had recorded its journal's tail, before their frees: the
/// next mount seeks past what the tree holds and below the device's next. Counted apart, as
/// TASK-149.41.1 has them, until it's fixed; found by a hunt (TASK-149.41), its seed kept.
#[test]
fn orphans_named_by_a_discard_between_two_ranges_stay_unsought() {
    let ops = vec![
        Op::Tmpfile(0),
        Op::Crash,
        Op::Write { file: 0, at: 0, len: 1, byte: 0 },
        Op::Compact,
        Op::Remount,
        Op::Create(2),
        Op::Symlink(0, Target::SHORT),
        Op::Read { file: 0, at: 4294963445, len: 11338 },
        Op::Open(13),
        Op::Write { file: 6, at: 24576, len: 55, byte: 9 },
        Op::Mkdir(0),
        Op::Create(5),
        Op::Clone { from: 11, to: 12, page: 21, pages: 3, to_page: 27 },
        Op::Link(14, 0),
        Op::Fill { file: 4, page: 54, pages: 47, stride: 1, byte: 12 },
        Op::ReadOn { cursor: 2, count: 0 },
        Op::Casefold { dir: 1, folds: false },
        Op::Rename(17, 9),
        Op::Write { file: 13, at: 2096241, len: 6564, byte: 235 },
        Op::FillWhileCompacting { file: 2, page: 6, pages: 29, byte: 161, flushing: true },
    ];
    let setting = Setting::awkward(15596825076520818603, 32768).with_graveyard_of_seed();
    let tally = Rc::new(Tally::default());
    assert_eq!(setting.run(ops, tally.clone()), Ok(()));
    // The same 8, left by each of two replays cut during the replay.
    assert_eq!(tally.unsought_orphans.get(), 16, "TASK-149.41.1's orphans");
}
