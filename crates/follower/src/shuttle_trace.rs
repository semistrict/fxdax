// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Traces of the protocol's runs under Shuttle, which TLC checks against the TLA+ spec
//! (specs/follower/FollowerTrace.tla; doc-8 testing 0 and 3, TASK-42.1's trace validation): each
//! step a run takes that the spec has an action for, as the TLA+ record the trace spec matches
//! against that action, in the order the run took them.
//!
//! A step is recorded where it is made, with no scheduling point of Shuttle's between the two: a
//! read of a key inside the lock it reads under; a step of the protocol's (an operation taking the
//! fill barrier, a batch waiting for it, taking it and letting it go) in the Shuttle kernel
//! (`shuttle_kernel`), under the lock it is made under: an operation says what it starts first
//! ([`reader_start`]), which the kernel records as it takes the barrier. Readers' steps name the
//! reader, which each reader's thread says it is first ([`reader`]).
//!
//! It costs nothing when nothing records, as in all but the traced runs: a flag first, and fields
//! made only for a step recorded, so that the exhaustive search's runs stay as fast.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// A run's trace, while it records.
struct Recorder {
    events: Vec<String>,
    /// The readers, by their tasks.
    readers: HashMap<shuttle::current::TaskId, String>,
    /// The operation each reader starts as it takes the fill barrier: its step's action and fields.
    starting: HashMap<shuttle::current::TaskId, (String, String)>,
}

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);
/// Whether a run records: what each step looks at first.
static RECORDING: AtomicBool = AtomicBool::new(false);

fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// Starts recording a run's trace.
pub fn start() {
    let recorder =
        Recorder { events: Vec::new(), readers: HashMap::new(), starting: HashMap::new() };
    *RECORDER.lock().unwrap() = Some(recorder);
    RECORDING.store(true, Ordering::Relaxed);
}

/// Stops recording, and returns the run's trace: its steps, as TLA+ records.
pub fn finish() -> Vec<String> {
    RECORDING.store(false, Ordering::Relaxed);
    RECORDER.lock().unwrap().take().map_or_else(Vec::new, |recorder| recorder.events)
}

/// The current thread is the reader `name`.
pub fn reader(name: &str) {
    if !recording() {
        return;
    }
    if let Some(recorder) = RECORDER.lock().unwrap().as_mut() {
        recorder.readers.insert(shuttle::current::me(), name.to_owned());
    }
}

/// Records the step `action`, with the TLA+ record fields `fields` makes (`f |-> "f1", e |-> 3`).
pub fn event(action: &str, fields: impl FnOnce() -> String) {
    if !recording() {
        return;
    }
    if let Some(recorder) = RECORDER.lock().unwrap().as_mut() {
        recorder.events.push(record(action, &fields()));
    }
}

/// Records the step `action` of the reader the current thread is, with the fields `fields`
/// makes: if it is one.
pub fn reader_event(action: &str, fields: impl FnOnce() -> String) {
    if !recording() {
        return;
    }
    let mut recorder = RECORDER.lock().unwrap();
    let Some(recorder) = recorder.as_mut() else { return };
    let Some(reader) = recorder.readers.get(&shuttle::current::me()) else { return };
    let fields = fields();
    let fields = if fields.is_empty() {
        format!("r |-> \"{reader}\"")
    } else {
        format!("r |-> \"{reader}\", {fields}")
    };
    recorder.events.push(record(action, &fields));
}

/// Records the current reader's read of a key, which found `value`.
pub fn read_key(value: u64) {
    reader_event("ReadKey", || format!("v |-> {value}"));
}

/// The current reader's operation starts, as it takes the fill barrier: its step `action`, with
/// the fields `fields` makes, which [`reader_started`] records then.
pub fn reader_start(action: &str, fields: impl FnOnce() -> String) {
    if !recording() {
        return;
    }
    if let Some(recorder) = RECORDER.lock().unwrap().as_mut() {
        recorder.starting.insert(shuttle::current::me(), (action.to_owned(), fields()));
    }
}

/// The current reader took the fill barrier: the step of the operation it said it starts.
pub fn reader_started() {
    if !recording() {
        return;
    }
    let started = {
        let mut recorder = RECORDER.lock().unwrap();
        recorder.as_mut().and_then(|recorder| recorder.starting.remove(&shuttle::current::me()))
    };
    if let Some((action, fields)) = started {
        reader_event(&action, || fields);
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
