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
use crate::object_store::extent_record::ExtentValue;
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
            if let ObjectValue::Extent(ExtentValue::Some { extent, extent_offset, .. }) = value
                && key.end > range.start
            {
                let start = max(key.start, range.start);
                let end = min(key.end, range.end);
                let device_start = extent_offset + (start - key.start);
                let device_range =
                    ExtentRange::new(*extent, device_start..device_start + (end - start));
                let access =
                    if device.is_open(*extent) { ExtentAccess::Open } else { ExtentAccess::Sealed };
                extents.push((FileExtent::new(start, device_range)?, access));
            }
            iter.advance().await?;
        }
        Ok(extents)
    }
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
}
