// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What the extent daemon's `MAP` costs a VM process, without a VM: a daemon on a thread of this
//! process, its client, and a window in this process's memory, as a VMM's extent device has them
//! but for KVM. It maps new extents writably, as the guest's Fxfs maps the staging extents small
//! files are stored in, commits after each few, as Fxfs's commits seal them, and maps the sealed
//! extents again read-only, timing each kind of request. Then the requests the guest's Fxfs makes
//! of extents not mapped: a block, and 32, written and read back (WRITE, READ), an ordered commit,
//! and the extent freed (FREE): what a request costs the daemon and its round trip, and a byte of
//! one, which the guest's REQUESTS lines, less this, leave to the device's queues and the VMM. For profiling, with perf on
//! Linux (scripts/example-gce.sh):
//!
//! ```sh
//! cargo run --release -p fxdax --example extentd-mapbench [MAPS [PAGES [MAPS_PER_COMMIT [maps]]]]
//! ```
//!
//! PAGES is the pages each map maps (default 32), MAPS_PER_COMMIT how many maps a commit
//! follows (default 8); `maps` times the maps alone, to profile them.

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
    use fxdax::{ExtentId, RemoteExtentStore, VolatileSlice, VolatileSliceMut, extentd};
    use std::os::unix::net::UnixListener;
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};

    /// The window's size: as many maps' pages as fit, reused in turn.
    const WINDOW: u64 = 1 << 30;

    /// Times each request of a kind, and of a MAP, the daemon's own time mapping
    /// (`Stats::map_ns`), and the window's changes it sent this process, as it made them
    /// (`Stats::stage_ns`): what is left is the round trip.
    #[derive(Default)]
    struct Timer {
        count: u32,
        total: Duration,
        max: Duration,
        daemon_ns: u64,
        stage_ns: u64,
    }

    impl Timer {
        fn time<T>(&mut self, f: impl FnOnce() -> T) -> T {
            let before = fxdax::stats::stats();
            let start = Instant::now();
            let value = f();
            let elapsed = start.elapsed();
            let counted = fxdax::stats::stats().since(&before);
            self.daemon_ns += counted.map_ns;
            self.stage_ns += counted.stage_ns;
            self.count += 1;
            self.total += elapsed;
            self.max = self.max.max(elapsed);
            value
        }

        fn print(&self, name: &str) {
            let count = self.count.max(1) as f64;
            let mean = self.total.as_secs_f64() * 1e6 / count;
            let max = self.max.as_secs_f64() * 1e6;
            let mut line = format!(
                "{name:<16} {:>8} requests {mean:>10.1} µs mean {max:>10.1} µs max",
                self.count
            );
            if self.daemon_ns > 0 {
                line += &format!(" {:>8.1} µs mapping", self.daemon_ns as f64 / 1e3 / count);
            }
            if self.stage_ns > 0 {
                line += &format!(" {:>8.1} µs window", self.stage_ns as f64 / 1e3 / count);
            }
            println!("{line}");
        }
    }

    pub fn main() {
        assert!(!cfg!(debug_assertions), "measure an optimized build: run with --release");
        let args: Vec<String> = std::env::args().skip(1).collect();
        let number = |at: usize, default: u64| args.get(at).map_or(default, |arg| arg.parse().unwrap());
        let (maps, pages, maps_per_commit) = (number(0, 2000), number(1, 32), number(2, 8));
        let maps_only = match args.get(3).map(String::as_str) {
            None => false,
            Some("maps") => true,
            Some(other) => panic!("{other}: the fourth argument is `maps`, or none"),
        };
        // A host of the multi-node system, as the daemon is (TASK-45): a bucket in its memory,
        // and placement 1, unless the environment names others.
        for (var, default) in [("FXDAX_EXTENTD_REMOTE", "memory:///mapbench"), ("FXDAX_EXTENTD_TOKEN", "1")] {
            if std::env::var_os(var).is_none() {
                // SAFETY: Before this process starts any other thread.
                unsafe { std::env::set_var(var, default) };
            }
        }

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
        if maps_only {
            return;
        }

        // Extents written, read back and freed, as the guest's Fxfs does those it doesn't map: a
        // block's, and a run of blocks', whose difference is what a byte costs.
        let mut sequence = maps;
        for blocks in [1, 32] {
            let (mut write, mut read, mut ordered, mut free) =
                (Timer::default(), Timer::default(), Timer::default(), Timer::default());
            let data = vec![7u8; (blocks * page_size) as usize];
            let mut back = vec![0u8; data.len()];
            for _ in 0..maps {
                sequence += 1;
                let extent = ExtentId { branch: store.branch(), sequence };
                write.time(|| store.write(extent, 0, &[VolatileSlice::from(&data[..])])).unwrap();
                read.time(|| store.read(extent, 0, &mut [VolatileSliceMut::from(&mut back[..])]))
                    .unwrap();
                assert_eq!(back, data);
                ordered.time(|| store.commit_ordered(None)).unwrap();
                free.time(|| store.free(extent)).unwrap();
            }
            let kib = blocks * page_size / 1024;
            write.print(&format!("write {kib} KiB"));
            read.print(&format!("read {kib} KiB"));
            ordered.print("commit ordered");
            free.print("free");
        }
    }
}
