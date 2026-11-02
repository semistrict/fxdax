// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A guest that writes through its DAX window more than its extent daemon's memory holds, syncing
//! nothing (TASK-150.20): the daemon can't evict an open extent's pages, nor seal them, so it refuses
//! a MAP past their budget, and the guest seals, and tries again: its writes go on, and lose
//! nothing, whichever way they came: write(2) over what the last left dead, as xfstests' generic/129
//! does, an append longer than the daemon's memory, and stores through a mapping. It told EIO
//! before.

use macros::{guest, host};

pub struct TestFxfsGuestHostMemory;

/// The daemon's memory.
#[cfg_attr(feature = "guest", allow(dead_code))]
const MEMORY: u64 = 128 << 20;

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs, spawn_daemon};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestSetup};

    const CAPACITY: u64 = 4 << 30;
    const WINDOW: u64 = 1 << 30;

    impl Test for TestFxfsGuestHostMemory {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn timeout_secs(&self) -> u64 {
            300
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let socket =
                std::env::temp_dir().join(format!("fxdax-memory-{}.sock", std::process::id()));
            spawn_daemon(&test_setup, &socket, "extentd", &[], Some(MEMORY))?;
            let mut vm = Vm::new(2, 2048)?;
            vm.guest_kernel()?;
            let path = test_setup.tmp_dir.join("extents");
            vm.extent_store_served("extents", &path, CAPACITY, WINDOW, &socket);
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;
    use nix::libc;
    use std::ffi::CString;
    use std::fs;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;

    const MNT: &str = "/mnt/fxfs";
    /// A write of generic/129's second loop (`looptest -t -b 102400`).
    const WRITE: usize = 102400;
    /// What each way writes, more than the daemon holds.
    const WRITTEN: usize = 2 * MEMORY as usize;
    const PAGE: usize = 4096;

    fn mount() {
        let (source, target, fstype) = (
            CString::new("extents").unwrap(),
            CString::new(MNT).unwrap(),
            CString::new("fxfs").unwrap(),
        );
        // SAFETY: Each is a string, as the call asks.
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn fill(buf: &mut [u8], round: usize) {
        for (at, word) in buf.chunks_mut(8).enumerate() {
            word.copy_from_slice(&((round << 32 | at) as u64).to_le_bytes()[..word.len()]);
        }
    }

    fn open(name: &str) -> fs::File {
        (fs::OpenOptions::new().create(true).truncate(true).read(true).write(true))
            .open(format!("{MNT}/{name}"))
            .unwrap()
    }

    /// As generic/129: truncates, writes, and reads back, each write's blocks new.
    fn over_what_the_last_left_dead() {
        let file = open("rewritten");
        let (mut bytes, mut read) = (vec![0; WRITE], vec![0; WRITE]);
        for round in 0..WRITTEN / WRITE {
            fill(&mut bytes, round);
            file.set_len(0).unwrap();
            file.write_all_at(&bytes, 0).unwrap_or_else(|err| panic!("write {round}: {err}"));
            file.read_exact_at(&mut read, 0).unwrap();
            assert!(read == bytes, "write {round} reads otherwise");
        }
    }

    /// An append longer than the daemon's memory, all of it read back.
    fn longer_than_memory() {
        let file = open("long");
        let mut bytes = vec![0; 1 << 20];
        for round in 0..WRITTEN / bytes.len() {
            fill(&mut bytes, round);
            let at = (round * bytes.len()) as u64;
            file.write_all_at(&bytes, at).unwrap_or_else(|err| panic!("append {round}: {err}"));
        }
        let mut read = vec![0; bytes.len()];
        for round in 0..WRITTEN / bytes.len() {
            fill(&mut bytes, round);
            file.read_exact_at(&mut read, (round * bytes.len()) as u64).unwrap();
            assert!(read == bytes, "append {round} reads otherwise");
        }
    }

    /// Stores through a mapping, each a fault, over a file longer than the daemon's memory.
    fn through_a_mapping() {
        let file = open("mapped");
        file.set_len(WRITTEN as u64).unwrap();
        // SAFETY: A new mapping of the file, which only this uses.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                WRITTEN,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        for page in (0..WRITTEN).step_by(PAGE) {
            // SAFETY: Within the mapping.
            unsafe { map.cast::<u8>().add(page).cast::<u64>().write_volatile(page as u64) };
        }
        // SAFETY: The mapping above.
        assert_eq!(unsafe { libc::munmap(map, WRITTEN) }, 0);
        let mut read = [0; 8];
        for page in (0..WRITTEN).step_by(PAGE) {
            file.read_exact_at(&mut read, page as u64).unwrap();
            assert_eq!(u64::from_le_bytes(read), page as u64, "page at {page:#x}");
        }
    }

    /// How many seals the guest made for memory (/proc/fs/fxfs/<tag>).
    fn memory_seals() -> u64 {
        let stats = fs::read_to_string("/proc/fs/fxfs/extents").unwrap();
        (stats.lines())
            .find_map(|line| line.strip_prefix("dax_memory_seals "))
            .expect("dax_memory_seals")
            .parse()
            .unwrap()
    }

    impl Test for TestFxfsGuestHostMemory {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount();
            let mut seals = memory_seals();
            for way in [over_what_the_last_left_dead, longer_than_memory, through_a_mapping] {
                way();
                let after = memory_seals();
                assert!(after > seals, "the daemon refused it no MAP");
                seals = after;
            }
            // SAFETY: A string, as the call asks.
            let target = CString::new(MNT).unwrap();
            assert_eq!(unsafe { libc::umount(target.as_ptr()) }, 0);
            println!("OK");
        }
    }
}
