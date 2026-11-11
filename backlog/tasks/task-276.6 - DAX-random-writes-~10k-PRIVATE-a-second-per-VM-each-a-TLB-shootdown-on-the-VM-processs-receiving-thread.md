---
id: TASK-276.6
title: >-
  DAX random writes: ~10k PRIVATE a second per VM, each a TLB shootdown on the
  VM process's receiving thread
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 05:12'
updated_date: '2026-11-11 07:25'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
type: enhancement
ordinal: 708000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-276 phase 1 (GCE c3-standard-8, nested KVM): fxfs-guest-dax random 4K writes stay at 10-12k IOPS for 2-8 jobs and 2-8 vCPUs, latency linear in jobs. Each write is a PRIVATE; the VM process applies every stage operation on its one receiving thread, in order, and that thread is saturated: 47% in mmap of private memory over an installed page and 13% in fallocate, both waiting on TLB-shootdown IPIs (smp_call_function_many_cond, kvm_flush_remote_tlbs), which nested virtualization makes dear. mmap takes mmap_lock for write, so more threads would not overlap them: the cost is per change, and the way out is fewer changes (a guest privatizing more than a page per write where it will write the rest, joining the PRIVATEs a batch of receives holds into one change, or pages written in place that were never installed).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A measured design that cuts the stage changes (or their shootdowns) per DAX write, keeping doc-13 rules and the uffd Shuttle tests
- [x] #2 fxfs-guest-dax random writes scale past 12k IOPS with jobs on the TASK-276 bench
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Re-measure on ubu1 (bare-metal KVM, AMD 8945HS): TASK-276's scaling bench, fxfs-guest-dax random 4K writes at 1/2/4/8 jobs and 2/4/8 vCPUs, fxfs-guest cold random reads, ext4 control; perf of the VM process's receiving thread (mmap, fallocate, wp_page_copy shares); numbers into the notes.
2. (a) The fallocate share: verify what it is (the stage's only fallocate is FALLOC_FL_PUNCH_HOLE of private pages that go, at a MAP or UNMAP over them) and design accordingly: punch only memory nothing maps any more, so that the punch zaps no PTE and flushes nothing.
3. (b) A MAP names pages, the first touch installs them: map/remap record pages left out, installing nothing; a page's first fault installs it (reinstall), a PRIVATE of a page never installed fills it in place (no mmap, no shootdown). MAP_AHEAD still installs and prefaults what it covers, for reads.
4. doc-13 (backlog doc update), specs/dax-cow (MAP/UNMAP/eviction racing a first touch, a mutation breaking it), uffd Shuttle tests (a store's first touch racing a MAP over the page, an UNMAP and eviction).
5. Interleaved A/B on ubu1, 6+ iterations a side, prebuilt artifacts, the exact landed tree; AC #2.
6. Land on main (CI green), Firecracker series only if the VMM side changes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Findings before measuring (2026-11-10): the receiving thread's fallocate (13% on GCE) is the stage's one fallocate, FALLOC_FL_PUNCH_HOLE of private pages a MAP or UNMAP ends, not allocation: each punch zapped two page-table entries, the window's and the fill view's (UFFDIO_COPY), a TLB flush each, besides the change's mmap. The GCE profile spanned the whole case, so most of it is likely the scale dir's final rm -rf (scripts/dax-stage-syscalls.bt to confirm on ubu1). And the TASK-276 bench's rand-write writes pages its rand-read-warm read whole before: each write's PRIVATE replaces a page installed, one shootdown, which lazy install cannot remove; scale-rand-write-unread added for the cold case (pages never read).

Done (branch task-276.6, Firecracker fxdax-v1.16.1-dax-fill b4f4f9d78): a MAP names pages, the first touch installs them (dax::uffd map/remap install nothing; reinstall at first fault under the pages' lock; a guest store's first touch goes to the window as a store; map_ahead installs and prefaults for MAP_AHEAD; the guest maps ahead for reads alone). Private pages filled by pwrite64 (seccomp: +pwrite64, -UFFDIO_COPY), emptied after the change's mmap took them away (one mmap per change, punches flush nothing; a lone private run punched in place). Tests: uffd units 34/34 on ubu1; Shuttle 54/54 (store and load first touches racing MAP, UNMAP, eviction; rules changes_take_away_installed, install_as_named_now each failing when off); specs/dax-cow DaxCowLoads + NoInstallAsNamed, NoChangesTakeAway, all 14 models pass; macOS workspace 2204/2204 and scripts 260/260; firecracker-series-check ok both ways for every commit to b4f4f9d78. doc-13 updated.

Step 1, bare metal (2026-11-10): ubu1 (AMD Ryzen 9 8945HS, 8 cores/16 threads, KVM on bare metal, kernel 7.0), main f36453323 + the bench (2aad035e0), prebuilt artifacts, each run a measurement job (fxjob --bench: CPUs 4-7 and 12-15 to itself, others' jobs held to the rest), 3 iterations, medians; stores and results on ZFS (/home), extentd's bucket on tmpfs. fxfs-guest-dax random 4K writes, IOPS, j1 / j2 / j4 / j8:
- read pages (scale-rand-write): 2 vCPUs 15.3k / 31.5k / 35.3k / 36.2k; 4 vCPUs 15.8k / 30.1k / 41.4k / 36.4k; 8 vCPUs 12.8k / 24.1k / 33.6k / 26.0k. p50 at 4 vCPUs 48 / 40 / 71 / 181 us.
- unread pages (scale-rand-write-unread): 2 vCPUs 13.1k / 25.0k / 20.1k / 18.5k; 4 vCPUs 14.2k / 23.5k / 22.9k / 22.7k; 8 vCPUs 11.6k / 21.1k / 19.4k / 15.4k; p99 at 4 vCPUs j4/j8 4.4 / 10.9 ms.
So on bare metal the writes scale past 12k with jobs on main already: GCE's 10-12k plateau is nested virtualization's price for the shootdowns. What plateaus at 33-41k is not the receiving thread: its samples (perf, 999 Hz, 4 vCPUs, 8 jobs) peak at 26% of a CPU in any second under DAX writes, 54% under fxfs-guest's cold reads. The unread writes' plateau, with ms p99s, is the daemon side: their pages aren't in extentd's memory (nothing read them), so each PRIVATE brings its source page in from the store first, where read pages' are cached.
Profile (4 vCPUs, j8, perf of the VM process, scripts/dax-stage-syscalls.bt on the bench CPUs): the rand-write j8 second, 8,192 PRIVATEs, each an mmap over the page installed (8,495 mmaps, 116 ms, 14 us each), 13,154 cross-CPU calls (20 ms) and 8,192 KVM remote flushes; the unread writes' second, 8,184 PRIVATEs and 247 mmaps (their pages not in memory, so not installed: filled in place already on main); fallocate 0 during writes, and 12,801 (94 ms) with 5,654 PROT_NONE mmaps in one second at the case's end, the unlinks' UNMAPs of private pages: the punches, as the code said. Shootdown batching: not designed, as the cap it would lift is nested-only here; options for nested hosts stay open for the user.

The change, measured (2026-11-10, ubu1, fxjob --bench; docs/measurements/dax-lazy-install-2026-11-10.md): an interleaved A/B, main 7e1740cb7 with the bench (83c212a7b) against the branch as it lands (9e338efa2, Firecracker 323b5a5af, fxdax-v1.16.1-r6), 3 rounds x 2 iterations, 4 vCPUs, ext4 the control: 11 real differences, all improvements, all writes to pages the guest never read (extents in extentd's memory): 13.2k / 24.4k / 25.4k / 25.4k -> 17.1k / 34.7k / 42.1k / 41.9k IOPS at 1/2/4/8 jobs, p99 0.60 / 1.16 / 1.99 ms -> 0.11 / 0.23 / 0.53 ms (j2/j4/j8). Writes to pages read before, and everything else, within drift. Traced: main's unread writes 8,380 mmaps over installed pages and 8,184 KVM flushes in their second, the branch's 134 and 0; the closing UNMAPs' 12,801 punches 32,883 cross-CPU calls and 13,122 KVM flushes on main, 6 and 321 on the branch. The earlier pre-rebase A/B agreed (9 real, all improvements).
The bench's first unread variant found the pages not in extentd's memory (nothing had read them), which main already filled in place: changed to read them first through the page cache, as files created in the mount are, so that extentd holds them.
AC #2 on bare metal: the writes scale past 12k with jobs (main and branch alike, 15k at 1 job to 39-41k at 4). Nested KVM was not re-measured (GCE retired): there, writes to pages read before still take a shootdown each, which only batching would cut; options not designed, the user's to decide.
Tests on the landed tree: guest e2e 18/18 (e2e suite, fxfs-guest-snapshot-dax, fxfs-guest-pgbench-dax-forked) from its own git worktree and kernel; the VMM's tests (test_config_space_as_upstream); Shuttle part; dax units 36/36 (windows_larger_than_the_map_count_read_whole needs >8 GiB on ubu1: TASK-342); firecracker-series-check ok both ways, 18 commits; macOS workspace 2,228, Shuttle, scripts 266; specs/dax-cow 14 models; upstream-changes.py --check. An earlier e2e run's failures were the run's: a kernel built from an rsync'd tree (build commit unknown) and budgets before TASK-263's fix (append_writeback 4.9 s of 3 s, TASK-263's symptom, fixed on main).

Landed at main aaba26d2b (with 87eb8b8c4: the unmapped-page-read-as-hole mutant and the guide's traces of the two new mutations; aaba26d2b: extentd's vm_processes_tell_the_daemon_what_their_windows_cost touches its page before its PRIVATE, as a map installs nothing now). Main CI 42949975927 green, every job. Firecracker fxdax-v1.16.1-r6 (323b5a5af) on r5.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A MAP now names pages and their first touch installs them (dax::uffd), so a page written before it is read, of extents the host's memory holds, is made private in place of nothing: no mmap, no TLB shootdown. Private pages are filled by pwrite (Firecracker fxdax-v1.16.1-r6's seccomp) and emptied once the change's mmap took them away, so the punches GCE's 13% fallocate was flush nothing. Measured on ubu1 (bare metal, fxjob --bench, interleaved A/B of the landed tree, 3x2): random writes to unread pages +29/+42/+66/+65% at 1/2/4/8 jobs (25.4k -> 41.9k at 8, p99 2.0 -> 0.53 ms), 11 real differences, no regression. On bare metal main's DAX random writes already scale past 12k (15k -> 41k), the receiving thread 26% busy at most: GCE's 10-12k cap was nested KVM's shootdown cost, which writes to pages read before still pay (batching left to the user). Tests: Shuttle (first touches racing MAP/UNMAP/eviction, two new rules), uffd units, specs/dax-cow (DaxCowLoads, two mutations), guest e2e 18/18, series check, main CI 42949975927.
<!-- SECTION:FINAL_SUMMARY:END -->
