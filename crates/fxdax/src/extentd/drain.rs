// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the daemon's drain, what SIGTERM has it do before it exits (TASK-141.4): its stores
//! closed, so durable, and their commits uploaded, as a host upgrading its daemon needs before it
//! starts the new one (backlog doc-11, `docs/upgrading-a-host.md`). Before, SIGTERM had the health
//! service say NOT_SERVING and the daemon exit, its stores as a crash leaves them: their last
//! ordered commits not durable, and none of what was durable since the last upload uploaded.
//!
//! [`drain`] does, in order:
//!
//! 1. The health service says NOT_SERVING ([`super::stop_serving`]).
//! 2. The daemon drains: its VM sockets take no more connections, the daemon's own and every
//!    tenant's, their listeners shut down; the control API refuses calls (`UNAVAILABLE`); and
//!    every VM process's connection is closed, its VM told its daemon went, as at any daemon's
//!    exit (`ExtentdLost`), its requests received served, its commits made durable and uploaded,
//!    the open segment's too, which a store closed leaves for the next to open it, and its store
//!    closed, as a connection whose VM process goes closes it.
//! 3. It waits for the connections to be gone and the control API's calls in flight to be done
//!    ([`Work`]); closes the points it keeps open for forks; and waits for every store's uploader
//!    to have uploaded what its store made durable, and stopped, as each does as its store closes
//!    ([`crate::remote::uploaders`]).
//!
//! All of it within a bound, `FXDAX_EXTENTD_DRAIN` seconds ([`bound`]): past it, what is left is
//! logged, and the daemon exits anyway, failing; its stores' commits that weren't durable are
//! lost, as at a crash, and those durable that weren't uploaded are uploaded by the next daemon to
//! open their stores.

use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::watch;

/// Whether the daemon drains: from [`drain`] on, for good.
static DRAINING: LazyLock<watch::Sender<bool>> = LazyLock::new(|| watch::channel(false).0);

/// Whether the daemon drains.
pub(super) fn draining() -> bool {
    *DRAINING.borrow()
}

/// Returns once the daemon drains: at once if it does.
pub(super) async fn started() {
    let mut draining = DRAINING.subscribe();
    // The sender lives as long as the process.
    let _ = draining.wait_for(|draining| *draining).await;
}

/// How long a drain takes at most when `FXDAX_EXTENTD_DRAIN` names none: long enough for a few
/// stores' last uploads, short of an orchestrator's patience before it kills.
pub const DEFAULT_BOUND: Duration = Duration::from_secs(60);

/// How long a drain takes at most: `FXDAX_EXTENTD_DRAIN` seconds, or [`DEFAULT_BOUND`].
pub fn bound() -> Result<Duration, String> {
    match std::env::var("FXDAX_EXTENTD_DRAIN") {
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_BOUND),
        Ok(secs) => secs
            .parse()
            .map(Duration::from_secs)
            .map_err(|_| format!("FXDAX_EXTENTD_DRAIN {secs:?} is no number of seconds")),
        Err(err) => Err(format!("FXDAX_EXTENTD_DRAIN: {err}")),
    }
}

/// What a drain left, past its bound: nothing of it if it drained.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Left {
    /// VM processes' connections not yet gone, their stores open.
    pub connections: usize,
    /// The control API's calls in flight.
    pub calls: usize,
    /// Stores whose uploaders have not stopped: open, or closing, uploading still.
    pub uploaders: usize,
}

impl std::fmt::Display for Left {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { connections, calls, uploaders } = self;
        write!(
            f,
            "{connections} VM processes' connections, {calls} control calls and {uploaders} \
             stores' uploaders left"
        )
    }
}

/// Drains the daemon, as the module says, within `within`: `Err` with what is left past it.
/// It blocks.
pub fn drain(within: Duration) -> Result<(), Left> {
    super::stop_serving();
    DRAINING.send_replace(true);
    super::server::stop_accepting();
    let drained = super::server::IO.block_on(async {
        tokio::time::timeout(within, async {
            super::server::CONNECTIONS.done().await;
            super::control::CALLS.done().await;
            super::stores::close_points().await;
            crate::remote::uploaders().wait_for(|uploading| *uploading == 0).await.map(drop)
        })
        .await
    });
    match drained {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(Left {
            connections: super::server::CONNECTIONS.count(),
            calls: super::control::CALLS.count(),
            uploaders: *crate::remote::uploaders().borrow(),
        }),
    }
}

/// fxdax: work of a kind that a drain waits for, counted as it begins and ends ([`Work::begin`]):
/// VM processes' connections, the control API's calls.
pub(super) struct Work(LazyLock<watch::Sender<usize>>);

impl Work {
    pub const fn new() -> Self {
        Self(LazyLock::new(|| watch::channel(0).0))
    }

    /// Begins work of the kind, until what it returns is dropped; or none, `None`, once the daemon
    /// drains: counted first, and given up if then the daemon drains, so that a drain that
    /// finds none counted finds none begun after, whatever their order.
    pub fn begin(&'static self) -> Option<Doing> {
        self.0.send_modify(|count| *count += 1);
        let doing = Doing(self);
        (!draining()).then_some(doing)
    }

    /// How much of it is under way.
    pub fn count(&self) -> usize {
        *self.0.borrow()
    }

    /// Returns once none of it is under way.
    async fn done(&self) {
        let mut count = self.0.subscribe();
        // The sender lives as long as the process.
        let _ = count.wait_for(|count| *count == 0).await;
    }
}

/// Work under way ([`Work::begin`]), until dropped.
pub(super) struct Doing(&'static Work);

impl Drop for Doing {
    fn drop(&mut self) {
        self.0.0.send_modify(|count| *count -= 1);
    }
}
