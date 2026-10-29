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
//! be written or removed. fxdax: nor is `files`, the segments' files open that the branches read
//! what they inherited from, while one opens: so that no branch's read waits for another's opening
//! (TASK-92.2). One other lock of the lineage's is held across I/O: `holds` while the store's
//! directory is taken ([`Dir::take`]), as its count and the taking change together, and fxdax: the
//! branches are read again, if another process changed them meanwhile (TASK-79.5).

use crate::branch_records::{BranchRecords, Forking, write_record};
use crate::dir::{Dir, File as _, read_file};
use crate::framed::{Files, Framed};
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
    files: Mutex<Files<SegmentId, D::File>>,
}

/// fxdax: a hold of a store's directory, taken for this process until it is dropped
/// ([`Lineage::hold_dir`]).
pub struct DirHold<D: Dir>(Arc<Lineage<D>>);

impl<D: Dir> Drop for DirHold<D> {
    fn drop(&mut self) {
        self.0.let_go();
    }
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
        let (records, ledger) = Self::load(&dir, first)?;
        Ok(Arc::new(Self {
            dir,
            block_size,
            options,
            records,
            ledger: Mutex::new(ledger),
            holds: Mutex::new(0),
            unpacked: Default::default(),
            files: Mutex::new(Files::new(options.open_files)),
        }))
    }

    /// The store's branches in `dir`, its first `first`, each as of its last commit that is
    /// durable: their records, and which of them hold each file of extents' data. Removes what
    /// deleted branches left, and the files of extents' data no branch holds an extent in: what
    /// commits that weren't durable wrote, or that were to go once the commits that let them go
    /// were. The store's directory must be taken.
    fn load(dir: &D, first: u64) -> Result<(BranchRecords, Ledger), Error> {
        // Each branch's newest record that is whole; older versions go.
        let records = BranchRecords::read(dir, first)?;
        let read = records.snapshot();
        // The layers forks are over, their parents' states at the forks: each index file they are
        // in read once, for all the forks of it (TASK-31).
        let layers = build_layers(dir, &read)?;
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
                    let recovered = recover(dir, branch, base)?;
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
            remove_branch_index(dir, branch, &keep)?;
            if keep.is_empty() && branch != first {
                records.forget(dir, branch)?;
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
        Ok((records, ledger))
    }

    /// fxdax: reads the store's branches again, if their records changed since the lineage read
    /// them: another process had the store's directory meanwhile, and forked or deleted branches,
    /// whose files and index files this one must not take for unclaimed (TASK-79.5). What another
    /// process's commits changed of a branch, its opening here recovers. None of its branches is
    /// open, nor being forked or deleted, here: the directory was let go of.
    fn reload(&self) -> Result<(), Error> {
        if self.records.unchanged(&self.dir)? {
            return Ok(());
        }
        let (records, ledger) = Self::load(&self.dir, self.first())?;
        self.records.replace(records);
        *self.ledger() = ledger;
        self.files.lock().unwrap().clear();
        Ok(())
    }

    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap()
    }

    /// The store's directory.
    pub fn dir(&self) -> &D {
        &self.dir
    }

    /// The framed file of `segment`, closed, which its branches share: opened if it isn't, holding
    /// no lock of the lineage's meanwhile, and kept, but for one opened meanwhile; the least lately
    /// read of the files no read uses closing, once more than `Options::open_files` are open.
    /// fxdax: the segment is not removed meanwhile (`Lineage::remove`), as the branch reading it
    /// holds an extent in it.
    pub(crate) fn segment_file(&self, segment: SegmentId) -> Result<Arc<Framed<D::File>>, Error> {
        if let Some(file) = self.files.lock().unwrap().get(segment) {
            return Ok(file);
        }
        let path = segment_path(segment);
        let file = Arc::new(Framed::open(self.dir.open(&path, true)?, &path)?);
        Ok(self.files.lock().unwrap().insert(segment, file))
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

    /// fxdax: holds the store's directory taken for this process until what it returns is dropped,
    /// as a branch open does: for what outlives the branch's store's closing and still writes in
    /// the directory, as its uploader does as it stops (TASK-129).
    pub fn hold_dir(self: &Arc<Self>) -> Result<DirHold<D>, Error> {
        self.hold()?;
        Ok(DirHold(self.clone()))
    }

    /// Holds the store's directory taken for this process, taking it if nothing held it: and then
    /// reading its branches again, if another process changed them meanwhile (TASK-79.5).
    fn hold(&self) -> Result<(), Error> {
        let mut holds = self.holds.lock().unwrap();
        if *holds == 0 {
            self.dir.take().context("taking the store's directory")?;
            if let Err(error) = self.reload() {
                self.dir.let_go();
                return Err(error.context("reading the store's branches again"));
            }
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

    /// Reserves `child`, a branch the store does not have, for a fork of a branch a store has open,
    /// whose commit is made, then recorded ([`Lineage::fork`]): no opening, fork or deletion of it
    /// begins until the fork is recorded, or fails, and queries of the branches the store has wait
    /// for it once its commit is made.
    pub(crate) fn forking(&self, child: u64) -> Result<Forking<'_>, Error> {
        self.records.fork(child)
    }

    /// Records a fork of `origin.parent`, which a store has open, as of `origin`: a new branch,
    /// the child `forking` reserved, of `capacity` bytes, over `layer`, its parent's state at the
    /// fork (TASK-31), which it holds the files of, and its index will hold what it changes over.
    pub(crate) fn fork(
        &self,
        forking: Forking<'_>,
        origin: Origin,
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
        forking.write(&self.dir, &record)?;
        // It holds its layer's files, which its parent holds until its next commit, before it is
        // found.
        self.ledger().attach(forking.child(), Over { origin, layer });
        forking.found(record);
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

    /// fxdax: how often a branch let go of files another made (TASK-79.2): once it changed, a
    /// branch's files found shared may be its alone.
    pub(crate) fn unshared(&self) -> u64 {
        self.ledger().unshared()
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
        if files.is_empty() {
            return;
        }
        for path in files.iter().flat_map(FileKey::paths) {
            if let Err(err) = self.dir.remove(&path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                // Opening the store removes it, as no branch claims it.
                log::warn!("extent store: failed to remove {path}: {err}");
            }
        }
        // fxdax: and their files open for the forks reading them, which would read them on, and
        // keep their room taken: though another file be made at their paths, a branch's ID being
        // taken again, or a segment's number (TASK-79.38). Once they are unlinked, so that none
        // opens them again meanwhile.
        let mut open = self.files.lock().unwrap();
        for file in &files {
            if let FileKey::Segment(segment) = file {
                open.remove(*segment);
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
