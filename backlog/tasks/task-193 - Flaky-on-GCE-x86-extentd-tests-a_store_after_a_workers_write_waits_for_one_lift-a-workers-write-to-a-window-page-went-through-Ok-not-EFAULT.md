---
id: TASK-193
title: >-
  Flaky on GCE x86:
  extentd::tests::a_store_after_a_workers_write_waits_for_one_lift, a worker's
  write to a window page went through (Ok, not EFAULT)
status: To Do
assignee: []
created_date: '2026-11-06 04:46'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 644000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r692 (2026-11-05, n2-standard-4 us-west1-b, at 089b2d112 with TASK-141.3/141.4's batch, which touches no DAX code): the workspace's only failure, crates/fxdax/src/extentd/tests.rs:975, 'the worker's write went through: left Ok(()), right Err(14)', after 4.2 s. It passed in Docker (aarch64) the same day. TASK-187's test, landed 2026-11-05: the worker's kernel write to a page of a 1 GiB copy-on-write window, every 64th page, is expected restricted (EFAULT) until the guest's own store lifts it; once in 1000 pages here it wasn't. Results: /tmp/fxdax-r692/fxdax-tests.log on the machine that ran it.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
