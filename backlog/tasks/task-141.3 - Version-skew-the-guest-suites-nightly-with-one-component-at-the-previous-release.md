---
id: TASK-141.3
title: >-
  The VM process, the daemon and the guest refuse a peer of another wire format:
  no version skew
status: Done
assignee:
  - '@claude'
created_date: '2026-10-31 20:30'
updated_date: '2026-11-06 04:47'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 490000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
AGENTS.md (no migrations, the user, 2026-11-05) covers wire formats: a VM process, a daemon or a guest of another build is refused, not served. Nothing checks today. OPEN carries no wire version (crates/fxdax/src/extentd/wire.rs); a request a newer VM process adds is refused EINVAL by an older daemon and let be, as BUILD is (wire.rs:177-181; crates/fxdax/src/extentd/client.rs:136-140); and a reply that grows, as OPEN's did with the host's max_map_count in args[3] (0f3ae7a36), is read by an older VM process as whatever it was. The guest's driver and the device negotiate virtio feature bits (doc-4), which let a guest run on a device of another build.

Proposal: one wire version, said by the VM process in OPEN and checked by the daemon, and one device protocol version the guest's driver checks at probe; any other refused, both named. BUILD then says nothing the version doesn't, or is required.

Rewritten 2026-11-05 from "Version skew: the guest suites nightly with one component at the previous release" (daemon N with VMM N-1, VMM N with guest N-1, optional requests tolerating EINVAL), which AGENTS.md's rule forbids.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 OPEN carries the VM process's wire version, and a daemon refuses another with an errno and a log naming both, by test
- [x] #2 The guest's driver refuses a device of another protocol version at probe, naming both
- [x] #3 No request is optional: nothing lets a peer's EINVAL for a request it doesn't know be (client.rs:136-140)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. wire::VERSION (1): OPEN's args[4] says the VM process's, its reply's args[4] the daemon's: the one place no version moves. The codec refuses an OPEN of another version (EPROTONOSUPPORT) before anything else of it; the daemon logs both and replies with its own; the VM process refuses a reply of another (an older daemon's 0), naming both.
2. A test fingerprints the wire (every request's encoding, Told/stage ops, replies' args, Header's layout, the bounds): a change of it without a new VERSION fails, naming the fingerprint to set.
3. No request optional: BUILD and TRANSFER required, failing the connection (client.rs over()); RemoteExtentStore's transfer area no longer an Option.
4. Device protocol: virtio_extent_config's first field version (VIRTIO_EXTENT_VERSION 1), Firecracker's ConfigSpace too; the driver refuses another at probe, naming both (module param virtio_extent.version for its test); after DRIVER_OK the driver fails probe on NEEDS_RESET.
5. Firecracker: activation refuses a driver that didn't take every feature the device offers (offered and taken named); INFO's short reply for a driver without F_FREEZE goes.
6. Tests: codec (other versions refused, fingerprint), extentd tests (daemon refuses and logs; client refuses daemon of version 0), Firecracker unit tests (config's version; activation refused), guest test case booting with virtio_extent.version=2 (no device bound, log names 1 and 2).
7. doc-4, doc-11, wire docs updated.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: rewritten to fit AGENTS.md's no-migrations rule (75bd872ba): skew tested to work became skew refused.

2026-11-05 implemented (6d3461d49 fxdax side; guest and vendor/firecracker pending GCE r692):
- Wire: wire::VERSION 1 in OPEN's args[4] and its reply's (codec::version, with_version). Daemon refuses another before decoding the rest (EPROTONOSUPPORT, log names both, reply carries its own); VM process refuses a reply of another, naming both (an older daemon's 0 included). the_wire_is_its_version fingerprints the wire against (VERSION, 0x4d5c_8480_5312_69da): a wire change without a version bump fails it.
- No optional requests: BUILD and TRANSFER required (transfer area no longer an Option); every failure after OPEN shuts the connection down (before, failures after OPEN left the connection open holding the store: a bug found in passing, fixed by over()/open() split).
- Device: virtio_extent_config.version first (VIRTIO_EXTENT_VERSION 1); driver refuses another at probe (module param virtio_extent.version for its guest test); driver fails probe on NEEDS_RESET after DRIVER_OK (and no longer removes a sysfs group it never made); Firecracker refuses activation unless the driver took every offered feature (took_every_feature), INFO's short F_FREEZE-less reply gone; ExtentState.protocol, a restore of another refused (ExtentError::Protocol); ExtentError::Store shows its causes ({1:#}), which hid the version message.
- Tests: codec opens_of_other_wire_versions_are_refused, the_wire_is_its_version; extentd vm_processes_of_other_wire_versions_are_refused, daemons_of_other_wire_versions_are_refused (rewritten from frees_of_many_extents_fail_against_daemons_without_them: the skew it tested is refused at OPEN now), openings_fail_on_refusals_of_what_they_ask; Firecracker the_configuration_says_its_version_first, drivers_take_every_feature_offered, snapshots_of_other_protocols_are_refused; guest case extent-guest-other-version.
- Structure: the wire's encoding was already in codec.rs alone (TASK-160.9), so the version is one field there; what kept versions from being enforced was that nothing tied VERSION to the encoding, which the fingerprint test does.
- Runs: macOS workspace 2118/2118 + shuttle; Linux (Docker aarch64) workspace 2307/2310 (3 fxdax::grants need a jail helper absent in the container); Firecracker vmm extent:: 17/17.
- Overlap: TASK-111.4 (stats) and TASK-165.7 (storage) must bump wire::VERSION with any wire change; TASK-160.7 (guest driver) touches driver.c's probe.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
VM process and daemon say wire::VERSION (1) in OPEN's args[4] and its reply's and refuse another, naming both (6d3461d49); every request required (BUILD, TRANSFER); a fingerprint test ties VERSION to the wire's encoding. The device's VIRTIO_EXTENT_VERSION (1) is its configuration's first field, the driver refuses another at probe, Firecracker refuses a driver that didn't take every offered feature and a snapshot of another protocol (vendor/firecracker 3d1e01bd2, guest aa8d91970). Verified: macOS workspace 2118/2118 + shuttle; Linux Docker 2307/2310 (3 grants tests needing a jail helper the container lacks); Firecracker vmm extent:: 17/17; GCE r692 jailed: fxfs-guest-dax, fxfs-guest-checks, extent-guest, extent-guest-other-version pass, workspace 2309/2310 (TASK-193, unrelated DAX flake); upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
