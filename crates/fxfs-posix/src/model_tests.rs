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
//! thousands of fragments at once, for the transactions that makes, which the journal must hold.
//!
//! Power cuts: the device records itself crashing at random barriers, with any of what it wrote
//! since the barrier before lost (`storage_device::fake_device::Crashes`). What each holds, replayed,
//! must pass fsck, be what the model was after one of the operations since the last sync, show none
//! of what was cut off past a file's end, and go on with the sequence. The power is cut during that
//! replay too, which must leave the same.
//!
//! Seeded by proptest, which says the sequence if one fails, shrunk.
//!
//! FXDAX_MODEL_CASES asks for more sequences, to hunt. FXDAX_MODEL_FSCK does the hunt with fsck after
//! every transaction, slowly: corruption is found at the transaction that made it, not operations
//! later.
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

use super::*;
use fxfs::filesystem::{FlushReason, ForceMajor, FxFilesystemBuilder};
use fxfs::fsck::{FsckOptions, fsck_with_options};
use fxfs::object_store::graveyard::Graveyard;
use fxfs::object_store::journal::JournalOptions;
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc;
use std::time::Duration;
use storage_device::DeviceHolder;
use storage_device::fake_device::{Crashes, FakeDevice, Op as DeviceOp};

const PAGE: u64 = 4096;
const FILE: u32 = S_IFREG | 0o644;
const DIR: u32 = S_IFDIR | 0o755;
/// The largest size a file may have.
const LARGEST: u64 = i64::MAX as u64;
/// The whole pages a file may have.
const PAGES_AT_MOST: u64 = LARGEST / PAGE;
/// The names a sequence uses in a directory: few, so that they meet.
const NAMES: usize = 6;
/// The directories a name may be in, of those there are: it's `name / NAMES`th, the root first, so
/// that a name of less than `NAMES` is in the root whatever there is, and sequences shrink to it.
const DIRS: usize = 4;
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
/// How much of a file is read whole, holes and all: a file may be as large as a file may be.
const WHOLLY_READ: u64 = 64 << 20;
/// What every symlink says.
const SYMLINK_TARGET: &[u8] = b"target";
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

/// A file's model: its size, the pages written, which holes aren't, and its extended attributes.
#[derive(Clone, Debug, Default)]
struct Data {
    size: u64,
    pages: BTreeMap<u64, Arc<Vec<u8>>>,
    xattrs: BTreeMap<u8, Vec<u8>>,
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
        if !pages.is_empty() {
            let punched: Vec<u64> = self.pages.range(pages).map(|(page, _)| *page).collect();
            punched.iter().for_each(|page| drop(self.pages.remove(page)));
        }
    }

    /// How many extents `pages` holds at most: nothing of its holes counts.
    fn extents_in(&self, pages: Range<u64>) -> u64 {
        if pages.is_empty() { 0 } else { self.pages.range(pages).count() as u64 }
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

#[derive(Clone, Copy, Debug)]
enum Entry {
    File(u64),
    Symlink(u64),
    Dir(u64),
}

impl Entry {
    fn ino(self) -> u64 {
        match self {
            Entry::File(ino) | Entry::Symlink(ino) | Entry::Dir(ino) => ino,
        }
    }

    /// Its type, as `readdir` says it.
    fn kind(self) -> u32 {
        match self {
            Entry::File(_) => S_IFREG,
            Entry::Symlink(_) => S_IFLNK,
            Entry::Dir(_) => S_IFDIR,
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
    Symlink(usize),
    Mkdir(usize),
    /// A file's new name, or a tmpfile's first, as `linkat` makes.
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
    Write { file: usize, at: u64, len: usize, byte: u8 },
    /// Whole pages, every `stride`th of `pages`, in as few transactions as hold them, as writeback
    /// writes them.
    Fill { file: usize, page: u64, pages: u64, stride: u64, byte: u8 },
    /// Whole pages, a transaction each, as a compaction runs, which commits racing a super-block
    /// fall on either side of, and if `flushing`, with the volume's store flushed after each, as
    /// an encrypted store flushes itself when it must: as can its end.
    FillWhileCompacting { file: usize, page: u64, pages: u64, byte: u8, flushing: bool },
    Read { file: usize, at: u64, len: usize },
    Truncate { file: usize, size: u64 },
    Punch { file: usize, page: u64, pages: u64 },
    Clone { from: usize, to: usize, page: u64, pages: u64, to_page: u64 },
    /// A clone to the source's end, wherever in a block that is, as the VFS lets one: the block's
    /// taken whole, with the size where it ends. It may not end before the destination's size.
    CloneToEnd { from: usize, to: usize, page: u64, to_page: u64 },
    SetXattr { file: usize, name: u8, len: usize, byte: u8 },
    RemoveXattr { file: usize, name: u8 },
    Sync,
    Fsync,
    Compact,
    Remount,
    /// A crash just after a sync: what's on the device holds all there was, replayed.
    Crash,
}

impl Op {
    /// What this makes of a file's pages, if it does so in many transactions and can be done of
    /// what `model` has.
    fn plan(&self, model: &Model) -> Option<Plan> {
        match *self {
            Op::Fill { file, page, pages, stride, byte } => {
                let bytes = Some(Arc::new(vec![byte; PAGE as usize]));
                let pages = (page..page + pages).step_by(stride as usize);
                let runs =
                    pages.map(|page| Run { pages: page..page + 1, bytes: bytes.clone() }).collect();
                Some(Plan { ino: model.file(file)?, runs, size: None })
            }
            Op::FillWhileCompacting { file, page, pages, byte, .. } => {
                Op::Fill { file, page, pages, stride: 1, byte }.plan(model)
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

/// A name, mostly the root's, of those there are (`DIRS`).
fn name() -> impl Strategy<Value = usize> {
    prop_oneof![3 => 0..NAMES, 1 => 0..NAMES * DIRS]
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
        1 => (file(), file(), 0u64..8, 1u64..8, 0u64..16).prop_map(
            |(from, to, page, pages, to_page)| Op::Clone { from, to, page, pages, to_page }
        ),
        1 => (file(), 0u64..(64 << 10)).prop_map(|(file, size)| Op::Truncate { file, size }),
        1 => (file(), 0u64..16, 1u64..4)
            .prop_map(|(file, page, pages)| Op::Punch { file, page, pages }),
        1 => (file(), 0u8..3, 1usize..64, any::<u8>())
            .prop_map(|(file, name, len, byte)| Op::SetXattr { file, name, len, byte }),
        2 => Just(Op::Sync),
        2 => Just(Op::Fsync),
        3 => Just(Op::Compact),
        2 => Just(Op::Remount),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let file = || 0usize..16;
    let length = || prop_oneof![1usize..64, Just(PAGE as usize), 1usize..(3 * PAGE as usize)];
    prop_oneof![
        3 => name().prop_map(Op::Create),
        1 => name().prop_map(Op::Symlink),
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
        1 => (file(), 0u8..3, prop_oneof![1usize..64, 1024usize..8192], any::<u8>())
            .prop_map(|(file, name, len, byte)| Op::SetXattr { file, name, len, byte }),
        1 => (file(), 0u8..3).prop_map(|(file, name)| Op::RemoveXattr { file, name }),
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
    /// What each directory's names link.
    names: BTreeMap<(u64, usize), Entry>,
    files: BTreeMap<u64, Data>,
    /// What's held open: what no name links, of these, is in the graveyard.
    open: BTreeSet<u64>,
    /// Of those, the tmpfiles no name has linked yet, which alone a VFS lets a link name with none
    /// left (`I_LINKABLE`).
    linkable: BTreeSet<u64>,
    /// The unlinks queued that may not be done yet.
    queued: Vec<Queued>,
    /// The files those leave nothing of, as a cut before them would find them.
    gone: BTreeMap<u64, Data>,
}

impl Model {
    fn new(root: u64) -> Self {
        Self {
            dirs: vec![root],
            names: BTreeMap::new(),
            files: BTreeMap::new(),
            open: BTreeSet::new(),
            linkable: BTreeSet::new(),
            queued: Vec::new(),
            gone: BTreeMap::new(),
        }
    }

    /// The ino of the `index`th file there is, if there is any: one open may have no names.
    fn file(&self, index: usize) -> Option<u64> {
        let files: Vec<u64> = self.files.keys().copied().collect();
        (!files.is_empty()).then(|| files[index % files.len()])
    }

    /// Which directory `name` is in, and which of its names it is.
    fn place(&self, name: usize) -> (u64, usize) {
        (self.dirs[name / NAMES % self.dirs.len()], name % NAMES)
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

    /// How many objects a volume with this in it has.
    fn objects(&self) -> u64 {
        let inos = self.names.values().map(|entry| entry.ino()).chain(self.open.iter().copied());
        let inos: BTreeSet<u64> = inos.chain(self.files.keys().copied()).collect();
        BARE_OBJECTS + inos.len() as u64
    }

    /// What a power cut leaves of this: what's open closes, and what no name links goes.
    fn cut(mut self) -> Self {
        let linked: BTreeSet<u64> = self.names.values().map(|entry| entry.ino()).collect();
        self.files.retain(|ino, _| linked.contains(ino));
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
}

impl Setting {
    fn calm(seed: u64, blocks: u64) -> Self {
        let (awkward, fsck_each, every_cut, failing) = (false, false, false, false);
        let (cuts_one_in, workers, batched_graveyard) = (CUTS_ONE_IN, false, false);
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
        Self { batched_graveyard: self.seed % 2 == 0, ..self }
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

    /// What opens a filesystem here.
    fn builder(&self) -> FxFilesystemBuilder {
        let builder = FxFilesystemBuilder::new().fsck_after_every_transaction(self.fsck_each);
        let builder = match self.batched_graveyard {
            true => builder.graveyard_batch_delay(BATCHES_APART),
            false => builder,
        };
        if !self.every_cut {
            return builder;
        }
        builder.journal_options(JournalOptions {
            reclaim_size: SHORT_RECLAIM,
            chunk_size: SHORT_RECLAIM,
            ..Default::default()
        })
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
        let done = run(self, ops, tally);
        fasync::LocalExecutorBuilder::new().build().run_singlethreaded(done)
    }

    /// Checks that `ops` hold here.
    fn holds(self, ops: Vec<Op>) {
        assert_eq!(self.run(ops, Rc::default()), Ok(()), "{self:?}");
    }
}

/// As the host does, but for the tasks that apply queued unlinks: a clock wakes them, and a seed
/// wouldn't say what a sequence does. What waits for what's queued applies it, unless `workers`.
struct Seeded {
    workers: bool,
}

impl Platform for Seeded {
    const KEEPS_DIRECTORY_TIMES: bool = false;

    type Durable = ();

    fn durable(&self) {}

    fn spawn_urgent(&self, future: impl Future<Output = ()> + Send + 'static) {
        if self.workers {
            fasync::Task::spawn(future).detach();
        }
    }
}

/// A filesystem and its model.
struct World {
    setting: Setting,
    filesystem: fxfs::filesystem::OpenFxFilesystem,
    // Boxed: the tasks applying queued unlinks refer to it as a world moves.
    fs: Box<Fxfs<Seeded>>,
    root: u64,
    model: Model,
    tally: Rc<Tally>,
}

/// Runs fsck, and says what it found.
async fn checked(filesystem: &fxfs::filesystem::OpenFxFilesystem) -> Result<(), String> {
    let issues = Arc::new(std::sync::Mutex::new(Vec::new()));
    let found = issues.clone();
    let on_error = Box::new(move |issue: &fxfs::fsck::errors::FsckIssue| {
        found.lock().unwrap().push(format!("{issue:?}"));
    });
    let options = FsckOptions { on_error, fail_on_warning: true, ..FsckOptions::default() };
    let done = fsck_with_options((**filesystem).clone(), &options).await;
    done.map(drop).map_err(|e| format!("fsck: {e:#}: {:?}", issues.lock().unwrap()))
}

fn name_of(name: usize) -> Vec<u8> {
    vec![b'a' + name as u8]
}

/// The bytes of `run`, a page a fill writes.
fn written(run: &Run) -> &[u8] {
    run.bytes.as_ref().expect("written")
}

fn xattr_of(name: u8) -> Vec<u8> {
    vec![b'x', b'a' + name]
}

impl World {
    async fn new(
        setting: Setting,
        crashes: &Arc<Crashes>,
        faults: &Arc<Faults>,
        tally: Rc<Tally>,
    ) -> Self {
        let device = DeviceHolder::new(setting.device(crashes, faults));
        let filesystem = setting.builder().format(true).open(device).await.expect("formatting");
        let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
        let volume =
            volumes.new_volume("test", NewChildStoreOptions::default()).await.expect("the volume");
        let root = volume.root_directory_object_id();
        let platform = Seeded { workers: setting.workers };
        let fs = Box::new(Fxfs::new((*filesystem).clone(), volume, false, platform));
        Self { setting, filesystem, fs, root, model: Model::new(root), tally }
    }

    /// Mounts what `device` holds, or replays it: what `model` should say.
    async fn mount(
        setting: Setting,
        device: DeviceHolder,
        root: u64,
        model: Model,
        tally: Rc<Tally>,
    ) -> Result<Self, String> {
        let filesystem = setting.builder().open(device).await;
        let filesystem = filesystem.map_err(|e| format!("opening: {e:#}"))?;
        let volumes = root_volume((*filesystem).clone()).await.map_err(|e| format!("{e:#}"))?;
        let volume = volumes.volume("test", StoreOptions::default()).await;
        let volume = volume.map_err(|e| format!("the volume: {e:#}"))?;
        drop(volumes);
        let platform = Seeded { workers: setting.workers };
        let fs = Box::new(Fxfs::new((*filesystem).clone(), volume, false, platform));
        Ok(Self { setting, filesystem, fs, root, model, tally })
    }

    /// Unmounts, as a guest's `umount` does, leaving the device.
    async fn unmount(self) -> Result<DeviceHolder, String> {
        let Self { filesystem, fs, .. } = self;
        fs.stop_delayed().await.map_err(|e| format!("stopping: {e:#}"))?;
        drop(fs);
        filesystem.close().await.map_err(|e| format!("closing: {e:#}"))?;
        Ok(filesystem.take_device().await)
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
            let times = SetAttr { valid: SET_MTIME, mtime_ns: 1, ..SetAttr::default() };
            let written = self.fs.write_inode(ino, &times).await;
            self.unless_full(written, "a directory's times")?;
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

    /// Checks file `ino` against `model`: its size, its links, its extended attributes, and what
    /// it reads back: all of it, as far as WHOLLY_READ, then around its pages and at its end.
    async fn check(&self, model: &Model, ino: u64) -> Result<(), String> {
        let data = &model.files[&ino];
        // As a lookup does, for the links the kernel takes from it.
        self.fs.drain(&[ino]).await.map_err(|e| format!("draining: {e:#}"))?;
        let attr = self.fs.attr(ino).await.map_err(|e| format!("attr of {ino}: {e:#}"))?;
        if attr.size != data.size {
            return Err(format!("{ino} is {} bytes, not {}", attr.size, data.size));
        }
        // In the graveyard, its record counts one, which the kernel doesn't.
        let links = model.links(ino).max(1);
        if attr.nlink != links {
            return Err(format!("{ino} has {} links, not {links}", attr.nlink));
        }
        let mut names = self.fs.list_xattrs(ino).await.map_err(|e| format!("xattrs: {e:#}"))?;
        names.sort();
        let expected: Vec<Vec<u8>> = data.xattrs.keys().map(|name| xattr_of(*name)).collect();
        if names != expected {
            return Err(format!("{ino} has the xattrs {names:?}, not {expected:?}"));
        }
        for (name, value) in &data.xattrs {
            let got = self.fs.get_xattr(ino, &xattr_of(*name)).await;
            let got = got.map_err(|e| format!("xattr {name} of {ino}: {e:#}"))?;
            if got != *value {
                return Err(format!("xattr {name} of {ino} isn't what was set"));
            }
        }
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
        let got = self.fs.read(ino, at, &mut read).await;
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

    /// Checks what the directory `dir` holds against `model`, and what its names link: their
    /// types, each by lookup too, its links, and what a symlink says.
    async fn holds_names(&self, model: &Model, dir: u64) -> Result<(), String> {
        let mut found = Vec::new();
        let listed = self.fs.readdir(dir, ReaddirStart::Index(0), |name, ino, kind| {
            found.push((name.as_bytes().to_vec(), ino, kind));
            true
        });
        listed.await.map_err(|e| format!("readdir of {dir}: {e:#}"))?;
        found.sort();
        let names: Vec<(Vec<u8>, u64, u32)> = model
            .names_in(dir)
            .map(|(name, entry)| (name_of(name), entry.ino(), entry.kind()))
            .collect();
        if found != names {
            return Err(format!("{dir} holds {found:?}, not {names:?}"));
        }
        let mut sub_dirs = 0;
        for (name, entry) in model.names_in(dir) {
            let attr = self.fs.lookup(dir, &name_of(name)).await;
            let attr = attr.map_err(|e| format!("lookup of {name} in {dir}: {e:#}"))?;
            if attr.ino != entry.ino() {
                return Err(format!("{name} in {dir} is {}, not {entry:?}", attr.ino));
            }
            match entry {
                Entry::File(_) => {}
                Entry::Symlink(ino) => {
                    let target = self.fs.readlink(ino).await;
                    let target = target.map_err(|e| format!("readlink of {ino}: {e:#}"))?;
                    if target != SYMLINK_TARGET {
                        return Err(format!("{ino} says {target:?}"));
                    }
                }
                Entry::Dir(_) => sub_dirs += 1,
            }
        }
        let attr = self.fs.attr(dir).await.map_err(|e| format!("attr of {dir}: {e:#}"))?;
        if attr.nlink != 2 + sub_dirs {
            return Err(format!("{dir} has {} links with {sub_dirs} subdirectories", attr.nlink));
        }
        Ok(())
    }

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

    /// Checks each file.
    async fn check_files(&self) -> Result<(), String> {
        for ino in self.model.files.keys() {
            self.check(&self.model, *ino).await?;
        }
        Ok(())
    }

    /// Checks everything, then fsck.
    async fn check_all(&self) -> Result<(), String> {
        self.holds(&self.model).await?;
        self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
        self.holds_nothing_else(&self.model).await?;
        checked(&self.filesystem).await
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
            self.check(&self.model, ino).await?;
        }
        Ok(())
    }

    /// Checks that the device holds no extent Fxfs doesn't know, a sync having freed what's dead,
    /// as what a mount reaped of a compaction cut short is: what a crash orphaned, freed by a
    /// mount, takes room Fxfs doesn't count otherwise.
    async fn holds_no_orphans(&self) -> Result<(), String> {
        // A sync starts what it lets die being freed, and what dies meanwhile waits for the next.
        let allocator = self.filesystem.allocator();
        loop {
            self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
            allocator.freed().await.map_err(|e| format!("freeing: {e:#}"))?;
            if !allocator.has_dead_extents() {
                break;
            }
        }
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

    /// Unmounts and mounts again, as a guest's `umount` and `mount` do: nothing stays open, or
    /// it wouldn't.
    async fn remount(mut self) -> Result<Self, String> {
        self.model.open.clone().into_iter().for_each(|ino| self.close(ino));
        let (setting, root) = (self.setting, self.root);
        let (model, tally) = (self.model.clone(), self.tally.clone());
        let device = self.unmount().await?;
        device.reopen(false);
        let mut world = Self::mount(setting, device, root, model, tally).await?;
        world.settle();
        Ok(world)
    }

    /// Checks what a crash leaves now, this filesystem going on: a copy of the device, as of a
    /// sync, replayed, holds what the model does, and passes fsck.
    async fn crash(&self) -> Result<(), String> {
        self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
        let device = self.filesystem.device().snapshot().map_err(|e| format!("snapshot: {e:#}"))?;
        let (model, tally) = (self.model.clone().cut(), self.tally.clone());
        let replayed = Self::mount(self.setting, device, self.root, model, tally).await?;
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
            let done = self.act(op).await;
            self.settle();
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

    /// Removes `name` now, a directory if it's empty.
    async fn unlink(&mut self, name: usize) -> Result<(), String> {
        let entry = self.model.entry(name).expect("looked up");
        let (dir, at) = self.model.place(name);
        let gone = self.fs.unlink(dir, &name_of(at), entry.ino()).await;
        if self.is_refused_as_not_empty(Some(entry), &gone)? {
            return Ok(());
        }
        gone.map_err(|e| format!("unlink: {e:#}"))?;
        self.model.names.remove(&(dir, at));
        self.forget(entry).await
    }

    /// Whether `done` was refused for replacing or removing `entry`, a directory with names in
    /// it, as it must be.
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
            Err(error) => Err(format!("a directory with names in it: {error:#}")),
            Ok(_) => Err(format!("{ino} went with names in it")),
        }
    }

    /// Does `op`, any but a remount, and checks what it changed.
    async fn act(&mut self, op: &Op) -> Result<(), String> {
        match *op {
            Op::Create(name) if self.model.entry(name).is_none() => {
                let (dir, at) = self.model.place(name);
                let made = self.fs.create(dir, &name_of(at), FILE, 0, 0, 0, &[]).await;
                let Some([attr, _]) = self.unless_full(made, "create")? else { return Ok(()) };
                if attr.ctime_ns < attr.mtime_ns {
                    return Err(format!("a new file's ctime is before its mtime: {attr:?}"));
                }
                self.model.names.insert((dir, at), Entry::File(attr.ino));
                self.model.files.insert(attr.ino, Data::default());
            }
            Op::Symlink(name) if self.model.entry(name).is_none() => {
                let (dir, at) = self.model.place(name);
                let made = self.fs.symlink(dir, &name_of(at), SYMLINK_TARGET, 0, 0).await;
                let Some([attr, _]) = self.unless_full(made, "symlink")? else { return Ok(()) };
                if attr.ctime_ns < attr.mtime_ns {
                    return Err(format!("a new symlink's ctime is before its mtime: {attr:?}"));
                }
                self.model.names.insert((dir, at), Entry::Symlink(attr.ino));
            }
            Op::Mkdir(name) if self.model.entry(name).is_none() => {
                let (dir, at) = self.model.place(name);
                let made = self.fs.create(dir, &name_of(at), DIR, 0, 0, 0, &[]).await;
                let Some([attr, _]) = self.unless_full(made, "mkdir")? else { return Ok(()) };
                self.model.names.insert((dir, at), Entry::Dir(attr.ino));
                self.model.dirs.push(attr.ino);
            }
            Op::Link(file, name) if self.model.entry(name).is_none() => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                let unnamed = self.model.links(ino) == 0;
                if unnamed && !self.model.linkable.contains(&ino) {
                    return Ok(());
                }
                let (dir, at) = self.model.place(name);
                let linked = self.fs.link(ino, dir, &name_of(at), unnamed).await;
                let Some([attr, _]) = self.unless_full(linked, "link")? else { return Ok(()) };
                self.model.linkable.remove(&ino);
                self.model.names.insert((dir, at), Entry::File(ino));
                if attr.nlink != self.model.links(ino) {
                    let links = self.model.links(ino);
                    return Err(format!("{ino} has {} links, not {links}", attr.nlink));
                }
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
                let (a, b) = (self.model.entry(a), self.model.entry(b));
                self.model.names.insert(first, b.expect("looked up"));
                self.model.names.insert(second, a.expect("looked up"));
            }
            Op::Tmpfile(name) => {
                let dir = self.model.place(name).0;
                let made = self.fs.tmpfile(dir, FILE, 0, 0).await;
                let Some(attr) = self.unless_full(made, "tmpfile")? else { return Ok(()) };
                self.model.files.insert(attr.ino, Data::default());
                self.model.open.insert(attr.ino);
                self.model.linkable.insert(attr.ino);
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
                self.check(&self.model, ino).await?;
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
            Op::Read { file, at, len } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                self.compare(&self.model.files[&ino], ino, at, len).await?;
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
                self.check(&self.model, ino).await?;
            }
            Op::Punch { file, page, pages } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                let data = &self.model.files[&ino];
                let (end, extents) = {
                    let end = (page + pages).min(data.size / PAGE);
                    (end, data.extents_in(page..end))
                };
                if page < end {
                    let journaled = self.filesystem.journal().file_offset();
                    let punched = self.fs.punch(ino, page * PAGE..end * PAGE).await;
                    self.journaled_for(extents, journaled)?;
                    if self.unless_full(punched, "punch")?.is_some() {
                        self.model.files.get_mut(&ino).expect("a file").punch(page..end);
                    }
                    self.check(&self.model, ino).await?;
                }
            }
            Op::SetXattr { file, name, len, byte } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                let value = vec![byte; len];
                let mode = SetExtendedAttributeMode::Set;
                let set = self.fs.set_xattr(ino, &xattr_of(name), &value, mode).await;
                set.map_err(|e| format!("set_xattr: {e:#}"))?;
                self.write_xattrs(ino, name, Some(value)).await?;
            }
            Op::RemoveXattr { file, name } => {
                let Some(ino) = self.model.file(file) else { return Ok(()) };
                if self.model.files[&ino].xattrs.contains_key(&name) {
                    let removed = self.fs.remove_xattr(ino, &xattr_of(name)).await;
                    removed.map_err(|e| format!("remove_xattr: {e:#}"))?;
                    self.write_xattrs(ino, name, None).await?;
                }
            }
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

    /// `model`, had `op`, which failed, worked: this filesystem says what a name it made links.
    async fn had_it_worked(&self, model: &Model, op: &Op) -> Option<Model> {
        let mut model = model.clone();
        if let Some(plan) = op.plan(&model) {
            model.files.get_mut(&plan.ino)?.follow(&plan);
            return Some(model);
        }
        match *op {
            Op::Create(name) | Op::Symlink(name) | Op::Mkdir(name) => {
                let (dir, at) = model.place(name);
                let ino = self.fs.lookup(dir, &name_of(at)).await.ok()?.ino;
                let entry = match op {
                    Op::Create(_) => Entry::File(ino),
                    Op::Symlink(_) => Entry::Symlink(ino),
                    _ => Entry::Dir(ino),
                };
                model.names.insert((dir, at), entry);
                match entry {
                    Entry::File(_) => drop(model.files.insert(ino, Data::default())),
                    Entry::Symlink(_) => {}
                    Entry::Dir(_) => model.dirs.push(ino),
                }
            }
            Op::Link(file, name) => {
                let ino = model.file(file)?;
                model.names.insert(model.place(name), Entry::File(ino));
            }
            Op::Unlink(name) | Op::UnlinkLater(name) => {
                let entry = model.names.remove(&model.place(name))?;
                model.dirs.retain(|dir| *dir != entry.ino());
                if model.links(entry.ino()) == 0 {
                    model.files.remove(&entry.ino());
                }
            }
            Op::Rename(from, to) => {
                let entry = model.names.remove(&model.place(from))?;
                if let Some(replaced) = model.names.insert(model.place(to), entry) {
                    model.dirs.retain(|dir| *dir != replaced.ino());
                    if model.links(replaced.ino()) == 0 {
                        model.files.remove(&replaced.ino());
                    }
                }
            }
            Op::Exchange(a, b) => {
                let (first, second) = (model.entry(a)?, model.entry(b)?);
                model.names.insert(model.place(a), second);
                model.names.insert(model.place(b), first);
            }
            Op::Write { file, at, len, byte } => {
                let ino = model.file(file)?;
                model.files.get_mut(&ino)?.write(at.min(LARGEST - len as u64), &vec![byte; len]);
            }
            Op::Truncate { file, size } => {
                let ino = model.file(file)?;
                model.files.get_mut(&ino)?.truncate(size);
            }
            Op::Punch { file, page, pages } => {
                let ino = model.file(file)?;
                let data = model.files.get_mut(&ino)?;
                data.punch(page..(page + pages).min(data.size / PAGE));
            }
            Op::SetXattr { file, name, len, byte } => {
                let ino = model.file(file)?;
                model.files.get_mut(&ino)?.xattrs.insert(name, vec![byte; len]);
            }
            Op::RemoveXattr { file, name } => {
                let ino = model.file(file)?;
                model.files.get_mut(&ino)?.xattrs.remove(&name);
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
        if self.unless_full(done, "a plan")?.is_some() {
            data.follow(plan);
        } else {
            data = self.stopped_short(data, plan).await?;
        }
        self.model.files.insert(plan.ino, data);
        self.check(&self.model, plan.ino).await
    }

    /// Writes back the change of `ino`'s extended attribute `name` to `value`, as the kernel's
    /// writeback of its inode does. One the filesystem has no room for it drops, as the model does.
    async fn write_xattrs(
        &mut self,
        ino: u64,
        name: u8,
        value: Option<Vec<u8>>,
    ) -> Result<(), String> {
        let written = self.fs.write_inode(ino, &SetAttr::default()).await;
        if self.unless_full(written, "write_inode")?.is_none() {
            self.fs.xattrs.lock().unwrap().remove(&ino);
        } else {
            let xattrs = &mut self.model.files.get_mut(&ino).expect("a file").xattrs;
            match value {
                Some(value) => xattrs.insert(name, value),
                None => xattrs.remove(&name),
            };
        }
        self.check(&self.model, ino).await
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
    async fn stopped_short(&self, world: &World, state: usize, ops: &[Op]) -> Option<Model> {
        let mut before = self.states[state.checked_sub(1)?].clone();
        let plan = ops.get(state - 1)?.plan(&before)?;
        let data = before.files.remove(&plan.ino)?;
        let short = world.stopped_short(data, &plan).await.ok()?;
        before.files.insert(plan.ino, short);
        Some(before)
    }

    /// Checks what power cut `cut` of a sequence, during operation `op` of `ops`, leaves on
    /// `device`: it replays, passes fsck, is the model after one of the operations since the last
    /// that was durable, and goes on with what the sequence did next.
    async fn check_cut(
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
            World::mount(setting, device, root, Model::new(root), tally.clone()).await?;
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
        if self.states[first..=last].iter().any(|state| !state.unlinked().is_empty()) {
            tally.cuts_of_the_unlinked.set(tally.cuts_of_the_unlinked.get() + 1);
        }
        for (_, device) in second_cuts.take() {
            tally.second_cuts.set(tally.second_cuts.get() + 1);
            let device = DeviceHolder::new(device);
            let again = World::mount(setting, device, root, held.clone(), tally.clone()).await;
            let again = again.map_err(|e| format!("cut during its replay: {e}"))?;
            again.check_all().await.map_err(|e| format!("cut during its replay: {e}"))?;
            again.unmount().await?;
        }
        world.model = held;
        world.grow_to_pages().await.map_err(|e| format!("past a file's end: {e}"))?;
        for next in ops[op..].iter().take(GOES_ON_FOR) {
            world = world.step(next).await.map_err(|e| format!("going on with {next:?}: {e}"))?;
        }
        world.check_all().await.map_err(|e| format!("gone on: {e}"))?;
        world.unmount().await.map(drop)
    }
}

async fn run(setting: Setting, ops: Vec<Op>, tally: Rc<Tally>) -> Result<(), String> {
    let crashes = setting.crashes();
    let faults = Faults::new(setting.seed);
    let world = World::new(setting, &crashes, &faults, tally.clone()).await;
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
        let checked = past.check_cut((cut, mark as usize), &ops, device, root, tally.clone()).await;
        checked.map_err(|e| format!("cut {cut}, during op {mark}: {e}"))?;
    }
    Ok(())
}

/// Checks what a sequence left, across a remount, and unmounts it.
async fn finish(world: World) -> Result<(), String> {
    world.check_all().await.map_err(|e| format!("at the end: {e}"))?;
    let world = world.remount().await.map_err(|e| format!("at the end: {e}"))?;
    world.check_all().await.map_err(|e| format!("remounted at the end: {e}"))?;
    world.unmount().await.map(drop)
}

/// Aborts, saying the sequence, if one takes longer than `limit`: each is sent here as it starts,
/// and this ends with the sender.
fn watch_sequences(limit: Duration) -> mpsc::Sender<String> {
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
    let config = Config { cases, failure_persistence: None, ..Config::default() };
    let room = prop_oneof![Just(ROOMY), Just(TIGHT)];
    let sequences = (any::<u64>(), room, proptest::collection::vec(op(), 1..48));
    let tally = Rc::new(Tally::default());
    let watched = watch_sequences(if fsck_each { 30 * SEQUENCE_LIMIT } else { SEQUENCE_LIMIT });
    TestRunner::new(config)
        .run(&sequences, |(seed, blocks, ops)| {
            watched.send(format!("{seed}, {blocks}, {ops:?}")).expect("the watchdog stopped");
            let setting = Setting::awkward(seed, blocks).with_graveyard_of_seed();
            let setting = setting.with_fsck_each(fsck_each).with_workers(workers);
            prop_assert_eq!(setting.run(ops, tally.clone()), Ok(()));
            Ok(())
        })
        .unwrap();
    let (cuts, second_cuts) = (tally.cuts.get(), tally.second_cuts.get());
    let unlinked = tally.cuts_of_the_unlinked.get();
    println!(
        "{cuts} power cuts, {second_cuts} of replays, {unlinked} of files open with no name, {} \
         refused",
        tally.refused.get()
    );
    assert!(cuts > 0, "no power cut");
    assert!(second_cuts > 0, "no power cut of a replay");
    assert!(unlinked > 0, "no power cut of a file open with no name");
    assert!(tally.refused.get() > 0, "no device filled");
}

/// A panic of any task since the last call, which a sequence can't see of one it doesn't wait for.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Sequences whose device's requests fail, each ended by the first its operation doesn't survive:
/// a filesystem that hit an error remounts, passes fsck, and holds what was durable, or what the
/// operation that failed got as far as: nothing panics, and no sync that said it was done isn't.
/// FXDAX_MODEL_FAILURES asks for more.
#[test]
fn a_device_that_fails_loses_nothing_durable() {
    let cases = std::env::var("FXDAX_MODEL_FAILURES").map_or(24, |cases| cases.parse().unwrap());
    let config = Config { cases, failure_persistence: None, ..Config::default() };
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
        let mut world = World::new(setting, &crashes, &faults, Rc::default()).await;
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
            let world = World::mount(setting, device, root, synced.clone(), tally).await.unwrap();
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
    let config = Config { cases, failure_persistence: None, ..Config::default() };
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
        Op::Exchange(3, 1),
        Op::Compact,
        Op::Rename(1, 4),
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
