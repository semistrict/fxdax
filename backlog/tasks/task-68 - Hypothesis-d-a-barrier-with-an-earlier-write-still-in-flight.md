---
id: TASK-68
title: 'Hypothesis (d): a barrier with an earlier write still in flight'
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-11-04 07:06'
labels: []
dependencies: []
priority: medium
ordinal: 177000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Still a note in TASK-26.6.2. extentd serves up to 8 (WORKERS) of a connection's requests at once, so a barrier orders only completed requests. A test, or an argument from the code, that Fxfs's journal never sends a barrier with an earlier write still in flight.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every barrier or commit request begins a Seal that waits for the device's writes in flight (fxfs-linux device.rs Seal::begin_with), so no barrier goes out with an earlier write unfinished.
<!-- SECTION:FINAL_SUMMARY:END -->
