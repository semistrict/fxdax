---
id: TASK-164.1
title: 'Host: copy-on-write DAX window pages'
status: Done
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-06 06:56'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 611000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
DAX_COW, private_max, MAP_ZEROS, PRIVATE in the extent device (doc-13 stage 1), offered beside today's writable mode.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store to a sealed or zero page gets a private copy at the same window address; MAP and UNMAP free it; eviction skips it
- [ ] #2 Past private_max refused (exit 159); KVM's async writes don't copy
- [ ] #3 doc-4 says so; unit, Shuttle and GCE tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**Copy-on-write DAX's private pages are the VM process's memory, laid out as the window is (TASK-164.1, 2026-11-04)**

Each private page was a page of the tenant's arena, mapped in the VM process on its own: a mapping each, as the arena's frames aren't laid out by window offset, so the second stage's run budget (about 30k) trimmed and reinstalled them by the thousand; r544 held 110,857 and its fault thread trimmed instead of lifting a vCPU's restriction (`KVM_RUN` kept failing with `EFAULT`, exit 159). Decided: a window that copies on write has the VM process's private memory (`dax::uffd::PrivateMemory`), a memfd as large as the window, mapped shared at the window's offsets wherever nothing is installed, only private pages filled (`UFFDIO_COPY` through a second view, holes elsewhere so faults still go to the stage). `STAGE_PRIVATE` (108) has the VM process copy the source page (pread of the arena it holds) or zeros in; an unmap or map over it punches the hole. Adjacent private pages and the memory around them are one mapping; trims never touch them. The daemon keeps only their offsets and count; `private_max` is still pinned against the tenant's arena as the window is made.
**Cost:** the private pages' memory is the VM process's (its memcg), no longer in the arena's bytes: a tenant's limit no longer bounds it, `private_max` does.
**Instead:** (1) private frames allocated next to their window neighbours' (`alloc_after`): best effort, and scattered pages (pgbench's) still cost a mapping each; (2) a per-window physmem arena laid out by window offset: chunk-granular accounting charges 2 MiB per scattered page; (3) charging the memfd against the tenant's cap (physmem `charge`): possible follow-up.
**Undoing it:** `Window::copy` back to `physmem::alloc` + `map_run`, and `leave_out` to anonymous memory.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The host offers copy-on-write DAX windows: a store to a sealed or zero page gets a private copy at the same address, held in private memory laid out like the window so adjacent private pages are one mapping and trims never touch them (the first design, a mapping per page, overran the VMM's mapping budget at 110,857 pages on GCE r544 and stalled vCPUs); private_max reserved against the tenant; MAP_ZEROS, PRIVATE; a driver's features page written again replaces it. Verified: macOS scripts/test.sh (workspace 2,089/2,089, Shuttle, scripts); GitHub CI, every job (run 41859737932: Linux workspace, Shuttle incl. the uffd private-memory models, guest-sim's 3,000 seeds and mutants); GCE r546 copy-on-write off in the jailer under the production seccomp filter (5 suites, refusal gate clean), r547 copy-on-write on (crash-dax, pgbench-dax-cow, pgbench-dax-cow-forked, bigmap-dax-cow pass).
<!-- SECTION:FINAL_SUMMARY:END -->
