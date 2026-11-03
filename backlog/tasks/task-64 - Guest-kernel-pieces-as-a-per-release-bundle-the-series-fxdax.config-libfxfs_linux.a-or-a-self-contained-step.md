---
id: TASK-64
title: >-
  Guest kernel pieces as a per-release bundle: the series, fxdax.config,
  libfxfs_linux.a or a self-contained step
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-11-03 06:16'
labels: []
dependencies: []
priority: low
ordinal: 173000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
build-guest-kernel.sh owns .config and builds libfxfs_linux.a with a pinned nightly and fxdax's sysroot. The sandbox team needs a supported per-release bundle their kernel Dockerfile applies on its own config: the patch series, fxdax.config, and a prebuilt libfxfs_linux.a or a self-contained step to build it. Cosmetic: kallsyms warns 'Please increase KSYM_NAME_LEN' for long Rust symbols; vmlinux is 468 MB with debug info (51 MB after objcopy --strip-debug --keep-section=.BTF). Production blocker.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-03: the user: the sandbox team has all the source and handles config and integration themselves; not ours to do.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Dropped, not done (2026-11-03): the sandbox team has all of fxdax's source and handles building and integrating it themselves; the user said not to work on it.
<!-- SECTION:FINAL_SUMMARY:END -->
