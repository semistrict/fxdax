// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the failpoints of a fork's protocol (TASK-44.10), here and in `extent_store` and
//! `extent_remote`: a step failing, as what its function returns, partway through a fork or the
//! cleanup of one that failed. The `fail` crate's: nothing without the `failpoints` feature, which
//! this crate's tests have. Tests set them one by one, or as a seed says ([`plan`]); a daemon built
//! with them, as FAILPOINTS or FXDAX_FAULT_SEED say ([`from_seed`]).

use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};

/// A failpoint's failure, as the function its step is in gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("failpoint {0}")]
pub struct Failpoint(pub &'static str);

/// A step of a protocol, which the failpoint of its name fails: the function it is in returns
/// [`Failpoint`] there, or whatever a test set the failpoint to do happens.
macro_rules! failpoint {
    ($name:literal) => {
        fail::fail_point!($name, |_| Err($crate::faults::Failpoint($name).into()))
    };
}
pub(crate) use failpoint;

/// The steps of a fork, in the order it takes them, those that fail: finding its child's ID free
/// in the bucket, its commit naming the child, then, on the uploader's side, its segment's upload
/// and acknowledgement, which gives the child its reference under the parent's cell (TASK-395.3)
/// and its record, and the checkpoint that makes it; then
/// the child's store made, taking its state from the bucket (TASK-165.7).
pub const FORK_STEPS: [&str; 20] = [
    "fork::durable",
    "remote::child_free",
    "fork::committed",
    "fork::flushed",
    "remote::forked",
    "remote::gather",
    "writer::upload",
    "writer::uploaded",
    "writer::acknowledge",
    "writer::acknowledged",
    "branch::record_fork",
    "branch::referred",
    "branch::recorded_fork",
    "remote::acknowledged",
    "remote::released",
    "writer::checkpoint",
    "writer::checkpointed",
    "remote::checkpointed",
    "fork::recorded",
    "fork::made",
];

/// The steps of a restore: a fork of a point, in the order it takes them, on any host holding a
/// copy of it (doc-7, Points): its child's ID found free, its child's record, its reference under
/// the point's origin's cell (TASK-395.3, after the record since TASK-395.10), a read of the
/// point's, and the record made; then the child's store. No upload, as it commits nothing.
pub const RESTORE_STEPS: [&str; 10] = [
    "fork::durable",
    "remote::child_free",
    "remote::forked",
    "point::record",
    "point::recorded",
    "point::referred",
    "point::checked",
    "point::made",
    "fork::recorded",
    "fork::made",
];

/// The steps of the daemon's around a fork: a guest's request, and a fork the control API asked
/// for once its store's commits are uploaded; and the fork made, the answer to its guest lost.
pub const DAEMON_STEPS: [&str; 3] = ["extentd::fork", "extentd::fork_uploaded", "extentd::forked"];

/// The steps of a failed fork's cleanup, its child deleted in the bucket as the fork's alone, in
/// order: the first two, or the next two, by whether the child has a record.
pub const CLEANUP_STEPS: [&str; 4] = [
    "branch::claimed",
    "branch::claim_lease_ended",
    "branch::lease_ended",
    "branch::children_recorded",
];

/// One in how many steps a seed fails.
const FAILING: u64 = 8;

/// The failpoints `seed` sets, with what `fail::cfg` sets them to: one step in [`FAILING`], of a
/// fork and its cleanup, failing once, at its first, second or third pass. The same seed sets the
/// same, which the `fail` crate's chances, drawn from no seed, do not.
pub fn plan(seed: u64) -> Vec<(&'static str, String)> {
    let mut rng = StdRng::seed_from_u64(seed);
    let steps = FORK_STEPS.into_iter().chain(DAEMON_STEPS).chain(CLEANUP_STEPS);
    let chosen = steps.filter(|_| rng.random_range(0..FAILING) == 0).collect::<Vec<_>>();
    let failing = |step| (step, format!("{}*off->1*return", rng.random_range(0..3)));
    chosen.into_iter().map(failing).collect()
}

/// Sets the failpoints the seed `FXDAX_FAULT_SEED` names ([`plan`]), if it names one, saying which:
/// so a run that failed says how to fail again.
pub fn from_seed() -> Result<(), String> {
    let Ok(seed) = std::env::var("FXDAX_FAULT_SEED") else { return Ok(()) };
    let seed = seed.parse().map_err(|err| format!("FXDAX_FAULT_SEED={seed}: {err}"))?;
    for (step, actions) in plan(seed) {
        log::info!("fxdax: fault seed {seed}: {step}={actions}");
        fail::cfg(step, &actions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A daemon's seed sets its plan's failpoints, as FXDAX_FAULT_SEED names it.
    #[test]
    fn seeds_set_their_failpoints() {
        // SAFETY: nextest runs each test in a process of its own, whose other threads don't read
        // the environment yet.
        unsafe { std::env::set_var("FXDAX_FAULT_SEED", "7") };
        from_seed().unwrap();
        let mut set = fail::list();
        set.sort();
        let mut planned: Vec<_> =
            plan(7).into_iter().map(|(step, actions)| (step.to_owned(), actions)).collect();
        planned.sort();
        assert_eq!(set, planned);
    }

    /// A seed says the same failpoints each time, and each is a step, failing once.
    #[test]
    fn seeds_say_what_fails() {
        assert_eq!(plan(7), plan(7));
        let plans: Vec<_> = (0..64).map(plan).collect();
        assert_ne!(plans[0], plans[1]);
        for (step, actions) in plans.iter().flatten() {
            let known = [&FORK_STEPS[..], &DAEMON_STEPS, &CLEANUP_STEPS].concat();
            assert!(known.contains(step), "{step}");
            assert!(actions.ends_with("*off->1*return"), "{actions}");
        }
        assert!(plans.iter().map(Vec::len).sum::<usize>() > 64, "{plans:?}");
    }
}
