// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own, a child of data_object_handle.rs: the pages of a file whose data frozen owners hold
//! (volume snapshots, which share their source's layers and extents), which can be cached and
//! shared by extent and offset.

use super::DataObjectHandle;
use crate::errors::FxfsError;
use crate::lsm_tree::Query;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_handle::ObjectHandle;
use crate::object_store::HandleOwner;
use crate::object_store::extent_record::{ExtentMode, ExtentValue};
use crate::object_store::object_record::{AttributeKey, ObjectKey, ObjectKeyData, ObjectValue};
use crate::object_store::transaction::{LockKey, lock_keys};
use anyhow::{Error, ensure};
use std::ops::Range;
use storage_device::{ExtentId, ExtentRange};

/// fxdax: a page of an object whose content is data on the device that a frozen owner holds (see
/// `Allocator::snapshot`), and so does not change while the owner is frozen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrozenPage {
    /// The page's offset in the object.
    pub offset: u64,
    /// The extent holding the page's content, and where in it the content starts.
    pub extent: ExtentId,
    pub extent_offset: u64,
    pub owner_object_id: u64,
}

impl<S: HandleOwner> DataObjectHandle<S> {
    /// fxdax: the pages of `page_size` bytes in `range`, which must be aligned to them, whose
    /// content is data that frozen owners hold: unencrypted, in a single extent, fully written and
    /// held by one owner. Pages past the end of the object are its caller's to leave out, as their
    /// content is zero past it.
    pub async fn frozen_pages(
        &self,
        range: Range<u64>,
        page_size: u64,
    ) -> Result<Vec<FrozenPage>, Error> {
        ensure!(range.start % page_size == 0 && range.end % page_size == 0, FxfsError::InvalidArgs);
        let store = self.store();
        let fs = store.filesystem();
        let allocator = fs.allocator();
        if range.is_empty() || self.handle.is_encrypted() || !allocator.has_frozen_owners() {
            return Ok(Vec::new());
        }
        let _guard = fs
            .lock_manager()
            .read_lock(lock_keys![LockKey::object_attribute(
                store.store_object_id(),
                self.object_id(),
                self.attribute_id(),
            )])
            .await;
        let layer_set = store.tree().layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger
            .query(Query::LimitedRange(&ObjectKey::extent(
                self.object_id(),
                self.attribute_id(),
                range.clone(),
            )))
            .await?;
        let mut pages = Vec::new();
        while let Some(ItemRef {
            key:
                ObjectKey {
                    object_id,
                    data: ObjectKeyData::Attribute(attribute_id, AttributeKey::Extent(extent)),
                },
            value: ObjectValue::Extent(value),
            ..
        }) = iter.get()
        {
            if *object_id != self.object_id()
                || *attribute_id != self.attribute_id()
                || extent.start >= range.end
            {
                break;
            }
            // Partially written extents read as zero where they were not written.
            if let ExtentValue::Some {
                extent: store_extent,
                extent_offset: device_offset,
                mode,
                ..
            } = value
                && !matches!(mode, ExtentMode::OverwritePartial(_))
            {
                // The whole pages of the extent within the range.
                let start = std::cmp::max(extent.start, range.start).next_multiple_of(page_size);
                let end = std::cmp::min(extent.end, range.end) / page_size * page_size;
                if start < end {
                    let device_start = device_offset + (start - extent.start);
                    let device_end = device_offset + (end - extent.start);
                    for (frozen, owner_object_id) in allocator
                        .frozen_ranges(ExtentRange::new(*store_extent, device_start..device_end))
                        .await?
                    {
                        // The pages wholly within the frozen range.
                        let frozen = frozen.range;
                        let first = (frozen.start - device_start).next_multiple_of(page_size);
                        let last = (frozen.end - device_start) / page_size * page_size;
                        for page in (first..last).step_by(page_size as usize) {
                            pages.push(FrozenPage {
                                offset: start + page,
                                extent: *store_extent,
                                extent_offset: device_start + page,
                                owner_object_id,
                            });
                        }
                    }
                }
            }
            iter.advance().await?;
        }
        Ok(pages)
    }
}
