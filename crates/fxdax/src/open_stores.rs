// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The stores the extent daemon's connections hold (`crate::extentd`), by their paths: a store has
//! one holder at a time, a connection that has it open, or the daemon itself while it forks,
//! syncs or resizes a store no connection has. A connection whose VM process is gone, still
//! holding its store, lets it go to the next that asks, as a VM restored from a snapshot at once
//! opens its VM's store (TASK-39.1).
//!
//! Generic over its holders, so that its tests may run it with holders of their own.

use crate::Errno;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// What holds a store: a connection.
pub(crate) trait Holder: Send + Sync {
    /// Whether it is going: its VM process gone, it lets its store go to the next that asks.
    fn going(&self) -> bool;
}

/// The stores held, by the paths that name them (`server::store_key`). A holder of none is the
/// daemon's own, which goes once its work is done.
pub(crate) struct Paths<H> {
    held: Mutex<HashMap<PathBuf, Weak<H>>>,
    /// Notified as a store's holder lets it go.
    released: Condvar,
}

impl<H: Holder> Paths<H> {
    pub fn new() -> Self {
        Self { held: Mutex::new(HashMap::new()), released: Condvar::new() }
    }

    /// Has `holder` hold the store `key` names: once its holder, if it has one, lets it go, if
    /// that holder is going, or is the daemon's, waiting at most `wait` (for ever, if none);
    /// unless a holder that isn't going has it, which fails with `EBUSY`.
    pub fn hold(&self, key: &Path, holder: Weak<H>, wait: Option<Duration>) -> Result<(), Errno> {
        let deadline = wait.map(|wait| Instant::now() + wait);
        let mut held = self.held.lock().unwrap();
        while let Some(other) = held.get(key) {
            let going = other.upgrade().is_none_or(|other| other.going());
            if !going {
                return Err(Errno::EBUSY);
            }
            held = match deadline {
                None => self.released.wait(held).unwrap(),
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(Errno::EBUSY);
                    }
                    self.released.wait_timeout(held, left).unwrap().0
                }
            };
        }
        held.insert(key.to_owned(), holder);
        Ok(())
    }

    /// Lets the store `key` names go, which the next waiting to hold it then may.
    pub fn release(&self, key: &Path) {
        self.held.lock().unwrap().remove(key);
        self.released.notify_all();
    }

    /// The connection holding the store `key` names, if one does.
    pub fn holder(&self, key: &Path) -> Option<Arc<H>> {
        self.held.lock().unwrap().get(key).and_then(Weak::upgrade)
    }

    /// The connection holding the store `key` names, if one does; or, if none does, the daemon
    /// holds it, until [`release`](Self::release). `EBUSY` if a holder of it is going, or the
    /// daemon's.
    pub fn holder_or_hold(&self, key: &Path) -> Result<Option<Arc<H>>, Errno> {
        let mut held = self.held.lock().unwrap();
        match held.get(key) {
            Some(holder) => Ok(Some(holder.upgrade().ok_or(Errno::EBUSY)?)),
            None => {
                held.insert(key.to_owned(), Weak::new());
                Ok(None)
            }
        }
    }
}
