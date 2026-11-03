// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;

mod fifo;

pub use fifo::*;

bitflags! {
    /// Options that may be used for writes.
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct WriteFlags: u32 {
        const FORCE_ACCESS = 1;
        const PRE_BARRIER = 2;
        /// fxdax: a barrier follows the write, as a flush once it completes would: one request,
        /// where a device's requests are round trips to its host (doc-4).
        const POST_BARRIER = 4;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteOptions<'a> {
    pub inline_crypto: InlineCryptoOptions,
    pub flags: WriteFlags,
    /// fxdax: with `POST_BARRIER`, the root its barrier sets, as a commit of the root does
    /// (`storage_device::Device::commit_root`): in the write's one request, where a device's
    /// requests are round trips to its host (doc-4). Not `repr(C)`, as upstream's is, for it.
    pub root: Option<&'a [u8]>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadOptions {
    pub inline_crypto: InlineCryptoOptions,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineCryptoOptions {
    pub is_enabled: bool,
    pub slot: u8,
    pub dun: u64,
}

impl InlineCryptoOptions {
    pub fn enabled(slot: u8, dun: u64) -> Self {
        Self { is_enabled: true, slot, dun }
    }
}
