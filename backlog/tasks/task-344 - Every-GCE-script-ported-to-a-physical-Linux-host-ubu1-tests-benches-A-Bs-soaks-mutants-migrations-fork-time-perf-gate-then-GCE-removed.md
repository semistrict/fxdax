---
id: TASK-344
title: >-
  Every GCE script ported to a physical Linux host (ubu1): tests, benches, A/Bs,
  soaks, mutants, migrations, fork time, perf gate; then GCE removed
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 07:29'
updated_date: '2026-11-11 17:59'
labels:
  - testing
  - ci
dependencies: []
priority: high
ordinal: 739000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user, 2026-11-11: no GCP; make sure everything is ported to run on a physical machine (ubu1).

Today's GCE entry points each create a disposable GCE host, set it up (scripts/lib/gce-host-startup.sh, linux-host-setup.sh) and run a scripts/lib/*-linux-host.sh there:
test-gce.sh, bench-gce.sh, bench-ab-gce.sh, perf-gate-gce.sh, soak-gce.sh, soak-workloads-gce.sh, mutants-gce.sh, guest-mutants-gce.py, gets-gce.sh, migrate-gce.sh, fork-time-gce.sh. They are backed by scripts/lib/gce.sh, and tested by scripts/tests/test_gce.py, test_bench_ab_gce.py, test_perf_gate_gce.py and fake_gcloud.py.

Port each to a named physical Linux host reached over ssh, ubu1 by default. That means:
- a git worktree of the pushed commit on the host, with its own CARGO_TARGET_DIR;
- the vendor/firecracker submodule initialised;
- the run under fxjob (fxjob --bench for timing);
- results copied back;
- the worktree removed afterwards.
The host is set up once by ubu1:~/docs/system-setup.d steps, not per run. Whatever a GCE startup script installed and those steps lack is added as a step.

Each port's tests replace the GCE ones and keep their coverage, against a fake ssh where the fakes were gcloud. Then remove the GCE scripts, lib/gce.sh, gce-host-startup.sh and their tests. Update AGENTS.md, README and any doc that tells people to use GCE. Historical measurements under docs/measurements stay as they are.

nightly.yml's guest-suites is TASK-284's, not this task's.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Every GCE entry point has a physical-host equivalent that runs on ubu1 under fxjob and is proven by one real run there
- [x] #2 The GCE scripts' tests are ported to the new scripts with the same coverage, then the GCE scripts, lib/gce.sh, gce-host-startup.sh and their tests are removed
- [x] #3 rg -i 'gcloud|gce' outside docs/measurements and backlog finds nothing that tells anyone to use GCP
- [x] #4 Anything a GCE host's setup installed that ubu1 lacks is a system-setup.d step
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. One client library, scripts/lib/host.sh, in place of lib/gce.sh, and its host half, scripts/lib/host-remote.sh, sent over ssh (bash -s): a run is a directory on the host (FXDAX_HOST, ubu1 by default; /mnt/fxdax-scratch/fxdax-host/RUN) holding a git worktree of the pushed commit (fetched from origin, vendor/firecracker initialised, an optional patch applied), its work directory (its own target dirs) and results; the host script runs detached under fxjob (--bench for timed drivers), as root with the user's toolchain (FXDAX_HOST_PROVISIONED=1), CARGO_BUILD_JOBS=4 and make -j4; the client waits on the job's lock, reconnecting; results come back by tar over ssh; the run directory and its worktree go at the end; resume and cancel for a run whose waiter died.
2. linux-host-setup.sh: host_kernel, one guest-kernel build for test/bench/bench-ab host scripts, reusing a kernel built from the same sources from a cache on the host (FXDAX_HOST_KERNELS), keyed by the kernel's inputs' content.
3. Port each driver to *-host.sh / guest-mutants-host.py, plus dax-gate.sh; drop the GCS-only results bucket (gcloud) from guest_results.py and test-linux-host.sh.
4. Port test_gce.py, test_bench_ab_gce.py, test_perf_gate_gce.py to test_host.py etc. against fake_ssh.py; then delete the GCE scripts, gce.sh, gce-host-startup.sh, their tests and fake_gcloud.py.
5. ubu1 setup: compare gce-host-startup.sh and linux-host-setup.sh with system-setup.d; add a step for what is missing (cargo-mutants, perf, bpftrace).
6. Docs: AGENTS.md, README, docs, comments naming GCE scripts.
7. Prove each entry point with one short real run on ubu1; macOS suite, ubu1 suite under fxjob, upstream-changes --check, CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Built: scripts/lib/host.sh (client) + scripts/lib/host-remote.sh (host half, sent over ssh as bash -s), replacing lib/gce.sh. A run = /mnt/fxdax-scratch/fxdax-host/RUN on FXDAX_HOST (ubu1): worktree of a commit origin has (fetched on demand), vendor/firecracker initialised, optional FXDAX_HOST_PATCH, trees/LABEL for other commits; the trees build in place (FXDAX_HOST_IN_PLACE), their own target dirs; job detached (setsid/nohup) under fxjob (--bench for timed drivers), as root with the user's Rust, CARGO_BUILD_JOBS/NEXTEST_TEST_THREADS/FXDAX_GUEST_KERNEL_JOBS=4, nice 19 for work; waited on by flock of the job's lock, reconnecting; results back by tar over ssh; run dir and worktrees removed whatever happens; resume/cancel for a driver stopped mid-run; leftover root processes of the job's scope reaped (TASK-353 for fxjob itself). Guest kernels: host_kernel in linux-host-setup.sh, cached on the host by scripts/guest-kernel-key.sh (now content-based, shared with CI's sandboxes cache). Drivers: test/bench/bench-ab/perf-gate/soak/soak-workloads/mutants/gets/migrate/fork-time-host.sh, guest-mutants-host.py, dax-gate.sh. Removed: the 11 GCE drivers, lib/gce.sh, gce-host-startup.sh, test_gce.py, fake_gcloud.py, the GCS results bucket (guest_results.py upload/fetch, its 2 tests). Tests: test_host.py (58, fake_ssh.py + fake_flock.py), test_bench_ab_host.py, test_perf_gate_host.py, test_linux_host Kernels/InPlace, test_soak_workloads cap tests, test_bench_history host runs. ubu1 setup: step 147-fxdax_host_runs.sh (linux-perf, bpftrace, cargo-mutants 27.1.0); the GCE startup's lease/poweroff/ready marker/needrestart were a disposable host's and are not ported (reasons in the step's header).

Proving runs on ubu1 (2026-11-11): test-host (fxfs-guest-dax pass, built the release kernel into the cache); bench-host (bench-fs-ext4-guest-kernel metadata, 1 iteration, kernel reused from cache); bench-ab-host (same-build A/B, then two commits HEAD~/HEAD in place, 1 round); perf-gate-host (all 11 cases pass both runs, gate ran; bigmap-read 3.53x < 4.0 -> TASK-352); soak-host (1024:2:2048, 1 round: driver ran, case timed out -> TASK-351); soak-workloads-host (crash group, FXDAX_SOAK_CAP=1: 3/3 pass); mutants-host (extent_frame shard 0/100: 3 caught, 1 unviable; twice); guest-mutants-host.py (task-230-create-keeps-listing killed by listings_follow_changes); gets-host (file:// 16 MiB, 1 s; twice); migrate-host (source and target phases pass, held bundle/bucket removed); fork-time-host (16x1 MiB base, 2 forks). Full Linux suite on ubu1 under fxjob via test-host.sh FXDAX_GUEST_TESTS=0: workspace 2488/2488 and every cargo part green; scripts part 277 OK after fixes (sqlite_extents socket path > SUN_LEN in deep trees; host_tree copy lacked .git -> in-place builds). macOS: clippy+workspace 2228/2228, scripts 283 OK. upstream-changes --check passes. Also fixed TASK-361 (main's 2 red scripts tests) to get CI green.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every GCE driver now runs on a Linux host of one's own over ssh (ubu1 by default) through one library, scripts/lib/host.sh with its host half host-remote.sh: a run is a worktree of a pushed commit (own target dirs, vendor/firecracker initialised, optional patch), its job under fxjob (--bench for measurements) as root with the user's Rust and -j4 bounds, waited on by a lock with reconnects, results back by tar, everything removed after (resume/cancel for a stopped driver; leftover root processes reaped). Guest kernels come from a content-keyed cache shared with CI (scripts/guest-kernel-key.sh). 11 drivers + guest-mutants-host.py + dax-gate.sh ported; GCE scripts, lib/gce.sh, gce-host-startup.sh, their tests and the GCS results bucket removed; tests ported to fake_ssh.py (test_host.py 58 tests, plus bench-ab/perf-gate/soak/kernel-cache tests). ubu1 step 147 adds perf, bpftrace, cargo-mutants. Verified: one real run of every entry point on ubu1 (see notes; soak and perf-gate exposed TASK-351, TASK-352), full Linux suite on ubu1 under fxjob green, macOS suite green, upstream-changes --check, branch CI 43064715753 green; landed 0c0d115c3. Filed TASK-346, 351, 352, 353, 356, 363.1.
<!-- SECTION:FINAL_SUMMARY:END -->
