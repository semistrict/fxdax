// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What the kernel's part of a DAX window's MAP costs as the mappings of the VMO it maps grow
//! (TASK-92.50). `Window::map` maps an extent's VMO over what the window maps there, with
//! `zx_vmar_map` and `ZX_VM_SPECIFIC_OVERWRITE`, and populates it, with `ZX_VMAR_OP_MAP_RANGE`;
//! `map_open` commits the pages of an open extent it maps for the guest to write first. This does
//! the same, through the same syscalls, to a guest physical address space whose second stage
//! counts the changes made to it, which a VM process's would make, for 1k, 10k and 100k mappings
//! of the VMO:
//!
//! - `fresh`: a map of a VMO's committed page at each page of the window, where nothing was mapped;
//! - `in order`, `spread`: a map of it again over ROUNDS of those mappings, where its populated
//!   page was, the next mapping each time, or one far from the last;
//! - `split`: a map of a page over the middle page of ROUNDS mappings of three, spread, as a MAP
//!   of part of what a larger mapping maps does, which splits the larger mapping in two;
//! - `commit`: a commit of a page of a VMO of a page for each mapping, of those mapped where
//!   nothing is committed yet, ROUNDS of them spread: what each page `map_open` commits costs, its
//!   page forked from the zero page and unmapped from the VMO's mappings of it.
//!
//! ```sh
//! cargo run --release -p zircon --example vmar-overwrite-bench [ROUNDS]
//! ```
//!
//! ROUNDS is how many maps or commits are timed each way at each count (default 10000), and at
//! most one for each mapping split or committed. Prints each one's time, and the second stage's
//! changes each made. The kernel's part alone: the extent daemon's remote stage waits for a VM
//! process to make each unmap it sends, which this one makes at once.

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

/// The stride of the spread rounds, coprime with the counts: a mapping far from the last each
/// time, whose lookups in the trees of mappings would miss the caches more as the trees grow,
/// where those of the rounds in order would find them cached. The spread rounds' times growing
/// with the count is consistent with that; the misses are not measured.
const STRIDE: usize = 7919;

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

/// A window of `pages` pages, whose second stage counts its changes, and a VMO of `vmo_pages`
/// pages, the first `committed` of them committed.
struct Setup {
    second_stage: Arc<CountingSecondStage>,
    /// The window's address space, kept while the window is used.
    _gpas: GuestPhysicalAspace,
    vmar: zx_handle_t,
    base: usize,
    vmo: zx_handle_t,
}

impl Setup {
    fn new(pages: usize, vmo_pages: usize, committed: usize) -> Self {
        let second_stage = Arc::new(CountingSecondStage::default());
        let gpas =
            GuestPhysicalAspace::create_with_second_stage(pages * PAGE_SIZE, second_stage.clone())
                .unwrap();
        let (vmar, rights) = gpas.create_vmar_dispatcher().unwrap();
        let base = vmar.dispatcher().vmar().base();
        let vmar = vmar.make_and_add_handle(rights).unwrap();
        let mut vmo = ZX_HANDLE_INVALID;
        // SAFETY: `vmo` is this function's.
        ok(unsafe { zx_vmo_create((vmo_pages * PAGE_SIZE) as u64, 0, &mut vmo) }, "create the VMO");
        let setup = Self { second_stage, _gpas: gpas, vmar, base, vmo };
        if committed > 0 {
            setup.commit(0, committed);
        }
        setup
    }

    /// Maps `pages` pages of the VMO from its page `page` into the window at its page `at`, over
    /// what is mapped there, and populates them, as `Window::map` does.
    fn map(&self, at: usize, page: usize, pages: usize) {
        let options = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC_OVERWRITE;
        let (offset, vmo_offset, len) =
            (at * PAGE_SIZE, (page * PAGE_SIZE) as u64, pages * PAGE_SIZE);
        let mut mapped = 0;
        // SAFETY: The window's memory is accessed by nothing; `mapped` is this function's.
        let status = unsafe {
            zx_vmar_map(self.vmar, options, offset, self.vmo, vmo_offset, len, &mut mapped)
        };
        ok(status, "map");
        let null = std::ptr::null_mut();
        let addr = self.base + offset;
        // SAFETY: No buffer is passed.
        let status =
            unsafe { zx_vmar_op_range(self.vmar, ZX_VMAR_OP_MAP_RANGE, addr, len, null, 0) };
        ok(status, "populate");
    }

    /// Commits `pages` pages of the VMO from its page `page`, as `map_open` does.
    fn commit(&self, page: usize, pages: usize) {
        let (offset, len) = ((page * PAGE_SIZE) as u64, (pages * PAGE_SIZE) as u64);
        let null = std::ptr::null_mut();
        // SAFETY: No buffer is passed.
        let status = unsafe { zx_vmo_op_range(self.vmo, ZX_VMO_OP_COMMIT, offset, len, null, 0) };
        ok(status, "commit");
    }

    /// Times `count` calls of `f`, each with its index, and prints a line of `what`'s results:
    /// each call's time and the changes each made.
    fn time(&self, mappings: usize, what: &str, count: usize, mut f: impl FnMut(usize)) {
        self.second_stage.take();
        let start = Instant::now();
        for index in 0..count {
            f(index);
        }
        report(mappings, what, start.elapsed(), count, self.second_stage.take());
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        // SAFETY: The handles are this one's.
        unsafe {
            ok(zx_handle_close(self.vmo), "close the VMO");
            ok(zx_handle_close(self.vmar), "close the window");
        }
    }
}

/// Prints a line of `what`'s results: each call's time and the changes each made.
fn report(mappings: usize, what: &str, elapsed: Duration, count: usize, changes: [usize; 3]) {
    let per_call = |changes: usize| changes as f64 / count as f64;
    println!(
        "{mappings:>7} mappings, {what:<8}: {:>8.0} ns each, second stage {:.2} maps, {:.2} \
         protects, {:.2} unmaps each",
        elapsed.as_nanos() as f64 / count as f64,
        per_call(changes[0]),
        per_call(changes[1]),
        per_call(changes[2]),
    );
}

fn main() {
    let rounds: usize = std::env::args().nth(1).map_or(10_000, |n| n.parse().unwrap());
    for mappings in [1_000, 10_000, 100_000] {
        // The rounds that must each be of another mapping.
        let distinct = rounds.min(mappings);
        // A page, committed, mapped at each page of the window, then over those mappings.
        let setup = Setup::new(mappings, 1, 1);
        setup.time(mappings, "fresh", mappings, |mapping| setup.map(mapping, 0, 1));
        setup.time(mappings, "in order", rounds, |round| setup.map(round % mappings, 0, 1));
        setup.time(mappings, "spread", rounds, |round| setup.map(round * STRIDE % mappings, 0, 1));
        drop(setup);

        // Three pages, committed, mapped together at each three of the window, then a page over
        // the middle one of some.
        let setup = Setup::new(3 * mappings, 3, 3);
        for mapping in 0..mappings {
            setup.map(3 * mapping, 0, 3);
        }
        setup.time(mappings, "split", distinct, |round| {
            setup.map(3 * (round * STRIDE % mappings) + 1, 1, 1)
        });
        drop(setup);

        // A page for each mapping, none committed, each mapped at a page of the window, then some
        // committed.
        let setup = Setup::new(mappings, mappings, 0);
        for mapping in 0..mappings {
            setup.map(mapping, mapping, 1);
        }
        setup
            .time(mappings, "commit", distinct, |round| setup.commit(round * STRIDE % mappings, 1));
    }
}
