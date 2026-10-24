---
id: TASK-99
title: >-
  2 GiB DAX VM: Firecracker exits on KVM_RUN EFAULT ('Bad address') 4 s into
  pgbench's load
status: To Do
assignee: []
created_date: '2026-10-24 20:05'
updated_date: '2026-10-24 20:10'
labels:
  - bug
dependencies: []
priority: high
ordinal: 385000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team, 2026-10-24, fxdax at ae54e01: the 2 GiB-window DAX pgbench VM died 4 s into the data load; Firecracker logged 'Failure during vcpu run: Bad address (os error 14)' and exited; nothing on the guest console before. KVM_RUN's EFAULT is a DAX window page the host's mapping of was invalid (or unregistered) as KVM faulted on it; Firecracker retries EFAULT for a moment only (dax::uffd docs). The 8 GiB variant and no DAX ran. Related: TASK-79.107 (a held device write killed the VM with Bad address).
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Firecracker (vcpu.rs, fxdax) retries KVM_RUN's EFAULT for 1 s (Efaults::PATIENCE) and then exits: the access stayed unresolvable, the window refusing the fault (nothing mapped there, ENOENT; or a store to a read-only page, EACCES). The VMM logs it first: 'unresolved DAX window fault at offset <offset> (<access>): <err>' (dax/uffd.rs). Needed from the sandbox team: that line, extentd's log around it, and the guest's dmesg if it can be kept (printk 7). Candidate: TASK-79.107's path, a fault's held copy failing to allocate (keep=true) under memory pressure, whose device write to a staging extent not yet open in extentd isn't what the window maps writably. Our fxfs-guest-pgbench-dax (2 GiB window, same load) passed every run today on GCE (r65, r68, r69 kernels b4/b6).
<!-- SECTION:NOTES:END -->
