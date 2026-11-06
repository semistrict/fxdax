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

/// How long at least goes from one pass's start to the next's.
const EVERY: Duration = Duration::from_secs(60);

/// How passes collect: as `fxdax-gc` does by default.
const OPTIONS: Options = Options { compact_below: 0.5, grace: 60_000 };

/// The collectors of the buckets this process has images of open, by their URLs.
static COLLECTING: Mutex<BTreeMap<String, Weak<Collecting>>> = Mutex::new(BTreeMap::new());

/// The collector of `bucket`, at `url`, an image's own, which this process's images of it share.
pub(crate) fn of(url: &str, bucket: &Arc<Bucket>) -> Arc<Collecting> {
    let mut collecting = COLLECTING.lock().unwrap();
    collecting.retain(|_, collector| collector.strong_count() > 0);
    if let Some(collector) = collecting.get(url).and_then(Weak::upgrade) {
        return collector;
    }
    let collector = Arc::new(Collecting {
        bucket: bucket.clone(),
        state: Mutex::new(State { idle: Some(Collector::new(OPTIONS)), ..Default::default() }),
    });
    collecting.insert(url.to_owned(), Arc::downgrade(&collector));
    collector
}

/// An image's own bucket's collector in this process. Dropped, it waits for the pass it runs, if any.
pub(crate) struct Collecting {
    bucket: Arc<Bucket>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The collector between passes, which keeps what it read from one to the next.
    idle: Option<Collector>,
    /// The pass running, which gives the collector back.
    running: Option<JoinHandle<Collector>>,
    /// When the last pass started.
    started: Option<Instant>,
    /// How many passes started.
    passes: u64,
}

impl State {
    /// Takes the collector back from the pass that ran, if it ended: a pass's thread that
    /// panicked gives back none, and a new collector, which remembers nothing, takes its place.
    fn reap(&mut self) {
        let Some(running) = self.running.take_if(|running| running.is_finished()) else { return };
        self.idle = Some(running.join().unwrap_or_else(|_| Collector::new(OPTIONS)));
    }
}

impl Collecting {
    /// Starts a pass, after an image's commit, if none runs and none started within [`EVERY`].
    pub(crate) fn after_commit(&self) {
        let mut state = self.state.lock().unwrap();
        state.reap();
        let due = state.started.is_none_or(|started| started.elapsed() >= EVERY);
        let Some(mut collector) = state.idle.take().filter(|_| due) else { return };
        let bucket = self.bucket.clone();
        let pass = std::thread::Builder::new().name("fxdax collect".into()).spawn(move || {
            pass(&mut collector, &bucket);
            collector
        });
        match pass {
            Ok(running) => {
                state.running = Some(running);
                state.started = Some(Instant::now());
                state.passes += 1;
            }
            Err(err) => {
                log::warn!("fxdax: starting a pass of an image's collector: {err}");
                state.idle = Some(Collector::new(OPTIONS));
            }
        }
    }

    /// How many passes started.
    #[cfg(test)]
    pub(crate) fn passes(&self) -> u64 {
        self.state.lock().unwrap().passes
    }

    /// Waits for the pass running, if any, then runs one, its compacted objects and the moves no
    /// state names going at once, with no grace period: as tests look at what a pass reclaims.
    #[cfg(test)]
    pub(crate) fn pass_now(&self) -> Report {
        let mut state = self.state.lock().unwrap();
        if let Some(running) = state.running.take() {
            state.idle = Some(running.join().unwrap());
        }
        let mut collector = Collector::new(Options { grace: 0, ..OPTIONS });
        let collected = crate::remote::block_on(collector.collect(&*self.bucket, rand::random()));
        collected.unwrap().unwrap()
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

/// A pass of `collector` over `bucket`, as a holder no other pass is: what it did, or why it
/// failed, logged. Nothing it could not read it deleted: the next pass tries again.
fn pass(collector: &mut Collector, bucket: &Bucket) {
    match crate::remote::block_on(collector.collect(bucket, rand::random())) {
        Ok(Ok(Report { deleted, compacted })) => {
            log::info!(
                "fxdax: an image's bucket collected: deleted {deleted}, compacted {compacted}"
            )
        }
        Ok(Err(err)) => log::warn!("fxdax: collecting an image's bucket: {err}"),
        Err(err) => log::warn!("fxdax: collecting an image's bucket: {err}"),
    }
}
