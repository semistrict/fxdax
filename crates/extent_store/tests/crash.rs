//! A store crashes at random points of random workloads, and recovers to a prefix of its commits:
//! exactly the extents and root of an epoch at least as late as the last commit made durable, by
//! a durable commit or a flush of commits only ordered.

use extent_store::{Description, MemDir, Options, Store};
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
    large_extent: 24 << 10,
    checkpoint_after: 2 << 10,
    open_files: 3,
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

/// Runs a random workload on `store` until it ends or the store fails, recording in `epochs` what
/// each commit began with, and returns the epoch of the last commit made durable.
fn workload(rng: &mut StdRng, store: &Store<MemDir>, epochs: &mut Vec<Model>) -> usize {
    let mut model = epochs.last().unwrap().clone();
    let mut open: Vec<ExtentId> = Vec::new();
    let mut next_sequence = store.info().unwrap().next_sequence;
    let mut durable = epochs.len() - 1;
    for _ in 0..STEPS {
        let result = match rng.random_range(0..13) {
            // A new extent, small or large.
            0..=3 => {
                let extent = ExtentId::new(BRANCH, next_sequence);
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
    durable
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
        let durable = workload(&mut rng, &store, &mut epochs);
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

        // It carries on, and crashing again after a flush keeps what it committed.
        let mut epochs = vec![epochs[epoch].clone()];
        workload(&mut rng, &store, &mut epochs);
        store.flush().unwrap_or_else(|e| panic!("round {round}: a commit failed: {e:?}"));
        let durable = epochs.len() - 1;
        drop(store);
        let store = Store::open(crashed.crash(round + ROUNDS), OPTIONS).unwrap();
        assert_eq!(contents(&store), epochs[durable], "round {round}: carrying on");
    }
    // The crashes hit commits as well as what came between them.
    assert!(crashes_mid_commit > ROUNDS / 10, "{crashes_mid_commit} crashes in commits");
}
