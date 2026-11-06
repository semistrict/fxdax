---
id: TASK-205
title: >-
  Uploads livelock below ~18 MB/s to object storage: object_store's 30 s
  whole-request timeout against up to 512 MiB of single PUTs at once, each
  retried from its start
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 06:58'
updated_date: '2026-11-06 08:04'
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
- [x] #1 Uploads make progress at any bandwidth above what one part needs: the timeout scales with the body, or the body goes in parts, or what is in flight at once shrinks after timeouts
- [x] #2 A test runs uploads over an object store slower than what is in flight per timeout (a bandwidth-limited proxy or server) and sees every commit acknowledged
- [x] #3 The bucket client's timeouts are fxdax's own, set where the client is built, not object_store's defaults
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Build buckets' object_store clients in extent_remote (store.rs object_store_with) per scheme rather than by parse_url_opts, with fxdax's own ClientOptions (connect 5 s, no whole-request timeout) and an HttpConnector of fxdax's (client.rs).
2. The connector: reqwest built from the ClientOptions object_store parsed, each request timed out on progress: no bytes of its body taken by the connection, or of its response read, for the stall time (30 s). Request bodies go in 16 KiB slices so taking them is progress.
3. Tests: an in-memory S3 server over HTTP whose reads go through a link of limited bandwidth (src/s3_server.rs, test-only), conformance and branch lifecycle over it; a writer's 8 segments with 16 MiB extents over a 16 MiB/s link with a 4 s stall all land once and are acknowledged, red first with a 4 s whole-request timeout; a server that never reads: a PUT fails Unavailable after the stall time rather than hang; unit tests of slicing and the stall clocks.
4. Measure the connector against object_store's own at loopback bandwidth (ignored client_throughput); GCS from a Mac (ignored gcs).
5. Decision line in OPEN_QUESTIONS.md; note on TASK-205 about the uploader's in-flight budget, for after TASK-165.9.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix (2026-11-06): buckets' requests time out by their progress, not as a whole. extent_remote/src/client.rs is fxdax's HttpConnector for object_store's S3, GCS and Azure stores: reqwest, built from the ClientOptions object_store parsed as object_store 0.14.2 builds its own (every ClientConfigKey mirrored; its ClientOptions::client is crate-private), whose requests fail as Timeout once no bytes moved for Client::stall (30 s): a request's body goes to its connection in 16 KiB slices, each one taken progress, and its response fails if none of it came for 30 s while it was read. store.rs builds every cloud store per scheme (object_store_with, as parse_url_opts but with_client_options(connect 5 s, no whole-request timeout) and with_http_connector), the only place in the repo a client is built (rg for parse_url, the builders and ClientOptions). Objects still go in one PUT, If-None-Match: * or If-Match, so conditional-write semantics are unchanged.

Alternatives, by reading object_store 0.14.2 and reqwest 0.13.5: multipart has no PutMode (PutMultipartOptions: tags, attributes, extensions; S3's If-None-Match on CompleteMultipartUpload is internal, GCS's XML multipart none), so segments would lose create-if-absent; reqwest's read_timeout runs from the request's start until its response's headers, not reset by the body's writes, so it is a whole-request deadline for uploads; a timeout scaled by the body's size assumes a bandwidth per request, which 64 requests sharing a link divide; wrapping object_store's own client sees no upload progress, HttpRequestBody being closed (Bytes or PutPayload only).

How found: the storage review of 2026-11-06, by reading. Blind spot: no test reached object storage over HTTP but the ignored RustFS and Azurite runs, and none ran any bucket slower than its bandwidth: every bucket in the suite was in process (InMemory, the simulation, file), with no client, timeouts or bandwidth. Test for the class: src/s3_server.rs, an S3 server in memory over HTTP (hyper), its upstream a link of N bytes/s that all connections share; store::tests::s3_buckets_conform runs bucket::conformance and branch_lifecycle through fxdax's client over it, so the client's headers, conditions, retries and timeouts are in the default suite. store::tests::uploads_land_over_a_slow_link: a writer's 8 segments with a 2 MiB extent each over a 4 MiB/s link, stall 2 s (twice what the link carries in a stall time in flight): every upload lands at its first attempt (creates counted: 16) and the last commit is acknowledged, in 4.2 s. Red with the bug back (object_store's own client, whole-request timeout 2 s): no upload landed in 30 s ('uploads landed: Elapsed'). store::tests::stalled_uploads_time_out: a server that reads nothing: a 32 MiB PUT fails Unavailable after 1.3 s (stall 1 s); red with the stall watch removed (hung to its 10 s bound). client::tests: slicing, sizes and progress; the stall clock; a response body that never comes fails Timeout after the stall time (paused time).

Around it: the collector's whole-object GETs and fetch's ranged GETs had the same 30 s; now the response stall (and object_store's retries of idempotent requests). FXDAX_REMOTE_LATENCY_MS adds latency per call, no bandwidth: the S3 server's link is its counterpart. GCS through the new client, from a Mac with application default credentials (ignored store::tests::gcs, FXDAX_GCS_URL=gs://echophase-fxdax-bench/t205-mac, deleted after): probe, conformance, branch lifecycle, then 32 x 8 MiB PUTs at once, 4.7 s, 57 MB/s, each at its first attempt. It found a test bug: conformance's 'no such version' was generation 0, which on GCS is 'no object', so the update created one; now generation 1 (bucket.rs). conformance had never run on GCS.

Structure: the class is a deadline that ignores progress, which requests sharing a link all reach together. Every cloud store is now built in one place with fxdax's own client, so none gets object_store's defaults; the deadline is on progress, which a shared link slows but does not stop. Nothing architectural.

Cost (ignored store::tests::client_throughput, release, 32 x 16 MiB PUTs at once to the S3 server in memory over loopback, 25 rounds each in turn, medians): fxdax's 7933, 7129, 7667 MB/s against object_store's own 7993, 7400, 7614 MB/s in three runs, -0.8%, -3.7%, +0.7%: noise.

For after TASK-165.9 (the uploader's, not touched here): with progress timeouts a host's 512 MiB and 64 requests in flight no longer livelock, but a commit's latency is what is in flight ahead of it over the bandwidth: at 2 MB/s, 512 MiB is over 4 minutes, and segments are acknowledged in order. The uploader could bound bytes in flight by the throughput it measures, a few seconds' worth, rather than UPLOAD_BYTES, so that a slow link queues commits locally (the WAL) rather than in flight.

Correction, after the Linux run (Docker, aarch64): the slow-link test as first written (8 x 2 MiB extents, 4 MiB/s, stall 2 s) passed on macOS and failed on Linux every time, no upload landing in 30 s; with an 8 s stall it passed in 4.5 s, 16 creates. Cause: progress is the body's slices taken by the connection, and what the socket buffers counts as taken. Over loopback (64 KiB segments) a socket the server reads slowly grows its send buffer toward Linux's 4 MiB bound at once, so a request's 2 MiB went into the kernel whole and nothing moved for 4 s (2 MiB at its 512 KiB/s share). Over a link that limits the bandwidth the buffer grows with the congestion window, a couple of round trips and the link's queue, so the 30 s stall holds; a peer reading slowly is what grows it to the bound, which the client module's doc now says. reqwest 0.13.5 gives no way to the socket (its Conn is sealed: no SO_SNDBUF, TCP_NOTSENT_LOWAT or TCP_INFO), and tcp_user_timeout, Linux's, would not see a server that acks and never answers. The test now uploads 8 x 16 MiB at 16 MiB/s with a 4 s stall (each request's share carries 8 MiB, twice the bound, in a stall time; in flight twice what the link carries in one): green in 9.4-9.7 s on macOS and Linux, three runs each; red with object_store's whole-request timeout of 4 s on both ('uploads landed: Elapsed' at 31 s).

Validation (2026-11-06): macOS FXDAX_TEST_PARTS='workspace sim' scripts/test.sh: 2038/2038 and 47/47 (then extent_remote again on the final code: 180/180). Linux, Docker (OrbStack, aarch64, privileged, /target/t205): workspace 2226/2230, the 4 failures the container's: builds_are_stamped_with_their_commit (no git through the worktree's .git link) and three grants tests (a file the rust image lacks, ENOENT); unprivileged first, 76 more of fxdax's failed for /dev/userfaultfd and io_uring (EPERM). sim 47/47. scripts/upstream-changes.py --check passes. Not run: GCE (scripts/test-gce.sh), so DoD #1 stays open.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Buckets' HTTP requests now time out by their progress, not as a whole. extent_remote::client is fxdax's HttpConnector: reqwest built from object_store's parsed ClientOptions, a request failing as Timeout once none of its body was taken by the connection (16 KiB slices) or none of its response came for 30 s. store.rs builds every S3, GCS and Azure store with it, connect timeout 5 s and no whole-request timeout. Objects still go in one conditional PUT. Verified by a bandwidth-limited in-memory S3 server (src/s3_server.rs): 128 MiB of a writer's uploads over 16 MiB/s with a 4 s stall all land once and are acknowledged (red under object_store's whole-request timeout), a server that never reads fails a PUT after the stall, and conformance runs over HTTP. Real GCS from a Mac: conformance and lifecycle (after fixing conformance's generation-0 version), and 32 x 8 MiB concurrent PUTs landing first time. Cost at loopback bandwidth: within noise (-3.7% to +0.7%). macOS and Docker Linux suites pass, apart from container-only failures; GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
