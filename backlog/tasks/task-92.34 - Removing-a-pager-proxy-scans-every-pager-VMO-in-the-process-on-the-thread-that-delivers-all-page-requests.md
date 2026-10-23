---
id: TASK-92.34
title: >-
  Removing a pager proxy scans every pager VMO in the process, on the thread
  that delivers all page requests
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 309000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Proxies sit in a Vec, and release_proxy finds one with a linear scan under the pager lock (pager_dispatcher.rs:57, 144-151), where upstream removes in O(1); the deviation is undocumented. extentd's single pager owns every cached extent's VMO (extents.rs:213-217), and the release runs on the single port-receiver thread (port_dispatcher.rs:345-349), so every VM's page requests wait. create_source takes the same lock inside the cache mutex. The cost is (VMs × FREE rate) × host-wide extent count, and evict_arena costs k·n, which can mean seconds.

Where: crates/zircon/src/object/pager_dispatcher.rs:57, crates/zircon/src/object/pager_dispatcher.rs:120, crates/zircon/src/object/pager_dispatcher.rs:144-151, crates/zircon/src/object/pager_proxy.rs:238-241, crates/zircon/src/object/pager_proxy.rs:295-299, crates/zircon/src/object/port_dispatcher.rs:345-349, crates/fxdax/src/extents.rs:213-217, crates/fxdax/src/extents.rs:1689-1706.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. crates/zircon/src/object/ is untouched since 6a0a3f8. Proxies are a Vec (pager_dispatcher.rs:57), and release_proxy does a linear position() scan under the pager lock before swap_remove (pager_dispatcher.rs:144-151). FxExtent::new (create_source) still runs inside the cache.extents mutex in cached() (crates/fxdax/src/extents.rs:1638-1650). Not tracked. TASK-53 fixed the analogous linear mapping removal. Related: TASK-53, TASK-16, TASK-79.7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [x] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A pager now keeps its VMOs' proxies in a BTreeMap keyed by address instead of a Vec. Releasing one proxy on the port thread now takes logarithmic time instead of scanning every pager VMO, and this deviation from upstream's intrusive list is documented in the module and on the field. Releasing 50,000 pager VMOs went from 3.8-4.4 s to 0.16-0.18 s (macOS, debug), measured by many_pager_vmos_are_let_go_of_in_any_order. The extent cache still creates an extent's VMO under its own mutex, but the pager lock taken there is now held for only one insert. The macOS and Linux (GCE) suites pass.
<!-- SECTION:FINAL_SUMMARY:END -->
