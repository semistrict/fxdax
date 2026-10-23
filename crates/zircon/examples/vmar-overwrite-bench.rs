// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What a DAX window's MAP costs the kernel as the mappings of the VMO it maps grow
//! (TASK-92.50). `Window::map` maps an extent's VMO over what the window maps there, with
//! `zx_vmar_map` and `ZX_VM_SPECIFIC_OVERWRITE`, and populates it, with `ZX_VMAR_OP_MAP_RANGE`;
//! this does the same, through the same syscalls, to a guest physical address space whose second
//! stage counts the changes made to it, which a VM process's would make. A VMO of a committed page
//! is mapped at each page of the window (fresh: nothing was mapped there), and then again over
//! ROUNDS of those mappings, where its populated page was mapped, in order and spread over the
//! window, for 1k, 10k and 100k mappings of it.
//!
//! ```sh
//! cargo run --release -p zircon --example vmar-overwrite-bench [ROUNDS]
//! ```
//!
//! ROUNDS is how many overwriting maps are timed each way at each count (default 10000). Prints
//! each map's time, fresh, in order and spread, and the second stage's changes each made. The
//! kernel's part alone: a VM process's stage is waited for on each unmap, which this one is not.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use zircon::hypervisor::aspace::{GuestPhysicalAspace, SecondStage};
use zircon::syscalls::{
    zx_handle_close, zx_vmar_map, zx_vmar_op_range, zx_vmo_create, zx_vmo_op_range,
};
use zircon::vm::physmap::PhysLocation;
use zx_types::*;

const PAGE_SIZE: usize = page::SIZE;

/// A second stage counting the changes made to it.
#[derive(Default)]
struct CountingSecondStage {
    maps: AtomicUsize,
    protects: AtomicUsize,
    unmaps: AtomicUsize,
}

impl CountingSecondStage {
    /// The maps, protects and unmaps made since the last call.
    fn take(&self) -> [usize; 3] {
        [&self.maps, &self.protects, &self.unmaps].map(|count| count.swap(0, Ordering::Relaxed))
    }
}

impl SecondStage for CountingSecondStage {
    fn map(&self, _vaddr: usize, _phys: PhysLocation, _len: usize, _prot: libc::c_int) {
        self.maps.fetch_add(1, Ordering::Relaxed);
    }

    fn protect(&self, _vaddr: usize, _len: usize, _prot: libc::c_int) {
        self.protects.fetch_add(1, Ordering::Relaxed);
    }

    fn unmap(&self, _vaddr: usize, _len: usize) {
        self.unmaps.fetch_add(1, Ordering::Relaxed);
    }
}

/// Panics unless `status` is `ZX_OK`.
fn ok(status: zx_status_t, what: &str) {
    assert_eq!(status, ZX_OK, "{what}");
}

/// Maps the page of `vmo` at `offset` into the window `vmar` at `base`, over what is mapped
/// there, and populates it, as `Window::map` does.
fn map(vmar: zx_handle_t, base: usize, vmo: zx_handle_t, offset: usize) {
    let options = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC_OVERWRITE;
    let mut mapped = 0;
    // SAFETY: The window's memory is accessed by nothing; `mapped` is this function's.
    let status = unsafe { zx_vmar_map(vmar, options, offset, vmo, 0, PAGE_SIZE, &mut mapped) };
    ok(status, "map");
    let null = std::ptr::null_mut();
    // SAFETY: No buffer is passed.
    let status =
        unsafe { zx_vmar_op_range(vmar, ZX_VMAR_OP_MAP_RANGE, base + offset, PAGE_SIZE, null, 0) };
    ok(status, "populate");
}

/// Prints a line of `what`'s results: each map's time and the changes each made.
fn report(mappings: usize, what: &str, elapsed: Duration, maps: usize, changes: [usize; 3]) {
    let per_map = |count: usize| count as f64 / maps as f64;
    println!(
        "{mappings:>7} mappings, {what:<8}: {:>6.0} ns a map, second stage {:.2} maps, {:.2} \
         protects, {:.2} unmaps a map",
        elapsed.as_nanos() as f64 / maps as f64,
        per_map(changes[0]),
        per_map(changes[1]),
        per_map(changes[2]),
    );
}

fn main() {
    let rounds: usize = std::env::args().nth(1).map_or(10_000, |n| n.parse().unwrap());
    for mappings in [1_000, 10_000, 100_000] {
        let second_stage = Arc::new(CountingSecondStage::default());
        let gpas = GuestPhysicalAspace::create_with_second_stage(
            mappings * PAGE_SIZE,
            second_stage.clone(),
        )
        .unwrap();
        let (vmar, rights) = gpas.create_vmar_dispatcher().unwrap();
        let base = vmar.dispatcher().vmar().base();
        let vmar = vmar.make_and_add_handle(rights).unwrap();
        let mut vmo = ZX_HANDLE_INVALID;
        // SAFETY: `vmo` is this function's.
        ok(unsafe { zx_vmo_create(PAGE_SIZE as u64, 0, &mut vmo) }, "create the VMO");
        let null = std::ptr::null_mut();
        // SAFETY: No buffer is passed.
        let status =
            unsafe { zx_vmo_op_range(vmo, ZX_VMO_OP_COMMIT, 0, PAGE_SIZE as u64, null, 0) };
        ok(status, "commit the VMO");

        let start = Instant::now();
        for mapping in 0..mappings {
            map(vmar, base, vmo, mapping * PAGE_SIZE);
        }
        report(mappings, "fresh", start.elapsed(), mappings, second_stage.take());

        // In order, the next mapping each time, and spread, by a stride coprime with the count, a
        // mapping far from the last each time, whose lookups in the trees of mappings miss the
        // caches more as the trees grow, where those of the rounds in order find them cached.
        const STRIDE: usize = 7919;
        for (what, stride) in [("in order", 1), ("spread", STRIDE)] {
            let start = Instant::now();
            for round in 0..rounds {
                map(vmar, base, vmo, round * stride % mappings * PAGE_SIZE);
            }
            report(mappings, what, start.elapsed(), rounds, second_stage.take());
        }

        // SAFETY: The handles are this function's.
        unsafe {
            ok(zx_handle_close(vmo), "close the VMO");
            ok(zx_handle_close(vmar), "close the window");
        }
    }
}
