// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What paging in a sealed extent costs the extent cache's pager (TASK-92.19), as a fork's cold
//! boot and its refaults page in its base's extents: extents written to a store in a temporary
//! directory and sealed by a commit, each of a few parts (`extent_frame::PART`, 1 MiB), mapped
//! into a DAX window in this process's memory, and read a part at a time, by a fault on each
//! part's first page, from each extent's end so that nothing is read ahead. Each fault pages its
//! part in: the part read from the store, unpacked if it is stored compressed, into the pager's
//! buffer, whose pages are then supplied to the extent's VMO, and mapped. Data of two kinds: half
//! its bits random, which the store keeps compressed, about half as large; and random, which it
//! keeps raw, as it does not compress.
//!
//! ```sh
//! cargo run --release -p fxdax --example pagein-bench [MIB [ROUNDS]]
//! ```
//!
//! MIB is how much data of each kind (default 256), ROUNDS how many times it is read, each of a
//! store of its own (default 3). Prints the time a part's page-in takes, and the CPU time the
//! process spends on it, on every thread, which other work on the host disturbs less: the median of
//! each kind's over the rounds.

use futures::executor::block_on;
use fxdax::{Access, ExtentId, ExtentStore, VolatileSlice};
use std::time::Instant;

/// The parts of an extent.
const PARTS: u64 = 4;

/// A run of `len` bytes, as random as `mask` lets each be.
fn data(seed: u64, len: usize, mask: u8) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8 & mask
        })
        .collect()
}

/// The CPU time the process has spent, on every thread, in microseconds.
fn cpu_micros() -> f64 {
    // SAFETY: getrusage writes the usage it returns into `usage`.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, &mut usage), 0);
        usage
    };
    let micros = |time: libc::timeval| time.tv_sec as f64 * 1e6 + time.tv_usec as f64;
    micros(usage.ru_utime) + micros(usage.ru_stime)
}

/// Reads `mib` of data as random as `mask` lets each byte be, a part at a time, from a store of its
/// own, and returns the time a part took, and the CPU time.
fn round(mib: u64, mask: u8) -> (f64, f64) {
    let part = extent_frame::PART;
    let extents = (mib << 20).div_ceil(PARTS * part);
    let len = PARTS * part;
    let dir = tempfile::tempdir().unwrap();
    let store = ExtentStore::create(dir.path().join("store"), 2 * extents * len).unwrap();
    for sequence in 1..=extents {
        let extent = ExtentId { branch: store.branch(), sequence };
        let bytes = data(sequence, len as usize, mask);
        store.write(extent, 0, &[VolatileSlice::from(&bytes[..])]).unwrap();
    }
    let window = store.create_dax_window(extents * len, None).unwrap();
    store.commit(None).unwrap();
    for sequence in 1..=extents {
        let extent = ExtentId { branch: store.branch(), sequence };
        store.map(extent, 0, len, (sequence - 1) * len, false).unwrap();
    }
    let (start, cpu) = (Instant::now(), cpu_micros());
    block_on(async {
        for at in (0..extents * PARTS).rev() {
            window.fault(at * part, Access::Read).await.unwrap();
        }
    });
    let parts = (extents * PARTS) as f64;
    (start.elapsed().as_secs_f64() * 1e6 / parts, (cpu_micros() - cpu) / parts)
}

/// The median of `times`, and all of them, in order.
fn median(mut times: Vec<f64>) -> (f64, String) {
    times.sort_by(f64::total_cmp);
    let all: Vec<String> = times.iter().map(|time| format!("{time:.0}")).collect();
    (times[times.len() / 2], all.join(", "))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mib: u64 = args.next().map_or(256, |n| n.parse().unwrap());
    let rounds: usize = args.next().map_or(3, |n| n.parse().unwrap());
    fxdax::kernel::configure(fxdax::kernel::Config::with_memory((4 * mib) << 20)).unwrap();
    for (kind, mask) in [("compressed", 0x0f), ("raw", 0xff)] {
        let (times, cpu): (Vec<f64>, Vec<f64>) = (0..rounds).map(|_| round(mib, mask)).unzip();
        let ((time, times), (cpu, cpus)) = (median(times), median(cpu));
        println!(
            "{kind:<10} {time:>6.0} µs a part ({times}), CPU {cpu:>6.0} µs ({cpus}), median of \
             {rounds}"
        );
    }
}
