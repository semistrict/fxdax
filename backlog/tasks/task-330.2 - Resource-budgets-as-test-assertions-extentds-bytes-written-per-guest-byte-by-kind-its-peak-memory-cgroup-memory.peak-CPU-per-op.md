---
id: TASK-330.2
title: >-
  Resource budgets as test assertions: extentd's bytes written per guest byte by
  kind, its peak memory (cgroup memory.peak), CPU per op
status: Done
assignee:
  - '@claude'
created_date: '2026-11-10 14:00'
updated_date: '2026-11-10 17:16'
labels:
  - testing
dependencies: []
parent_task_id: TASK-330
priority: high
ordinal: 731000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-310 added per-kind written-bytes counters to extentd's stats; TASK-317 added cgroup peak memory to test-e2e.sh (FXDAX_EXTENTD_MEMORY_MAX). Turn them into assertions with budgets in the e2e and bench cases. Example: write amplification at most 1.3x the guest data plus framing; peak anonymous memory at most the configured budgets plus a fixed slack. A tuning change that trades one metric for another then fails here, not at a customer.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 TASK-310's double write and TASK-317's 6 GB growth, each put back, fail these assertions
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Daemon-side: extentd::process samples RssAnon/RssShmem and CPU each stats poll against an anonymous-memory budget derived from doc-7 Memory (upload memory + 288 MiB framing per store written at once + 192 MiB fetches + tenants' rooms' peak + 64 MiB own); stats line 'process:' and metrics.
2. Write accounting: extent_store::written counts writes; daemon line written_writes, stats_written_bytes.
3. scripts/budgets.py, run by test-e2e.sh (and so bench.sh) over every daemon's stats file: fails on memory over budget, uploads past budget, disk writes (io_write_bytes) past what the daemon says it wrote; reports bytes per guest byte by kind and CPU per request.
4. remote::write_budgets: a host store's large-file and small-file workloads held to the design's per-kind bounds (TASK-310's spill + in-place framing), and on Linux io_write_bytes to the counted writes.
5. Mutants (crates/fxdax/mutants): large-extents-framed-in-copies, written-back-twice (write_budgets), uploads-copied-each-try (soak memory budget).
6. Verify on ubu1: unit tests, mutants killed, e2e cases pass budgets.py.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Built on TASK-310's counters (on main since 5949ac1f2/80e682ed5; this work was based on t310-land, rebased onto main once TASK-310 landed).
Budgets, each derived in code from the design, not measured numbers plus slack:
- Memory (extentd::process): every 200 ms RssAnon against upload memory + 288 MiB per store written at once (a framing's data and resident framed bytes, 2 x (segment + extent), TASK-325; plus a commit's extent copy and two 16 MiB chunks) + 192 MiB fetches + tenants' rooms' peak + 64 MiB own. First tried 384 MiB per store (framed Vec capacity): that left too little room above a correct daemon for an upload held twice to show; RSS counts written pages only, so 288 is the bound. Stats 'process:' line and metrics fxdax_extentd_process_*.
- Disk (scripts/budgets.py, every test-e2e.sh/bench.sh run): the daemon's io_write_bytes <= its stores' counted bytes + a page per write + stats file pages + log + bucket objects (uploaded + upload memory in flight, if the bucket is on a disk) + fetched parts. The in-flight term came from a run whose case killed its daemon with PUTs in flight (1.35 GB over without it).
- Write amplification by kind (remote::write_budgets, a host store with production options): large file 768 MiB incompressible in two bursts past the segment limit: rewritten (copied+own+framed) <= bursts x 2 x segment_limit + overhead + a tail per extent; measured 384 MiB of a 384 MiB bound (the design's exact cost). Small files: never copied nor own, framed <= raw + framing overhead + log. Linux: io_write_bytes <= counted + a page per write.
- CPU per op: measured and reported (process cpu-ns, metric, budgets.py cpu_ns_per_request, the write test's CPU), not asserted. A 2x-of-framing bound I tried failed on ubu1 in the test's debug build (2.8 s against 2.57): no design statement bounds CPU, and a test build is no measurement.
Verification (ubu1 unless said):
- Mutants (crates/fxdax/mutants.sh): large-extents-framed-in-copies killed (960 MiB rewritten against 384), written-back-twice killed (1545 MiB written for 1167 counted), uploads-copied-each-try killed by the soak's memory budget (60 MiB past 2944 within 47 s; with guests, 3.1 GB of 2.95 GB).
- e2e fxfs-guest, -dax, -checks (t263 kernel, prebuilt artifacts): budgets.py passed: anon 264 MiB of 1058, disk 5464 MiB written of 6559 allowed; per guest byte raw 1.0 copied 0.42 framed 0.02 log 0.01.
- macOS scripts/test.sh passed (2219 workspace tests, sim, shuttle, scripts, clippy, mutants --check). Linux workspace+sim+shuttle passed on ubu1 (2475/2475). scripts/upstream-changes.py --check passes.

Main CI 42859291294 green with these commits (every job).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd now holds itself to a memory budget derived from doc-7 (upload memory, 288 MiB per store written, fetches, rooms, its own), sampled every 200 ms and reported in its stats line and metrics. scripts/budgets.py fails every test-e2e.sh/bench.sh run whose daemon passed that budget, its upload budget, or wrote more to disk than it counted. It also reports bytes per guest byte by kind and CPU per request. remote::write_budgets holds a host store to the design's per-kind write cost for a large file and small files. Verified on ubu1: TASK-310's double write (framing in copies), a write-behind double write, and TASK-317's per-try copy each fail a budget as mutants. e2e fxfs-guest/-dax/-checks pass budgets.py. The macOS and Linux suites pass. CPU per op is measured and reported but not asserted: no design statement bounds it.
<!-- SECTION:FINAL_SUMMARY:END -->
