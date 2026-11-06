// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Random sequences of what dax.c and data.c ask of Fxfs, on guest/sim's model of it and on Fxfs
//! (fxfs-posix over a fake device), each file compared after every operation: its size, its bytes
//! up to it, and its blocks, whole, through its extents (past the end too, as a DAX window maps the
//! last block whole), which are holes on both sides alike, shared alike, and never open in Fxfs
//! where the model says sealed, which would have the sim's dax.c MAP what a host refuses.
//!
//! The operations: writeback (fxfs_write_many), of whole pages and of parts of them, up to the
//! file's size, paid with room reserved or not, and failing between its bytes and its commit as a
//! crash would (fxfs_set_fail_commits, FAIL_COMMITS); a truncate's size (fxfs_write_inode), cutting
//! with and without FXFS_SET_KEEP_TAIL, and growing; a clone (fxfs_clone_range), of whole blocks or
//! to the source's end; a punch; a seal (fxfs_fsync); and a remount of Fxfs, which the model, with
//! no journal, doesn't have.
//!
//! The sim interleaves tasks at the model's scheduling points (`sim_point`), where its locks let
//! another's operation in: a seal's COMMIT and its end, and writeback's copy of each page, its
//! write, the write's reply, and its commit. Some sequences run an operation there, in the model,
//! and in Fxfs before or after the one it fell in, as the point says. The model's own bugs found
//! by the sim alone so far were of the two kinds: a growth that kept what a cut left past the end
//! (TASK-149.14.2), and an extent a seal minted and the device sealed which the model called open
//! until the seal's end (seed 32826).
//!
//! FXDAX_SIM_DIFF_CASES asks for more sequences, FXDAX_SIM_DIFF_OPS for longer ones.

use crate::model::{self, Errno, Model};
use fuchsia_async as fasync;
use fxfs::filesystem::{FxFilesystem, OpenFxFilesystem};
use fxfs::object_store::data_object_handle::{ExtentAccess, FAIL_COMMITS};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
use fxfs_posix::{Fxfs, Platform, SET_KEEP_TAIL, SET_SIZE, SetAttr};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use storage_device::fake_device::FakeDevice;
use storage_device::{DeviceHolder, ExtentId};

const PAGE: u64 = 4096;
/// The files a sequence has: few, so that clones and interleavings meet.
const FILES: usize = 3;
/// The pages operations reach: more than the model packs into its staging extent at once (64), so
/// that some writebacks have extents of their own.
const PAGES: u64 = 96;
/// Fxfs's device: room for anything a sequence writes.
const DEVICE_BLOCKS: u64 = 65536;

/// As the host does: nothing queued wakes by the clock but what Fxfs spawns.
struct Plain;

impl Platform for Plain {
    const KEEPS_DIRECTORY_TIMES: bool = false;

    type Durable = ();

    fn durable(&self) {}

    fn spawn_urgent(&self, future: impl Future<Output = ()> + Send + 'static) {
        fasync::Task::spawn(future).detach();
    }
}

#[derive(Clone, Debug)]
enum Op {
    Write(Write),
    /// A truncate to `size`, keeping the last block if `keep_tail` (a DAX file's).
    Resize {
        file: usize,
        size: Size,
        keep_tail: bool,
    },
    /// A clone of `pages` of `src` from page `from`, or to its end if 0, to `dst` at page `to`.
    Clone {
        src: usize,
        from: u64,
        pages: u64,
        dst: usize,
        to: u64,
    },
    Punch {
        file: usize,
        from: u64,
        pages: u64,
    },
    Fsync,
    Remount,
    /// `outer`, and `inner` the `nth` time the model reaches `point` in `outer`, if its locks let
    /// it in, else after.
    During {
        point: Point,
        nth: u8,
        outer: Box<Op>,
        inner: Box<Op>,
    },
}

/// A writeback of `runs`, which become parts of whole pages, but at either end; `dirty`, paid with
/// room reserved for them; failing between its bytes and its commit if `fails`.
#[derive(Clone, Debug)]
struct Write {
    file: usize,
    runs: Vec<Run>,
    dirty: bool,
    fails: bool,
    fill: u8,
}

#[derive(Clone, Copy, Debug)]
struct Run {
    /// Its first page; of those from the file's last on, if `at_end`, where what a cut left meets
    /// what a writeback keeps of a block.
    page: u64,
    at_end: bool,
    pages: u64,
    /// Bytes into its first page it starts at.
    start: u16,
    end: End,
}

#[derive(Clone, Copy, Debug)]
enum End {
    Whole,
    /// So many bytes short of its last page's end.
    Short(u16),
    /// At the file's size, if that is within its last page: a DAX file's last page written back.
    AtSize,
}

#[derive(Clone, Copy, Debug)]
enum Size {
    Bytes(u64),
    /// The file's size, and this many bytes more.
    Near(i16),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Point {
    SealCommit,
    SealEnd,
    Copy,
    Write,
    Reply,
    Commit,
}

impl Point {
    const ALL: [Self; 6] =
        [Self::SealCommit, Self::SealEnd, Self::Copy, Self::Write, Self::Reply, Self::Commit];

    /// Its name, the model's `sim_point`'s.
    fn name(self) -> &'static str {
        match self {
            Self::SealCommit => "a seal's COMMIT",
            Self::SealEnd => "a seal's end",
            Self::Copy => "a Fxfs copy",
            Self::Write => "a Fxfs write",
            Self::Reply => "a Fxfs write's reply",
            Self::Commit => "writeback's commit",
        }
    }

    fn of_seals(self) -> bool {
        matches!(self, Self::SealCommit | Self::SealEnd)
    }

    /// Whether what runs here comes before the operation it fell in, for Fxfs, run one after the
    /// other: before a seal's COMMIT or a writeback's blocks, the seal seals what it does, and a
    /// writeback's extent is minted after the seal; after them, not.
    fn before(self) -> bool {
        !matches!(self, Self::SealEnd | Self::Commit)
    }
}

/// An operation as both sides are asked it, of files by index.
#[derive(Clone)]
enum Call {
    Write { file: usize, writes: Vec<(u64, Vec<u8>)>, dirty: u64, fails: bool },
    WriteInode { file: usize, set: SetAttr },
    Clone { src: usize, offset: u64, len: u64, dst: usize, dst_offset: u64, size: u64 },
    Punch { file: usize, offset: u64, len: u64 },
    Fsync,
    Remount,
}

/// As `Debug`'s, but of a write's bytes their length and first byte alone.
impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Write { file, writes, dirty, fails } => {
                let writes: Vec<_> =
                    writes.iter().map(|(at, data)| (*at, data.len(), data[0])).collect();
                write!(f, "Write {{ file: {file}, (offset, len, first byte): {writes:?}, ")?;
                write!(f, "dirty: {dirty}, fails: {fails} }}")
            }
            Self::WriteInode { file, set } => {
                let keep_tail = set.valid & SET_KEEP_TAIL != 0;
                write!(
                    f,
                    "WriteInode {{ file: {file}, size: {}, keep_tail: {keep_tail} }}",
                    set.size
                )
            }
            Self::Clone { src, offset, len, dst, dst_offset, size } => write!(
                f,
                "Clone {{ src: {src}, offset: {offset}, len: {len}, dst: {dst}, \
                 dst_offset: {dst_offset}, size: {size} }}"
            ),
            Self::Punch { file, offset, len } => {
                write!(f, "Punch {{ file: {file}, offset: {offset}, len: {len} }}")
            }
            Self::Fsync => write!(f, "Fsync"),
            Self::Remount => write!(f, "Remount"),
        }
    }
}

impl Call {
    /// The files whose locks it takes.
    fn files(&self) -> Vec<usize> {
        match self {
            Self::Write { file, .. } | Self::WriteInode { file, .. } | Self::Punch { file, .. } => {
                vec![*file]
            }
            Self::Clone { src, dst, .. } => vec![*src, *dst],
            Self::Fsync | Self::Remount => vec![],
        }
    }

    /// Whether the model's locks let it run at `point` in `outer`, and the sequence means it to.
    fn may_run_in(&self, point: Point, outer: &Self) -> bool {
        match (point.of_seals(), outer) {
            // A seal holds `writes` for writing: no writeback, nor another seal.
            (true, Self::Fsync) => {
                matches!(self, Self::WriteInode { .. } | Self::Clone { .. } | Self::Punch { .. })
            }
            // A writeback holds its file, and from its write to its reply, `writes` for reading.
            (false, Self::Write { file, fails, .. }) => {
                let theirs = self.files().contains(file);
                match self {
                    Self::Fsync => matches!(point, Point::Copy | Point::Commit),
                    // A failing writeback's fails at its commit, which this would take.
                    Self::Write { fails: false, .. } => !theirs && !fails,
                    Self::WriteInode { .. } | Self::Clone { .. } | Self::Punch { .. } => !theirs,
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

/// What a call came to, on either side: whether it failed.
type Outcome = Result<(), String>;

/// The bytes a writeback writes at `offset`: of it, by `fill`, and of the place, by the offset.
fn data(fill: u8, offset: u64, len: u64) -> Vec<u8> {
    (offset..offset + len).map(|at| fill ^ (at % 251) as u8).collect()
}

/// `op`, as both sides are asked it, the files `sizes` long: None if it asks nothing.
fn resolve(op: &Op, sizes: &[u64], index: usize) -> Option<Call> {
    let pages_of = |size: u64| size.div_ceil(PAGE);
    match op {
        Op::Write(write) => {
            let size = sizes[write.file];
            let mut runs = write.runs.clone();
            for run in &mut runs {
                run.page = if run.at_end { size / PAGE + run.page % 3 } else { run.page % PAGES };
            }
            runs.sort_by_key(|run| run.page);
            let (mut writes, mut next, mut pages) = (Vec::new(), 0, 0);
            let fill = write.fill.wrapping_add((index as u8).wrapping_mul(29));
            for run in runs {
                // No two share a block.
                if run.page < next {
                    continue;
                }
                // Within the last page, past the end: what a cut kept there meets what a write
                // of part of the block keeps.
                let past_end = size % PAGE;
                let start_in = match u64::from(run.start) {
                    0 => 0,
                    start if run.at_end && run.page == size / PAGE => {
                        past_end + start % (PAGE - past_end)
                    }
                    start => start % PAGE,
                };
                let start = run.page * PAGE + start_in;
                let last = run.page + run.pages - 1;
                let whole = (last + 1) * PAGE;
                let end = match run.end {
                    End::Whole => whole,
                    End::Short(short) => whole - u64::from(short) % PAGE,
                    End::AtSize if size > last * PAGE && size < whole => size,
                    End::AtSize => whole,
                };
                if end <= start {
                    continue;
                }
                writes.push((start, data(fill, start, end - start)));
                pages += last + 1 - run.page;
                next = last + 1;
            }
            let dirty = if write.dirty { pages * PAGE } else { 0 };
            (!writes.is_empty()).then_some(Call::Write {
                file: write.file,
                writes,
                dirty,
                fails: write.fails,
            })
        }
        Op::Resize { file, size, keep_tail } => {
            let size = match size {
                Size::Bytes(bytes) => bytes % (PAGES * PAGE),
                Size::Near(more) => sizes[*file].saturating_add_signed((*more).into()),
            };
            let valid = SET_SIZE | if *keep_tail { SET_KEEP_TAIL } else { 0 };
            Some(Call::WriteInode {
                file: *file,
                set: SetAttr { valid, size, ..SetAttr::default() },
            })
        }
        Op::Clone { src, from, pages, dst, to } => {
            let (src_size, dst_size) = (sizes[*src], sizes[*dst]);
            if src_size == 0 {
                return None;
            }
            let offset = from % pages_of(src_size) * PAGE;
            let whole = (src_size - offset) / PAGE;
            // A clone to the source's end may end within a block, which it takes whole.
            let mut len = if *pages == 0 || whole == 0 {
                src_size - offset
            } else {
                (*pages).min(whole) * PAGE
            };
            let mut dst_offset = to % (pages_of(dst_size) + 8) * PAGE;
            let blocks = len.next_multiple_of(PAGE);
            // The VFS refuses a clone within a file whose ranges overlap.
            if src == dst && dst_offset < offset + blocks && offset < dst_offset + blocks {
                dst_offset = offset + blocks;
            }
            // And a clone of part of a block to before the end of the file it goes to.
            if len % PAGE != 0 && dst_offset + len < dst_size {
                len -= len % PAGE;
            }
            // As data.c asks it: of whole blocks, the size what the bytes reach.
            (len != 0).then_some(Call::Clone {
                src: *src,
                offset,
                len: len.next_multiple_of(PAGE),
                dst: *dst,
                dst_offset,
                size: dst_size.max(dst_offset + len),
            })
        }
        Op::Punch { file, from, pages } => {
            let offset = from % (pages_of(sizes[*file]) + 4) * PAGE;
            Some(Call::Punch { file: *file, offset, len: pages * PAGE })
        }
        Op::Fsync => Some(Call::Fsync),
        Op::Remount => Some(Call::Remount),
        Op::During { .. } => unreachable!("resolved part by part"),
    }
}

/// Has the model do `call`.
fn on_model(model: &Model, inos: &[u64], call: &Call) -> Outcome {
    let errno = |Errno(errno)| format!("errno {errno}");
    match call {
        Call::Write { file, writes, dirty, fails } => {
            let ino = inos[*file];
            if *dirty != 0 {
                model.reserve_dirty(ino, *dirty).map_err(errno)?;
            }
            model.fail_commits(u64::from(*fails));
            let writes: Vec<_> = writes.iter().map(|(at, data)| (*at, &data[..])).collect();
            let written = model.write_many(ino, &writes, *dirty);
            model.fail_commits(0);
            written.map_err(errno)
        }
        Call::WriteInode { file, set } => model.write_inode(inos[*file], set).map_err(errno),
        Call::Clone { src, offset, len, dst, dst_offset, size } => model
            .clone_range(inos[*src], *offset, *len, inos[*dst], *dst_offset, *size)
            .map(drop)
            .map_err(errno),
        Call::Punch { file, offset, len } => model.punch(inos[*file], *offset, *len).map_err(errno),
        Call::Fsync => model.fsync().map_err(errno),
        // The model has no journal: what it holds is what a mount would find.
        Call::Remount => Ok(()),
    }
}

/// Fxfs, mounted, of fxfs-posix.
struct Real {
    filesystem: OpenFxFilesystem,
    fs: Fxfs<Plain>,
}

impl Real {
    async fn new(seed: u64) -> Self {
        let device = DeviceHolder::new(FakeDevice::new(DEVICE_BLOCKS, PAGE as u32).awkward(seed));
        let filesystem = FxFilesystem::new_empty(device).await.expect("formatting");
        let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
        let volume = volumes.new_volume("test", NewChildStoreOptions::default()).await;
        let fs = Fxfs::new((*filesystem).clone(), volume.expect("a volume"), false, Plain);
        Self { filesystem, fs }
    }

    async fn unmount(self) -> DeviceHolder {
        let Self { filesystem, fs } = self;
        fs.stop_delayed().await.expect("stopping");
        drop(fs);
        filesystem.close().await.expect("closing");
        filesystem.take_device().await
    }

    async fn remount(self) -> Self {
        let device = self.unmount().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.expect("opening");
        let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
        let volume = volumes.volume("test", StoreOptions::default()).await.expect("the volume");
        let fs = Fxfs::new((*filesystem).clone(), volume, false, Plain);
        Self { filesystem, fs }
    }

    async fn new_file(&self, name: &str) -> u64 {
        let root = self.fs.root().await.expect("the root").ino;
        let file = self.fs.create(root, name.as_bytes(), 0o100644, 0, 0, 0, &[]).await;
        file.expect("creating")[0].ino
    }

    async fn call(self, inos: &[u64], call: &Call) -> (Self, Outcome) {
        let fs = &self.fs;
        let failed = |error: anyhow::Error| format!("{error:#}");
        let done = match call {
            Call::Write { file, writes, dirty, fails } => {
                let ino = inos[*file];
                if *dirty != 0 {
                    fs.reserve_dirty(ino, *dirty).expect("room for dirty pages");
                }
                FAIL_COMMITS.store(u64::from(*fails), Ordering::Relaxed);
                let mut writes: Vec<_> = writes.iter().map(|(at, data)| (*at, &data[..])).collect();
                let written = fs.write_dirty(ino, &mut writes, *dirty).await;
                FAIL_COMMITS.store(0, Ordering::Relaxed);
                written.map_err(failed)
            }
            Call::WriteInode { file, set } => fs.setattr(inos[*file], set).await.map_err(failed),
            Call::Clone { src, offset, len, dst, dst_offset, size } => {
                let range = *offset..offset + len;
                let cloned = fs.clone_range(inos[*src], range, inos[*dst], *dst_offset, *size);
                cloned.await.map(drop).map_err(failed)
            }
            Call::Punch { file, offset, len } => {
                fs.punch(inos[*file], *offset..offset + len).await.map_err(failed)
            }
            Call::Fsync => fs.fsync().await.map_err(failed),
            Call::Remount => return (self.remount().await, Ok(())),
        };
        (self, done)
    }

    /// The file `ino`'s blocks, by page: the extent, its block, and whether it is open.
    async fn blocks(&self, ino: u64) -> BTreeMap<u64, (ExtentId, u64, bool)> {
        let extents = self.fs.dax_extents(ino, 0..i64::MAX as u64).await.expect("extents");
        let mut blocks = BTreeMap::new();
        for extent in extents {
            let id = ExtentId::new(extent.branch, extent.sequence);
            for page in 0..extent.len / PAGE {
                let block = extent.extent_offset / PAGE + page;
                let open = extent.access == ExtentAccess::Open;
                blocks.insert(extent.offset / PAGE + page, (id, block, open));
            }
        }
        blocks
    }

    async fn read_block(&self, extent: ExtentId, block: u64) -> Vec<u8> {
        let device = self.filesystem.device();
        let mut buffer = device.allocate_buffer(PAGE as usize).await;
        device.read(extent, block * PAGE, buffer.as_mut()).await.expect("reading a block");
        buffer.as_ref().to_vec()
    }
}

/// How often each kind of thing a sequence means to do came about.
#[derive(Default)]
struct Tally {
    interleaved: RefCell<BTreeMap<Point, u64>>,
    failed_commits: Cell<u64>,
    tails_kept: Cell<u64>,
    remounts: Cell<u64>,
}

struct World {
    model: Model,
    real: Real,
    /// Each file's inode on each side.
    model_inos: Vec<u64>,
    real_inos: Vec<u64>,
}

impl World {
    async fn new(seed: u64) -> Self {
        let (model, real) = (Model::new(), Real::new(seed).await);
        let mut world = Self { model, real, model_inos: Vec::new(), real_inos: Vec::new() };
        for file in 0..FILES {
            world.model_inos.push(world.model.new_file());
            world.real_inos.push(world.real.new_file(&format!("file{file}")).await);
        }
        world
    }

    fn sizes(&self) -> Vec<u64> {
        self.model_inos.iter().map(|ino| self.model.size(*ino)).collect()
    }

    /// Has Fxfs do `call`, which the model came to as `expected`.
    async fn real(mut self, call: &Call, expected: Outcome) -> Result<Self, String> {
        let (real, done) = self.real.call(&self.real_inos, call).await;
        self.real = real;
        match (expected, done) {
            (Ok(()), Ok(())) | (Err(_), Err(_)) => Ok(self),
            (model, real) => Err(format!("{call:?}: the model {model:?}, Fxfs {real:?}")),
        }
    }

    /// Has both sides do `call`.
    async fn both(self, call: &Call, tally: &Tally) -> Result<Self, String> {
        tally.note(call);
        let done = on_model(&self.model, &self.model_inos, call);
        self.real(call, done).await
    }

    async fn step(mut self, op: &Op, index: usize, tally: &Tally) -> Result<Self, String> {
        let Op::During { point, nth, outer, inner } = op else {
            let Some(call) = resolve(op, &self.sizes(), 2 * index) else { return Ok(self) };
            return self.both(&call, tally).await;
        };
        let sizes = self.sizes();
        let Some(outer) = resolve(outer, &sizes, 2 * index) else { return Ok(self) };
        let inner = match &**inner {
            // An inner writeback never fails: a failure is the next commit's, whichever's it is.
            Op::Write(write) => Op::Write(Write { fails: false, ..write.clone() }),
            inner => inner.clone(),
        };
        // Of the sizes before `outer`, which changes none of a file `inner` may run on at a point.
        let Some(inner) = resolve(&inner, &sizes, 2 * index + 1) else {
            return self.both(&outer, tally).await;
        };
        tally.note(&outer);
        tally.note(&inner);
        // What ran at the point: `inner`'s outcome.
        let ran: Rc<RefCell<Option<Outcome>>> = Rc::default();
        if inner.may_run_in(*point, &outer) {
            let (point, nth) = (*point, *nth);
            let (ran, inner, inos) = (ran.clone(), inner.clone(), self.model_inos.clone());
            let mut reached = 0;
            self.model.at_points(move |model, what| {
                if what != point.name() || ran.borrow().is_some() {
                    return;
                }
                if reached == nth {
                    *ran.borrow_mut() = Some(on_model(model, &inos, &inner));
                }
                reached += 1;
            });
        }
        let outer_done = on_model(&self.model, &self.model_inos, &outer);
        self.model.at_points(|_, _| {});
        let at_point = ran.borrow_mut().take();
        let interleaved = at_point.is_some();
        let inner_done = match at_point {
            Some(done) => {
                *tally.interleaved.borrow_mut().entry(*point).or_default() += 1;
                done
            }
            None => on_model(&self.model, &self.model_inos, &inner),
        };
        if interleaved && point.before() {
            self = self.real(&inner, inner_done).await?;
            self.real(&outer, outer_done).await
        } else {
            self = self.real(&outer, outer_done).await?;
            self.real(&inner, inner_done).await
        }
    }

    /// Checks that each file is alike on both sides.
    async fn check(&self) -> Result<(), String> {
        for file in 0..FILES {
            self.check_file(file).await.map_err(|error| format!("file {file}: {error}"))?;
        }
        Ok(())
    }

    async fn check_file(&self, file: usize) -> Result<(), String> {
        let (model_ino, real_ino) = (self.model_inos[file], self.real_inos[file]);
        let size = self.model.size(model_ino);
        let real_size = self.real.fs.attr(real_ino).await.expect("attributes").size;
        if size != real_size {
            return Err(format!("size: the model's {size}, Fxfs's {real_size}"));
        }
        let mut read = vec![0; size as usize];
        let got = self.real.fs.read(real_ino, 0, &mut read).await.expect("reading");
        let model_read = self.model.read(model_ino, 0, size as usize);
        if got != size as usize || read != model_read {
            let at = read.iter().zip(&model_read).position(|(a, b)| a != b);
            return Err(format!("read {got} of {size} bytes, first unlike at {at:?}"));
        }
        let mut model_blocks = BTreeMap::new();
        for extent in self.model.extents(model_ino) {
            for page in 0..extent.len / PAGE {
                let block = (extent.sequence, extent.extent_offset / PAGE + page);
                model_blocks
                    .insert(extent.offset / PAGE + page, (block, extent.access == model::OPEN));
            }
        }
        let real_blocks = self.real.blocks(real_ino).await;
        let (model_pages, real_pages): (BTreeSet<_>, BTreeSet<_>) =
            (model_blocks.keys().collect(), real_blocks.keys().collect());
        if model_pages != real_pages {
            return Err(format!(
                "pages with blocks: the model's {model_pages:?}, Fxfs's {real_pages:?}"
            ));
        }
        for (page, (extent, block, open)) in &real_blocks {
            if *open && !model_blocks[page].1 {
                return Err(format!("page {page}'s extent: open in Fxfs, sealed in the model's"));
            }
            let real = self.real.read_block(*extent, *block).await;
            if real != self.model.read(model_ino, page * PAGE, PAGE as usize) {
                return Err(format!("page {page}'s block, whole, differs"));
            }
        }
        Ok(())
    }

    /// Checks that the files share blocks alike: pages that share one on one side do on the other.
    async fn check_sharing(&self) -> Result<(), String> {
        let mut model: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        let mut real: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for file in 0..FILES {
            for extent in self.model.extents(self.model_inos[file]) {
                for page in 0..extent.len / PAGE {
                    let block = (extent.sequence, extent.extent_offset / PAGE + page);
                    model.entry(block).or_default().insert((file, extent.offset / PAGE + page));
                }
            }
            for (page, (extent, block, _)) in self.real.blocks(self.real_inos[file]).await {
                real.entry((extent, block)).or_default().insert((file, page));
            }
        }
        let shared = |blocks: Vec<BTreeSet<_>>| -> BTreeSet<_> {
            blocks.into_iter().filter(|pages| pages.len() > 1).collect()
        };
        let (model, real) =
            (shared(model.into_values().collect()), shared(real.into_values().collect()));
        if model != real {
            return Err(format!("blocks shared: the model's {model:?}, Fxfs's {real:?}"));
        }
        Ok(())
    }

    async fn end(self) -> Result<(), String> {
        for ino in &self.model_inos {
            let reserved = self.model.reserved(*ino);
            if reserved != 0 {
                return Err(format!("the model holds {reserved} bytes of room for dirty pages"));
            }
        }
        self.real.unmount().await;
        Ok(())
    }
}

impl Tally {
    fn note(&self, call: &Call) {
        match call {
            Call::Write { fails: true, .. } => {
                self.failed_commits.set(self.failed_commits.get() + 1)
            }
            Call::WriteInode { set, .. } if set.valid & SET_KEEP_TAIL != 0 => {
                self.tails_kept.set(self.tails_kept.get() + 1)
            }
            Call::Remount => self.remounts.set(self.remounts.get() + 1),
            _ => {}
        }
    }
}

/// Runs `ops` on both sides, Fxfs's device as awkward as `seed` says, checking after each.
fn run(seed: u64, ops: &[Op], tally: &Tally) -> Result<(), String> {
    fasync::LocalExecutorBuilder::new().build().run_singlethreaded(async {
        let mut world = World::new(seed).await;
        for (index, op) in ops.iter().enumerate() {
            world = world.step(op, index, tally).await.map_err(|e| format!("op {index}: {e}"))?;
            world.check().await.map_err(|e| format!("after op {index}, {op:?}: {e}"))?;
            world.check_sharing().await.map_err(|e| format!("after op {index}, {op:?}: {e}"))?;
        }
        world = world.step(&Op::Fsync, ops.len(), tally).await?;
        world.check().await.map_err(|e| format!("after the last fsync: {e}"))?;
        world.end().await
    })
}

fn file() -> impl Strategy<Value = usize> {
    0..FILES
}

fn write() -> impl Strategy<Value = Write> {
    let end = prop_oneof![
        4 => Just(End::Whole),
        2 => (1..PAGE as u16).prop_map(End::Short),
        3 => Just(End::AtSize),
    ];
    // Writeback's start at a page's, but a write of part of the last block past the end, which
    // keeps what is before it up to the end alone.
    let start = prop_oneof![3 => Just(0u16), 1 => 1..PAGE as u16];
    let pages = prop_oneof![8 => 1..4u64, 2 => 4..16u64, 1 => 60..80u64];
    let at_end = prop::bool::weighted(0.3);
    let run = (0..PAGES, at_end, pages, start, end)
        .prop_map(|(page, at_end, pages, start, end)| Run { page, at_end, pages, start, end });
    let fails = prop::bool::weighted(0.1);
    (file(), prop::collection::vec(run, 1..4), any::<bool>(), fails, any::<u8>())
        .prop_map(|(file, runs, dirty, fails, fill)| Write { file, runs, dirty, fails, fill })
}

fn simple() -> impl Strategy<Value = Op> {
    let size = prop_oneof![
        (0..PAGES * PAGE).prop_map(Size::Bytes),
        (0..PAGES).prop_map(|page| Size::Bytes(page * PAGE)),
        (-5000i16..5000).prop_map(Size::Near),
    ];
    prop_oneof![
        6 => write().prop_map(Op::Write),
        4 => (file(), size, any::<bool>())
            .prop_map(|(file, size, keep_tail)| Op::Resize { file, size, keep_tail }),
        3 => (file(), 0..PAGES, prop_oneof![1 => Just(0u64), 3 => 1..9u64], file(), 0..PAGES + 8)
            .prop_map(|(src, from, pages, dst, to)| Op::Clone { src, from, pages, dst, to }),
        2 => (file(), 0..PAGES + 4, 1..17u64)
            .prop_map(|(file, from, pages)| Op::Punch { file, from, pages }),
        2 => Just(Op::Fsync),
        1 => Just(Op::Remount),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let point = prop::sample::select(Point::ALL.to_vec());
    // A cut near the end, which writes the last block again unless it keeps it: what a seal's
    // points reach, as a writeback can't.
    let cut = (file(), -4096i16..0, any::<bool>()).prop_map(|(file, by, keep_tail)| Op::Resize {
        file,
        size: Size::Near(by),
        keep_tail,
    });
    let inner = prop_oneof![2 => simple(), 1 => cut];
    let during = (point, 0..4u8, write(), inner).prop_map(|(point, nth, write, inner)| {
        let outer = if point.of_seals() { Op::Fsync } else { Op::Write(write) };
        Op::During { point, nth, outer: Box::new(outer), inner: Box::new(inner) }
    });
    prop_oneof![8 => simple(), 3 => during]
}

#[test]
fn the_sims_model_does_as_fxfs_does() {
    let cases = std::env::var("FXDAX_SIM_DIFF_CASES").map_or(64, |cases| cases.parse().unwrap());
    let most: usize = std::env::var("FXDAX_SIM_DIFF_OPS").map_or(48, |most| most.parse().unwrap());
    let config = Config { cases, failure_persistence: None, ..Config::default() };
    let sequences = (any::<u64>(), prop::collection::vec(op(), 1..most));
    let tally = Tally::default();
    TestRunner::new(config)
        .run(&sequences, |(seed, ops)| {
            prop_assert_eq!(run(seed, &ops, &tally), Ok(()));
            Ok(())
        })
        .unwrap();
    let interleaved = tally.interleaved.borrow();
    println!(
        "interleaved {interleaved:?}, {} commits failed, {} tails kept, {} remounts",
        tally.failed_commits.get(),
        tally.tails_kept.get(),
        tally.remounts.get()
    );
    for point in Point::ALL {
        assert!(interleaved.get(&point).is_some_and(|n| *n > 0), "nothing ran at {point:?}");
    }
    assert!(tally.failed_commits.get() > 0, "no commit failed");
    assert!(tally.tails_kept.get() > 0, "no cut kept its tail");
    assert!(tally.remounts.get() > 0, "no remount");
}

/// A writeback of one run of `pages` pages from `page`, `start` bytes into its first and ending
/// as `end` says.
fn write_run(file: usize, page: u64, pages: u64, start: u16, end: End) -> Op {
    let run = Run { page, at_end: false, pages, start, end };
    Op::Write(Write { file, runs: vec![run], dirty: true, fails: false, fill: 7 })
}

/// The model's drifts from Fxfs found so far, each as a sequence that shows it whatever the seed,
/// beside the random ones that found them: every growth zeroing what a cut that kept the tail left
/// past the end, and a write of part of the block keeping it up to the end alone (TASK-149.14.2);
/// a seal forgetting which extents are open only after its COMMIT sealed them, for a truncate at
/// its end to write one (seed 32826); and a cut moving a last block holding zeros past the end,
/// which Fxfs leaves (TASK-149.29).
#[test]
fn the_models_past_drifts_from_fxfs() {
    let two_pages = || write_run(0, 0, 2, 0, End::Whole);
    let cut = |size, keep_tail| Op::Resize { file: 0, size: Size::Bytes(size), keep_tail };
    let sequences = [
        (
            "a write past a kept tail",
            vec![two_pages(), cut(1000, true), write_run(0, 16, 1, 0, End::Whole)],
        ),
        (
            "a write in a kept tail's block, past the end",
            vec![two_pages(), cut(1000, true), write_run(0, 0, 1, 1999, End::Short(2096))],
        ),
        (
            "a write of a kept tail's block's start",
            vec![two_pages(), cut(1000, true), write_run(0, 0, 1, 0, End::Short(3596))],
        ),
        (
            "a cut at a seal's end",
            vec![
                two_pages(),
                Op::During {
                    point: Point::SealEnd,
                    nth: 0,
                    outer: Box::new(Op::Fsync),
                    inner: Box::new(cut(1000, false)),
                },
            ],
        ),
        (
            "a growth past a tail of zeros, shared",
            vec![
                write_run(0, 0, 1, 0, End::Short(1)),
                Op::Clone { src: 0, from: 0, pages: 0, dst: 1, to: 0 },
                write_run(0, 1, 4, 0, End::Whole),
            ],
        ),
    ];
    let tally = Tally::default();
    for (what, ops) in sequences {
        assert_eq!(run(0, &ops, &tally), Ok(()), "{what}");
    }
}
