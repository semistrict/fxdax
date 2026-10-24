---
id: TASK-96
title: >-
  The store as a standalone layer: a Durable-Objects-like primitive over object
  storage and local SSD alone
status: To Do
assignee: []
created_date: '2026-10-24 15:24'
labels: []
dependencies: []
priority: high
ordinal: 382000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Below Fxfs, fxdax's storage is a primitive of its own: a fenced, forkable, transactional store of write-once extents with an atomic root and one writer per branch, kept in object storage. It is grown as a general infrastructure layer in the shape of Cloudflare's Durable Objects, but with storage more general than SQLite: anything built from extents and a root (a filesystem, a log, a KV store, a database's pages). The one constraint: object storage plus each machine's own SSD and memory, and no other infrastructure (no coordination service, no database, no membership service). Fxfs and the VM attachment stay clients of it, and the filesystem work goes on.

Settled with the user in a design interview on 2026-10-24. These are decisions, not proposals:

- **Public API:** the core store: extents, root, commit, fork, views, leases, and watching for commits. A filesystem, a log or a KV store are libraries over it (Fxfs is one). A commit never spans branches, as in Durable Objects.
- **Commit levels:** *ordered* (log order only, surviving nothing yet), *replicated* (on the writer's SSD and one peer's: survives a lost machine), *bucket* (in object storage). No "local only" level.
- **Peer log:** the writer and one peer, both named in the branch's lease. A lost or lagging peer: replicated commits wait while the writer picks a new peer, sends it the unuploaded tail, and records it in the lease with a conditional PUT; the guarantee never weakens silently. A taker recovers the tail from the peer the lease names, then fences.
- **Leases:** renewed in the bucket, stamped by object storage's own time (Last-Modified). A lease not renewed for T plus a clock-skew bound may be taken by anyone; the holder stops writing T after its last successful renewal by its own clock. The fencing tokens and fenced segments of doc-7 keep it safe whatever the clocks do.
- **Placement:** the daemon that first opens a free branch for writing claims its lease, with a policy hook to redirect. An open of a branch with a live writer elsewhere is redirected there; an explicit "move here" hands over gracefully (drain to the peer or the bucket, pass the lease), losing no commits and waiting for no expiry.
- **Discovery:** the lease names its writer's address. Clients GET it, connect, and cache it; a stale cache gets a redirect (or EROFS) and rereads.
- **Membership:** each daemon keeps an object under `members/` (address, capacity), renewed like a lease; picking a peer lists it. The lease, not the listing, is authoritative.
- **Naming:** a client-chosen name maps deterministically (by hashing within a namespace) to a branch ID, as Durable Objects' idFromName does, with opaque new IDs for unnamed objects. Forks get random IDs, made unique by a conditional create.
- **Extent IDs:** the writer mints them (no round trip); the host refuses any outside the writer's branch, or already written and sealed (the gap TASK-79.6 shows).
- **Readers (views, followers on other machines):** read uploaded segments from the bucket into their own daemon's SSD cache, and ask the writer's daemon only for the tail newer than the bucket.
- **Idle branches:** after an idle time, the writer uploads everything, releases the lease and its peer, and keeps its SSD copy as a cache, evicted under pressure. The next open anywhere claims the lease again, warm if it lands here.
- **History:** any commit can be pinned as a named snapshot, kept until deleted; unpinned commits stay readable for a retention window, for point-in-time restore (a fork from that epoch). Garbage collection respects both.
- **Garbage collection:** a separate `fxdax-gc` process per bucket prefix, as today.
- **Process shape:** one daemon per machine (extentd, generalized) owns the SSD, the caches, the uploads, the peer log and the leases.
- **Protocol:** one framed binary protocol: a core that every client gets, and fxdax's VM attachment (MAP, window, faults, stage ops) as a negotiated extension on the same connection. It runs over the local Unix socket, and over TCP for remote clients and between daemons (the peer log, readers' tails, handoff).
- **Tenancy and trust:** a daemon serves isolated tenants (grants, memory and disk caps, as in TASK-43 and TASK-94). Encryption and authentication are the infrastructure's (a mesh, WireGuard), which hands the daemon each connection's verified identity; the daemon maps identity to tenant and enforces grants.
- **Bucket layout:** a fresh layout, no compatibility (there are no existing buckets to keep): leases gain the writer's address, its peer and the renewal time; `members/` and the name mapping are new.
- **Home:** its own crates in this repository, with its own specification document (a successor to doc-7, as doc-4 is the device's).

Left as parameters, not decisions: the lease time T and the skew bound, the upload-lag bounds, the retention window, and the idle time.

Prior art in the repo: doc-7 (object storage layout, leases, fencing, forks, collection), doc-8 (followers), doc-4 (the device protocol), crates/extent_store, crates/extent_remote, crates/fxdax/src/extentd (wire.rs, the gRPC control API in crates/fxdax/proto).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A specification document (backlog doc) defines the core store API, the three commit levels, the lease, peer-log, placement, discovery, membership, naming, retention and watch protocols, the bucket layout, and each operation's object-storage request cost
- [ ] #2 The store core, the daemon and the protocol are crates of their own, and the VM attachment is a negotiated protocol extension that fxdax's device uses unchanged
- [ ] #3 A commit asking for 'replicated' completes only once on the writer's SSD and its peer's; a test that kills the writer's machine after such a commit and before its upload shows a taker recovers it from the peer
- [ ] #4 Losing the peer stalls replicated commits until a new peer holds the tail and the lease names it; a test shows no replicated commit is acknowledged on one copy meanwhile
- [ ] #5 Leases expire by object storage's time: a test with a silent holder shows another daemon takes the branch after T plus the skew bound, and a holder cut off stops writing by its own clock, with fencing voiding anything it lands late
- [ ] #6 Opening a free branch claims it on the opening daemon; opening a held one redirects to the writer the lease names; an explicit move hands over with no lost commit, each shown by a test
- [ ] #7 Daemons register under members/, and a writer picks its peer from that listing; names map deterministically to branch IDs within a namespace; the host refuses extent IDs outside the writer's branch or already sealed: each shown by a test
- [ ] #8 Readers on other machines read uploaded data from the bucket into their own cache and only the newer tail from the writer; clients can watch a branch's commits from an epoch at a chosen level: each shown by a test
- [ ] #9 An idle branch is uploaded and its lease and peer released, its SSD copy kept as a cache; named snapshots and a retention window are kept by garbage collection and restorable by fork: each shown by a test
- [ ] #10 The protocol runs over TCP between daemons and for remote clients, and the daemon maps the connection identity the infrastructure gives to a tenant and enforces its grants, shown by a test
- [ ] #11 fxdax's guest suites, crash and pgbench tests pass on GCE over the new layer
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
