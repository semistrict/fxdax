// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of data_object_handle.rs: what a DAX window maps a file's data from
//! (backlog doc-13), which Fuchsia doesn't have: the extents holding it, and whether each is
//! sealed, as the window maps only sealed extents.

use super::{DataObjectHandle, FileExtent};
use crate::lsm_tree::Query;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_handle::ObjectHandle;
use crate::object_store::extent_record::{ExtentMode, ExtentValue};
use crate::object_store::object_record::{AttributeKey, ObjectKey, ObjectKeyData, ObjectValue};
use crate::object_store::{Extent, HandleOwner};
use anyhow::Error;
use std::cmp::{max, min};
use std::ops::Range;
use storage_device::ExtentRange;

/// fxdax: whether a DAX window may map a file's blocks ([`DataObjectHandle::extents_in`]): it maps
/// sealed extents only, read-only (backlog doc-13). By the numbers of
/// guest/linux/fs/fxfs/fxfs.h's `enum fxfs_dax_access`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum ExtentAccess {
    /// Their extent isn't known to be open: a barrier came since it was minted, which sealed it, or
    /// found nothing of it on the device yet. The window may map them.
    #[default]
    Sealed = 0,
    /// Their extent is open still, until the device's next barrier seals it: the window may not
    /// map them yet (the device refuses a MAP of an open extent).
    Open = 1,
}

impl<S: HandleOwner> DataObjectHandle<S> {
    /// fxdax: the extents holding the file's data in `range`, clipped to it, in order, with whether
    /// a DAX window may map them yet, as their extents are sealed (`ExtentAccess`). What it maps
    /// the range from (backlog doc-13). Holes are left out.
    pub async fn extents_in(
        &self,
        range: Range<u64>,
    ) -> Result<Vec<(FileExtent, ExtentAccess)>, Error> {
        let store = self.store();
        let device = store.device();
        let layer_set = store.tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger
            .query(Query::FullRange(&ObjectKey::attribute(
                self.object_id(),
                self.attribute_id(),
                AttributeKey::Extent(Extent(range.clone()).search_key()),
            )))
            .await?;
        let mut extents = Vec::new();
        while let Some(ItemRef {
            key:
                ObjectKey {
                    object_id,
                    data: ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(key)),
                },
            value,
            ..
        }) = iter.get()
            && *object_id == self.object_id()
            && *attribute_id == self.attribute_id()
            && key.start < range.end
        {
            if let ObjectValue::Extent(ExtentValue::Some { extent, extent_offset, mode, .. }) =
                value
                && key.end > range.start
            {
                let clipped = max(key.start, range.start)..min(key.end, range.end);
                let access =
                    if device.is_open(*extent) { ExtentAccess::Open } else { ExtentAccess::Sealed };
                for part in written(mode, key, clipped, self.block_size().get()) {
                    let device_start = extent_offset + (part.start - key.start);
                    let device_range = ExtentRange::new(
                        *extent,
                        device_start..device_start + (part.end - part.start),
                    );
                    extents.push((FileExtent::new(part.start, device_range)?, access));
                }
            }
            iter.advance().await?;
        }
        Ok(extents)
    }
}

/// fxdax: the parts of `range`, of an extent of `mode` holding a file's `key`, that hold what was
/// written: all of it, but of an allocation's extent, whose bitmap says which of its blocks were,
/// those written alone. The others the device doesn't hold, and they read as zeros, as holes do: a
/// window mapping them would map what isn't there (TASK-149.36).
fn written(
    mode: &ExtentMode,
    key: &Range<u64>,
    range: Range<u64>,
    block_size: u64,
) -> Vec<Range<u64>> {
    let ExtentMode::OverwritePartial(bitmap) = mode else { return vec![range] };
    let mut parts: Vec<Range<u64>> = Vec::new();
    let mut block = (range.start - key.start) / block_size;
    while key.start + block * block_size < range.end {
        let from = max(key.start + block * block_size, range.start);
        let to = min(key.start + (block + 1) * block_size, range.end);
        if bitmap.get(block as usize).unwrap_or(false) {
            match parts.last_mut() {
                Some(last) if last.end == from => last.end = to,
                _ => parts.push(from..to),
            }
        }
        block += 1;
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::ExtentAccess;
    use crate::filesystem::SyncOptions;
    use crate::object_handle::{ObjectHandle, WriteObjectHandle};
    use crate::object_store::data_object_handle::tests::test_filesystem_and_object_with_key;
    use std::ops::Range;

    /// fxdax: `extents_in` gives the extents holding a file's data in a range, clipped to it, and
    /// whether each is sealed, as a DAX window maps sealed extents only (backlog doc-13): blocks
    /// written are unsealed until the device's next barrier seals their extent.
    #[fuchsia::test]
    async fn test_extents_in_says_whether_sealed() {
        let (fs, object) = test_filesystem_and_object_with_key(None, false).await;
        let bs = fs.block_size().get();
        let mut buf = object.allocate_buffer(2 * bs as usize).await;
        buf.fill(3);
        object.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        let extents_in = async |range: Range<u64>| {
            object
                .extents_in(range)
                .await
                .expect("extents_in failed")
                .into_iter()
                .map(|(e, access)| (e.logical_range(), access))
                .collect::<Vec<_>>()
        };
        assert_eq!(extents_in(5..2 * bs - 1).await, vec![(5..2 * bs - 1, ExtentAccess::Open)]);

        fs.sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect("sync failed");
        assert_eq!(extents_in(5..2 * bs - 1).await, vec![(5..2 * bs - 1, ExtentAccess::Sealed)]);
        fs.close().await.expect("close failed");
    }

    /// fxdax: of what an allocation made, `extents_in` gives what was written since alone: the
    /// device doesn't hold the rest, which reads as zeros, as holes do, and a window that mapped it
    /// would map what isn't there (TASK-149.36).
    #[fuchsia::test]
    async fn test_extents_in_leaves_out_what_an_allocation_left_unwritten() {
        let (fs, object) = test_filesystem_and_object_with_key(None, false).await;
        let bs = fs.block_size().get();
        object.allocate(0..4 * bs).await.expect("allocate failed");
        let extents_in = async |range: Range<u64>| {
            let extents = object.extents_in(range).await.expect("extents_in failed");
            extents.into_iter().map(|(e, _)| e.logical_range()).collect::<Vec<_>>()
        };
        assert_eq!(extents_in(0..4 * bs).await, Vec::<Range<u64>>::new());

        let mut buf = object.allocate_buffer(bs as usize).await;
        buf.fill(3);
        object.write_or_append(Some(bs), buf.as_ref()).await.expect("write failed");
        assert_eq!(extents_in(0..4 * bs).await, vec![bs..2 * bs]);
        assert_eq!(extents_in(bs + 5..3 * bs).await, vec![bs + 5..2 * bs]);
        fs.close().await.expect("close failed");
    }
}
