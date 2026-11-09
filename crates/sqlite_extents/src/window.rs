// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Memory a volume maps its files' committed blocks into, read-only, as a VM's DAX window maps
//! its filesystem's (backlog doc-13): so that a read copies from memory, with no request of the
//! device, and SQLite's memory-mapped I/O (`xFetch`) reads pages in place, copying nothing.
//!
//! A file holds a region of the window, laid out as the file is: its chunk `k` at `k` blocks into
//! the region, mapped to the block of the extent that holds it, or to zeros if a hole. A commit
//! maps what it changed again, before any read sees the file as of it (`volume`).

use anyhow::Error;
use std::ptr::NonNull;
use storage_device::{Device, ExtentId};

/// What a run of a window maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contents {
    /// A sealed extent's bytes, from `offset`.
    Extent { extent: ExtentId, offset: u64 },
    /// Zeros.
    Zeros,
}

/// A run of a window: `len` bytes at `at` mapping `contents`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub at: u64,
    pub len: u64,
    pub contents: Contents,
}

/// A window. Offsets and lengths are multiples of the device's block size, which is the window's
/// page size.
pub trait Window: Send + Sync {
    /// The memory: its first byte, and how many it holds.
    fn memory(&self) -> (NonNull<u8>, u64);

    /// Maps each run, read-only, replacing what was mapped there: blocks of `device`'s sealed
    /// extents, or zeros.
    fn map(&self, device: &dyn Device, runs: &[Run]) -> Result<(), Error>;

    /// Unmaps `len` bytes at `at`: nothing reads them until they are mapped again.
    fn unmap(&self, at: u64, len: u64) -> Result<(), Error>;
}
