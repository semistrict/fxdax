// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The collector of the file buckets images keep their branches in, each its own, beside its
//! directory (backlog doc-14, TASK-165.6): what reclaims what their branches no longer need, and
//! compacts what is mostly freed, there, the only cleaner of what images free (TASK-165.8). No
//! `fxdax-gc` collects an image's own bucket, as one does a host's: the processes that have images
//! of it open do, a pass after an image's commit, on a thread of its own, at most one every
//! [`EVERY`]. A process has one collector of a bucket, however many of its images it has open, an
//! image and its clones, which reads again only the branches' records that changed (`Collector`).

use extent_remote::collector::{Collector, Options, Report};
use extent_remote::store::Store as Bucket;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often passes run, and how they collect.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pace {
    /// How long at least goes from one pass's start to the next's.
    pub every: Duration,
    pub options: Options,
}

/// A pass a minute, collecting as `fxdax-gc` does by default.
const PACE: Pace = Pace { every: EVERY, options: Options { compact_below: 0.5, grace: 60_000 } };

/// How long at least goes from one pass's start to the next's.
const EVERY: Duration = Duration::from_secs(60);

#[cfg(test)]
thread_local! {
    /// The pace of the collectors this thread makes, if not [`PACE`] ([`quicken`]).
    static QUICKENED: std::cell::Cell<Option<Pace>> = const { std::cell::Cell::new(None) };
}

/// Has the collectors this thread makes from now on go at `pace`: as tests have passes run often,
/// and delete at once, among commits (TASK-291.3).
#[cfg(test)]
pub(crate) fn quicken(pace: Pace) {
    QUICKENED.with(|quickened| quickened.set(Some(pace)));
}

/// The pace of a collector made now.
fn pace() -> Pace {
    #[cfg(test)]
    if let Some(pace) = QUICKENED.with(std::cell::Cell::get) {
        return pace;
    }
    PACE
}

/// The collectors of the buckets this process has images of open, by their URLs.
static COLLECTING: Mutex<BTreeMap<String, Weak<Collecting>>> = Mutex::new(BTreeMap::new());

/// The collector of `bucket`, at `url`, an image's own, which this process's images of it share.
pub(crate) fn of(url: &str, bucket: &Arc<Bucket>) -> Arc<Collecting> {
    let mut collecting = COLLECTING.lock().unwrap();
    collecting.retain(|_, collector| collector.strong_count() > 0);
    if let Some(collector) = collecting.get(url).and_then(Weak::upgrade) {
        return collector;
    }
    let pace = pace();
    let collector = Arc::new(Collecting {
        bucket: bucket.clone(),
        pace,
        state: Mutex::new(State { idle: Some(Collector::new(pace.options)), ..Default::default() }),
    });
    collecting.insert(url.to_owned(), Arc::downgrade(&collector));
    collector
}

/// An image's own bucket's collector in this process. Dropped, it waits for the pass it runs, if any.
pub(crate) struct Collecting {
    bucket: Arc<Bucket>,
    pace: Pace,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The collector between passes, which keeps what it read from one to the next.
    idle: Option<Collector>,
    /// The pass running, which gives the collector back, and what it did.
    running: Option<JoinHandle<(Collector, Report)>>,
    /// When the last pass started.
    started: Option<Instant>,
    /// How many passes started.
    passes: u64,
    /// What the passes that ended did, together.
    collected: Report,
}

impl State {
    /// Takes the collector back from the pass that ran, if it ended: a pass's thread that
    /// panicked gives back none, and a new collector of `options`, which remembers nothing, takes
    /// its place.
    fn reap(&mut self, options: Options) {
        let Some(running) = self.running.take_if(|running| running.is_finished()) else { return };
        self.ended(running, options);
    }

    /// Takes the collector back from `running`, waiting for it to end, and what it did.
    fn ended(&mut self, running: JoinHandle<(Collector, Report)>, options: Options) {
        let (collector, report) =
            running.join().unwrap_or_else(|_| (Collector::new(options), Report::default()));
        self.idle = Some(collector);
        self.collected += report;
    }
}

impl Collecting {
    /// Starts a pass, after an image's commit, if none runs and none started within its pace's
    /// [`Pace::every`].
    pub(crate) fn after_commit(&self) {
        let mut state = self.state.lock().unwrap();
        state.reap(self.pace.options);
        let due = state.started.is_none_or(|started| started.elapsed() >= self.pace.every);
        let Some(mut collector) = state.idle.take().filter(|_| due) else { return };
        let bucket = self.bucket.clone();
        let pass = std::thread::Builder::new().name("fxdax collect".into()).spawn(move || {
            let report = pass(&mut collector, &bucket);
            (collector, report)
        });
        match pass {
            Ok(running) => {
                state.running = Some(running);
                state.started = Some(Instant::now());
                state.passes += 1;
            }
            Err(err) => {
                log::warn!("fxdax: starting a pass of an image's collector: {err}");
                state.idle = Some(Collector::new(self.pace.options));
            }
        }
    }

    /// How many passes started.
    #[cfg(test)]
    pub(crate) fn passes(&self) -> u64 {
        self.state.lock().unwrap().passes
    }

    /// What the passes started did, together, the one running waited for.
    #[cfg(test)]
    pub(crate) fn collected(&self) -> Report {
        let mut state = self.state.lock().unwrap();
        if let Some(running) = state.running.take() {
            state.ended(running, self.pace.options);
        }
        state.collected
    }

    /// Waits for the pass running, if any, then runs one, its compacted objects and the moves no
    /// state names going at once, with no grace period: as tests look at what a pass reclaims.
    #[cfg(test)]
    pub(crate) fn pass_now(&self) -> Report {
        let mut state = self.state.lock().unwrap();
        if let Some(running) = state.running.take() {
            state.ended(running, self.pace.options);
        }
        let mut collector = Collector::new(Options { grace: 0, ..self.pace.options });
        let collected =
            extent_store::upload::block_on(collector.collect(&*self.bucket, rand::random()));
        collected.unwrap().unwrap()
    }

    /// Runs passes as [`Collecting::pass_now`] does until two in a row delete and compact nothing,
    /// every image of the bucket closed, and says what they left that they should have collected
    /// ([`extent_remote::testing::uncollected`]): the space freed comes back (TASK-390). Two, as a
    /// pass over a dead cell retires the references that kept what its parents' cells reclaim at
    /// their next (TASK-395.4); and a pass retiring them again deletes nothing.
    #[cfg(test)]
    pub(crate) fn collected_alone(&self) -> Vec<String> {
        const PASSES: usize = 12;
        let mut quiet = 0;
        for _ in 0..PASSES {
            let report = self.pass_now();
            quiet = if report.deleted == 0 && report.compacted == 0 { quiet + 1 } else { 0 };
            if quiet == 2 {
                break;
            }
        }
        assert_eq!(quiet, 2, "{PASSES} passes alone, which kept doing something");
        let left =
            extent_remote::testing::uncollected(&*self.bucket, self.pace.options.compact_below);
        extent_store::upload::block_on(left).unwrap()
    }
}

impl Drop for Collecting {
    fn drop(&mut self) {
        let running = self.state.get_mut().unwrap().running.take();
        if let Some(running) = running {
            let _ = running.join();
        }
    }
}

/// A pass of `collector` over `bucket`, as a holder no other pass is: what it did, logged and
/// returned, or why it failed, logged. Nothing it could not read it deleted: the next pass tries
/// again.
fn pass(collector: &mut Collector, bucket: &Bucket) -> Report {
    // Where a test holds a pass up, to drop the images it collects for meanwhile (TASK-291.3).
    fail::fail_point!("collect::pass", |_| Report::default());
    let failed = match extent_store::upload::block_on(collector.collect(bucket, rand::random())) {
        Ok(Ok(report)) => {
            let Report { deleted, compacted, retired, deleted_bytes, compacted_bytes, skipped } =
                report;
            log::info!(
                "fxdax: an image's bucket collected: deleted {deleted} ({deleted_bytes} bytes), \
                 compacted {compacted} ({compacted_bytes} bytes), retired {retired} references, \
                 {skipped} cells unchanged"
            );
            return report;
        }
        Ok(Err(err)) => err.to_string(),
        Err(err) => err.to_string(),
    };
    log::warn!("fxdax: collecting an image's bucket: {failed}");
    Report::default()
}
