// Copyright The libkrun Authors.
// Copyright 2026 The fxdax Authors: changed for fxdax's guest tests.
// SPDX-License-Identifier: Apache-2.0

//! fxdax's guest tests: VMs booted with fxdax's Firecracker and guest kernel, each a test case run
//! by the runner (`runner test --test-case NAME`), its host half here under the `host` feature and
//! its guest half, in the guest agent, under `guest`. Moved from libkrun's tests (TASK-40), whose
//! framework this is.

/// A POSIX ACL xattr's value: its version, then each entry's tag, permissions and id.
pub fn posix_acl(entries: &[(u16, u16, u32)]) -> Vec<u8> {
    let mut value = 2u32.to_le_bytes().to_vec();
    for &(tag, perm, id) in entries {
        value.extend(tag.to_le_bytes());
        value.extend(perm.to_le_bytes());
        value.extend(id.to_le_bytes());
    }
    value
}

mod test_pjdfstest;
use test_pjdfstest::TestPjdfstest;

mod test_xfstests;
use test_xfstests::TestXfstests;

mod test_fxfs_guest;
use test_fxfs_guest::TestFxfsGuest;
mod test_fxfs_guest_resize;
use test_fxfs_guest_resize::TestFxfsGuestResize;
mod test_fxfs_guest_stats;
use test_fxfs_guest_stats::TestFxfsGuestStats;
mod test_fxfs_guest_live_fork;
use test_fxfs_guest_live_fork::TestFxfsGuestLiveFork;
mod test_fxfs_guest_pgbench;
use test_fxfs_guest_pgbench::{
    Database as PgbenchDatabase, Probe, Storage as PgbenchStorage, TestFxfsGuestPgbench,
};
mod test_fxfs_guest_bigmap;
// fxdax: what extentd asks of a disk it shares with many VMs and the bucket (TASK-310).
mod test_fxfs_guest_shared_disk;
use test_fxfs_guest_shared_disk::TestFxfsGuestSharedDisk;
mod test_fxfs_guest_bucket_soak;
use test_fxfs_guest_bucket_soak::TestFxfsGuestBucketSoak;
// fxdax: sandboxes as LangSmith's CI runs them, two hosts and a bucket on one disk (TASK-330.1).
mod test_fxfs_guest_sandboxes;
use test_fxfs_guest_bigmap::TestFxfsGuestBigmap;
use test_fxfs_guest_sandboxes::TestFxfsGuestSandboxes;
// fxdax: what a VM costs its host's memory, measured when its guest asks (TASK-164.5).
mod measure;
mod test_fxfs_guest_daemon_lost;
use test_fxfs_guest_daemon_lost::TestFxfsGuestDaemonLost;
mod test_fxfs_guest_follow;
use test_fxfs_guest_follow::TestFxfsGuestFollow;
mod test_fxfs_guest_follow_remount;
use test_fxfs_guest_follow_remount::TestFxfsGuestFollowRemount;
mod test_fxfs_guest_follow_torture;
use test_fxfs_guest_follow_torture::{Mutant, TestFxfsGuestFollowTorture};
mod test_fxfs_guest_import;
use test_fxfs_guest_import::TestFxfsGuestImport;
mod test_fxfs_guest_automount;
// fxdax: the guest Fxfs's tracepoints under bpftrace (TASK-44.16).
mod test_fxfs_guest_bpftrace;
use test_fxfs_guest_bpftrace::TestFxfsGuestBpftrace;
mod test_fxfs_guest_git_fetch;
mod test_fxfs_guest_shared_volumes;
use test_fxfs_guest_automount::TestFxfsGuestAutomount;
use test_fxfs_guest_git_fetch::TestFxfsGuestGitFetch;
use test_fxfs_guest_shared_volumes::TestFxfsGuestSharedVolumes;
// fxdax: what a checkout the size of Linux's costs, shared and not (TASK-136.1).
mod test_linux_checkout;
use test_linux_checkout::{Checkout, TestLinuxCheckout};
mod test_fxfs_guest_volumes;
use test_fxfs_guest_volumes::TestFxfsGuestVolumes;
mod test_fxfs_guest_docker;
use test_fxfs_guest_docker::TestFxfsGuestDocker;
mod test_fxfs_guest_snapshot;
use test_fxfs_guest_snapshot::{
    Mode as SnapshotMode, ReadOnly as SnapshotReadOnly, TestFxfsGuestSnapshot, VOLUMES,
};
mod test_extent_guest;
use test_extent_guest::{TestExtentGuest, TestExtentGuestOtherVersion};
mod test_runner_deadlock;
use test_runner_deadlock::TestRunnerDeadlock;

#[cfg(any(feature = "host", target_os = "linux"))]
mod test_fxfs_guest_checks;
#[cfg(any(feature = "host", target_os = "linux"))]
use test_fxfs_guest_checks::{PAGE_CACHE_CHECKS, TestFxfsGuestChecks};
#[cfg(any(feature = "host", target_os = "linux"))]
mod test_fxfs_guest_script;
#[cfg(any(feature = "host", target_os = "linux"))]
use test_fxfs_guest_script::TestFxfsGuestScript;
#[cfg(any(feature = "host", target_os = "linux"))]
mod test_fxfs_guest_fsx;
#[cfg(any(feature = "host", target_os = "linux"))]
use test_fxfs_guest_fsx::TestFxfsGuestFsx;
// fxdax: hours of mixed load through every crash doc-12 names (TASK-67), its plans and verdicts in
// `soak`.
pub mod soak;
#[cfg(any(feature = "host", target_os = "linux"))]
mod test_fxfs_guest_soak;
#[cfg(any(feature = "host", target_os = "linux"))]
use test_fxfs_guest_soak::TestFxfsGuestSoak;

// The scripts of operations of fxfs-guest-checks' random_ops and fxfs-guest-script: what they are,
// the guest's running of them, and the host's replays of them in VMs.
pub mod script;
#[cfg(all(feature = "guest", target_os = "linux"))]
mod script_run;
// fxdax: a script on two filesystems at once, step by step (TASK-44.15).
#[cfg(all(feature = "guest", target_os = "linux"))]
mod differential;
#[cfg(feature = "host")]
mod script_vm;

// What the guest's checks print as they fail.
#[cfg(feature = "guest")]
mod kernel_log;

// fxdax: what the guest halves of the guest's Fxfs's tests share: evicting, DAX, a mount's counts,
// and copying on write's files read in from sealed extents (doc-13).
#[cfg(feature = "guest")]
mod guest_fxfs;

// What the guest's tests unbind the extent driver with.
#[cfg(feature = "guest")]
mod extent_driver;

#[cfg(any(feature = "guest", test))]
mod sqlite;
#[cfg(any(feature = "host", target_os = "linux"))]
mod test_fs_bench;
// fxdax: the loop-device baseline's disk keeps what a guest fsyncs (TASK-350.1).
#[cfg(any(feature = "host", target_os = "linux"))]
mod test_ext4_loop_persists;
#[cfg(any(feature = "host", target_os = "linux"))]
use test_fs_bench::{Backend as BenchBackend, TestFsBench};
#[cfg(any(feature = "host", target_os = "linux"))]
use test_ext4_loop_persists::TestExt4LoopPersists;

pub enum TestOutcome {
    Pass,
    Fail(String),
    Timeout,
    Skip(&'static str),
    Report(Box<dyn ReportImpl>),
}

pub enum ShouldRun {
    Yes,
    No(&'static str),
}

impl ShouldRun {
    /// Returns Yes unless on macOS, in which case returns No with the given reason.
    pub fn yes_unless_macos(reason: &'static str) -> Self {
        if cfg!(target_os = "macos") { ShouldRun::No(reason) } else { ShouldRun::Yes }
    }
}

/// fxdax: declares [`Tag`], each tag's variant and name, its doc its help (`runner list --help`).
macro_rules! tags {
    ($($(#[doc = $doc:literal])+ $tag:ident = $name:literal,)+) => {
        /// fxdax: the suites a case is in, which the scripts select it by (`runner list --tag`),
        /// none of them keeping a list of names: each list a script kept missed cases the others
        /// had, and a case new to the harness was in none (TASK-160.29). Every case has one at
        /// least, [`Tag::Manual`] if no suite runs it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Tag {
            $($(#[doc = $doc])+ $tag,)+
        }

        impl Tag {
            pub const ALL: &[Tag] = &[$(Tag::$tag,)+];

            /// The tag as the scripts name it.
            pub fn name(self) -> &'static str {
                match self {
                    $(Tag::$tag => $name,)+
                }
            }

            /// What it selects: its doc, on a line.
            pub fn about(self) -> &'static str {
                match self {
                    $(Tag::$tag => concat!($($doc),+).trim_start(),)+
                }
            }

            /// The tag named `name`.
            pub fn named(name: &str) -> Option<Tag> {
                Self::ALL.iter().copied().find(|tag| tag.name() == name)
            }
        }
    };
}

tags! {
    /// scripts/test-e2e.sh's cases unless it is told others, which scripts/test-gce.sh runs
    /// unless FXDAX_E2E_TEST says otherwise: the guest's Fxfs, its crash tests and checks, and
    /// pgbench, with a DAX window and without, in minutes.
    E2e = "e2e",
    /// scripts/test-guest-kernel.sh's: the guest kernel's Fxfs, as its every feature is used.
    Kernel = "kernel",
    /// The nightly's (.github/workflows/nightly.yml), as production runs them: e2e's and
    /// kernel's, every other case cheap enough on a release kernel, and xfstests.
    Nightly = "nightly",
    /// scripts/dax-gate.sh's suites, rounds of each, on its first host.
    DaxGate = "dax-gate",
    /// scripts/soak-workloads-gce.sh's crash group: the VM stopped mid-workload.
    SoakCrash = "soak-crash",
    /// scripts/soak-workloads-gce.sh's checks group: with delays and on a small window.
    SoakChecks = "soak-checks",
    /// scripts/soak-workloads-gce.sh's sanitized group, on the debug kernel.
    SoakSanitized = "soak-sanitized",
    /// scripts/soak-workloads-gce.sh's replicas group: followers, snapshots and migrations.
    SoakReplicas = "soak-replicas",
    /// scripts/soak-workloads-gce.sh's mixed group: VMs on forks of a base under mixed load for
    /// hours, through VMs, the daemon and the host crashing and uploads failing.
    SoakMixed = "soak-mixed",
    /// scripts/perf-gate-gce.sh's cases of one VM each.
    PerfSingle = "perf-single",
    /// scripts/perf-gate-gce.sh's density run, VMs on forks of one base.
    PerfDensity = "perf-density",
    /// scripts/bench.sh's cases unless FXDAX_BENCH_CASES says others.
    Bench = "bench",
    /// In no suite, run by name alone: measurements of one thing (a profile, a lock's
    /// contention, a fork's), and runner-deadlock, which fails on purpose.
    Manual = "manual",
}

/// fxdax: the cases of `cases` named by `names` or tagged with one of `tags`, each once, in their
/// order there: those a script runs (`runner list`). It fails on a name no case has.
pub fn select(
    cases: Vec<TestCase>,
    names: &[String],
    tags: &[Tag],
) -> Result<Vec<TestCase>, String> {
    if let Some(unknown) = names.iter().find(|name| cases.iter().all(|case| case.name != *name)) {
        return Err(format!("no such test case: {unknown}"));
    }
    Ok(cases
        .into_iter()
        .filter(|case| {
            names.iter().any(|name| name == case.name)
                || case.tags.iter().any(|tag| tags.contains(tag))
        })
        .collect())
}

pub fn test_cases() -> Vec<TestCase> {
    // Register your test here, with the suites it is in (fxdax: TASK-160.29):
    vec![
        TestCase::new(
            "pjdfstest-fxfs-guest",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestPjdfstest { dax: false }),
        ),
        TestCase::new(
            "pjdfstest-fxfs-guest-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestPjdfstest { dax: true }),
        ),
        // fxdax: the kernel's own filesystem suite, nightly, its known failures checked each
        // night: in no suite, it ran only when someone named it (TASK-149.20.1).
        TestCase::new(
            "xfstests-fxfs-guest",
            &[Tag::Nightly],
            Box::new(TestXfstests { dax: false }),
        ),
        TestCase::new(
            "xfstests-fxfs-guest-dax",
            &[Tag::Nightly],
            Box::new(TestXfstests { dax: true }),
        ),
        TestCase::new(
            "fxfs-guest",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuest::host_formats()),
        ),
        TestCase::new(
            "fxfs-guest-format",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuest::guest_formats()),
        ),
        TestCase::new(
            "fxfs-guest-crash",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly, Tag::SoakCrash],
            Box::new(TestFxfsGuest::crash()),
        ),
        TestCase::new(
            "fxfs-guest-resize",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestResize),
        ),
        TestCase::new(
            "fxfs-guest-stats",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestStats),
        ),
        TestCase::new(
            "fxfs-guest-shared-disk",
            &[Tag::Manual],
            Box::new(TestFxfsGuestSharedDisk { dax: false }),
        ),
        TestCase::new(
            "fxfs-guest-shared-disk-dax",
            &[Tag::Manual],
            Box::new(TestFxfsGuestSharedDisk { dax: true }),
        ),
        // fxdax: guests writing against a degraded bucket for half an hour, TASK-330's
        // qualification's (TASK-330.3).
        TestCase::new("fxfs-guest-bucket-soak", &[Tag::Manual], Box::new(TestFxfsGuestBucketSoak)),
        // fxdax: sandboxes as LangSmith's CI runs them, TASK-330's qualification's, which
        // scripts/sandboxes.sh runs with the S3 server it needs (TASK-330.1).
        TestCase::new("fxfs-guest-sandboxes", &[Tag::Manual], Box::new(TestFxfsGuestSandboxes)),
        TestCase::new(
            "fxfs-guest-live-fork",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestLiveFork),
        ),
        TestCase::new(
            "fxfs-guest-volumes",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestVolumes { dax: false }),
        ),
        TestCase::new(
            "fxfs-guest-volumes-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestVolumes { dax: true }),
        ),
        TestCase::new(
            "fxfs-guest-shared-volumes",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestSharedVolumes { dax: false }),
        ),
        TestCase::new(
            "fxfs-guest-shared-volumes-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestSharedVolumes { dax: true }),
        ),
        TestCase::new(
            "fxfs-guest-automount",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestAutomount { dax: false }),
        ),
        TestCase::new(
            "fxfs-guest-automount-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestAutomount { dax: true }),
        ),
        TestCase::new(
            "fxfs-guest-bpftrace-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestBpftrace),
        ),
        TestCase::new(
            "fxfs-guest-git-fetch",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestGitFetch),
        ),
        TestCase::new(
            "linux-checkout-ext4",
            &[Tag::Manual],
            Box::new(TestLinuxCheckout { on: Checkout::Ext4 }),
        ),
        TestCase::new(
            "linux-checkout-ext4-pmem",
            &[Tag::Manual],
            Box::new(TestLinuxCheckout { on: Checkout::Ext4Pmem }),
        ),
        TestCase::new(
            "linux-checkout-fxfs",
            &[Tag::Manual],
            Box::new(TestLinuxCheckout { on: Checkout::Fxfs }),
        ),
        TestCase::new(
            "linux-checkout-fxfs-dax",
            &[Tag::Manual],
            Box::new(TestLinuxCheckout { on: Checkout::FxfsDax }),
        ),
        TestCase::new(
            "fxfs-guest-pgbench",
            &[Tag::E2e, Tag::Nightly, Tag::PerfSingle],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::Fxfs)),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-dax",
            &[Tag::E2e, Tag::Nightly, Tag::PerfSingle],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::FxfsDax)),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-profile",
            &[Tag::Manual],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::Fxfs).probed(Probe::Profile)),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-dax-profile",
            &[Tag::Manual],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::FxfsDax).probed(Probe::Profile)),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-locks",
            &[Tag::Manual],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::Fxfs).probed(Probe::Locks)),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-nofsync",
            &[Tag::Manual],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::Fxfs).without_fsync()),
        ),
        TestCase::new(
            "pgbench-ext4-dax-pmem",
            &[Tag::PerfSingle],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::Ext4DaxPmem)),
        ),
        TestCase::new(
            "pgbench-ext4-blk-unsafe",
            &[Tag::PerfSingle],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::Ext4BlkUnsafe)),
        ),
        TestCase::new(
            "pgbench-ext4-blk-writeback",
            &[Tag::PerfSingle],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::Ext4BlkWriteback)),
        ),
        TestCase::new(
            "pgbench-ext4-loop-writeback",
            &[Tag::PerfSingle],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::Ext4LoopWriteback)),
        ),
        TestCase::new(
            "pgbench-ext4-loop-unsafe",
            &[Tag::PerfSingle],
            Box::new(TestFxfsGuestPgbench::on(PgbenchStorage::Ext4LoopUnsafe)),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-forked",
            &[Tag::PerfSingle],
            Box::new(
                TestFxfsGuestPgbench::on(PgbenchStorage::Fxfs)
                    .with_database(PgbenchDatabase::Forked),
            ),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-dax-forked",
            &[Tag::PerfSingle],
            Box::new(
                TestFxfsGuestPgbench::on(PgbenchStorage::FxfsDax)
                    .with_database(PgbenchDatabase::Forked),
            ),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-density",
            &[Tag::PerfDensity],
            Box::new(
                TestFxfsGuestPgbench::on(PgbenchStorage::Fxfs)
                    .with_database(PgbenchDatabase::Density),
            ),
        ),
        TestCase::new(
            "fxfs-guest-pgbench-dax-density",
            &[Tag::PerfDensity],
            Box::new(
                TestFxfsGuestPgbench::on(PgbenchStorage::FxfsDax)
                    .with_database(PgbenchDatabase::Density),
            ),
        ),
        TestCase::new(
            "fxfs-guest-bigmap",
            &[Tag::PerfSingle],
            Box::new(TestFxfsGuestBigmap { dax: false }),
        ),
        TestCase::new(
            "fxfs-guest-bigmap-dax",
            &[Tag::PerfSingle],
            Box::new(TestFxfsGuestBigmap { dax: true }),
        ),
        TestCase::new(
            "fxfs-guest-daemon-lost",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestDaemonLost),
        ),
        TestCase::new("runner-deadlock", &[Tag::Manual], Box::new(TestRunnerDeadlock)),
        TestCase::new(
            "fxfs-guest-follow",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestFollow { dax: false }),
        ),
        TestCase::new(
            "fxfs-guest-follow-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestFollow { dax: true }),
        ),
        TestCase::new(
            "fxfs-guest-follow-remount",
            &[Tag::Nightly],
            Box::new(TestFxfsGuestFollowRemount { dax: false }),
        ),
        TestCase::new(
            "fxfs-guest-follow-remount-dax",
            &[Tag::Nightly, Tag::SoakReplicas],
            Box::new(TestFxfsGuestFollowRemount { dax: true }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture",
            &[Tag::Kernel, Tag::Nightly, Tag::SoakReplicas],
            Box::new(TestFxfsGuestFollowTorture { dax: false, mutant: None }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-dax",
            &[Tag::Kernel, Tag::Nightly, Tag::SoakReplicas],
            Box::new(TestFxfsGuestFollowTorture { dax: true, mutant: None }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestFollowTorture { dax: false, mutant: Some(Mutant::Invalidation) }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestFollowTorture { dax: true, mutant: Some(Mutant::Invalidation) }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant-faults-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestFollowTorture {
                dax: true,
                mutant: Some(Mutant::InvalidationFaults),
            }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant-barrier-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestFollowTorture { dax: true, mutant: Some(Mutant::Barrier) }),
        ),
        TestCase::new(
            "fxfs-guest-follow-torture-mutant-nested-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestFollowTorture { dax: true, mutant: Some(Mutant::Nested) }),
        ),
        TestCase::new(
            "fxfs-guest-import",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestImport { dax: false }),
        ),
        TestCase::new(
            "fxfs-guest-import-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestImport { dax: true }),
        ),
        TestCase::new(
            "fxfs-guest-snapshot",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestSnapshot {
                dax: false,
                mode: SnapshotMode::Zygote,
                volumes: 0,
                read_only: SnapshotReadOnly::Nothing,
            }),
        ),
        TestCase::new(
            "fxfs-guest-snapshot-dax",
            &[Tag::Kernel, Tag::Nightly, Tag::SoakReplicas],
            Box::new(TestFxfsGuestSnapshot {
                dax: true,
                mode: SnapshotMode::Zygote,
                volumes: 0,
                read_only: SnapshotReadOnly::Nothing,
            }),
        ),
        TestCase::new(
            "fxfs-guest-suspend-dax",
            &[Tag::Kernel, Tag::Nightly, Tag::SoakReplicas],
            Box::new(TestFxfsGuestSnapshot {
                dax: true,
                mode: SnapshotMode::Suspend,
                volumes: 0,
                read_only: SnapshotReadOnly::Nothing,
            }),
        ),
        TestCase::new(
            "fxfs-guest-suspend-read-only-volume",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestSnapshot {
                dax: false,
                mode: SnapshotMode::Suspend,
                volumes: 0,
                read_only: SnapshotReadOnly::Volume,
            }),
        ),
        TestCase::new(
            "fxfs-guest-suspend-remounted-read-only",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestSnapshot {
                dax: false,
                mode: SnapshotMode::Suspend,
                volumes: 0,
                read_only: SnapshotReadOnly::Remounted,
            }),
        ),
        TestCase::new(
            "fxfs-guest-snapshot-volumes",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestSnapshot {
                dax: false,
                mode: SnapshotMode::Suspend,
                volumes: VOLUMES,
                read_only: SnapshotReadOnly::Nothing,
            }),
        ),
        TestCase::new(
            "fxfs-guest-snapshot-volumes-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestSnapshot {
                dax: true,
                mode: SnapshotMode::Suspend,
                volumes: VOLUMES,
                read_only: SnapshotReadOnly::Nothing,
            }),
        ),
        TestCase::new(
            "fxfs-guest-migrate-dax",
            &[Tag::Kernel, Tag::Nightly, Tag::SoakReplicas],
            Box::new(TestFxfsGuestSnapshot {
                dax: true,
                mode: SnapshotMode::Migrate,
                volumes: 0,
                read_only: SnapshotReadOnly::Nothing,
            }),
        ),
        TestCase::new(
            "fxfs-guest-migrate-tenant-dax",
            &[Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestSnapshot {
                dax: true,
                mode: SnapshotMode::TenantMigrate,
                volumes: 0,
                read_only: SnapshotReadOnly::Nothing,
            }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-checks",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly, Tag::SoakChecks, Tag::SoakSanitized],
            Box::new(PAGE_CACHE_CHECKS),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-checks-dax",
            &[
                Tag::E2e,
                Tag::Kernel,
                Tag::Nightly,
                Tag::DaxGate,
                Tag::SoakChecks,
                Tag::SoakSanitized,
            ],
            Box::new(TestFxfsGuestChecks { dax: true, ..PAGE_CACHE_CHECKS }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-checks-dax-delay",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly, Tag::DaxGate, Tag::SoakChecks],
            Box::new(TestFxfsGuestChecks { dax: true, delay: true, ..PAGE_CACHE_CHECKS }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-checks-dax-small",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly, Tag::DaxGate, Tag::SoakChecks],
            Box::new(TestFxfsGuestChecks { dax: true, small_window: true, ..PAGE_CACHE_CHECKS }),
        ),
        // fxdax: a check outrunning its budget fails the run by name (TASK-168).
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-checks-overrun",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly],
            Box::new(TestFxfsGuestChecks { overrun: true, ..PAGE_CACHE_CHECKS }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("fxfs-guest-soak", &[Tag::SoakMixed], Box::new(TestFxfsGuestSoak)),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-script",
            &[Tag::Nightly],
            Box::new(TestFxfsGuestScript { dax: false, full: false, killed: false }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-script-dax",
            &[Tag::Nightly],
            Box::new(TestFxfsGuestScript { dax: true, full: false, killed: false }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-script-full",
            &[Tag::Nightly],
            Box::new(TestFxfsGuestScript { dax: false, full: true, killed: false }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-script-full-dax",
            &[Tag::Nightly],
            Box::new(TestFxfsGuestScript { dax: true, full: true, killed: false }),
        ),
        // fxdax: the script's VM killed part way, its files checked against what a kill may leave
        // (TASK-149.2).
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-script-killed",
            &[Tag::Nightly],
            Box::new(TestFxfsGuestScript { dax: false, full: false, killed: true }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-script-killed-dax",
            &[Tag::Nightly],
            Box::new(TestFxfsGuestScript { dax: true, full: false, killed: true }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-script-killed-full",
            &[Tag::Nightly],
            Box::new(TestFxfsGuestScript { dax: false, full: true, killed: true }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-script-killed-full-dax",
            &[Tag::Nightly],
            Box::new(TestFxfsGuestScript { dax: true, full: true, killed: true }),
        ),
        // fxdax: fsx-style scripts on Fxfs and ext4 in one guest (TASK-44.15).
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-fsx",
            &[Tag::Kernel, Tag::Nightly, Tag::SoakChecks],
            Box::new(TestFxfsGuestFsx { dax: false, delay: false, faults: false }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-fsx-dax",
            &[Tag::Kernel, Tag::Nightly, Tag::SoakChecks],
            Box::new(TestFxfsGuestFsx { dax: true, delay: false, faults: false }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-fsx-delay-dax",
            &[Tag::Nightly, Tag::SoakChecks],
            Box::new(TestFxfsGuestFsx { dax: true, delay: true, faults: false }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "fxfs-guest-fsx-faults-dax",
            &[Tag::SoakSanitized],
            Box::new(TestFxfsGuestFsx { dax: true, delay: false, faults: true }),
        ),
        TestCase::new(
            "fxfs-guest-dax",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly, Tag::DaxGate],
            Box::new(TestFxfsGuest::host_formats().with_dax()),
        ),
        TestCase::new(
            "fxfs-guest-crash-dax",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly, Tag::DaxGate, Tag::SoakCrash],
            Box::new(TestFxfsGuest::crash().with_dax()),
        ),
        TestCase::new(
            "fxfs-guest-crash-dax-killed",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly, Tag::DaxGate, Tag::SoakCrash],
            Box::new(TestFxfsGuest::crash_killed()),
        ),
        TestCase::new(
            "fxfs-guest-crash-dax-daemon-killed",
            &[Tag::Nightly],
            Box::new(TestFxfsGuest::crash_daemon_killed()),
        ),
        TestCase::new(
            "fxfs-guest-crash-stray-dax",
            &[Tag::E2e, Tag::Nightly],
            Box::new(TestFxfsGuest::crash_stray()),
        ),
        TestCase::new(
            "fxfs-guest-faults-dax",
            &[Tag::SoakSanitized],
            Box::new(TestFxfsGuest::faults()),
        ),
        TestCase::new(
            "fxfs-guest-docker-dax",
            &[Tag::E2e, Tag::Nightly],
            Box::new(TestFxfsGuestDocker { build: false }),
        ),
        // fxdax: LangSmith's builds, on 8 vCPUs, which oopsed 2 in 17 times (TASK-311).
        TestCase::new(
            "fxfs-guest-docker-build-dax",
            &[Tag::Manual],
            Box::new(TestFxfsGuestDocker { build: true }),
        ),
        TestCase::new(
            "extent-guest",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly],
            Box::new(TestExtentGuest),
        ),
        TestCase::new(
            "extent-guest-other-version",
            &[Tag::E2e, Tag::Kernel, Tag::Nightly],
            Box::new(TestExtentGuestOtherVersion),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-ext4-guest-kernel",
            &[Tag::Bench],
            Box::new(TestFsBench { backend: BenchBackend::Ext4GuestKernel }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new("ext4-loop-persists", &[Tag::Nightly], Box::new(TestExt4LoopPersists)),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-ext4-loop",
            &[Tag::Bench],
            Box::new(TestFsBench { backend: BenchBackend::Ext4Loop }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-ext4-loop-unsafe",
            &[Tag::Bench],
            Box::new(TestFsBench { backend: BenchBackend::Ext4LoopUnsafe }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-ext4-dax-pmem",
            &[Tag::Bench],
            Box::new(TestFsBench { backend: BenchBackend::Ext4DaxPmem }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-ext4-pmem",
            &[Tag::Manual],
            Box::new(TestFsBench { backend: BenchBackend::Ext4Pmem }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-fxfs-guest",
            &[Tag::Bench],
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuest }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-fxfs-guest-dax",
            &[Tag::Bench],
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDax }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fs-fxfs-guest-dax-followed",
            &[Tag::Manual],
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxFollowed }),
        ),
        TestCase::new(
            "bench-fs-fxfs-guest-dax-followed-idle",
            &[Tag::Manual],
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxFollowedIdle }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-fxfs-guest-dax-warm",
            &[Tag::Manual],
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxForkWarm }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-fxfs-guest-dax-cold",
            &[Tag::Manual],
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxForkCold }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-fxfs-guest-warm",
            &[Tag::Manual],
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestForkWarm }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-ext4-dax-pmem",
            &[Tag::Manual],
            Box::new(TestFsBench { backend: BenchBackend::Ext4DaxPmemFork }),
        ),
        #[cfg(any(feature = "host", target_os = "linux"))]
        TestCase::new(
            "bench-fork-ext4-pmem",
            &[Tag::Manual],
            Box::new(TestFsBench { backend: BenchBackend::Ext4PmemFork }),
        ),
        TestCase::new(
            "bench-fork-fxfs-guest-dax-profiled",
            &[Tag::Manual],
            Box::new(TestFsBench { backend: BenchBackend::FxfsGuestDaxForkProfiled }),
        ),
    ]
}

////////////////////
// Implementation details:
//////////////////

pub trait ReportImpl {
    fn fmt_text(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
    fn fmt_gh_markdown(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

pub trait Report: ReportImpl {
    fn text(&self) -> ReportText<'_, Self> {
        ReportText(self)
    }

    fn gh_markdown(&self) -> ReportGhMarkdown<'_, Self> {
        ReportGhMarkdown(self)
    }
}

impl<T: ReportImpl + ?Sized> Report for T {}

pub struct ReportText<'a, T: ReportImpl + ?Sized>(pub &'a T);

impl<T: ReportImpl + ?Sized> std::fmt::Display for ReportText<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt_text(f)
    }
}

pub struct ReportGhMarkdown<'a, T: ReportImpl + ?Sized>(pub &'a T);

impl<T: ReportImpl + ?Sized> std::fmt::Display for ReportGhMarkdown<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt_gh_markdown(f)
    }
}

use macros::{guest, host};
#[host]
use std::path::PathBuf;

#[cfg(all(feature = "guest", feature = "host"))]
compile_error!("Cannot enable both guest and host in the same binary!");

#[cfg(feature = "host")]
mod common;

#[cfg(feature = "host")]
pub mod rootfs;

mod shared_volumes;

#[cfg(feature = "host")]
pub mod vm;
#[cfg(feature = "host")]
pub mod loop_device;

#[cfg(all(feature = "guest", target_os = "linux"))]
pub mod guest_init;

/// Under a VMM whose guest has a single console (Firecracker's serial port), the guest agent's
/// standard output goes there between these bytes (start and end of text), and its standard error
/// between the next two (shift out, shift in), which the host splits apart, and from what the
/// kernel says there.
pub const STDOUT_START: u8 = 0x02;
pub const STDOUT_END: u8 = 0x03;
pub const STDERR_START: u8 = 0x0e;
pub const STDERR_END: u8 = 0x0f;

/// fxdax: in a test's environment, when the runner kills it if it runs on, in seconds since the
/// epoch: so that its VMs' guests say where they are stuck before then ([`GUEST_WATCHDOG`]).
pub const TEST_DEADLINE: &str = "FXDAX_TEST_DEADLINE";

/// fxdax: in the guest agent's environment, the seconds after which the guest, still running,
/// dumps its tasks' and CPUs' stacks, shortly before the runner's deadline (`guest_init`).
pub const GUEST_WATCHDOG: &str = "FXDAX_GUEST_WATCHDOG_SECS";

#[host]
#[derive(Clone, Debug)]
pub struct TestSetup {
    pub test_case: String,
    // A tmp directory for misc. artifacts used be the test (e.g. sockets)
    pub tmp_dir: PathBuf,
}

#[host]
impl TestSetup {
    /// Register a PID to be killed after the test finishes.
    ///
    /// The runner will SIGKILL these PIDs after check() returns, even if the
    /// test crashed. Use this for background processes (e.g. gvproxy) that
    /// outlive the VM.
    pub fn register_cleanup_pid(&self, pid: u32) {
        use std::io::Write;
        let path = self.tmp_dir.join("cleanup.pids");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("Failed to open cleanup.pids");
        writeln!(file, "{}", pid).expect("Failed to write cleanup PID");
    }
}

#[host]
pub trait Test {
    /// Start the VM
    fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()>;

    /// Checks the output of the (host) process which started the VM
    fn check(self: Box<Self>, stdout: Vec<u8>, _test_setup: TestSetup) -> TestOutcome {
        let output = String::from_utf8(stdout).unwrap();
        if output == "OK\n" {
            TestOutcome::Pass
        } else {
            TestOutcome::Fail(format!("expected exactly {:?}, got {:?}", "OK\n", output))
        }
    }

    /// Check if this test should run on this platform.
    fn should_run(&self) -> ShouldRun {
        ShouldRun::Yes
    }

    /// Return the image (its Containerfile, fxdax: and what its build takes from the host) if
    /// this test needs a custom rootfs image.
    /// The runner will build the image via podman and extract it before launching the VM.
    /// If podman is unavailable, the test is skipped.
    fn rootfs_image(&self) -> Option<rootfs::Image> {
        None
    }

    /// Per-test timeout in seconds. The runner kills the test if it exceeds this.
    fn timeout_secs(&self) -> u64 {
        15
    }

    /// fxdax: what a test that ran out of time, its VM gone, has to add to that.
    fn timed_out(self: Box<Self>, _test_setup: TestSetup) -> Option<String> {
        None
    }

    /// fxdax: whether the runner fails the test, once it passed, on its VMs' kernels complaining
    /// on their consoles (`vm::complaints`, TASK-44.8): unless it means them to.
    fn checks_console(&self) -> bool {
        true
    }
}

#[guest]
pub trait Test {
    /// This will be executed in the guest, you can panic! if the test failed!
    fn in_guest(self: Box<Self>) {}
}

pub struct TestCase {
    pub name: &'static str,
    /// fxdax: the suites it is in.
    pub tags: &'static [Tag],
    pub test: Box<dyn Test>,
}

impl TestCase {
    // Your test can be parametrized, so you can add the same test multiple times constructed with
    // different parameters with and specify a different name here.
    pub fn new(name: &'static str, tags: &'static [Tag], test: Box<dyn Test>) -> Self {
        Self { name, tags, test }
    }

    /// Check if this test should run on this platform.
    #[host]
    pub fn should_run(&self) -> ShouldRun {
        self.test.should_run()
    }

    #[host]
    pub fn rootfs_image(&self) -> Option<rootfs::Image> {
        self.test.rootfs_image()
    }

    #[host]
    pub fn timeout_secs(&self) -> u64 {
        self.test.timeout_secs()
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_testcases_have_unique_names() {
        let test_cases = test_cases();
        let mut names: HashSet<&str> = HashSet::new();

        for test_case in test_cases {
            let name = test_case.name();
            let was_inserted = names.insert(name);
            if !was_inserted {
                panic!("test_cases() contains multiple items named `{name}`")
            }

            if name == "all" {
                panic!("test_cases() contains test named {name}, but the name is reseved")
            }
        }
    }

    /// fxdax: the names of the cases of `cases` in no suite, not even [`Tag::Manual`].
    fn untagged(cases: &[TestCase]) -> Vec<&'static str> {
        cases.iter().filter(|case| case.tags.is_empty()).map(|case| case.name).collect()
    }

    /// fxdax: a case new to the harness says where it runs, or where it doesn't: one in no suite
    /// was run by none of the scripts, as xfstests' and the scripts' cases were (TASK-160.29).
    #[test]
    fn every_case_is_in_a_suite() {
        assert_eq!(untagged(&test_cases()), Vec::<&str>::new());
        let mut cases = test_cases();
        cases.push(TestCase::new("untagged", &[], Box::new(TestRunnerDeadlock)));
        assert_eq!(untagged(&cases), ["untagged"]);
    }

    /// fxdax: no suite is empty, and each tag's name is its own.
    #[test]
    fn every_tag_selects_cases() {
        let cases = test_cases();
        for &tag in Tag::ALL {
            assert!(cases.iter().any(|case| case.tags.contains(&tag)), "{tag:?} selects nothing");
            assert_eq!(Tag::named(tag.name()), Some(tag));
            assert!(!tag.about().is_empty() && !tag.about().starts_with(' '), "{tag:?}");
        }
        assert_eq!(Tag::named("nope"), None);
    }

    fn names_of(cases: Vec<TestCase>) -> Vec<&'static str> {
        cases.into_iter().map(|case| case.name).collect()
    }

    /// fxdax: a selection is of the cases named and tagged, once each, as registered.
    #[test]
    fn a_selection_is_of_cases_named_and_tagged() {
        let select = |names: &[&str], tags: &[Tag]| {
            let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
            select(test_cases(), &names, tags).map(names_of)
        };
        assert_eq!(
            select(&[], &[Tag::PerfDensity]),
            Ok(vec!["fxfs-guest-pgbench-density", "fxfs-guest-pgbench-dax-density"])
        );
        // Named before the tag's, but run as registered, and once.
        assert_eq!(
            select(&["fxfs-guest-dax", "extent-guest"], &[Tag::DaxGate, Tag::PerfDensity]),
            Ok(vec![
                "fxfs-guest-pgbench-density",
                "fxfs-guest-pgbench-dax-density",
                "fxfs-guest-checks-dax",
                "fxfs-guest-checks-dax-delay",
                "fxfs-guest-checks-dax-small",
                "fxfs-guest-dax",
                "fxfs-guest-crash-dax",
                "fxfs-guest-crash-dax-killed",
                "extent-guest",
            ])
        );
        assert_eq!(select(&[], &[]), Ok(vec![]));
        assert_eq!(
            select(&["fxfs-guest-dax", "fxfs-guest-nope"], &[]),
            Err("no such test case: fxfs-guest-nope".to_string())
        );
    }
}
