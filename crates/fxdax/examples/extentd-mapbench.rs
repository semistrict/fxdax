// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What the extent daemon's `MAP` costs a VM process, without a VM: a daemon on a thread of this
//! process, its client, and a window in this process's memory, as libkrun's extent device has them
//! but for KVM. It maps new extents writably, as the guest's Fxfs maps the staging extents small
//! files are stored in, commits after each few, as Fxfs's commits seal them, and maps the sealed
//! extents again read-only, timing each kind of request. For profiling, with perf on Linux:
//!
//! ```sh
//! cargo run --release -p fxdax --example extentd-mapbench [MAPS [PAGES [MAPS_PER_COMMIT]]]
//! ```
//!
//! PAGES is the pages each map maps (default 32), MAPS_PER_COMMIT how many maps a commit
//! follows (default 8).

#[cfg(target_os = "linux")]
fn main() {
    bench::main();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("the extent daemon runs on Linux only");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
mod bench {
    use fxdax::{ExtentId, RemoteExtentStore, extentd};
    use std::os::unix::net::UnixListener;
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};

    /// The window's size: as many maps' pages as fit, reused in turn.
    const WINDOW: u64 = 1 << 30;

    /// Times each request of a kind.
    #[derive(Default)]
    struct Timer {
        count: u32,
        total: Duration,
        max: Duration,
    }

    impl Timer {
        fn time<T>(&mut self, f: impl FnOnce() -> T) -> T {
            let start = Instant::now();
            let value = f();
            let elapsed = start.elapsed();
            self.count += 1;
            self.total += elapsed;
            self.max = self.max.max(elapsed);
            value
        }

        fn print(&self, name: &str) {
            let mean = self.total.as_secs_f64() * 1e6 / self.count.max(1) as f64;
            let max = self.max.as_secs_f64() * 1e6;
            println!(
                "{name:<14} {:>8} requests {mean:>10.1} µs mean {max:>10.1} µs max",
                self.count
            );
        }
    }

    pub fn main() {
        assert!(!cfg!(debug_assertions), "measure an optimized build: run with --release");
        let mut args = std::env::args().skip(1).map(|arg| arg.parse::<u64>().unwrap());
        let maps = args.next().unwrap_or(2000);
        let pages = args.next().unwrap_or(32);
        let maps_per_commit = args.next().unwrap_or(8);

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("extentd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || extentd::serve(listener));
        let store =
            RemoteExtentStore::connect(&socket, dir.path().join("store"), 64 << 30).unwrap();

        // SAFETY: Maps anonymous memory, which the window takes over.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                WINDOW as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let memory = NonNull::new(memory.cast::<u8>()).unwrap();
        // SAFETY: The memory is this process's, which nothing else uses, and it outlives the store.
        unsafe { store.create_dax_window_in_host_memory(memory, WINDOW, None) }.unwrap();

        let page_size = 4096u64;
        let len = pages * page_size;
        let slots = WINDOW / len / 2;
        let (mut writable, mut commit, mut read_only, mut unmap) =
            (Timer::default(), Timer::default(), Timer::default(), Timer::default());
        let mut sealed = Vec::new();
        for i in 0..maps {
            let extent = ExtentId { branch: store.branch(), sequence: i + 1 };
            let offset = (i % slots) * len;
            writable.time(|| store.map(extent, 0, len, offset, true)).unwrap();
            // The guest stores a small file's data in each page.
            for page in 0..pages {
                // SAFETY: The window's memory, mapped writably.
                unsafe {
                    memory.as_ptr().add((offset + page * page_size) as usize).write(page as u8)
                };
            }
            sealed.push((extent, offset));
            if (i + 1) % maps_per_commit == 0 {
                commit.time(|| store.commit(None)).unwrap();
                // Fxfs maps the extents it sealed again read-only, elsewhere in the window.
                for (extent, _) in sealed.drain(..) {
                    let at = WINDOW / 2 + (extent.sequence % slots) * len;
                    read_only.time(|| store.map(extent, 0, len, at, false)).unwrap();
                    unmap.time(|| store.unmap(at, len)).unwrap();
                }
            }
        }
        writable.print("map writable");
        commit.print("commit");
        read_only.print("map read-only");
        unmap.print("unmap");
    }
}
