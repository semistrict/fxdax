// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A lineage's branches written, freed, cleaned, forked, closed, reopened and deleted at random,
//! seeded, and the lineage itself reopened from its directory: each open branch reads what it
//! wrote and inherited, whatever its parents and forks freed; and once every branch's commits are
//! durable and checkpointed, the directory holds the files of the live branches' extents, and no
//! others, before and after the lineage opens again (TASK-44.1). Which files a fork shares with
//! the branches it came from, and when one goes, is the lineage's bookkeeping, which this checks
//! from outside. On the host, too: a directory no branch has open is let go, for another process.

use extent_store::{Description, Dir, HostDir, Lineage, MemDir, Options, Place, Store};
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use storage_device::ExtentId;

const BLOCK: usize = 4096;
const FIRST: u64 = 7;
/// Small segments, large extents of their own past 32 KiB, and a checkpoint at each commit, so
/// that no closed segment waits long for one to cover it.
const OPTIONS: Options = Options {
    segment_size: 32 << 10,
    segment_age: std::time::Duration::from_secs(3600),
    large_extent: 32 << 10,
    checkpoint_after: 1,
    open_files: 2,
    view_lag: None,
    view_kept_bytes: None,
};

/// `len` bytes of `seed`'s pattern.
fn pattern(seed: u64, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i as u64 % 251) as u8 ^ seed as u8 ^ (seed >> 8) as u8).collect()
}

/// A branch: its store, if open, and what it holds: each extent's pattern seed and length.
struct Branch<D: Dir> {
    store: Option<Store<D>>,
    extents: BTreeMap<ExtentId, (u64, usize)>,
    next_sequence: u64,
}

struct World<D: Dir> {
    /// The directory, as the lineage opens it anew.
    dir: Box<dyn Fn() -> D>,
    lineage: Arc<Lineage<D>>,
    branches: BTreeMap<u64, Branch<D>>,
    next_branch: u64,
    rng: StdRng,
    seed: u64,
}

impl<D: Dir> World<D> {
    fn new(seed: u64, dir: Box<dyn Fn() -> D>) -> Self {
        let description =
            Description { block_size: BLOCK as u32, capacity: 64 << 20, branch: FIRST };
        let lineage = Lineage::create(dir(), description, OPTIONS).unwrap();
        let store = lineage.open_branch(FIRST).unwrap();
        let first = Branch { store: Some(store), extents: BTreeMap::new(), next_sequence: 1 };
        Self {
            dir,
            lineage,
            branches: BTreeMap::from([(FIRST, first)]),
            next_branch: FIRST + 1,
            rng: StdRng::seed_from_u64(seed),
            seed,
        }
    }

    fn pick(&mut self, open: bool) -> Option<u64> {
        let candidates: Vec<u64> = (self.branches.iter())
            .filter(|(_, branch)| branch.store.is_some() == open)
            .map(|(id, _)| *id)
            .collect();
        (!candidates.is_empty()).then(|| candidates[self.rng.random_range(0..candidates.len())])
    }

    fn store(&self, id: u64) -> &Store<D> {
        self.branches[&id].store.as_ref().unwrap()
    }

    fn step(&mut self, step: usize) {
        match self.rng.random_range(0..100) {
            0..28 => {
                let Some(id) = self.pick(true) else { return };
                let blocks =
                    if self.rng.random_range(0..4) == 0 { 9 } else { self.rng.random_range(1..4) };
                let seed = self.rng.random::<u64>();
                let branch = self.branches.get_mut(&id).unwrap();
                let extent = ExtentId::new(id, branch.next_sequence);
                branch.next_sequence += 1;
                let data = pattern(seed, blocks * BLOCK);
                branch.store.as_ref().unwrap().write(extent, 0, &data).unwrap();
                branch.extents.insert(extent, (seed, data.len()));
            }
            28..46 => {
                let Some(id) = self.pick(true) else { return };
                let extents: Vec<ExtentId> = self.branches[&id].extents.keys().copied().collect();
                if extents.is_empty() {
                    return;
                }
                let extent = extents[self.rng.random_range(0..extents.len())];
                self.store(id).free(extent).unwrap();
                self.branches.get_mut(&id).unwrap().extents.remove(&extent);
            }
            46..58 => {
                let Some(id) = self.pick(true) else { return };
                self.store(id).commit(None).unwrap();
            }
            58..62 => {
                // Cleaning takes none of the segments other branches share.
                let Some(id) = self.pick(true) else { return };
                self.store(id).clean(0.9).unwrap();
            }
            62..72 => {
                let Some(id) = self.pick(true) else { return };
                let child = self.next_branch;
                self.next_branch += 1;
                self.store(id).fork(child).unwrap();
                let extents = self.branches[&id].extents.clone();
                self.branches.insert(child, Branch { store: None, extents, next_sequence: 1 });
            }
            72..82 => {
                let Some(id) = self.pick(false) else { return };
                let store = self.lineage.open_branch(id).unwrap();
                self.branches.get_mut(&id).unwrap().store = Some(store);
            }
            82..92 => {
                let Some(id) = self.pick(true) else { return };
                self.close(id);
            }
            92..98 => {
                let Some(id) = self.pick(false) else { return };
                self.lineage.delete(id).unwrap();
                self.branches.remove(&id);
            }
            _ => self.reopen_lineage(),
        }
        self.check_reads(step);
    }

    /// Commits what `id` wrote since its last commit, which a store closed forgets, and closes it.
    fn close(&mut self, id: u64) {
        let store = self.branches.get_mut(&id).unwrap().store.take().unwrap();
        store.commit(None).unwrap();
        drop(store);
    }

    /// Closes every branch, and opens the lineage anew from its directory.
    fn reopen_lineage(&mut self) {
        let ids: Vec<u64> = self.branches.keys().copied().collect();
        for id in ids {
            if self.branches[&id].store.is_some() {
                self.close(id);
            }
        }
        self.lineage = Lineage::open((self.dir)(), OPTIONS).unwrap();
    }

    /// Each open branch reads what it holds.
    fn check_reads(&self, step: usize) {
        for (id, branch) in &self.branches {
            let Some(store) = &branch.store else { continue };
            for (extent, (seed, len)) in &branch.extents {
                let mut data = vec![0; *len];
                if let Err(error) = store.read(*extent, 0, &mut data) {
                    let seed = self.seed;
                    panic!("seed {seed}, step {step}: branch {id:#x} reads {extent:?}: {error:#}");
                }
                assert!(
                    data == pattern(*seed, *len),
                    "seed {}, step {step}: branch {id:#x} reads other data in {extent:?}",
                    self.seed
                );
            }
        }
    }

    /// Opens every branch, closes its open segment, commits each durably thrice (a checkpoint
    /// each), and checks what it reads, and that the directory holds the files their extents are
    /// in, and no others.
    fn settle(&mut self) {
        let ids: Vec<u64> = self.branches.keys().copied().collect();
        for id in &ids {
            if self.branches[id].store.is_none() {
                let store = self.lineage.open_branch(*id).unwrap();
                self.branches.get_mut(id).unwrap().store = Some(store);
            }
        }
        // A segment a commit closes, dead or not, stays until the next commit's checkpoint covers
        // its footer.
        for close in [true, false, false] {
            for id in &ids {
                if close {
                    self.store(*id).close_segment().unwrap();
                }
                self.store(*id).commit(None).unwrap();
                self.store(*id).flush().unwrap();
            }
        }
        self.check_reads(usize::MAX);
        let mut needed = BTreeSet::new();
        for (id, branch) in &self.branches {
            for (extent, _, place) in branch.store.as_ref().unwrap().places().unwrap() {
                match place {
                    Place::Run { branch, number, .. } => {
                        needed.insert(format!("segments/{branch:016x}/{number:016x}"));
                    }
                    Place::Own => {
                        let (branch, sequence) = (extent.branch, extent.sequence);
                        needed.insert(format!("extents/{branch:016x}/{sequence:016x}"));
                    }
                    place => panic!("seed {}: {id:#x}'s {extent:?} settled {place:?}", self.seed),
                }
            }
        }
        let dir = self.lineage.dir();
        let mut held = BTreeSet::new();
        for kind in ["segments", "extents"] {
            for branch in dir.list(kind).unwrap_or_default() {
                for name in dir.list(&format!("{kind}/{branch}")).unwrap_or_default() {
                    held.insert(format!("{kind}/{branch}/{name}"));
                }
            }
        }
        let extra: Vec<_> = held.difference(&needed).collect();
        let missing: Vec<_> = needed.difference(&held).collect();
        assert!(
            extra.is_empty() && missing.is_empty(),
            "seed {}: files left {extra:?}, missing {missing:?}",
            self.seed
        );
    }

    /// Runs `steps` steps, settles, and does so again once the lineage is reopened.
    fn run(&mut self, steps: usize) {
        for step in 0..steps {
            self.step(step);
        }
        self.settle();
        self.reopen_lineage();
        self.settle();
    }
}

/// Many seeds of a few hundred steps, in memory: `LINEAGE_SOAK` multiplies them.
#[test]
fn branches_hold_their_files_and_no_others() {
    let soak: u64 = std::env::var("LINEAGE_SOAK").map_or(1, |soak| soak.parse().unwrap());
    for seed in 0..40 * soak {
        let dir = MemDir::new();
        World::new(seed, Box::new(move || dir.clone())).run(300);
    }
}

/// Some seeds on the host; and a directory no branch has open is let go: another opening takes it
/// at once.
#[test]
fn branches_on_the_host_hold_their_files_and_let_their_directory_go() {
    for seed in 0..4 {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_owned();
        let mut world = World::new(seed, Box::new(move || HostDir::new(&path, false).unwrap()));
        world.run(150);
        let ids: Vec<u64> = world.branches.keys().copied().collect();
        for id in ids {
            world.close(id);
        }
        let other = HostDir::new(temp.path(), false).unwrap();
        let taking = std::time::Instant::now();
        other.take().unwrap();
        assert!(taking.elapsed() < std::time::Duration::from_secs(1), "the directory was held");
        other.let_go();
    }
}
