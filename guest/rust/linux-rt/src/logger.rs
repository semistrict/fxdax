// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A logger for the `log` crate, which prints with printk.

use crate::ffi;
use core::fmt::Write;
use log::{Level, LevelFilter, Log, Metadata, Record};

/// The most a message prints: longer ones are cut short.
const MAX_MESSAGE: usize = 512;

struct Printk;

impl Log for Printk {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        let level = match record.level() {
            Level::Error => 3,
            Level::Warn => 4,
            Level::Info => 6,
            Level::Debug | Level::Trace => 7,
        };
        let mut message = Message::new();
        let _ = write!(message, "{}", record.args());
        // SAFETY: Prints the message's bytes.
        unsafe { ffi::fxfs_rt_log(level, message.bytes().as_ptr().cast(), message.bytes().len()) };
    }

    fn flush(&self) {}
}

/// Logs with printk, at `level` and above.
pub fn init_logger(level: LevelFilter) {
    static PRINTK: Printk = Printk;
    // Only the first call sets the logger; later ones change only the level.
    let _ = log::set_logger(&PRINTK);
    log::set_max_level(level);
}

/// A message formatted without allocating, cut short at `MAX_MESSAGE` bytes.
pub(crate) struct Message {
    bytes: [u8; MAX_MESSAGE],
    len: usize,
}

impl Message {
    pub(crate) const fn new() -> Self {
        Self { bytes: [0; MAX_MESSAGE], len: 0 }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for Message {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(MAX_MESSAGE - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
