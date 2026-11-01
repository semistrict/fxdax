// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax's own: the layers a volume has of a base, a volume another filesystem wrote (backlog
// doc-10). A base layer is no object of this filesystem: its store's information says where its
// extents are, which the device reads through to the base, and the allocator never holds.

use crate::errors::FxfsError;
use crate::object_handle::{INVALID_OBJECT_ID, LayerObject, ObjectHandle, ReadObjectHandle};
use anyhow::{Error, ensure};
use async_trait::async_trait;
use fprint::TypeFingerprint;
use serde::{Deserialize, Serialize};
use std::cmp::{max, min};
use std::sync::Arc;
use storage_device::buffer::{BufferFuture, MutableBufferRef};
use storage_device::{Device, ExtentId};
use storage_units::BlockSize;

/// A layer file of a base: its size, and where its bytes are.
pub type BaseLayer = BaseLayerV61;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypeFingerprint)]
pub struct BaseLayerV61 {
    pub size: u64,
    /// In order, leaving out what reads as zeros.
    pub extents: Vec<BaseExtentV61>,
}

/// The bytes `offset..offset + length` of a base layer, `extent_offset` bytes into `extent`.
pub type BaseExtent = BaseExtentV61;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypeFingerprint)]
pub struct BaseExtentV61 {
    pub offset: u64,
    pub length: u64,
    pub extent: ExtentId,
    pub extent_offset: u64,
}

impl BaseLayer {
    /// Checks what a manifest says of the layer, which another filesystem wrote.
    fn check(&self, block_size: BlockSize) -> Result<(), Error> {
        let mut end = 0;
        for extent in &self.extents {
            ensure!(
                extent.offset >= end
                    && extent.length > 0
                    && block_size.is_aligned(extent.offset)
                    && block_size.is_aligned(extent.length)
                    && block_size.is_aligned(extent.extent_offset)
                    && extent.extent_offset.checked_add(extent.length).is_some(),
                FxfsError::Inconsistent
            );
            end = extent.offset.checked_add(extent.length).ok_or(FxfsError::Inconsistent)?;
        }
        ensure!(end <= self.size && block_size.is_aligned(self.size), FxfsError::Inconsistent);
        Ok(())
    }
}

/// A base layer, read through the device.
pub struct BaseLayerObject {
    device: Arc<dyn Device>,
    block_size: BlockSize,
    layer: BaseLayer,
}

impl BaseLayerObject {
    pub fn new(
        device: Arc<dyn Device>,
        block_size: BlockSize,
        layer: BaseLayer,
    ) -> Result<Self, Error> {
        layer.check(block_size)?;
        Ok(Self { device, block_size, layer })
    }
}

impl ObjectHandle for BaseLayerObject {
    fn object_id(&self) -> u64 {
        INVALID_OBJECT_ID
    }

    fn block_size(&self) -> BlockSize {
        self.block_size
    }

    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.device.allocate_buffer(size)
    }
}

#[async_trait]
impl ReadObjectHandle for BaseLayerObject {
    async fn read_aligned(&self, offset: u64, buf: MutableBufferRef<'_>) -> Result<usize, Error> {
        ensure!(self.block_size.is_aligned(offset), FxfsError::InvalidArgs);
        if offset >= self.layer.size {
            return Ok(0);
        }
        let len = min(buf.len() as u64, self.layer.size - offset);
        let end = offset + len;
        let mut buf = buf.subslice_mut(..len as usize);
        buf.fill(0);
        let extents = &self.layer.extents;
        let first = extents.partition_point(|extent| extent.offset + extent.length <= offset);
        for extent in extents[first..].iter().take_while(|extent| extent.offset < end) {
            let start = max(extent.offset, offset);
            let stop = min(extent.offset + extent.length, end);
            self.device
                .read(
                    extent.extent,
                    extent.extent_offset + (start - extent.offset),
                    buf.reborrow()
                        .subslice_mut((start - offset) as usize..(stop - offset) as usize),
                )
                .await?;
        }
        Ok(len as usize)
    }

    fn get_size(&self) -> u64 {
        self.layer.size
    }
}

impl LayerObject for BaseLayerObject {}

#[cfg(test)]
mod tests {
    use super::{BaseExtent, BaseLayer, BaseLayerObject};
    use crate::object_handle::ReadObjectHandle;
    use std::sync::Arc;
    use storage_device::fake_device::FakeDevice;
    use storage_device::{Device, ExtentId};
    use storage_units::BlockSize;

    const BLOCK: u64 = 4096;

    fn extent(offset: u64, length: u64, sequence: u64) -> BaseExtent {
        BaseExtent { offset, length, extent: ExtentId::new(1, sequence), extent_offset: BLOCK }
    }

    async fn write(device: &FakeDevice, sequence: u64, byte: u8) {
        let mut buffer = device.allocate_buffer(2 * BLOCK as usize).await;
        buffer.fill(byte);
        device.write(ExtentId::new(1, sequence), 0, buffer.as_ref()).await.expect("write failed");
    }

    fn object(device: Arc<FakeDevice>, layer: BaseLayer) -> anyhow::Result<BaseLayerObject> {
        BaseLayerObject::new(device, BlockSize::new(BLOCK as u32).unwrap(), layer)
    }

    /// Reads cross extents, and what no extent holds reads as zeros.
    #[fuchsia::test]
    async fn reads_cross_extents_and_holes() {
        let device = Arc::new(FakeDevice::new(1024, BLOCK as u32));
        write(&device, 1, 0xa).await;
        write(&device, 2, 0xb).await;
        let layer = BaseLayer {
            size: 5 * BLOCK,
            extents: vec![extent(BLOCK, BLOCK, 1), extent(3 * BLOCK, BLOCK, 2)],
        };
        let object = object(device.clone(), layer).expect("new failed");

        let mut buffer = device.allocate_buffer(6 * BLOCK as usize).await;
        assert_eq!(
            object.read_aligned(0, buffer.as_mut()).await.expect("read failed"),
            5 * BLOCK as usize
        );
        let expected: Vec<u8> =
            [0, 0xa, 0, 0xb, 0].iter().flat_map(|byte| vec![*byte; BLOCK as usize]).collect();
        assert_eq!(buffer.as_ptr_slice().to_vec()[..5 * BLOCK as usize], expected);

        assert_eq!(object.read_aligned(5 * BLOCK, buffer.as_mut()).await.expect("read failed"), 0);
    }

    /// A read of a layer whose base the device doesn't reach fails as a read of the extent does.
    #[fuchsia::test]
    async fn reads_of_extents_the_device_lacks_fail() {
        let device = Arc::new(FakeDevice::new(1024, BLOCK as u32));
        let layer = BaseLayer { size: BLOCK, extents: vec![extent(0, BLOCK, 1)] };
        let object = object(device.clone(), layer).expect("new failed");
        let mut buffer = device.allocate_buffer(BLOCK as usize).await;
        object.read_aligned(0, buffer.as_mut()).await.expect_err("read succeeded");
    }

    /// What another filesystem says of a layer is not trusted to be in order, aligned or within it.
    #[fuchsia::test]
    async fn layers_that_make_no_sense_are_refused() {
        let device = Arc::new(FakeDevice::new(1024, BLOCK as u32));
        let overlapping = vec![extent(0, 2 * BLOCK, 1), extent(BLOCK, BLOCK, 2)];
        for layer in [
            BaseLayer { size: 2 * BLOCK, extents: overlapping },
            BaseLayer { size: BLOCK, extents: vec![extent(0, 2 * BLOCK, 1)] },
            BaseLayer { size: 2 * BLOCK, extents: vec![extent(1, BLOCK, 1)] },
            BaseLayer { size: 2 * BLOCK, extents: vec![extent(0, 0, 1)] },
            BaseLayer { size: 2 * BLOCK, extents: vec![extent(u64::MAX - BLOCK + 1, BLOCK, 1)] },
            BaseLayer { size: BLOCK + 1, extents: vec![] },
        ] {
            object(device.clone(), layer).err().expect("new succeeded");
        }
    }
}
