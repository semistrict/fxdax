// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Which branches of a lineage hold each file of extents' data, and which files may go (see the
//! crate's docs, Forks): a branch holds the files of the extents it holds of its own, not through
//! a layer, which it claims; and, if a fork's, through the layer it is over, its parent's state at
//! the fork, the files of the layer's extents, but for those it let go of (TASK-31). A file no
//! branch holds goes once the commits that let it go are durable.
//!
//! The ledger is memory alone: it is told what branches did, in events (the lineage opened, a
//! fork, claims, releases, a recovery, durable commits, a deletion), and says which files may go
//! ([`Ledger::removable`]), which the lineage removes.

use crate::layers::{Holdings, Layer};
use crate::layout::{FileKey, Origin};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// See the module's docs. It keeps these true, which [`Ledger::check`] checks after each event
/// in debug builds:
///
/// - a branch is over a layer exactly when it is among the layer's holders: only
///   [`Ledger::attach`] and [`Ledger::detach`] change either;
/// - a layer is kept only while a branch is over it;
/// - the branches that let a file of a layer go are holders of the layer, which holds the file;
/// - a branch claims a file exactly when it is among the holders of the file's claim: the claims
///   are kept by file, and by branch too, so that a branch's deletion or recovery finds what it
///   claims without looking through every file's claim.
pub(crate) struct Ledger {
    /// The branches the lineage knows: live as it opened, forked since, or deleted since, but for
    /// those whose records went.
    branches: BTreeMap<u64, Branch>,
    /// The layers forks are over, by their identities ([`layer_key`]), and the forks over each,
    /// which hold its files but for those they let go of.
    layers: HashMap<usize, Inherited>,
    /// The branches holding extents in each file of extents' data, of their own: not through the
    /// layer they are over. And the releases of the file.
    claims: HashMap<FileKey, Claim>,
    /// The files no branch holds, which go once the commits that let them go are durable.
    doomed: Vec<Doomed>,
    /// fxdax: how often a branch let go of files another made, as a fork lets go of its parent's,
    /// and a deletion of what it inherited: each time, those files' makers may hold them alone
    /// ([`Ledger::shared`]), which they find out once it changed (TASK-79.2).
    unshared: u64,
}

/// A branch, as the ledger knows it.
struct Branch {
    /// The layer it is over, if a fork's over one.
    over: Option<Over>,
    /// Its last epoch that is durable.
    durable: u64,
    /// The files it holds extents in of its own, which it claims: its claims ([`Ledger::claims`]),
    /// by branch.
    claimed: BTreeSet<FileKey>,
}

/// The layer a fork is over: its parent's state at the fork, which was as of `origin`.
pub(crate) struct Over {
    pub origin: Origin,
    pub layer: Arc<Layer>,
}

/// A branch live as the lineage opened ([`Ledger::opened`]): the layer it is over, if a fork's;
/// and, unless it is a fork not yet opened, which holds its layer alone, the epoch it recovered,
/// its last durable, and what it holds as of it.
pub(crate) struct Live {
    pub branch: u64,
    pub over: Option<Over>,
    pub recovered: Option<(u64, Holdings)>,
}

#[derive(Default)]
struct Claim {
    holders: BTreeSet<u64>,
    /// The branches that let it go, and their commits that did.
    releases: Vec<Release>,
}

/// A branch's letting a file go, by its commit `epoch`: durable once that commit is, or at once if
/// it is 0, which a deletion's is, its record durable.
#[derive(Clone, Copy)]
struct Release {
    branch: u64,
    epoch: u64,
}

/// A file no branch holds, which goes once its releases are durable; and the holders of the layers
/// holding it that let it go, by layer, whose marks dooming it erased: it takes them back if a
/// branch recovered holds it again ([`Ledger::recovered`]).
struct Doomed {
    file: FileKey,
    releases: Vec<Release>,
    dropped: Vec<(usize, BTreeSet<u64>)>,
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

/// A layer's key in [`Ledger::layers`], its identity: its address, as it is held there. A branch
/// forking twice with nothing changed in between gives both forks one layer
/// ([`Extents::freeze`](crate::layers::Extents::freeze)), which they share an entry of.
fn layer_key(layer: &Arc<Layer>) -> usize {
    Arc::as_ptr(layer) as usize
}

impl Ledger {
    /// The ledger of a lineage just opened, of its `live` branches: each over its layer, if a
    /// fork's, holding the files of its own, and having let go of those of its layer it holds no
    /// extent in. It dooms none: a branch found after another that let a file go may hold it, and
    /// the opening removes the files no branch holds ([`Ledger::held`]).
    pub(crate) fn opened(live: Vec<Live>) -> Self {
        let mut ledger = Self {
            branches: BTreeMap::new(),
            layers: HashMap::new(),
            claims: HashMap::new(),
            doomed: Vec::new(),
            unshared: 0,
        };
        for Live { branch, over, recovered } in live {
            ledger.insert(branch, over);
            if let Some((epoch, Holdings { own, dropped })) = recovered {
                ledger.branches.get_mut(&branch).unwrap().durable = epoch;
                ledger.claim(branch, own);
                for file in dropped {
                    ledger.let_go_of_inherited(branch, file);
                }
            }
        }
        ledger.check();
        ledger
    }

    /// Records `branch`, a fork just recorded, over `over`'s layer, whose files it holds.
    pub(crate) fn attach(&mut self, branch: u64, over: Over) {
        self.insert(branch, Some(over));
        self.check();
    }

    /// Records that `branch`, a branch it knows, holds extents in `files`, of its own.
    pub(crate) fn claim(&mut self, branch: u64, files: impl IntoIterator<Item = FileKey>) {
        let Some(known) = self.branches.get_mut(&branch) else {
            panic!("branch {branch:#x}, which the ledger does not know, claims files");
        };
        for file in files {
            if known.claimed.insert(file) {
                self.claims.entry(file).or_default().holders.insert(branch);
            }
        }
        self.check();
    }

    /// Records that `branch`'s commit `epoch` lets `files` go, which it held of its own or through
    /// the layer it is over: they go once no branch holds them, and the commits that let them go
    /// are durable.
    pub(crate) fn release(&mut self, branch: u64, epoch: u64, files: Vec<FileKey>) {
        for file in files {
            if !self.let_go_of_own(branch, file) && !self.let_go_of_inherited(branch, file) {
                continue;
            }
            if file.branch() != branch {
                self.unshared += 1;
            }
            self.claims.entry(file).or_default().releases.push(Release { branch, epoch });
            self.doom_unless_held(file);
        }
        self.check();
    }

    /// Records that `branch` holds extents in exactly `holdings.own` of its own, and none in
    /// `holdings.dropped` of the layer it is over, as its state, just recovered as of its durable
    /// `epoch`, says: what its commits after it, never durable, did is undone (TASK-79.5).
    pub(crate) fn recovered(&mut self, branch: u64, epoch: u64, holdings: &Holdings) {
        self.undo_after(branch, epoch, holdings);
        let Holdings { own, dropped } = holdings;
        let held: Vec<FileKey> = self.branches[&branch].claimed.difference(own).copied().collect();
        self.claim(branch, own.iter().copied());
        self.release(branch, 0, held);
        self.release(branch, 0, dropped.iter().copied().collect());
        self.branches.get_mut(&branch).unwrap().durable = epoch;
    }

    /// Undoes what `branch`'s commits after `epoch`, never durable, did, its state as of `epoch`
    /// holding `holdings` (TASK-79.5): their releases are as durable as that state; the files they
    /// doomed that it holds are held again, by those who held them as they were doomed; and what
    /// they let go of the layer it is over is held again, but for what it dropped, which
    /// [`Ledger::recovered`] lets go of anew.
    fn undo_after(&mut self, branch: u64, epoch: u64, holdings: &Holdings) {
        let releases = (self.claims.values_mut().flat_map(|claim| &mut claim.releases))
            .chain(self.doomed.iter_mut().flat_map(|doomed| &mut doomed.releases));
        for release in releases.filter(|release| release.branch == branch) {
            release.epoch = release.epoch.min(epoch);
        }
        let over = self.branches[&branch].over.as_ref().map(|over| layer_key(&over.layer));
        if let Some(inherited) = over.and_then(|key| self.layers.get_mut(&key)) {
            for by in inherited.dropped.values_mut() {
                by.remove(&branch);
            }
            inherited.dropped.retain(|_, by| !by.is_empty());
        }
        let inherits = |file: FileKey| {
            !holdings.dropped.contains(&file)
                && over.is_some_and(|key| self.layers[&key].layer.holds(file))
        };
        let (back, doomed): (Vec<Doomed>, Vec<Doomed>) = std::mem::take(&mut self.doomed)
            .into_iter()
            .partition(|doomed| holdings.own.contains(&doomed.file) || inherits(doomed.file));
        self.doomed = doomed;
        for Doomed { file, releases, dropped } in back {
            self.claims.entry(file).or_default().releases.extend(releases);
            for (key, by) in dropped {
                let Some(inherited) = self.layers.get_mut(&key) else { continue };
                let by: BTreeSet<u64> = (by.into_iter())
                    .filter(|holder| *holder != branch && inherited.holders.contains(holder))
                    .collect();
                if !by.is_empty() {
                    inherited.dropped.insert(file, by);
                }
            }
        }
    }

    /// Records that `branch`'s commits up to `epoch` are durable.
    pub(crate) fn durable(&mut self, branch: u64, epoch: u64) {
        if let Some(branch) = self.branches.get_mut(&branch) {
            branch.durable = branch.durable.max(epoch);
        }
    }

    /// Records that `branch` is deleted, durably: it lets go, at once, of what it held, of its own
    /// and of the layer it is over, if any, whose files no other branch holds go, all of them with
    /// the last branch over it. It stays known, with its durable epoch, which what it let go of
    /// before waits for, until it is forgotten ([`Ledger::forget`]).
    pub(crate) fn detach(&mut self, branch: u64) {
        // Its forks hold what they inherited of its extents: through the layers they are over.
        let held: Vec<FileKey> = (self.branches.get(&branch))
            .map_or_else(Vec::new, |known| known.claimed.iter().copied().collect());
        // Its record says it is deleted, durably: its letting them go is durable.
        self.release(branch, 0, held);
        let over = self.branches.get_mut(&branch).and_then(|branch| branch.over.take());
        if let Some(over) = over {
            self.leave_layer(branch, &over.layer);
        }
        self.check();
    }

    /// Forgets `branch`, deleted, as its record goes: the commits of a branch it does not know
    /// are durable, as it says what may go ([`Ledger::removable`]), until a fork takes the ID.
    pub(crate) fn forget(&mut self, branch: u64) {
        self.branches.remove(&branch);
        self.check();
    }

    /// The files doomed whose letting go is durable, which go now.
    pub(crate) fn removable(&mut self) -> Vec<FileKey> {
        let branches = &self.branches;
        let durable = |release: &Release| {
            release.epoch == 0
                || branches
                    .get(&release.branch)
                    .is_none_or(|branch| branch.durable >= release.epoch)
        };
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.doomed)
            .into_iter()
            .partition(|doomed| doomed.releases.iter().all(durable));
        self.doomed = kept;
        gone.into_iter().map(|doomed| doomed.file).collect()
    }

    /// Whether a branch holds an extent in `file`: of its own, or through the layer it is over.
    pub(crate) fn held(&self, file: FileKey) -> bool {
        self.claims.get(&file).is_some_and(|claim| !claim.holders.is_empty())
            || self.layers.values().any(|inherited| inherited.holds(file, None))
    }

    /// Whether a branch other than `branch` holds an extent in `file`: of its own, or through the
    /// layer it is over.
    pub(crate) fn shared(&self, branch: u64, file: FileKey) -> bool {
        (self.claims.get(&file))
            .is_some_and(|claim| claim.holders.iter().any(|holder| *holder != branch))
            || self.layers.values().any(|inherited| inherited.holds(file, Some(branch)))
    }

    /// fxdax: how often a branch let go of files another made: once it changed, a branch's files
    /// it found shared may be its alone (TASK-79.2).
    pub(crate) fn unshared(&self) -> u64 {
        self.unshared
    }

    /// The layer `branch` is over, its parent's state at the fork, if a fork's over it.
    pub(crate) fn base(&self, branch: u64) -> Option<Arc<Layer>> {
        Some(self.branches.get(&branch)?.over.as_ref()?.layer.clone())
    }

    /// `parent`'s state at its latest fork of those live, which that fork is over, if any.
    pub(crate) fn latest_layer_of(&self, parent: u64) -> Option<Arc<Layer>> {
        (self.branches.values())
            .filter_map(|branch| branch.over.as_ref())
            .filter(|over| over.origin.parent == parent)
            .max_by_key(|over| (over.origin.index, over.origin.epoch))
            .map(|over| over.layer.clone())
    }

    /// Records `branch`, over `over`'s layer, if any, whose files it holds; nothing durable yet.
    fn insert(&mut self, branch: u64, over: Option<Over>) {
        if let Some(over) = &over {
            let layer = over.layer.clone();
            let inherited = self.layers.entry(layer_key(&layer)).or_insert_with(|| Inherited {
                layer,
                holders: BTreeSet::new(),
                dropped: HashMap::new(),
            });
            inherited.holders.insert(branch);
        }
        self.branches.insert(branch, Branch { over, durable: 0, claimed: BTreeSet::new() });
    }

    /// Records that `branch` holds no extent in `file` of its own, if it claimed it. Returns
    /// whether it did.
    fn let_go_of_own(&mut self, branch: u64, file: FileKey) -> bool {
        let claimed =
            self.branches.get_mut(&branch).is_some_and(|known| known.claimed.remove(&file));
        if claimed {
            self.claims.get_mut(&file).unwrap().holders.remove(&branch);
        }
        claimed
    }

    /// Records that `branch` holds no extent in `file` of the layer it is over, if it is over one
    /// holding it. Returns whether it held it.
    fn let_go_of_inherited(&mut self, branch: u64, file: FileKey) -> bool {
        let Some(over) = self.branches.get(&branch).and_then(|branch| branch.over.as_ref()) else {
            return false;
        };
        let inherited = self.layers.get_mut(&layer_key(&over.layer)).unwrap();
        inherited.layer.holds(file) && inherited.dropped.entry(file).or_default().insert(branch)
    }

    /// Dooms `file`, unless a branch holds it.
    fn doom_unless_held(&mut self, file: FileKey) {
        if self.held(file) {
            return;
        }
        let releases = self.claims.remove(&file).map_or_else(Vec::new, |claim| claim.releases);
        let dropped = (self.layers.iter_mut())
            .filter_map(|(key, inherited)| Some((*key, inherited.dropped.remove(&file)?)))
            .collect();
        self.doomed.push(Doomed { file, releases, dropped });
    }

    /// Records that `branch`, deleted, is over `layer` no longer: the files of it no other branch
    /// holds go, all of them with the last branch over it.
    fn leave_layer(&mut self, branch: u64, layer: &Arc<Layer>) {
        let key = layer_key(layer);
        let Some(inherited) = self.layers.get_mut(&key) else { return };
        inherited.holders.remove(&branch);
        self.unshared += 1;
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
                self.claims.entry(file).or_default().releases.push(Release { branch, epoch: 0 });
                self.doom_unless_held(file);
            }
        }
    }

    /// Checks, in debug builds, that what it keeps holds together (see the type's docs).
    fn check(&self) {
        if !cfg!(debug_assertions) {
            return;
        }
        for (key, inherited) in &self.layers {
            assert_eq!(*key, layer_key(&inherited.layer), "a layer kept under another's key");
            assert!(!inherited.holders.is_empty(), "a layer no branch is over is kept");
            for holder in &inherited.holders {
                let over = self.branches.get(holder).and_then(|branch| branch.over.as_ref());
                assert!(
                    over.is_some_and(|over| layer_key(&over.layer) == *key),
                    "branch {holder:#x} holds a layer it is not over"
                );
            }
            for (file, by) in &inherited.dropped {
                assert!(inherited.layer.holds(*file), "{file:?} let go of, of a layer without it");
                assert!(
                    by.is_subset(&inherited.holders),
                    "{file:?} let go of by others than holders"
                );
            }
        }
        for (branch, entry) in &self.branches {
            for file in &entry.claimed {
                assert!(
                    (self.claims.get(file)).is_some_and(|claim| claim.holders.contains(branch)),
                    "branch {branch:#x} claims {file:?}, whose claim it is no holder of"
                );
            }
            let Some(over) = &entry.over else { continue };
            assert!(
                (self.layers.get(&layer_key(&over.layer)))
                    .is_some_and(|inherited| inherited.holders.contains(branch)),
                "branch {branch:#x} is over a layer it does not hold"
            );
        }
        for (file, claim) in &self.claims {
            for holder in &claim.holders {
                assert!(
                    (self.branches.get(holder)).is_some_and(|known| known.claimed.contains(file)),
                    "{file:?}'s claim holds branch {holder:#x}, which claims it not"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::Extents;
    use crate::layout::{Data, Extent, Piece, SegmentId};
    use storage_device::ExtentId;

    /// The branch whose states its forks are over.
    const PARENT: u64 = 1;

    /// The file of `branch`'s segment `number`.
    fn segment(branch: u64, number: u64) -> FileKey {
        FileKey::Segment(SegmentId { branch, number })
    }

    /// A state of the parent's, which forks are over, holding an extent in each of its segments
    /// `numbers`.
    fn layer(numbers: &[u64]) -> Arc<Layer> {
        let mut extents = Extents::default();
        for &number in numbers {
            let segment = SegmentId { branch: PARENT, number };
            let piece = Piece { segment, offset: 0, extent_offset: 0, len: 4096 };
            let extent = Extent { len: 4096, data: Data::Pieces(vec![piece]) };
            extents.insert(ExtentId::new(PARENT, number), extent);
        }
        extents.freeze(None, b"")
    }

    /// What a fork of the parent's at its commit `epoch` is over: `layer`.
    fn over(layer: &Arc<Layer>, epoch: u64) -> Over {
        let origin = Origin { parent: PARENT, epoch, next_sequence: 0, index: 0 };
        Over { origin, layer: layer.clone() }
    }

    /// `branch`, live as the lineage opened, over `over` if a fork's, as of its commit `epoch`,
    /// holding `own` of its own and having let `dropped` of its layer go.
    fn live(
        branch: u64,
        over: Option<Over>,
        epoch: u64,
        own: &[FileKey],
        dropped: &[FileKey],
    ) -> Live {
        let holdings = Holdings {
            own: own.iter().copied().collect(),
            dropped: dropped.iter().copied().collect(),
        };
        Live { branch, over, recovered: Some((epoch, holdings)) }
    }

    fn sorted(mut files: Vec<FileKey>) -> Vec<FileKey> {
        files.sort();
        files
    }

    /// A file two branches hold goes once both let it go, and both commits that did are durable.
    #[test]
    fn files_go_once_every_holder_let_them_go_durably() {
        let file = segment(2, 0);
        let mut ledger =
            Ledger::opened(vec![live(2, None, 1, &[file], &[]), live(3, None, 1, &[], &[])]);
        ledger.claim(3, [file]);
        ledger.release(2, 2, vec![file]);
        ledger.durable(2, 2);
        assert!(ledger.held(file));
        assert_eq!(ledger.removable(), []);

        ledger.release(3, 5, vec![file]);
        assert!(!ledger.held(file));
        ledger.durable(3, 4);
        assert_eq!(ledger.removable(), []);
        ledger.durable(3, 5);
        assert_eq!(ledger.removable(), [file]);
        assert_eq!(ledger.removable(), []);

        // Durable in the other order.
        let mut ledger =
            Ledger::opened(vec![live(2, None, 1, &[file], &[]), live(3, None, 1, &[file], &[])]);
        ledger.release(2, 2, vec![file]);
        ledger.release(3, 2, vec![file]);
        ledger.durable(3, 2);
        assert_eq!(ledger.removable(), []);
        ledger.durable(2, 2);
        assert_eq!(ledger.removable(), [file]);
    }

    /// A fork holds the files of the layer it is over, whatever its parent lets go of, until it
    /// lets them go itself.
    #[test]
    fn forks_hold_their_layers_files_until_they_let_them_go() {
        let base = layer(&[0, 1]);
        let (zero, one) = (segment(PARENT, 0), segment(PARENT, 1));
        let mut ledger = Ledger::opened(vec![live(PARENT, None, 1, &[zero, one], &[])]);
        ledger.attach(2, over(&base, 1));
        ledger.release(PARENT, 2, vec![zero, one]);
        ledger.durable(PARENT, 2);
        assert!(ledger.held(zero) && ledger.held(one));
        assert_eq!(ledger.removable(), []);

        ledger.release(2, 1, vec![zero]);
        assert_eq!(ledger.removable(), []);
        ledger.durable(2, 1);
        assert_eq!(ledger.removable(), [zero]);
        assert!(ledger.held(one));
        assert!(Arc::ptr_eq(&ledger.base(2).unwrap(), &base));
        assert!(ledger.base(PARENT).is_none());
    }

    /// The opening dooms nothing: a file one fork over a layer let go of, another still holds;
    /// and one all let go of the opening removes itself, as no branch holds it.
    #[test]
    fn opening_dooms_nothing() {
        let base = layer(&[0]);
        let zero = segment(PARENT, 0);
        let ledger = Ledger::opened(vec![
            live(2, Some(over(&base, 1)), 1, &[], &[zero]),
            live(3, Some(over(&base, 1)), 1, &[], &[]),
        ]);
        assert!(ledger.held(zero));

        let mut ledger = Ledger::opened(vec![
            live(2, Some(over(&base, 1)), 1, &[], &[zero]),
            live(3, Some(over(&base, 1)), 1, &[], &[zero]),
        ]);
        assert!(!ledger.held(zero));
        assert_eq!(ledger.removable(), []);
    }

    /// A fork deleted leaves its layer: the files the forks over it still let go of all go at
    /// once; with the last of them, all the layer's files no other branch holds.
    #[test]
    fn forks_deleted_let_their_layers_files_go() {
        let base = layer(&[0, 1]);
        let (zero, one) = (segment(PARENT, 0), segment(PARENT, 1));
        let mut ledger = Ledger::opened(vec![
            live(2, Some(over(&base, 1)), 1, &[], &[]),
            live(3, Some(over(&base, 1)), 1, &[], &[one]),
        ]);
        ledger.detach(2);
        assert_eq!(ledger.removable(), [one]);
        assert!(ledger.held(zero));

        let base = layer(&[0, 1, 2]);
        let two = segment(PARENT, 2);
        let mut ledger = Ledger::opened(vec![
            live(2, Some(over(&base, 1)), 1, &[], &[]),
            live(4, None, 1, &[two], &[]),
        ]);
        ledger.detach(2);
        assert_eq!(ledger.removable(), [zero, one]);
        assert!(ledger.held(two));
        assert!(ledger.base(2).is_none());
    }

    /// What a branch let go of by a commit never durable, as its flush failed, waits once it is
    /// deleted, until it is forgotten as its record goes; and, if a fork takes its ID, for that
    /// fork's commits.
    #[test]
    fn releases_wait_for_a_deleted_branch_until_it_is_forgotten() {
        let file = segment(2, 0);
        let mut ledger = Ledger::opened(vec![live(2, None, 1, &[file], &[])]);
        ledger.release(2, 2, vec![file]);
        ledger.detach(2);
        assert_eq!(ledger.removable(), []);
        ledger.forget(2);
        assert_eq!(ledger.removable(), [file]);

        let base = layer(&[]);
        let mut ledger = Ledger::opened(vec![live(2, None, 1, &[file], &[])]);
        ledger.release(2, 2, vec![file]);
        ledger.detach(2);
        ledger.forget(2);
        ledger.attach(2, over(&base, 1));
        assert_eq!(ledger.removable(), []);
        ledger.durable(2, 2);
        assert_eq!(ledger.removable(), [file]);
    }

    /// A branch recovered lets go, at once, of what it claimed that its state no longer holds, and
    /// of what of its layer that state dropped.
    #[test]
    fn recovering_lets_go_of_what_the_state_no_longer_holds() {
        let base = layer(&[0, 1]);
        let own = |number| segment(2, number);
        let mut ledger =
            Ledger::opened(vec![live(2, Some(over(&base, 1)), 3, &[own(0), own(1)], &[])]);
        ledger.claim(2, [own(2)]);
        let holdings = Holdings {
            own: BTreeSet::from([own(0)]),
            dropped: BTreeSet::from([segment(PARENT, 0)]),
        };
        ledger.recovered(2, 3, &holdings);
        assert_eq!(sorted(ledger.removable()), [segment(PARENT, 0), own(1), own(2)]);
        assert!(ledger.held(own(0)) && ledger.held(segment(PARENT, 1)));
    }

    /// A branch recovered as of its last durable commit, after a flush failed, holds what that
    /// commit's state holds, though the commits not durable let it go: none of it goes as the
    /// branch commits again, and what it lets go of then goes once that is durable (TASK-79.5).
    #[test]
    fn recovering_takes_back_what_commits_not_durable_let_go() {
        let file = segment(2, 0);
        let mut ledger = Ledger::opened(vec![live(2, None, 1, &[file], &[])]);
        ledger.release(2, 2, vec![file]);
        let holdings = Holdings { own: BTreeSet::from([file]), dropped: BTreeSet::new() };
        ledger.recovered(2, 1, &holdings);
        ledger.durable(2, 2);
        assert_eq!(ledger.removable(), []);
        assert!(ledger.held(file));
        ledger.release(2, 3, vec![file]);
        ledger.durable(2, 3);
        assert_eq!(ledger.removable(), [file]);
    }

    /// So of what a fork lets go of the layer it is over: its state recovered holds it, what the
    /// other forks over the layer let go of since notwithstanding; and what those had let go of
    /// before stays let go of (TASK-79.5).
    #[test]
    fn recovering_takes_back_what_commits_not_durable_let_go_of_the_layer() {
        let base = layer(&[0, 1]);
        let (zero, one) = (segment(PARENT, 0), segment(PARENT, 1));
        let forks = || {
            Ledger::opened(vec![
                live(2, Some(over(&base, 1)), 1, &[], &[]),
                live(3, Some(over(&base, 1)), 1, &[], &[one]),
            ])
        };
        let nothing = Holdings { own: BTreeSet::new(), dropped: BTreeSet::new() };
        let mut ledger = forks();
        ledger.release(2, 2, vec![zero]);
        ledger.recovered(2, 1, &nothing);
        ledger.release(3, 2, vec![zero]);
        ledger.durable(3, 2);
        ledger.durable(2, 2);
        assert_eq!(ledger.removable(), []);
        assert!(ledger.held(zero));

        // Doomed by the commit not durable, which every other fork had let go of.
        let mut ledger = forks();
        ledger.release(2, 2, vec![one]);
        ledger.recovered(2, 1, &nothing);
        ledger.durable(2, 2);
        assert_eq!(ledger.removable(), []);
        assert!(ledger.held(one) && !ledger.shared(2, one));
        ledger.release(2, 3, vec![one]);
        ledger.durable(2, 3);
        assert_eq!(ledger.removable(), [one]);
    }

    /// Another branch holds a file of its own, or through the layer it is over, but for those it
    /// let go of.
    #[test]
    fn files_are_shared_through_claims_and_layers() {
        let base = layer(&[0, 1]);
        let (zero, one, two) = (segment(PARENT, 0), segment(PARENT, 1), segment(PARENT, 2));
        let mut ledger = Ledger::opened(vec![live(PARENT, None, 1, &[zero, one, two], &[])]);
        ledger.attach(2, over(&base, 1));
        ledger.release(2, 1, vec![one]);
        assert!(ledger.shared(PARENT, zero));
        assert!(!ledger.shared(PARENT, one));
        assert!(!ledger.shared(PARENT, two));
        assert!(ledger.shared(2, zero) && ledger.shared(2, two));

        ledger.attach(3, over(&layer(&[]), 2));
        ledger.claim(3, [two]);
        assert!(ledger.shared(PARENT, two));
    }

    /// A branch recovered, or deleted, lets go of what it claimed alone: another's claims on the
    /// same files stay, and on others.
    #[test]
    fn branches_let_go_of_their_own_claims_alone() {
        let (shared, mine, theirs) = (segment(2, 0), segment(2, 1), segment(3, 0));
        let mut ledger = Ledger::opened(vec![
            live(2, None, 1, &[shared, mine], &[]),
            live(3, None, 1, &[shared, theirs], &[]),
        ]);
        let nothing = Holdings { own: BTreeSet::new(), dropped: BTreeSet::new() };
        ledger.recovered(2, 1, &nothing);
        assert_eq!(ledger.removable(), [mine]);
        assert!(!ledger.shared(3, shared));

        let next = segment(2, 2);
        ledger.claim(2, [shared, next]);
        ledger.detach(2);
        assert_eq!(ledger.removable(), [next]);
        assert!(ledger.held(shared) && ledger.held(theirs));
        assert!(!ledger.shared(3, shared) && !ledger.shared(3, theirs));
    }

    /// fxdax: a branch's lettings go of files another made are counted, which tell those files'
    /// makers they may hold them alone: a fork's of its layer's, and a deletion's; not a branch's
    /// of its own (TASK-79.2).
    #[test]
    fn lettings_go_of_others_files_are_counted() {
        let base = layer(&[0, 1]);
        let (zero, one, own) = (segment(PARENT, 0), segment(PARENT, 1), segment(2, 0));
        let mut ledger = Ledger::opened(vec![
            live(PARENT, None, 1, &[zero, one], &[]),
            live(2, Some(over(&base, 1)), 1, &[own], &[]),
        ]);
        ledger.release(2, 2, vec![own]);
        ledger.release(PARENT, 2, vec![zero]);
        assert_eq!(ledger.unshared(), 0);
        assert!(ledger.shared(PARENT, one));
        ledger.release(2, 3, vec![one]);
        assert_eq!(ledger.unshared(), 1);
        assert!(!ledger.shared(PARENT, one));
        ledger.detach(2);
        assert_eq!(ledger.unshared(), 2);
    }

    /// A branch's latest state its live forks are over, whichever forks were deleted.
    #[test]
    fn the_latest_layer_is_the_latest_live_forks() {
        let (early, late) = (layer(&[0]), layer(&[0, 1]));
        let mut ledger = Ledger::opened(vec![live(PARENT, None, 1, &[], &[])]);
        ledger.attach(2, over(&early, 1));
        ledger.attach(3, over(&late, 2));
        let elsewhere = Origin { parent: 3, epoch: 9, next_sequence: 0, index: 1 };
        ledger.attach(4, Over { origin: elsewhere, layer: layer(&[]) });
        assert!(Arc::ptr_eq(&ledger.latest_layer_of(PARENT).unwrap(), &late));
        ledger.detach(3);
        assert!(Arc::ptr_eq(&ledger.latest_layer_of(PARENT).unwrap(), &early));
        ledger.detach(2);
        assert!(ledger.latest_layer_of(PARENT).is_none());
    }
}
