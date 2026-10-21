// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What fxdax's tests under Shuttle (awslabs/shuttle, the `shuttle` feature) share: how many
//! schedules they run, Shuttle's configuration, and how a test shows that one of the code's rules
//! is needed, by turning it off and failing as the rule prevents.

use shuttle::Config;

/// The schedules a test runs: `each`, which `SHUTTLE_SOAK` multiplies.
pub(crate) fn runs(each: usize) -> usize {
    let soak: usize = std::env::var("SHUTTLE_SOAK").map_or(1, |soak| soak.parse().unwrap());
    each * soak
}

/// Shuttle's configuration: threads' stacks as large as the kernel's calls need.
pub(crate) fn config() -> Config {
    let mut config = Config::new();
    config.stack_size = 4 << 20;
    config
}

/// The batches of random schedules a test runs at most to find what it looks for: the failure a
/// rule turned off makes ([`fails_with`]), or each of the races it is to reach.
pub(crate) const BATCHES: usize = 8;

/// Runs `test`, which turns a rule off, expecting one of its schedules to fail with a panic
/// saying `expected`. Its schedules are random: a batch may find none that fails, which isn't that
/// the rule is not needed (TASK-55), so `test` runs, its schedules new each time, until one does,
/// up to [`BATCHES`] times. The panics are collected, and printed, by a hook installed for good,
/// which Shuttle's chains to: a failing run may leave the thread counted as panicking, when a hook
/// can't be set back. So each such test must be a process of its own, as nextest runs them.
pub(crate) fn fails_with(expected: &str, test: impl Fn() + std::panic::RefUnwindSafe) {
    static PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        PANICS.lock().unwrap().push(info.payload_as_str().unwrap_or_default().to_owned());
    }));
    let failed = (0..BATCHES).any(|_| match std::panic::catch_unwind(&test) {
        Ok(()) => false,
        // Its payload may hold Shuttle's objects, which can't be dropped outside a run.
        Err(payload) => {
            std::mem::forget(payload);
            true
        }
    });
    let panics = std::mem::take(&mut *PANICS.lock().unwrap());
    assert!(failed, "no schedule failed, expecting {expected:?}");
    assert!(
        panics.iter().any(|panic| panic.contains(expected)),
        "expected {expected:?}, got {panics:#?}"
    );
}
