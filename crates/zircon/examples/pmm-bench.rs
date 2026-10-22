// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What chunks going back to the host cost the PMM's other allocations (TASK-92.35). One thread
//! frees the pages of VMOs of an arena, as a VM's teardown frees its cache, a few chunks' worth
//! each, written to, as a cache's are: each chunk goes back to the host as the last of its pages
//! is freed. Another thread meanwhile allocates a page of another arena, which has free pages, and
//! frees it, again and again, timing each: what it waits beyond its own work is the PMM's lock,
//! held by the first.
//!
//! ```sh
//! cargo run --release -p zircon --example pmm-bench [CHUNKS [CHUNKS_PER_VMO]]
//! ```
//!
//! CHUNKS is how many chunks (2 MiB) the freed VMOs hold together (default 256), and
//! CHUNKS_PER_VMO how many each holds (default 2). Prints how long the frees took, a chunk at a
//! time, and the other thread's allocations' times: the median, the slowest in a thousand and the
//! slowest.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use zircon::boot_options::BootOptions;
use zircon::vm::page::VmPageDoublyLinkedList;
use zircon::vm::physmap::{CHUNK_SIZE, paddr_to_physmap};
use zircon::vm::pmm::{self, PmmOptDelayReuse};

const CHUNK_PAGES: usize = CHUNK_SIZE / page::SIZE;

fn main() {
    let mut args = std::env::args().skip(1);
    let chunks: usize = args.next().map_or(256, |n| n.parse().unwrap());
    let per_vmo: usize = args.next().map_or(2, |n| n.parse().unwrap());

    // Room for the freed VMOs, their arena's spare, and the other arena's chunk and spare.
    let memory_bytes = ((chunks + 4) * CHUNK_SIZE) as u64;
    BootOptions::set(BootOptions { memory_bytes, ..Default::default() }).unwrap();
    let node = pmm::node();
    let (freed, other) = (pmm::create_arena(), pmm::create_arena());

    let mut vmos: Vec<VmPageDoublyLinkedList> = (0..chunks.div_ceil(per_vmo))
        .map(|_| {
            let mut pages = VmPageDoublyLinkedList::new();
            node.alloc_pages_in(freed, per_vmo * CHUNK_PAGES, 0, &mut pages).unwrap();
            for page in pages.iter() {
                // SAFETY: Writes a page this owns.
                unsafe { *paddr_to_physmap(page.paddr()) = 1 };
            }
            pages
        })
        .collect();
    // A page of the other arena's held throughout, so that its chunk never goes back.
    let held = node.alloc_page_in(other, 0).unwrap();
    let free_chunks = node.count_free_chunks();

    let done = Arc::new(AtomicBool::new(false));
    let allocator = std::thread::spawn({
        let done = done.clone();
        move || {
            let node = pmm::node();
            let mut times = Vec::new();
            while !done.load(Ordering::Relaxed) {
                let start = Instant::now();
                let page = node.alloc_page_in(other, 0).unwrap();
                // SAFETY: The page was just allocated, and is this thread's.
                unsafe { node.free_page(page, PmmOptDelayReuse::Default) };
                times.push(start.elapsed());
            }
            times
        }
    });
    let start = Instant::now();
    for pages in &mut vmos {
        // SAFETY: The pages were allocated above, and are this thread's.
        unsafe { node.free_list(pages, PmmOptDelayReuse::Default) };
    }
    let freeing = start.elapsed();
    done.store(true, Ordering::Relaxed);
    let mut times = allocator.join().unwrap();
    times.sort();
    let gone_back = node.count_free_chunks() - free_chunks;
    // SAFETY: The page was allocated above.
    unsafe { node.free_page(held, PmmOptDelayReuse::Default) };

    let micros = |time: Duration| time.as_secs_f64() * 1e6;
    println!(
        "frees: {chunks} chunks in VMOs of {per_vmo}, {gone_back} gone back, in {:.1} ms: {:.1} µs a chunk",
        freeing.as_secs_f64() * 1e3,
        micros(freeing) / chunks as f64,
    );
    let at = |fraction: f64| micros(times[((times.len() - 1) as f64 * fraction) as usize]);
    println!(
        "other allocations: {}, median {:.2} µs, 99.9th percentile {:.1} µs, slowest {:.1} µs",
        times.len(),
        at(0.5),
        at(0.999),
        at(1.0),
    );
}
