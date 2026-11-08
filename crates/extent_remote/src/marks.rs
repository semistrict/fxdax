// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what a bucket's readers know of the collector's newest mark-only pass (doc-7, Garbage
//! collection; TASK-264), which warn of reaching what it marked.
//!
//! A mark-only pass deletes nothing: it writes what it would have deleted to `gc/marks/<epoch>`
//! ([`layout::Marks`]). No correct reader reaches what no live branch reached, so a read of a key
//! the newest marks name is a bug in the collector, which a pass that deleted would have turned
//! into data lost; unless the reader is one the protocol lets find it gone, and start again: a
//! recovery that, finding a segment missing, finds a newer checkpoint covering it, or a reader
//! within the grace period of an object compacted. Each such read is logged, at `warn`, with the
//! key, its kind (which names its branch), the request, the marks' epoch, and the IDs of the span
//! current if spans are on; in the extent daemon, its log names the guest request it is for too
//! (`request_id`, `fxdax::request::current`).
//!
//! A read asks one load first, whether the marks name any key, and only then one lookup in their
//! set: a bucket no mark-only pass marked costs a load a read. The marks are read again every
//! [`REFRESH`], by one read of the bucket's, alongside it (`store::Store`), never one a read.
//!
//! Listings are not warned of. A listing names what its caller passes over as well as what it goes
//! by: a branch's older checkpoints, which recovery lists past to its newest; the segments a
//! checkpoint covers, among which a writer finds its branch's last; a fork's parent's checkpoints
//! newer than the fork. Those are what the collector deletes, which their callers find gone
//! without harm. What a caller goes by of a listing, it reads, which is warned of. A marked key in
//! a listing is logged at `debug`, but the collector's census of every object, which lists every
//! marked key by design.

use crate::bucket::{Meta, Version};
use crate::layout::{self, Marks};
use crate::store::Request;
use std::collections::HashSet;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How often a bucket's readers read the newest marks again. The collector marks once a pass, every
/// so many minutes: a minute late, a reader warns of a key a new pass marked a minute late, and of
/// every key an older pass marked, still marked, at once. It costs a listing of `gc/marks/` a
/// minute a bucket, and a read of the marks only once they change: nothing to speak of beside a
/// host's reads, which it adds no round trip to.
pub const REFRESH: Duration = Duration::from_secs(60);

/// What a bucket's readers know of the newest marks: a [`store::Store`](crate::store::Store)'s.
pub(crate) struct Watch {
    /// Whether the marks held name any key: what each read asks first.
    any: AtomicBool,
    held: RwLock<Held>,
    /// What [`Watch::due`] counts from.
    started: Instant,
    /// When, in milliseconds after `started`, the marks are next read: at once, at first.
    due: AtomicU64,
}

/// The marks held.
#[derive(Default)]
struct Held {
    /// The marks object they are of, as listed: its key and version.
    object: Option<(String, Version)>,
    epoch: u64,
    keys: HashSet<Box<str>>,
}

impl Watch {
    pub(crate) fn new() -> Self {
        Self {
            any: AtomicBool::new(false),
            held: RwLock::default(),
            started: Instant::now(),
            due: AtomicU64::new(0),
        }
    }

    /// Whether the marks are due to be read again, which this call then takes on: one caller a
    /// [`REFRESH`].
    pub(crate) fn take_refresh(&self) -> bool {
        let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let due = self.due.load(Ordering::Relaxed);
        let next = now.saturating_add(REFRESH.as_millis() as u64);
        now >= due
            && self.due.compare_exchange(due, next, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }

    /// Whether the marks held are of `newest`, the newest marks object listed: at its key, of its
    /// version.
    pub(crate) fn holds(&self, newest: &Meta) -> bool {
        let held = self.held.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        held.object
            .as_ref()
            .is_some_and(|(key, version)| *key == newest.key && *version == newest.version)
    }

    /// Holds `marks`, of the object `of` lists; or none, no marks object being there.
    pub(crate) fn hold(&self, marks: Option<(&Meta, Marks)>) {
        let held = match marks {
            None => Held::default(),
            Some((of, marks)) => Held {
                object: Some((of.key.clone(), of.version.clone())),
                epoch: marks.epoch,
                keys: marks.marked.into_iter().map(|marked| marked.key.into()).collect(),
            },
        };
        let any = !held.keys.is_empty();
        *self.held.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = held;
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
        let held = self.held.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !held.keys.contains(key) {
            return;
        }
        let (epoch, kind, request) = (held.epoch, layout::kind(key), request.name());
        drop(held);
        let saying = "a reader reached what the collector's newest mark-only pass found no live \
                      branch needs, and would have deleted";
        match crate::trace::current_ids() {
            Some((trace_id, span_id)) => log::warn!(
                key, kind:?, request, marks = epoch, trace_id, span_id;
                "extent_remote: {request} {key}: {saying} (gc/marks/{epoch:016x})"
            ),
            None => log::warn!(
                key, kind:?, request, marks = epoch;
                "extent_remote: {request} {key}: {saying} (gc/marks/{epoch:016x})"
            ),
        }
    }

    /// Logs, at `debug`, the marked keys of `listed`, a listing of `prefix`; none of the census of
    /// every object, the collector's, whose prefix is empty.
    pub(crate) fn listed(&self, prefix: &str, listed: &[Meta]) {
        if prefix.is_empty() || !self.any.load(Ordering::Relaxed) {
            return;
        }
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }
        let held = self.held.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        for meta in listed.iter().filter(|meta| held.keys.contains(meta.key.as_str())) {
            let (key, epoch) = (&meta.key, held.epoch);
            log::debug!(key, prefix, marks = epoch; "extent_remote: listed {key}, marked");
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
    use crate::layout::Marked;

    fn meta(key: &str, e_tag: &str) -> Meta {
        let version = Version { e_tag: Some(e_tag.to_owned()), version: None };
        Meta { key: key.to_owned(), version, size: 3, modified: 0 }
    }

    /// The marks are read at once, then once a [`REFRESH`], by one caller.
    #[test]
    fn refreshes_are_taken_once_an_interval() {
        let watch = Watch::new();
        assert_eq!([watch.take_refresh(), watch.take_refresh()], [true, false]);
    }

    /// Marks held name their keys, and those alone; a marks object of another version is other
    /// marks; none held, nothing is marked.
    #[test]
    fn marks_held_are_of_their_object() {
        let watch = Watch::new();
        let segment = layout::segment(1, 2);
        let of = meta(&layout::marks(7), "a");
        let marked = vec![Marked::of(&meta(&segment, "s"))];
        watch.hold(Some((&of, Marks { epoch: 7, grace: 0, marked })));
        assert!(watch.any.load(Ordering::Relaxed));
        assert!(watch.held.read().unwrap().keys.contains(segment.as_str()));
        assert!(!watch.held.read().unwrap().keys.contains(layout::segment(1, 3).as_str()));
        assert_eq!([watch.holds(&of), watch.holds(&meta(&layout::marks(7), "b"))], [true, false]);
        watch.hold(None);
        assert!(!watch.any.load(Ordering::Relaxed));
        assert!(!watch.holds(&of));
    }
}
