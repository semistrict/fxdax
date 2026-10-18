// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Traces of the protocol's runs under Shuttle, which TLC checks against the TLA+ spec
//! (specs/follower/FollowerTrace.tla; doc-8 testing 0 and 3, TASK-42.1's trace validation): each
//! step a run takes that the spec has an action for, as the TLA+ record the trace spec matches
//! against that action, in the order the run took them.
//!
//! A step is recorded where it is made, with no scheduling point of Shuttle's between the two: a
//! read of a key inside the lock it reads under; a step of the protocol's (a read beginning, its
//! validation, its fallback, a batch's start and end, the grace period) in the Shuttle kernel
//! (`shuttle_kernel`), right after the operation of Shuttle's it is. Readers' steps name the reader,
//! which each reader's thread says it is first ([`reader`]).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// A run's trace, while it records.
struct Recorder {
    events: Vec<String>,
    /// The readers, by their tasks.
    readers: HashMap<shuttle::current::TaskId, String>,
    /// The readers holding the follower's mutex, as their fallback: their reads are its step.
    locked: HashSet<shuttle::current::TaskId>,
}

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);

/// Starts recording a run's trace.
pub fn start() {
    let recorder = Recorder { events: Vec::new(), readers: HashMap::new(), locked: HashSet::new() };
    *RECORDER.lock().unwrap() = Some(recorder);
}

/// Stops recording, and returns the run's trace: its steps, as TLA+ records.
pub fn finish() -> Vec<String> {
    RECORDER.lock().unwrap().take().map_or_else(Vec::new, |recorder| recorder.events)
}

/// The current thread is the reader `name`.
pub fn reader(name: &str) {
    if let Some(recorder) = RECORDER.lock().unwrap().as_mut() {
        recorder.readers.insert(shuttle::current::me(), name.to_owned());
    }
}

/// Records the step `action`, with `fields`, TLA+ record fields (`f |-> "f1", e |-> 3`).
pub fn event(action: &str, fields: &str) {
    if let Some(recorder) = RECORDER.lock().unwrap().as_mut() {
        recorder.events.push(record(action, fields));
    }
}

/// Records the step `action` of the reader the current thread is, with `fields`: if it is one.
pub fn reader_event(action: &str, fields: &str) {
    let mut recorder = RECORDER.lock().unwrap();
    let Some(recorder) = recorder.as_mut() else { return };
    let Some(reader) = recorder.readers.get(&shuttle::current::me()) else { return };
    let fields = if fields.is_empty() {
        format!("r |-> \"{reader}\"")
    } else {
        format!("r |-> \"{reader}\", {fields}")
    };
    recorder.events.push(record(action, &fields));
}

/// Records the current reader's read of a key, which found `value`: unless it reads holding the
/// follower's mutex, whose step ([`locked`]) its reads are.
pub fn read_key(value: u64) {
    let locked = {
        let recorder = RECORDER.lock().unwrap();
        recorder.as_ref().is_some_and(|recorder| recorder.locked.contains(&shuttle::current::me()))
    };
    if !locked {
        reader_event("ReadKey", &format!("v |-> {value}"));
    }
}

/// The current reader takes the follower's mutex, as its fallback (`LockedRead`), or lets it go.
pub fn locked(taken: bool) {
    let me = shuttle::current::me();
    let is_reader = {
        let mut recorder = RECORDER.lock().unwrap();
        let Some(recorder) = recorder.as_mut() else { return };
        if taken {
            recorder.locked.insert(me);
        } else {
            recorder.locked.remove(&me);
        }
        recorder.readers.contains_key(&me)
    };
    if taken && is_reader {
        reader_event("LockedRead", "");
    }
}

fn record(action: &str, fields: &str) -> String {
    if fields.is_empty() {
        format!("[a |-> \"{action}\"]")
    } else {
        format!("[a |-> \"{action}\", {fields}]")
    }
}

/// `traces` as a TLA+ module of their own each, `FollowerTraceData` (the trace spec extends it),
/// in a directory of its own under `dir`, `<prefix>-<index>`, with the model's constants:
/// `readers`, `max_commits`.
pub fn write_modules(
    dir: &std::path::Path,
    prefix: &str,
    traces: &[Vec<String>],
    readers: &[&str],
    max_commits: usize,
) -> std::io::Result<()> {
    for (index, trace) in traces.iter().enumerate() {
        let run = dir.join(format!("{prefix}-{index:04}"));
        std::fs::create_dir_all(&run)?;
        let readers = readers.iter().map(|r| format!("\"{r}\"")).collect::<Vec<_>>().join(", ");
        let module = format!(
            "---- MODULE FollowerTraceData ----\n\
             \\* A run of crates/follower's Shuttle tests (shuttle_trace), generated.\n\
             TraceReaders == {{{readers}}}\n\
             TraceMaxCommits == {max_commits}\n\
             Trace == <<\n    {}\n>>\n\
             ====\n",
            trace.join(",\n    ")
        );
        std::fs::write(run.join("FollowerTraceData.tla"), module)?;
    }
    Ok(())
}
