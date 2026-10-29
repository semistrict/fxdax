---
id: TASK-119.21
title: >-
  Guest DAX: with the diagnostics on, checks-dax's guest accesses a window page
  the host maps nothing at, and the VM stops (vCPU EFAULT 1 s)
status: To Do
assignee: []
created_date: '2026-10-29 13:35'
updated_date: '2026-10-29 15:03'
labels:
  - dax
  - bug
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 467000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r228c (main 7b146e0b5, kernel built), fxfs-guest-checks-dax with FXDAX_GUEST_CMDLINE='fxfs.dax_check_moves=1 fxfs.dax_shadow=1 fxfs.dax_check_seals=1 fxfs.dax_verify_seals=1': after direct_io passed, the VMM logged 'the DAX window refused an access to a page the window maps nothing at, at offset 0x1002a000 (made 34)', the page withheld, and a second later 'Failure during vcpu run: Bad address': the VM stopped. The guest took a page as mapped that the host had unmapped: guest and window disagree. Before it, the store to window offset 0x10341000 was refused 8 times in 3 s (made 21 to 61), and 12 times at the same offset in the mutant run r228m with check_moves and shadow on: a page refused again and again (TASK-44.12's signal). check_moves logs every kept range from Rust (fxfs_set_check_kept, ~3 ms a line on the serial console, 4850 lines), which may only widen a race present without them. Artifacts: /tmp/fxdax-r228c/e2e/fxfs-guest-checks-dax.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Which diagnostic's run reproduces it is known, and the access's window offset traced (fxfs.dax_trace) to the guest's mapping of it
- [ ] #2 Fixed, or shown a diagnostic's own fault; checks-dax with every diagnostic on passes on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r229 (each diagnostic alone, checks-dax, 1 round): check_moves and check_seals passed; verify_seals failed on its own findings ('sealed region ... page 1 at 106b1000: 1 bytes differ from the device's: 0x0 through the window, 0x1 the device's', and dozens more, all zeros through the window) with refusals of accesses to pages the window maps nothing at (0x106a0000, 0x12fcf000, 0x13202000): fxfs_dax_verify_sealed reads a sealed region's pages through the window holding a region reference, but not the extent, which Fxfs frees once evacuations empty it, the window then mapping zeros or nothing there: a racy diagnostic, and the likely source of r228c's withheld page (0x1002a000) and VM stop. shadow alone: every guest check passed, but the host's fsck failed: TASK-119.22.

401bbf9e6 (main): verify_seals no longer reads sealed staging regions after the seal (verify_sealed removed), only pages written since the last seal as a seal begins (verify_writable). GCE r232, checks-dax with all four diagnostics on: still stopped in round 1 of 2, after stats, in log_writes: 'an access to a page the window maps nothing at, at offset 0x10671000', the vCPU failing a second later. Before it, a store at 0x10341000 refused 9 times in 3 s: the same offset in every run with dax_check_moves on (r228c, r228m, r229-check_moves, which passed, r232), in none without: check_moves (fxfs_set_check_kept, Rust's kept-range checks) changes what the guest does there. Next: the four diagnostics in pairs, to find which pair stops the VM; fxfs.dax_trace for 0x10671000's history.

GCE r235 (main 98c82281b), checks-dax with pairs (r234's pairs were malformed, a zsh loop): check_moves+shadow and shadow+check_seals passed (516 and 186 refusals, many 'maps nothing at', which resolved); shadow+verify_seals stopped the VM after stats, in log_writes: 'an access to a page the window maps nothing at, at offset 0x10802000', the vCPU failing a second later. So fxfs_dax_verify_writable, as a seal begins, reads a page the shadow says the guest mapped writably since the last seal while the host maps nothing there: the shadow stale (an unmapping it doesn't record), or the host's unmapping one the guest still takes as mapped. Next: which, by fxfs.dax_trace of the offset.
<!-- SECTION:NOTES:END -->
