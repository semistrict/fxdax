// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What a pass of eviction of an arena's own pages on demand (`Evictor::evict_arena_on_demand`,
//! TASK-79.76) costs relief, which asks it for relief's two chunks' worth (`pmm::RELIEF_BYTES`) of
//! an arena at its own cap: an arena of GIB GiB holding PAGES, one of
//!
//! - `anonymous`: pages no page queue holds, as nothing evicts anonymous pages, which a tenant's
//!   arena holds once its cached extents are gone: what a pass costs with nothing to evict, the
//!   arena's allocations failing, and each asking for a pass again;
//! - `isolated`: the clean pages of a pager-backed VMO, every one of them mapped, as a guest maps
//!   its page cache, and aged into the isolate queue, older than any other: what a pass's
//!   evictions cost, as it finds them at once;
//! - `young`: as `isolated`, but none aged, as a page cache churning faster than aging isolates
//!   it: what finding the oldest of them costs too.
//!
//! ```sh
//! cargo run --release -p zircon --example arena-eviction-bench [PAGES [GIB [PASSES]]]
//! ```
//!
//! PAGES is `anonymous` by default, GIB the size of the arena (default 4), and PASSES how many
//! passes are made (default 16). Prints the pages a pass visited and evicted, and the passes' mean
//! and slowest times.

#[path = "support/pager.rs"]
mod pager;

use std::time::{Duration, Instant};
use zircon::boot_options::BootOptions;
use zircon::hypervisor::aspace::GuestPhysicalAspace;
use zircon::vm::arch_vm_aspace::{ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE};
use zircon::vm::evictor::Evictor;
use zircon::vm::page::VmPageDoublyLinkedList;
use zircon::vm::page_queues::NUM_RECLAIM;
use zircon::vm::physmap::CHUNK_SIZE;
use zircon::vm::pmm::{self, PmmOptDelayReuse, RELIEF_BYTES};
use zircon::vm::vm_address_region::VMAR_FLAG_SPECIFIC;

/// An hour, in milliseconds: longer than the benchmark runs, for the queues not to age meanwhile.
const HOUR_MS: u32 = 3_600_000;

fn main() {
    let mut args = std::env::args().skip(1);
    let holding = args.next().unwrap_or_else(|| "anonymous".into());
    let gib: usize = args.next().map_or(4, |n| n.parse().unwrap());
    let passes: u32 = args.next().map_or(16, |n| n.parse().unwrap());
    let bytes = gib << 30;

    BootOptions::set(BootOptions {
        // Room for the arena's pages, and the default arena's.
        memory_bytes: (bytes + 4 * CHUNK_SIZE) as u64,
        // Nothing but the passes evicts the arena's pages, or ages them, while the benchmark runs.
        oom_enabled: false,
        page_scanner_min_aging_interval_ms: HOUR_MS,
        page_scanner_max_aging_interval_ms: HOUR_MS,
        ..Default::default()
    })
    .unwrap();
    let node = pmm::node();
    // Its spare would be one more chunk of the arena's, of free pages.
    node.keep_spare_chunks(false);
    let evictor = node.evictor();
    evictor.enable_eviction(false);
    let arena = pmm::create_arena();
    let mut anonymous = VmPageDoublyLinkedList::new();
    let mut mapped = None;
    match holding.as_str() {
        "anonymous" => node.alloc_pages_in(arena, bytes / page::SIZE, 0, &mut anonymous).unwrap(),
        "isolated" | "young" => {
            let vmo = pmm::with_arena(arena, || {
                let vmo = pager::vmo(bytes as u64);
                pager::supply(&vmo, 0, &pager::committed(bytes as u64));
                vmo
            });
            let gpas = GuestPhysicalAspace::create(bytes).unwrap();
            let flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;
            let (mapping, _) = (gpas.root_vmar())
                .create_vm_mapping(0, bytes, 0, VMAR_FLAG_SPECIFIC, vmo, 0, flags, "cache")
                .unwrap();
            mapping.map_range(0, bytes, false, false).unwrap();
            if holding == "isolated" {
                (0..NUM_RECLAIM).for_each(|_| node.page_queues().rotate_reclaim_queues());
            }
            mapped = Some(gpas);
        }
        _ => panic!("PAGES is anonymous, isolated or young, not {holding}"),
    }

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
    unsafe { node.free_list(&mut anonymous, PmmOptDelayReuse::Default) };
    drop(mapped);

    let millis = |time: Duration| time.as_secs_f64() * 1e3;
    println!(
        "{gib} GiB arena of {holding} pages: a pass visited {} pages and evicted {}, in {:.3} ms on \
         average, the slowest {:.3} ms",
        visited / u64::from(passes),
        evicted / u64::from(passes),
        millis(times.iter().sum::<Duration>() / passes),
        millis(*times.last().unwrap()),
    );
}
