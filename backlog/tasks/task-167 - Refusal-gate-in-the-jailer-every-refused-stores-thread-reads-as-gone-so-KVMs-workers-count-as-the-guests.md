---
id: TASK-167
title: >-
  Refusal gate: in the jailer every refused store's thread reads as gone, so
  KVM's workers count as the guest's
status: Done
assignee: []
created_date: '2026-11-04 08:37'
updated_date: '2026-11-04 16:35'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 620000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r534 (batch 2026-11-04b, FXDAX_FIRECRACKER_JAIL=1, FXDAX_FIRECRACKER_SECCOMP=real): fxfs-guest-pgbench-dax and the four other suites pass, but scripts/dax-refusals.py fails the run on 791 refusals, each 'by thread N (gone)': uffd.rs's thread_name reads /proc/self/task/N/comm and /proc/N/comm, which the jailed VMM can't see, so a KVM async page-fault worker's store (TASK-156's excused case) can't be told from a vCPU's. Never met before: TASK-69's jailed runs had pgbench without DAX. Fix: classify by a registry of the VMM's vCPU threads (batch 2026-11-04c's fxdax::guest_thread_started) rather than by /proc names: log 'vCPU' or 'not a vCPU', and have dax-refusals.py fail only on vCPUs. Copy-on-write DAX (TASK-164) turns most of these refusals into copies anyway.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A jailed pgbench-dax run passes the refusal gate, its workers' refusals classified as not the guest's
- [ ] #2 A vCPU's refused store still fails the gate, in and out of the jailer
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A refused store's log says whether a vCPU made it, from the VMM's own registry of its vCPU threads, not /proc, which a jailed VMM lacks; scripts/dax-refusals.py fails only on a vCPU's (test_the_vmm_says_whose_a_store_was, red under the old rule). GCE r543 and r546: jailed pgbench-dax passes the gate. Verified: macOS scripts/test.sh (workspace 2,089/2,089, Shuttle, scripts); GitHub CI, every job (run 41859737932: Linux workspace, Shuttle incl. the uffd private-memory models, guest-sim's 3,000 seeds and mutants); GCE r546 copy-on-write off in the jailer under the production seccomp filter (5 suites, refusal gate clean), r547 copy-on-write on (crash-dax, pgbench-dax-cow, pgbench-dax-cow-forked, bigmap-dax-cow pass).
<!-- SECTION:FINAL_SUMMARY:END -->
