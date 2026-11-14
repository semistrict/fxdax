---
id: TASK-406.2
title: A host fetches parts from the peer that owns them before the bucket
status: Done
assignee:
  - '@claude'
created_date: '2026-11-14 12:17'
updated_date: '2026-11-14 14:11'
labels:
  - sandbox
  - cache
  - bucket
dependencies: []
parent_task_id: TASK-406
priority: high
ordinal: 794000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On JuiceFS the sandbox team's hosts serve each other's cache misses, so a block comes from object storage about once per cluster, not once per host: what matters when a popular snapshot boots on many cold hosts at once. fxdax's fetcher (`extent_remote::fetch::Fetcher`) is inside the daemon, so they cannot add it from outside. Their design (~500 lines in their JuiceFS fork): each part has an owner among the peers, by rendezvous hashing of (key, part); a host asks the owner before a bucket GET; the owner serves from its part cache or loads the part once; the requester checks the part against the CRCs of the object's tail; on any error it falls back to the bucket and marks the peer down for a while. Trusted node network, no auth: every host can read the whole bucket already.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 With peers configured (this host's address and the list, by environment, and replaced at runtime by a control RPC), a part another host owns by rendezvous hashing of (key, part) over the list is asked of that host before the bucket; a part this host owns, or any part with no peers, goes to the bucket as before
- [x] #2 The owner serves a part from its memory or part cache, or fetches it once from the bucket however many hosts ask at once, and keeps it; it serves only framed data objects (segments, extents, checkpoints, moves), never records, leases, cells or references, and only the version of the object the asker's tail names
- [x] #3 The asker checks a served part against the CRC its tail gives; a part that fails, a peer error or a peer past its deadline falls back to the bucket, and an unreachable or failing peer is skipped for a while
- [x] #4 Only fetchers of the host's own bucket (`FXDAX_EXTENTD_REMOTE`) ask peers; a served request never asks another peer
- [x] #5 Tests: two hosts sharing a bucket, a cold read on one served by the other with no bucket GET of the part; a down peer, a lying peer and a peer that owns nothing fall back to the bucket; ownership moves with the list
- [x] #6 Documented in the daemon docs and doc-7, with the bucket GETs it saves measured on two daemons
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote::peer: traits Peers (owner of (key, part), if another host and up) and Peer (stored bytes of a part, as a Stored says, or none); served(key): segments, extents, checkpoints, moves.
2. fetch: Fetcher::asking(peers); a flight whose part another host owns asks it under the GET grant, checks the bytes against its tail's CRC and length, falls back to the bucket; Flown keyed by whether it asks, so the owner's served and local reads share one flight. Fetcher::stored_part(key, part, stored) serves a part: by the host's own tail, if it says the part is stored as asked, through memory, cache, or one GET, never asking peers. Tails stay bucket-only: a stale peer tail would make the second read of a changed object fail Corrupt.
3. fxdax::remote::peers: proto fxdax.peer.v1 (GetPart, bucket URL checked); HostPeers: me, list, rendezvous by SipHash-1-3 of (address, key, part), per-peer lazy tonic channel with connect and request deadlines, down for 30 s on any failure but NOT_FOUND; counters. Server on the remotes' runtime. Registered process-wide with the host bucket: remote::Fetcher::new asks peers only for that bucket.
4. extentd: FXDAX_EXTENTD_PEER_ADDRESS (listen, this host's name in the list), FXDAX_EXTENTD_PEERS (initial list), needs FXDAX_EXTENTD_CACHE; control RPC SetPeers; counters in the stats file.
5. Tests: fetch.rs with a fake Peers (served part, lying peer, own part); peers.rs in-process two hosts over TCP sharing one bucket (bucket GETs fall by the part count), down peer, lying peer, NOT_FOUND, ownership moving with the list, records refused.
6. Two daemons on ubu1: GETs measured; docs (daemon header, doc-7).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified: extent_remote fetch tests parts_another_host_owns_come_from_it (owner fetches each part once, asks no peer, a third host GETs only its tail), parts_their_owner_gives_wrong_come_from_the_bucket, peers_are_given_only_the_part_they_ask_for; fxdax remote::peers tests (3 hosts: 16 GETs fewer for 8 parts; failing peer asked once then down for DOWN; NOT_FOUND keeps it up; ownership moves only to a host added; set keeps down state; records/leases never asked); extentd::peers tests over gRPC (served as stored, then from memory; NOT_FOUND; INVALID_ARGUMENT/FAILED_PRECONDITION; unreachable fails within the connect time; HOST:PORT); tests/peers.rs on ubu1: two daemons, the second host's ranged GETs 21 alone, 11 with peers, 10 parts given, the first host making 7 either way; SetPeers through the control API (EINVAL, ENOTSUP). Measured in docs/measurements/peer-parts-2026-11-14.md. Not tested directly: a peer that answers past its 5 s deadline (tonic's per-request timeout, the same fallback path as any error), and a store of another bucket not asking peers (by Arc::ptr_eq with the registered host bucket in remote::Fetcher::new). Tails are never asked of peers (a stale peer tail would fail a recreated object's read twice, as corrupt). Suites as TASK-406.1's.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A host's fetchers ask the peer that owns a part (rendezvous hashing, SipHash-1-3, of HOST:PORT with key and part) before the bucket, under the same GET budget; the owner serves it from memory or its part cache, or one GET however many ask, never asking another peer, only for framed data objects and only the version the asker's tail names; the asker checks CRC and length and on any failure fetches from the bucket and puts the peer down for 30 s. gRPC over TCP (proto/fxdax/peer/v1/peer.proto), configured by FXDAX_EXTENTD_PEER_ADDRESS, FXDAX_EXTENTD_PEERS and the SetPeers control RPC; needs a part cache; counted in the stats file and /metrics. On two daemons, a point read on the second host took 11 ranged GETs instead of 21, the first serving 10 parts from its cache with no extra GET. Verified by unit, gRPC and two-daemon tests on ubu1 and macOS; full suites pass but for pre-existing TASK-315.
<!-- SECTION:FINAL_SUMMARY:END -->
