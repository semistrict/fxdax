// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the bytes this process's stores wrote to their files, by kind (TASK-310): what a host's
//! disk is asked to take of a store, each byte a guest writes once in a segment's raw file, as the
//! write-ahead log, and once more framed, in the closed segment's framed file, which object storage
//! keeps. Written through the host's page cache: what of a raw file goes before it is written back,
//! its segment framed, its pages leave dirty, never reaching the disk (the process's
//! `cancelled_write_bytes`, /proc/PID/io).

use std::sync::atomic::{AtomicU64, Ordering};

/// What a store wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A guest's writes, as it writes them: to the open segment's raw file, or to an open extent's
    /// own, if it is large from its first write.
    Raw,
    /// A commit's copies of the extents it seals, not in one run of the open segment, into it.
    Copied,
    /// Closed segments, framed.
    Framed,
    /// Extents stored in files of their own, framed as they are sealed: large ones.
    Own,
    /// The segments' logs of the commits in them, and the write-ahead log's base.
    Log,
}

/// The bytes this process's stores wrote, by kind ([`written`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub raw: u64,
    pub copied: u64,
    pub framed: u64,
    pub own: u64,
    pub log: u64,
}

impl Written {
    /// Each kind, by its name in the extent daemon's stats.
    pub fn named(&self) -> [(&'static str, u64); 5] {
        [
            ("raw", self.raw),
            ("copied", self.copied),
            ("framed", self.framed),
            ("own", self.own),
            ("log", self.log),
        ]
    }
}

static WRITTEN: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// A store of this process's wrote `bytes` of `kind`.
pub(crate) fn wrote(kind: Kind, bytes: usize) {
    WRITTEN[kind as usize].fetch_add(bytes as u64, Ordering::Relaxed);
}

/// The bytes this process's stores wrote since it started, by kind.
pub fn written() -> Written {
    let load = |kind: Kind| WRITTEN[kind as usize].load(Ordering::Relaxed);
    Written {
        raw: load(Kind::Raw),
        copied: load(Kind::Copied),
        framed: load(Kind::Framed),
        own: load(Kind::Own),
        log: load(Kind::Log),
    }
}
