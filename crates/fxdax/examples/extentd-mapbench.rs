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
//! one, which the guest's REQUESTS lines, less this, leave to the device's queues and the VMM.
//! Then a database's fsyncs, as the guest's Fxfs makes them (TASK-41): a staging extent mapped
//! writably, as far as Fxfs's first map of one, a page of it stored to, and a journal block
//! written with an ordered commit (`WRITE_COMMIT`), which seals the staging extent; each commit's
//! phases timed (`Stats::commits`). For profiling, with perf on Linux (scripts/example-gce.sh):
//!
//! ```sh
//! cargo run --release -p fxdax --example extentd-mapbench [MAPS [PAGES [MAPS_PER_COMMIT [maps|fsyncs|faults|all [CONNECTIONS]]]]]
//! ```
//!
//! PAGES is the pages each map maps (default 32), MAPS_PER_COMMIT how many maps a commit
//! follows (default 8); `maps` times the maps alone, `fsyncs` the fsyncs alone, PAGES then the
//! pages of the staging extent each stores to, and `faults` the faults alone, to profile them, and
//! `all` everything.
//! CONNECTIONS is how many other VM processes' connections, each of a store of its own, the daemon
//! serves meanwhile, idle, as a host's hundreds of VMs (default 0). The stores to the pages of a
//! writable map are timed too. And, last, or alone with `faults`, the loads of extents mapped
//! read-only whose pages were evicted, each extent's first a fault the daemon resolves (`FAULT`),
//! paging it in.

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
    use fxdax::{Commit, ExtentId, RemoteExtentStore, VolatileSlice, VolatileSliceMut};
    use fxdax::{WriteBarriers, extentd};
    use std::os::unix::net::UnixListener;
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};

    /// The window's size: as many maps' pages as fit, reused in turn.
    const WINDOW: u64 = 1 << 30;

    /// The pages of a staging extent the guest's Fxfs maps first (guest/linux/fs/fxfs/dax.c's
    /// FXFS_DAX_REGION_FIRST).
    const STAGING_PAGES: u64 = 16;

    /// Times each request of a kind, and of a MAP, the daemon's own time mapping
    /// (`Stats::map_ns`), and the window's changes it sent this process, as it made them
    /// (`Stats::stage_ns`): what is left is the round trip. Of a commit, its phases: committing the
    /// store, sealing the window's mappings, and waiting for object storage.
    #[derive(Default)]
    struct Timer {
        count: u32,
        total: Duration,
        max: Duration,
        daemon_ns: u64,
        stage_ns: u64,
        faults: u64,
        commit_write_ns: u64,
        commit_written_bytes: u64,
        commit_store_ns: u64,
        commit_seal_ns: u64,
        commit_wait_ns: u64,
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
            self.faults += counted.faults;
            self.commit_write_ns += counted.commit_write_ns;
            self.commit_written_bytes += counted.commit_written_bytes;
            self.commit_store_ns += counted.commit_store_ns;
            self.commit_seal_ns += counted.commit_seal_ns;
            self.commit_wait_ns += counted.commit_wait_ns;
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
            if self.faults > 0 {
                let faults = self.faults as f64;
                let each = self.total.as_secs_f64() * 1e6 / faults;
                line += &format!(" {:>8.1} faults {each:>8.1} µs a fault", faults / count);
            }
            let phases = [
                ("write", self.commit_write_ns),
                ("store", self.commit_store_ns),
                ("seal", self.commit_seal_ns),
                ("wait", self.commit_wait_ns),
            ];
            if phases.iter().any(|(_, ns)| *ns > 0) {
                for (phase, ns) in phases {
                    line += &format!(" {:>8.1} µs {phase}", ns as f64 / 1e3 / count);
                }
                line += &format!(" {:>8.1} KiB", self.commit_written_bytes as f64 / 1024. / count);
            }
            println!("{line}");
        }
    }

    pub fn main() {
        assert!(!cfg!(debug_assertions), "measure an optimized build: run with --release");
        let args: Vec<String> = std::env::args().skip(1).collect();
        let number =
            |at: usize, default: u64| args.get(at).map_or(default, |arg| arg.parse().unwrap());
        let (maps, pages, maps_per_commit) = (number(0, 2000), number(1, 32), number(2, 8));
        let only = args.get(3).map(String::as_str).filter(|only| *only != "all");
        if let Some(other) = only.filter(|only| !["maps", "fsyncs", "faults"].contains(only)) {
            panic!("{other}: the fourth argument is `maps`, `fsyncs`, `faults`, `all`, or none");
        }
        let connections = number(4, 0);
        // A host of the multi-node system, as the daemon is (TASK-45): a bucket in its memory,
        // and placement 1, unless the environment names others.
        for (var, default) in
            [("FXDAX_EXTENTD_REMOTE", "memory:///mapbench"), ("FXDAX_EXTENTD_TOKEN", "1")]
        {
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
        // The other VM processes' connections, idle.
        extentd::raise_open_files_limit().unwrap();
        let _others: Vec<_> = (0..connections)
            .map(|n| {
                let path = dir.path().join(format!("other-{n}"));
                RemoteExtentStore::connect(&socket, path, 1 << 30).unwrap()
            })
            .collect();

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
        if only == Some("fsyncs") {
            fsyncs(&store, memory, maps, pages, 1);
            return;
        }
        if only == Some("faults") {
            faults(&store, memory, maps, pages, 1);
            return;
        }
        let len = pages * page_size;
        let slots = WINDOW / len / 2;
        let (mut writable, mut commit, mut read_only, mut unmap) =
            (Timer::default(), Timer::default(), Timer::default(), Timer::default());
        let mut stores = Timer::default();
        let mut sealed = Vec::new();
        for i in 0..maps {
            let extent = ExtentId { branch: store.branch(), sequence: i + 1 };
            let offset = (i % slots) * len;
            writable.time(|| store.map(extent, 0, len, offset, true)).unwrap();
            // The guest stores a small file's data in each page.
            stores.time(|| {
                for page in 0..pages {
                    // SAFETY: The window's memory, mapped writably.
                    unsafe {
                        memory.as_ptr().add((offset + page * page_size) as usize).write(page as u8)
                    };
                }
            });
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
        stores.print("stores");
        commit.print("commit");
        read_only.print("map read-only");
        unmap.print("unmap");
        if only == Some("maps") {
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
        fsyncs(&store, memory, maps, 1, sequence + 1);
        faults(&store, memory, maps, pages, sequence + 2 * maps + 1);
    }

    /// Makes `count` extents of `pages` pages each, from `sequence` on, mapped read-only, as the
    /// guest's Fxfs maps the extents of files it reads, and has their pages evicted, as a host whose
    /// memory runs low evicts them (`ShrinkMemory`): the loads of each extent's pages after that,
    /// timed, take FAULTs, which the daemon resolves, paging the extent in from the store
    /// (TASK-92.20).
    fn faults(
        store: &RemoteExtentStore,
        memory: NonNull<u8>,
        count: u64,
        pages: u64,
        sequence: u64,
    ) {
        let page_size = 4096u64;
        let len = pages * page_size;
        let slots = WINDOW / len;
        let data = vec![7u8; len as usize];
        let extents: Vec<_> = (0..count)
            .map(|i| {
                let extent = ExtentId { branch: store.branch(), sequence: sequence + i };
                store.write(extent, 0, &[VolatileSlice::from(&data[..])]).unwrap();
                (extent, (i % slots) * len)
            })
            .collect();
        store.commit(None).unwrap();
        for (extent, at) in &extents {
            store.map(*extent, 0, len, *at, false).unwrap();
        }
        extentd::memory::shrink(None, 0).unwrap();
        let mut loads = Timer::default();
        for (_, at) in &extents {
            loads.time(|| {
                for page in 0..pages {
                    // SAFETY: The window's memory, mapped read-only.
                    let at = unsafe { memory.as_ptr().add((at + page * page_size) as usize) };
                    // SAFETY: As above.
                    assert_eq!(unsafe { at.read_volatile() }, 7);
                }
            });
        }
        loads.print(&format!("faults: loads {pages}"));
    }

    /// Makes `count` fsyncs of a database's, as the guest's Fxfs makes them, of extents from
    /// `sequence` on: a staging extent mapped writably, `pages` of it stored to, and a journal
    /// block written with an ordered commit, which seals the staging extent.
    fn fsyncs(
        store: &RemoteExtentStore,
        memory: NonNull<u8>,
        count: u64,
        pages: u64,
        mut sequence: u64,
    ) {
        let page_size = 4096u64;
        let len = STAGING_PAGES.max(pages) * page_size;
        let slots = WINDOW / len;
        let (mut stage, mut commit) = (Timer::default(), Timer::default());
        let journal = vec![7u8; page_size as usize];
        let ordered = WriteBarriers { before: false, after: Some(Commit::Ordered) };
        for i in 0..count {
            let staging = ExtentId { branch: store.branch(), sequence };
            let block = ExtentId { branch: store.branch(), sequence: sequence + 1 };
            sequence += 2;
            let offset = (i % slots) * len;
            stage.time(|| store.map(staging, 0, len, offset, true)).unwrap();
            for page in 0..pages {
                // SAFETY: The window's memory, mapped writably.
                unsafe { memory.as_ptr().add((offset + page * page_size) as usize).write(i as u8) };
            }
            let data = [VolatileSlice::from(&journal[..])];
            commit.time(|| store.write_with_barriers(block, 0, &data, ordered)).unwrap();
        }
        stage.print(&format!("fsync: stage {pages}"));
        commit.print(&format!("fsync: commit {pages}"));
    }
}
