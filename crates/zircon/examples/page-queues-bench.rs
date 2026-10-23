// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What a pager-backed page's moves into and out of the page queues' reclaim queues cost, which
//! count the pages each chunk of physical memory has in them (`PageQueues`, TASK-79.76): a VMO of
//! an arena's, of MIB MiB, has its pages supplied (each enters the reclaim queues), a byte of each
//! written (each leaves them, dirty), written back (each enters them again, clean) and evicted
//! (each leaves them, and is freed), ROUNDS times.
//!
//! ```sh
//! cargo run --release -p zircon --example page-queues-bench [MIB [ROUNDS]]
//! ```
//!
//! MIB is the size of the VMO (default 256), and ROUNDS how many rounds are made (default 32).
//! Prints each step's time a page, the median of the rounds' and the fastest. The pages supplied
//! are committed to an anonymous VMO before the step, which takes them from it.

#[path = "support/pager.rs"]
mod pager;

use std::time::{Duration, Instant};
use zircon::boot_options::BootOptions;
use zircon::vm::physmap::CHUNK_SIZE;
use zircon::vm::pmm;

/// An hour, in milliseconds: longer than the benchmark runs, for the queues not to age meanwhile.
const HOUR_MS: u32 = 3_600_000;

/// The steps of a round, in order.
const STEPS: [&str; 4] = ["supply", "write", "write back", "evict"];

fn main() {
    let mut args = std::env::args().skip(1);
    let mib: usize = args.next().map_or(256, |n| n.parse().unwrap());
    let rounds: usize = args.next().map_or(32, |n| n.parse().unwrap());
    let len = (mib << 20) as u64;
    let pages = len / page::SIZE as u64;

    BootOptions::set(BootOptions {
        // Room for the VMO's pages and the anonymous VMO's they come from, and the default arena's.
        memory_bytes: 2 * len + 4 * CHUNK_SIZE as u64,
        // Nothing but the rounds evicts the VMO's pages, or ages them, while the benchmark runs.
        oom_enabled: false,
        page_scanner_min_aging_interval_ms: HOUR_MS,
        page_scanner_max_aging_interval_ms: HOUR_MS,
        ..Default::default()
    })
    .unwrap();
    let arena = pmm::create_arena();
    let vmo = pmm::with_arena(arena, || pager::vmo(len));

    let mut times: [Vec<Duration>; STEPS.len()] = Default::default();
    for _ in 0..rounds {
        let committed = pmm::with_arena(arena, || pager::committed(len));
        let steps: [&dyn Fn(); STEPS.len()] = [
            &|| pager::supply(&vmo, 0, &committed),
            &|| (0..len).step_by(page::SIZE).for_each(|offset| vmo.write(&[1], offset).unwrap()),
            &|| {
                vmo.writeback_begin(0, len, false).unwrap();
                vmo.writeback_end(0, len).unwrap();
            },
            &|| assert_eq!(vmo.evict_range(0, len), Ok(pages)),
        ];
        for (step, times) in steps.iter().zip(&mut times) {
            let start = Instant::now();
            step();
            times.push(start.elapsed());
        }
    }

    let nanos = |time: Duration| time.as_secs_f64() * 1e9 / pages as f64;
    for (step, mut times) in STEPS.iter().zip(times) {
        times.sort();
        println!(
            "{step}: {:.1} ns a page, the median of {rounds} rounds of {pages} pages; the fastest \
             {:.1} ns",
            nanos(times[times.len() / 2]),
            nanos(times[0]),
        );
    }
}
