---
id: TASK-111.4
title: >-
  fxdaxctl stats --json: one consistent snapshot of extentd, VMM and guest
  counters, each with its build commit
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 01:25'
updated_date: '2026-11-06 04:38'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 418000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team diffs Prometheus scrapes, /proc/fs/fxfs/root and per-thread /proc CPU separately to get per-phase counters (TASK-111); the reads are not taken at one moment and do not say which build produced them. Their round-7 names partly predate copy-on-write DAX: commit_seal_s (commit_seal_ns) went with writable DAX (extentd.proto fields 16, 17, 19 reserved); stage_s is extentd's stage_ns. Before this task the guest's counters never reached the host: they lived in the guest's /proc/fs/fxfs/TAG alone.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One command prints extentd, VMM and guest counters as JSON from one read
- [x] #2 Each source carries its build commit
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Wire: the daemon asks each VM process that opened a store (Told::Stats, STATS_ASK 109, args[0] an ask number); the VM process says its stage counts (STAGE_COUNTS) and answers (Request::Stats, STATS 31, args[0] the ask, its guest's text as payload with STATS_GUEST, MAX_STATS 64 KiB at most). An answer answers earlier asks.
2. VMM: RemoteExtentStore::on_stats_asked / answer_stats; without a callback, or a guest that takes asks, it answers at once with none. vendor/firecracker's device offers VIRTIO_EXTENT_F_STATS (bit 11) to writers and followers; an ask sets the configuration's stats_asked and signals a configuration change while the activating driver acked the feature; T_STATS (17) forwards the guest's answer.
3. Guest: drivers/virtio_extent answers a changed stats_asked from a work item with what virtio_extent_set_stats's provider writes; fs/fxfs provides /proc/fs/fxfs/TAG's text and its build line, fxfs_vfs_stats_show through a seq_file over the buffer.
4. Daemon: extentd/snapshot.rs, a Report per connection that the connection holds (store, branch, path, follower, build, stage counts, host mappings, window gauges, pending asks), a registry of Weak<Report>; take(wait) asks, waits (2 s default, 60 s most), then reads the daemon's counters and gauges (metrics::gauges, shared with /metrics). Control API GetSnapshot.
5. fxdaxctl stats [--json] through the daemon.
6. Tests: codec, guest text parsing, extentd snapshot tests, fxdaxctl end to end, the device's, guest case fxfs-guest-stats.
7. doc-4: F_STATS, stats_asked, STATS, a Stats section.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in b772af8da (fxdax, guest, doc-4, measurement) and vendor/firecracker dcecf56a7 (branch fxdax-t1114, not pushed). Verified: extentd::snapshot::tests (guest text: counters, histograms, build, bad lines left out), extentd::tests::snapshots_hold_the_daemons_vm_processes_and_guests_stats (an answering, a guestless and a silent VM process; one answer answering two asks), codec tests for STATS/STATS_ASK/StageCounts, fxdaxctl stats_are_one_snapshot_of_the_daemon_and_its_vms (the binary through a daemon, JSON and lines; refused without the daemon), the device's stats_are_asked_of_guests_that_take_asks and stats_go_to_the_daemon, guest case fxfs-guest-stats on GCE r681 (fxdaxctl stats --json against a live VM: the three builds one commit, its answer in 0.96 ms, its guest's 70 counters and 10 histograms, each counter between the guest's own reads before and after; docs/measurements/fxdaxctl-stats-2026-11-05). Runs: macOS workspace 2115/2115 + shuttle parts; Linux Docker 2295/2303, the 8 failures the known Docker-only ones (eu-stack stacks, grants as root) and two that fail only under that run's memory load (extent_store durable_commits_keep_the_open_segments_pages_cached_on_the_host, extentd childs_opened_during_their_forks_wait_for_them; each 30/30 alone); GCE r680 Linux workspace 2303/2303 and every other part; GCE r681 fxfs-guest-stats, fxfs-guest, fxfs-guest-resize, fxfs-guest-follow, vmm extent:: 16/16. r680's guest half failed to build: the driver used vext_stats_work before declaring it, and the case returned a borrow; both fixed before r681. Found compiling the driver against an arm64 tree afterwards: request_names was sized by its last initializer, so the sysfs loop to T_LAST read past it with T_STATS added; now sized VIRTIO_EXTENT_T_LAST + 1, so a type added unnamed is skipped, not overrun. Not exercised in a guest: a follower's answer (the device offers F_STATS to followers; no case runs one with stats asked). Overlap: TASK-160.7 (driver's per-type switches: STATS adds a row to vext_args and vext_data_len); vendor/firecracker's extent device; extentd server.rs/client.rs/codec.rs; nothing in extent_store/extent_remote. Decisions in OPEN_QUESTIONS.md.

Rebased 2026-11-06 onto origin/main 5f18867c5 (TASK-187, TASK-190, TASK-141.1 and others): fxdax 82e0eb8fc; vendor/firecracker 96cb97976 on the fork's e64c4b283 (branch fxdax-t1114; its Cargo.lock keeps fxdax's url, which the fork's 8b1a45e6a lacked). OPEN_QUESTIONS.md's conflict, both sides' decisions kept. Reruns: the device's tests (vmm extent::) 16/16, Linux arm64 Docker, built --locked; macOS workspace 2119/2119 and the shuttle parts; Linux Docker 2303/2310, the 6 known Docker-only failures (eu-stack stacks, grants as root) and extentd vm_processes_that_leave_replies_unread_are_let_go, which failed once under the full run's load (the daemon hung up while the test still sent its READs: its send unwrapped a broken pipe) and passed 40/40 alone. Supersedes the SHAs above (b772af8da, dcecf56a7).

Rebased 2026-11-06 onto origin/main 6754d1469 (TASK-160.7's one table of request types): fxdax 7a0f88f44; vendor/firecracker unchanged, 96cb97976. STATS is a row of the driver's vext_types[] (vext_args_stats from req->offset, data out of data_len bytes, not writing the store), which replaces the four switches and the request_names array (whose resizing went with it); its sysfs loop runs to VIRTIO_EXTENT_T_LAST, 17, over a table of T_LAST + 1. Reruns: guest kernel built in Docker (arm64), check-guest.sh arm64 ok; the device's tests (vmm extent::) 16/16 --locked; Linux Docker fxdax 2304/2310, the 6 known Docker-only failures; GCE r682 (us-west2-a, Firecracker jailed) fxfs-guest-stats, fxfs-guest, fxfs-guest-dax passed, the snapshot's three builds 21a094a79, the VM's answer in 1.1 ms. Supersedes the SHAs above.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdaxctl stats [--json] prints one snapshot (the control API's GetSnapshot): the daemon's counters and gauges and build, and each VM process's, as it answered an ask that its device passes to the guest (doc-4's F_STATS), with the guest's /proc/fs/fxfs counters, histograms and build; the daemon reads its own once all answered. Verified by unit and extentd tests, an fxdaxctl end-to-end test, the device's tests, and fxfs-guest-stats against a live VM on GCE r681; macOS, Docker and GCE r680 workspace runs.
<!-- SECTION:FINAL_SUMMARY:END -->
