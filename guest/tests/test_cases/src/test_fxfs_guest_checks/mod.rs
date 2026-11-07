// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Checks of the guest kernel's own Fxfs (fxfs-guest-checks), against what
// fxdax's own Fxfs on the host sees.
//
// The host creates an fxdax image, an extent store, which the VM gets as an
// extent-store device, served by fxdax's extent daemon, and boots fxdax's
// guest kernel (FXDAX_GUEST_KERNEL), which mounts it with its Fxfs;
// fxfs-guest-checks-dax with a DAX window, which Fxfs maps files' data from
// (on hosts whose pages are 4 KiB, Fxfs's blocks). The guest checks whether
// files are DAX-mapped, and exercises writes through mappings, regular writes
// and namespace operations, and evicts a file to write it back. After the VM
// exits the host reopens the image, checks it, and checks what the guest left
// behind.

pub struct TestFxfsGuestChecks {
    /// Whether the device has a DAX window, and the guest maps files into it.
    pub dax: bool,
    /// fxdax: whether the guest's DAX code sleeps at the steps of its protocol where tasks race
    /// (fxfs.dax_delay_us), from the run's seed, widening the races' windows (TASK-119.3).
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub delay: bool,
    /// fxdax: whether the DAX window is small, 32 MiB, as the checks' files fill it: ranges are
    /// reclaimed under load (TASK-119.12).
    pub small_window: bool,
    /// fxdax: whether the suite runs the checks made to fail ([`Suites::Failing`]) before the page
    /// cache's: one crashes, and one hangs in the kernel past its budget, which the guest ends
    /// (TASK-168). Each fails alone, by name, and the rest pass (TASK-160.30).
    pub overrun: bool,
}

/// The suite without a DAX window, whose files are in the page cache: the others are this, with
/// what they change.
pub const PAGE_CACHE_CHECKS: TestFxfsGuestChecks =
    TestFxfsGuestChecks { dax: false, delay: false, small_window: false, overrun: false };

impl TestFxfsGuestChecks {
    /// fxdax: the checks this suite runs, in their order, those of `only` alone, names separated
    /// by commas, if it names any (FXDAX_CHECKS): each with its index in [`CHECKS`]. Fails on a
    /// name no check has, which ran nothing and passed (TASK-160.30).
    pub(crate) fn checks(&self, only: &str) -> Result<Vec<(usize, &'static Check)>, String> {
        let only: Vec<&str> = only.split(',').filter(|name| !name.is_empty()).collect();
        if let Some(unknown) = only.iter().find(|name| !CHECKS.iter().any(|c| c.name == **name)) {
            return Err(format!("FXDAX_CHECKS names {unknown:?}, which no check is called"));
        }
        let ours = |check: &Check| check.suites.have(self);
        let named = |check: &Check| only.is_empty() || only.contains(&check.name);
        Ok(CHECKS.iter().enumerate().filter(|(_, check)| ours(check) && named(check)).collect())
    }
}

/// fxdax: in the host's environment, what scales the checks' budgets besides the guest kernel and
/// storage, for a slower host; in the guest's, all that scales them, as the host has it.
const SCALE: &str = "FXDAX_CHECKS_SCALE";

/// fxdax: how a check's process ended, as the guest's log says after the check's time
/// (TASK-160.30).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "host", allow(dead_code))]
pub(crate) enum Ended {
    Passed,
    /// With a status other than 0: 101 as it panicked, the panic's message before it in the log.
    Exited(i32),
    /// By a signal: a crash, such as a SIGBUS.
    Signalled(i32),
    /// Running on past its budget: killed.
    Overran,
}

impl std::fmt::Display for Ended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Ended::Passed => write!(f, "passed"),
            Ended::Exited(status) => write!(f, "exited with {status}"),
            Ended::Signalled(signal) => {
                // SAFETY: strsignal's string, which this copies before anything calls it again.
                let name = unsafe { std::ffi::CStr::from_ptr(nix::libc::strsignal(signal)) };
                write!(f, "killed by signal {signal} ({})", name.to_string_lossy())
            }
            Ended::Overran => write!(f, "still running after its budget: killed"),
        }
    }
}

/// fxdax: which of the suites a check runs in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Suites {
    All,
    /// Those with a DAX window.
    Dax,
    /// Those with a DAX window of 512 MiB, which the checks' files don't fill.
    LargeDax,
    /// Those without one, whose files are in the page cache.
    PageCache,
    /// Those with a DAX window of 32 MiB, which they do.
    SmallDax,
    /// The overrun suite's alone: the checks made to fail, which no other runs
    /// ([`TestFxfsGuestChecks::overrun`]).
    Failing,
}

impl Suites {
    fn have(self, suite: &TestFxfsGuestChecks) -> bool {
        match self {
            Suites::All => true,
            Suites::Dax => suite.dax,
            Suites::LargeDax => suite.dax && !suite.small_window,
            Suites::PageCache => !suite.dax,
            Suites::SmallDax => suite.dax && suite.small_window,
            Suites::Failing => suite.overrun,
        }
    }
}

/// fxdax: a check of the guest's Fxfs: its name, which suites run it, and its budget, what it may
/// take on a release guest kernel on GCE's n2-standard-4 with its store in the daemon's memory:
/// twice the most it took there, in any suite (GCE r560's page cache, r563's DAX), and 3 s at
/// least, which the host scales for other kernels and storage (`host::TestFxfsGuestChecks::run`).
/// One that runs on past it, the guest kills, its stacks dumped, and fails by name: rather than
/// the suite outrunning a limit of its own, which a few slow checks crossed and named none of them
/// (TASK-168). A suite's limit is its checks' budgets', and more, for its VM's boot.
#[derive(Debug, PartialEq)]
pub(crate) struct Check {
    pub(crate) name: &'static str,
    pub(crate) suites: Suites,
    pub(crate) budget: std::time::Duration,
}

/// fxdax: every check, in the order they run, a declaration of it: `name: suites, budget in
/// seconds => body`, the body a function of the guest's, given its suite's fields in parentheses
/// where it takes them, such as `test_mmap_write(dax)`, and, after a comma, the host's verify of
/// what it left in the image, if it has one, a function of the host's (`host::Image`). Given to
/// `$then`, which makes of them what each side needs: the [`CHECKS`] both have, the guest's bodies
/// of them, and the host's verifies, so that none can disagree on which is which (TASK-168). The
/// guest runs each body in a process of its own (TASK-160.30).
macro_rules! checks {
    ($then:ident) => {
        $then! {
            // Made to fail, first, in the overrun suite alone: they fail by name, and the rest
            // pass after them (TASK-160.30).
            crashes: Failing, 3 => test_crashes;
            hangs: Failing, 3 => test_hangs;
            host_file: All, 3 => test_host_file;
            mmap_write: All, 3 => test_mmap_write(dax), verify_mmap_write;
            regular_write: All, 3 => test_regular_write, verify_regular_write;
            namespace: All, 3 => test_namespace, verify_namespace;
            listings_follow_changes: All, 3 => test_listings_follow_changes;
            truncate_mapped: All, 3 => test_truncate_mapped, verify_truncate_mapped;
            eviction_writes_back: All, 3 => test_eviction_writes_back, verify_eviction_writes_back;
            tmpfile: All, 3 => test_tmpfile, verify_tmpfile;
            fallocate: All, 3 => test_fallocate, verify_fallocate;
            zero_mapped: All, 3 => test_zero_mapped;
            seal_race: All, 3 => test_seal_race(dax);
            writes_across_seals: All, 23 => test_writes_across_seals;
            stats: All, 3 => test_stats;
            log_writes: All, 21 => test_log_writes;
            acls: All, 3 => test_acls;
            xattrs_read_as_written_back: All, 3 => test_xattrs_read_as_written_back;
            failed_write_inode_reported: All, 3 => test_failed_write_inode_reported;
            // They fill the filesystem, then remove what they filled it with; one remounts it,
            // which the window's checks' counts, from the filesystem's opening, come after.
            xattr_on_full_filesystem_is_refused: All, 5 => test_xattr_on_full_filesystem_is_refused;
            xattrs_bounded: All, 8 => test_xattrs_bounded;
            inherited_acls_on_full_filesystem: All, 5 => test_inherited_acls_on_full_filesystem;
            freeze: All, 3 => test_freeze;
            failed_thaw_thaws_on_retry: All, 3 => test_failed_thaw_thaws_on_retry;
            read_only_freezes: All, 3 => test_read_only_freezes;
            show_options: All, 3 => test_show_options(dax);
            write_removes_privileges: All, 3 => test_write_removes_privileges;
            delayed_unlinks: All, 3 => test_delayed_unlinks, verify_delayed_unlinks;
            sparse: All, 3 => test_sparse;
            blocks_follow_writes: All, 3 => test_blocks_follow_writes;
            write_from_own_mapping: All, 3 => test_write_from_own_mapping;
            write_from_same_page: All, 3 => test_write_from_same_page;
            short_copy: All, 3 => test_short_copy;
            append_stopped_short: All, 3 => test_append_stopped_short;
            append_stopped_short_in_a_page: All, 3 => test_append_stopped_short_in_a_page;
            write_past_end_copies_nothing: All, 3 => test_write_past_end_copies_nothing;
            direct_read_past_end: All, 3 => test_direct_read_past_end;
            store_past_end: All, 3 => test_store_past_end;
            store_past_end_then_written: All, 3 => test_store_past_end_then_written;
            store_past_end_then_cloned: All, 3 => test_store_past_end_then_cloned;
            read_into_own_mapping: All, 3 => test_read_into_own_mapping;
            write_over_mapped_hole: All, 3 => test_write_over_mapped_hole;
            file_handles: All, 3 => test_file_handles;
            unsupported: All, 3 => test_unsupported;
            reflinks: All, 3 => test_reflinks;
            removed_directories: All, 3 => test_removed_directories;
            lockups_show_every_cpu: All, 3 => test_lockups_show_every_cpu;
            file_attributes: All, 3 => test_file_attributes;
            idmapped_mount: All, 3 => test_idmapped_mount;
            multigrain_times: All, 3 => test_multigrain_times;
            cache_shrinks: All, 3 => test_cache_shrinks;
            direct_io: All, 3 => test_direct_io;
            timestamps_clamped: All, 3 => test_timestamps_clamped;
            random_ops: All, 8 => test_random_ops, verify_random_ops;
            torture: All, 38 => test_torture, verify_torture;
            pmd_write_unlink: Dax, 3 => test_pmd_write_unlink;
            mapped_after_write: Dax, 3 => test_mapped_after_write;
            dropped_after_seals: Dax, 3 => test_dropped_after_seals;
            reads_stay_mapped: LargeDax, 3 => test_reads_stay_mapped;
            writes_private: LargeDax, 3 => test_writes_private;
            stale_entry_flushed: LargeDax, 3 => test_stale_entry_flushed;
            pmd_over_stale_index: LargeDax, 3 => test_pmd_over_stale_index;
            faults_writes_compaction: Dax, 17 => test_faults_writes_compaction;
            append_pages: Dax, 3 => test_append_pages;
            append_size: Dax, 3 => test_append_size;
            append_writeback: Dax, 3 => test_append_writeback;
            append_while_mapped: Dax, 3 => test_append_while_mapped;
            append_size_follows_data: Dax, 3 => test_append_size_follows_data;
            append_reserved_tail: Dax, 3 => test_append_reserved_tail, verify_append_reserved_tail;
            map_failed: Dax, 3 => test_map_failed;
            map_many_failed: Dax, 3 => test_map_many_failed;
            write_killed: Dax, 3 => test_write_killed;
            // Copying on write's own (doc-13, TASK-164.2, 164.3).
            cow_store_persists: Dax, 3 => test_cow_store_persists;
            cow_unwritten_stay_shared: Dax, 3 => test_cow_unwritten_stay_shared;
            cow_dirty_truncate_punch_clone: Dax, 3 => test_cow_dirty_truncate_punch_clone;
            cow_commit_after_bytes: Dax, 3 => test_cow_commit_after_bytes;
            cow_created_in_page_cache: Dax, 3 => test_cow_created_in_page_cache;
            rebind_against_loads_and_stores: Dax, 3 => test_rebind_against_loads_and_stores;
            stores_under_rebinds: Dax, 5 => test_stores_under_rebinds;
            // Files on DAX are in no page cache.
            large_folios: PageCache, 3 => test_large_folios;
            scattered_writeback: PageCache, 3 => test_scattered_writeback;
            partial_writeback: PageCache, 3 => test_partial_writeback;
            writeback_out_of_memory: PageCache, 20 => test_writeback_out_of_memory;
            // Of a small window, which it fills.
            window_held_killed: SmallDax, 4 => test_window_held_killed;
            // Last: it fills the filesystem, then removes what it filled it with.
            enospc: All, 33 => test_enospc;
            write_times_add_up: Dax, 3 => test_write_times_add_up;
            // Of the checks before, on a small window (TASK-119.12).
            window_filled: SmallDax, 3 => test_window_filled;
            // Last: it unmounts, which what the others left open would keep it from.
            unbind_waits_for_unmount: All, 3 => test_unbind_waits_for_unmount;
        }
    };
}

/// fxdax: [`checks`]'s table, the host's and the guest's.
macro_rules! check_table {
    ($($name:ident: $suites:ident, $budget:literal => $body:ident $(($field:ident))?
        $(, $verify:ident)?;)*) => {
        pub(crate) const CHECKS: &[Check] = &[$(Check {
            name: stringify!($name),
            suites: Suites::$suites,
            budget: std::time::Duration::from_secs($budget),
        },)*];
    };
}
checks!(check_table);

/// Offset of the second 2 MiB DAX chunk.
const CHUNK: usize = 2 << 20;
const MAPPED_SIZE: usize = CHUNK + (1 << 20);
const HOST_CONTENT: &[u8] = b"written by the host\n";
const WRITTEN: &[u8] = b"written through FUSE_WRITE\n";
const EVICTED: &[u8] = b"written back when its inode is evicted\n";
/// fxdax: random_ops's file, and its model's (TASK-119.2).
const RANDOM_OPS: &str = "random-ops";
const RANDOM_OPS_MODEL: &str = "random-ops.model";
/// fxdax: random_ops's operations, as it made them, each a line, which replay it (TASK-119.15).
const RANDOM_OPS_HISTORY: &str = "random-ops.history";
/// What append_reserved_tail leaves, for the host to read.
const APPEND_RESERVED: &str = "append-reserved";

/// What the file `APPEND_RESERVED` holds.
fn append_reserved() -> Vec<u8> {
    (0..(256 << 10) + 100 + 3 * 4096).map(|at: usize| (at % 251) as u8).collect()
}
/// fxdax: torture's files, each with its model beside it, `<name>.model` (TASK-119.5).
const TORTURE_FILES: usize = 4;
fn torture(n: usize) -> String {
    format!("torture-{n}")
}
/// Size of the image, which is sparse.
#[cfg_attr(feature = "guest", allow(dead_code))]
const IMAGE_SIZE: u64 = 256 << 20;
/// fxdax: the directory a coverage kernel's counts of fs/fxfs's sources `COVERED` are left in for
/// the host, as the guest's checks end (TASK-119.14).
const COVERAGE: &str = "coverage";
const COVERED: [&str; 3] = ["dax", "device", "super"];

/// What fxfs-guest-checks' "punched" holds, `len` long: ones, but for the hole punched and zeros
/// from `zeroed` on.
fn punched_contents(len: usize, zeroed: usize) -> Vec<u8> {
    (0..len).map(|i| u8::from(!(1000..9000).contains(&i) && i < zeroed)).collect()
}

/// Content of the guest's mmapped file at `offset`.
fn pattern(offset: usize) -> u8 {
    (offset % 251) as u8
}

#[cfg(feature = "host")]
pub(crate) mod host;

#[cfg(feature = "guest")]
pub(crate) mod guest;

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE_CACHE: TestFxfsGuestChecks = PAGE_CACHE_CHECKS;
    const DAX: TestFxfsGuestChecks = TestFxfsGuestChecks { dax: true, ..PAGE_CACHE };
    const SMALL: TestFxfsGuestChecks = TestFxfsGuestChecks { small_window: true, ..DAX };
    const OVERRUN: TestFxfsGuestChecks = TestFxfsGuestChecks { overrun: true, ..PAGE_CACHE };

    fn names(suite: &TestFxfsGuestChecks, only: &str) -> Vec<&'static str> {
        suite.checks(only).unwrap().into_iter().map(|(_, check)| check.name).collect()
    }

    /// A suite's last `n` checks.
    fn last(suite: &TestFxfsGuestChecks, n: usize) -> Vec<&'static str> {
        let names = names(suite, "");
        names[names.len() - n..].to_vec()
    }

    /// The suites' checks, as they ran before the table (TASK-168): the end of each, which the
    /// checks before leave what they look at, and each suite's own.
    #[test]
    fn suites_run_their_checks_in_order() {
        assert_eq!(
            last(&PAGE_CACHE, 8),
            [
                "random_ops",
                "torture",
                "large_folios",
                "scattered_writeback",
                "partial_writeback",
                "writeback_out_of_memory",
                "enospc",
                "unbind_waits_for_unmount",
            ]
        );
        assert_eq!(
            last(&DAX, 4),
            ["stores_under_rebinds", "enospc", "write_times_add_up", "unbind_waits_for_unmount"]
        );
        assert_eq!(
            last(&SMALL, 6),
            [
                "stores_under_rebinds",
                "window_held_killed",
                "enospc",
                "write_times_add_up",
                "window_filled",
                "unbind_waits_for_unmount",
            ]
        );
        assert!(names(&DAX, "").contains(&"reads_stay_mapped"));
        assert!(!names(&SMALL, "").contains(&"reads_stay_mapped"));
        assert!(!names(&PAGE_CACHE, "").contains(&"pmd_write_unlink"));
    }

    /// The overrun suite runs the checks made to fail, then the page cache's, as that suite does;
    /// no other suite runs them (TASK-160.30).
    #[test]
    fn failing_checks_run_first_in_the_overrun_suite_alone() {
        let overrun = names(&OVERRUN, "");
        assert_eq!(overrun[..2], ["crashes", "hangs"]);
        assert_eq!(overrun[2..], names(&PAGE_CACHE, ""));
        for suite in [PAGE_CACHE, DAX, SMALL] {
            assert!(!names(&suite, "").contains(&"crashes"));
        }
    }

    /// How a check ended, as the guest's log says it.
    #[test]
    fn ends_say_how() {
        assert_eq!(Ended::Exited(101).to_string(), "exited with 101");
        let signalled = Ended::Signalled(nix::libc::SIGBUS).to_string();
        assert_eq!(signalled, format!("killed by signal {} (Bus error)", nix::libc::SIGBUS));
        assert_eq!(Ended::Overran.to_string(), "still running after its budget: killed");
    }

    /// FXDAX_CHECKS's checks run in the table's order, those of the suite alone.
    #[test]
    fn named_checks_run_alone() {
        assert_eq!(names(&DAX, "enospc,sparse,large_folios"), ["sparse", "enospc"]);
        let checks = DAX.checks("enospc").unwrap();
        assert_eq!(CHECKS[checks[0].0].name, "enospc");
    }

    /// A name no check has fails, which ran nothing and passed (TASK-160.30).
    #[test]
    fn unknown_names_fail() {
        assert_eq!(
            DAX.checks("sparse,nope"),
            Err("FXDAX_CHECKS names \"nope\", which no check is called".to_owned())
        );
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = CHECKS.iter().map(|check| check.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CHECKS.len());
    }
}
