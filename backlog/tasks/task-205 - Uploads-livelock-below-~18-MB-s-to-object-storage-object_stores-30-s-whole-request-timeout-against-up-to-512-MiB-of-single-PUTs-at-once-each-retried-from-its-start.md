---
id: TASK-205
title: >-
  Uploads livelock below ~18 MB/s to object storage: object_store's 30 s
  whole-request timeout against up to 512 MiB of single PUTs at once, each
  retried from its start
status: To Do
assignee: []
created_date: '2026-11-06 06:58'
labels:
  - storage
  - object-storage
dependencies: []
priority: medium
type: bug
ordinal: 647000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the storage review of 2026-11-06, by reading, not measured. fxdax builds every bucket client with object_store's defaults (extent_remote/src/store.rs object_store_at: parse_url_opts with the environment, no ClientOptions), whose request timeout is 30 s for the whole request, body included (object_store 0.14.2 client/mod.rs:394; reqwest's total timeout). Every object goes in one PUT (no multipart anywhere in extent_remote): a segment up to Options::max_data, segment_size plus a write, 32 + 64 MiB (extent_store store.rs:129, host_store.rs:156), an own extent up to 64 MiB. A host's uploads hold up to 512 MiB and 64 requests at once (remote.rs UPLOADS, UPLOAD_BYTES; a store IN_FLIGHT = 16 segments). They share the host's upstream fairly, so below about 512 MiB / 30 s = 18 MB/s every PUT times out at about the same moment having sent part of its body; object_store does not retry a conditional PUT on a timeout (not idempotent, client/retry.rs:275), extent_remote maps it Unavailable and retries it from its first byte after at most 5 s (bucket.rs create, store.rs backoff), and the next round times out the same way: nothing lands, commits stall at the lag bound for as long as the bandwidth stays low, not just slow. One 96 MiB segment alone needs 3.2 MB/s. Hosts this hits: instances with a low baseline (AWS t3.small, 128 Mbit/s once its burst is spent), a bucket far away (the sandbox team's RustFS, 25 ms), a link the guests saturate, every store catching up after an outage at once. The collector's whole-object GETs of objects it compacts are bounded by the same 30 s. Nothing in the tests runs object storage below its bandwidth.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Uploads make progress at any bandwidth above what one part needs: the timeout scales with the body, or the body goes in parts, or what is in flight at once shrinks after timeouts
- [ ] #2 A test runs uploads over an object store slower than what is in flight per timeout (a bandwidth-limited proxy or server) and sees every commit acknowledged
- [ ] #3 The bucket client's timeouts are fxdax's own, set where the client is built, not object_store's defaults
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
