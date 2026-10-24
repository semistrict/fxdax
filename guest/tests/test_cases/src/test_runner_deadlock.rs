// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a test that deadlocks on purpose, two threads taking two locks in opposite orders, so
//! that the runner's timeout saves its stacks (TASK-44.13): `stacks-<pid>.txt` in its directory
//! name `holds_first_then_second` and `holds_second_then_first`, each waiting. It runs only when
//! FXDAX_RUNNER_DEADLOCK=1 asks for it, and always fails, by its timeout.

use macros::{guest, host};

pub struct TestRunnerDeadlock;

#[host]
mod host {
    use super::*;
    use crate::{ShouldRun, Test, TestSetup};
    use std::sync::{Arc, Barrier, Mutex};

    #[inline(never)]
    fn holds_first_then_second(first: &Mutex<()>, second: &Mutex<()>, both: &Barrier) {
        let _first = first.lock().unwrap();
        both.wait();
        let _second = second.lock().unwrap();
    }

    #[inline(never)]
    fn holds_second_then_first(first: &Mutex<()>, second: &Mutex<()>, both: &Barrier) {
        let _second = second.lock().unwrap();
        both.wait();
        let _first = first.lock().unwrap();
    }

    impl Test for TestRunnerDeadlock {
        fn should_run(&self) -> ShouldRun {
            if std::env::var("FXDAX_RUNNER_DEADLOCK").as_deref() == Ok("1") {
                ShouldRun::Yes
            } else {
                ShouldRun::No("a deadlock on purpose: FXDAX_RUNNER_DEADLOCK=1 runs it")
            }
        }

        fn timeout_secs(&self) -> u64 {
            5
        }

        fn start_vm(self: Box<Self>, _test_setup: TestSetup) -> anyhow::Result<()> {
            let locks = Arc::new((Mutex::new(()), Mutex::new(()), Barrier::new(2)));
            let theirs = locks.clone();
            let other = std::thread::spawn(move || {
                let (first, second, both) = &*theirs;
                holds_second_then_first(first, second, both);
            });
            let (first, second, both) = &*locks;
            holds_first_then_second(first, second, both);
            other.join().unwrap();
            Ok(())
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    impl Test for TestRunnerDeadlock {}
}
