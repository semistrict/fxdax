// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a vCPU's accesses that fail, as `KVM_RUN` does with `EFAULT` where a DAX window's memory
//! is inaccessible, and when the VMM gives up on them (TASK-187).
//!
//! The second stage makes memory inaccessible for a moment as it changes it, so that a fault
//! between its system calls fails rather than resolving wrongly (`dax::uffd`): the VMM retries the
//! access, which goes through once the change is done. It makes it so for good where the guest
//! strays, its access to a page the window maps nothing at withheld, or a store past its private
//! pages refused: the access never goes through, and the VMM stops the VM, the guest's doing
//! (Firecracker's exit status 159).
//!
//! An access is given up on once it has failed for [`AccessFailures::PATIENCE`], each failure
//! within [`AccessFailures::IN_A_ROW`] of the last. Other exits of the vCPU between them don't end
//! the run: a vCPU retrying an access takes the guest's interrupts each time it enters it, whose
//! handlers exit to the VMM, a virtio device's reading its interrupt status. Firecracker ended a
//! run at any such exit once, and the vCPU taking the devices' interrupts failed a dozen times,
//! exited, failed again, for as long as the guest ran, never giving up (GCE r621, r622, r641).

use std::time::{Duration, Instant};

/// fxdax: a vCPU's accesses failing (see the module docs): the run of them it is in, if any.
#[derive(Debug, Default)]
pub struct AccessFailures {
    run: Option<FailingRun>,
}

/// fxdax: a run of a vCPU's accesses failing, each within [`AccessFailures::IN_A_ROW`] of the
/// last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailingRun {
    /// When the first failed.
    pub first: Instant,
    /// When the last failed.
    pub last: Instant,
    /// How many failed.
    pub failures: u64,
    /// How many of the vCPU's runs among them exited otherwise.
    pub exits: u64,
}

/// fxdax: what a vCPU does with an access that failed ([`AccessFailures::failed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Retries it: the window may be changing its memory.
    Retry {
        /// The run of failures before it, if this one is too long after it to be the same
        /// access's, which ended it.
        ended: Option<FailingRun>,
    },
    /// Gives up on it, the guest's access refused: it has failed for [`AccessFailures::PATIENCE`].
    GiveUp(FailingRun),
}

impl AccessFailures {
    /// How long a vCPU retries an access that keeps failing.
    pub const PATIENCE: Duration = Duration::from_secs(1);
    /// How soon after the last a failure is of the same run.
    pub const IN_A_ROW: Duration = Duration::from_millis(10);

    /// The vCPU's access failed at `now`: whether to retry it or give up on it. On Linux, a retry
    /// has the second stages lift what they restricted for workers' writes first
    /// ([`guest_access_failed`](crate::guest_access_failed)), so that a store they kept from
    /// faulting faults as one.
    pub fn failed(&mut self, now: Instant) -> Failure {
        let ended = self.run.take_if(|run| now - run.last >= Self::IN_A_ROW);
        let run =
            self.run.get_or_insert(FailingRun { first: now, last: now, failures: 0, exits: 0 });
        (run.last, run.failures) = (now, run.failures + 1);
        if now - run.first >= Self::PATIENCE {
            return Failure::GiveUp(*run);
        }
        #[cfg(target_os = "linux")]
        crate::guest_access_failed();
        Failure::Retry { ended }
    }

    /// The vCPU ran, exiting otherwise than failing, at `now`, read only if it is in a run: the
    /// exit counted among the run's failures, or the run, if it has ended.
    pub fn ran(&mut self, now: impl FnOnce() -> Instant) -> Option<FailingRun> {
        let run = self.run.as_mut()?;
        if now() - run.last < Self::IN_A_ROW {
            run.exits += 1;
            return None;
        }
        self.run.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What [`AccessFailures`] answers of a vCPU's access that never goes through, from `start`,
    /// failing every `step`, the vCPU exiting `exits` times between each two failures: its give
    /// up, a run it said ended, or, once `until` has passed, its retry.
    fn stays_refused(start: Instant, step: Duration, exits: u64, until: Duration) -> Failure {
        let mut failures = AccessFailures::default();
        let mut now = start;
        loop {
            match failures.failed(now) {
                Failure::Retry { ended: None } if now - start < until => {}
                answer => return answer,
            }
            for _ in 0..exits {
                assert_eq!(failures.ran(|| now), None, "a run ended {:?} in", now - start);
            }
            now += step;
        }
    }

    /// An access that stays refused is given up on once it has failed for a second, failing every
    /// 30 µs, with no other exit between: 33,334 failures.
    #[test]
    fn an_access_that_stays_refused_is_given_up_on() {
        let start = Instant::now();
        let step = Duration::from_micros(30);
        let answer = stays_refused(start, step, 0, 2 * AccessFailures::PATIENCE);
        let last = start + 33_334 * step;
        assert_eq!(
            answer,
            Failure::GiveUp(FailingRun { first: start, last, failures: 33_335, exits: 0 })
        );
    }

    /// TASK-187: so it is with the vCPU exiting between its failures, as the guest's interrupt
    /// handlers have it, a virtio device's: thirteen exits between each two failures, 25 µs apart,
    /// as vCPU 0's in GCE r621, which failed 13 times, ran on, failed again, for as long as the
    /// guest ran.
    #[test]
    fn exits_between_failures_end_no_run() {
        let start = Instant::now();
        let step = Duration::from_micros(25);
        let answer = stays_refused(start, step, 13, 2 * AccessFailures::PATIENCE);
        let (failures, last) = (40_001, start + 40_000 * step);
        let run = FailingRun { first: start, last, failures, exits: 13 * (failures - 1) };
        assert_eq!(answer, Failure::GiveUp(run));
    }

    /// Accesses failing for a moment, as the window changes their pages, then going through, are
    /// retried for as long as they come: a failure every 10 ms, the vCPU running between, is a run
    /// of its own each time, never given up on.
    #[test]
    fn failures_apart_are_runs_of_their_own() {
        let mut failures = AccessFailures::default();
        let start = Instant::now();
        assert_eq!(failures.ran(|| unreachable!("the clock read in no run")), None);
        assert_eq!(failures.failed(start), Failure::Retry { ended: None });
        let first = FailingRun { first: start, last: start, failures: 1, exits: 0 };
        let mut now = start + Duration::from_micros(1);
        assert_eq!(failures.ran(|| now), None, "a run ended a microsecond in");
        now = start + AccessFailures::IN_A_ROW;
        assert_eq!(failures.ran(|| now), Some(FailingRun { exits: 1, ..first }));
        assert_eq!(failures.ran(|| unreachable!("the clock read in no run")), None);
        // Each failure, IN_A_ROW after the last, ends the run that one began, for ten seconds.
        let mut last = now;
        assert_eq!(failures.failed(last), Failure::Retry { ended: None });
        for _ in 0..1000 {
            now = last + AccessFailures::IN_A_ROW;
            let ended = FailingRun { first: last, last, failures: 1, exits: 0 };
            assert_eq!(failures.failed(now), Failure::Retry { ended: Some(ended) });
            last = now;
        }
    }
}
