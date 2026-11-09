// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The guest kernel's messages, which say what a filesystem logged when a guest test fails.

use nix::libc;
use std::fs;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

/// Prints the kernel's messages, from /dev/kmsg, to standard error, each as "kernel: RECORD".
pub fn print() {
    for record in records() {
        eprint!("kernel: {record}");
    }
}

/// The kernel's messages, from /dev/kmsg: each record, as its reader gets it.
pub fn records() -> Vec<String> {
    kmsg().map_or_else(Vec::new, read)
}

/// How long the kernel took from its start to running init, as its log's record of it says,
/// if the log still has it: `PRIORITY,SEQUENCE,MICROSECONDS,FLAGS;Run INIT as init process`.
pub fn init_started_ns() -> Option<u64> {
    records().iter().find_map(|record| {
        let (header, message) = record.split_once(';')?;
        if !(message.starts_with("Run ") && message.trim_end().ends_with(" as init process")) {
            return None;
        }
        Some(header.split(',').nth(2)?.parse::<u64>().ok()? * 1000)
    })
}

/// fxdax: the guest's stacks, its CPUs', blocked tasks' and every task's (sysrq l, w and t), each
/// record as "kernel: RECORD": the kernel's hung task reports name only tasks in uninterruptible
/// sleep, not one looping, or sleeping killably, holding what they wait for (load_during_move's,
/// GCE r526).
pub fn stacks() -> Vec<String> {
    // A reader past every record there is so far: what the sysrqs say, without all before them.
    let kmsg = kmsg().filter(|kmsg| {
        // SAFETY: A plain system call on an open file.
        unsafe { libc::lseek(kmsg.as_raw_fd(), 0, libc::SEEK_END) >= 0 }
    });
    for key in [b"l", b"w", b"t"] {
        let _ = fs::write("/proc/sysrq-trigger", key);
    }
    kmsg.map_or_else(Vec::new, read).into_iter().map(|record| format!("kernel: {record}")).collect()
}

fn kmsg() -> Option<fs::File> {
    fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/kmsg").ok()
}

/// Each record `kmsg` has, until there are no more.
fn read(mut kmsg: fs::File) -> Vec<String> {
    let mut record = [0u8; 8192];
    let mut records = Vec::new();
    // Each read is a record, until there are no more.
    loop {
        match kmsg.read(&mut record) {
            Ok(n) => records.push(String::from_utf8_lossy(&record[..n]).into_owned()),
            // Records it had yet to read were overwritten, as a sysrq t's many may: it reads on
            // from the oldest left.
            Err(err) if err.raw_os_error() == Some(libc::EPIPE) => {
                records.push("(records overwritten before they were read)\n".to_owned())
            }
            Err(_) => return records,
        }
    }
}
