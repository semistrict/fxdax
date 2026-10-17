// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: boot profiles of base images (TASK-38). The forks of a base image read much the same
//! of it as they start: what their guests' kernels, Fxfs and services read at boot. A fork's store
//! records the units of the base's extents its page-ins read in its first [`WINDOW`], in the order
//! they were first read, up to [`MOST`] bytes, and writes them, once, as the base's profile, in the
//! store's directory (`profiles/<base branch>`): the first fork's, so that a fork whose start the
//! profile sped up does not replace it with less. A fork opened where the profile is reads its
//! units into the extent cache, behind it, as its guest boots ([`replay`]), so that its first reads
//! find them in memory, as a fork of a base another VM read already does.

use crate::ExtentId;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long after a fork's store opens its page-ins are recorded.
pub(crate) const WINDOW: Duration = Duration::from_secs(60);

/// The most bytes of units a profile holds.
pub(crate) const MOST: u64 = 1 << 30;

/// The first line of a profile.
const HEADER: &str = "fxdax boot profile 1";

/// A unit of an extent a page-in read: its extent, and its offset and length in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Unit {
    pub extent: ExtentId,
    pub offset: u64,
    pub len: u64,
}

/// The path of the profile of the base `branch` of the store whose directory is `dir`.
pub(crate) fn path(dir: &Path, branch: u64) -> PathBuf {
    dir.join("profiles").join(format!("{branch:016x}"))
}

/// The units of the profile at `path`, in the order they were first read.
pub(crate) fn read(path: &Path) -> io::Result<Vec<Unit>> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    if lines.next() != Some(HEADER) {
        return Err(invalid("not a boot profile"));
    }
    let mut units = Vec::new();
    for line in lines {
        let fields: Vec<u64> = line
            .split(' ')
            .map(|field| u64::from_str_radix(field, 16))
            .collect::<Result<_, _>>()
            .map_err(|_| invalid("a bad line"))?;
        let [branch, sequence, offset, len] = fields[..] else {
            return Err(invalid("a bad line"));
        };
        units.push(Unit { extent: ExtentId { branch, sequence }, offset, len });
    }
    Ok(units)
}

/// Records a fork's page-ins of its base's extents, for its base's profile at `path`.
pub(crate) struct Recorder {
    path: PathBuf,
    /// The fork's own branch, whose extents no other fork holds.
    own: u64,
    until: Instant,
    units: Mutex<Recorded>,
    written: AtomicBool,
}

#[derive(Default)]
struct Recorded {
    units: Vec<Unit>,
    seen: HashSet<(ExtentId, u64)>,
    bytes: u64,
}

impl Recorder {
    /// A recorder of the page-ins of the extents the fork of branch `own` inherited, for its base's
    /// profile at `path`, from now until [`WINDOW`] after.
    pub(crate) fn new(path: PathBuf, own: u64) -> Self {
        Self {
            path,
            own,
            until: Instant::now() + WINDOW,
            units: Mutex::default(),
            written: AtomicBool::new(false),
        }
    }

    /// Records that a page-in read `len` bytes at `offset` in `extent`: of a unit of an inherited
    /// extent, within the window and [`MOST`].
    pub(crate) fn record(&self, extent: ExtentId, offset: u64, len: u64) {
        // The fork's own extents, which no other fork holds, are no part of the base's profile.
        if extent.branch == self.own {
            return;
        }
        if Instant::now() > self.until || self.written.load(Ordering::Relaxed) {
            return;
        }
        let mut recorded = self.units.lock().unwrap();
        if recorded.bytes + len > MOST || !recorded.seen.insert((extent, offset)) {
            return;
        }
        recorded.bytes += len;
        recorded.units.push(Unit { extent, offset, len });
    }

    /// Writes the profile, once, unless one is there already, as what a fork recorded first.
    pub(crate) fn write(&self) -> io::Result<()> {
        if self.written.swap(true, Ordering::Relaxed) || self.path.exists() {
            return Ok(());
        }
        let recorded = self.units.lock().unwrap();
        if recorded.units.is_empty() {
            return Ok(());
        }
        let mut text = format!("{HEADER}\n");
        for unit in &recorded.units {
            let Unit { extent, offset, len } = unit;
            text.push_str(&format!(
                "{:x} {:x} {offset:x} {len:x}\n",
                extent.branch, extent.sequence
            ));
        }
        let dir = self.path.parent().expect("a profile is in a directory");
        std::fs::create_dir_all(dir)?;
        // Whole, or not at all.
        let new = self.path.with_extension("new");
        std::fs::write(&new, text)?;
        std::fs::rename(&new, &self.path)
    }
}
