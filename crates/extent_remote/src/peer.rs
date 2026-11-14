// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the other hosts sharing a host's bucket, which its fetcher asks for parts before the
//! bucket (TASK-406.2): each part of an object has an owner among them, which keeps it once it
//! fetched it, so that a part many hosts read, as the forks of a popular base booting on many cold
//! hosts do, comes from the bucket about once for them all, not once for each.
//!
//! What a peer gives is checked as what the bucket gives is, against the CRC the asker's own tail
//! gives the part ([`Stored::check`]), and a part that fails it, as any failure of the peer's, is
//! fetched from the bucket: a peer is trusted with no more than the bucket's latency. Only parts
//! are asked of peers, never tails: a tail is what checks a part, and a stale one, of an object
//! deleted and created again at its key, would have a reader fail the part twice, as corrupt
//! ([`crate::fetch`]), rather than read the object anew from the bucket.

use crate::layout::{self, Kind};
use extent_frame::Stored;
use futures::future::BoxFuture;
use std::sync::Arc;

/// The hosts sharing a bucket with this one, each the owner of some of its objects' parts.
pub trait Peers: Send + Sync {
    /// The host that owns part `part` of the object at `key`, if another host does, and is asked
    /// now: this one fetches it from the bucket otherwise.
    fn owner(&self, key: &str, part: u64) -> Option<Arc<dyn Peer>>;
}

/// A host its peers ask for parts.
pub trait Peer: Send + Sync {
    /// The stored bytes of part `part` of the object at `key`, stored as `stored` says: none if it
    /// has no such part, or failed to give it, in time.
    fn part(&self, key: &str, part: u64, stored: Stored) -> BoxFuture<'static, Option<Vec<u8>>>;

    /// A part it gave failed the CRC the asker's tail gives it: it is no peer to ask for a while.
    fn failed(&self);
}

/// Whether the object at `key` is one whose parts peers give each other: one of extents' data, or
/// of their places, framed and checked by its tail, which a fetcher reads by parts. Never a branch's
/// record, a lease, a cell's object or a reference, which are no frames and change in place.
pub fn served(key: &str) -> bool {
    matches!(
        layout::kind(key),
        Kind::Segment { .. } | Kind::Extent { .. } | Kind::Checkpoint { .. } | Kind::Moves { .. }
    )
}
