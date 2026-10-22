// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The collector's moves (doc-7, Layout): where the live extents of an object it compacted went,
//! which a reader that finds the object gone follows, a writer records in its next checkpoint, and
//! the collector keeps while a branch's way to its extents goes through them.

use crate::bucket::Bucket;
use crate::layout::{self, Location, Moves};
use crate::{Error, object};
use futures::{StreamExt, TryStreamExt};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use storage_device::ExtentId;

/// How many times an extent may have moved.
pub(crate) const MOVES: usize = 64;

/// Whether the collector moved an extent when it compacted the object holding it.
pub(crate) enum Moved {
    /// It did not compact the object.
    No,
    To(Location),
    /// It compacted the object and left the extent out: no live branch held it.
    Dropped,
}

/// The maps of compacted objects' moves, each read once.
#[derive(Default)]
pub(crate) struct MovesMaps {
    maps: HashMap<String, Option<BTreeMap<ExtentId, Location>>>,
}

/// How many objects' moves [`MovesMaps::read`] reads at once: each a round trip to object storage.
const READ_AT_ONCE: usize = 32;

impl MovesMaps {
    /// Reads the moves of the objects at `keys` not read yet, several at once: a checkpoint follows
    /// every extent of its branch, each object's a round trip, which one at a time, as the branch
    /// grew, held the uploader up for tens of seconds at 25 ms of latency (TASK-91.2).
    pub async fn read(
        &mut self,
        bucket: &impl Bucket,
        keys: impl IntoIterator<Item = String>,
    ) -> Result<(), Error> {
        let unread: BTreeSet<String> =
            keys.into_iter().filter(|key| !self.maps.contains_key(key)).collect();
        let read: Vec<_> = futures::stream::iter(unread)
            .map(|key| async move {
                let moves: Option<Moves> = object::footer(bucket, &layout::moves(&key)).await?;
                Ok::<_, Error>((key, moves.map(|moves| moves.extents.into_iter().collect())))
            })
            .buffer_unordered(READ_AT_ONCE)
            .try_collect()
            .await?;
        self.maps.extend(read);
        Ok(())
    }

    /// Whether the collector moved `extent` from the object at `key`, and where to.
    pub async fn moved(
        &mut self,
        bucket: &impl Bucket,
        extent: ExtentId,
        key: &str,
    ) -> Result<Moved, Error> {
        if !self.maps.contains_key(key) {
            let moves: Option<Moves> = object::footer(bucket, &layout::moves(key)).await?;
            let map = moves.map(|moves| moves.extents.into_iter().collect());
            self.maps.insert(key.to_owned(), map);
        }
        Ok(match &self.maps[key] {
            None => Moved::No,
            Some(map) => map.get(&extent).cloned().map_or(Moved::Dropped, Moved::To),
        })
    }

    /// Where `extent` at `location` is now, following its moves as far as they go.
    pub async fn follow(
        &mut self,
        bucket: &impl Bucket,
        extent: ExtentId,
        location: &Location,
    ) -> Result<Location, Error> {
        let mut location = location.clone();
        // An extent the collector dropped no live branch held.
        while let Moved::To(to) = self.moved(bucket, extent, &location.key).await? {
            location = to;
        }
        Ok(location)
    }
}
