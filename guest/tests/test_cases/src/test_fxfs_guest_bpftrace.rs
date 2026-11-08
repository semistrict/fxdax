// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the guest Fxfs's tracepoints under bpftrace (TASK-44.16, docs/guest-tracepoints.md):
//! scripts/bpftrace/fxfs-latency.bt, run in a guest with a DAX window, attaches to fs/fxfs's events,
//! and while files are written, fsynced, read in sealed, loaded and stored to through a mapping and
//! fsynced again, measures the seals and the window's MAPs: it ends with a histogram of each, and
//! of the MAPs' bytes, and the bytes the seals sealed. fxfs-guest-checks' tracepoints check
//! (test_fxfs_guest_checks) says what the events are, step by step; this, that the documented
//! script runs on them.

use macros::{guest, host};

pub struct TestFxfsGuestBpftrace;

/// The script, as the docs give it.
#[cfg_attr(feature = "host", allow(dead_code))]
const SCRIPT: &str = include_str!("../../../../scripts/bpftrace/fxfs-latency.bt");

#[host]
mod host {
    use super::*;
    use crate::common::{Window, dax_window, fxdax_guest_should_run, setup_rootfs};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestSetup};

    /// Alpine's bpftrace (Apache-2.0), which the guest runs the script with, and the UAPI's
    /// headers (GPL-2.0 WITH Linux-syscall-note), which it compiles the events' formats against,
    /// the guest kernel having no BTF.
    const CONTAINERFILE: &str = "FROM alpine:3.22\nRUN apk add --no-cache bpftrace linux-headers\n";
    const CAPACITY: u64 = 128 << 20;

    impl Test for TestFxfsGuestBpftrace {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn rootfs_image(&self) -> Option<&'static str> {
            Some(CONTAINERFILE)
        }

        fn timeout_secs(&self) -> u64 {
            120
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let path = test_setup.tmp_dir.join("extents");
            let mut vm = Vm::new(2, 1024)?;
            vm.guest_kernel()?;
            vm.extent_store("extents", &path, CAPACITY, dax_window(Window::Large)?)?;
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;
    use crate::guest_fxfs::{evict, is_dax};

    use nix::libc;
    use std::ffi::CString;
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::fd::AsRawFd;
    use std::process::{Command, Stdio};

    const MNT: &str = "/mnt/fxfs";
    const TRACING: &str = "/sys/kernel/tracing";
    const PAGE: usize = 4096;
    /// What the script says once it has attached (its BEGIN).
    const READY: &str = "Tracing fxfs seals and window MAPs; Ctrl-C to end.";

    fn mount(source: &str, target: &str, fstype: &str) {
        fs::create_dir_all(target).unwrap();
        let (source, fstype) = (CString::new(source).unwrap(), CString::new(fstype).unwrap());
        let target = CString::new(target).unwrap();
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount {fstype:?}: {}", std::io::Error::last_os_error());
    }

    /// A file of `pages` pages, but for a hole of one in their midst, written and fsynced, read in
    /// again sealed, DAX, each page loaded through a shared mapping, one stored to, and fsynced
    /// again. The hole makes it two pieces, which a range of its own maps (range), rather than a
    /// slice of an extent's region, which an extent another file shares may have mapped already.
    fn workload(name: &str, pages: usize) {
        use std::io::{Seek, SeekFrom};
        let path = format!("{MNT}/{name}");
        let hole = pages / 2 * PAGE..(pages / 2 + 1) * PAGE;
        let mut data: Vec<u8> = (0..pages * PAGE).map(|i| (i % 251) as u8).collect();
        data[hole.clone()].fill(0);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&data[..hole.start]).unwrap();
        file.seek(SeekFrom::Start(hole.end as u64)).unwrap();
        file.write_all(&data[hole.end..]).unwrap();
        file.sync_all().unwrap();
        drop(file);
        evict(&path);
        assert!(is_dax(&path), "{path}, read in again once sealed, isn't DAX");
        let file = fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let len = data.len();
        let prot = libc::PROT_READ | libc::PROT_WRITE;
        let map =
            unsafe { libc::mmap(std::ptr::null_mut(), len, prot, libc::MAP_SHARED, file.as_raw_fd(), 0) };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let bytes = unsafe { std::slice::from_raw_parts_mut(map.cast::<u8>(), len) };
        for page in 0..pages {
            assert_eq!(bytes[page * PAGE], data[page * PAGE], "{path}'s page {page}");
        }
        bytes[PAGE + 1] = !data[PAGE + 1];
        file.sync_all().unwrap();
        assert_eq!(unsafe { libc::munmap(map, len) }, 0);
    }

    impl Test for TestFxfsGuestBpftrace {
        fn in_guest(self: Box<Self>) {
            if !std::path::Path::new(&format!("{TRACING}/events")).exists() {
                mount("tracefs", TRACING, "tracefs");
            }
            mount("extents", MNT, "fxfs");
            fs::write("/fxfs-latency.bt", SCRIPT).unwrap();
            let mut bpftrace = Command::new("/usr/bin/bpftrace")
                .arg("/fxfs-latency.bt")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("bpftrace");
            let mut stdout = BufReader::new(bpftrace.stdout.take().unwrap());
            let mut line = String::new();
            loop {
                line.clear();
                if stdout.read_line(&mut line).unwrap() == 0 {
                    let mut stderr = String::new();
                    bpftrace.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
                    panic!("bpftrace ended before it attached: {:?}\n{stderr}", bpftrace.wait());
                }
                if line.trim_end() == READY {
                    break;
                }
            }

            for (i, pages) in [4, 16, 64].into_iter().enumerate() {
                workload(&format!("file-{i}"), pages);
            }

            assert_eq!(unsafe { libc::kill(bpftrace.id() as i32, libc::SIGINT) }, 0);
            let mut output = String::new();
            stdout.read_to_string(&mut output).unwrap();
            let mut stderr = String::new();
            bpftrace.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
            let status = bpftrace.wait().unwrap();
            eprintln!("bpftrace's output:\n{output}\nits errors:\n{stderr}");
            assert!(status.success(), "bpftrace: {status:?}\n{stderr}");
            for what in ["@seal_us:", "@sealed_bytes:", "@map_bytes:"] {
                assert!(output.contains(what), "no {what} in bpftrace's output:\n{output}");
            }
            // The loads' MAPs, of each file's range's own pages.
            assert!(output.contains("@map_us[range]:"), "no range's MAP in:\n{output}");
            assert!(!output.contains("@map_errors"), "a MAP failed:\n{output}");
            let sealed = (output.lines())
                .find_map(|line| line.strip_prefix("@sealed_bytes: "))
                .and_then(|bytes| bytes.trim().parse::<u64>().ok())
                .unwrap_or_else(|| panic!("no bytes sealed in bpftrace's output:\n{output}"));
            // The files' pages but their holes, 3 + 15 + 63, and each a page written back again.
            assert!(sealed >= (84 * PAGE) as u64, "the seals sealed {sealed} bytes");
            println!("OK");
        }
    }
}
