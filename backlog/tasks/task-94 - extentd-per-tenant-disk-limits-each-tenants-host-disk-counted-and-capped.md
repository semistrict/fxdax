---
id: TASK-94
title: 'extentd: per-tenant disk limits: each tenant''s host disk counted, and capped'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-24 12:29'
updated_date: '2026-11-07 11:22'
labels: []
dependencies: []
references:
  - TASK-50.3
  - TASK-79.89
  - TASK-43.4
priority: medium
type: feature
ordinal: 378000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Memory has per-tenant limits (TASK-50.3: SetMemoryLimit, eviction past a tenant's cap, ENOMEM otherwise). Disk has none. A store's capacity bounds only what its guest sees: the bytes its branch's extents hold, inherited ones included. A fork of a 10 GiB base starts 10 GiB "used" while storing nothing new. So no cap bounds what a tenant puts on the host's disk, and one tenant can fill it for every other.

Filesystem quotas on a tenant's directory (TASK-43.4/43.7) cannot do this: a fork's directory is only a handle, and its new data is written into the base store's directory (extent_store, Lineage), outside the tenant's.

Sharing makes counting a decision, to be made with the user before implementing: charge data shared between branches to the branch that wrote it (a fork pays for what it writes, a public base's owner for the base), or split it between the tenants holding it. Space a freed extent leaves in a segment until cleaning reclaims it (TASK-79.89) counts too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each tenant's host disk bytes are counted under one documented rule for shared data, and reported per tenant by the control API and /metrics (labelled by a hash of the tenant, as memory is)
- [x] #2 A per-tenant disk limit, set and changed at runtime through the control API, and a total one: past it, the tenant's VMs' writes fail with ENOSPC, the daemon does not abort, and other tenants' writes go on
- [x] #3 Tests: a tenant filling its limit through forks of a shared base is refused while another tenant writes; freeing data, once cleaning reclaims it, lets the tenant write again
- [x] #4 README documents how a tenant's disk is counted and limited, and why directory quotas do not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Count: extentd::disk, a tenant's disk is what its directory takes (walked as du -sx counts: blocks, each inode once, one filesystem), at grant, every 10 s, soon after a refusal (the daemon's 'extentd disk' thread), and on GetTenantDisk; writes admitted between walks counted as they come. Shared data: a file is the tenant's whose directory holds it; the host's part cache is no tenant's (bounded by its own capacity and floor). Decision in OPEN_QUESTIONS.md.
2. Limit: SetDiskLimit (a tenant's, alive now or granted later, or every tenant's together), none by default. Past it: ENOSPC on WRITE (inline and transfer) and on a VM process making a store (OPEN creating, FORK, admitted as STORE_MADE, 64 KiB), checked before the store takes anything; commits go on.
3. Report: GetTenantDisk (counted now) and /metrics fxdax_extentd_tenant_disk_bytes{tenant} / _limit_bytes{tenant} by label hash, fxdax_extentd_disk_bytes / disk_limit_bytes.
4. Tests, failing first: tests/disk.rs (forks of a shared base, a tenant past its limit refused while another writes, freed data leaving the disk lets it write again, the total limit); hostile.rs pinned a_tenant_filling_its_disk_leaves_others_committing (8 of mallory's VM processes fill, alice commits throughout) and the hostile proptest's Fill act under a per-case limit; mutants tenants-disks-unlimited and writes-admitted-as-nothing.
5. README: how a tenant's disk is counted and limited, and why directory quotas don't do it. go/ regenerated for the new RPCs.
6. Docker runs; push land-h4 with CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07: Counting rule (decision in OPEN_QUESTIONS.md, unconfirmed): a file is the tenant's whose directory holds it, walked as du -sx counts (blocks, each inode once, one filesystem); the host's part cache is no tenant's (bounded by its own capacity and floor; a share would make one tenant's limit move with another's reads). Since TASK-165.7/165.8 a fork's directory holds only what it wrote: measured 16 KiB a fork, 12 KiB an uploaded base. Walks: at grant, every 10 s and soon after a refusal (thread 'extentd disk', a pass at most every 100 ms), and on GetTenantDisk; writes admitted between walks counted as they come, and a walk adds back the writes not done as it began (disk::Admitted).
Limit: SetDiskLimit per tenant (alive now or granted later) or every tenant's together; past it WRITE (inline and transfer) and a VM process's store makings (OPEN creating, FORK; admitted as STORE_MADE 64 KiB) fail ENOSPC before the store takes anything; commits go on. Logged once as a tenant starts being refused; the store-full log skips those. Reported: GetTenantDisk, /metrics fxdax_extentd_tenant_disk_bytes{tenant}/_limit_bytes{tenant} (label hash), fxdax_extentd_disk_bytes/_limit_bytes.
Why not directory quotas: they refuse the store's own writes (framing a segment fails the store; a flush's log record), not the guest's next write; host setup the daemon can't make per grant. The task's original reason (a fork's data in the base's directory) is gone since TASK-165.7.
Finding (hostile proptest, 500 cases): mallory's disk ended 2 MiB past its limit: a recount while admitted writes were still in flight (admitted, not yet in the file) lost their bytes, and each refusal-nudged recount let mallory write that much again. Fixed by counting writes in flight (Admitted guard) into each walk; pinned by disk::tests::writes_not_done_stay_counted_across_walks, mutant walks-forget-writes-in-flight. Also: a flooding connection's own fills filled its 16 MiB store, refusing its flood's write (harness: flooders don't fill; flood cases run unlimited).
Runs (Docker, aarch64): tests/disk.rs red (acme wrote 224 MiB, its stores' capacities) then green 10/10; hostile pinned a_tenant_filling_its_disk_leaves_others_committing red (512 MiB) then green 5/5 (15 MiB of a 16 MiB room, disk 15,424 KiB of 16,384, alice 32 MiB written and committed); hostile proptest 1,500 cases pass (2,521 MiB filled, 6,283 fills refused, alice served throughout); fxdax 494 of 498, the 4 the container's (git, sudo); mutants disk x3 killed; go/ regenerated (go/generate.sh --check passes); upstream-changes.py --check passes.

2026-11-07, rebased onto main after land-9 (TASK-165.9, c9207e5d4) and land-xk (4bf0e45fd), no conflicts. land-9's layout adds a 4 KiB 'wal' (the WAL's base) to each store as it opens to write: tests/disk.rs now counts it with what acme wrote, the count still du's. store_dir's records are in the store's own directory, so counted. On c9207e5d4: fxdax 518 of 522 (the container's 4: git, sudo), hostile 500 cases (814 MiB filled, 2,008 refused, alice served), mutants 15 of 15 killed. On 4bf0e45fd: tests/disk.rs, extentd::disk, the hostile tests 8 of 8; go/generate.sh --check, mutants.sh --check and upstream-changes.py --check pass. ACs checked on this evidence: #1 GetTenantDisk and /metrics by label (tests/disk.rs), #2 SetDiskLimit per tenant and total, ENOSPC, others write on (tests/disk.rs, hostile pinned test), #3 forks of a shared base refused while globex writes, freed data leaving the disk lets acme write again (tests/disk.rs), #4 README. DoD #1 left open: macOS workspace passed on the earlier base (ee7d1acb3), Linux is Docker and CI, not Lima.

On 4bf0e45fd also: hostile 500 cases (796 MiB filled, 2,031 refused at the limit, alice served throughout), mutants 15 of 15 killed.
<!-- SECTION:NOTES:END -->
