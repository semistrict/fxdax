// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Shuttle tests' failing runs, each in a process of its own.
//!
//! A failing Shuttle run tears down its other tasks, closing its locks: one waiting for a lock
//! wakes to find it closed, and its panic, as it unwinds, can abort the process, which nothing in
//! it can catch. It also leaves the locks of what lives as long as the process (the in-process
//! kernel's) closed, so no run follows it there. A test that expects a run to fail (a rule turned
//! off, a schedule replayed) runs it in a child process of the test binary, and looks at how the
//! child ended, and what it said.

use std::ffi::OsStr;
use std::process::Command;

/// The environment variable that names the test a child process runs the failing part of.
const ALONE: &str = "SHUTTLE_RUN_ALONE";

/// Runs `test`, a test of this binary, in a process of its own, with `env`: whether it passed,
/// and what it said on stderr.
pub fn run_alone(test: &str, env: &[(&str, &OsStr)]) -> (bool, String) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture"])
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (output.status.success(), String::from_utf8_lossy(&output.stderr).into_owned())
}

/// The times a run that is to fail is tried at most ([`fails_alone`]).
const BATCHES: usize = 8;

/// Runs `run`, which is to fail with a panic saying `expected`, in a process of its own: the
/// calling test again, in a child process, with `env`, where this runs `run` alone. Returns what
/// the child said on stderr. A random scheduler's run may find no failing schedule, which isn't
/// that the rule turned off is not needed (TASK-55): it is tried again, each child's schedules
/// new, up to [`BATCHES`] times. (An exhaustive scheduler's run fails at once, or every time.)
pub fn fails_alone(expected: &str, env: &[(&str, &OsStr)], run: impl FnOnce()) -> String {
    let test = std::thread::current().name().expect("a test's thread is named for it").to_owned();
    if std::env::var(ALONE).as_deref() == Ok(test.as_str()) {
        run();
        return String::new();
    }
    let mut env = env.to_vec();
    env.push((ALONE, OsStr::new(&test)));
    let stderr = (0..BATCHES)
        .map(|_| run_alone(&test, &env))
        .find_map(|(passed, stderr)| (!passed).then_some(stderr))
        .unwrap_or_else(|| panic!("no run failed in {BATCHES} tries"));
    assert!(stderr.contains(expected), "expected a failure saying {expected:?}: {stderr}");
    stderr
}

/// The message of the first panic `stderr` reports, as Rust's panic hook prints it.
pub fn first_panic(stderr: &str) -> Option<&str> {
    let mut lines = stderr.lines();
    lines.find(|line| line.contains("panicked at"))?;
    lines.next()
}
