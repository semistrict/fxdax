// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A guest's disk is trusted by nobody after a host failure or a bad upload. So images with real
//! content, in each way Fxfs protects them, have bytes changed across them, then are mounted, read
//! and checked, and written to: a mount or fsck may refuse them, and what they read may be wrong,
//! but nothing panics, hangs or runs out of memory.
//!
//! An image is one of four: journal checksums of data on or off (`barriers_enabled`), and closed
//! cleanly, or crashed with a journal to replay. It is mapped, by what mounts it, into the parts
//! each case aims at (`Part`), so that each is hit however small: the root, super-blocks, journal,
//! store information, layer files, the allocator's, file data, and anywhere at all.
//!
//! The super-blocks and the journal are checksummed, and a change to them would never get past
//! the first block read. So half of the cases that aim at them seal what they changed, as anyone
//! who wrote the image would, and what reads them meets it: a `sealed` case.
//!
//! Each case is what a seed makes it (FXDAX_CORRUPT_SEED, the first). FXDAX_CORRUPT_CASES asks for
//! more of them, to hunt. Every panic is counted by where it was, and the test fails saying each,
//! with a seed that gets there, so that one run shows them all: a seed replays what was flipped,
//! not the bytes of what a new image holds, which is why what fixes one pins it by what it hit.
//!
//! What it proves, printed for each part as it ends: a case is `refused` by the mount, `caught` by
//! fsck, `failed` a read, was `same` as it was, or read back `different` with no one saying so. No
//! panic is the assertion: nothing here asks that a flip be found, as only the super-blocks', the
//! journal's and the data being written's checksums can find one, which tests of their own pin
//! (`a_flip_in_both_super_blocks_is_refused`, `a_flip_in_data_being_written_...`). The rest, with
//! no checksum anyone reads, is `different`, or `same` of bytes nothing looks at.

use super::model_tests::{checked, watch_sequences};
use super::tests::Tested;
use super::*;
use fxfs::checksum::fletcher64;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
use fxfs::lsm_tree::Query;
use fxfs::lsm_tree::types::{ItemRef, LayerIterator as _};
use fxfs::object_store::journal::RESET_XOR;
use fxfs::object_store::journal::super_block::SuperBlockInstance;
use fxfs::object_store::object_record::ObjectKeyData;
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{ExtentValue, NewChildStoreOptions, StoreOptions};
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use storage_device::fake_device::{Crashes, FakeDevice};
use storage_device::{DeviceHolder, ExtentId};

const FILE: u32 = S_IFREG | 0o644;
const DIR: u32 = S_IFDIR | 0o755;
const PAGE: u64 = 4096;
/// The journal's and the super-blocks' blocks, each sealed by a checksum in its last bytes.
const SEALED_BLOCK: usize = PAGE as usize;
const SEAL: usize = std::mem::size_of::<u64>();
/// Cases an image gets, unless FXDAX_CORRUPT_CASES says otherwise.
const CASES: u64 = 120;
/// How long a case may take: one that takes longer hangs.
const CASE_LIMIT: Duration = Duration::from_secs(120);
/// The most memory a case may hold: its image, and what a mount needs. More came from a length
/// nobody checked.
const MOST_HELD: usize = 1 << 30;
/// How much of a file a case reads: a size may be anything.
const MOST_READ: u64 = 8 << 20;
const READ_AT_ONCE: usize = 1 << 18;
/// How many files and directories a case reads: a directory may hold itself.
const MOST_ENTRIES: usize = 4096;
/// The most changes a case makes.
const MOST_DAMAGES: u32 = 3;
/// What a case's changes put in a number.
const AWKWARD: [u64; 9] = [0, 1, 4095, 4096, 1 << 31, 1 << 32, 1 << 48, i64::MAX as u64, u64::MAX];
/// How bincode, which Fxfs's records are written with, starts the largest of numbers.
const HUGE_VARINT: [u8; 9] = [253, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];

/// Holds a case to `MOST_HELD`, which a test run alone by nextest has to itself: past it, says
/// which case and from where, and aborts, as what's asked for might otherwise not be touched, and
/// so never noticed.
struct Bounded;

/// What's held, and how much may be: nothing is checked until this is set.
static HELD: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(0);
static CASE: Mutex<Option<String>> = Mutex::new(None);

#[global_allocator]
static ALLOCATOR: Bounded = Bounded;

impl Bounded {
    fn check(size: usize) {
        let held = HELD.fetch_add(size, Ordering::Relaxed) + size;
        let limit = LIMIT.load(Ordering::Relaxed);
        if limit != 0 && held > limit {
            // Nothing below may come back here.
            LIMIT.store(0, Ordering::Relaxed);
            let backtrace = std::backtrace::Backtrace::force_capture();
            let case = CASE.lock().unwrap();
            eprintln!("{held} bytes held, of at most {limit}, in {case:?}:\n{backtrace}");
            std::process::abort();
        }
    }
}

// SAFETY: this only counts what `System` does.
unsafe impl GlobalAlloc for Bounded {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::check(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        Self::check(size);
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

/// What part of an image a case aims at.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Part {
    Root,
    SuperBlock,
    Journal,
    StoreInfo,
    RootLayer,
    AllocatorInfo,
    AllocatorLayer,
    VolumeLayer,
    FileData,
    Anywhere,
}

impl Part {
    const ALL: [Part; 10] = [
        Part::Root,
        Part::SuperBlock,
        Part::Journal,
        Part::StoreInfo,
        Part::RootLayer,
        Part::AllocatorInfo,
        Part::AllocatorLayer,
        Part::VolumeLayer,
        Part::FileData,
        Part::Anywhere,
    ];

    /// Whether checksums stand in the way of what aims here.
    fn sealed(self) -> bool {
        matches!(self, Part::SuperBlock | Part::Journal)
    }
}

/// Where a part of an image is.
#[derive(Clone, Debug)]
struct Region {
    part: Part,
    extent: ExtentId,
    bytes: std::ops::Range<usize>,
}

/// A run of sealed blocks, each one's checksum seeded by the last: a super-block, or the journal
/// from its checkpoint to its end.
#[derive(Clone, Debug)]
struct Chain {
    seed: u64,
    blocks: Vec<SealedBlock>,
}

#[derive(Clone, Debug)]
struct SealedBlock {
    extent: ExtentId,
    at: usize,
    /// Whether the journal was reset here, which changes its seed.
    reset: bool,
}

impl Chain {
    /// What of `blocks`, in order, is sealed from `seed` on.
    fn of(device: &FakeDevice, seed: u64, blocks: Vec<(ExtentId, usize)>) -> Self {
        let mut chain = Self { seed, blocks: Vec::new() };
        let mut last = seed;
        for (extent, at) in blocks {
            // A journal's end may have extents never written.
            let Some(held) = device.held(extent) else { break };
            let Some(block) = held.get(at..at + SEALED_BLOCK) else { break };
            let (contents, seal) = block.split_at(SEALED_BLOCK - SEAL);
            let seal = u64::from_le_bytes(seal.try_into().unwrap());
            let reset = if seal == fletcher64(contents, last) {
                false
            } else if seal == fletcher64(contents, last ^ RESET_XOR) {
                true
            } else {
                break;
            };
            chain.blocks.push(SealedBlock { extent, at, reset });
            last = seal;
        }
        chain
    }

    /// Seals what the blocks hold, whatever was changed in them.
    fn reseal(&self, extents: &mut HashMap<ExtentId, Vec<u8>>) {
        let mut last = self.seed;
        for SealedBlock { extent, at, reset } in &self.blocks {
            let Some(held) = extents.get_mut(extent) else { return };
            let Some(block) = held.get_mut(*at..*at + SEALED_BLOCK) else { return };
            let (contents, seal) = block.split_at_mut(SEALED_BLOCK - SEAL);
            last = fletcher64(contents, if *reset { last ^ RESET_XOR } else { last });
            seal.copy_from_slice(&last.to_le_bytes());
        }
    }
}

/// How an image was left.
#[derive(Clone, Copy, Debug)]
enum Ending {
    /// Compacted and unmounted, with next to nothing to replay.
    Closed,
    /// Crashed just after a sync, with a journal to replay.
    Crashed,
}

/// What a volume reads as, by path, as a hash of each entry's attributes and contents.
type Reading = BTreeMap<String, u64>;

/// An image to corrupt: what's on it, where, and what it reads as.
struct Image {
    barriers: bool,
    device: FakeDevice,
    regions: Vec<Region>,
    chains: Vec<Chain>,
    reading: Reading,
}

/// One change to an image, as a seed says.
#[derive(Clone, Copy, Debug)]
struct Case {
    seed: u64,
    part: Part,
    /// Whether the changes are sealed (`Chain::reseal`).
    sealed: bool,
}

/// What came of a case.
#[derive(Clone, Copy, Debug, Default)]
struct Outcome {
    refused: u64,
    caught: u64,
    failed: u64,
    same: u64,
    different: u64,
    writes_refused: u64,
}

impl Outcome {
    fn add(&mut self, other: &Outcome) {
        self.refused += other.refused;
        self.caught += other.caught;
        self.failed += other.failed;
        self.same += other.same;
        self.different += other.different;
        self.writes_refused += other.writes_refused;
    }
}

/// A panic, by where it was: how many cases met it, and one of them.
static PANICS: Mutex<BTreeMap<String, (u64, String)>> = Mutex::new(BTreeMap::new());

/// Runs `done` to its end, whatever panics in it, as cases must to be counted.
fn run_case<T>(done: impl Future<Output = T>) -> Option<T> {
    let run = || fasync::LocalExecutorBuilder::new().build().run_singlethreaded(done);
    std::panic::catch_unwind(AssertUnwindSafe(run)).ok()
}

fn count_panics() {
    std::panic::set_hook(Box::new(|panic| {
        let place = panic.location().map_or(String::new(), |place| place.to_string());
        let message = panic.payload_as_str().unwrap_or("").to_string();
        let case = CASE.lock().unwrap().clone().unwrap_or_default();
        let mut panics = PANICS.lock().unwrap();
        let (count, _) = panics.entry(place).or_insert((0, format!("{case}: {message}")));
        *count += 1;
    }));
}

fn builder(barriers: bool, read_only: bool) -> FxFilesystemBuilder {
    // As the guest's mount.
    FxFilesystemBuilder::new().barriers_enabled(barriers).read_only(read_only).lazy_volumes(true)
}

/// What an image holds: each thing Fxfs keeps records of. `round` makes its names its own.
async fn populate(fs: &Fxfs<Tested>, root: u64, round: u8) {
    let name = |name: &str| format!("{name}{round}").into_bytes();
    let new_file = async |dir, name: &[u8]| {
        fs.create(dir, name, FILE, 0, 0, 0, &[]).await.expect("creating")[0].ino
    };
    let dir = fs.create(root, &name("dir"), DIR, 0, 0, 0, &[]).await.expect("mkdir")[0].ino;
    let deeper = fs.create(dir, b"deeper", DIR, 0, 0, 0, &[]).await.expect("mkdir")[0].ino;
    for (index, size) in [0usize, 1, 4095, 4096, 4097, 65536, 1 << 18].into_iter().enumerate() {
        let ino = new_file(deeper, format!("file{index}").as_bytes()).await;
        if size != 0 {
            fs.write(ino, 0, &vec![index as u8 + round; size]).await.expect("writing");
        }
    }
    // Many extents, a hole between each.
    let fragments = new_file(dir, b"fragments").await;
    for page in (0..256).step_by(2) {
        fs.write(fragments, page * PAGE, &[round; PAGE as usize]).await.expect("writing");
    }
    let sparse = new_file(dir, b"sparse").await;
    fs.write(sparse, 1 << 32, b"far").await.expect("writing");
    let attributed = new_file(root, &name("attributed")).await;
    fs.set_xattr(attributed, b"small", b"value", SetExtendedAttributeMode::Set)
        .await
        .expect("set_xattr");
    fs.set_xattr(attributed, b"large", &[round; 8192], SetExtendedAttributeMode::Set)
        .await
        .expect("set_xattr");
    fs.write_inode(attributed, &SetAttr::default()).await.expect("write_inode");
    fs.symlink(dir, b"link", b"somewhere/else", 0, 0).await.expect("symlink");
    fs.link(fragments, root, &name("hard"), false).await.expect("link");
    fs.rename(dir, b"sparse", sparse, root, &name("renamed"), None).await.expect("rename");
    let clone = new_file(root, &name("clone")).await;
    fs.clone_range(fragments, 0..64 * PAGE, clone, 0, 64 * PAGE).await.expect("clone");
    fs.punch(fragments, 8 * PAGE..16 * PAGE).await.expect("punch");
    fs.setattr(clone, &SetAttr { valid: SET_SIZE, size: 32 * PAGE, ..SetAttr::default() })
        .await
        .expect("truncate");
    // What a mount reaps: unlinked, as the kernel has it open.
    let unlinked = new_file(root, &name("unlinked")).await;
    fs.write(unlinked, 0, &[round; 2 * PAGE as usize]).await.expect("writing");
    fs.unlink(root, &name("unlinked"), unlinked).await.expect("unlink");
    let many = fs.create(root, &name("many"), DIR, 0, 0, 0, &[]).await.expect("mkdir")[0].ino;
    for index in 0..64 {
        new_file(many, format!("{index:0>200}").as_bytes()).await;
    }
}

/// Reads all a volume holds, as its VFS does, up to where a corrupt one may lead: errors, of which
/// there may be any number, are counted. A directory holding itself is read once.
async fn read_volume(fs: &Fxfs<Tested>, root: u64) -> (Reading, u64) {
    let (mut reading, mut errors) = (Reading::new(), 0);
    let mut seen = BTreeSet::from([root]);
    let mut queue = vec![(String::from("/"), root)];
    while let Some((path, ino)) = queue.pop() {
        if reading.len() >= MOST_ENTRIES {
            break;
        }
        let mut hash = DefaultHasher::new();
        let attr = match fs.attr(ino).await {
            Ok(attr) => attr,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        (attr.size, attr.mode, attr.nlink, attr.mtime_ns).hash(&mut hash);
        match attr.mode & S_IFMT {
            S_IFDIR => {
                let mut entries = Vec::new();
                let read = fs.readdir(ino, ReaddirStart::Index(0), |name, ino, kind| {
                    entries.push((name.to_string(), ino, kind));
                    entries.len() < MOST_ENTRIES
                });
                errors += read.await.is_err() as u64;
                for (name, child, kind) in entries {
                    (&name, kind).hash(&mut hash);
                    errors += fs.lookup(ino, name.as_bytes()).await.is_err() as u64;
                    if seen.insert(child) {
                        queue.push((format!("{path}{name}/"), child));
                    }
                }
            }
            S_IFLNK => match fs.readlink(ino).await {
                Ok(target) => target.hash(&mut hash),
                Err(_) => errors += 1,
            },
            _ => {
                let mut buffer = vec![0; READ_AT_ONCE];
                let mut at = 0;
                while at < attr.size.min(MOST_READ) {
                    match fs.read(ino, at, &mut buffer).await {
                        Ok(0) => break,
                        Ok(read) => {
                            buffer[..read].hash(&mut hash);
                            at += read as u64;
                        }
                        Err(_) => {
                            errors += 1;
                            break;
                        }
                    }
                }
            }
        }
        match fs.list_xattrs(ino).await {
            Ok(names) => {
                for name in names {
                    match fs.get_xattr(ino, &name).await {
                        Ok(value) => (name, value).hash(&mut hash),
                        Err(_) => errors += 1,
                    }
                }
            }
            Err(_) => errors += 1,
        }
        reading.insert(path, hash.finish());
    }
    (reading, errors)
}

/// The volume `test` of a mounted filesystem, as a guest's mount opens it.
async fn volume_of(filesystem: &OpenFxFilesystem, read_only: bool) -> Result<Fxfs<Tested>, Error> {
    let volumes = root_volume((**filesystem).clone()).await?;
    let volume = volumes.volume("test", StoreOptions::default()).await?;
    Ok(Fxfs::new((**filesystem).clone(), volume, read_only, Tested))
}

/// Unmounts, as a guest does, which may fail, as what it had to write finds what's there.
async fn unmount(fs: Option<Fxfs<Tested>>, filesystem: OpenFxFilesystem) {
    if let Some(fs) = fs {
        let _ = fs.stop_delayed().await;
    }
    let _ = filesystem.close().await;
}

/// Calls `each` with the object, range in the object and range in an extent, of each extent of
/// `store`, in order.
async fn extents_of(
    store: &ObjectStore,
    mut each: impl FnMut(u64, std::ops::Range<u64>, ExtentId, std::ops::Range<usize>),
) {
    let layer_set = store.tree().layer_set();
    let mut merger = layer_set.merger();
    let mut items = merger.query(Query::FullScan).await.expect("scanning");
    while let Some(ItemRef { key, value, .. }) = items.get() {
        if let (
            ObjectKeyData::Attribute(_, AttributeKey::Extent(range)),
            ObjectValue::Extent(ExtentValue::Some { extent, extent_offset, .. }),
        ) = (&key.data, value)
        {
            let len = range.0.end - range.0.start;
            let bytes = *extent_offset as usize..(*extent_offset + len) as usize;
            each(key.object_id, range.0.clone(), *extent, bytes);
        }
        items.advance().await.expect("scanning");
    }
}

/// Where the parts of what `device` holds are, as what mounts it knows.
async fn map(device: &FakeDevice, barriers: bool) -> (Vec<Region>, Vec<Chain>) {
    let filesystem = builder(barriers, true).open(DeviceHolder::new(device.recovered(0))).await;
    let filesystem = filesystem.expect("mounting what was made");
    let header = filesystem.super_block_header();
    let root_store = filesystem.root_store();
    let root_id = root_store.store_object_id();
    let allocator = filesystem.allocator();
    let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
    let test = volumes.volume("test", StoreOptions::default()).await.expect("the volume");
    let snapshot = volumes.volume("snapshot", StoreOptions::default()).await.expect("the snapshot");
    let (test_id, snapshot_id) = (test.store_object_id(), snapshot.store_object_id());
    let mut volume_layers = test.parent_objects();
    volume_layers.extend(snapshot.parent_objects());

    let mut regions = Vec::new();
    let mut in_journal = Vec::new();
    let mut in_super_blocks = BTreeMap::<u64, Vec<(ExtentId, usize)>>::new();
    let blocks = |bytes: &std::ops::Range<usize>| bytes.clone().step_by(SEALED_BLOCK);

    let checkpoint = header.journal_checkpoint.clone();
    let root_parent = filesystem.root_parent_store();
    let mut region = |part, extent, bytes| regions.push(Region { part, extent, bytes });
    extents_of(&root_parent, |object, range, extent, bytes| {
        let part = if object == header.journal_object_id {
            let first = PAGE * (checkpoint.file_offset / PAGE);
            let live = (range.start..range.end).step_by(SEALED_BLOCK).zip(blocks(&bytes));
            let live = live.filter(|(offset, _)| *offset >= first);
            in_journal.extend(live.map(|(_, at)| (extent, at)));
            Part::Journal
        } else if object == root_id {
            Part::StoreInfo
        } else {
            Part::RootLayer
        };
        region(part, extent, bytes);
    })
    .await;
    let root_layers = root_store.parent_objects();
    let allocator_layers = allocator.parent_objects();
    let super_blocks = [SuperBlockInstance::A.object_id(), SuperBlockInstance::B.object_id()];
    extents_of(&root_store, |object, _, extent, bytes| {
        let part = if super_blocks.contains(&object) {
            in_super_blocks
                .entry(object)
                .or_default()
                .extend(blocks(&bytes).map(|at| (extent, at)));
            Part::SuperBlock
        } else if object == header.allocator_object_id {
            Part::AllocatorInfo
        } else if allocator_layers.contains(&object) {
            Part::AllocatorLayer
        } else if volume_layers.contains(&object) {
            Part::VolumeLayer
        } else if object == test_id || object == snapshot_id {
            Part::StoreInfo
        } else if root_layers.contains(&object) {
            Part::RootLayer
        } else {
            Part::Anywhere
        };
        region(part, extent, bytes);
    })
    .await;
    extents_of(&test, |_, _, extent, bytes| region(Part::FileData, extent, bytes)).await;

    let mut chains = vec![Chain::of(device, checkpoint.checksum, in_journal)];
    chains.extend(in_super_blocks.into_values().map(|blocks| Chain::of(device, 0, blocks)));
    for chain in &chains {
        assert!(!chain.blocks.is_empty(), "{chain:?} holds nothing sealed");
    }
    drop((test, snapshot, volumes, root_parent, root_store, allocator));
    unmount(None, filesystem).await;
    (regions, chains)
}

impl Image {
    async fn new(barriers: bool, ending: Ending) -> Self {
        let crashes = Crashes::new(0, u32::MAX);
        let device = FakeDevice::new(16384, PAGE as u32).awkward(0).record_crashes(crashes.clone());
        let filesystem = builder(barriers, false).format(true).open(DeviceHolder::new(device));
        let filesystem = filesystem.await.expect("formatting");
        let device = filesystem.device();
        let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
        let new = NewChildStoreOptions::default();
        let volume = volumes.new_volume("test", new).await.expect("the volume");
        let root = volume.root_directory_object_id();
        let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);

        // Layers of a few ages, a snapshot sharing the oldest, and a journal holding the youngest.
        let journal = filesystem.journal();
        populate(&fs, root, 0).await;
        journal.force_compact().await.expect("compacting");
        volumes.snapshot_volume("test", "snapshot").await.expect("the snapshot");
        populate(&fs, root, 1).await;
        journal.force_compact().await.expect("compacting");
        populate(&fs, root, 2).await;
        fs.sync().await.expect("syncing");

        let (reading, errors) = read_volume(&fs, root).await;
        assert_eq!(errors, 0);
        // What's recorded is the next barrier's: of a close, there are many.
        match ending {
            Ending::Closed => {
                fs.stop_delayed().await.expect("stopping");
                journal.force_compact().await.expect("compacting");
                drop((fs, volumes));
                filesystem.close().await.expect("closing");
                let device = filesystem.take_device().await;
                device.reopen(false);
                crashes.mark(0);
                crashes.last();
                device.flush().await.expect("flushing");
            }
            Ending::Crashed => {
                crashes.mark(0);
                crashes.last();
                device.flush().await.expect("flushing");
                fs.stop_delayed().await.expect("stopping");
                drop((fs, volumes));
                filesystem.close().await.expect("closing");
            }
        }
        let (_, device) = crashes.take().next().expect("what the device held");
        let (regions, chains) = map(&device, barriers).await;
        Self { barriers, device, regions, chains, reading }
    }

    /// The image, as `case` leaves it.
    fn damaged(&self, case: Case) -> FakeDevice {
        let device = self.device.recovered(case.seed);
        let mut rng = StdRng::seed_from_u64(case.seed);
        device.corrupt(|extents, root| {
            for _ in 0..rng.random_range(1..=MOST_DAMAGES) {
                match case.part {
                    Part::Root => damage_bytes(&mut rng, root),
                    Part::Anywhere => {
                        let ids: BTreeSet<ExtentId> = extents.keys().copied().collect();
                        let extent = *ids.iter().nth(rng.random_range(0..ids.len())).unwrap();
                        let bytes = 0..extents[&extent].len();
                        damage_extent(&mut rng, extents, extent, bytes);
                    }
                    part => {
                        let regions = self.regions.iter().filter(|region| region.part == part);
                        let regions: Vec<_> = regions.collect();
                        assert!(!regions.is_empty(), "no {part:?}");
                        let region = regions[rng.random_range(0..regions.len())];
                        damage_extent(&mut rng, extents, region.extent, region.bytes.clone());
                    }
                }
            }
            if case.sealed {
                self.chains.iter().for_each(|chain| chain.reseal(extents));
            }
        });
        device
    }
}

/// Changes `bytes` somewhere, leaning on its edges, where what describes the rest is.
fn damage_bytes(rng: &mut StdRng, bytes: &mut [u8]) {
    let len = bytes.len();
    if len == 0 {
        return;
    }
    let edge = len.min(256);
    let at = match rng.random_range(0..4) {
        0 => rng.random_range(0..edge),
        1 => len - rng.random_range(1..=edge),
        _ => rng.random_range(0..len),
    };
    let run = (len - at).min(1 << rng.random_range(1..7));
    match rng.random_range(0..8) {
        0 => bytes[at] ^= 1 << rng.random_range(0..8),
        1 => bytes[at] = rng.random(),
        2 => rng.fill(&mut bytes[at..at + run]),
        3 => bytes[at..at + run].fill(0),
        4 => bytes[at..at + run].fill(0xff),
        5 => {
            let number = AWKWARD[rng.random_range(0..AWKWARD.len())].to_le_bytes();
            let width = (1usize << rng.random_range(1..4)).min(len - at);
            bytes[at..at + width].copy_from_slice(&number[..width]);
        }
        6 => {
            let width = HUGE_VARINT.len().min(len - at);
            bytes[at..at + width].copy_from_slice(&HUGE_VARINT[..width]);
        }
        // One of Fxfs's own: a block where another should be.
        _ => {
            let block = SEALED_BLOCK.min(len);
            let from = rng.random_range(0..=(len - block));
            let to = at.min(len - block);
            bytes.copy_within(from..from + block, to);
        }
    }
}

/// Changes `bytes` of `extent`, or loses it, or its end.
fn damage_extent(
    rng: &mut StdRng,
    extents: &mut HashMap<ExtentId, Vec<u8>>,
    extent: ExtentId,
    bytes: std::ops::Range<usize>,
) {
    let Some(held) = extents.get_mut(&extent) else { return };
    match rng.random_range(0..32) {
        0 => drop(extents.remove(&extent)),
        1 => held.truncate(held.len() / SEALED_BLOCK / 2 * SEALED_BLOCK),
        _ => {
            let end = bytes.end.min(held.len());
            let mut part = held[bytes.start.min(end)..end].to_vec();
            damage_bytes(rng, &mut part);
            held[bytes.start.min(end)..end].copy_from_slice(&part);
        }
    }
}

/// Mounts `case` read-only, which changes nothing, to see what a checking tool would: what it is
/// that came of the case.
async fn read_only(image: &Image, case: Case) -> Outcome {
    let mut outcome = Outcome::default();
    let device = DeviceHolder::new(image.damaged(case));
    let Ok(filesystem) = builder(image.barriers, true).open(device).await else {
        outcome.refused = 1;
        return outcome;
    };
    let caught = checked(&filesystem).await.is_err();
    let fs = volume_of(&filesystem, true).await;
    match &fs {
        Err(_) => outcome.refused = 1,
        Ok(fs) => {
            let (reading, errors) = read_volume(fs, fs.volume.root_directory_object_id()).await;
            if caught {
                outcome.caught = 1;
            } else if errors > 0 {
                outcome.failed = 1;
            } else if reading == image.reading {
                outcome.same = 1;
            } else {
                outcome.different = 1;
            }
        }
    }
    unmount(fs.ok(), filesystem).await;
    outcome
}

/// Mounts `case` writably, as a guest would, reads it, changes it and checks it. A filesystem that
/// mounted may fail any of it.
async fn writable(image: &Image, case: Case, outcome: &mut Outcome) {
    let device = DeviceHolder::new(image.damaged(case));
    let Ok(filesystem) = builder(image.barriers, false).open(device).await else { return };
    let fs = volume_of(&filesystem, false).await.ok();
    if let Some(fs) = &fs {
        let root = fs.volume.root_directory_object_id();
        read_volume(fs, root).await;
        let wrote = async {
            let ino = fs.create(root, b"after", FILE, 0, 0, 0, &[]).await?[0].ino;
            fs.write(ino, 0, b"written after").await?;
            fs.sync().await
        };
        outcome.writes_refused = wrote.await.is_err() as u64;
    }
    let _ = filesystem.journal().force_compact().await;
    let _ = checked(&filesystem).await;
    unmount(fs, filesystem).await;
}

async fn try_case(image: &Image, case: Case) -> Outcome {
    let mut outcome = read_only(image, case).await;
    writable(image, case, &mut outcome).await;
    outcome
}

/// Flips an image of each kind of `barriers` and `ending`, as FXDAX_CORRUPT_CASES and
/// FXDAX_CORRUPT_SEED say.
fn flips_never_panic(barriers: bool, ending: Ending) {
    let number =
        |name, default| std::env::var(name).map_or(default, |n: String| n.parse().unwrap());
    let (cases, first) = (number("FXDAX_CORRUPT_CASES", CASES), number("FXDAX_CORRUPT_SEED", 0));
    let image = run_case(Image::new(barriers, ending)).expect("an image");
    count_panics();
    let watched = watch_sequences(CASE_LIMIT);
    LIMIT.store(HELD.load(Ordering::Relaxed) + MOST_HELD, Ordering::Relaxed);
    let mut tally = BTreeMap::<(Part, bool), (u64, Outcome)>::new();
    for seed in first..first + cases {
        let index = seed as usize;
        let part = Part::ALL[index % Part::ALL.len()];
        let sealed = part.sealed() && (index / Part::ALL.len()).is_multiple_of(2);
        let case = Case { seed, part, sealed };
        let said = format!("{case:?} of barriers {barriers}, {ending:?}");
        watched.send(said.clone()).expect("the watchdog stopped");
        *CASE.lock().unwrap() = Some(said);
        let outcome = run_case(try_case(&image, case)).unwrap_or_default();
        let (count, outcomes) = tally.entry((part, sealed)).or_default();
        *count += 1;
        outcomes.add(&outcome);
    }
    LIMIT.store(0, Ordering::Relaxed);
    let mut all = Outcome::default();
    for ((part, sealed), (count, outcome)) in &tally {
        println!("{part:?}, sealed {sealed}: {count} cases: {outcome:?}");
        all.add(outcome);
    }
    println!("all: {all:?}");
    let panics = std::mem::take(&mut *PANICS.lock().unwrap());
    for (place, (count, first)) in &panics {
        println!("{count} panics at {place}: first {first}");
    }
    assert_eq!(panics.len(), 0, "{panics:#?}");
    // What's found, and what isn't, come of every run: one that didn't get that far says little.
    assert!(all.refused > 0, "nothing refused: {all:?}");
    assert!(all.same > 0, "nothing the same: {all:?}");
    assert!(all.different > 0, "nothing read differently: {all:?}");
}

#[test]
fn flips_in_a_closed_image_with_data_checksums_journaled() {
    flips_never_panic(false, Ending::Closed);
}

#[test]
fn flips_in_a_crashed_image_with_data_checksums_journaled() {
    flips_never_panic(false, Ending::Crashed);
}

#[test]
fn flips_in_a_closed_image_with_barriers() {
    flips_never_panic(true, Ending::Closed);
}

#[test]
fn flips_in_a_crashed_image_with_barriers() {
    flips_never_panic(true, Ending::Crashed);
}

/// The images are what they say: flips of one that differed only in name would prove nothing more.
#[test]
fn a_closed_image_has_next_to_nothing_to_replay() {
    let blocks = |image: &Image| image.chains[0].blocks.len();
    let (closed, crashed) = run_case(async {
        (Image::new(false, Ending::Closed).await, Image::new(false, Ending::Crashed).await)
    })
    .expect("no panic");
    assert!(blocks(&closed) * 4 < blocks(&crashed), "{} {}", blocks(&closed), blocks(&crashed));
}

/// The checksums of the super-blocks prove this much, whatever else does: changing a byte of each
/// that is read refuses the image.
#[test]
fn a_flip_in_both_super_blocks_is_refused() {
    run_case(async {
        let image = Image::new(false, Ending::Closed).await;
        let super_blocks: Vec<_> = (image.chains.iter()).filter(|chain| chain.seed == 0).collect();
        assert_eq!(super_blocks.len(), 2);
        let mut rng = StdRng::seed_from_u64(0);
        let longest = super_blocks.iter().map(|chain| chain.blocks.len()).max().unwrap();
        for block in 0..longest {
            let device = image.device.recovered(0);
            device.corrupt(|extents, _| {
                for chain in &super_blocks {
                    let SealedBlock { extent, at, .. } = &chain.blocks[block % chain.blocks.len()];
                    let at = at + rng.random_range(0..SEALED_BLOCK - SEAL);
                    extents.get_mut(extent).expect("a super-block")[at] ^= 1;
                }
            });
            let mounted = builder(false, true).open(DeviceHolder::new(device)).await;
            assert!(mounted.is_err(), "block {block} of {longest} wasn't found flipped");
        }
    })
    .expect("no panic");
}

/// What journaling data's checksums buys: a flip of data written since the device last flushed is
/// found, and replay stops short of what wrote it, as of a write the device lost, leaving a file
/// as it was: with barriers it isn't, and it reads as it was changed. Past that flush, or on a
/// read, neither finds it, as the checksums in extent records are only ever written:
/// `flips_never_panic` counts those as `different`.
fn a_flip_in_data_being_written(barriers: bool, expected: &[u8]) {
    let run = async {
        let crashes = Crashes::new(0, u32::MAX);
        let device = FakeDevice::new(16384, PAGE as u32).calm().record_crashes(crashes.clone());
        let device = DeviceHolder::new(device);
        let filesystem = builder(barriers, false).format(true).open(device).await.expect("format");
        let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
        let new = NewChildStoreOptions::default();
        let volume = volumes.new_volume("test", new).await.expect("the volume");
        let root = volume.root_directory_object_id();
        let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
        fs.sync().await.expect("syncing");
        let ino = fs.create(root, b"file", FILE, 0, 0, 0, &[]).await.expect("creating")[0].ino;
        fs.write(ino, 0, &[1; PAGE as usize]).await.expect("writing");
        fs.sync().await.expect("syncing");
        crashes.mark(0);
        crashes.last();
        filesystem.device().flush().await.expect("flushing");
        fs.stop_delayed().await.expect("stopping");
        drop(fs);
        drop(volumes);
        filesystem.close().await.expect("closing");
        let (_, device) = crashes.take().next().expect("what the device held");

        let filesystem = builder(barriers, true).open(DeviceHolder::new(device.recovered(0)));
        let filesystem = filesystem.await.expect("mounting what was made");
        let fs = volume_of(&filesystem, true).await.expect("the volume");
        let mut data = None;
        extents_of(&fs.volume, |_, _, extent, bytes| data = Some((extent, bytes))).await;
        let (extent, bytes) = data.expect("the file's data");
        unmount(Some(fs), filesystem).await;

        device.corrupt(|extents, _| extents.get_mut(&extent).unwrap()[bytes.start] ^= 0xff);
        let filesystem = builder(barriers, true).open(DeviceHolder::new(device));
        let filesystem = filesystem.await.expect("mounting");
        let fs = volume_of(&filesystem, true).await.expect("the volume");
        let root = fs.volume.root_directory_object_id();
        let attr = fs.lookup(root, b"file").await.expect("the file");
        let mut read = vec![0; attr.size as usize];
        let len = fs.read(attr.ino, 0, &mut read).await.expect("reading");
        read.truncate(len);
        unmount(Some(fs), filesystem).await;
        read
    };
    assert_eq!(run_case(run).expect("no panic"), expected);
}

#[test]
fn a_flip_in_data_being_written_is_found_by_journaled_checksums() {
    a_flip_in_data_being_written(false, &[]);
}

#[test]
fn a_flip_in_data_being_written_is_not_found_with_barriers() {
    let mut flipped = vec![1; PAGE as usize];
    flipped[0] ^= 0xff;
    a_flip_in_data_being_written(true, &flipped);
}
