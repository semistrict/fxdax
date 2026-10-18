// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A store's branches (see the crate's docs): their records, the claims they hold on the files of
//! extents' data, and the forks not yet opened, whose parents keep what they need.

use crate::dir::{Dir, File as _};
use crate::layout::{
    BRANCHES, BranchRecord, DESCRIPTION, Data, Description, EXTENTS, Extent, INDEX, OPEN_EXTENTS,
    OPEN_SEGMENTS, Origin, SEGMENTS, SegmentId, StoreFile, branch_dir, extent_path, index_path,
    open_extent_path, open_segment_path, parse_number, parse_record_name, record_path,
    segment_path,
};
use crate::store::{Options, Store, read_file, recover, state_at};
use crate::sync::{Mutex, MutexGuard};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use storage_device::ExtentId;

/// A file of extents' data, which the branches holding extents in it claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum FileKey {
    Segment(SegmentId),
    Own(ExtentId),
}

impl FileKey {
    /// Its paths: framed, and raw, while it is open.
    pub(crate) fn paths(&self) -> [String; 2] {
        match self {
            Self::Segment(segment) => [segment_path(*segment), open_segment_path(*segment)],
            Self::Own(extent) => [extent_path(*extent), open_extent_path(*extent)],
        }
    }
}

/// The files `extents`' data is in.
pub(crate) fn files_of<'a>(
    extents: impl IntoIterator<Item = (&'a ExtentId, &'a Extent)>,
) -> BTreeSet<FileKey> {
    let mut files = BTreeSet::new();
    for (extent, entry) in extents {
        match &entry.data {
            Data::Pieces(pieces) => {
                files.extend(pieces.iter().map(|piece| FileKey::Segment(piece.segment)));
            }
            Data::Own => {
                files.insert(FileKey::Own(*extent));
            }
            Data::Remote => {}
        }
    }
    files
}

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
}

struct Branches {
    entries: BTreeMap<u64, Entry>,
    /// The last opening of a branch's.
    openings: u64,
    /// The branches holding extents in each file of extents' data.
    claims: HashMap<FileKey, Claim>,
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
}

#[derive(Default)]
struct Claim {
    holders: BTreeSet<u64>,
    /// The branches that let it go, and the epochs of the commits that did.
    drops: Vec<(u64, u64)>,
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
        crate::store::write_checkpoint(&dir, branch, 0, &Default::default())?;
        let record = BranchRecord { capacity, origin: None, resolved: true, deleted: false };
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
            entries.insert(branch, Entry { record, version, open: None, durable: 0 });
        }
        ensure!(entries.contains_key(&first), "the store's first branch has no record");

        // What each branch holds: its own state, or its parent's at the fork, if it was not opened
        // since.
        let mut claims: HashMap<FileKey, Claim> = HashMap::new();
        for (&branch, entry) in &mut entries {
            if entry.record.deleted {
                continue;
            }
            let extents = match (entry.record.resolved, entry.record.origin) {
                (true, _) => {
                    let recovered = recover(&dir, branch)?;
                    entry.durable = recovered.epoch;
                    recovered.extents
                }
                (false, Some(origin)) => {
                    state_at(&dir, origin.parent, origin.index, origin.epoch)
                        .with_context(|| format!("branch {branch:#x}'s parent at its fork"))?
                        .extents
                }
                (false, None) => bail!("branch {branch:#x} is no fork, and has no index"),
            };
            for file in files_of(&extents) {
                claims.entry(file).or_default().holders.insert(branch);
            }
        }

        // Deleted branches go, but for their index files forks not yet opened need, and the first's
        // record, which says the store was deleted.
        let pinned = pinned_indexes(&entries);
        let deleted: Vec<u64> = (entries.iter())
            .filter(|(_, entry)| entry.record.deleted)
            .map(|(branch, _)| *branch)
            .collect();
        for branch in deleted {
            let keep = pinned.get(&branch).cloned().unwrap_or_default();
            remove_branch_index(&dir, branch, &keep)?;
            if keep.is_empty() && branch != first {
                let version = entries.remove(&branch).unwrap().version;
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
                            claims.contains_key(&FileKey::Segment(SegmentId { branch, number }))
                        }
                        EXTENTS => {
                            claims.contains_key(&FileKey::Own(ExtentId::new(branch, number)))
                        }
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
            branches: Mutex::new(Branches { entries, openings: 0, claims, doomed: Vec::new() }),
            holds: Mutex::new(0),
            unpacked: Default::default(),
        }))
    }

    fn state(&self) -> MutexGuard<'_, Branches> {
        self.branches.lock().unwrap()
    }

    /// The store's directory.
    pub fn dir(&self) -> &D {
        &self.dir
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
            // Its forks not yet opened hold what they inherited of its extents, as when it frees
            // them ([`Lineage::transfer`]).
            let heirs = branches.heirs(branch);
            if !heirs.is_empty() {
                let extents = recover(&self.dir, branch)?.extents;
                for (heir, origin) in heirs {
                    let inherited =
                        (extents.iter()).filter(|(extent, _)| inherited(**extent, &origin));
                    for file in files_of(inherited) {
                        branches.claims.entry(file).or_default().holders.insert(heir);
                    }
                }
            }
            let held: Vec<FileKey> = (branches.claims.iter())
                .filter(|(_, claim)| claim.holders.contains(&branch))
                .map(|(file, _)| *file)
                .collect();
            // Its record says it is deleted, durably: its letting them go is durable.
            branches.release(branch, 0, held);
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
    /// `child`, of `capacity` bytes, whose state is its parent's at the fork until it is opened.
    pub(crate) fn fork(&self, origin: Origin, child: u64, capacity: u64) -> Result<(), Error> {
        let mut branches = self.state();
        ensure!(!branches.entries.contains_key(&child), "the store has a branch {child:#x}");
        let record =
            BranchRecord { capacity, origin: Some(origin), resolved: false, deleted: false };
        write_record(&self.dir, child, 0, &record)?;
        branches.entries.insert(child, Entry { record, version: 0, open: None, durable: 0 });
        Ok(())
    }

    /// Records that `branch`, a fork, has its state in its own index: its parent no longer keeps
    /// its index file as of the fork, which goes now if its store is not open.
    pub(crate) fn resolved(&self, branch: u64) -> Result<(), Error> {
        let mut branches = self.state();
        let entry = branches.entries.get_mut(&branch).unwrap();
        let record = BranchRecord { resolved: true, ..entry.record };
        write_record(&self.dir, branch, entry.version + 1, &record)?;
        self.dir.remove(&record_path(branch, entry.version))?;
        entry.version += 1;
        entry.record = record;
        let Some(origin) = record.origin else { return Ok(()) };
        let parent_open =
            branches.entries.get(&origin.parent).is_some_and(|entry| entry.open.is_some());
        let still_pinned =
            pinned_indexes(&branches.entries).get(&origin.parent).cloned().unwrap_or_default();
        if !parent_open && !still_pinned.contains(&origin.index) {
            let numbers = list_numbers(&self.dir, &branch_dir(INDEX, origin.parent))?;
            // Not the parent's newest, which it recovers from.
            if numbers.last().is_some_and(|newest| *newest != origin.index) {
                self.dir.remove(&index_path(origin.parent, origin.index))?;
            }
        }
        Ok(())
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

    /// The index files of `branch` that forks not yet opened need.
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

    /// Records that `branch` holds extents in exactly `files`, as its state, just recovered, says.
    pub(crate) fn reset(&self, branch: u64, epoch: u64, files: &BTreeSet<FileKey>) {
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

    /// Records that `branch` let go of `extent`, whose data is in `files`: the forks of it not yet
    /// opened that inherited it hold it now.
    pub(crate) fn transfer(&self, branch: u64, extent: ExtentId, files: &BTreeSet<FileKey>) {
        if files.is_empty() {
            return;
        }
        let mut branches = self.state();
        let heirs: Vec<u64> = (branches.heirs(branch).into_iter())
            .filter(|(_, origin)| inherited(extent, origin))
            .map(|(child, _)| child)
            .collect();
        for heir in heirs {
            for &file in files {
                branches.claims.entry(file).or_default().holders.insert(heir);
            }
        }
    }

    /// Whether a branch other than `branch` holds an extent in `file`.
    pub(crate) fn shared(&self, branch: u64, file: FileKey) -> bool {
        (self.state().claims.get(&file))
            .is_some_and(|claim| claim.holders.iter().any(|holder| *holder != branch))
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

    /// `branch`'s forks not yet opened, which hold what they inherited of it, and where they were
    /// forked.
    fn heirs(&self, branch: u64) -> Vec<(u64, Origin)> {
        (self.entries.iter())
            .filter(|(_, entry)| !entry.record.resolved && !entry.record.deleted)
            .filter_map(|(child, entry)| Some((*child, entry.record.origin?)))
            .filter(|(_, origin)| origin.parent == branch)
            .collect()
    }

    fn release(&mut self, branch: u64, epoch: u64, files: Vec<FileKey>) {
        for file in files {
            let Some(claim) = self.claims.get_mut(&file) else { continue };
            if !claim.holders.remove(&branch) {
                continue;
            }
            claim.drops.push((branch, epoch));
            if claim.holders.is_empty() {
                let claim = self.claims.remove(&file).unwrap();
                self.doomed.push((file, claim.drops));
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

/// Whether a fork made at `origin` inherited `extent` from its parent: an extent of another branch
/// the parent held, or one the parent minted before the fork.
fn inherited(extent: ExtentId, origin: &Origin) -> bool {
    extent.branch != origin.parent || extent.sequence < origin.next_sequence
}

/// The index files of each branch that forks not yet opened need.
fn pinned_indexes(entries: &BTreeMap<u64, Entry>) -> HashMap<u64, BTreeSet<u64>> {
    let mut pinned: HashMap<u64, BTreeSet<u64>> = HashMap::new();
    for entry in entries.values().filter(|entry| !entry.record.resolved && !entry.record.deleted) {
        if let Some(origin) = entry.record.origin {
            pinned.entry(origin.parent).or_default().insert(origin.index);
        }
    }
    pinned
}

/// The numbers of the files in `path`, in order.
pub(crate) fn list_numbers<D: Dir>(dir: &D, path: &str) -> Result<Vec<u64>, Error> {
    let mut numbers = Vec::new();
    for name in dir.list(path)? {
        numbers.push(parse_number(&name).ok_or_else(|| anyhow!("{path}/{name} is no file"))?);
    }
    numbers.sort_unstable();
    Ok(numbers)
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

/// Creates `branch`'s directories of segments, extents and index files, where it has none.
pub(crate) fn create_branch_dirs<D: Dir>(dir: &D, branch: u64) -> Result<(), Error> {
    for kind in [SEGMENTS, EXTENTS, INDEX, OPEN_SEGMENTS, OPEN_EXTENTS] {
        let path = branch_dir(kind, branch);
        match dir.create_dir(&path) {
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            created => created.with_context(|| format!("creating {path}"))?,
        }
    }
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
