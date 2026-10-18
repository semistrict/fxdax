// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: views of a branch as of a commit, through which followers read the store while
//! its writer goes on (backlog doc-8, Store views; TASK-42.2).
//!
//! A commit seals every extent written before it, and an extent's ID is never used again, so the
//! branch as of commit N is the extents it held then, each as it is still, wherever cleaning has
//! moved its data since, and its root then. A view is no copy of the extent table: it is an epoch
//! and a root, and reads through the store. What it needs kept is the extents freed since N: a
//! free of an extent sealed before is kept while a view older than the free lives, or a view pins
//! it ([`View::pin`], a follower's graveyard), and its data counts as used.
//!
//! Opening, advancing and dropping a view take this module's lock alone, never the store's state,
//! so they never wait for a commit. Each commit publishes itself here ([`Views::publish`]),
//! holding the state, and decides in the same critical section which frees are kept: a view that
//! opened before is as of an earlier commit, and counted; one that opens after is as of this one,
//! and needs none of its frees.
//!
//! A view that falls too far behind, stale for `Options::view_lag` or longer, or keeping more
//! than `Options::view_kept_bytes` of frees as the oldest view, is cut off: it keeps nothing, and
//! its reads, advances and pins fail with [`CutOff`] (doc-8, decision 7).

use crate::sync::{Mutex, rules};
use anyhow::{Error, bail};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use storage_device::ExtentId;

/// The error of a view cut off: it fell too far behind, and keeps nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the view fell too far behind the store's writer, and was cut off")]
pub struct CutOff;

/// A view of a branch as of a commit (see the module docs). Dropping it lets go of what it keeps.
pub struct View {
    id: u64,
    epoch: u64,
    root: Arc<[u8]>,
    views: Arc<Views>,
}

impl View {
    /// The epoch of the commit it is as of.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The branch's root as of its commit.
    pub fn root(&self) -> &[u8] {
        &self.root
    }

    /// Moves it to the latest commit. It no longer keeps what was freed up to that commit, which
    /// goes with the commit after, but for what it pins.
    pub fn advance(&mut self) -> Result<(), Error> {
        let mut views = self.views.inner.lock().unwrap();
        let (epoch, root) = (views.epoch, views.root.clone());
        let follower = views.live.get_mut(&self.id).unwrap();
        if follower.cut {
            bail!(CutOff);
        }
        follower.epoch = epoch;
        follower.stale_since = None;
        (self.epoch, self.root) = (epoch, root);
        Ok(())
    }

    /// Keeps `extent`, which it holds, until [`unpin`](Self::unpin)ned or the view goes, whatever
    /// commit the view advances to: what a follower holds open that its writer deletes.
    pub fn pin(&self, extent: ExtentId) -> Result<(), Error> {
        let mut views = self.views.inner.lock().unwrap();
        let follower = views.live.get_mut(&self.id).unwrap();
        if follower.cut {
            bail!(CutOff);
        }
        follower.pins.insert(extent);
        Ok(())
    }

    /// Lets go of `extent`, pinned, which goes with the next commit if nothing else keeps it.
    pub fn unpin(&self, extent: ExtentId) {
        let mut views = self.views.inner.lock().unwrap();
        views.live.get_mut(&self.id).unwrap().pins.remove(&extent);
    }
}

impl Drop for View {
    fn drop(&mut self) {
        self.views.inner.lock().unwrap().live.remove(&self.id);
    }
}

impl std::fmt::Debug for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View").field("id", &self.id).field("epoch", &self.epoch).finish()
    }
}

/// A branch's views, shared with them.
pub(crate) struct Views {
    inner: Mutex<Inner>,
}

struct Inner {
    /// The latest commit, which a view opens as of: its epoch, and the root as of it.
    epoch: u64,
    root: Arc<[u8]>,
    next_id: u64,
    live: HashMap<u64, Follower>,
}

/// A live view's state.
struct Follower {
    epoch: u64,
    pins: HashSet<ExtentId>,
    cut: bool,
    /// Since when a commit came after its epoch.
    stale_since: Option<Instant>,
}

/// What the live views keep of the frees: those after the oldest's epoch, and those they pin.
pub(crate) struct Keeping {
    oldest: Option<u64>,
    pins: HashSet<ExtentId>,
}

impl Keeping {
    /// Whether they keep `extent`, freed by the commit `freed`.
    pub fn keeps(&self, extent: ExtentId, freed: u64) -> bool {
        (rules().keep_frees_for_views && self.oldest.is_some_and(|oldest| oldest < freed))
            || (rules().keep_pins && self.pins.contains(&extent))
    }
}

/// The limits past which a view is cut off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub lag: Option<Duration>,
    pub kept_bytes: Option<u64>,
}

impl Views {
    /// The views of a branch as of `epoch`, its root `root`.
    pub fn new(epoch: u64, root: &[u8]) -> Arc<Self> {
        let inner = Inner { epoch, root: root.into(), next_id: 0, live: HashMap::new() };
        Arc::new(Self { inner: Mutex::new(inner) })
    }

    /// A view as of the latest commit.
    pub fn open(self: &Arc<Self>) -> View {
        let mut views = self.inner.lock().unwrap();
        let (id, epoch, root) = (views.next_id, views.epoch, views.root.clone());
        views.next_id += 1;
        let follower = Follower { epoch, pins: HashSet::new(), cut: false, stale_since: None };
        views.live.insert(id, follower);
        View { id, epoch, root, views: self.clone() }
    }

    /// Fails with [`CutOff`] if `view` was cut off.
    pub fn check(&self, view: &View) -> Result<(), Error> {
        debug_assert!(std::ptr::eq(self, &*view.views), "a view of another branch");
        if self.inner.lock().unwrap().live[&view.id].cut {
            bail!(CutOff);
        }
        Ok(())
    }

    /// What the live views keep, as of now.
    pub fn keeping(&self) -> Keeping {
        Self::keeping_of(&self.inner.lock().unwrap())
    }

    /// Publishes commit `epoch`, whose root is `root`, which views open as of from now on; cuts
    /// off the views past `limits`, `kept_bytes` saying how many bytes of frees views would keep;
    /// and returns what the views left keep. The store holds its state, so that what is kept is
    /// decided in the same critical section as the commit is published.
    pub fn publish(
        &self,
        epoch: u64,
        root: &[u8],
        limits: Limits,
        kept_bytes: impl Fn(&Keeping) -> u64,
    ) -> Keeping {
        let mut views = self.inner.lock().unwrap();
        views.epoch = epoch;
        if *views.root != *root {
            views.root = root.into();
        }
        let now = Instant::now();
        for follower in views.live.values_mut().filter(|follower| !follower.cut) {
            if follower.epoch < epoch {
                let since = *follower.stale_since.get_or_insert(now);
                if limits.lag.is_some_and(|lag| now.duration_since(since) >= lag) {
                    follower.cut = true;
                }
            }
        }
        let mut keeping = Self::keeping_of(&views);
        while let Some(limit) = limits.kept_bytes
            && keeping.oldest.is_some()
            && kept_bytes(&keeping) > limit
        {
            let oldest = views.live.values_mut().filter(|follower| !follower.cut);
            oldest.min_by_key(|follower| follower.epoch).unwrap().cut = true;
            keeping = Self::keeping_of(&views);
        }
        keeping
    }

    fn keeping_of(views: &Inner) -> Keeping {
        let live = || views.live.values().filter(|follower| !follower.cut);
        Keeping {
            oldest: live().map(|follower| follower.epoch).min(),
            pins: live().flat_map(|follower| follower.pins.iter().copied()).collect(),
        }
    }
}
