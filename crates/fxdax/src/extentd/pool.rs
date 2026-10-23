// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The threads that serve VM processes' requests, which every connection shares (TASK-34): as
//! many as requests run at once, up to a bound the host's processors set, not a set of threads per
//! connection. A thread idle for a while goes. Requests block: on the store's disk, a page-in, a
//! guest's acknowledgement, an upload a durable commit waits for. None waits for a request queued
//! behind it, so a full pool delays requests, and deadlocks none. fxdax: and pools of their own
//! for the work every store's flushes and cleaning do behind it (`server::flush_behind`,
//! TASK-92.49), as many threads as do it at once, up to a bound, not one each.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

/// A request to run, and what follows it.
pub(super) type Job = Box<dyn FnOnce() + Send>;

/// How long a thread waits for work before it goes.
const IDLE: Duration = Duration::from_secs(10);

/// The pool. See the module's docs.
pub(super) struct Pool {
    state: Mutex<State>,
    work: Condvar,
    /// The most threads it runs.
    max: usize,
    /// Its threads' name.
    name: &'static str,
}

#[derive(Default)]
struct State {
    jobs: VecDeque<Job>,
    threads: usize,
    /// Those waiting for work.
    idle: usize,
}

impl Pool {
    /// A pool of up to `max` threads, named `name`.
    pub(super) fn new(name: &'static str, max: usize) -> Self {
        Self { state: Mutex::default(), work: Condvar::new(), max, name }
    }

    /// The process's, which requests run on, of up to 16 threads a processor, and 64 at least.
    pub(super) fn get() -> &'static Pool {
        static POOL: OnceLock<Pool> = OnceLock::new();
        POOL.get_or_init(|| {
            let cpus = std::thread::available_parallelism().map_or(4, |cpus| cpus.get());
            Pool::new("extentd request", (16 * cpus).max(64))
        })
    }

    /// Runs `job` on a thread of the pool: an idle one, or a new one, unless it runs as many as it
    /// may already, when the job waits for one.
    pub(super) fn run(&'static self, job: Job) {
        let mut state = self.state.lock().unwrap();
        state.jobs.push_back(job);
        if state.idle >= state.jobs.len() {
            self.work.notify_one();
            return;
        }
        if state.threads < self.max {
            state.threads += 1;
            let spawned =
                std::thread::Builder::new().name(self.name.into()).spawn(move || self.work_on());
            if let Err(err) = spawned {
                state.threads -= 1;
                log::error!("extentd: starting a thread, {}: {err}", self.name);
            }
        }
    }

    /// fxdax: runs `work` on a thread of the pool, as [`Pool::run`] runs a job, and returns what it
    /// returns, once it has, holding no thread while it waits: none if it panicked.
    pub(super) async fn call<T: Send + 'static>(
        &'static self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (done, returned) = tokio::sync::oneshot::channel();
        self.run(Box::new(move || {
            // Unheard if its caller gave up on it.
            let _ = done.send(work());
        }));
        returned.await.ok()
    }

    /// A thread's work: the jobs, until it waits for one longer than [`IDLE`]. A job that panics
    /// fails alone: the thread goes on to the next, which a thread dead and counted would leave to
    /// none, every connection's requests waiting for good once all were (TASK-79.1).
    fn work_on(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(job) = state.jobs.pop_front() {
                drop(state);
                if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)) {
                    let what = (panic.downcast_ref::<&str>().copied())
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
                    log::error!("extentd: a job panicked, {}: {}", self.name, what.unwrap_or("?"));
                }
                state = self.state.lock().unwrap();
                continue;
            }
            state.idle += 1;
            let (woken, waited) = self.work.wait_timeout(state, IDLE).unwrap();
            state = woken;
            state.idle -= 1;
            if waited.timed_out() && state.jobs.is_empty() {
                state.threads -= 1;
                return;
            }
        }
    }

    /// How many threads it runs now.
    #[cfg(test)]
    pub(super) fn threads(&self) -> usize {
        self.state.lock().unwrap().threads
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A pool of its own, of up to `max` threads.
    fn pool(max: usize) -> &'static Pool {
        Box::leak(Box::new(Pool::new("extentd test", max)))
    }

    /// A job that panics leaves the pool its thread, which runs the next job (TASK-79.1). A pool
    /// whose thread died with it, counted still, runs none: this waits a minute for it, then fails.
    #[test]
    fn jobs_after_a_panicking_one_run() {
        let pool = pool(1);
        pool.run(Box::new(|| panic!("a request panicked, as a test's does")));
        let (ran, done) = std::sync::mpsc::channel();
        pool.run(Box::new(move || ran.send(()).unwrap()));
        let waited = done.recv_timeout(std::time::Duration::from_secs(60));
        assert_eq!(waited, Ok(()), "the job after a panicking one never ran");
        assert_eq!(pool.threads(), 1);
    }

    /// Jobs run at once up to the pool's bound, which no more threads than that serve.
    #[test]
    fn runs_jobs_on_threads_up_to_its_bound() {
        let pool = pool(4);
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = Arc::new(Mutex::new(released));
        let (running, ran) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        for _ in 0..8 {
            let (released, running, ran) = (released.clone(), running.clone(), ran.clone());
            pool.run(Box::new(move || {
                running.fetch_add(1, Ordering::SeqCst);
                released.lock().unwrap().recv().unwrap();
                running.fetch_sub(1, Ordering::SeqCst);
                ran.fetch_add(1, Ordering::SeqCst);
            }));
        }
        while running.load(Ordering::SeqCst) < 4 {
            std::thread::yield_now();
        }
        assert_eq!((pool.threads(), running.load(Ordering::SeqCst)), (4, 4));
        for _ in 0..8 {
            release.send(()).unwrap();
        }
        while ran.load(Ordering::SeqCst) < 8 {
            std::thread::yield_now();
        }
        assert_eq!(pool.threads(), 4);
    }

    /// fxdax: work called on a pool returns what it returns, on a thread of the pool's; or, if it
    /// panics, nothing (TASK-92.49).
    #[test]
    fn calls_return_what_their_work_does() {
        let pool = pool(1);
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let named =
            runtime.block_on(pool.call(|| std::thread::current().name().map(str::to_owned)));
        assert_eq!(named, Some(Some("extentd test".to_owned())));
        let panicked = runtime.block_on(pool.call(|| panic!("work panicked, as a test's does")));
        assert_eq!(panicked, None::<()>);
    }
}
