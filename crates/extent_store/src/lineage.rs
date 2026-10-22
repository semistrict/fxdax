// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A store's branches (see the crate's docs): their records, the claims they hold on the files of
//! extents' data, and the layers forks are over, their parents' states at the forks, whose files
//! they hold, and whose parents keep the index files they are in (TASK-31).

use crate::dir::{Dir, File as _, read_file};
use crate::framed::Framed;
use crate::index::{layers_at, recover, write_checkpoint};
use crate::layers::{Holdings, Layer};
use crate::layout::{
    BRANCHES, BranchRecord, DESCRIPTION, Description, EXTENTS, FileKey, INDEX, OPEN_EXTENTS,
    OPEN_SEGMENTS, Origin, SEGMENTS, SegmentId, StoreFile, branch_dir, create_branch_dirs,
    index_path, list_numbers, parse_number, parse_record_name, record_path, segment_path,
};
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
    /// The branch opening the store, rather than one of its branches, opens.
    first: u64,
    pub(crate) options: Options,
    branches: Mutex<Branches>,
    /// What holds the store's directory taken for this process ([`Dir::take`]): its branches
    /// open, and its deletions under way.
    holds: Mutex<usize>,
    /// The parts of its files read lately, unpacked.
    pub(crate) unpacked: crate::framed::Unpacked,
    /// The closed segments' files its branches read what they inherited from, open: one for all
    /// the forks reading it, the most `Options::open_files` but for those reads use (TASK-34).
    files: Mutex<HashMap<SegmentId, Arc<Framed<D::File>>>>,
}

struct Branches {
    entries: BTreeMap<u64, Entry>,
    /// The last opening of a branch's.
    openings: u64,
    /// The branches holding extents in each file of extents' data, of their own: not through the
    /// layer they are over. And the branches that let the file go.
    claims: HashMap<FileKey, Claim>,
    /// The layers forks are over, by their addresses, and the forks over each, which hold its
    /// files but for those they let go of.
    layers: HashMap<usize, Inherited>,
    /// The files no branch claims, which go once the commits that let them go are durable.
    doomed: Vec<(FileKey, Vec<(u64, u64)>)>,
}

struct Entry {
    record: BranchRecord,
    /// The version of its record's file.
    version: u64,
    /// The store's opening of it, if a store has it open.
    open: Option<u64>,
    /// The last epoch of its that is durable.
    durable: u64,
    /// The layer it is over, its parent's state at the fork, if a fork's over it.
    layer: Option<Arc<Layer>>,
}

#[derive(Default)]
struct Claim {
    holders: BTreeSet<u64>,
    /// The branches that let it go, and the epochs of the commits that did.
    drops: Vec<(u64, u64)>,
}

/// A layer forks are over, which they hold the files of (TASK-31).
struct Inherited {
    layer: Arc<Layer>,
    holders: BTreeSet<u64>,
    /// The files of the layer holders let go of: they hold no extent in them any more.
    dropped: HashMap<FileKey, BTreeSet<u64>>,
}

impl Inherited {
    /// Whether a holder of it, but `except`, holds `file` through it.
    fn holds(&self, file: FileKey, except: Option<u64>) -> bool {
        let dropped = self.dropped.get(&file);
        self.layer.holds(file)
            && (self.holders.iter()).any(|holder| {
                Some(*holder) != except && !dropped.is_some_and(|by| by.contains(holder))
            })
    }
}

/// The layers forks are over, by where they were forked: the parent, its index file, and its
/// epoch.
type Layers = HashMap<(u64, u64, u64), Arc<Layer>>;

/// A layer's key in [`Branches::layers`]: its address, as it is held there.
fn layer_key(layer: &Arc<Layer>) -> usize {
    Arc::as_ptr(layer) as usize
}

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
        let mut versions: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for name in dir.list(BRANCHES)? {
            let (branch, version) = parse_record_name(&name)
                .ok_or_else(|| anyhow!("{BRANCHES}/{name} is no branch's record"))?;
            versions.entry(branch).or_default().push(version);
        }
        let mut entries = BTreeMap::new();
        for (branch, mut numbers) in versions {
            numbers.sort_unstable_by(|a, b| b.cmp(a));
            let mut found = None;
            for &version in &numbers {
                let file = dir.open(&record_path(branch, version), false)?;
                let text = String::from_utf8(read_file(&file)?).unwrap_or_default();
                match BranchRecord::parse(&text) {
                    Ok(record) if found.is_none() => found = Some((version, record)),
                    _ => dir.remove(&record_path(branch, version))?,
                }
            }
            let Some((version, record)) = found else {
                // fxdax: a fork the host crashed while making, its record's first version never
                // durable whole (a later version is written before the one before goes): the fork
                // never happened, as Store::fork never returned, and its torn record is gone
                // (above). Not the first branch's, though: that's a store never made.
                ensure!(branch != first, "branch {branch:#x} has no whole record");
                log::warn!(
                    "extent store: branch {branch:#x}'s record was never whole: a fork not made"
                );
                continue;
            };
            let entry = Entry { record, version, open: None, durable: 0, layer: None };
            entries.insert(branch, entry);
        }
        ensure!(entries.contains_key(&first), "the store's first branch has no record");

        // The layers forks are over, their parents' states at the forks: each index file they are
        // in read once, for all the forks of it (TASK-31).
        let layers = build_layers(&dir, &entries)?;
        let mut branches = Branches {
            entries,
            openings: 0,
            claims: HashMap::new(),
            layers: HashMap::new(),
            doomed: Vec::new(),
        };
        // What each branch holds: its own state, over the layer it is over, if a fork's.
        let live: Vec<(u64, BranchRecord)> = (branches.entries.iter())
            .filter(|(_, entry)| !entry.record.deleted)
            .map(|(branch, entry)| (*branch, entry.record))
            .collect();
        for (branch, record) in live {
            let base = match record.over_parent() {
                Some(origin) => Some(
                    (layers.get(&(origin.parent, origin.index, origin.epoch)).cloned())
                        .ok_or_else(|| anyhow!("branch {branch:#x} has no parent's state"))?,
                ),
                None => None,
            };
            if let Some(base) = &base {
                branches.entries.get_mut(&branch).unwrap().layer = Some(base.clone());
                branches.hold_layer(branch, base.clone());
            }
            match (record.resolved, record.origin) {
                (true, _) => {
                    let recovered = recover(&dir, branch, base)?;
                    branches.entries.get_mut(&branch).unwrap().durable = recovered.epoch;
                    let Holdings { own, dropped } = recovered.extents.holdings();
                    for file in own {
                        branches.claims.entry(file).or_default().holders.insert(branch);
                    }
                    for file in dropped {
                        branches.let_go_of_inherited(branch, file);
                    }
                }
                (false, Some(_)) => {}
                (false, None) => bail!("branch {branch:#x} is no fork, and has no index"),
            }
        }

        // Deleted branches go, but for their index files forks need, and the first's record, which
        // says the store was deleted.
        let pinned = pinned_indexes(&branches.entries);
        let deleted: Vec<u64> = (branches.entries.iter())
            .filter(|(_, entry)| entry.record.deleted)
            .map(|(branch, _)| *branch)
            .collect();
        for branch in deleted {
            let keep = pinned.get(&branch).cloned().unwrap_or_default();
            remove_branch_index(&dir, branch, &keep)?;
            if keep.is_empty() && branch != first {
                let version = branches.entries.remove(&branch).unwrap().version;
                dir.remove(&record_path(branch, version))?;
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
                            branches.held(FileKey::Segment(SegmentId { branch, number }))
                        }
                        EXTENTS => branches.held(FileKey::Own(ExtentId::new(branch, number))),
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
            first,
            options,
            branches: Mutex::new(branches),
            holds: Mutex::new(0),
            unpacked: Default::default(),
            files: Mutex::new(HashMap::new()),
        }))
    }

    fn state(&self) -> MutexGuard<'_, Branches> {
        self.branches.lock().unwrap()
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
        self.first
    }

    /// Whether the store was deleted ([`Lineage::delete`] of its first branch): its directory
    /// holds its forks' data alone, which they share.
    pub fn is_deleted(&self) -> bool {
        self.state().entries[&self.first].record.deleted
    }

    /// Whether the store has `branch`, not deleted.
    pub fn has(&self, branch: u64) -> bool {
        self.state().entries.get(&branch).is_some_and(|entry| !entry.record.deleted)
    }

    /// Whether a store of this process has `branch` open.
    pub fn is_open(&self, branch: u64) -> bool {
        self.state().entries.get(&branch).is_some_and(|entry| entry.open.is_some())
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
        (self.state().entries.iter())
            .filter(|(_, entry)| !entry.record.deleted)
            .map(|(branch, entry)| (*branch, entry.record.origin.map(|origin| origin.parent)))
            .collect()
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
        let (token, record) = {
            let mut branches = self.state();
            branches.openings += 1;
            let token = branches.openings;
            let entry = branches.live(branch, self.first)?;
            ensure!(entry.open.is_none(), "branch {branch:#x} is open");
            entry.open = Some(token);
            (token, entry.record)
        };
        Store::open_in(self.clone(), branch, token, record).inspect_err(|_| {
            // Not open, which the caller lets go of.
            self.state().entries.get_mut(&branch).unwrap().open = None;
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
            let mut branches = self.state();
            let entry = branches.live(branch, self.first)?;
            ensure!(entry.open.is_none(), "branch {branch:#x} is open");
            let record = BranchRecord { deleted: true, ..entry.record };
            write_record(&self.dir, branch, entry.version + 1, &record)?;
            self.dir.remove(&record_path(branch, entry.version))?;
            entry.version += 1;
            entry.record = record;
            let layer = entry.layer.take();
            // Its forks hold what they inherited of its extents: through the layers they are over.
            let held: Vec<FileKey> = (branches.claims.iter())
                .filter(|(_, claim)| claim.holders.contains(&branch))
                .map(|(file, _)| *file)
                .collect();
            // Its record says it is deleted, durably: its letting them go is durable.
            branches.release(branch, 0, held);
            if let Some(layer) = layer {
                branches.leave_layer(branch, &layer);
            }
            let pinned = pinned_indexes(&branches.entries).remove(&branch).unwrap_or_default();
            remove_branch_index(&self.dir, branch, &pinned)?;
            if pinned.is_empty() && branch != self.first {
                let version = branches.entries.remove(&branch).unwrap().version;
                self.dir.remove(&record_path(branch, version))?;
            }
            branches.removable()
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
        let mut branches = self.state();
        ensure!(!branches.entries.contains_key(&child), "the store has a branch {child:#x}");
        let record = BranchRecord {
            capacity,
            origin: Some(origin),
            resolved: false,
            layered: true,
            deleted: false,
        };
        write_record(&self.dir, child, 0, &record)?;
        let entry =
            Entry { record, version: 0, open: None, durable: 0, layer: Some(layer.clone()) };
        branches.entries.insert(child, entry);
        branches.hold_layer(child, layer);
        Ok(())
    }

    /// Records that `branch`, a fork, has an index: of what it changed over its parent's state at
    /// the fork, whose index file its parent keeps for it while it lives.
    pub(crate) fn resolved(&self, branch: u64) -> Result<(), Error> {
        let mut branches = self.state();
        let entry = branches.entries.get_mut(&branch).unwrap();
        let record = BranchRecord { resolved: true, layered: true, ..entry.record };
        write_record(&self.dir, branch, entry.version + 1, &record)?;
        self.dir.remove(&record_path(branch, entry.version))?;
        entry.version += 1;
        entry.record = record;
        Ok(())
    }

    /// The layer `branch` is over, its parent's state at the fork, if a fork's over it.
    pub(crate) fn base(&self, branch: u64) -> Option<Arc<Layer>> {
        self.state().entries.get(&branch)?.layer.clone()
    }

    /// `branch`'s state at its latest fork of those live, which that fork is over, if any.
    pub(crate) fn latest_layer_of(&self, branch: u64) -> Option<Arc<Layer>> {
        let branches = self.state();
        (branches.entries.values())
            .filter(|entry| !entry.record.deleted)
            .filter_map(|entry| Some((entry.record.over_parent()?, entry.layer.as_ref()?)))
            .filter(|(origin, _)| origin.parent == branch)
            .max_by_key(|(origin, _)| (origin.index, origin.epoch))
            .map(|(_, layer)| layer.clone())
    }

    /// Sets `branch`'s capacity, durably, in its record ([`Store::set_capacity`]).
    pub(crate) fn set_capacity(&self, branch: u64, capacity: u64) -> Result<(), Error> {
        let mut branches = self.state();
        let entry = branches.entries.get_mut(&branch).unwrap();
        let record = BranchRecord { capacity, ..entry.record };
        write_record(&self.dir, branch, entry.version + 1, &record)?;
        self.dir.remove(&record_path(branch, entry.version))?;
        entry.version += 1;
        entry.record = record;
        Ok(())
    }

    /// The index files of `branch` that forks need: those of the states they are over.
    pub(crate) fn pinned(&self, branch: u64) -> BTreeSet<u64> {
        pinned_indexes(&self.state().entries).remove(&branch).unwrap_or_default()
    }

    /// Records that the store of `branch`'s opening `token` closed it, which lets go of the
    /// store's directory, if it was the last open.
    pub(crate) fn closed(&self, branch: u64, token: u64) {
        let closed = {
            let mut branches = self.state();
            match branches.entries.get_mut(&branch) {
                Some(entry) if entry.open == Some(token) => {
                    entry.open = None;
                    true
                }
                _ => false,
            }
        };
        if closed {
            self.let_go();
        }
    }

    /// Records that `branch` holds extents in `files`.
    pub(crate) fn claim(&self, branch: u64, files: impl IntoIterator<Item = FileKey>) {
        let mut branches = self.state();
        for file in files {
            branches.claims.entry(file).or_default().holders.insert(branch);
        }
    }

    /// Records that `branch` holds extents in exactly `files` of its own, and none in `dropped` of
    /// the layer it is over, as its state, just recovered, says.
    pub(crate) fn reset(
        &self,
        branch: u64,
        epoch: u64,
        files: &BTreeSet<FileKey>,
        dropped: &BTreeSet<FileKey>,
    ) {
        let removed = {
            let mut branches = self.state();
            let held: Vec<FileKey> = (branches.claims.iter())
                .filter(|(file, claim)| claim.holders.contains(&branch) && !files.contains(file))
                .map(|(file, _)| *file)
                .collect();
            for &file in files {
                branches.claims.entry(file).or_default().holders.insert(branch);
            }
            branches.release(branch, 0, held);
            branches.release(branch, 0, dropped.iter().copied().collect());
            branches.entries.get_mut(&branch).unwrap().durable = epoch;
            branches.removable()
        };
        self.remove(removed);
    }

    /// Records that `branch`'s commit `epoch` lets `files` go: they go once no branch holds an
    /// extent in them, and the commits that let them go are durable.
    pub(crate) fn release(&self, branch: u64, epoch: u64, files: Vec<FileKey>) {
        self.state().release(branch, epoch, files);
    }

    /// Whether a branch other than `branch` holds an extent in `file`: of its own, or through the
    /// layer it is over.
    pub(crate) fn shared(&self, branch: u64, file: FileKey) -> bool {
        let branches = self.state();
        (branches.claims.get(&file))
            .is_some_and(|claim| claim.holders.iter().any(|holder| *holder != branch))
            || branches.layers.values().any(|inherited| inherited.holds(file, Some(branch)))
    }

    /// Records that `branch`'s commits up to `epoch` are durable, and removes the files that can go
    /// now.
    pub(crate) fn durable(&self, branch: u64, epoch: u64) {
        let removed = {
            let mut branches = self.state();
            if let Some(entry) = branches.entries.get_mut(&branch) {
                entry.durable = entry.durable.max(epoch);
            }
            branches.removable()
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

impl Branches {
    /// `branch`'s entry, unless it was deleted; `first` is the store's first branch.
    fn live(&mut self, branch: u64, first: u64) -> Result<&mut Entry, Error> {
        match self.entries.get_mut(&branch) {
            Some(entry) if !entry.record.deleted => Ok(entry),
            Some(_) if branch == first => {
                bail!("the store was deleted: its directory holds its forks' data alone")
            }
            _ => bail!("the store has no branch {branch:#x}"),
        }
    }

    /// Whether a branch holds an extent in `file`: of its own, or through the layer it is over.
    fn held(&self, file: FileKey) -> bool {
        self.claims.get(&file).is_some_and(|claim| !claim.holders.is_empty())
            || self.layers.values().any(|inherited| inherited.holds(file, None))
    }

    /// Records that `branch` is over `layer`, which it holds the files of.
    fn hold_layer(&mut self, branch: u64, layer: Arc<Layer>) {
        let inherited = self.layers.entry(layer_key(&layer)).or_insert_with(|| Inherited {
            layer,
            holders: BTreeSet::new(),
            dropped: HashMap::new(),
        });
        inherited.holders.insert(branch);
    }

    /// Records that `branch` holds no extent in `file` of the layer it is over, if it is over one
    /// holding it. Returns whether it held it.
    fn let_go_of_inherited(&mut self, branch: u64, file: FileKey) -> bool {
        let mut held = false;
        for inherited in self.layers.values_mut() {
            if inherited.holders.contains(&branch) && inherited.layer.holds(file) {
                held |= inherited.dropped.entry(file).or_default().insert(branch);
            }
        }
        held
    }

    /// Records that `branch`'s commit `epoch` lets `files` go, which it held of its own or through
    /// the layer it is over: they go once no branch holds them, and the commits that let them go
    /// are durable.
    fn release(&mut self, branch: u64, epoch: u64, files: Vec<FileKey>) {
        for file in files {
            let own = self.claims.get_mut(&file).is_some_and(|claim| claim.holders.remove(&branch));
            if !own && !self.let_go_of_inherited(branch, file) {
                continue;
            }
            self.claims.entry(file).or_default().drops.push((branch, epoch));
            self.doom_unless_held(file);
        }
    }

    /// Dooms `file`, unless a branch holds it.
    fn doom_unless_held(&mut self, file: FileKey) {
        if self.held(file) {
            return;
        }
        let drops = self.claims.remove(&file).map_or_else(Vec::new, |claim| claim.drops);
        for inherited in self.layers.values_mut() {
            inherited.dropped.remove(&file);
        }
        self.doomed.push((file, drops));
    }

    /// Records that `branch`, deleted, is over `layer` no longer: the files of it no other branch
    /// holds go, all of them with the last branch over it.
    fn leave_layer(&mut self, branch: u64, layer: &Arc<Layer>) {
        let key = layer_key(layer);
        let Some(inherited) = self.layers.get_mut(&key) else { return };
        inherited.holders.remove(&branch);
        let let_go: BTreeSet<FileKey> = (inherited.dropped.iter())
            .filter(|(_, by)| by.contains(&branch))
            .map(|(file, _)| *file)
            .collect();
        for by in inherited.dropped.values_mut() {
            by.remove(&branch);
        }
        let files: Vec<FileKey> = if inherited.holders.is_empty() {
            self.layers.remove(&key).unwrap().layer.files().into_iter().collect()
        } else {
            // Those every other branch over it let go of.
            let holders = inherited.holders.len();
            (inherited.dropped.iter())
                .filter(|(_, by)| by.len() == holders)
                .map(|(file, _)| *file)
                .collect()
        };
        for file in files.into_iter().filter(|file| !let_go.contains(file)) {
            if !self.held(file) {
                self.claims.entry(file).or_default().drops.push((branch, 0));
                self.doom_unless_held(file);
            }
        }
    }

    /// The files doomed whose letting go is durable, which go now.
    fn removable(&mut self) -> Vec<FileKey> {
        let entries = &self.entries;
        let durable = |(branch, epoch): &(u64, u64)| {
            *epoch == 0 || entries.get(branch).is_none_or(|entry| entry.durable >= *epoch)
        };
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.doomed)
            .into_iter()
            .partition(|(_, drops)| drops.iter().all(durable));
        self.doomed = kept;
        gone.into_iter().map(|(file, _)| file).collect()
    }
}

/// The index files of each branch that live forks need: those of the states they are over, and of
/// the states those are over, up their lines, deleted branches' included (TASK-31).
fn pinned_indexes(entries: &BTreeMap<u64, Entry>) -> HashMap<u64, BTreeSet<u64>> {
    let mut pinned: HashMap<u64, BTreeSet<u64>> = HashMap::new();
    for entry in entries.values().filter(|entry| !entry.record.deleted) {
        let mut record = entry.record;
        while let Some(origin) = record.over_parent() {
            if !pinned.entry(origin.parent).or_default().insert(origin.index) {
                // Its line, from there, pinned already.
                break;
            }
            let Some(parent) = entries.get(&origin.parent) else { break };
            record = parent.record;
        }
    }
    pinned
}

/// The layers live forks are over, by where they were forked (TASK-31): each parent's state at each
/// fork point of its live forks, and at the points of its own parent's it is over, up their lines,
/// each index file they are in read once, for all the forks of it.
fn build_layers<D: Dir>(dir: &D, entries: &BTreeMap<u64, Entry>) -> Result<Layers, Error> {
    let mut wanted: BTreeMap<(u64, u64), BTreeSet<u64>> = BTreeMap::new();
    for entry in entries.values().filter(|entry| !entry.record.deleted) {
        let mut record = entry.record;
        while let Some(origin) = record.over_parent() {
            if !wanted.entry((origin.parent, origin.index)).or_default().insert(origin.epoch) {
                break;
            }
            let parent = entries.get(&origin.parent);
            let parent =
                parent.ok_or_else(|| anyhow!("branch {:#x} has no record", origin.parent))?;
            record = parent.record;
        }
    }
    let mut built = HashMap::new();
    for &(parent, index) in wanted.keys() {
        build_layers_of(dir, entries, &wanted, parent, index, &mut built)?;
    }
    Ok(built)
}

/// Builds the layers `wanted` names of `parent`'s index file `index`, and those they are over.
fn build_layers_of<D: Dir>(
    dir: &D,
    entries: &BTreeMap<u64, Entry>,
    wanted: &BTreeMap<(u64, u64), BTreeSet<u64>>,
    parent: u64,
    index: u64,
    built: &mut Layers,
) -> Result<(), Error> {
    let epochs = &wanted[&(parent, index)];
    if built.contains_key(&(parent, index, *epochs.first().unwrap())) {
        return Ok(());
    }
    let base = match entries[&parent].record.over_parent() {
        Some(origin) => {
            let key = (origin.parent, origin.index, origin.epoch);
            if !built.contains_key(&key) {
                build_layers_of(dir, entries, wanted, origin.parent, origin.index, built)?;
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

/// Writes `branch`'s record's `version`, durably.
fn write_record<D: Dir>(
    dir: &D,
    branch: u64,
    version: u64,
    record: &BranchRecord,
) -> Result<(), Error> {
    let path = record_path(branch, version);
    let file = dir.create(&path, false).with_context(|| format!("creating {path}"))?;
    file.write_at(record.text().as_bytes(), 0)?;
    file.sync()?;
    dir.sync_dir(BRANCHES)?;
    dir.barrier()?;
    Ok(())
}
