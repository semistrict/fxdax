---
id: TASK-229
title: >-
  A sandbox's cold read of the Linux checkout over its attached generation takes
  26-46 s, against 5.6 s in a fresh VM on the publisher's store, with the data
  already in the daemon
status: To Do
assignee: []
created_date: '2026-11-07 10:46'
labels: []
dependencies: []
references:
  - TASK-136.1
  - TASK-227
  - docs/measurements/linux-checkout-2026-11-07.md
priority: medium
type: bug
ordinal: 664000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in TASK-136.1's and TASK-227's linux-checkout-fxfs-dax runs (GCE r872, r881, r882, r888, r889). The case's sandboxes attach the published generation of the 1.54 GB Linux tree (91k files) and read every working-tree file once with caches dropped. That cold read takes 26-31 s on an 8 GiB window with 8 GiB of daemon memory (r881-r889) and 34-35 s with 6 GiB (r872). A fresh VM that opens the publisher's own store and reads the same tree takes 4.85-5.6 s (fxfs-dax reopened), and ext4 on virtio-pmem takes 8.7-9.4 s. Warm rereads are fast everywhere: 560-590 ms in the sandboxes.

What is known (r888, the host's memory lines over each VM's cold read, phases ROLE-status to ROLE-read in the case's log.txt):
- It is not bucket fetches or daemon page-ins. The daemon downloaded 39.7 kB for 3.6 GB read from its bucket over the whole run. sandbox-0 paged in 2,088 times (2,664 faults, the daemon's memory grew 1.77 GB). sandbox-1 and sandbox-2 had 0 page-ins and 0 faults, so the daemon already held everything, and still took 26-30 s.
- The data is read through DAX, not the page cache: the guest's Cached grew 9.7 MB.
- A sandbox's window maps 529,469 pages (2.17 GB) for the 1.54 GB read; the reopened VM's maps 402,229 (1.65 GB).
- The sandboxes' cold git status (0.87-1.11 s) is close to the reopened VM's (0.99 s), so metadata lookups alone don't explain it.
- Without DAX, Fxfs's cold read is slow everywhere (r872: reopened 43.4 s, sandboxes 41.9-46.4 s). DAX makes the publisher's store fast but leaves the sandboxes close to the non-DAX time.
- docs/measurements/linux-checkout-2026-11-07.md says a sandbox is slow because the daemon doesn't hold the attached generation warm. r888's sandbox-1 and -2 contradict that.

Open: the time split per VM (MAPs, their latency, guest faults, the layered volume's extent lookups in the guest). scripts/extent-transport.bt aggregates the whole run, not per VM.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Where a sandbox's cold read spends its time is measured per VM and recorded (transport trace or guest profile), against the reopened VM's in the same run
- [ ] #2 A sandbox's cold read of the Linux checkout, with its base already in the daemon's memory, takes no more than 1.5x a fresh VM's on the publisher's store in the same run, or the remaining gap is explained and recorded in OPEN_QUESTIONS
- [ ] #3 docs/measurements/linux-checkout-2026-11-07.md's explanation of the sandboxes' cold read is corrected
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
