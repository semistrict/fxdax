---
id: TASK-313
title: >-
  test_cases' guest unit tests print 'deadlock in SIGSEGV handler' under a
  loaded host
status: To Do
assignee: []
created_date: '2026-11-10 00:42'
labels: []
dependencies: []
priority: low
type: bug
ordinal: 721000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/test-e2e.sh's 'cargo test -q -p test_cases --features guest' on ubu1 (load ~65, other agents' VMs) printed 'deadlock in SIGSEGV handler' between the 87th and 114th test; all 114 passed. The message is std's stack-overflow SIGSEGV handler (library/std/src/sys/pal/unix/stack_overflow/thread_info.rs, with_current_info) giving up after 10M spins on THREAD_INFO's lock: a thread took SIGSEGV while another held that lock (a thread being created or exiting). Some test of differential.rs / script_run.rs (hostile scripts, refusals, planted bugs; four were running >60 s then) takes a SIGSEGV it means to handle, or crashes a child: find which, and whether its SIGSEGV is handled by a handler of its own or falls to std's, which then aborts the process it lands in if the spin gives up. Found while working TASK-311.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test that takes the SIGSEGV is named, and its fault handled without std's stack-overflow handler's spin, or the message explained as harmless with evidence
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
