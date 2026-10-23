// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What the host refusing chunks costs the PMM's allocations (TASK-79.37), as it does once the
//! process makes as many mappings as it may (`vm.max_map_count`): here the arena's object stood in
//! for by a read-only file, which the host maps no chunk of writable. The arena allocates the pages
//! of its spare chunk, as an arena starting on its spare does, each allocation taking its page; then
//! more, each failing, as it has no free page left and the host maps it no chunk.
//!
//! ```sh
//! cargo run --release -p zircon --example pmm-refused-bench
//! ```
//!
//! Prints, for each, the allocations, their mean time, and the warnings the PMM logged meanwhile.

use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};
use zircon::boot_options::BootOptions;
use zircon::vm::physmap::CHUNK_SIZE;
use zircon::vm::pmm::{self, PmmOptDelayReuse};
use zx_status::Status;

const CHUNK_PAGES: usize = CHUNK_SIZE / page::SIZE;

/// How many allocations fail, the arena out of pages.
const FAILING: usize = 512;

fn main() {
    let memory_bytes = (16 * CHUNK_SIZE) as u64;
    BootOptions::set(BootOptions { memory_bytes, ..Default::default() }).unwrap();
    let node = pmm::node();
    let arena = pmm::create_arena();
    // A chunk and a spare; the chunk gone back, its slot of the arena's object free, which each
    // chunk asked of the host takes, so that the object is never grown meanwhile.
    let page = node.alloc_page_in(arena, 0).unwrap();
    // SAFETY: The page was just allocated, and is this function's.
    unsafe { node.free_page(page, PmmOptDelayReuse::Default) };
    assert_eq!(node.arena_bytes(arena), CHUNK_SIZE as u64, "its spare");

    let object = node.phys_memory().arena_fd(arena).unwrap();
    let writable = object.try_clone().unwrap();
    let read_only = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
    replace(object.as_raw_fd(), read_only.as_raw_fd());
    let mut pages = Vec::with_capacity(CHUNK_PAGES);
    let (spare, spare_warnings) = logged(|| {
        timed(CHUNK_PAGES, || pages.push(node.alloc_page_in(arena, 0).expect("a spare's page")))
    });
    let (failing, failing_warnings) = logged(|| {
        timed(FAILING, || assert_eq!(node.alloc_page_in(arena, 0), Err(Status::NO_MEMORY)))
    });
    replace(object.as_raw_fd(), writable.as_raw_fd());
    for page in pages {
        // SAFETY: The pages were allocated above.
        unsafe { node.free_page(page, PmmOptDelayReuse::Default) };
    }

    let micros = |time: Duration, count: usize| time.as_secs_f64() * 1e6 / count as f64;
    println!(
        "spare's pages: {CHUNK_PAGES} allocations, {:.2} µs each, {spare_warnings} warnings",
        micros(spare, CHUNK_PAGES),
    );
    println!(
        "no free page: {FAILING} allocations failing, {:.2} µs each, {failing_warnings} warnings",
        micros(failing, FAILING),
    );
}

/// Makes the descriptor `fd` the file `by` is, as `dup2` does.
fn replace(fd: i32, by: i32) {
    // SAFETY: Replaces a descriptor this process owns, which nothing uses meanwhile.
    let made = unsafe { libc::dup2(by, fd) };
    assert_eq!(made, fd, "dup2: {}", std::io::Error::last_os_error());
}

/// Runs `f` `count` times, and returns how long they took.
fn timed(count: usize, mut f: impl FnMut()) -> Duration {
    let start = Instant::now();
    for _ in 0..count {
        f();
    }
    start.elapsed()
}

/// Runs `f`, with standard error a file, and returns what it returns and how many warnings of the
/// PMM's it logged.
fn logged<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let path = std::env::temp_dir().join(format!("pmm-refused-bench-{}.log", std::process::id()));
    let log = std::fs::File::create(&path).unwrap();
    // SAFETY: Duplicates standard error, kept to put back below.
    let stderr = unsafe { libc::dup(2) };
    assert!(stderr >= 0, "dup: {}", std::io::Error::last_os_error());
    replace(2, log.as_raw_fd());
    let result = f();
    replace(2, stderr);
    // SAFETY: Closes the duplicate made above.
    unsafe { libc::close(stderr) };
    let logged = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    (result, logged.lines().filter(|line| line.starts_with("WARNING: PMM:")).count())
}
