// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The simulation (backlog doc-8, testing 3): the writer makes random operations on an extent
//! store's filesystem, syncing now and then; a follower follows it, mounting Fxfs read-only on a
//! view of the store's latest commit, again and again; readers read files through the follower's
//! mount of the moment. Every interleaving of their tasks is Shuttle's to choose. Then:
//!
//! - the history checker (`history`) requires what each reader found to be what the writer's
//!   files were after some operation the follower's commit may hold, and each reader's to move
//!   forward;
//! - each of the follower's mounts is compared with a mount of its commit of the checker's own,
//!   file by file (testing 5);
//! - a mount of the last commit holds what the writer's last operation left.
//!
//! Until the follower that applies the writer's commits to a mount (TASK-42.4), the follower
//! mounts each commit anew: what the checker is first shown to catch with is mutations of it.

use crate::device::ViewDevice;
use crate::history::{History, Read};
use crate::model::{Model, NAMES, Op, Writer, observe};
use extent_store::{Description, MemDir, Options as StoreOptions, Store, View};
use fuchsia_async as fasync;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
use fxfs::object_store::directory::Directory;
use fxfs::object_store::{HandleOptions, ObjectDescriptor, ObjectStore};
use shuttle::rand::{Rng, thread_rng};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use storage_device::DeviceHolder;

/// What the simulation runs: the writer's operations, and the readers.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub operations: usize,
    pub readers: usize,
    pub mutation: Mutation,
}

impl Default for Profile {
    fn default() -> Self {
        Self { operations: 12, readers: 2, mutation: Mutation::None }
    }
}

/// A follower's bug, which the simulation must catch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    None,
    /// Its views read extents of commits after their own: it replays on past its commit.
    ReadsPastItsView,
    /// It shows one file as its first mount did, and the others as they are: it skipped the
    /// changes to that one.
    SkipsChangesTo(&'static str),
    /// It shows two files as its mount before showed them, and the others as its current one does:
    /// it applied part of a commit, as a transaction renaming between them would show.
    SplitsCommits,
}

/// A follower's mount: Fxfs, read-only, on a view, as of the view's commit.
struct Mount {
    epoch: u64,
    fs: Arc<OpenFxFilesystem>,
}

/// Mounts Fxfs read-only on `view`, past it if `past_its_view` (a mutation).
async fn mount(store: &Arc<Store<MemDir>>, view: View, past_its_view: bool) -> Mount {
    let epoch = view.epoch();
    let device = match past_its_view {
        false => ViewDevice::new(store.clone(), view),
        true => ViewDevice::reading_past_its_view(store.clone(), view),
    };
    let device = DeviceHolder::new(device);
    let fs = FxFilesystemBuilder::new().read_only(true).open(device).await.expect("mount failed");
    Mount { epoch, fs: Arc::new(fs) }
}

/// The follower's mounts: its current one, and every one it made, which go once all is done.
#[derive(Default)]
struct Follower {
    current: Option<(u64, Arc<OpenFxFilesystem>)>,
    previous: Option<Arc<OpenFxFilesystem>>,
    first: Option<Arc<OpenFxFilesystem>>,
    all: Vec<Arc<OpenFxFilesystem>>,
}

/// The root directory of `fs`'s root store, entry by entry: each one's object, descriptor and
/// properties, and, of a file, its extents and contents.
async fn tree(fs: &OpenFxFilesystem) -> BTreeMap<String, String> {
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
                format!("file {object_id} {properties:?} {extents:?} {contents:?}")
            }
            ObjectDescriptor::Directory => {
                let directory =
                    Directory::open(&store, object_id).await.expect("opening a directory failed");
                let properties = directory.get_properties().await.expect("properties failed");
                format!("directory {object_id} {properties:?}")
            }
            other => format!("{other:?} {object_id}"),
        };
        tree.insert(name, node);
    }
    tree
}

/// Compares `fs`, a follower's mount, with `oracle`, the checker's of the same commit (doc-8,
/// testing 5): their root directories, entry by entry.
async fn compare(fs: &OpenFxFilesystem, oracle: &OpenFxFilesystem, epoch: u64) {
    let (found, expected) = (tree(fs).await, tree(oracle).await);
    assert_eq!(found, expected, "the follower's tree at commit {epoch} isn't its commit's");
}

/// What the simulation's runs reached, over a test's (each a process of its own): a check of
/// schedules that miss what the checks are for.
#[derive(Debug, Default)]
pub struct Reached {
    /// The follower's mounts after its first.
    pub remounts: usize,
    /// Reads checked, and of them those of a file the writer had made by their commits.
    pub reads: usize,
    pub reads_of_files: usize,
}

impl Reached {
    fn add(&mut self, history: &History, mounts: usize) {
        self.remounts += mounts.saturating_sub(1);
        self.reads += history.reads().len();
        self.reads_of_files +=
            history.reads().iter().filter(|read| matches!(read.found, Ok(Some(_)))).count();
    }
}

pub static REACHED: std::sync::Mutex<Reached> =
    std::sync::Mutex::new(Reached { remounts: 0, reads: 0, reads_of_files: 0 });

/// One run.
pub async fn simulate(profile: Profile) {
    let description = Description { block_size: 4096, capacity: 64 << 20, branch: 1 };
    let store = Store::create(MemDir::new(), description, StoreOptions::default()).unwrap();
    let store = Arc::new(store);
    let writer = Writer::new(store.clone()).await;
    writer.apply(Op::Sync).await;
    let history = Arc::new(fuchsia_sync::Mutex::new(History::new()));
    history.lock().synced(store.epoch().unwrap());
    let follower = Arc::new(fuchsia_sync::Mutex::new(Follower::default()));
    // The writer's last sync returned; the follower mounted a commit after it.
    let done = Arc::new(AtomicBool::new(false));
    let followed = Arc::new(AtomicBool::new(false));

    let following = {
        let (store, follower, done) = (store.clone(), follower.clone(), done.clone());
        let followed = followed.clone();
        fasync::Task::spawn(async move {
            loop {
                let last = done.load(Ordering::SeqCst);
                let view = store.view();
                let oracle = mount(&store, view.duplicate().unwrap(), false).await;
                let past = profile.mutation == Mutation::ReadsPastItsView;
                let Mount { epoch, fs } = mount(&store, view, past).await;
                compare(&fs, &oracle.fs, epoch).await;
                oracle.fs.close().await.expect("closing the checker's mount failed");
                {
                    let mut follower = follower.lock();
                    follower.first.get_or_insert_with(|| fs.clone());
                    follower.previous = follower.current.take().map(|(_, fs)| fs);
                    follower.current = Some((epoch, fs.clone()));
                    follower.all.push(fs);
                }
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
                    let mounted = {
                        let follower = follower.lock();
                        let first = follower.first.clone();
                        let previous = follower.previous.clone();
                        follower.current.clone().map(|(epoch, fs)| match profile.mutation {
                            Mutation::SkipsChangesTo(skipped) if skipped == name => {
                                (epoch, first.unwrap())
                            }
                            Mutation::SplitsCommits if matches!(name, "c" | "d") => {
                                (epoch, previous.unwrap_or(fs))
                            }
                            _ => (epoch, fs),
                        })
                    };
                    if let Some((epoch, fs)) = mounted {
                        let found = observe(&fs, name).await;
                        history.lock().read(Read { reader, epoch, name, found });
                    }
                    fasync::yield_now().await;
                }
            })
        })
        .collect();

    // The writer.
    let mut model = Model::new();
    for _ in 0..profile.operations {
        let op = Op::random(&model, &mut thread_rng());
        let after = store.epoch().unwrap();
        writer.apply(op).await;
        match op {
            Op::Sync => history.lock().synced(store.epoch().unwrap()),
            _ => {
                op.apply(&mut model);
                history.lock().operation(after, model.clone());
            }
        }
    }
    writer.apply(Op::Sync).await;
    history.lock().synced(store.epoch().unwrap());
    done.store(true, Ordering::SeqCst);
    following.await;
    for reader in readers {
        reader.await;
    }

    // A mount of the last commit holds what the writer's last operation left.
    let last = mount(&store, store.view(), false).await;
    for name in NAMES {
        let found = observe(&last.fs, name).await;
        assert_eq!(found, Ok(model.get(name).copied()), "{name} at the last commit");
    }
    last.fs.close().await.expect("close failed");

    history.lock().check().unwrap_or_else(|error| panic!("the history is wrong: {error}"));
    REACHED.lock().unwrap().add(&history.lock(), follower.lock().all.len());
    let all = std::mem::take(&mut follower.lock().all);
    *follower.lock() = Follower::default();
    for fs in all {
        Arc::into_inner(fs).expect("a mount still in use").close().await.expect("close failed");
    }
    writer.fs.close().await.expect("closing the writer failed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use shuttle::scheduler::{PctScheduler, RandomScheduler};
    use shuttle::{Config, Runner};

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

    #[test]
    fn followers_show_prefixes_randomly() {
        Runner::new(RandomScheduler::new(runs()), config()).run(run(Profile::default()));
        reached();
    }

    #[test]
    fn followers_show_prefixes_by_pct() {
        Runner::new(PctScheduler::new(3, runs()), config()).run(run(Profile::default()));
        reached();
    }

    /// Checks the runs reached what the checks are for: remounts, in most runs (PCT favours some
    /// tasks for long), and reads of files the writer made, many.
    fn reached() {
        let reached = REACHED.lock().unwrap();
        let runs = runs();
        assert!(reached.remounts >= runs / 2, "{reached:?} in {runs} runs");
        assert!(reached.reads_of_files >= runs, "{reached:?} in {runs} runs");
    }

    /// Replays the schedule the file `SHUTTLE_REPLAY` names holds, as a failing run prints it.
    #[test]
    fn replay() {
        let Some(path) = std::env::var_os("SHUTTLE_REPLAY") else { return };
        let schedule = std::fs::read_to_string(path).unwrap();
        let scheduler = shuttle::scheduler::ReplayScheduler::new_from_encoded(schedule.trim());
        Runner::new(scheduler, config()).run(run(Profile::default()));
    }

    /// Runs the simulation with `mutation`, expecting a schedule to fail saying `expected`.
    fn fails_with(mutation: Mutation, expected: &str) {
        static PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        std::panic::set_hook(Box::new(|info| {
            eprintln!("{info}");
            PANICS.lock().unwrap().push(info.payload_as_str().unwrap_or_default().to_owned());
        }));
        let profile = Profile { mutation, ..Profile::default() };
        // A mutation may open a window of a few steps: ten times the schedules.
        let failed = match std::panic::catch_unwind(|| {
            Runner::new(RandomScheduler::new(10 * runs()), config()).run(run(profile));
        }) {
            Ok(()) => false,
            // Its payload may hold Shuttle's objects, which can't be dropped outside a run.
            Err(payload) => {
                std::mem::forget(payload);
                true
            }
        };
        let panics = std::mem::take(&mut *PANICS.lock().unwrap());
        assert!(failed, "no schedule failed with {mutation:?}");
        assert!(
            panics.iter().any(|panic| panic.contains(expected)),
            "with {mutation:?}, expected {expected:?}, got {panics:#?}"
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
}
