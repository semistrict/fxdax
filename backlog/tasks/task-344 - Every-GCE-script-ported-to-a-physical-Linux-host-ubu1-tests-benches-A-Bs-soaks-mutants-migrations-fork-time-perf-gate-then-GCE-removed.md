---
id: TASK-344
title: >-
  Every GCE script ported to a physical Linux host (ubu1): tests, benches, A/Bs,
  soaks, mutants, migrations, fork time, perf gate; then GCE removed
status: To Do
assignee: []
created_date: '2026-11-11 07:29'
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
