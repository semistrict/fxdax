---
id: TASK-25.5
title: 'libkrun: set the guest''s clock precisely at boot'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 19:08'
updated_date: '2026-10-10 21:05'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: bug
ordinal: 30000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Files on virtio-fs are stamped by the host's clock, which is ~170 ms ahead of a libkrun guest's on GCE (mtime-ahead-of-guest in docs/measurements/gce-fs-bench-2026-10-10T1823Z.md), while ext4 stamps them with the guest's own. make warns of clock skew, and a build's generated files can look out of date (GNU make's configure failed once, gce-fs-bench-2026-10-10T1731Z). The guest's clock starts from the RTC's whole seconds. Set it from the host's at boot, to the microsecond, in libkrun's init.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A file the guest writes on virtio-fs is stamped within 5 ms of the guest's clock, measured by the benchmark's mtime observation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Find why the guest's clock lags the host's (kernel log, clocksource; KVM PTP clock against the guest's). 2. Set the guest's clock from KVM's PTP clock in libkrun's init at boot (one ADJ_SETOFFSET). 3. Measure the mtime observation on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause (T2057Z, T2100Z): the guest boots on kvm-clock and switches to the TSC, its wall clock 170 ms behind the host's; KVM's PTP clock (/dev/ptp0) reads the host's in ~10 µs. libkrun's init now moves the guest's clock onto it at boot with one ADJ_SETOFFSET (libkrun 660fd98). Verified on GCE (T2103Z): mtime-ahead-of-guest 0.13 ms on fxdax, -0.19 ms through passthrough. Only on KVM: on macOS the timesync over vsock stays. Workspace tests do not cover init.c (it runs in the guests the benchmark boots, which exercised it); init.c compiles with -Wall in Lima without new warnings.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
libkrun's init sets the guest's clock from the host's at boot through KVM's PTP clock (one ADJ_SETOFFSET), so files on virtio-fs, stamped by the host's clock, match the guest's. Verified on GCE (docs/measurements/gce-fs-bench-2026-10-10T2103Z): the benchmark's mtime observation went from 170 ms to 0.13 ms (fxdax) and -0.19 ms (passthrough).
<!-- SECTION:FINAL_SUMMARY:END -->
