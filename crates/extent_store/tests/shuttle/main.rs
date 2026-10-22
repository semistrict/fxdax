// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the store's writer, the closer of its open segment and its uploader, run together under
//! Shuttle (awslabs/shuttle), whose scheduler chooses every interleaving of their threads at each
//! lock, condition variable and file operation, and replays a failing one from its seed
//! (TASK-44.4).
//!
//! The writer writes extents (some in segments, some large enough for files of their own), frees
//! some, and commits, ordered or durably, recording what the store holds as of each commit. The
//! closer closes the open segment between commits, as fxdax's uploader has the store do. The
//! uploader, as fxdax's does, closes it too and takes each segment closed once its commits are
//! durable; it reads what each of the segment's commits wrote and the extents its footer names
//! (from the store, or kept for it if freed since), and releases its commits. Their history is
//! checked:
//!
//! - every extent a commit wrote reads, for the uploader, as the writer wrote it, though freed
//!   since, until the uploader releases the commit;
//! - every extent a closed segment's footer names, the segment holds as the writer wrote it as of
//!   the segment's last commit, or its file of its own does, and it reads so for the uploader;
//! - once all is done, the store holds what the writer's last commit left;
//! - the store, crashed and reopened, holds at least every commit the uploader took: it only
//!   takes the segments of durable ones.
//!
//! Commits are made durable by a flusher, too, while an uploader begins, as of the latest commit,
//! and while the branch closes: the uploader takes only the segments of commits a flush made
//! durable, and the branch, closed, holds its last commit durably. A free races a write in flight,
//! too, which is dropped, whenever it finishes; and writes fill segments between commits, which
//! go to the next as each is full.
//!
//! So are a lineage's branches, while a fork's or a deletion's I/O stalls (lineage.rs).
//!
//! Each rule the store relies on for these ([`Rules`]) is turned off by a test that expects a
//! failure: a check that never fails proves nothing. Run with the `shuttle` feature:
//! `cargo nextest run -p extent_store --features shuttle --test shuttle`.

#![cfg(feature = "shuttle")]

use extent_frame::segment::Chain;
use extent_store::{Description, File as _, MemDir, Options, Rules, Store, set_rules};
use shuttle::scheduler::{PctScheduler, RandomScheduler, ReplayScheduler};
use shuttle::sync::Mutex;
use shuttle::sync::atomic::{AtomicBool, Ordering};
use shuttle::thread;
use shuttle::{Config, Runner};
use std::collections::BTreeMap;
use std::sync::Arc;
use storage_device::{ExtentId, NoExtent};

mod lineage;

const BLOCK: usize = 4096;
const BRANCH: u64 = 7;
/// Small segments, so that commits and the closer close several, and an extent of four blocks in
/// a file of its own.
const OPTIONS: Options = Options {
    max_extent: 64 << 20,
    segment_size: 3 * BLOCK as u64,
    segment_age: std::time::Duration::from_secs(3600),
    large_extent: 4 * BLOCK as u64,
    checkpoint_after: 4 << 10,
    open_files: 2,
    view_lag: None,
    view_kept_bytes: None,
};
/// The chain of a writer of the branch in object storage, whose segments the store closes.
const WRITER: Chain = Chain::new(BRANCH, 1, 0, None, 0);

/// The schedules each test runs, randomly and by PCT: some seconds; `SHUTTLE_SOAK` multiplies them.
fn runs() -> usize {
    let soak: usize = std::env::var("SHUTTLE_SOAK").map_or(1, |soak| soak.parse().unwrap());
    1500 * soak
}

/// `Ok` if `holds`, or else what `failure` says went wrong.
fn check(holds: bool, failure: impl FnOnce() -> String) -> Result<(), String> {
    if holds { Ok(()) } else { Err(failure()) }
}

fn extent(sequence: u64) -> ExtentId {
    ExtentId::new(BRANCH, sequence)
}

/// Bytes each their offset modulo 251, plus `seed`.
fn pattern(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8 ^ seed).collect()
}

/// What the store holds as of a commit: each extent's data.
type Holds = BTreeMap<ExtentId, Vec<u8>>;

/// The writer's steps: extents written (sequence, blocks), freed, commits, and a fork, which
/// commits durably.
#[derive(Clone, Copy)]
enum Step {
    Write(u64, usize),
    Free(u64),
    Commit { durable: bool },
    Fork,
}

/// The branch the writer forks.
const CHILD: u64 = BRANCH + 1;

const STEPS: &[Step] = &[
    Step::Write(1, 1),
    Step::Write(2, 4),
    Step::Commit { durable: false },
    Step::Write(3, 2),
    Step::Free(1),
    Step::Commit { durable: true },
    Step::Free(2),
    Step::Write(4, 1),
    Step::Fork,
    Step::Commit { durable: false },
    Step::Free(3),
    Step::Write(5, 1),
    Step::Commit { durable: true },
];

/// How the follower follows: advancing its view, and pinning an extent, or dropping its view and
/// opening another, which no view may be live between.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Follow {
    Advance,
    Reopen,
}

/// One run: the writer, the closer, the uploader and a follower, then the checks.
fn run() {
    run_with(true, Follow::Advance);
}

/// One run without the uploader, which keeps frees too, so that only views keep them.
fn following() {
    run_with(false, Follow::Advance);
}

/// One run without the uploader, its follower reopening its view.
fn reopening() {
    run_with(false, Follow::Reopen);
}

fn run_with(uploading: bool, follow: Follow) {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Arc::new(Store::create(dir.clone(), description, OPTIONS).unwrap());
    if uploading {
        assert!(store.upload(WRITER, 0).unwrap());
    }
    // What the store holds as of each commit, by epoch, the writer's record.
    let history = Arc::new(Mutex::new(vec![Holds::new()]));
    let written = Arc::new(AtomicBool::new(false));
    // What the store held at the fork, which its child holds.
    let forked = Arc::new(Mutex::new(Holds::new()));

    let writer = {
        let (store, history, written, forked) =
            (store.clone(), history.clone(), written.clone(), forked.clone());
        thread::spawn(move || {
            let mut holds = Holds::new();
            for step in STEPS {
                match *step {
                    Step::Write(sequence, blocks) => {
                        let data = pattern(sequence as u8, blocks * BLOCK);
                        store.write(extent(sequence), 0, &data).unwrap();
                        holds.insert(extent(sequence), data);
                    }
                    Step::Free(sequence) => {
                        store.free(extent(sequence)).unwrap();
                        holds.remove(&extent(sequence));
                    }
                    Step::Commit { .. } | Step::Fork => {
                        match *step {
                            Step::Fork => {
                                store.fork(CHILD).unwrap();
                                *forked.lock().unwrap() = holds.clone();
                            }
                            Step::Commit { durable: true } => store.commit(None).unwrap(),
                            _ => store.commit_ordered(None).unwrap(),
                        }
                        let mut history = history.lock().unwrap();
                        assert_eq!(store.epoch().unwrap(), history.len() as u64);
                        history.push(holds.clone());
                    }
                }
            }
            written.store(true, Ordering::SeqCst);
        })
    };

    let closer = {
        let store = store.clone();
        thread::spawn(move || {
            for _ in 0..3 {
                store.close_now().unwrap();
                thread::yield_now();
            }
        })
    };

    // The last commit the uploader took, which the store must recover at least.
    let taken = Arc::new(Mutex::new(0));
    let uploader = uploading.then(|| {
        let (store, dir, history, written, taken) =
            (store.clone(), dir.clone(), history.clone(), written.clone(), taken.clone());
        thread::spawn(move || {
            // What the store held as of commit `epoch`, which the writer records once the commit
            // returns: its record may come later.
            let held = |epoch: u64| loop {
                if let Some(holds) = history.lock().unwrap().get(epoch as usize) {
                    break holds.clone();
                }
                thread::yield_now();
            };
            loop {
                // Whether the writer was done before this round: then it takes everything.
                let done = written.load(Ordering::SeqCst);
                // As fxdax's uploader does (remote.rs): closes the open segment between commits,
                // and takes the segments closed whose commits are durable, reading them and their
                // extents' own files.
                store.close_now().unwrap();
                // Taking them opens their extents' own files, which the store keeps for it though
                // their extents are freed.
                let segments = store
                    .closed()
                    .unwrap_or_else(|error| panic!("taking the closed segments: {error:#}"));
                for closed in segments {
                    let last = closed.last();
                    // Each commit it holds, after those of the segments taken before: what the
                    // commit wrote reads, for the uploader, as it was written, though freed since;
                    // and the host crashing now, the store recovers at least the commit.
                    let after = *taken.lock().unwrap();
                    for epoch in after + 1..=last {
                        let (before, holds) = (held(epoch - 1), held(epoch));
                        for (extent, data) in &holds {
                            if before.get(extent) == Some(data) {
                                continue;
                            }
                            let mut read = vec![0; data.len()];
                            store.read_kept(*extent, 0, &mut read).unwrap_or_else(|error| {
                                panic!("{extent:?} of commit {epoch}: {error:#}")
                            });
                            assert!(
                                &read == data,
                                "{extent:?} of commit {epoch} reads other data than was written"
                            );
                        }
                        for seed in [epoch, epoch + 100] {
                            let crashed = Store::open(dir.crash(seed), OPTIONS).unwrap();
                            let recovered = crashed.epoch().unwrap();
                            assert!(
                                recovered >= epoch,
                                "uploaded commit {epoch} before durable: a crash recovered \
                                 {recovered}"
                            );
                        }
                        *taken.lock().unwrap() = epoch;
                    }
                    let holds = held(last);
                    let data = extent_frame::data(&closed.bytes().unwrap()).unwrap();
                    let own: BTreeMap<_, _> = closed.own_bytes().unwrap().into_iter().collect();
                    let in_segment = closed.footer.extents.iter().map(|&(extent, offset, len)| {
                        (extent, len, data[offset as usize..(offset + len) as usize].to_vec())
                    });
                    let on_their_own = closed.footer.own.iter().map(|(extent, location)| {
                        let data = extent_frame::data(&own[&location.key]).unwrap();
                        (*extent, location.len, data)
                    });
                    // Each extent its footer names, as its last commit left it: in it, or in a
                    // file of its own, and for the uploader, though freed since.
                    for (extent, len, uploaded) in in_segment.chain(on_their_own) {
                        let mut data = vec![0; len as usize];
                        store.read_kept(extent, 0, &mut data).unwrap_or_else(|error| {
                            panic!("{extent:?} of commit {last}: {error:#}")
                        });
                        assert!(
                            Some(&data) == holds.get(&extent) && uploaded == data,
                            "{extent:?} of commit {last} reads other data than was written"
                        );
                    }
                    // Uploaded: the commits it holds are released, whose frees its extents' own
                    // files may be of, and it may go.
                    store.release(last).unwrap();
                    store.uploaded(closed.number).unwrap();
                }
                if done {
                    return;
                }
                thread::yield_now();
            }
        })
    });

    // A follower (TASK-42.2): reads, through a view, every extent the commit it is of holds, as
    // the writer recorded it, though freed since; advances; and pins an extent the writer frees
    // later, which reads as it was written after the view advances past its free.
    let follower = {
        let (store, history, written) = (store.clone(), history.clone(), written.clone());
        thread::spawn(move || {
            let pinned = extent(2);
            let mut pin = None;
            let mut view = Some(store.view());
            // What it finds wrong, it panics with once its view is gone: a task panicking unwinds
            // through what it holds, whose drops take Shuttle's locks, which fail in a failed run
            // with panics that abort the process (TASK-55).
            let followed = (|| -> Result<(), String> {
                loop {
                    let current = view.as_mut().unwrap();
                    let done = written.load(Ordering::SeqCst);
                    // A view opens as of a commit as it is made: its record may come later.
                    let holds = loop {
                        if let Some(holds) = history.lock().unwrap().get(current.epoch() as usize) {
                            break holds.clone();
                        }
                        thread::yield_now();
                    };
                    for (extent, data) in &holds {
                        let mut read = vec![0; data.len()];
                        store.read_view(current, *extent, 0, &mut read).map_err(|error| {
                            format!("a view's read of {extent:?} at {}: {error:#}", current.epoch())
                        })?;
                        check(&read == data, || {
                            format!("a view's read of {extent:?} reads other data")
                        })?;
                    }
                    // An extent its commit doesn't hold, and it doesn't pin, it refuses: written
                    // since, or freed before, kept for others (the uploader) or not.
                    for sequence in 1..=5 {
                        let extent = extent(sequence);
                        let pinning = pin.is_some() && extent == pinned;
                        if holds.contains_key(&extent) || pinning {
                            continue;
                        }
                        let mut read = vec![0; BLOCK];
                        let result = store.read_view(current, extent, 0, &mut read);
                        check(
                            matches!(&result, Err(error) if error.downcast_ref::<NoExtent>().is_some()),
                            || {
                                format!(
                                    "a view at {} read {extent:?}, which its commit doesn't hold: {result:?}",
                                    current.epoch()
                                )
                            },
                        )?;
                    }
                    if follow == Follow::Advance
                        && pin.is_none()
                        && let Some(data) = holds.get(&pinned)
                    {
                        current.pin(pinned).map_err(|error| format!("pinning: {error:#}"))?;
                        pin = Some(data.clone());
                    }
                    if let Some(data) = &pin {
                        let mut read = vec![0; data.len()];
                        store.read_view(current, pinned, 0, &mut read).map_err(|error| {
                            format!(
                                "a view's read of pinned {pinned:?} at {}: {error:#}",
                                current.epoch()
                            )
                        })?;
                        check(&read == data, || {
                            format!("a view's read of pinned {pinned:?} reads other data")
                        })?;
                    }
                    if done {
                        return Ok(());
                    }
                    match follow {
                        Follow::Advance => {
                            current.advance().map_err(|error| format!("advancing: {error:#}"))?
                        }
                        Follow::Reopen => {
                            view = None;
                            thread::yield_now();
                            view = Some(store.view());
                        }
                    }
                    thread::yield_now();
                }
            })();
            drop(view);
            if let Err(failure) = followed {
                panic!("{failure}");
            }
        })
    };

    writer.join().unwrap();
    closer.join().unwrap();
    if let Some(uploader) = uploader {
        uploader.join().unwrap();
    }
    follower.join().unwrap();

    // The store holds what the writer's last commit left, and nothing it freed.
    let holds = history.lock().unwrap().last().unwrap().clone();
    for (extent, data) in &holds {
        let mut read = vec![0; data.len()];
        store.read(*extent, 0, &mut read).unwrap();
        assert!(&read == data, "{extent:?} reads other data than was written");
    }
    let held: Vec<_> = store.extents().unwrap().into_iter().map(|(extent, _)| extent).collect();
    assert_eq!(held, holds.keys().copied().collect::<Vec<_>>());

    // The fork holds what the store held at the fork, though the store freed some since.
    let forked = forked.lock().unwrap().clone();
    let child = store.lineage().open_branch(CHILD).unwrap();
    let held: Vec<_> = child.extents().unwrap().into_iter().map(|(extent, _)| extent).collect();
    assert_eq!(held, forked.keys().copied().collect::<Vec<_>>());
    for (extent, data) in &forked {
        let mut read = vec![0; data.len()];
        child.read(*extent, 0, &mut read).unwrap();
        assert!(&read == data, "{extent:?} reads other data in the fork than was written");
    }

    // Crashed, at any of these points, the store recovers at least what the uploader took.
    let taken = *taken.lock().unwrap();
    for seed in 0..4 {
        let crashed = Store::open(dir.crash(seed), OPTIONS).unwrap();
        let epoch = crashed.epoch().unwrap();
        assert!(epoch >= taken, "crash {seed}: recovered commit {epoch}, uploaded {taken}");
    }
}

/// One run of writes racing commits, as a VM's requests may: a writer writes extents, in segments
/// and in a file of its own, while a committer commits and the closer closes the open segment
/// between commits. Each extent a commit sealed holds what was written to it, and the segments
/// closed after hold what their commits sealed, in them.
fn racing() {
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Arc::new(Store::create(MemDir::new(), description, OPTIONS).unwrap());
    assert!(store.upload(WRITER, 0).unwrap());
    // Extents of one block, in segments, and of four, in a file of its own.
    const WRITES: [(u64, usize); 3] = [(1, 1), (2, 4), (3, 1)];
    let writer = {
        let store = store.clone();
        thread::spawn(move || {
            for (sequence, blocks) in WRITES {
                store.write(extent(sequence), 0, &pattern(sequence as u8, blocks * BLOCK)).unwrap();
            }
            // A write begun in a segment and abandoned, as a failed one is: commits and closings
            // waiting for it go on.
            drop(store.begin_write(extent(9), 0, BLOCK as u64).unwrap());
        })
    };
    let committer = {
        let store = store.clone();
        thread::spawn(move || {
            for _ in 0..2 {
                store.commit_ordered(None).expect("a write raced a commit");
            }
        })
    };
    let closer = {
        let store = store.clone();
        thread::spawn(move || {
            for _ in 0..2 {
                store.close_now().expect("a write raced a commit");
                thread::yield_now();
            }
        })
    };
    for thread in [writer, committer, closer] {
        thread.join().unwrap();
    }
    store.commit(None).expect("a write raced a commit");
    store.close_segment().unwrap();
    store.commit(None).expect("a write raced a commit");
    for (sequence, blocks) in WRITES {
        let mut data = vec![0; blocks * BLOCK];
        store.read(extent(sequence), 0, &mut data).expect("a write raced a commit");
        assert!(
            data == pattern(sequence as u8, blocks * BLOCK),
            "a write raced a commit: extent {sequence} reads other data"
        );
    }
}

/// Segments of three blocks, and extents of four at most: a segment holds seven blocks at most,
/// past which writes between commits go to another (TASK-79.2).
const ROLLING: Options = Options { max_extent: 4 * BLOCK as u64, ..OPTIONS };

/// One run of writes filling segments between commits, as a VM's requests may (TASK-79.2): a
/// writer writes extents of two blocks, in segments, while a committer commits and the closer
/// closes the open segment between commits. The open segment, full, is set aside, or, holding
/// commits' data, closed, and writes go to the next, while commits seal what was written. Each
/// extent reads as written, and the store, crashed, opens again holding what its last commit did.
fn rolling_over() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Arc::new(Store::create(dir.clone(), description, ROLLING).unwrap());
    assert!(store.upload(WRITER, 0).unwrap());
    const EXTENTS: u64 = 6;
    let writer = {
        let store = store.clone();
        thread::spawn(move || {
            for sequence in 1..=EXTENTS {
                let data = pattern(sequence as u8, 2 * BLOCK);
                store.write(extent(sequence), 0, &data).expect("a write filling segments failed");
            }
        })
    };
    let committer = {
        let store = store.clone();
        thread::spawn(move || {
            for _ in 0..2 {
                store.commit_ordered(None).expect("a commit of writes filling segments failed");
            }
        })
    };
    let closer = {
        let store = store.clone();
        thread::spawn(move || {
            for _ in 0..2 {
                store.close_now().expect("a closing of segments being filled failed");
                thread::yield_now();
            }
        })
    };
    for thread in [writer, committer, closer] {
        thread.join().unwrap();
    }
    store.commit(None).expect("a commit of writes filling segments failed");
    let check = |store: &Store<MemDir>| {
        for sequence in 1..=EXTENTS {
            let mut data = vec![0; 2 * BLOCK];
            store.read(extent(sequence), 0, &mut data).unwrap();
            assert!(
                data == pattern(sequence as u8, 2 * BLOCK),
                "extent {sequence}, written filling segments, reads other data"
            );
        }
    };
    check(&store);
    check(&Store::open(dir.crash(0), ROLLING).unwrap());
}

/// One run of a free racing a write in flight, as a VM's requests may (TASK-79.1): a writer
/// begins a write of an extent of four blocks, in a file of its own, and finishes it; once it has
/// begun, a freer frees the extent, and writes its second block again, in a segment. The write the
/// free raced is dropped, whenever it finishes: the extent holds the freer's write alone, and the
/// store commits, and opens again holding that.
fn freeing_during_a_write() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Arc::new(Store::create(dir.clone(), description, OPTIONS).unwrap());
    let begun = Arc::new(AtomicBool::new(false));
    let writer = {
        let (store, begun) = (store.clone(), begun.clone());
        thread::spawn(move || {
            let data = pattern(1, 4 * BLOCK);
            let write = store.begin_write(extent(1), 0, data.len() as u64).unwrap();
            begun.store(true, Ordering::SeqCst);
            write.file().write_at(&data, write.offset()).unwrap();
            write.finish();
        })
    };
    let freer = {
        let store = store.clone();
        thread::spawn(move || {
            while !begun.load(Ordering::SeqCst) {
                thread::yield_now();
            }
            store.free(extent(1)).unwrap();
            store
                .write(extent(1), BLOCK as u64, &pattern(2, BLOCK))
                .expect("a write to a freed extent landed: the extent is sealed");
        })
    };
    for thread in [writer, freer] {
        thread.join().unwrap();
    }
    let written = [vec![0; BLOCK], pattern(2, BLOCK)].concat();
    let check = |store: &Store<MemDir>| {
        let held = store.extents().unwrap();
        assert!(
            held == [(extent(1), 2 * BLOCK as u64)],
            "a write to a freed extent landed: the store holds {held:?}"
        );
        let mut read = vec![0; 2 * BLOCK];
        store.read(extent(1), 0, &mut read).unwrap();
        assert!(read == written, "a write to a freed extent landed: the extent reads other data");
    };
    check(&store);
    store.commit(None).expect("a write to a freed extent landed: the commit failed");
    drop(Arc::into_inner(store).unwrap());
    check(&Store::open(dir, OPTIONS).expect("a write to a freed extent landed: no store opens"));
}

/// One run of an uploader beginning while commits are made durable: a writer orders commits, a
/// flusher makes them durable, and the uploader begins, as fxdax's does, as of the store's latest
/// commit, closes the open segment between commits, and takes the segments closed. A crash
/// recovers at least the last commit of each segment it took: it takes only those of commits a
/// flush made durable, not those of commits a flush took and has yet to sync.
fn uploading_during_flushes() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Arc::new(Store::create(dir.clone(), description, OPTIONS).unwrap());
    let writer = {
        let store = store.clone();
        thread::spawn(move || {
            for sequence in 1..=3 {
                store.write(extent(sequence), 0, &pattern(sequence as u8, BLOCK)).unwrap();
                store.commit_ordered(None).unwrap();
            }
        })
    };
    let flusher = {
        let store = store.clone();
        thread::spawn(move || {
            for _ in 0..3 {
                store.flush().unwrap();
                thread::yield_now();
            }
        })
    };
    let uploader = {
        let store = store.clone();
        thread::spawn(move || {
            // A commit between the snapshot and the start, and the uploader starts again.
            while !store.upload(WRITER, store.epoch().unwrap()).unwrap() {
                thread::yield_now();
            }
            store.close_now().unwrap();
            for closed in store.closed().unwrap() {
                let crashed = Store::open(dir.crash(closed.last()), OPTIONS).unwrap();
                let epoch = crashed.epoch().unwrap();
                assert!(
                    epoch >= closed.last(),
                    "uploaded a segment of commit {} before durable: a crash recovered {epoch}",
                    closed.last()
                );
            }
        })
    };
    for thread in [writer, flusher, uploader] {
        thread.join().unwrap();
    }
}

/// One run of the branch closing while a flush makes its commits durable: once closed, it holds
/// its last commit durably, as another store opening it, or a crash, finds.
fn closing_during_a_flush() {
    let dir = MemDir::new();
    let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: BRANCH };
    let store = Arc::new(Store::create(dir.clone(), description, OPTIONS).unwrap());
    for sequence in 1..=2 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, BLOCK)).unwrap();
        store.commit_ordered(None).unwrap();
    }
    let flusher = {
        let store = store.clone();
        thread::spawn(move || store.flush().unwrap())
    };
    let closer = {
        let store = store.clone();
        thread::spawn(move || {
            store.close().unwrap();
            let epoch = Store::open(dir.crash(0), OPTIONS).unwrap().epoch().unwrap();
            assert!(epoch == 2, "a crash after the branch closed recovered commit {epoch} of 2");
        })
    };
    for thread in [flusher, closer] {
        thread.join().unwrap();
    }
}

/// Shuttle's configuration: threads' stacks large enough for the store's (Shuttle runs each on a
/// stack of its own, of 60 KiB by default).
fn config() -> Config {
    let mut config = Config::new();
    config.stack_size = 4 << 20;
    config
}

/// Runs random schedules of `test`.
fn randomly(test: fn()) {
    Runner::new(RandomScheduler::new(runs()), config()).run(test);
}

#[test]
fn writer_closer_and_uploader_randomly() {
    set_rules(Rules::ALL);
    randomly(run);
}

#[test]
fn writer_closer_and_uploader_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(run);
}

#[test]
fn writes_racing_commits_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(racing);
}

#[test]
fn writes_filling_segments_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(rolling_over);
}

#[test]
fn writes_filling_segments_randomly() {
    set_rules(Rules::ALL);
    randomly(rolling_over);
}

#[test]
fn frees_racing_writes_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(freeing_during_a_write);
}

#[test]
fn uploads_begun_during_flushes_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(uploading_during_flushes);
}

#[test]
fn closing_during_a_flush_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(closing_during_a_flush);
}

#[test]
fn writer_and_follower_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(following);
}

#[test]
fn writer_and_follower_reopening_by_pct() {
    set_rules(Rules::ALL);
    Runner::new(PctScheduler::new(3, runs()), config()).run(reopening);
}

/// Replays the schedule the file `SHUTTLE_REPLAY` names holds, as a failing run prints it, if set:
/// with this test's configuration, which `shuttle::replay`'s default stacks are too small for; and
/// with the rules `SHUTTLE_RULES_OFF` names (separated by commas) off, as a `needs_` test had them.
#[test]
fn replay() {
    let Some(path) = std::env::var_os("SHUTTLE_REPLAY") else { return };
    let mut rules = Rules::ALL;
    for rule in std::env::var("SHUTTLE_RULES_OFF").unwrap_or_default().split(',') {
        match rule.trim() {
            "" => {}
            "keep_frees" => rules.keep_frees = false,
            "close_waits_for_writers" => rules.close_waits_for_writers = false,
            "commit_waits_for_writes" => rules.commit_waits_for_writes = false,
            "drop_writes_to_freed_extents" => rules.drop_writes_to_freed_extents = false,
            "durable_only" => rules.durable_only = false,
            "durable_once_synced" => rules.durable_once_synced = false,
            "close_waits_for_flushes" => rules.close_waits_for_flushes = false,
            "keep_frees_for_views" => rules.keep_frees_for_views = false,
            "keep_pins" => rules.keep_pins = false,
            "decide_at_publish" => rules.decide_at_publish = false,
            "views_refuse_later_extents" => rules.views_refuse_later_extents = false,
            "views_hold_their_commit" => rules.views_hold_their_commit = false,
            "wait_for_forks_and_deletions" => rules.wait_for_forks_and_deletions = false,
            other => panic!("SHUTTLE_RULES_OFF names no rule {other:?}"),
        }
    }
    set_rules(rules);
    let schedule = std::fs::read_to_string(path).unwrap();
    let scheduler = ReplayScheduler::new_from_encoded(schedule.trim());
    // The scenario it is of: `SHUTTLE_SCENARIO`, by its function's name.
    let scenario = match std::env::var("SHUTTLE_SCENARIO").as_deref() {
        Ok("following") => following,
        Ok("reopening") => reopening,
        Ok("racing") => racing,
        Ok("rolling_over") => rolling_over,
        Ok("freeing_during_a_write") => freeing_during_a_write,
        Ok("uploading_during_flushes") => uploading_during_flushes,
        Ok("closing_during_a_flush") => closing_during_a_flush,
        Ok("deleting_stalled_marking") => lineage::deleting_stalled_marking,
        Ok("deleting_stalled_removing_its_index") => lineage::deleting_stalled_removing_its_index,
        Ok("forking_stalled") => lineage::forking_stalled,
        Ok("first_opening_stalled") => lineage::first_opening_stalled,
        Ok("capacity_change_stalled") => lineage::capacity_change_stalled,
        Ok("openings_and_deletions_during_a_deletion") => {
            lineage::openings_and_deletions_during_a_deletion
        }
        Ok("forks_to_one_branch") => lineage::forks_to_one_branch,
        Ok("listing_during_a_deletion") => lineage::listing_during_a_deletion,
        _ => run,
    };
    Runner::new(scheduler, config()).run(scenario);
}

/// The batches of schedules a `needs_` test runs at most to find its failure, by PCT and randomly
/// in turn.
const BATCHES: usize = 8;

/// Runs the schedules with `rules`, expecting one to fail with a panic saying one of `expected`: a
/// violation the rule turned off prevents, not another. A rule may prevent several, of which the
/// schedule decides which shows first. A failing schedule's first panic may poison locks, which
/// other threads then panic on: any of its panics may say it.
///
/// The schedules are random: a batch may find none that fails, which isn't that the rule is not
/// needed (TASK-55: one of 1,500 PCT schedules missed on Linux). Batches are run, by PCT and
/// randomly in turn, until one finds it, up to [`BATCHES`].
///
/// The panics are collected, and printed, by a hook installed for good, which Shuttle's chains
/// to: a failing run may leave the thread counted as panicking (its tasks' coroutines torn down
/// mid-panic), when the hook can't be set back. Each test is a process of its own (nextest).
fn fails_without(rules: Rules, expected: &[&str], test: fn()) {
    static PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        PANICS.lock().unwrap().push(info.payload_as_str().unwrap_or_default().to_owned());
    }));
    set_rules(rules);
    let failed = (0..BATCHES).any(|batch| {
        match std::panic::catch_unwind(|| {
            if batch % 2 == 0 {
                Runner::new(PctScheduler::new(3, runs()), config()).run(test);
            } else {
                Runner::new(RandomScheduler::new(runs()), config()).run(test);
            }
        }) {
            Ok(()) => false,
            // Its payload may hold Shuttle's objects (a lock's poisoned guard), which can't be
            // dropped outside a Shuttle run.
            Err(payload) => {
                std::mem::forget(payload);
                true
            }
        }
    });
    set_rules(Rules::ALL);
    let panics = std::mem::take(&mut *PANICS.lock().unwrap());
    assert!(failed, "no schedule failed with {rules:?}");
    assert!(
        panics.iter().any(|panic| expected.iter().any(|expected| panic.contains(expected))),
        "with {rules:?}, expected one of {expected:?}, got {panics:#?}"
    );
}

/// Frees not kept for the uploader: its read of an extent a commit wrote and a later one freed
/// finds no extent; or, taking the segments closed, it finds the file of extent 2, the one stored
/// on its own that the writer frees, gone.
#[test]
fn needs_frees_kept() {
    let expected = ["no extent", "extents/0000000000000007/0000000000000002 doesn't exist"];
    fails_without(Rules { keep_frees: false, ..Rules::ALL }, &expected, run);
}

#[test]
fn needs_closes_to_wait_for_writers() {
    let rules = Rules { close_waits_for_writers: false, ..Rules::ALL };
    fails_without(rules, &["reads other data"], run);
}

/// Commits sealing extents writes in progress write to: what they write lands after the seal, in
/// a segment closed before the next commit names it, or lost from an extent's own file.
#[test]
fn needs_commits_to_wait_for_writes() {
    let rules = Rules { commit_waits_for_writes: false, ..Rules::ALL };
    fails_without(rules, &["a write raced a commit"], racing);
}

/// Writes to extents freed since they began recorded as they finish: the extent holds the
/// write's data, or another placement's, which panics the store, or is sealed outside the open
/// extents, which refuses the freer's write.
#[test]
fn needs_writes_to_freed_extents_dropped() {
    let rules = Rules { drop_writes_to_freed_extents: false, ..Rules::ALL };
    let expected =
        ["a write to a freed extent landed", "an extent's data is in segments or its own file"];
    fails_without(rules, &expected, freeing_during_a_write);
}

#[test]
fn needs_durable_commits_only() {
    fails_without(Rules { durable_only: false, ..Rules::ALL }, &["before durable"], run);
}

/// Commits a flush has taken counted durable before it syncs them: an uploader beginning meanwhile
/// takes a segment of commits a crash then loses.
#[test]
fn needs_commits_durable_once_synced() {
    let rules = Rules { durable_once_synced: false, ..Rules::ALL };
    fails_without(rules, &["before durable"], uploading_during_flushes);
}

/// The branch closing while a flush is in flight, as if nothing were left to flush: a crash then
/// loses commits made before it closed.
#[test]
fn needs_closes_to_wait_for_flushes() {
    let rules = Rules { close_waits_for_flushes: false, ..Rules::ALL };
    fails_without(rules, &["a crash after the branch closed"], closing_during_a_flush);
}

/// Frees not kept for views: a view's read of an extent its commit holds finds it gone, or, the
/// follower having pinned it too late, its read of it pinned.
#[test]
fn needs_frees_kept_for_views() {
    let rules = Rules { keep_frees_for_views: false, ..Rules::ALL };
    fails_without(rules, &["a view's read of"], following);
}

#[test]
fn needs_pins_kept() {
    let rules = Rules { keep_pins: false, ..Rules::ALL };
    fails_without(rules, &["a view's read of pinned"], following);
}

/// What a commit's frees keep decided apart from its publishing: a view opening between the two,
/// as of the commit before, finds an extent it holds freed and gone.
#[test]
fn needs_frees_decided_as_commits_are_published() {
    let rules = Rules { decide_at_publish: false, ..Rules::ALL };
    fails_without(rules, &["a view's read of"], reopening);
}

/// Views reading frees kept for others from before their commits: a follower, reading what its
/// commit doesn't hold, would take another's data for its own.
#[test]
fn needs_views_to_hold_only_their_commit() {
    let rules = Rules { views_hold_their_commit: false, ..Rules::ALL };
    fails_without(rules, &["which its commit doesn't hold"], following);
}

/// Views reading extents sealed after their commits: a follower's journal replay would go on past
/// its view's commit.
#[test]
fn needs_views_to_refuse_later_extents() {
    let rules = Rules { views_refuse_later_extents: false, ..Rules::ALL };
    fails_without(rules, &["which its commit doesn't hold"], following);
}
