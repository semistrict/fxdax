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

use super::tests::Tested;
use super::*;
use fxfs::filesystem::FxFilesystemBuilder;
use fxfs::fsck::{FsckOptions, fsck_with_options};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Duration;
use storage_device::DeviceHolder;
use storage_device::fake_device::{Crashes, FakeDevice};

const PAGE: u64 = 4096;
const FILE: u32 = S_IFREG | 0o644;
/// The largest size a file may have.
const LARGEST: u64 = i64::MAX as u64;
/// The names a sequence uses: few, so that they meet.
const NAMES: usize = 6;
/// The room a sequence has: enough, or less than a few of its fills want.
const ROOMY: u64 = 32768;
const TIGHT: u64 = 2048;
/// How much of a file is read whole, holes and all: a file may be as large as a file may be.
const WHOLLY_READ: u64 = 64 << 20;
/// A power cut is recorded at one barrier in this many.
const CUTS_ONE_IN: u32 = 3;
/// A power cut of a replay is recorded at one barrier in this many.
const SECOND_CUTS_ONE_IN: u32 = 2;
/// How long a sequence may take. A filesystem that spins forever is a failure proptest can't see,
/// waiting for the sequence to end: a graveyard walking every attribute ID was found only by the
/// time a run took.
const SEQUENCE_LIMIT: Duration = Duration::from_secs(120);
/// The operations a filesystem that survived a power cut goes on with.
const GOES_ON_FOR: usize = 4;

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

    /// Makes `page` what a plan says, the file reaching it.
    fn put(&mut self, page: u64, bytes: &Option<Arc<Vec<u8>>>) {
        match bytes {
            Some(bytes) => self.pages.insert(page, bytes.clone()),
            None => self.pages.remove(&page),
        };
        self.size = self.size.max((page + 1) * PAGE);
    }

    fn punch(&mut self, pages: std::ops::Range<u64>) {
        self.pages.retain(|page, _| !pages.contains(page));
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

#[derive(Clone, Debug)]
enum Entry {
    File(u64),
    Symlink(u64),
}

/// What an operation of many transactions makes of a file's pages, each in turn, a page or a hole:
/// so one stopped short, by a full filesystem or a power cut, has made its first, and no others.
struct Plan {
    ino: u64,
    pages: Vec<(u64, Option<Arc<Vec<u8>>>)>,
}

#[derive(Clone, Debug)]
enum Op {
    Create(usize),
    Symlink(usize),
    Link(usize, usize),
    Unlink(usize),
    Rename(usize, usize),
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
                let bytes = Arc::new(vec![byte; PAGE as usize]);
                let pages = (page..page + pages).step_by(stride as usize);
                let pages = pages.map(|page| (page, Some(bytes.clone()))).collect();
                Some(Plan { ino: model.file(file)?, pages })
            }
            Op::Clone { from, to, page, pages, to_page } => {
                let (src, dst) = (model.file(from)?, model.file(to)?);
                let end = (page + pages).min(model.files[&src].size / PAGE);
                let to_end = to_page + end.saturating_sub(page);
                if page >= end || src == dst && page < to_end && to_page < end {
                    return None;
                }
                let source = &model.files[&src].pages;
                let pages = (page..end).map(|at| (to_page + at - page, source.get(&at).cloned()));
                Some(Plan { ino: dst, pages: pages.collect() })
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

fn op() -> impl Strategy<Value = Op> {
    let name = || 0..NAMES;
    let file = || 0usize..16;
    let length = || prop_oneof![1usize..64, Just(PAGE as usize), 1usize..(3 * PAGE as usize)];
    prop_oneof![
        3 => name().prop_map(Op::Create),
        1 => name().prop_map(Op::Symlink),
        1 => (file(), name()).prop_map(|(file, name)| Op::Link(file, name)),
        2 => name().prop_map(Op::Unlink),
        1 => (name(), name()).prop_map(|(from, to)| Op::Rename(from, to)),
        6 => (file(), offset(), length(), any::<u8>())
            .prop_map(|(file, at, len, byte)| Op::Write { file, at, len, byte }),
        3 => (file(), 0u64..64, fill(), 1u64..=2, any::<u8>()).prop_map(
            |(file, page, pages, stride, byte)| Op::Fill { file, page, pages, stride, byte }
        ),
        2 => (file(), offset(), length()).prop_map(|(file, at, len)| Op::Read { file, at, len }),
        3 => (file(), offset()).prop_map(|(file, size)| Op::Truncate { file, size }),
        2 => (file(), 0u64..32, 1u64..8)
            .prop_map(|(file, page, pages)| Op::Punch { file, page, pages }),
        2 => (file(), file(), 0u64..16, prop_oneof![1u64..8, 64u64..4096], 0u64..32).prop_map(
            |(from, to, page, pages, to_page)| Op::Clone { from, to, page, pages, to_page }
        ),
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

/// What a sequence's files should be.
#[derive(Clone, Debug, Default)]
struct Model {
    names: BTreeMap<usize, Entry>,
    files: BTreeMap<u64, Data>,
}

impl Model {
    /// The ino of the `index`th file a name links, if any is.
    fn file(&self, index: usize) -> Option<u64> {
        let files: Vec<u64> = self.files.keys().copied().collect();
        (!files.is_empty()).then(|| files[index % files.len()])
    }

    fn links(&self, ino: u64) -> u32 {
        (self.names.values())
            .filter(|entry| matches!(entry, Entry::File(of) | Entry::Symlink(of) if *of == ino))
            .count() as u32
    }
}

/// What a run of sequences got to, which the test says it got to: one that never cut the power, or
/// never filled a device, would pass without having looked.
#[derive(Default)]
struct Tally {
    cuts: Cell<u64>,
    second_cuts: Cell<u64>,
    refused: Cell<u64>,
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
}

impl Setting {
    fn calm(seed: u64, blocks: u64) -> Self {
        Self { seed, blocks, cuts_one_in: CUTS_ONE_IN, awkward: false, fsck_each: false }
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

    /// What opens a filesystem here.
    fn builder(&self) -> FxFilesystemBuilder {
        FxFilesystemBuilder::new().fsck_after_every_transaction(self.fsck_each)
    }

    /// The seed of what cuts the replay of power cut `cut`, during operation `op`: of those of
    /// sequences found on a calm device, as it was then.
    fn seed_of_cut(&self, cut: usize, op: usize) -> u64 {
        let seed = op as u64 * 31 + cut as u64 + 1;
        if self.awkward { seed.wrapping_mul(self.seed | 1) } else { seed }
    }

    /// The device, recording `crashes`.
    fn device(&self, crashes: &Arc<Crashes>) -> FakeDevice {
        let device = FakeDevice::new(self.blocks, 4096);
        let device = if self.awkward { device.awkward(self.seed) } else { device.calm() };
        device.record_crashes(crashes.clone())
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

/// A filesystem and its model.
struct World {
    setting: Setting,
    filesystem: fxfs::filesystem::OpenFxFilesystem,
    fs: Fxfs<Tested>,
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
    let options = FsckOptions { on_error, ..FsckOptions::default() };
    let done = fsck_with_options((**filesystem).clone(), &options).await;
    done.map(drop).map_err(|e| format!("fsck: {e:#}: {:?}", issues.lock().unwrap()))
}

fn name_of(name: usize) -> Vec<u8> {
    vec![b'a' + name as u8]
}

fn xattr_of(name: u8) -> Vec<u8> {
    vec![b'x', b'a' + name]
}

impl World {
    async fn new(setting: Setting, crashes: &Arc<Crashes>, tally: Rc<Tally>) -> Self {
        let device = DeviceHolder::new(setting.device(crashes));
        let filesystem = setting.builder().format(true).open(device).await.expect("formatting");
        let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
        let volume =
            volumes.new_volume("test", NewChildStoreOptions::default()).await.expect("the volume");
        let root = volume.root_directory_object_id();
        let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
        Self { setting, filesystem, fs, root, model: Model::default(), tally }
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
        let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
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

    /// What's left of `entry`, its name gone: a file no name links goes as the kernel lets it go.
    fn forget(&mut self, entry: Entry) {
        let (Entry::File(ino) | Entry::Symlink(ino)) = entry;
        if self.model.links(ino) == 0 {
            self.model.files.remove(&ino);
            self.fs.evict(ino, true);
        }
    }

    /// Checks file `ino` against `model`: its size, its links, its extended attributes, and what
    /// it reads back: all of it, as far as WHOLLY_READ, then around its pages and at its end.
    async fn check(&self, model: &Model, ino: u64) -> Result<(), String> {
        let data = &model.files[&ino];
        let attr = self.fs.attr(ino).await.map_err(|e| format!("attr of {ino}: {e:#}"))?;
        if attr.size != data.size {
            return Err(format!("{ino} is {} bytes, not {}", attr.size, data.size));
        }
        if attr.nlink != model.links(ino) {
            return Err(format!("{ino} has {} links, not {}", attr.nlink, model.links(ino)));
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
        for (page, bytes) in &plan.pages {
            let at = page * PAGE;
            let got = self.fs.read(ino, at, &mut read).await;
            let got = got.map_err(|e| format!("read at {at} of {ino}: {e:#}"))?;
            let expected = bytes.as_ref().map_or(&hole, |bytes| &**bytes);
            if at + PAGE > size || got != read.len() || read != *expected {
                break;
            }
            data.put(*page, bytes);
        }
        Ok(data)
    }

    /// Checks the names there are and each file against `model`.
    async fn holds(&self, model: &Model) -> Result<(), String> {
        let mut found = Vec::new();
        let listed = self.fs.readdir(self.root, ReaddirStart::Index(0), |name, _, _| {
            found.push(name.as_bytes().to_vec());
            true
        });
        listed.await.map_err(|e| format!("readdir: {e:#}"))?;
        found.sort();
        let names: Vec<Vec<u8>> = model.names.keys().map(|name| name_of(*name)).collect();
        if found != names {
            return Err(format!("the directory holds {found:?}, not {names:?}"));
        }
        for (name, entry) in &model.names {
            let (Entry::File(ino) | Entry::Symlink(ino)) = entry;
            let attr = self.fs.lookup(self.root, &name_of(*name)).await;
            let attr = attr.map_err(|e| format!("lookup of {name}: {e:#}"))?;
            if attr.ino != *ino {
                return Err(format!("{name} is {}, not {ino}", attr.ino));
            }
        }
        for ino in model.files.keys() {
            self.check(model, *ino).await?;
        }
        Ok(())
    }

    /// Checks everything, then fsck.
    async fn check_all(&self) -> Result<(), String> {
        self.holds(&self.model).await?;
        self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
        checked(&self.filesystem).await
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

    /// Unmounts and mounts again, as a guest's `umount` and `mount` do.
    async fn remount(self) -> Result<Self, String> {
        let (setting, root) = (self.setting, self.root);
        let (model, tally) = (self.model.clone(), self.tally.clone());
        let device = self.unmount().await?;
        device.reopen(false);
        Self::mount(setting, device, root, model, tally).await
    }

    /// Checks what a crash leaves now, this filesystem going on: a copy of the device, as of a
    /// sync, replayed, holds what the model does, and passes fsck.
    async fn crash(&self) -> Result<(), String> {
        self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
        let device = self.filesystem.device().snapshot().map_err(|e| format!("snapshot: {e:#}"))?;
        let (model, tally) = (self.model.clone(), self.tally.clone());
        let replayed = Self::mount(self.setting, device, self.root, model, tally).await?;
        replayed.check_all().await?;
        replayed.unmount().await.map(drop)
    }

    /// Does `op`, if it can be done of what there is, and checks what it changed.
    async fn step(mut self, op: &Op) -> Result<Self, String> {
        let root = self.root;
        match *op {
            Op::Create(name) if !self.model.names.contains_key(&name) => {
                let made = self.fs.create(root, &name_of(name), FILE, 0, 0, 0, &[]).await;
                let Some([attr, _]) = self.unless_full(made, "create")? else { return Ok(self) };
                if attr.ctime_ns < attr.mtime_ns {
                    return Err(format!("a new file's ctime is before its mtime: {attr:?}"));
                }
                self.model.names.insert(name, Entry::File(attr.ino));
                self.model.files.insert(attr.ino, Data::default());
            }
            Op::Symlink(name) if !self.model.names.contains_key(&name) => {
                let made = self.fs.symlink(root, &name_of(name), b"target", 0, 0).await;
                let Some([attr, _]) = self.unless_full(made, "symlink")? else { return Ok(self) };
                if attr.ctime_ns < attr.mtime_ns {
                    return Err(format!("a new symlink's ctime is before its mtime: {attr:?}"));
                }
                self.model.names.insert(name, Entry::Symlink(attr.ino));
            }
            Op::Link(file, name) if !self.model.names.contains_key(&name) => {
                let Some(ino) = self.model.file(file) else { return Ok(self) };
                let linked = self.fs.link(ino, root, &name_of(name), false).await;
                let Some([attr, _]) = self.unless_full(linked, "link")? else { return Ok(self) };
                self.model.names.insert(name, Entry::File(ino));
                if attr.nlink != self.model.links(ino) {
                    let links = self.model.links(ino);
                    return Err(format!("{ino} has {} links, not {links}", attr.nlink));
                }
            }
            Op::Unlink(name) if self.model.names.contains_key(&name) => {
                let entry = self.model.names.remove(&name).expect("looked up");
                let (Entry::File(ino) | Entry::Symlink(ino)) = entry;
                let gone = self.fs.unlink(root, &name_of(name), ino).await;
                gone.map_err(|e| format!("unlink: {e:#}"))?;
                self.forget(entry);
            }
            Op::Rename(from, to) if from != to && self.model.names.contains_key(&from) => {
                let (Entry::File(ino) | Entry::Symlink(ino)) = self.model.names[&from];
                let new_ino = self.model.names.get(&to).map(|replaced| {
                    let (Entry::File(ino) | Entry::Symlink(ino)) = replaced;
                    *ino
                });
                let renamed =
                    self.fs.rename(root, &name_of(from), ino, root, &name_of(to), new_ino).await;
                if self.unless_full(renamed, "rename")?.is_none() {
                    return Ok(self);
                }
                let entry = self.model.names.remove(&from).expect("looked up");
                let replaced = self.model.names.insert(to, entry);
                if let Some(replaced) = replaced {
                    self.forget(replaced);
                }
            }
            Op::Write { file, at, len, byte } => {
                let Some(ino) = self.model.file(file) else { return Ok(self) };
                let at = at.min(LARGEST - len as u64);
                let bytes = vec![byte; len];
                let written = self.fs.write(ino, at, &bytes).await;
                if self.unless_full(written, "write")?.is_some() {
                    self.model.files.get_mut(&ino).expect("a file").write(at, &bytes);
                }
                self.check(&self.model, ino).await?;
            }
            Op::Fill { .. } | Op::Clone { .. } => {
                let Some(plan) = op.plan(&self.model) else { return Ok(self) };
                let done = match *op {
                    Op::Clone { from, page, to_page, .. } => {
                        let src = self.model.file(from).expect("planned");
                        let end = page + plan.pages.len() as u64;
                        let size =
                            self.model.files[&plan.ino].size.max((to_page + end - page) * PAGE);
                        let range = page * PAGE..end * PAGE;
                        let dst_offset = to_page * PAGE;
                        self.fs.clone_range(src, range, plan.ino, dst_offset, size).await.map(drop)
                    }
                    _ => {
                        let bytes: Vec<&[u8]> = (plan.pages.iter())
                            .map(|(_, bytes)| bytes.as_ref().expect("written")[..].as_ref())
                            .collect();
                        let mut writes: Vec<(u64, &[u8])> = (plan.pages.iter())
                            .zip(bytes)
                            .map(|((page, _), bytes)| (page * PAGE, bytes))
                            .collect();
                        self.fs.write_many(plan.ino, &mut writes).await
                    }
                };
                let mut data = self.model.files[&plan.ino].clone();
                if self.unless_full(done, "a plan")?.is_some() {
                    plan.pages.iter().for_each(|(page, bytes)| data.put(*page, bytes));
                } else {
                    data = self.stopped_short(data, &plan).await?;
                }
                self.model.files.insert(plan.ino, data);
                self.check(&self.model, plan.ino).await?;
            }
            Op::Read { file, at, len } => {
                let Some(ino) = self.model.file(file) else { return Ok(self) };
                self.compare(&self.model.files[&ino], ino, at, len).await?;
            }
            Op::Truncate { file, size } => {
                let Some(ino) = self.model.file(file) else { return Ok(self) };
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
                let Some(ino) = self.model.file(file) else { return Ok(self) };
                let data = self.model.files.get_mut(&ino).expect("a file");
                let end = (page + pages).min(data.size / PAGE);
                if page < end {
                    let punched = self.fs.punch(ino, page * PAGE..end * PAGE).await;
                    if self.unless_full(punched, "punch")?.is_some() {
                        self.model.files.get_mut(&ino).expect("a file").punch(page..end);
                    }
                    self.check(&self.model, ino).await?;
                }
            }
            Op::SetXattr { file, name, len, byte } => {
                let Some(ino) = self.model.file(file) else { return Ok(self) };
                let value = vec![byte; len];
                let mode = SetExtendedAttributeMode::Set;
                let set = self.fs.set_xattr(ino, &xattr_of(name), &value, mode).await;
                set.map_err(|e| format!("set_xattr: {e:#}"))?;
                self.write_xattrs(ino, name, Some(value)).await?;
            }
            Op::RemoveXattr { file, name } => {
                let Some(ino) = self.model.file(file) else { return Ok(self) };
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
            Op::Remount => {
                self = self.remount().await?;
                for ino in self.model.files.keys() {
                    self.check(&self.model, *ino).await?;
                }
            }
            _ => {}
        }
        Ok(self)
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
        let second_cuts = Crashes::new(seed, SECOND_CUTS_ONE_IN);
        second_cuts.mark(0);
        let device = DeviceHolder::new(device.record_crashes(second_cuts.clone()));
        let setting = self.setting;
        let mut world =
            World::mount(setting, device, root, Model::default(), tally.clone()).await?;
        second_cuts.stop();
        checked(&world.filesystem).await?;
        world.holds_no_orphans().await?;
        let (first, last) = (self.durable[op], (op + 1).min(self.states.len() - 1));
        let mut differs = Vec::new();
        let mut held = None;
        for state in (first..=last).rev() {
            let mut candidates = vec![self.states[state].clone()];
            if let Some(short) = self.stopped_short(&world, state, ops).await {
                candidates.push(short);
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
    let crashes = Crashes::new(setting.seed, setting.cuts_one_in);
    let mut world = World::new(setting, &crashes, tally.clone()).await;
    let root = world.root;
    let mut past = Past { setting, states: vec![Model::default()], durable: vec![0] };
    world.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
    crashes.mark(0);
    for (i, op) in ops.iter().enumerate() {
        world = world.step(op).await.map_err(|e| format!("op {i}, {op:?}: {e}"))?;
        past.states.push(world.model.clone());
        let durable = matches!(op, Op::Sync | Op::Fsync | Op::Crash | Op::Remount);
        past.durable.push(if durable { i + 1 } else { past.durable[i] });
        crashes.mark(i as u64 + 1);
    }
    world.check_all().await.map_err(|e| format!("at the end: {e}"))?;
    let world = world.remount().await.map_err(|e| format!("at the end: {e}"))?;
    world.check_all().await.map_err(|e| format!("remounted at the end: {e}"))?;
    world.unmount().await?;
    for (cut, (mark, device)) in crashes.take().into_iter().enumerate() {
        let checked = past.check_cut((cut, mark as usize), &ops, device, root, tally.clone()).await;
        checked.map_err(|e| format!("a power cut during op {mark}: {e}"))?;
    }
    Ok(())
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
    let config = Config { cases, failure_persistence: None, ..Config::default() };
    let room = prop_oneof![Just(ROOMY), Just(TIGHT)];
    let sequences = (any::<u64>(), room, proptest::collection::vec(op(), 1..48));
    let tally = Rc::new(Tally::default());
    let watched = watch_sequences(if fsck_each { 30 * SEQUENCE_LIMIT } else { SEQUENCE_LIMIT });
    TestRunner::new(config)
        .run(&sequences, |(seed, blocks, ops)| {
            watched.send(format!("{seed}, {blocks}, {ops:?}")).expect("the watchdog stopped");
            let setting = Setting::awkward(seed, blocks).with_fsck_each(fsck_each);
            prop_assert_eq!(setting.run(ops, tally.clone()), Ok(()));
            Ok(())
        })
        .unwrap();
    let (cuts, second_cuts) = (tally.cuts.get(), tally.second_cuts.get());
    println!("{cuts} power cuts, {second_cuts} of replays, {} refused", tally.refused.get());
    assert!(cuts > 0, "no power cut");
    assert!(second_cuts > 0, "no power cut of a replay");
    assert!(tally.refused.get() > 0, "no device filled");
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
