---
id: TASK-311
title: >-
  Guest kernel: oops then soft lockups (smp_call_function_many_cond from
  kvm_flush_tlb_multi/wp_page_copy) in 8-vCPU docker-build VMs
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 23:57'
updated_date: '2026-11-10 00:07'
labels:
  - guest-kernel
dependencies: []
priority: high
type: bug
ordinal: 719000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Reported by LangSmith after the bump 6a513509a -> 96c7808b8. Two of 17 CI snapshot builds crashed, both during docker image export in an 8-vCPU builder VM on an 8-vCPU runner. GCE saw 0 of 104, half of those with the full CI suite running alongside.

Kernel 6.18.54+ on virtio-PCI. The kernel was tainted D, so an oops came first; it scrolled out of their 32 KiB console tail.

About 28 s after boot, all vCPUs reported soft lockups for 4+ min in smp_call_function_many_cond, called from kvm_flush_tlb_multi / wp_page_copy. Other stacks:
- One vCPU (docker-compose) spins with IRQs off in __pv_queued_spin_lock_slowpath, from list_lru_del / workingset_update_node / filemap_add_folio.
- docker-buildx is in D state in make_task_dead -> exit_mmap.
- In the second crash, a vCPU waits in __pte_offset_map_lock from do_anonymous_page.

Reading: a task died holding a spinlock, and every other vCPU then hung on TLB-flush IPIs.

Kernel changes in the range: virtio-MMIO dropped, ftrace enabled, patch 0013 (mprotect PTE batching, only smaller batches).

LangSmith's host now logs 16 KiB from the first fault report, and they will send the oops when it recurs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxdax's own VM console capture keeps the first oops (the first report, not only the tail)
- [ ] #2 Attempted reproduction on ubu1: 8-vCPU guest, docker build plus image export under host CPU contention, debug kernel (lockdep, KASAN), N rounds recorded
- [ ] #3 Root cause identified from the first oops (ours or LangSmith's) and fixed, with a test or mutant that catches it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Keep the first oops: vm::first_report (first complaint, its context and trace, 16 KiB at most); the runner prints each console's first report for failed/timed-out tests and records it in the outcome JSON and GitHub summary; 'Oops:' becomes a complaint; log tail cut at a char boundary.
2. Fail fast: CONFIG_PANIC_ON_OOPS=y and CONFIG_PANIC_TIMEOUT=-1 in fxdax.config (a panic is a crash doc-12 covers; the oops is then the console's last report); build script checks them; fxfs-guest-checks oops_panics checks the booted kernel. No pstore/ramoops: a Firecracker VM's RAM goes with it.
3. Reproduce on ubu1: 8-vCPU guest, docker build + image export, host CPU contention, release and debug (lockdep/KASAN/DEBUG_LIST) kernels, N rounds.
4. Root-cause and fix with a test/mutant if reproduced; else leave open with notes on what LangSmith should send.
<!-- SECTION:PLAN:END -->
