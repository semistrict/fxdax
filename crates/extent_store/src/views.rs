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

use crate::sync::{Condvar, Mutex, rules};
use anyhow::{Error, bail};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use storage_device::ExtentId;

/// The error of a view cut off: it fell too far behind, and keeps nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the view fell too far behind the store's writer, and was cut off")]
pub struct CutOff;

/// The context of a view's `NoExtent` for an extent it let go of: freed by a commit before the
/// one it holds since, not pinned, and kept for another (the uploader). A follower reads what its
/// commits hold, so its read of one is a bug; one of an extent written after its commit is not:
/// Fxfs's journal replay reads on to the first extent its commit doesn't hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the view let go of {0:?}, freed before the commit it holds since")]
pub struct NotHeld(pub ExtentId);

/// A view of a branch as of a commit (see the module docs). Dropping it lets go of what it keeps.
///
/// It holds what its commit holds; and, advanced keeping ([`View::advance_keeping`]), what the
/// commits since the one it advanced from held, until it lets go of them ([`View::let_go`]): what
/// a follower applying the commits between reads, and its readers may still be reading.
pub struct View {
    id: u64,
    epoch: u64,
    /// The commit it holds what was held since: `epoch`, but while it keeps an earlier one.
    since: u64,
    root: Arc<[u8]>,
    views: Arc<Views>,
}

impl View {
    /// The epoch of the commit it is as of.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The epoch of the commit it holds what was held since.
    pub fn since(&self) -> u64 {
        self.since
    }

    /// The branch's root as of its commit.
    pub fn root(&self) -> &[u8] {
        &self.root
    }

    /// Moves it to the latest commit. It no longer keeps what was freed up to that commit, which
    /// goes with the commit after, but for what it pins.
    pub fn advance(&mut self) -> Result<(), Error> {
        self.advance_keeping()?;
        self.let_go(self.epoch);
        Ok(())
    }

    /// Moves it to the latest commit, holding still what the commits since the one it was as of
    /// held, until it lets go of them.
    pub fn advance_keeping(&mut self) -> Result<(), Error> {
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

    /// Lets go of what the commits before the commit of `epoch` held, `epoch` at most its own:
    /// the frees up to that commit go with the commit after, but for what it pins. A let-go of an
    /// epoch before the one it holds since changes nothing: it may come after the view's next
    /// advance, as a device's requests, served at once, may.
    pub fn let_go(&mut self, epoch: u64) {
        let since = epoch.min(self.epoch).max(self.since);
        let mut views = self.views.inner.lock().unwrap();
        views.live.get_mut(&self.id).unwrap().since = since;
        self.since = since;
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

    /// Another view as of its commit, which keeps what it keeps of the frees, but not its pins.
    pub fn duplicate(&self) -> Result<View, Error> {
        let mut views = self.views.inner.lock().unwrap();
        if views.live[&self.id].cut {
            bail!(CutOff);
        }
        let id = views.next_id;
        views.next_id += 1;
        let follower = Follower {
            epoch: self.epoch,
            since: self.since,
            pins: HashSet::new(),
            cut: false,
            stale_since: views.live[&self.id].stale_since,
        };
        views.live.insert(id, follower);
        let (epoch, since, root, views) =
            (self.epoch, self.since, self.root.clone(), self.views.clone());
        Ok(View { id, epoch, since, root, views })
    }

    /// Lets go of `extent`, pinned, which goes with the next commit if nothing else keeps it.
    pub fn unpin(&self, extent: ExtentId) {
        let mut views = self.views.inner.lock().unwrap();
        views.live.get_mut(&self.id).unwrap().pins.remove(&extent);
    }

    /// The extents it pins.
    pub fn pinned(&self) -> Vec<ExtentId> {
        let views = self.views.inner.lock().unwrap();
        views.live[&self.id].pins.iter().copied().collect()
    }

    /// Whether it pins `extent`.
    pub fn pins(&self, extent: ExtentId) -> bool {
        self.views.inner.lock().unwrap().live[&self.id].pins.contains(&extent)
    }

    /// Waits at most `timeout` for a commit after its own, returning the latest commit's epoch,
    /// if one came: what tells a follower to advance. Fails with [`CutOff`] if it was cut off.
    pub fn newer(&self, timeout: Duration) -> Result<Option<u64>, Error> {
        let newer = self.views.newer(self.epoch, timeout);
        self.views.check(self)?;
        Ok(newer)
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
    /// Notified as each commit is published.
    published: Condvar,
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
    /// The commit it holds what was held since: what it keeps the frees after.
    since: u64,
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
    /// The epoch of the oldest live view, if any.
    pub fn oldest(&self) -> Option<u64> {
        self.oldest
    }

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
        Arc::new(Self { inner: Mutex::new(inner), published: Condvar::new() })
    }

    /// A view as of the latest commit.
    pub fn open(self: &Arc<Self>) -> View {
        let mut views = self.inner.lock().unwrap();
        let (id, epoch, root) = (views.next_id, views.epoch, views.root.clone());
        views.next_id += 1;
        let follower =
            Follower { epoch, since: epoch, pins: HashSet::new(), cut: false, stale_since: None };
        views.live.insert(id, follower);
        View { id, epoch, since: epoch, root, views: self.clone() }
    }

    /// Waits at most `timeout` for a commit after `epoch`'s, returning the latest commit's epoch,
    /// if one came.
    pub fn newer(&self, epoch: u64, timeout: Duration) -> Option<u64> {
        let views = self.inner.lock().unwrap();
        let older = |views: &mut Inner| views.epoch <= epoch;
        let (views, _) = self.published.wait_timeout_while(views, timeout, older).unwrap();
        (views.epoch > epoch).then_some(views.epoch)
    }

    /// Cuts every view off: they keep nothing, and fail with [`CutOff`] from now on.
    pub fn cut_all(&self) {
        let mut views = self.inner.lock().unwrap();
        views.live.values_mut().for_each(|follower| follower.cut = true);
        self.published.notify_all();
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
            oldest.min_by_key(|follower| follower.since).unwrap().cut = true;
            keeping = Self::keeping_of(&views);
        }
        self.published.notify_all();
        keeping
    }

    fn keeping_of(views: &Inner) -> Keeping {
        let live = || views.live.values().filter(|follower| !follower.cut);
        Keeping {
            oldest: live().map(|follower| follower.since).min(),
            pins: live().flat_map(|follower| follower.pins.iter().copied()).collect(),
        }
    }
}
