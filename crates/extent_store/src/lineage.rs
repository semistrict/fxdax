// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A store's branches (see the crate's docs). Their state has three owners: their records
//! ([`BranchRecords`]), which write them; the ledger of which branches hold each file of extents'
//! data ([`Ledger`]), which is memory alone; and the lineage, which does the rest of the I/O: it
//! opens the store, building the layers forks are over, their parents' states at the forks, from
//! the index files their parents keep (TASK-31), and removes the index files deleted branches
//! leave, and the files the ledger says may go.
//!
//! The records' lock and the ledger's each cover what they guard, in memory: neither is held for
//! I/O, nor taken holding the other. A store takes them holding its own state's lock, never the
//! other way round, so that no branch's commit waits for another's records, or its index files, to
//! be written or removed. Two other locks of the lineage's are held across I/O: `holds` while the
//! store's directory is taken ([`Dir::take`]), as its count and the taking change together; and
//! `files` while a segment's file opens (the 2026-10-21 review's performance issue 4).

use crate::branch_records::{BranchRecords, write_record};
use crate::dir::{Dir, File as _, read_file};
use crate::framed::Framed;
use crate::index::{layers_at, recover, write_checkpoint};
use crate::layers::{Holdings, Layer};
use crate::layout::{
    BRANCHES, BranchRecord, DESCRIPTION, Description, EXTENTS, FileKey, INDEX, OPEN_EXTENTS,
    OPEN_SEGMENTS, Origin, SEGMENTS, SegmentId, StoreFile, branch_dir, create_branch_dirs,
    index_path, list_numbers, parse_number, segment_path,
};
use crate::ledger::{Ledger, Live, Over};
use crate::store::{Options, Store};
use crate::sync::{Mutex, MutexGuard};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use storage_device::ExtentId;

/// A store's directory and its branches: a lineage, whose branches share the files of the extents
/// they inherited from each other. Each branch is opened as a [`Store`] of its own, at most once
/// at a time, and only by one process: the branches' stores in a process share the lineage.
pub struct Lineage<D: Dir> {
    pub(crate) dir: D,
    pub(crate) block_size: u32,
    pub(crate) options: Options,
    /// Its branches' records, which their stores and their deletions change.
    records: BranchRecords,
    /// Which branches hold each file of extents' data, and which files may go.
    ledger: Mutex<Ledger>,
    /// What holds the store's directory taken for this process ([`Dir::take`]): its branches
    /// open, and its deletions under way.
    holds: Mutex<usize>,
    /// The parts of its files read lately, unpacked.
    pub(crate) unpacked: crate::framed::Unpacked,
    /// The closed segments' files its branches read what they inherited from, open: one for all
    /// the forks reading it, the most `Options::open_files` but for those reads use (TASK-34).
    files: Mutex<HashMap<SegmentId, Arc<Framed<D::File>>>>,
}

/// The layers forks are over, by where they were forked: the parent, its index file, and its
/// epoch.
type Layers = HashMap<(u64, u64, u64), Arc<Layer>>;

impl<D: Dir> Lineage<D> {
    /// Makes a new store in `dir`, which is empty, of one branch, as `description` describes, and
    /// opens it.
    pub fn create(dir: D, description: Description, options: Options) -> Result<Arc<Self>, Error> {
        let Description { block_size, capacity, branch } = description;
        ensure!(
            block_size.is_power_of_two() && block_size as usize <= crate::framed::ALIGNMENT,
            "bad block size {block_size}"
        );
        ensure!(capacity >= block_size as u64, "the store is smaller than a block");
        dir.take().context("taking the store's directory")?;
        for path in [BRANCHES, SEGMENTS, EXTENTS, INDEX, OPEN_SEGMENTS, OPEN_EXTENTS] {
            dir.create_dir(path).with_context(|| format!("creating {path}"))?;
        }
        create_branch_dirs(&dir, branch)?;
        write_checkpoint(&dir, branch, 0, &Default::default(), None)?;
        let record =
            BranchRecord { capacity, origin: None, resolved: true, layered: false, deleted: false };
        write_record(&dir, branch, 0, &record)?;
        // The description is last: until it is durable, the directory is no store.
        let file = dir.create(DESCRIPTION, false)?;
        file.write_at(StoreFile { block_size, first: branch }.text().as_bytes(), 0)?;
        file.sync()?;
        dir.sync_dir("")?;
        dir.barrier()?;
        Self::open(dir, options)
    }

    /// Opens the store in `dir`, each of its branches as of its last commit that is durable, and
    /// removes the files of extents' data no branch holds an extent in: what commits that weren't
    /// durable wrote, or that were to go once the commits that let them go were. It fails while
    /// another process has branches of the store open ([`Dir::take`]).
    pub fn open(dir: D, options: Options) -> Result<Arc<Self>, Error> {
        dir.take().context("taking the store's directory")?;
        // Dropped, the directory is let go of: closed.
        let lineage = Self::open_taken(dir, options)?;
        lineage.dir.let_go();
        Ok(lineage)
    }

    /// [`Lineage::open`], with the store's directory taken.
    fn open_taken(dir: D, options: Options) -> Result<Arc<Self>, Error> {
        let StoreFile { block_size, first } = {
            let file = dir.open(DESCRIPTION, false).context("opening the store's description")?;
            let text = String::from_utf8(read_file(&file)?).context("reading the description")?;
            StoreFile::parse(&text)?
        };

        // Each branch's newest record that is whole; older versions go.
        let records = BranchRecords::read(&dir, first)?;
        let read = records.snapshot();
        // The layers forks are over, their parents' states at the forks: each index file they are
        // in read once, for all the forks of it (TASK-31).
        let layers = build_layers(&dir, &read)?;
        // What each branch holds: its own state, over the layer it is over, if a fork's.
        let mut live = Vec::new();
        for (&branch, record) in read.iter().filter(|(_, record)| !record.deleted) {
            let over = match record.over_parent() {
                Some(origin) => Some(Over {
                    origin,
                    layer: (layers.get(&(origin.parent, origin.index, origin.epoch)).cloned())
                        .ok_or_else(|| anyhow!("branch {branch:#x} has no parent's state"))?,
                }),
                None => None,
            };
            let recovered = match (record.resolved, record.origin) {
                (true, _) => {
                    let base = over.as_ref().map(|over| over.layer.clone());
                    let recovered = recover(&dir, branch, base)?;
                    Some((recovered.epoch, recovered.extents.holdings()))
                }
                (false, Some(_)) => None,
                (false, None) => bail!("branch {branch:#x} is no fork, and has no index"),
            };
            live.push(Live { branch, over, recovered });
        }
        let ledger = Ledger::opened(live);

        // Deleted branches go, but for their index files forks need, and the first's record, which
        // says the store was deleted.
        let pinned = records.pinned_indexes();
        for (&branch, _) in read.iter().filter(|(_, record)| record.deleted) {
            let keep = pinned.get(&branch).cloned().unwrap_or_default();
            remove_branch_index(&dir, branch, &keep)?;
            if keep.is_empty() && branch != first {
                records.forget(&dir, branch)?;
            }
        }

        // The files of extents' data no branch claims, and the raw files of extents open, which
        // no commit made durable refers to: they are framed once sealed.
        for kind in [SEGMENTS, EXTENTS, OPEN_SEGMENTS, OPEN_EXTENTS] {
            for branch_name in dir.list(kind)? {
                let branch = parse_number(&branch_name)
                    .ok_or_else(|| anyhow!("{kind}/{branch_name} is no branch's"))?;
                for name in dir.list(&branch_dir(kind, branch))? {
                    let number = parse_number(&name)
                        .ok_or_else(|| anyhow!("{kind}/{branch_name}/{name} is no file of its"))?;
                    let claimed = match kind {
                        SEGMENTS | OPEN_SEGMENTS => {
                            ledger.held(FileKey::Segment(SegmentId { branch, number }))
                        }
                        EXTENTS => ledger.held(FileKey::Own(ExtentId::new(branch, number))),
                        _ => false,
                    };
                    if !claimed {
                        let path = format!("{}/{name}", branch_dir(kind, branch));
                        dir.remove(&path).with_context(|| format!("removing {path}"))?;
                    }
                }
            }
        }
        dir.barrier()?;
        Ok(Arc::new(Self {
            dir,
            block_size,
            options,
            records,
            ledger: Mutex::new(ledger),
            holds: Mutex::new(0),
            unpacked: Default::default(),
            files: Mutex::new(HashMap::new()),
        }))
    }

    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap()
    }

    /// The store's directory.
    pub fn dir(&self) -> &D {
        &self.dir
    }

    /// The framed file of `segment`, closed, which its branches share: opened if it isn't; the
    /// files no read uses closing, once more than `Options::open_files` are open.
    pub(crate) fn segment_file(&self, segment: SegmentId) -> Result<Arc<Framed<D::File>>, Error> {
        let mut files = self.files.lock().unwrap();
        if let Some(file) = files.get(&segment) {
            return Ok(file.clone());
        }
        let path = segment_path(segment);
        let file = Arc::new(Framed::open(self.dir.open(&path, true)?, &path)?);
        if files.len() >= self.options.open_files {
            files.retain(|_, file| Arc::strong_count(file) > 1);
        }
        files.insert(segment, file.clone());
        Ok(file)
    }

    /// The branch opening the store, rather than one of its branches, opens.
    pub fn first(&self) -> u64 {
        self.records.first()
    }

    /// Whether the store was deleted ([`Lineage::delete`] of its first branch): its directory
    /// holds its forks' data alone, which they share.
    pub fn is_deleted(&self) -> bool {
        self.records.is_deleted()
    }

    /// Whether the store has `branch`, not deleted.
    pub fn has(&self, branch: u64) -> bool {
        self.records.has(branch)
    }

    /// Whether a store of this process has `branch` open.
    pub fn is_open(&self, branch: u64) -> bool {
        self.records.is_open(branch)
    }

    /// Runs `f` with the store's directory taken for this process ([`Dir::take`]): failing, after
    /// a while, if another process has branches of it open.
    pub fn holding<R>(&self, f: impl FnOnce() -> Result<R, Error>) -> Result<R, Error> {
        self.hold()?;
        let result = f();
        self.let_go();
        result
    }

    /// Holds the store's directory taken for this process, taking it if nothing held it.
    fn hold(&self) -> Result<(), Error> {
        let mut holds = self.holds.lock().unwrap();
        if *holds == 0 {
            self.dir.take().context("taking the store's directory")?;
        }
        *holds += 1;
        Ok(())
    }

    /// Lets a hold of the store's directory go, and the directory, with the last.
    fn let_go(&self) {
        let mut holds = self.holds.lock().unwrap();
        *holds -= 1;
        if *holds == 0 {
            self.dir.let_go();
        }
    }

    /// The branches, with the branch each was forked from, if any.
    pub fn branches(&self) -> Vec<(u64, Option<u64>)> {
        self.records.branches()
    }

    /// Opens `branch`, which no store has open, taking the store's directory for this process while
    /// it is open ([`Dir::take`]).
    pub fn open_branch(self: &Arc<Self>, branch: u64) -> Result<Store<D>, Error> {
        self.hold()?;
        let opened = self.open_held(branch);
        if opened.is_err() {
            self.let_go();
        }
        opened
    }

    fn open_held(self: &Arc<Self>, branch: u64) -> Result<Store<D>, Error> {
        let (token, record) = self.records.open(branch)?;
        Store::open_in(self.clone(), branch, token, record).inspect_err(|_| {
            // Not open, which the caller lets go of.
            self.records.closed(branch, token);
        })
    }

    /// Deletes `branch`, which no store has open: the files of its extents go once no other branch
    /// holds an extent in them. Deleting the first branch deletes the store, whose forks live on:
    /// what they share of it stays, in its directory, until they go too (TASK-39.4).
    pub fn delete(&self, branch: u64) -> Result<(), Error> {
        self.holding(|| self.delete_held(branch))
    }

    fn delete_held(&self, branch: u64) -> Result<(), Error> {
        let removed = {
            let deletion = self.records.delete(branch)?;
            deletion.mark(&self.dir)?;
            // Its record says it is deleted, durably: what it held, it lets go of at once.
            self.ledger().detach(branch);
            let pinned = deletion.pinned();
            remove_branch_index(&self.dir, branch, &pinned)?;
            if pinned.is_empty() && branch != self.first() {
                self.ledger().forget(branch);
                deletion.forget(&self.dir)?;
            }
            // Its ID reserved still: no fork to it yet has what it let go of wait for its commits.
            self.ledger().removable()
        };
        self.remove(removed);
        Ok(())
    }

    /// Records a fork of `origin.parent`, which a store has open, as of `origin`: a new branch,
    /// `child`, of `capacity` bytes, over `layer`, its parent's state at the fork (TASK-31), which
    /// it holds the files of, and its index will hold what it changes over.
    pub(crate) fn fork(
        &self,
        origin: Origin,
        child: u64,
        capacity: u64,
        layer: Arc<Layer>,
    ) -> Result<(), Error> {
        let record = BranchRecord {
            capacity,
            origin: Some(origin),
            resolved: false,
            layered: true,
            deleted: false,
        };
        let forking = self.records.fork(child, record)?;
        forking.write(&self.dir)?;
        // It holds its layer's files, which its parent holds until its next commit, before it is
        // found.
        self.ledger().attach(child, Over { origin, layer });
        forking.found();
        Ok(())
    }

    /// Records that `branch`, a fork, has an index: of what it changed over its parent's state at
    /// the fork, whose index file its parent keeps for it while it lives. For the store of its
    /// opening `token`.
    pub(crate) fn resolved(&self, branch: u64, token: u64) -> Result<(), Error> {
        let resolved = |record| BranchRecord { resolved: true, layered: true, ..record };
        self.records.change(&self.dir, branch, token, resolved)
    }

    /// The layer `branch` is over, its parent's state at the fork, if a fork's over it.
    pub(crate) fn base(&self, branch: u64) -> Option<Arc<Layer>> {
        self.ledger().base(branch)
    }

    /// `branch`'s state at its latest fork of those live, which that fork is over, if any.
    pub(crate) fn latest_layer_of(&self, branch: u64) -> Option<Arc<Layer>> {
        self.ledger().latest_layer_of(branch)
    }

    /// Sets `branch`'s capacity, durably, in its record ([`Store::set_capacity`]), for the store
    /// of its opening `token`.
    pub(crate) fn set_capacity(&self, branch: u64, token: u64, capacity: u64) -> Result<(), Error> {
        let sized = |record| BranchRecord { capacity, ..record };
        self.records.change(&self.dir, branch, token, sized)
    }

    /// The index files of `branch` that forks need: those of the states they are over.
    pub(crate) fn pinned(&self, branch: u64) -> BTreeSet<u64> {
        self.records.pinned(branch)
    }

    /// Records that the store of `branch`'s opening `token` closed it, which lets go of the
    /// store's directory, if it was the last open.
    pub(crate) fn closed(&self, branch: u64, token: u64) {
        if self.records.closed(branch, token) {
            self.let_go();
        }
    }

    /// Records that `branch` holds extents in `files`.
    pub(crate) fn claim(&self, branch: u64, files: impl IntoIterator<Item = FileKey>) {
        self.ledger().claim(branch, files);
    }

    /// Records that `branch` holds extents in exactly `holdings.own` of its own, and none in
    /// `holdings.dropped` of the layer it is over, as its state, just recovered as of its durable
    /// `epoch`, says; and removes the files that can go now.
    pub(crate) fn recovered(&self, branch: u64, epoch: u64, holdings: &Holdings) {
        let removed = {
            let mut ledger = self.ledger();
            ledger.recovered(branch, epoch, holdings);
            ledger.removable()
        };
        self.remove(removed);
    }

    /// Records that `branch`'s commit `epoch` lets `files` go: they go once no branch holds an
    /// extent in them, and the commits that let them go are durable.
    pub(crate) fn release(&self, branch: u64, epoch: u64, files: Vec<FileKey>) {
        // Most commits let nothing go.
        if files.is_empty() {
            return;
        }
        self.ledger().release(branch, epoch, files);
    }

    /// Whether a branch other than `branch` holds an extent in `file`: of its own, or through the
    /// layer it is over.
    pub(crate) fn shared(&self, branch: u64, file: FileKey) -> bool {
        self.ledger().shared(branch, file)
    }

    /// Records that `branch`'s commits up to `epoch` are durable, and removes the files that can go
    /// now.
    pub(crate) fn durable(&self, branch: u64, epoch: u64) {
        let removed = {
            let mut ledger = self.ledger();
            ledger.durable(branch, epoch);
            ledger.removable()
        };
        self.remove(removed);
    }

    fn remove(&self, files: Vec<FileKey>) {
        for path in files.iter().flat_map(FileKey::paths) {
            if let Err(err) = self.dir.remove(&path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                // Opening the store removes it, as no branch claims it.
                log::warn!("extent store: failed to remove {path}: {err}");
            }
        }
    }
}

/// The layers live forks are over, by where they were forked (TASK-31): each parent's state at each
/// fork point of its live forks, and at the points of its own parent's it is over, up their lines,
/// each index file they are in read once, for all the forks of it. `records` are the branches'.
fn build_layers<D: Dir>(dir: &D, records: &BTreeMap<u64, BranchRecord>) -> Result<Layers, Error> {
    let mut wanted: BTreeMap<(u64, u64), BTreeSet<u64>> = BTreeMap::new();
    for record in records.values().filter(|record| !record.deleted) {
        let mut record = *record;
        while let Some(origin) = record.over_parent() {
            if !wanted.entry((origin.parent, origin.index)).or_default().insert(origin.epoch) {
                break;
            }
            let parent = records.get(&origin.parent);
            record = *parent.ok_or_else(|| anyhow!("branch {:#x} has no record", origin.parent))?;
        }
    }
    let mut built = HashMap::new();
    for &(parent, index) in wanted.keys() {
        build_layers_of(dir, records, &wanted, parent, index, &mut built)?;
    }
    Ok(built)
}

/// Builds the layers `wanted` names of `parent`'s index file `index`, and those they are over.
fn build_layers_of<D: Dir>(
    dir: &D,
    records: &BTreeMap<u64, BranchRecord>,
    wanted: &BTreeMap<(u64, u64), BTreeSet<u64>>,
    parent: u64,
    index: u64,
    built: &mut Layers,
) -> Result<(), Error> {
    let epochs = &wanted[&(parent, index)];
    if built.contains_key(&(parent, index, *epochs.first().unwrap())) {
        return Ok(());
    }
    let base = match records[&parent].over_parent() {
        Some(origin) => {
            let key = (origin.parent, origin.index, origin.epoch);
            if !built.contains_key(&key) {
                build_layers_of(dir, records, wanted, origin.parent, origin.index, built)?;
            }
            Some(built[&key].clone())
        }
        None => None,
    };
    let layers = layers_at(dir, parent, index, epochs, base)
        .with_context(|| format!("branch {parent:#x}'s states at its forks"))?;
    for (epoch, layer) in layers {
        built.insert((parent, index, epoch), layer);
    }
    Ok(())
}

/// Removes `branch`'s index files, but for those numbered in `keep`.
fn remove_branch_index<D: Dir>(dir: &D, branch: u64, keep: &BTreeSet<u64>) -> Result<(), Error> {
    let path = branch_dir(INDEX, branch);
    match dir.list(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
        Ok(_) => {}
    }
    for number in list_numbers(dir, &path)? {
        if !keep.contains(&number) {
            dir.remove(&index_path(branch, number))?;
        }
    }
    dir.sync_dir(&path)?;
    Ok(())
}

/// The lineage's tests under Shuttle (the `shuttle` feature), which only they can run:
/// `cargo nextest run -p extent_store --features shuttle --lib shuttle_tests::`. A branch's record
/// written, or a deletion's index files removed, stalls in its file system, as on a slow disk, for
/// as long as a test says. Shuttle, whose waits that can't end are deadlocks it reports, shows
/// that the other branches go on meanwhile; and that the openings, forks, deletions and lists of
/// branches that wait for a stalled fork or deletion act on what came of it, which the tests that
/// turn [`Rules::wait_for_forks_and_deletions`] off show to need the wait.
#[cfg(all(test, feature = "shuttle"))]
mod shuttle_tests {
    use super::*;
    use crate::layout::record_path;
    use crate::mem::{MemDir, MemFile};
    use crate::sync::{Condvar, Rules, set_rules, thread};
    use shuttle::scheduler::{PctScheduler, RandomScheduler};
    use shuttle::sync::atomic::{AtomicBool, Ordering};
    use std::io;

    /// The operations of a file system a test stalls.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Op {
        Create,
        SyncDir,
    }

    #[derive(Default)]
    struct Stall {
        /// The operation and path whose next call stalls.
        at: Option<(Op, String)>,
        /// Whether a call is stalled.
        stalled: bool,
        /// What the stalled call gives, once the test lets it go.
        outcome: Option<io::Result<()>>,
    }

    /// A [`MemDir`] whose next call of an operation on a path, once armed, stalls until the test
    /// lets it go, saying whether it fails: on Shuttle's condition variable, so that Shuttle
    /// schedules the other threads meanwhile.
    #[derive(Clone)]
    struct Stalling {
        dir: MemDir,
        stall: Arc<(Mutex<Stall>, Condvar)>,
    }

    impl Stalling {
        fn new() -> Self {
            Self {
                dir: MemDir::new(),
                stall: Arc::new((Mutex::new(Stall::default()), Condvar::new())),
            }
        }

        /// Has the next call of `op` on `path` stall.
        fn arm(&self, op: Op, path: &str) {
            self.stall.0.lock().unwrap().at = Some((op, path.to_owned()));
        }

        /// Waits until the call armed is stalled.
        fn wait_until_stalled(&self) {
            let (stall, changed) = &*self.stall;
            let mut stall = stall.lock().unwrap();
            while !stall.stalled {
                stall = changed.wait(stall).unwrap();
            }
        }

        /// Lets the call stalled go on, giving `outcome`: if an error, it fails without doing
        /// anything.
        fn let_go(&self, outcome: io::Result<()>) {
            let (stall, changed) = &*self.stall;
            stall.lock().unwrap().outcome = Some(outcome);
            changed.notify_all();
        }

        /// Stalls a call of `op` on `path`, if armed for it, until the test lets it go, and gives
        /// what the test said.
        fn pass(&self, op: Op, path: &str) -> io::Result<()> {
            let (stall, changed) = &*self.stall;
            let mut stall = stall.lock().unwrap();
            if stall.at.as_ref().is_none_or(|(at, at_path)| (*at, at_path.as_str()) != (op, path)) {
                return Ok(());
            }
            stall.at = None;
            stall.stalled = true;
            changed.notify_all();
            while stall.outcome.is_none() {
                stall = changed.wait(stall).unwrap();
            }
            stall.stalled = false;
            stall.outcome.take().unwrap()
        }
    }

    impl Dir for Stalling {
        type File = MemFile;

        fn create(&self, path: &str, data: bool) -> io::Result<MemFile> {
            self.pass(Op::Create, path)?;
            self.dir.create(path, data)
        }

        fn open(&self, path: &str, data: bool) -> io::Result<MemFile> {
            self.dir.open(path, data)
        }

        fn remove(&self, path: &str) -> io::Result<()> {
            self.dir.remove(path)
        }

        fn create_dir(&self, path: &str) -> io::Result<()> {
            self.dir.create_dir(path)
        }

        fn list(&self, path: &str) -> io::Result<Vec<String>> {
            self.dir.list(path)
        }

        fn sync_dir(&self, path: &str) -> io::Result<()> {
            self.pass(Op::SyncDir, path)?;
            self.dir.sync_dir(path)
        }

        fn barrier(&self) -> io::Result<()> {
            self.dir.barrier()
        }
    }

    const BLOCK: usize = 4096;
    /// The store's first branch, which the others are forks of.
    const PARENT: u64 = 1;
    /// Its forks: one deleted, or opened for the first time, as a test says; the sibling, which
    /// goes on meanwhile; and another, which forks or changes its capacity.
    const FORK: u64 = 2;
    const SIBLING: u64 = 3;
    const OTHER: u64 = 4;
    /// The branch `OTHER` forks to; and, first, the sibling, in a test of two forks to one branch.
    const CHILD: u64 = 5;
    /// Small segments, and a checkpoint at each commit, which asks which index files forks need.
    const OPTIONS: Options = Options {
        segment_size: 2 * BLOCK as u64,
        segment_age: std::time::Duration::from_secs(3600),
        large_extent: 1 << 20,
        checkpoint_after: 1,
        open_files: 2,
        view_lag: None,
        view_kept_bytes: None,
    };

    /// The parent's extent its forks inherit.
    fn inherited() -> ExtentId {
        ExtentId::new(PARENT, 1)
    }

    /// A store of the parent, closed, which wrote an extent, and forked to each of `forks`, none of
    /// them opened yet.
    fn forked(dir: &Stalling, forks: &[u64]) -> Arc<Lineage<Stalling>> {
        let description =
            Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: PARENT };
        let lineage = Lineage::create(dir.clone(), description, OPTIONS).unwrap();
        let parent = lineage.open_branch(PARENT).unwrap();
        parent.write(inherited(), 0, &[1; BLOCK]).unwrap();
        for &fork in forks {
            parent.fork(fork).unwrap();
        }
        drop(parent);
        lineage
    }

    /// What the sibling reads once it has gone on: the extent it wrote, and the one it inherited.
    type Reads = (Vec<u8>, Vec<u8>);

    /// The sibling's work: it writes, frees, commits, a checkpoint each time, which asks which of
    /// its index files forks need, makes its commits durable, which removes the files they let
    /// go of, and cleans; then reads what it holds.
    fn sibling_works(sibling: &Store<Stalling>) -> Reads {
        let (kept, freed) = (ExtentId::new(SIBLING, 1), ExtentId::new(SIBLING, 2));
        sibling.write(kept, 0, &[3; BLOCK]).unwrap();
        sibling.write(freed, 0, &[4; BLOCK]).unwrap();
        sibling.commit(None).unwrap();
        sibling.free(freed).unwrap();
        sibling.commit_ordered(None).unwrap();
        sibling.flush().unwrap();
        sibling.clean(0.9).unwrap();
        let (mut own, mut inherited_data) = (vec![0; BLOCK], vec![0; BLOCK]);
        sibling.read(kept, 0, &mut own).unwrap();
        sibling.read(inherited(), 0, &mut inherited_data).unwrap();
        (own, inherited_data)
    }

    /// Runs `stalled`, which stalls at `op` on `path`, on a thread of its own; meanwhile the
    /// sibling works, and the parent is opened and closed; then lets `stalled` go on. Returns what
    /// `stalled` gave, and what the sibling read.
    fn while_stalled<R: Send + 'static>(
        dir: &Stalling,
        lineage: &Arc<Lineage<Stalling>>,
        (op, path): (Op, &str),
        stalled: impl FnOnce() -> R + Send + 'static,
    ) -> (R, Reads) {
        let sibling = Arc::new(lineage.open_branch(SIBLING).unwrap());
        dir.arm(op, path);
        let stalled = thread::spawn(stalled);
        dir.wait_until_stalled();
        let working = thread::spawn(move || sibling_works(&sibling));
        let opening = {
            let lineage = lineage.clone();
            thread::spawn(move || lineage.open_branch(PARENT).and_then(|parent| parent.close()))
        };
        let reads = working.join().unwrap();
        opening.join().unwrap().unwrap();
        dir.let_go(Ok(()));
        (stalled.join().unwrap(), reads)
    }

    /// Runs `test` under Shuttle: in schedules by PCT, which runs a thread as far as it goes before
    /// those of lower priority, as a thread that waits for a fork or deletion in flight needs to
    /// before the test lets it go; then in random ones.
    fn explore(test: impl Fn() + Clone + Send + Sync + 'static) {
        let config = || {
            let mut config = shuttle::Config::new();
            config.stack_size = 4 << 20;
            config
        };
        shuttle::Runner::new(PctScheduler::new(3, RUNS), config()).run(test.clone());
        shuttle::Runner::new(RandomScheduler::new(RUNS), config()).run(test);
    }

    /// The schedules each test runs, of each kind.
    const RUNS: usize = 100;

    /// The sibling read what it wrote and inherited.
    fn assert_read(reads: Reads) {
        assert_eq!(reads, (vec![3; BLOCK], vec![1; BLOCK]), "the sibling reads other data");
    }

    /// A fork's deletion, its record's next version written, or its index files removed, stalls
    /// no other branch.
    #[test]
    fn deletions_stall_no_other_branch() {
        for stall in [(Op::SyncDir, BRANCHES.to_owned()), (Op::SyncDir, branch_dir(INDEX, FORK))] {
            explore(move || {
                let dir = Stalling::new();
                let lineage = forked(&dir, &[FORK, SIBLING]);
                // Opened once, it has index files of its own, which its deletion removes.
                drop(lineage.open_branch(FORK).unwrap());
                let deleting = lineage.clone();
                let stall = (stall.0, stall.1.as_str());
                let (deleted, reads) =
                    while_stalled(&dir, &lineage, stall, move || deleting.delete(FORK));
                deleted.unwrap();
                assert_read(reads);
                assert_eq!(lineage.branches(), [(PARENT, None), (SIBLING, Some(PARENT))]);
            });
        }
    }

    /// A fork, its record written, stalls no other branch.
    #[test]
    fn forks_stall_no_other_branch() {
        explore(|| {
            let dir = Stalling::new();
            let lineage = forked(&dir, &[SIBLING, OTHER]);
            let other = lineage.open_branch(OTHER).unwrap();
            let stall = (Op::Create, record_path(CHILD, 0));
            let (forked, reads) =
                while_stalled(&dir, &lineage, (stall.0, &stall.1), move || other.fork(CHILD));
            forked.unwrap();
            assert_read(reads);
            assert_eq!(
                lineage.branches(),
                [
                    (PARENT, None),
                    (SIBLING, Some(PARENT)),
                    (OTHER, Some(PARENT)),
                    (CHILD, Some(OTHER))
                ]
            );
        });
    }

    /// A fork's first opening, which records that its index holds its state, stalls no other
    /// branch.
    #[test]
    fn first_openings_stall_no_other_branch() {
        explore(|| {
            let dir = Stalling::new();
            let lineage = forked(&dir, &[FORK, SIBLING]);
            let opening = lineage.clone();
            let (opened, reads) =
                while_stalled(&dir, &lineage, (Op::SyncDir, BRANCHES), move || {
                    let fork = opening.open_branch(FORK)?;
                    let mut data = vec![0; BLOCK];
                    fork.read(inherited(), 0, &mut data)?;
                    Ok::<_, Error>(data)
                });
            assert_eq!(opened.unwrap(), [1; BLOCK]);
            assert_read(reads);
            assert_eq!(
                lineage.branches(),
                [(PARENT, None), (FORK, Some(PARENT)), (SIBLING, Some(PARENT))]
            );
        });
    }

    /// A branch's capacity changed, in its record, stalls no other branch.
    #[test]
    fn capacity_changes_stall_no_other_branch() {
        explore(|| {
            let dir = Stalling::new();
            let lineage = forked(&dir, &[SIBLING, OTHER]);
            let other = lineage.open_branch(OTHER).unwrap();
            let (changed, reads) =
                while_stalled(&dir, &lineage, (Op::SyncDir, BRANCHES), move || {
                    other.set_capacity(2 << 20)?;
                    Ok::<_, Error>(other.description().capacity)
                });
            assert_eq!(changed.unwrap(), 2 << 20);
            assert_read(reads);
        });
    }

    /// Openings and deletions of a branch during its deletion: they wait for it, and act on what
    /// came of it. Done, the branch is gone; failed before it wrote anything, the branch is as it
    /// was, and opens.
    fn openings_and_deletions_during_a_deletion() {
        let dir = Stalling::new();
        let lineage = forked(&dir, &[FORK]);
        dir.arm(Op::SyncDir, BRANCHES);
        let deleting = spawn_deletion(&lineage);
        dir.wait_until_stalled();
        let (opening, deleting_again) = (spawn_opening(&lineage), spawn_deletion(&lineage));
        dir.let_go(Ok(()));
        deleting.join().unwrap().unwrap();
        assert_eq!(opening.join().unwrap().unwrap_err(), "the store has no branch 0x2");
        assert_eq!(deleting_again.join().unwrap().unwrap_err(), "the store has no branch 0x2");

        let dir = Stalling::new();
        let lineage = forked(&dir, &[FORK]);
        dir.arm(Op::Create, &record_path(FORK, 1));
        let deleting = spawn_deletion(&lineage);
        dir.wait_until_stalled();
        let opening = spawn_opening(&lineage);
        dir.let_go(Err(io::Error::other("the disk failed")));
        let failed = deleting.join().unwrap().unwrap_err();
        assert_eq!(failed, format!("creating {}", record_path(FORK, 1)));
        opening.join().unwrap().unwrap();
    }

    /// The fork's deletion, on a thread of its own: what it gave, as text.
    fn spawn_deletion(lineage: &Arc<Lineage<Stalling>>) -> thread::JoinHandle<Result<(), String>> {
        let lineage = lineage.clone();
        thread::spawn(move || lineage.delete(FORK).map_err(|error| error.to_string()))
    }

    /// The fork's opening, and its closing, on a thread of its own: what the opening gave, as
    /// text.
    fn spawn_opening(lineage: &Arc<Lineage<Stalling>>) -> thread::JoinHandle<Result<(), String>> {
        let lineage = lineage.clone();
        thread::spawn(move || {
            lineage.open_branch(FORK).map(drop).map_err(|error| error.to_string())
        })
    }

    #[test]
    fn openings_and_deletions_wait_for_a_deletion_in_flight() {
        set_rules(Rules::ALL);
        explore(openings_and_deletions_during_a_deletion);
    }

    #[test]
    #[should_panic(expected = "branch 0x2 is being forked or deleted")]
    fn needs_openings_and_deletions_to_wait() {
        set_rules(Rules { wait_for_forks_and_deletions: false, ..Rules::ALL });
        explore(openings_and_deletions_during_a_deletion);
    }

    /// Two forks to one branch, from two branches: the first stalls, writing the fork's record,
    /// and fails; the second waits for it, and is made.
    fn forks_to_one_branch() {
        let dir = Stalling::new();
        let lineage = forked(&dir, &[SIBLING, OTHER]);
        let sibling = lineage.open_branch(SIBLING).unwrap();
        let other = lineage.open_branch(OTHER).unwrap();
        dir.arm(Op::Create, &record_path(CHILD, 0));
        let first = thread::spawn(move || sibling.fork(CHILD).map_err(|error| error.to_string()));
        dir.wait_until_stalled();
        let second = thread::spawn(move || other.fork(CHILD).map_err(|error| error.to_string()));
        dir.let_go(Err(io::Error::other("the disk failed")));
        let failed = first.join().unwrap().unwrap_err();
        assert_eq!(failed, format!("creating {}", record_path(CHILD, 0)));
        second.join().unwrap().unwrap();
        assert_eq!(
            lineage.branches(),
            [(PARENT, None), (SIBLING, Some(PARENT)), (OTHER, Some(PARENT)), (CHILD, Some(OTHER))]
        );
    }

    #[test]
    fn forks_to_one_branch_wait_for_each_other() {
        set_rules(Rules::ALL);
        explore(forks_to_one_branch);
    }

    #[test]
    #[should_panic(expected = "branch 0x5 is being forked or deleted")]
    fn needs_forks_to_wait() {
        set_rules(Rules { wait_for_forks_and_deletions: false, ..Rules::ALL });
        explore(forks_to_one_branch);
    }

    /// The branches listed while a fork's deletion is in flight, removing its index files, once
    /// the parent's deletion is done: none, but only once the fork's is done too. So of two
    /// deletions of a store's last branches at once, as [`delete_host`](crate::delete_host)'s, the
    /// last to be done finds the store has no branches, and removes its directory, and none finds
    /// so before.
    fn listing_during_a_deletion() {
        let dir = Stalling::new();
        let lineage = forked(&dir, &[FORK]);
        // Opened once, it has index files of its own, which its deletion removes.
        drop(lineage.open_branch(FORK).unwrap());
        dir.arm(Op::SyncDir, &branch_dir(INDEX, FORK));
        let deleting = spawn_deletion(&lineage);
        dir.wait_until_stalled();
        let released = Arc::new(AtomicBool::new(false));
        let listing = {
            let (lineage, released) = (lineage.clone(), released.clone());
            thread::spawn(move || {
                lineage.delete(PARENT).unwrap();
                let listed = lineage.branches();
                (listed, released.load(Ordering::SeqCst))
            })
        };
        released.store(true, Ordering::SeqCst);
        dir.let_go(Ok(()));
        deleting.join().unwrap().unwrap();
        let (listed, after) = listing.join().unwrap();
        assert!(after, "listed {listed:?} before the deletion in flight was done");
        assert_eq!(listed, []);
    }

    #[test]
    fn lists_of_branches_wait_for_a_deletion_in_flight() {
        set_rules(Rules::ALL);
        explore(listing_during_a_deletion);
    }

    #[test]
    #[should_panic(expected = "before the deletion in flight was done")]
    fn needs_lists_of_branches_to_wait() {
        set_rules(Rules { wait_for_forks_and_deletions: false, ..Rules::ALL });
        explore(listing_during_a_deletion);
    }
}
