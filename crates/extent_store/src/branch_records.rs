// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The branches' records (see the crate's docs): each branch's newest `branches/<branch>.<version>`
//! file, whole, which says its capacity, where it was forked from, whether its index holds its
//! state yet, and whether it is deleted; and, in memory, each record, its version, and the store
//! that has its branch open, if any.
//!
//! A record changes by writing its next version, durably, then removing the last, with no lock
//! held: only its taker changes it, the store of the opening that has the branch open, or the
//! branch's deletion, so that no other change races its versions, and no other branch waits for
//! its I/O. A fork reserves its branch from before the commit that names it until its record is
//! written, and a deletion while it writes and removes its records ([`Forking`], [`Deletion`]):
//! openings, forks and deletions of the branch wait until it is done, and act on what came of it,
//! as do the queries of which branches the store has, but for a fork whose commit is being made,
//! which is not made yet.

use crate::dir::{Dir, File as _, read_file};
use crate::layout::{BRANCHES, BranchRecord, parse_record_name, record_path};
use crate::sync::{Condvar, Mutex, MutexGuard, rules};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// See the module's docs.
pub(crate) struct BranchRecords {
    /// The branch opening the store, rather than one of its branches, opens.
    first: u64,
    table: Mutex<Table>,
    /// Notified when a branch is reserved no longer.
    settled: Condvar,
}

struct Table {
    entries: BTreeMap<u64, Entry>,
    /// The branches being forked or deleted ([`Reservation`]).
    reserved: BTreeSet<u64>,
    /// fxdax: those of them whose forks' commits are being made, before their records are written
    /// ([`Forking`]): queries of the branches the store has wait not for them, as the forks are not
    /// made yet, and a commit may wait long, for writes in progress (TASK-79.78).
    committing: BTreeSet<u64>,
    /// The last opening of a branch's.
    openings: u64,
}

struct Entry {
    record: BranchRecord,
    /// The version of its record's file.
    version: u64,
    /// The store's opening of it, if a store has it open.
    open: Option<u64>,
}

/// Who changes a branch's record: the store of the opening that has it open, or its deletion.
#[derive(Clone, Copy, Debug)]
enum Taker {
    Store(u64),
    Deletion,
}

impl BranchRecords {
    /// The records in `dir` of a store whose first branch is `first`: each branch's newest that is
    /// whole. Older versions go, and torn ones.
    pub(crate) fn read<D: Dir>(dir: &D, first: u64) -> Result<Self, Error> {
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
            entries.insert(branch, Entry { record, version, open: None });
        }
        ensure!(entries.contains_key(&first), "the store's first branch has no record");
        let (reserved, committing) = (BTreeSet::new(), BTreeSet::new());
        let table = Table { entries, reserved, committing, openings: 0 };
        Ok(Self { first, table: Mutex::new(table), settled: Condvar::new() })
    }

    /// The branch opening the store, rather than one of its branches, opens.
    pub(crate) fn first(&self) -> u64 {
        self.first
    }

    /// fxdax: whether the records in `dir` are those it read: no other process changed them since
    /// (TASK-79.5).
    pub(crate) fn unchanged<D: Dir>(&self, dir: &D) -> Result<bool, Error> {
        let mut listed = BTreeSet::new();
        for name in dir.list(BRANCHES)? {
            let record = parse_record_name(&name)
                .ok_or_else(|| anyhow!("{BRANCHES}/{name} is no branch's record"))?;
            listed.insert(record);
        }
        let table = self.lock();
        let read = table.entries.iter().map(|(branch, entry)| (*branch, entry.version));
        Ok(listed == read.collect())
    }

    /// fxdax: the records `read`, read again, in place of these, of which no branch is open, nor
    /// being forked or deleted (TASK-79.5). The openings go on being numbered as they were.
    pub(crate) fn replace(&self, read: BranchRecords) {
        let mut table = self.lock();
        debug_assert!(table.reserved.is_empty(), "records replaced while a branch is reserved");
        debug_assert!(table.entries.values().all(|entry| entry.open.is_none()));
        let openings = table.openings;
        *table = Table { openings, ..read.table.into_inner().unwrap() };
    }

    /// Each branch's record, deleted branches' included.
    pub(crate) fn snapshot(&self) -> BTreeMap<u64, BranchRecord> {
        (self.lock().entries.iter()).map(|(branch, entry)| (*branch, entry.record)).collect()
    }

    /// Whether the store was deleted, its first branch: once the forks and deletions in flight are
    /// done.
    pub(crate) fn is_deleted(&self) -> bool {
        self.quiet().entries[&self.first].record.deleted
    }

    /// Whether the store has `branch`, not deleted: once the forks and deletions in flight are done.
    pub(crate) fn has(&self, branch: u64) -> bool {
        self.quiet().entries.get(&branch).is_some_and(|entry| !entry.record.deleted)
    }

    /// Whether a store has `branch` open.
    pub(crate) fn is_open(&self, branch: u64) -> bool {
        self.lock().entries.get(&branch).is_some_and(|entry| entry.open.is_some())
    }

    /// The branches, with the branch each was forked from, if any: once the forks and deletions in
    /// flight are done.
    pub(crate) fn branches(&self) -> Vec<(u64, Option<u64>)> {
        (self.quiet().entries.iter())
            .filter(|(_, entry)| !entry.record.deleted)
            .map(|(branch, entry)| (*branch, entry.record.origin.map(|origin| origin.parent)))
            .collect()
    }

    /// The index files of each branch that live forks need (see [`Table::pinned_indexes`]).
    pub(crate) fn pinned_indexes(&self) -> HashMap<u64, BTreeSet<u64>> {
        self.lock().pinned_indexes()
    }

    /// The index files of `branch` that forks need: those of the states they are over.
    pub(crate) fn pinned(&self, branch: u64) -> BTreeSet<u64> {
        self.lock().pinned_indexes().remove(&branch).unwrap_or_default()
    }

    /// Opens `branch`, which no store has open, once a fork or deletion of it in flight is done:
    /// the opening's token, and the branch's record.
    pub(crate) fn open(&self, branch: u64) -> Result<(u64, BranchRecord), Error> {
        let mut table = self.settled(self.lock(), branch)?;
        table.openings += 1;
        let token = table.openings;
        let entry = table.live(branch, self.first)?;
        ensure!(entry.open.is_none(), "branch {branch:#x} is open");
        entry.open = Some(token);
        Ok((token, entry.record))
    }

    /// Records that the store of `branch`'s opening `token` closed it. Returns whether it had it
    /// open.
    pub(crate) fn closed(&self, branch: u64, token: u64) -> bool {
        match self.lock().entries.get_mut(&branch) {
            Some(entry) if entry.open == Some(token) => {
                entry.open = None;
                true
            }
            _ => false,
        }
    }

    /// Changes `branch`'s record as `change` says, durably, for the store of its opening `token`,
    /// which has it open.
    pub(crate) fn change<D: Dir>(
        &self,
        dir: &D,
        branch: u64,
        token: u64,
        change: impl FnOnce(BranchRecord) -> BranchRecord,
    ) -> Result<(), Error> {
        self.rewrite(dir, branch, Taker::Store(token), change)
    }

    /// Reserves `child`, a branch the store does not have, once a fork or deletion of it in
    /// flight is done, for a fork, whose commit is made before its record is written
    /// ([`Forking::write`]).
    pub(crate) fn fork(&self, child: u64) -> Result<Forking<'_>, Error> {
        let mut table = self.settled(self.lock(), child)?;
        ensure!(!table.entries.contains_key(&child), "the store has a branch {child:#x}");
        table.committing.insert(child);
        Ok(Forking { reservation: self.reserve(&mut table, child) })
    }

    /// Reserves `branch`, which no store has open, once a fork or deletion of it in flight is
    /// done, for its deletion.
    pub(crate) fn delete(&self, branch: u64) -> Result<Deletion<'_>, Error> {
        let mut table = self.settled(self.lock(), branch)?;
        let entry = table.live(branch, self.first)?;
        ensure!(entry.open.is_none(), "branch {branch:#x} is open");
        Ok(Deletion { reservation: self.reserve(&mut table, branch) })
    }

    /// Removes the record of `branch`, deleted, and forgets the branch, whatever the removal gave:
    /// its ID is free again. The opening forgets so the deleted branches nothing pins; a deletion,
    /// its branch ([`Deletion::forget`]).
    pub(crate) fn forget<D: Dir>(&self, dir: &D, branch: u64) -> Result<(), Error> {
        let version = self.lock().entries[&branch].version;
        let removed = dir.remove(&record_path(branch, version));
        self.lock().entries.remove(&branch);
        Ok(removed?)
    }

    fn lock(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap()
    }

    /// `table`, once `branch` is reserved no longer: its fork or deletion done. Without the rule
    /// to wait, failing at once if it is reserved.
    fn settled<'a>(
        &'a self,
        mut table: MutexGuard<'a, Table>,
        branch: u64,
    ) -> Result<MutexGuard<'a, Table>, Error> {
        while table.reserved.contains(&branch) {
            ensure!(
                rules().wait_for_forks_and_deletions,
                "branch {branch:#x} is being forked or deleted"
            );
            table = self.settled.wait(table).unwrap();
        }
        Ok(table)
    }

    /// The table, once no branch is reserved but for forks whose commits are being made: what
    /// came of the forks and deletions in flight, which a query of the branches the store has
    /// tells. Without the rule to wait, at once.
    fn quiet(&self) -> MutexGuard<'_, Table> {
        let mut table = self.lock();
        while rules().wait_for_forks_and_deletions && table.settling() {
            table = self.settled.wait(table).unwrap();
        }
        table
    }

    /// Reserves `branch`, which `table` holds no reservation of.
    fn reserve(&self, table: &mut Table, branch: u64) -> Reservation<'_> {
        table.reserved.insert(branch);
        Reservation { records: self, branch }
    }

    /// Changes `branch`'s record as `change` says, for `taker`, which must have it: its next
    /// version written, durably, then its last removed, holding no lock; then, both done, in
    /// memory. Only the taker changes it, so that no other change races its versions.
    fn rewrite<D: Dir>(
        &self,
        dir: &D,
        branch: u64,
        taker: Taker,
        change: impl FnOnce(BranchRecord) -> BranchRecord,
    ) -> Result<(), Error> {
        let (taken, version, record) = {
            let table = self.lock();
            let entry = &table.entries[&branch];
            let taken = match taker {
                Taker::Store(token) => entry.open == Some(token),
                Taker::Deletion => table.reserved.contains(&branch),
            };
            (taken, entry.version, entry.record)
        };
        assert!(taken, "branch {branch:#x}'s record changed by {taker:?}, which has it not");
        let record = change(record);
        write_record(dir, branch, version + 1, &record)?;
        dir.remove(&record_path(branch, version))?;
        let mut table = self.lock();
        let entry = table.entries.get_mut(&branch).unwrap();
        entry.version = version + 1;
        entry.record = record;
        Ok(())
    }
}

impl Table {
    /// Whether a fork or deletion in flight but a fork whose commit is being made reserves its
    /// branch: one whose records are being written or removed.
    fn settling(&self) -> bool {
        self.reserved.difference(&self.committing).next().is_some()
    }

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

    /// The index files of each branch that live forks need: those of the states they are over, and
    /// of the states those are over, up their lines, deleted branches' included (TASK-31).
    fn pinned_indexes(&self) -> HashMap<u64, BTreeSet<u64>> {
        let mut pinned: HashMap<u64, BTreeSet<u64>> = HashMap::new();
        for entry in self.entries.values().filter(|entry| !entry.record.deleted) {
            let mut record = entry.record;
            while let Some(origin) = record.over_parent() {
                if !pinned.entry(origin.parent).or_default().insert(origin.index) {
                    // Its line, from there, pinned already.
                    break;
                }
                let Some(parent) = self.entries.get(&origin.parent) else { break };
                record = parent.record;
            }
        }
        pinned
    }
}

/// A branch reserved for its fork or its deletion, until dropped: no opening, fork or deletion of
/// it begins meanwhile, and queries of the branches the store has wait.
struct Reservation<'a> {
    records: &'a BranchRecords,
    branch: u64,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        {
            let mut table = self.records.lock();
            table.reserved.remove(&self.branch);
            table.committing.remove(&self.branch);
        }
        self.records.settled.notify_all();
    }
}

/// A fork's branch, reserved from before the fork's commit until its record is written
/// ([`BranchRecords::fork`]).
pub(crate) struct Forking<'a> {
    reservation: Reservation<'a>,
}

impl Forking<'_> {
    /// The fork's branch.
    pub(crate) fn child(&self) -> u64 {
        self.reservation.branch
    }

    /// Writes the fork's record's first version, `record`, durably, its commit made: queries of
    /// the branches the store has wait for the fork from now.
    pub(crate) fn write<D: Dir>(&self, dir: &D, record: &BranchRecord) -> Result<(), Error> {
        let Reservation { records, branch } = self.reservation;
        records.lock().committing.remove(&branch);
        write_record(dir, branch, 0, record)
    }

    /// Records the fork, its record, `record`, written: what waits for it finds it.
    pub(crate) fn found(self, record: BranchRecord) {
        let entry = Entry { record, version: 0, open: None };
        self.reservation.records.lock().entries.insert(self.reservation.branch, entry);
    }
}

/// A branch's deletion, which reserves it ([`BranchRecords::delete`]).
pub(crate) struct Deletion<'a> {
    reservation: Reservation<'a>,
}

impl Deletion<'_> {
    /// Records the branch deleted, durably: its record's next version says so.
    pub(crate) fn mark<D: Dir>(&self, dir: &D) -> Result<(), Error> {
        let Reservation { records, branch } = self.reservation;
        let deleted = |record| BranchRecord { deleted: true, ..record };
        records.rewrite(dir, branch, Taker::Deletion, deleted)
    }

    /// The index files of the branch that forks need: those of the states they are over.
    pub(crate) fn pinned(&self) -> BTreeSet<u64> {
        self.reservation.records.pinned(self.reservation.branch)
    }

    /// Removes the branch's record, and forgets the branch ([`BranchRecords::forget`]): its ID is
    /// free again, once the deletion is done.
    pub(crate) fn forget<D: Dir>(&self, dir: &D) -> Result<(), Error> {
        self.reservation.records.forget(dir, self.reservation.branch)
    }
}

/// Writes `branch`'s record's `version`, durably.
pub(crate) fn write_record<D: Dir>(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemDir;
    use crate::layout::Origin;

    const FIRST: u64 = 7;
    const CHILD: u64 = 8;

    fn record(capacity: u64) -> BranchRecord {
        BranchRecord { capacity, origin: None, resolved: true, layered: false, deleted: false }
    }

    /// The record of a fork of the first branch, not yet opened.
    fn fork() -> BranchRecord {
        let origin = Origin { parent: FIRST, epoch: 1, next_sequence: 1, index: 0 };
        BranchRecord { origin: Some(origin), resolved: false, layered: true, ..record(1) }
    }

    /// A store's directory of records: the first branch's, and, if `child`, a fork's, both their
    /// first versions.
    fn written(child: bool) -> MemDir {
        let dir = MemDir::new();
        dir.create_dir(BRANCHES).unwrap();
        write_record(&dir, FIRST, 0, &record(1)).unwrap();
        if child {
            write_record(&dir, CHILD, 0, &fork()).unwrap();
        }
        dir
    }

    /// The first part of `branch`'s record's `version`: torn.
    fn write_torn(dir: &MemDir, branch: u64, version: u64) {
        let file = dir.create(&record_path(branch, version), false).unwrap();
        file.write_at(b"fxdax extent store branch\ncapacity 3\n", 0).unwrap();
    }

    /// The records' files.
    fn paths(dir: &MemDir) -> Vec<String> {
        dir.list(BRANCHES).unwrap().into_iter().map(|name| format!("{BRANCHES}/{name}")).collect()
    }

    /// A change writes the record's next version, then removes the last, which reading anew
    /// finds. One whose last version stays fails, and changes nothing in memory.
    #[test]
    fn changes_write_the_next_version_and_remove_the_last() {
        let dir = written(false);
        let records = BranchRecords::read(&dir, FIRST).unwrap();
        let (token, opened) = records.open(FIRST).unwrap();
        assert_eq!(opened, record(1));
        let capacity = |capacity| move |record| BranchRecord { capacity, ..record };
        records.change(&dir, FIRST, token, capacity(2)).unwrap();
        assert_eq!(paths(&dir), [record_path(FIRST, 1)]);
        records.change(&dir, FIRST, token, capacity(3)).unwrap();
        assert_eq!(paths(&dir), [record_path(FIRST, 2)]);
        assert_eq!(records.snapshot(), BTreeMap::from([(FIRST, record(3))]));
        let read = BranchRecords::read(&dir, FIRST).unwrap();
        assert_eq!(read.snapshot(), BTreeMap::from([(FIRST, record(3))]));

        // Its next version written, its last not removed: create, write, sync, sync the
        // directory, the barrier, then the removal, which fails.
        dir.fail_after(Some(5));
        let failed = records.change(&dir, FIRST, token, capacity(4)).unwrap_err();
        assert_eq!(failed.to_string(), "the file system crashed");
        assert_eq!(records.snapshot(), BTreeMap::from([(FIRST, record(3))]));
    }

    /// Reading keeps each branch's newest whole record: older versions go, and torn ones, and the
    /// record of a fork whose first version was torn, which was never made. A first branch with no
    /// whole record is a store never made.
    #[test]
    fn reading_keeps_the_newest_whole_records() {
        let dir = written(false);
        write_record(&dir, FIRST, 1, &record(2)).unwrap();
        write_torn(&dir, FIRST, 2);
        write_torn(&dir, CHILD, 0);
        let records = BranchRecords::read(&dir, FIRST).unwrap();
        assert_eq!(records.snapshot(), BTreeMap::from([(FIRST, record(2))]));
        assert_eq!(paths(&dir), [record_path(FIRST, 1)]);

        let dir = MemDir::new();
        dir.create_dir(BRANCHES).unwrap();
        write_torn(&dir, FIRST, 0);
        let error = BranchRecords::read(&dir, FIRST).err().unwrap();
        assert_eq!(error.to_string(), "branch 0x7 has no whole record");
    }

    /// A fork whose record fails to be written leaves no branch, and lets its reservation go: a
    /// fork to the same branch may be made then.
    #[test]
    fn failed_forks_leave_no_branch() {
        let dir = written(false);
        let records = BranchRecords::read(&dir, FIRST).unwrap();
        dir.fail_after(Some(0));
        let forking = records.fork(CHILD).unwrap();
        let failed = forking.write(&dir, &fork()).unwrap_err();
        assert_eq!(failed.to_string(), format!("creating {}", record_path(CHILD, 0)));
        drop(forking);
        dir.fail_after(None);
        assert!(!records.has(CHILD));

        let forking = records.fork(CHILD).unwrap();
        forking.write(&dir, &fork()).unwrap();
        forking.found(fork());
        assert_eq!(records.branches(), [(FIRST, None), (CHILD, Some(FIRST))]);
        let again = records.fork(CHILD).err().unwrap();
        assert_eq!(again.to_string(), "the store has a branch 0x8");
    }

    /// A deletion dropped lets its branch go: unmarked, the branch is as it was; marked, deleted;
    /// and forgotten, its record gone and its ID free.
    #[test]
    fn dropped_deletions_let_their_branches_go() {
        let dir = written(true);
        let records = BranchRecords::read(&dir, FIRST).unwrap();
        drop(records.delete(CHILD).unwrap());
        let (token, _) = records.open(CHILD).unwrap();
        assert_eq!(records.delete(CHILD).err().unwrap().to_string(), "branch 0x8 is open");
        assert!(records.closed(CHILD, token));

        let deletion = records.delete(CHILD).unwrap();
        deletion.mark(&dir).unwrap();
        drop(deletion);
        assert!(!records.has(CHILD));
        let opening = records.open(CHILD).err().unwrap();
        assert_eq!(opening.to_string(), "the store has no branch 0x8");
        assert_eq!(paths(&dir), [record_path(FIRST, 0), record_path(CHILD, 1)]);

        let dir = written(true);
        let records = BranchRecords::read(&dir, FIRST).unwrap();
        let deletion = records.delete(CHILD).unwrap();
        deletion.mark(&dir).unwrap();
        deletion.forget(&dir).unwrap();
        drop(deletion);
        assert_eq!(paths(&dir), [record_path(FIRST, 0)]);
        let forking = records.fork(CHILD).unwrap();
        forking.write(&dir, &fork()).unwrap();
        forking.found(fork());
        assert!(records.has(CHILD));
    }

    /// Only the store that has a branch open changes its record: not one that closed it.
    #[test]
    #[should_panic(expected = "branch 0x7's record changed by Store(1), which has it not")]
    fn changes_by_a_store_without_the_branch_panic() {
        let dir = written(false);
        let records = BranchRecords::read(&dir, FIRST).unwrap();
        let (token, _) = records.open(FIRST).unwrap();
        assert!(records.closed(FIRST, token));
        let _ = records.change(&dir, FIRST, token, |record| record);
    }
}
