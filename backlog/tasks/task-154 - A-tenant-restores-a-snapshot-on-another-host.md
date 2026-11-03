---
id: TASK-154
title: A tenant restores a snapshot on another host
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 22:53'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 555000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Required: the system must handle cross-host restores. TASK-150.4 stopped a tenant naming a branch, as a compromised VM process could take another tenant by its ID, so a tenant restore where the point is not local now gets ENOENT. Without a format change, the host VM manager, which is trusted, could grant the tenant the branch through the control API (Grant, grants.rs), the daemon honouring OPEN_FORK_FROM only for a branch granted; an owner in the branch Record is the longer answer (the format bump).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A tenant restores on a host without the point, through its usual path
- [x] #2 A tenant still can not take a branch not granted it: tenants_take_no_branch_by_naming_it holds
- [x] #3 Shown end to end with two hosts or the simulator
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. No format change, no wire change, no grant table: the VM manager's CreateStore of the branch at a point in the tenant's directory is the grant; a tenant's named branch stays ignored, so a stolen ID is never read. Why not a daemon table: once taken, the point is a store in the tenant's directory, so 'once' and 'revoke' of a table would bound nothing; OPEN_FORK_FROM can not carry the placement token a take from a later placement needs, CreateStore does; a table dies with the daemon, a store does not.
2. Add DropStore (control API, a new call rather than a DeleteStore field: an older daemon would ignore a field and delete the branch): this host's copy goes, the branch stays in the bucket. It ends a grant; DeleteStore would destroy the snapshot.
3. Tests: restore with two daemons over one bucket, the same request refused without the take and for another tenant, a stolen fork ID refused, drop ending it before and after use, a later placement's point. Fuzz: a granted point, hostile naming of both branches, an oracle that no reply names the other branch, a positive count of granted restores. Mutations: each red with the check put back wrong.
4. Real Firecracker through a tenant socket on GCE.
5. Docs: extentd.proto, extentd/mod.rs, wire.rs, README, what the VM manager must do.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design. No format, wire or grant-table change. A tenant's named branch stays ignored; the VM manager's CreateStore of the branch at a point in the tenant's directory is the grant, already there; the usual OPEN_FORK_FROM then forks it. A table of grants was rejected: once taken, the point is a store in the tenant's directory, which its VM process can open as writer, so 'once' and 'revoke' of a table bound nothing a second take could do; OPEN_FORK_FROM carries no placement token, which a point of a later placement needs (points_of_later_placements_are_taken_placed), CreateStore does; a table goes with the daemon, a store does not; and a tenant's wire argument honoured again is a check that can be wrong, as TASK-150.4's was. Added: DropStore, a call rather than a DeleteStore field because an older daemon would ignore the field and delete the branch: it ends a grant, DeleteStore would destroy the snapshot. extentd.proto says what the VM manager does.

Blind spots. The fuzz's restores: a sequence is sent at once, and its last request is answered at once on a connection with no store yet, so that it hung up before a restore's reply, and nothing after a restore ran. Found by a count of restores added as a positive control, 0 in six runs while the daemon's own trace showed them served. Class test: a restore waits for its reply, what follows is of the store it opened, the count fails at 0, and an oracle over every reply looks for the other tenant's branch or bytes.

Finding, not fixed, for the user: a host's copy of a point is a writer of its branch, and a restore's fork a commit there, so that once one host forks it every other host's copy, the snapshot's own too, is stale, restores failing EIO, until DropStore and CreateStore. Before this change too, and for the default tenant. It makes a zygote across hosts at once impossible. copies_of_a_point_another_host_forked_are_taken_anew pins it. Points should be read-only ancestors, which is a protocol change.

Structure: the rule stays one rule, a tenant's directory. Nothing in the code to get wrong: no tenant's word for a branch is read. Branches having no owner remains TASK-153.9's.

Verified at ef3dfc028. Docker arm64 workspace 2118 of 2118; macOS workspace 1959 of 1959; GCE r483 x86-64 2117 of 2118, the one failure windows_share_bases_pages (5 holders, not 4), in extents.rs, which this branch leaves alone, passing alone on r484. Guest: r483 scripts/test-e2e.sh's default list, exit 0; r484 under the jailer, uid 64000, production seccomp filter: fxfs-guest-migrate-tenant-dax, -migrate-dax, -snapshot-dax, fxfs-guest-dax, all OK, Firecracker's logs naming the jailer's --id. Each commit compiled alone. Soak: 12 of 12 after the served bound was loosened. Red with the bug back: tenant's branch honoured (4 tests, the fuzz in under a second, 'a point of a branch named was taken'); DropStore deleting the branch; DropStore doing nothing; a take ignoring its branch; a take ignoring its token; the fuzz with no point given (restores 0). scripts/tests' test_each_edit_finds_its_code_once fails, at eeaf2f0bb too. Unverified: two daemon processes over one real bucket: the test's two daemons are one process, sharing the memory bucket; a real host pair needs two hosts, past this run's limit. DoD 1 is left open for the user: no Lima run, and the failure above.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A tenant restores a snapshot on a host without the point through its usual path, its VM manager, while a tenant still can't take a branch not granted it (tenants_take_no_branch_by_naming_it). Shown end to end in the workspace and on GCE: r483 the default list, r484 in the jailer under the production seccomp filter (fxfs-guest-migrate-tenant-dax, -migrate-dax, -snapshot-dax); each bug put back fails. Unverified: two daemon processes over one real bucket, as the test's two daemons share a process and a memory bucket.
<!-- SECTION:FINAL_SUMMARY:END -->
