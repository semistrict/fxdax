// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What eviction of an arena's own pages on demand (`Evictor::evict_arena_on_demand`, TASK-79.76)
//! costs relief's pass for an arena at its own cap, beyond the evictions it makes, with none of its
//! pages to evict: an arena of GIB GiB of pages no page queue holds, as nothing evicts anonymous
//! pages, which a tenant's arena holds once its cached extents are gone, its allocations failing,
//! and each asking for a pass again. Each pass asks for relief's two chunks' worth.
//!
//! ```sh
//! cargo run --release -p zircon --example arena-eviction-bench [GIB [PASSES]]
//! ```
//!
//! GIB is the size of the arena (default 4), and PASSES how many passes are made (default 16).
//! Prints the pages a pass visited and evicted, and the passes' mean and slowest times.

use std::time::{Duration, Instant};
use zircon::boot_options::BootOptions;
use zircon::vm::evictor::Evictor;
use zircon::vm::page::VmPageDoublyLinkedList;
use zircon::vm::physmap::CHUNK_SIZE;
use zircon::vm::pmm::{self, PmmOptDelayReuse};

/// What relief's pass frees for an arena at least, and asks the kernel's evictor for of what its
/// user's evictor leaves: a chunk's worth twice over (`pmm::relief`'s `RELIEF_BYTES`).
const RELIEF_BYTES: u64 = 2 * CHUNK_SIZE as u64;

fn main() {
    let mut args = std::env::args().skip(1);
    let gib: usize = args.next().map_or(4, |n| n.parse().unwrap());
    let passes: u32 = args.next().map_or(16, |n| n.parse().unwrap());
    let bytes = gib << 30;

    // Room for the arena's pages, and the default arena's.
    let memory_bytes = (bytes + 4 * CHUNK_SIZE) as u64;
    BootOptions::set(BootOptions { memory_bytes, ..Default::default() }).unwrap();
    let node = pmm::node();
    // Its spare would be one more chunk of the arena's, of free pages.
    node.keep_spare_chunks(false);
    let evictor = node.evictor();
    evictor.enable_eviction(false);
    let arena = pmm::create_arena();
    let mut pages = VmPageDoublyLinkedList::new();
    node.alloc_pages_in(arena, bytes / page::SIZE, 0, &mut pages).unwrap();

    let visited = Evictor::get_global_stats().arena_pages_visited;
    let mut evicted = 0;
    let mut times: Vec<Duration> = (0..passes)
        .map(|_| {
            let start = Instant::now();
            evicted += evictor.evict_arena_on_demand(arena, RELIEF_BYTES).pager_backed;
            start.elapsed()
        })
        .collect();
    let visited = Evictor::get_global_stats().arena_pages_visited - visited;
    times.sort();
    // SAFETY: The pages were allocated above, and are this function's.
    unsafe { node.free_list(&mut pages, PmmOptDelayReuse::Default) };

    let millis = |time: Duration| time.as_secs_f64() * 1e3;
    println!(
        "{gib} GiB arena, no page to evict: a pass visited {} pages and evicted {}, in {:.3} ms on \
         average, the slowest {:.3} ms",
        visited / u64::from(passes),
        evicted / u64::from(passes),
        millis(times.iter().sum::<Duration>() / passes),
        millis(*times.last().unwrap()),
    );
}
