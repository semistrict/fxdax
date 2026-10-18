// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The simulation (backlog doc-8, testing 3): the writer makes random operations on an extent
//! store's filesystem, syncing and compacting now and then; a follower, Fxfs mounted read-only on
//! a view of the store, follows it (`FxFilesystem::follow`, TASK-42.4); readers read files through
//! the follower, and holders hold files open through it, reading them as the writer goes on. Every
//! interleaving of their tasks is Shuttle's to choose. Then:
//!
//! - the history checker (`history`) requires what each reader found to be what the writer's
//!   files were after some operation the follower's commit may hold, each reader's to move
//!   forward, and what a holder reads of a file the writer deleted to be what it last held;
//! - after each batch the follower applies, its tree is compared with a mount of its commit of the
//!   checker's own (testing 5): its root directory, file by file, and its root stores' records;
//! - each batch's changes cover what changed in the follower's tree;
//! - once the holders are done, the follower's next batch lets go of all it kept for them: its
//!   records are its commit's, and its view pins nothing;
//! - a mount of the last commit holds what the writer's last operation left.
//!
//! Readers are kept out while the follower applies a batch (TASK-42.5 lifts this).

use crate::device::ViewDevice;
use crate::history::{HeldRead, History, Read};
use crate::model::{Model, NAMES, Op, Writer, lookup, observe, read};
use extent_store::{Description, MemDir, Options as StoreOptions, Store, View};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
use fxfs::follower::{Changes, StoreObject};
use fxfs::lsm_tree::Query;
use fxfs::lsm_tree::types::{ItemRef, LayerIterator};
use fxfs::object_store::directory::Directory;
use fxfs::object_store::transaction::{LockKey, LockManager, lock_keys};
use fxfs::object_store::{ExtentValue, HandleOptions, ObjectDescriptor, ObjectStore, ObjectValue};
use shuttle::rand::{Rng, thread_rng};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use storage_device::DeviceHolder;

/// What the simulation runs: the writer's operations, the readers, and the holders.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub operations: usize,
    pub readers: usize,
    pub holders: usize,
    pub mutation: Mutation,
}

impl Default for Profile {
    fn default() -> Self {
        Self { operations: 16, readers: 2, holders: 1, mutation: Mutation::None }
    }
}

/// A follower's bug, which the simulation must catch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    None,
    /// Its view reads extents of commits after its own: it replays on past its commit.
    ReadsPastItsView,
    /// It shows one file as its first commit held it, and the others as they are: it skipped the
    /// changes to that one.
    SkipsChangesTo(&'static str),
    /// It shows two files as its previous batch did, and the others as its current one does: it
    /// applied part of a commit, as a transaction renaming between them would show.
    SplitsCommits,
    /// It breaks one of Fxfs's follower rules.
    Breaks(fxfs::follower::Rules),
}

/// The lock that keeps readers out while the follower applies a batch: one key, read by readers.
fn gate() -> fxfs::object_store::transaction::LockKeys {
    lock_keys![LockKey::object(0, 0)]
}

/// Mounts Fxfs read-only on `view`: the checker's own mount of a commit.
async fn mount(store: &Arc<Store<MemDir>>, view: View) -> Arc<OpenFxFilesystem> {
    let device = DeviceHolder::new(ViewDevice::new(store.clone(), view));
    let fs = FxFilesystemBuilder::new().read_only(true).open(device).await.expect("mount failed");
    Arc::new(fs)
}

/// The follower: Fxfs mounted read-only on a view, which it advances.
struct Follower {
    fs: Arc<OpenFxFilesystem>,
    view: Arc<Mutex<View>>,
    gate: LockManager,
    state: Mutex<Following>,
}

/// Where the follower is, and the checker's mounts of commits it followed.
struct Following {
    /// The commit its last batch brought it to.
    epoch: u64,
    /// The checker's mounts of the first commit, of its last batch's, and of the one before.
    first: Arc<OpenFxFilesystem>,
    last: Arc<OpenFxFilesystem>,
    previous: Arc<OpenFxFilesystem>,
    /// Every mount of the checker's, closed once all is done.
    oracles: Vec<Arc<OpenFxFilesystem>>,
    /// The objects holders held, whose records the follower may keep while they do.
    held: BTreeSet<u64>,
    /// The batches that changed something.
    batches: usize,
}

/// A node of a root directory: its object, and what of it a batch's changes cover.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Node {
    object_id: u64,
    attributes: String,
    data: String,
}

/// The root directory of `fs`'s root store, entry by entry: each one's object, descriptor and
/// properties, and, of a file, its extents and contents.
async fn tree(fs: &OpenFxFilesystem) -> BTreeMap<String, Node> {
    let store = fs.root_store();
    let root = Directory::open(&store, store.root_directory_object_id())
        .await
        .expect("opening the root directory failed");
    let layer_set = store.tree().layer_set();
    let mut merger = layer_set.merger();
    let mut entries = Vec::new();
    let mut iter = root.iter(&mut merger).await.expect("iterating the root failed");
    while let Some((name, object_id, descriptor)) = iter.get() {
        entries.push((name.to_owned(), object_id, descriptor.clone()));
        iter.advance().await.expect("iterating the root failed");
    }
    let mut tree = BTreeMap::new();
    for (name, object_id, descriptor) in entries {
        let node = match descriptor {
            ObjectDescriptor::File => {
                let options = HandleOptions::default();
                let handle = ObjectStore::open_object(&store, object_id, options, None)
                    .await
                    .expect("opening a file failed");
                let properties = handle.get_properties().await.expect("properties failed");
                let extents = handle.device_extents().await.expect("extents failed");
                let contents = observe(fs, &name).await;
                Node {
                    object_id,
                    attributes: format!("file {properties:?}"),
                    data: format!("{extents:?} {contents:?}"),
                }
            }
            ObjectDescriptor::Directory => {
                let directory =
                    Directory::open(&store, object_id).await.expect("opening a directory failed");
                let properties = directory.get_properties().await.expect("properties failed");
                Node { object_id, attributes: format!("directory {properties:?}"), data: "".into() }
            }
            other => Node { object_id, attributes: format!("{other:?}"), data: "".into() },
        };
        tree.insert(name, node);
    }
    tree
}

/// The records of `store`'s tree, but those deleted and those of the objects `excluded`.
async fn records(store: &ObjectStore, excluded: &BTreeSet<u64>) -> Vec<String> {
    let layer_set = store.tree().layer_set();
    let mut merger = layer_set.merger();
    let mut iter = merger.query(Query::FullScan).await.expect("scanning a store failed");
    let mut records = Vec::new();
    while let Some(ItemRef { key, value }) = iter.get() {
        let deleted = matches!(value, ObjectValue::None | ObjectValue::Extent(ExtentValue::None));
        if !deleted && !excluded.contains(&key.object_id) {
            records.push(format!("{key:?} {value:?}"));
        }
        iter.advance().await.expect("scanning a store failed");
    }
    records
}

/// Compares `fs`, the follower, with `oracle`, the checker's mount of the same commit (doc-8,
/// testing 5): their root directories, entry by entry, and their root stores' records, but those
/// of the objects `excluded`. Returns the follower's tree.
async fn compare(
    fs: &OpenFxFilesystem,
    oracle: &OpenFxFilesystem,
    epoch: u64,
    excluded: &BTreeSet<u64>,
) -> BTreeMap<String, Node> {
    let (found, expected) = (tree(fs).await, tree(oracle).await);
    assert_eq!(found, expected, "the follower's tree at commit {epoch} isn't its commit's");
    // Its stores' trees are of the layers its commit's are: a flush's swapped in.
    let layers = |fs: &OpenFxFilesystem| {
        [fs.root_parent_store(), fs.root_store()].map(|store| {
            let layers = store.tree().immutable_layer_set().layers;
            layers.iter().map(|layer| layer.handle().map(|handle| handle.object_id())).collect()
        })
    };
    let (found_layers, expected_layers): ([Vec<Option<u64>>; 2], _) = (layers(fs), layers(oracle));
    assert_eq!(
        found_layers, expected_layers,
        "the follower's layers at commit {epoch} aren't its commit's"
    );
    for (found, expected) in [
        (fs.root_parent_store(), oracle.root_parent_store()),
        (fs.root_store(), oracle.root_store()),
    ] {
        let store = found.store_object_id();
        let (found, expected) =
            (records(&found, excluded).await, records(&expected, excluded).await);
        assert!(
            found == expected,
            "the follower's records of store {store} at commit {epoch} aren't its commit's: \
             {found:#?} where its commit's are {expected:#?}"
        );
    }
    found
}

/// Checks that `changes` cover what changed between the follower's trees `before` and `after` a
/// batch, in the root directory `root` of the store `store`.
fn check_changes(
    store: u64,
    root: u64,
    before: &BTreeMap<String, Node>,
    after: &BTreeMap<String, Node>,
    changes: &Changes,
    epoch: u64,
) {
    let entries = |tree: &BTreeMap<String, Node>| -> BTreeMap<String, u64> {
        tree.iter().map(|(name, node)| (name.clone(), node.object_id)).collect()
    };
    let objects = |tree: &BTreeMap<String, Node>| -> BTreeMap<u64, Node> {
        tree.values().map(|node| (node.object_id, node.clone())).collect()
    };
    let object = |object_id| StoreObject { store_object_id: store, object_id };
    let miss = |what: String| -> ! {
        panic!("a batch's changes miss {what}, to commit {epoch}: {changes:?}")
    };
    if entries(before) != entries(after) && !changes.entries.contains(&object(root)) {
        miss(format!("the root directory's entries, {before:?} to {after:?}"));
    }
    let (before, after) = (objects(before), objects(after));
    for (object_id, node) in &after {
        let changed = |was: &Node| was.attributes != node.attributes;
        if before.get(object_id).is_none_or(changed)
            && !changes.attributes.contains(&object(*object_id))
        {
            miss(format!(
                "object {object_id}'s attributes, {:?} to {node:?}",
                before.get(object_id)
            ));
        }
        let Some(was) = before.get(object_id) else { continue };
        if was.data != node.data
            && !changes.data.get(&object(*object_id)).is_some_and(|ranges| {
                ranges.iter().any(|range| range.start == 0 && range.end >= crate::model::BLOCK)
            })
        {
            miss(format!("object {object_id}'s data, {was:?} to {node:?}"));
        }
    }
    for object_id in before.keys().filter(|object_id| !after.contains_key(object_id)) {
        if !changes.deleted.contains(&object(*object_id)) {
            miss(format!("object {object_id}'s deletion"));
        }
    }
}

/// The follower's next batch: it follows, and is compared with its new commit, its changes with
/// what changed: but for the records of objects holders held, if `excluding_held`. Readers are
/// kept out meanwhile.
async fn follow(follower: &Follower, store: &Arc<Store<MemDir>>, excluding_held: bool) {
    let _gate = follower.gate.write_lock(gate()).await;
    let excluded =
        if excluding_held { follower.state.lock().held.clone() } else { BTreeSet::new() };
    let fs = &follower.fs;
    let before = tree(fs).await;
    let changes = fs.follow().await.expect("following failed");
    let (epoch, view) = {
        let view = follower.view.lock();
        (view.epoch(), view.duplicate().expect("duplicating the follower's view failed"))
    };
    let oracle = mount(store, view).await;
    let after = compare(fs, &oracle, epoch, &excluded).await;
    let root_store = fs.root_store();
    let (store_object_id, root) =
        (root_store.store_object_id(), root_store.root_directory_object_id());
    check_changes(store_object_id, root, &before, &after, &changes, epoch);
    let mut state = follower.state.lock();
    state.epoch = epoch;
    state.previous = std::mem::replace(&mut state.last, oracle.clone());
    state.oracles.push(oracle);
    if changes != Changes::default() {
        state.batches += 1;
    }
}

/// What the simulation's runs reached, over a test's (each a process of its own): a check of
/// schedules that miss what the checks are for.
#[derive(Debug, Default)]
pub struct Reached {
    /// The follower's batches that changed something.
    pub batches: usize,
    /// Reads checked, and of them those of a file the writer had made by their commits.
    pub reads: usize,
    pub reads_of_files: usize,
    /// Holders' reads of files the writer had deleted by their commits.
    pub reads_of_deleted: usize,
}

impl Reached {
    fn add(&mut self, history: &History, batches: usize) {
        self.batches += batches;
        self.reads += history.reads().len();
        self.reads_of_files +=
            history.reads().iter().filter(|read| matches!(read.found, Ok(Some(_)))).count();
        self.reads_of_deleted += history.held_reads_of_deleted();
    }
}

pub static REACHED: std::sync::Mutex<Reached> =
    std::sync::Mutex::new(Reached { batches: 0, reads: 0, reads_of_files: 0, reads_of_deleted: 0 });

/// One run.
pub async fn simulate(profile: Profile) {
    #[cfg(feature = "shuttle")]
    fxfs::follower::set_rules(match profile.mutation {
        Mutation::Breaks(rules) => rules,
        _ => fxfs::follower::Rules::ALL,
    });
    let description = Description { block_size: 4096, capacity: 64 << 20, branch: 1 };
    let store = Store::create(MemDir::new(), description, StoreOptions::default()).unwrap();
    let store = Arc::new(store);
    let writer = Writer::new(store.clone()).await;
    writer.apply(Op::Sync).await;
    let history = Arc::new(Mutex::new(History::new()));
    history.lock().synced(store.epoch().unwrap());

    // The follower, mounted on the commit the writer made first, and the checker's mount of it.
    let follower = {
        let view = store.view();
        let epoch = view.epoch();
        let first = mount(&store, view.duplicate().unwrap()).await;
        let device = match profile.mutation {
            Mutation::ReadsPastItsView => ViewDevice::reading_past_its_view(store.clone(), view),
            _ => ViewDevice::new(store.clone(), view),
        };
        let view = device.view();
        let device = DeviceHolder::new(device);
        let fs = FxFilesystemBuilder::new().read_only(true).open(device).await;
        let fs = Arc::new(fs.expect("mounting the follower failed"));
        compare(&fs, &first, epoch, &BTreeSet::new()).await;
        let following = Following {
            epoch,
            first: first.clone(),
            last: first.clone(),
            previous: first.clone(),
            oracles: vec![first],
            held: BTreeSet::new(),
            batches: 0,
        };
        Arc::new(Follower { fs, view, gate: LockManager::new(), state: Mutex::new(following) })
    };
    // The writer's last sync returned; the follower followed a commit after it.
    let done = Arc::new(AtomicBool::new(false));
    let followed = Arc::new(AtomicBool::new(false));

    let following = {
        let (store, follower, done) = (store.clone(), follower.clone(), done.clone());
        let followed = followed.clone();
        fasync::Task::spawn(async move {
            loop {
                let last = done.load(Ordering::SeqCst);
                follow(&follower, &store, true).await;
                if last {
                    followed.store(true, Ordering::SeqCst);
                    return;
                }
                fasync::yield_now().await;
            }
        })
    };

    let readers: Vec<_> = (0..profile.readers)
        .map(|reader| {
            let (follower, history, followed) =
                (follower.clone(), history.clone(), followed.clone());
            fasync::Task::spawn(async move {
                // Random files, until the follower followed the last commit; then each once.
                let mut finally: Option<Vec<&'static str>> = None;
                loop {
                    let name = match &mut finally {
                        Some(names) => match names.pop() {
                            Some(name) => name,
                            None => break,
                        },
                        None if followed.load(Ordering::SeqCst) => {
                            finally = Some(NAMES.to_vec());
                            continue;
                        }
                        None => NAMES[thread_rng().gen_range(0..NAMES.len())],
                    };
                    {
                        let _gate = follower.gate.read_lock(gate()).await;
                        let (epoch, fs) = {
                            let state = follower.state.lock();
                            let fs = match profile.mutation {
                                Mutation::SkipsChangesTo(skipped) if skipped == name => {
                                    state.first.clone()
                                }
                                Mutation::SplitsCommits if matches!(name, "c" | "d") => {
                                    state.previous.clone()
                                }
                                _ => follower.fs.clone(),
                            };
                            (state.epoch, fs)
                        };
                        let found = observe(&fs, name).await;
                        history.lock().read(Read { reader, epoch, name, found });
                    }
                    fasync::yield_now().await;
                }
            })
        })
        .collect();

    // Holders: each holds a random file open through some of the follower's batches, reading it.
    let holders: Vec<_> = (0..profile.holders)
        .map(|holder| {
            let reader = profile.readers + holder;
            let (follower, history, followed) =
                (follower.clone(), history.clone(), followed.clone());
            fasync::Task::spawn(async move {
                while !followed.load(Ordering::SeqCst) {
                    let name = NAMES[thread_rng().gen_range(0..NAMES.len())];
                    let held = {
                        let _gate = follower.gate.read_lock(gate()).await;
                        let fs = &follower.fs;
                        let root_store = fs.root_store().store_object_id();
                        lookup(fs, name).await.expect("lookup failed").map(|object_id| {
                            follower.state.lock().held.insert(object_id);
                            (object_id, fs.hold(root_store, object_id))
                        })
                    };
                    let Some((object_id, hold)) = held else {
                        fasync::yield_now().await;
                        continue;
                    };
                    for _ in 0..thread_rng().gen_range(1..=4) {
                        {
                            let _gate = follower.gate.read_lock(gate()).await;
                            let epoch = follower.state.lock().epoch;
                            let found = read(&follower.fs, object_id).await;
                            history.lock().held_read(HeldRead { reader, epoch, object_id, found });
                        }
                        fasync::yield_now().await;
                    }
                    let _gate = follower.gate.read_lock(gate()).await;
                    std::mem::drop(hold);
                }
            })
        })
        .collect();

    // The writer.
    let mut model = Model::new();
    for _ in 0..profile.operations {
        let op = Op::random(&model, &mut thread_rng());
        let after = store.epoch().unwrap();
        let created = writer.apply(op).await;
        match op {
            Op::Sync => history.lock().synced(store.epoch().unwrap()),
            Op::Compact => {}
            _ => {
                op.apply(&mut model, created);
                history.lock().operation(after, model.clone());
            }
        }
    }
    writer.apply(Op::Sync).await;
    history.lock().synced(store.epoch().unwrap());
    done.store(true, Ordering::SeqCst);
    following.await;
    for task in readers.into_iter().chain(holders) {
        task.await;
    }

    // With the holders done, the follower's next batch lets go of all it kept for them.
    follow(&follower, &store, false).await;
    let pins = follower.view.lock().pins();
    assert!(pins.is_empty(), "the follower pins {pins:?} with no file held");

    // A mount of the last commit holds what the writer's last operation left.
    let last = mount(&store, store.view()).await;
    for name in NAMES {
        let found = observe(&last, name).await;
        let expected = model.get(name).map(|file| file.contents);
        assert_eq!(found, Ok(expected), "{name} at the last commit");
    }
    Arc::into_inner(last).unwrap().close().await.expect("close failed");

    history.lock().check().unwrap_or_else(|error| panic!("the history is wrong: {error}"));
    let Follower { fs, state, .. } = Arc::into_inner(follower).expect("the follower still in use");
    let Following { first, last, previous, oracles, batches, .. } = state.into_inner();
    REACHED.lock().unwrap().add(&history.lock(), batches);
    std::mem::drop((first, last, previous));
    for fs in oracles.into_iter().chain([fs]) {
        Arc::into_inner(fs).expect("a mount still in use").close().await.expect("close failed");
    }
    writer.fs.close().await.expect("closing the writer failed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxfs::follower::Rules;
    use shuttle::scheduler::{PctScheduler, RandomScheduler, ReplayScheduler, Scheduler};
    use shuttle::{Config, FailurePersistence, Runner};

    /// The runs each test makes: some seconds; `SIM_SOAK` multiplies them.
    fn runs() -> usize {
        let soak: usize = std::env::var("SIM_SOAK").map_or(1, |soak| soak.parse().unwrap());
        20 * soak
    }

    fn config() -> Config {
        let mut config = Config::new();
        config.stack_size = 16 << 20;
        config
    }

    fn run(profile: Profile) -> impl Fn() + Send + Sync + 'static {
        move || fasync::LocalExecutor::new().run_singlethreaded(simulate(profile))
    }

    /// Runs `profile`'s simulation under `scheduler`, once a run has warmed the process up: a
    /// process's first run initializes what lives as long as the process (the in-process kernel,
    /// statics), taking Shuttle's locks then only, so that its schedules replay in no other
    /// process, nor another's in it. After it, every run starts alike, and a failing run's
    /// schedule replays exactly (`failing_schedules_replay`).
    fn simulations(scheduler: impl Scheduler + 'static, config: Config, profile: Profile) {
        static WARM: std::sync::Once = std::sync::Once::new();
        WARM.call_once(|| {
            let scheduler = RandomScheduler::new_from_seed(0, 1);
            Runner::new(scheduler, self::config()).run(run(Profile::default()));
            *REACHED.lock().unwrap() = Reached::default();
        });
        Runner::new(scheduler, config).run(run(profile));
    }

    #[test]
    fn followers_show_prefixes_randomly() {
        simulations(RandomScheduler::new(runs()), config(), Profile::default());
        reached();
        let reached = REACHED.lock().unwrap();
        assert!(reached.reads_of_deleted >= 1, "{reached:?} in {} runs", runs());
    }

    #[test]
    fn followers_show_prefixes_by_pct() {
        simulations(PctScheduler::new(3, runs()), config(), Profile::default());
        reached();
    }

    /// Checks the runs reached what the checks are for: batches that changed something, in most
    /// runs (PCT favours some tasks for long), and reads of files the writer made, many. Reads of
    /// files held open that the writer deleted are the random scheduler's to reach: under PCT, a
    /// holder of low priority mostly runs once the writer is done.
    fn reached() {
        let reached = REACHED.lock().unwrap();
        let runs = runs();
        assert!(reached.batches >= runs / 2, "{reached:?} in {runs} runs");
        assert!(reached.reads_of_files >= runs, "{reached:?} in {runs} runs");
    }

    /// Replays the schedule the file `SHUTTLE_REPLAY` names holds, as a failing run persists it,
    /// of the simulation with the mutation `SIM_MUTATION` names, if any (`Mutation`'s variants
    /// but `Breaks`).
    #[test]
    fn replay() {
        let Some(path) = std::env::var_os("SHUTTLE_REPLAY") else { return };
        let mutation = match std::env::var("SIM_MUTATION").as_deref() {
            Err(_) => Mutation::None,
            Ok("ReadsPastItsView") => Mutation::ReadsPastItsView,
            Ok("SplitsCommits") => Mutation::SplitsCommits,
            Ok(name) => match name.strip_prefix("SkipsChangesTo:") {
                Some(file) => {
                    Mutation::SkipsChangesTo(NAMES.into_iter().find(|n| *n == file).unwrap())
                }
                None => panic!("no mutation {name}"),
            },
        };
        let scheduler = ReplayScheduler::new_from_file(path).unwrap();
        simulations(scheduler, config(), Profile { mutation, ..Profile::default() });
    }

    /// The panics of `runs`, in order, if it failed. A failing schedule's first panic may poison
    /// locks, which other tasks then panic on.
    ///
    /// The panics are collected, and printed, by a hook installed for good, which Shuttle's chains
    /// to: a failing run may leave the thread counted as panicking, when the hook can't be set
    /// back. Each test is a process of its own (nextest).
    fn panics_of(runs: impl FnOnce()) -> Option<Vec<String>> {
        static PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        std::panic::set_hook(Box::new(|info| {
            eprintln!("{info}");
            PANICS.lock().unwrap().push(info.payload_as_str().unwrap_or_default().to_owned());
        }));
        let failed = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(runs)) {
            Ok(()) => false,
            // Its payload may hold Shuttle's objects, which can't be dropped outside a run.
            Err(payload) => {
                std::mem::forget(payload);
                true
            }
        };
        let panics = std::mem::take(&mut *PANICS.lock().unwrap());
        failed.then_some(panics)
    }

    /// Runs the simulation with `mutation`, expecting a schedule to fail saying `expected`.
    fn fails_with(mutation: Mutation, expected: &str) {
        let profile = Profile { mutation, ..Profile::default() };
        // A mutation may open a window of a few steps: ten times the schedules.
        let panics = panics_of(|| {
            simulations(RandomScheduler::new(10 * runs()), config(), profile);
        });
        let panics = panics.unwrap_or_else(|| panic!("no schedule failed with {mutation:?}"));
        assert!(
            panics.iter().any(|panic| panic.contains(expected)),
            "with {mutation:?}, expected {expected:?}, got {panics:#?}"
        );
    }

    /// A failing schedule replays exactly (doc-8, testing 3): Fxfs, the store and the simulation
    /// choose nothing but what Shuttle chooses, so a failure found once is found again, the same,
    /// from its schedule. In a process of its own: a failed run leaves the process's Shuttle locks
    /// closed (the in-process kernel's, which live as long as it), so no run follows one there.
    #[test]
    fn failing_schedules_replay() {
        let directory =
            std::env::temp_dir().join(format!("follower-sim-replay-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut persisting = config();
        persisting.failure_persistence = FailurePersistence::File(Some(directory.clone()));
        let profile = Profile { mutation: Mutation::SplitsCommits, ..Profile::default() };
        let found = panics_of(|| {
            simulations(RandomScheduler::new(10 * runs()), persisting, profile);
        });
        let found = found.and_then(|panics| panics.first().cloned()).expect("no schedule failed");
        // Shuttle persists the schedule as the failing task panics, and as the run fails.
        let mut schedules: Vec<_> =
            std::fs::read_dir(&directory).unwrap().map(|entry| entry.unwrap().path()).collect();
        schedules.sort();
        let schedule = schedules.first().expect("no schedule persisted");
        let replayed = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sim::tests::replay", "--nocapture"])
            .env("SHUTTLE_REPLAY", schedule)
            .env("SIM_MUTATION", "SplitsCommits")
            .output()
            .unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
        let output = String::from_utf8_lossy(&replayed.stderr);
        assert!(
            !replayed.status.success() && output.contains(&found),
            "the failing schedule replayed otherwise than {found:?}: {output}"
        );
    }

    #[test]
    fn catches_followers_reading_past_their_views() {
        fails_with(Mutation::ReadsPastItsView, "isn't its commit's");
    }

    #[test]
    fn catches_followers_splitting_commits() {
        fails_with(Mutation::SplitsCommits, "the history is wrong");
    }

    #[test]
    fn catches_followers_skipping_changes() {
        fails_with(Mutation::SkipsChangesTo("a"), "the history is wrong");
    }

    #[test]
    fn needs_flushed_layers_swapped() {
        let rules = Rules { swap_flushed_layers: false, ..Rules::ALL };
        fails_with(Mutation::Breaks(rules), "layers at commit");
    }

    #[test]
    fn needs_held_files_kept() {
        let rules = Rules { keep_held: false, ..Rules::ALL };
        fails_with(Mutation::Breaks(rules), "holding object");
    }

    #[test]
    fn needs_changes_told_whole() {
        let rules = Rules { tell_data: false, ..Rules::ALL };
        fails_with(Mutation::Breaks(rules), "a batch's changes miss");
    }

    #[test]
    fn needs_released_files_let_go() {
        let rules = Rules { apply_released: false, ..Rules::ALL };
        fails_with(Mutation::Breaks(rules), "aren't its commit's");
    }
}
