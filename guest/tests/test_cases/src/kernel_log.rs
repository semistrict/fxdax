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

/// fxdax: runs `log`, then prints what the kernel logged meanwhile, as [`print`] does: what a
/// sysrq says, without all that came before it.
pub fn print_during(log: impl FnOnce()) {
    // A reader past every record there is so far.
    let kmsg = kmsg().filter(|kmsg| {
        // SAFETY: A plain system call on an open file.
        unsafe { libc::lseek(kmsg.as_raw_fd(), 0, libc::SEEK_END) >= 0 }
    });
    log();
    for record in kmsg.map_or_else(Vec::new, read) {
        eprint!("kernel: {record}");
    }
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
