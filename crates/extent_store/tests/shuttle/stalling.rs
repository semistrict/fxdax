// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A store's directory whose file system stalls a call, as on a slow disk, for as long as a
//! scenario says: so that Shuttle, whose waits that can't end are deadlocks it reports, shows what
//! goes on meanwhile.

use extent_store::{Dir, MemDir, MemFile};
use shuttle::sync::{Condvar, Mutex};
use std::io;
use std::sync::Arc;

/// The operations of a file system a scenario stalls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Op {
    Create,
    SyncDir,
}

#[derive(Default)]
struct Stall {
    /// The operation and path whose next call stalls.
    at: Option<(Op, String)>,
    /// Whether a call is stalled.
    stalled: bool,
    /// What the stalled call gives, once the scenario lets it go.
    outcome: Option<io::Result<()>>,
}

/// A [`MemDir`] whose next call of an operation on a path, once armed, stalls until the scenario
/// lets it go, saying whether it fails: on Shuttle's condition variable, so that Shuttle schedules
/// the other threads meanwhile.
#[derive(Clone)]
pub(crate) struct Stalling {
    dir: MemDir,
    stall: Arc<(Mutex<Stall>, Condvar)>,
}

impl Stalling {
    pub(crate) fn new() -> Self {
        Self { dir: MemDir::new(), stall: Arc::new((Mutex::new(Stall::default()), Condvar::new())) }
    }

    /// Has the next call of `op` on `path` stall.
    pub(crate) fn arm(&self, op: Op, path: &str) {
        self.stall.0.lock().unwrap().at = Some((op, path.to_owned()));
    }

    /// Waits until the call armed is stalled.
    pub(crate) fn wait_until_stalled(&self) {
        let (stall, changed) = &*self.stall;
        let mut stall = stall.lock().unwrap();
        while !stall.stalled {
            stall = changed.wait(stall).unwrap();
        }
    }

    /// Lets the call stalled go on, giving `outcome`: if an error, it fails without doing anything.
    pub(crate) fn let_go(&self, outcome: io::Result<()>) {
        let (stall, changed) = &*self.stall;
        stall.lock().unwrap().outcome = Some(outcome);
        changed.notify_all();
    }

    /// Stalls a call of `op` on `path`, if armed for it, until the scenario lets it go, and gives
    /// what the scenario said.
    fn pass(&self, op: Op, path: &str) -> io::Result<()> {
        let (stall, changed) = &*self.stall;
        let mut stall = stall.lock().unwrap();
        if stall.at.as_ref().is_none_or(|(at, at_path)| (*at, at_path.as_str()) != (op, path)) {
            return Ok(());
        }
        stall.at = None;
        stall.stalled = true;
        changed.notify_all();
        while stall.outcome.is_none() {
            stall = changed.wait(stall).unwrap();
        }
        stall.stalled = false;
        stall.outcome.take().unwrap()
    }
}

impl Dir for Stalling {
    type File = MemFile;

    fn create(&self, path: &str, data: bool) -> io::Result<MemFile> {
        self.pass(Op::Create, path)?;
        self.dir.create(path, data)
    }

    fn open(&self, path: &str, data: bool) -> io::Result<MemFile> {
        self.dir.open(path, data)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        self.dir.remove(path)
    }

    fn create_dir(&self, path: &str) -> io::Result<()> {
        self.dir.create_dir(path)
    }

    fn list(&self, path: &str) -> io::Result<Vec<String>> {
        self.dir.list(path)
    }

    fn sync_dir(&self, path: &str) -> io::Result<()> {
        self.pass(Op::SyncDir, path)?;
        self.dir.sync_dir(path)
    }

    fn barrier(&self) -> io::Result<()> {
        self.dir.barrier()
    }
}
