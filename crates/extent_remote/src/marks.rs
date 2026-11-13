// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what a bucket's readers know of the collector's newest mark-only passes (doc-7, Garbage
//! collection; TASK-264), which warn of reaching what they marked: a pass's over each cell they
//! read (TASK-395.2).
//!
//! A mark-only pass over a cell deletes nothing: it writes what it would have deleted to
//! `<C>gc/marks/<epoch>` ([`layout::Marks`]). No correct reader reaches what no live branch reached,
//! so a read of a key the newest marks name is a bug in the collector, which a pass that deleted
//! would have turned into data lost; unless the reader is one the protocol lets find it gone, and
//! start again: a recovery that, finding a segment missing, finds a newer checkpoint covering it,
//! or a reader within the grace period of an object compacted. Each such read is logged, at `warn`,
//! with the key, its kind (which names its branch), the request, the marks' epoch, and the IDs of
//! the span current if spans are on; in the extent daemon, its log names the guest request it is
//! for too (`request_id`, `fxdax::request::current`).
//!
//! A read asks one load first, whether any marks held name a key, and only then one lookup in the
//! set of its cell's: a bucket no mark-only pass marked costs a load a read. A cell's marks are
//! read again every [`REFRESH`], alongside a read of the cell's (`store::Store`), never one a read;
//! a cell no read reaches costs nothing.
//!
//! Listings are not warned of. A listing names what its caller passes over as well as what it goes
//! by: a branch's older checkpoints, which recovery lists past to its newest; the segments a
//! checkpoint covers, among which a writer finds its branch's last; a fork's parent's checkpoints
//! newer than the fork. Those are what the collector deletes, which their callers find gone
//! without harm. What a caller goes by of a listing, it reads, which is warned of. A marked key in
//! a listing is logged at `debug`, but the collector's census of every object of a cell, which
//! lists every marked key by design.

use crate::bucket::{Meta, Version};
use crate::layout::{self, Cell, Marks};
use crate::store::Request;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How often a bucket's readers read a cell's newest marks again. The collector marks once a pass,
/// every so many minutes: a minute late, a reader warns of a key a new pass marked a minute late,
/// and of every key an older pass marked, still marked, at once. It costs a listing of the cell's
/// `gc/marks/` a minute for each cell read, and a read of the marks only once they change: nothing
/// to speak of beside a host's reads of the cell, which it adds no round trip to.
pub const REFRESH: Duration = Duration::from_secs(60);

/// What a bucket's readers know of the newest marks of the cells they read: a
/// [`store::Store`](crate::store::Store)'s.
pub(crate) struct Watch {
    /// Whether the marks held, of any cell, name any key: what each read asks first.
    any: AtomicBool,
    cells: RwLock<HashMap<Cell, Held>>,
    /// What a cell's [`Held::due`] counts from.
    started: Instant,
}

/// The marks held of a cell.
#[derive(Default)]
struct Held {
    /// The marks object they are of, as listed: its key and version.
    object: Option<(String, Version)>,
    epoch: u64,
    keys: HashSet<Box<str>>,
    /// When, in milliseconds after the watch started, they are next read.
    due: u64,
}

impl Watch {
    pub(crate) fn new() -> Self {
        Self { any: AtomicBool::new(false), cells: RwLock::default(), started: Instant::now() }
    }

    /// Whether `cell`'s marks are due to be read again, which this call then takes on: one caller
    /// a [`REFRESH`]; at once, for a cell not read before.
    pub(crate) fn take_refresh(&self, cell: Cell) -> bool {
        let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let next = now.saturating_add(REFRESH.as_millis() as u64);
        let due = |cells: &HashMap<Cell, Held>| cells.get(&cell).is_none_or(|held| now >= held.due);
        if !due(&self.cells.read().unwrap_or_else(|poisoned| poisoned.into_inner())) {
            return false;
        }
        let mut cells = self.cells.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !due(&cells) {
            return false;
        }
        cells.entry(cell).or_default().due = next;
        true
    }

    /// Whether the marks held of `cell` are of `newest`, the newest marks object listed of it: at
    /// its key, of its version.
    pub(crate) fn holds(&self, cell: Cell, newest: &Meta) -> bool {
        let cells = self.cells.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        cells
            .get(&cell)
            .and_then(|held| held.object.as_ref())
            .is_some_and(|(key, version)| *key == newest.key && *version == newest.version)
    }

    /// Holds `marks` of `cell`, of the object `of` lists; or none, no marks object being there.
    pub(crate) fn hold(&self, cell: Cell, marks: Option<(&Meta, Marks)>) {
        let mut cells = self.cells.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let held = cells.entry(cell).or_default();
        let due = held.due;
        *held = match marks {
            None => Held { due, ..Held::default() },
            Some((of, marks)) => Held {
                object: Some((of.key.clone(), of.version.clone())),
                epoch: marks.epoch,
                keys: marks.marked.into_iter().map(|marked| marked.key.into()).collect(),
                due,
            },
        };
        let any = cells.values().any(|held| !held.keys.is_empty());
        self.any.store(any, Ordering::Relaxed);
    }

    /// Warns if `key`, which a request of kind `request` reads, is marked.
    #[inline]
    pub(crate) fn reached(&self, request: Request, key: &str) {
        if self.any.load(Ordering::Relaxed) {
            self.reached_marked(request, key);
        }
    }

    #[cold]
    fn reached_marked(&self, request: Request, key: &str) {
        let Some(cell) = layout::cell_of(key) else { return };
        let cells = self.cells.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(held) = cells.get(&cell).filter(|held| held.keys.contains(key)) else { return };
        let (epoch, kind, request) = (held.epoch, layout::kind(key), request.name());
        drop(cells);
        let marks = layout::marks(cell, epoch);
        let saying = "a reader reached what the collector's newest mark-only pass found no live \
                      branch needs, and would have deleted";
        match crate::trace::current_ids() {
            Some((trace_id, span_id)) => log::warn!(
                key, kind:?, request, marks = epoch, trace_id, span_id;
                "extent_remote: {request} {key}: {saying} ({marks})"
            ),
            None => log::warn!(
                key, kind:?, request, marks = epoch;
                "extent_remote: {request} {key}: {saying} ({marks})"
            ),
        }
    }

    /// Logs, at `debug`, the marked keys of `listed`, a listing of `prefix`; none of the census of
    /// every object of a cell, the collector's, whose prefix is the cell's whole.
    pub(crate) fn listed(&self, prefix: &str, listed: &[Meta]) {
        if !self.any.load(Ordering::Relaxed) || layout::cell_of_prefix(prefix).is_some() {
            return;
        }
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }
        let cells = self.cells.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        for meta in listed {
            let Some(held) = layout::cell_of(&meta.key).and_then(|cell| cells.get(&cell)) else {
                continue;
            };
            if held.keys.contains(meta.key.as_str()) {
                let (key, epoch) = (&meta.key, held.epoch);
                log::debug!(key, prefix, marks = epoch; "extent_remote: listed {key}, marked");
            }
        }
    }
}

/// What the tests' log took, each record a line: its level, its message, then its key-values,
/// ` key=value` each. Logs from the first call on, at `info` and graver, once a process, as a log
/// is (nextest runs each test in a process of its own).
#[cfg(test)]
pub(crate) fn captured() -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    use std::sync::{Arc, Mutex, OnceLock};

    struct Captured(Arc<Mutex<Vec<String>>>);

    struct Pairs<'a>(&'a mut String);

    impl<'kvs> log::kv::VisitSource<'kvs> for Pairs<'_> {
        fn visit_pair(
            &mut self,
            key: log::kv::Key<'kvs>,
            value: log::kv::Value<'kvs>,
        ) -> Result<(), log::kv::Error> {
            self.0.push_str(&format!(" {key}={value}"));
            Ok(())
        }
    }

    impl log::Log for Captured {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Info
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                let mut line = format!("{} {}", record.level(), record.args());
                record.key_values().visit(&mut Pairs(&mut line)).unwrap();
                self.0.lock().unwrap().push(line);
            }
        }

        fn flush(&self) {}
    }

    static LINES: OnceLock<Arc<Mutex<Vec<String>>>> = OnceLock::new();
    LINES
        .get_or_init(|| {
            let lines = Arc::default();
            log::set_boxed_logger(Box::new(Captured(Arc::clone(&lines))))
                .expect("the test's process has no log yet");
            log::set_max_level(log::LevelFilter::Info);
            lines
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Marked, TEST_CELL, test_branch};

    fn meta(key: &str, e_tag: &str) -> Meta {
        let version = Version { e_tag: Some(e_tag.to_owned()), version: None };
        Meta { key: key.to_owned(), version, size: 3, modified: 0 }
    }

    /// A cell's marks are read at once, then once a [`REFRESH`], by one caller; another cell's apart.
    #[test]
    fn refreshes_are_taken_once_an_interval_a_cell() {
        let watch = Watch::new();
        let other = Cell { tenant: 1, id: 2 };
        assert_eq!([watch.take_refresh(TEST_CELL), watch.take_refresh(TEST_CELL)], [true, false]);
        assert_eq!([watch.take_refresh(other), watch.take_refresh(other)], [true, false]);
    }

    /// Marks held name their keys, of their cell's, and those alone; a marks object of another
    /// version is other marks; none held, nothing is marked. Another cell's held are its own.
    #[test]
    fn marks_held_are_of_their_object() {
        let watch = Watch::new();
        let segment = layout::segment(test_branch(1), 2);
        let of = meta(&layout::marks(TEST_CELL, 7), "a");
        let marked = vec![Marked::of(&meta(&segment, "s"))];
        watch.hold(TEST_CELL, Some((&of, Marks { epoch: 7, grace: 0, marked })));
        assert!(watch.any.load(Ordering::Relaxed));
        let held = |key: &str| {
            let cells = watch.cells.read().unwrap();
            layout::cell_of(key)
                .and_then(|cell| cells.get(&cell))
                .is_some_and(|held| held.keys.contains(key))
        };
        assert!(held(&segment));
        assert!(!held(&layout::segment(test_branch(1), 3)));
        let other = Cell { tenant: 1, id: 2 };
        let elsewhere = layout::segment(layout::Branch { cell: other, id: 1 }, 2);
        assert!(!held(&elsewhere));
        assert_eq!(
            [
                watch.holds(TEST_CELL, &of),
                watch.holds(TEST_CELL, &meta(&layout::marks(TEST_CELL, 7), "b"))
            ],
            [true, false]
        );
        assert!(!watch.holds(other, &of));
        watch.hold(other, None);
        assert!(watch.any.load(Ordering::Relaxed), "the other cell's marks, none, leave these");
        watch.hold(TEST_CELL, None);
        assert!(!watch.any.load(Ordering::Relaxed));
        assert!(!watch.holds(TEST_CELL, &of));
    }
}
