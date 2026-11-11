---
id: TASK-344
title: >-
  Every GCE script ported to a physical Linux host (ubu1): tests, benches, A/Bs,
  soaks, mutants, migrations, fork time, perf gate; then GCE removed
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 07:29'
updated_date: '2026-11-11 07:38'
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
- [ ] #1 Every GCE entry point has a physical-host equivalent that runs on ubu1 under fxjob and is proven by one real run there
- [ ] #2 The GCE scripts' tests are ported to the new scripts with the same coverage, then the GCE scripts, lib/gce.sh, gce-host-startup.sh and their tests are removed
- [ ] #3 rg -i 'gcloud|gce' outside docs/measurements and backlog finds nothing that tells anyone to use GCP
- [ ] #4 Anything a GCE host's setup installed that ubu1 lacks is a system-setup.d step
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
