// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A stand-in for `src/performance/memory/attribution/refaults_vmo/src/lib.rs`: the page refault
//! counter, kept in memory rather than in a VMO other processes can read, as nothing reads it here.

use std::sync::atomic::{AtomicU64, Ordering};

pub struct PageRefaultCounter {
    count: AtomicU64,
}

impl PageRefaultCounter {
    /// Creates a new read-write PageRefaultCounter.
    pub fn new() -> Result<Self, zx::Status> {
        Ok(PageRefaultCounter { count: AtomicU64::new(0) })
    }

    pub fn increment(&self, count: u64, order: Ordering) {
        self.count.fetch_add(count, order);
    }

    pub fn read(&self, order: Ordering) -> u64 {
        self.count.load(order)
    }
}
