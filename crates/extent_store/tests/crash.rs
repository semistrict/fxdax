// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A store crashes at random points of random workloads, and recovers to a prefix of its commits:
//! exactly the extents and root of an epoch at least as late as the last commit made durable, by
//! a durable commit or a flush of commits only ordered.

use extent_store::{Description, Lineage, MemDir, Options, Store};
use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use std::collections::BTreeMap;
use storage_device::{ExtentId, SealedExtent};

const BLOCK: usize = 4096;
const BRANCH: u64 = 3;
const ROUNDS: u64 = 300;
const STEPS: usize = 200;

/// Small segments, large extents and index files, so that workloads make many of each.
const OPTIONS: Options = Options {
    segment_size: 48 << 10,
    segment_age: std::time::Duration::from_secs(3600),
    large_extent: 24 << 10,
    checkpoint_after: 2 << 10,
    open_files: 3,
    view_lag: None,
    view_kept_bytes: None,
};

/// What a store holds: its extents' contents, and its root.
#[derive(Clone, Default, PartialEq, Eq)]
struct Model {
    extents: BTreeMap<ExtentId, Vec<u8>>,
    root: Vec<u8>,
}

impl std::fmt::Debug for Model {
    /// Each extent's sequence number, length and hash, and the root.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let extents: Vec<_> = self
            .extents
            .iter()
            .map(|(extent, data)| {
                (extent.sequence, data.len() / BLOCK, rapidhash::rapidhash(data) as u16)
            })
            .collect();
        write!(f, "{extents:?} root {:?}", String::from_utf8_lossy(&self.root))
    }
}

/// One to `most` blocks of data.
fn random_data(rng: &mut StdRng, most: usize) -> Vec<u8> {
    let blocks = rng.random_range(1..=most);
    let byte: u8 = rng.random();
    (0..blocks * BLOCK).map(|i| byte ^ (i / BLOCK) as u8).collect()
}

/// Runs a random workload on `store`, which holds `model`, committed or not, and whose commits up
/// to `durable` are durable, until it ends or the store fails, recording in `epochs` what each
/// commit began with, and returns the epoch of the last commit made durable, and what the store
/// holds then, committed or not. The commits after `durable` may be ordered only, as a workload
/// before this one may have ended on such commits.
fn workload(
    rng: &mut StdRng,
    store: &Store<MemDir>,
    epochs: &mut Vec<Model>,
    mut model: Model,
    mut durable: usize,
) -> (usize, Model) {
    let branch = store.description().branch;
    let mut open: Vec<ExtentId> = Vec::new();
    let mut next_sequence = store.info().unwrap().next_sequence;
    for _ in 0..STEPS {
        let result = match rng.random_range(0..13) {
            // A new extent, small or large.
            0..=3 => {
                let extent = ExtentId::new(branch, next_sequence);
                next_sequence += 1;
                let data = random_data(rng, 8);
                let result = store.write(extent, 0, &data);
                if result.is_ok() {
                    model.extents.insert(extent, data);
                    open.push(extent);
                }
                result
            }
            // More of an open extent, over what it holds or past it.
            4..=5 if !open.is_empty() => {
                let extent = open[rng.random_range(0..open.len())];
                let held = model.extents[&extent].len() / BLOCK;
                let offset = rng.random_range(0..=held);
                let data = random_data(rng, 2);
                let result = store.write(extent, (offset * BLOCK) as u64, &data);
                if result.is_ok() {
                    let contents = model.extents.get_mut(&extent).unwrap();
                    if contents.len() < offset * BLOCK + data.len() {
                        contents.resize(offset * BLOCK + data.len(), 0);
                    }
                    contents[offset * BLOCK..offset * BLOCK + data.len()].copy_from_slice(&data);
                }
                result
            }
            6..=7 if !model.extents.is_empty() => {
                let extents: Vec<_> = model.extents.keys().copied().collect();
                let extent = extents[rng.random_range(0..extents.len())];
                let result = store.free(extent);
                if result.is_ok() {
                    model.extents.remove(&extent);
                    open.retain(|open| *open != extent);
                }
                result
            }
            // Cleaning, which commits durably, or a commit, durable or only ordered, setting the
            // root or not.
            8..=11 => {
                let clean = rng.random_bool(0.2);
                let ordered = !clean && rng.random_bool(0.5);
                let root =
                    (!clean && rng.random_bool(0.5)).then(|| format!("root {}", epochs.len()));
                if let Some(root) = &root {
                    model.root = root.clone().into_bytes();
                }
                epochs.push(model.clone());
                open.clear();
                let root = root.as_deref().map(str::as_bytes);
                let result = if clean {
                    store.clean(rng.random_range(0.1..0.9)).map(|_| ())
                } else if ordered {
                    store.commit_ordered(root)
                } else {
                    store.commit(root)
                };
                if result.is_ok() && !ordered {
                    durable = epochs.len() - 1;
                }
                result
            }
            // The commits only ordered so far made durable.
            12 => {
                let result = store.flush();
                if result.is_ok() {
                    durable = epochs.len() - 1;
                }
                result
            }
            _ => Ok(()),
        };
        if result.is_err() {
            break;
        }
    }
    (durable, model)
}

/// What `store` holds.
fn contents(store: &Store<MemDir>) -> Model {
    let mut extents = BTreeMap::new();
    for (extent, len) in store.extents().unwrap() {
        let mut data = vec![0; len as usize];
        store.read(extent, 0, &mut data).unwrap();
        extents.insert(extent, data);
    }
    Model { extents, root: store.root().unwrap() }
}

#[test]
fn recovers_a_prefix_of_its_commits() {
    let mut crashes_mid_commit = 0;
    for round in 0..ROUNDS {
        let mut rng = StdRng::seed_from_u64(round);
        let dir = MemDir::new();
        let description =
            Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };
        let store = Store::create(dir.clone(), description, OPTIONS).unwrap();
        let mut epochs = vec![Model::default()];
        // The file system fails from a random point on, as the host crashes.
        dir.fail_after(Some(rng.random_range(0..600)));
        let (durable, _) = workload(&mut rng, &store, &mut epochs, Model::default(), 0);
        drop(store);
        if durable + 1 < epochs.len() {
            crashes_mid_commit += 1;
        }

        let crashed = dir.crash(round);
        let store = Store::open(crashed.clone(), OPTIONS)
            .unwrap_or_else(|e| panic!("round {round}: the store doesn't open: {e:?}"));
        let epoch = store.epoch().unwrap() as usize;
        assert!(
            (durable..epochs.len()).contains(&epoch),
            "round {round}: recovered epoch {epoch}, of {durable}..{}",
            epochs.len()
        );
        assert_eq!(contents(&store), epochs[epoch], "round {round}: epoch {epoch} differs");
        // What it recovered is sealed, and minting resumes past it.
        let info = store.info().unwrap();
        for (extent, _) in store.extents().unwrap() {
            assert!(extent.sequence < info.next_sequence, "round {round}");
            let error = store.write(extent, 0, &[0; BLOCK]).expect_err("rewrote a sealed extent");
            assert_eq!(error.downcast_ref::<SealedExtent>(), Some(&SealedExtent(extent)));
        }

        // Its closed segments' footers are its history up to their last commit, none past it.
        let rebuilt = store.rebuild().unwrap();
        assert!(rebuilt.epoch <= epoch as u64, "round {round}: footers past epoch {epoch}");

        // It carries on, and crashing again after a flush keeps what it committed.
        let recovered = epochs[epoch].clone();
        let mut epochs = vec![recovered.clone()];
        workload(&mut rng, &store, &mut epochs, recovered, 0);
        store.flush().unwrap_or_else(|e| panic!("round {round}: a commit failed: {e:?}"));
        let durable = epochs.len() - 1;
        drop(store);
        let store = Store::open(crashed.crash(round + ROUNDS), OPTIONS).unwrap();
        assert_eq!(contents(&store), epochs[durable], "round {round}: carrying on");
        // And closing its open segment, its footers say what it holds.
        store.close_segment().unwrap();
        store.commit(None).unwrap();
        let rebuilt = store.rebuild().unwrap();
        assert_eq!(rebuilt.epoch, store.epoch().unwrap(), "round {round}");
        assert_eq!(rebuilt.extents, store.places().unwrap(), "round {round}");
        assert_eq!(rebuilt.root, store.root().unwrap(), "round {round}");
    }
    // The crashes hit commits as well as what came between them.
    assert!(crashes_mid_commit > ROUNDS / 10, "{crashes_mid_commit} crashes in commits");
}

/// The epoch `store` recovered, which must be one of `epochs` from `durable` on, and what it holds
/// must be what that epoch began with.
fn check(round: u64, name: &str, store: &Store<MemDir>, epochs: &[Model], durable: usize) {
    let epoch = store.epoch().unwrap() as usize;
    assert!(
        (durable..epochs.len()).contains(&epoch),
        "round {round}: {name} recovered epoch {epoch}, of {durable}..{}",
        epochs.len()
    );
    assert_eq!(contents(store), epochs[epoch], "round {round}: {name}'s epoch {epoch} differs");
}

/// A parent and its fork, which inherited its extents, run workloads by turns and crash, and each
/// recovers a prefix of its own commits, whatever the other freed; or the fork is opened only
/// after the crash, and holds what its parent held at the fork, whatever its parent freed and
/// cleaned since.
#[test]
fn forks_recover_prefixes_of_their_commits() {
    const CHILD: u64 = BRANCH + 1;
    for round in 0..ROUNDS / 3 {
        let mut rng = StdRng::seed_from_u64(round);
        let dir = MemDir::new();
        let description =
            Description { block_size: BLOCK as u32, capacity: 4 << 20, branch: BRANCH };
        let parent = Store::create(dir.clone(), description, OPTIONS).unwrap();
        let mut parent_epochs = vec![Model::default()];
        let (_, held) = workload(&mut rng, &parent, &mut parent_epochs, Model::default(), 0);
        // The fork commits, durably, what the parent held, which the child starts from.
        parent_epochs.push(held.clone());
        parent.fork(CHILD).unwrap();
        let lineage = parent.lineage().clone();
        let opened = rng.random_bool(0.5);
        let child = opened.then(|| lineage.open_branch(CHILD).unwrap());
        let mut child_epochs = vec![held.clone()];
        let (mut parent_durable, mut child_durable) = (parent_epochs.len() - 1, 0);
        // What each holds, committed or not, from turn to turn.
        let (mut parent_held, mut child_held) = (held.clone(), held);

        dir.fail_after(Some(rng.random_range(0..1200)));
        for turn in 0..6 {
            let (store, epochs, durable, held) = match (turn % 2, &child) {
                (0, _) => (&parent, &mut parent_epochs, &mut parent_durable, &mut parent_held),
                (_, Some(child)) => (child, &mut child_epochs, &mut child_durable, &mut child_held),
                (_, None) => continue,
            };
            let (last_durable, now) = workload(&mut rng, store, epochs, held.clone(), *durable);
            *durable = last_durable;
            *held = now;
            if dir.has_failed() {
                break;
            }
        }
        drop((parent, child, lineage));

        let crashed = dir.crash(round);
        let lineage = Lineage::open(crashed, OPTIONS)
            .unwrap_or_else(|e| panic!("round {round}: the store doesn't open: {e:?}"));
        let parent = lineage.open_branch(BRANCH).unwrap();
        check(round, "the parent", &parent, &parent_epochs, parent_durable);
        let child = lineage.open_branch(CHILD).unwrap();
        check(round, "the child", &child, &child_epochs, child_durable);
    }
}
