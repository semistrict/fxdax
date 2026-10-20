// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The guest kernel's messages, which say what a filesystem logged when a guest test fails.

use nix::libc;
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;

/// Prints the kernel's messages, from /dev/kmsg, to standard error, each as "kernel: RECORD".
pub fn print() {
    for record in records() {
        eprint!("kernel: {record}");
    }
}

/// The kernel's messages, from /dev/kmsg: each record, as its reader gets it.
pub fn records() -> Vec<String> {
    let Ok(mut kmsg) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/dev/kmsg")
    else {
        return Vec::new();
    };
    let mut record = [0u8; 8192];
    let mut records = Vec::new();
    // Each read is a record, until there are no more.
    while let Ok(n) = kmsg.read(&mut record) {
        records.push(String::from_utf8_lossy(&record[..n]).into_owned());
    }
    records
}
