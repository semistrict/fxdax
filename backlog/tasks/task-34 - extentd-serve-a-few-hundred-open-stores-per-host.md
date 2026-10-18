---
id: TASK-34
title: 'extentd: serve a few hundred open stores per host'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 15:26'
updated_date: '2026-10-18 21:54'
labels:
  - performance
  - extentd
dependencies: []
priority: medium
ordinal: 103000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Forks of one base image are expected to run a few hundred at a time on one host, each a VM whose extent store extentd serves. Today each connection takes 8 worker threads (server.rs WORKERS) and a flush thread, each store may keep 64 files open (Options::open_files), and each holds its own copy of its branch's extent index in memory. None of it was tried with more than a handful of VMs. Scope: the disk image (extentd, extent_store).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 extentd serves 300 open stores, forks of one base, on one host, with threads, open files and memory per store measured and bounded, shown by a test or bench that opens them
- [x] #2 Threads do not grow linearly with connections: requests are served from a shared pool sized by the host
- [x] #3 A store's open files and its memory but for what it changed are bounded per store, and documented
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. remote.rs: one runtime for every store's uploader and fetches, sized by the host (no runtime and thread per store); the uploader a task on it, its disk work in block_in_place.
2. extentd pool.rs: a host-wide pool of request threads, grown on demand up to a bound sized by the host, idle ones going; each connection runs at most WORKERS requests at once in it, the rest queued.
3. wire.rs: an async receiver (recvmsg on a non-blocking socket, through tokio's AsyncFd), and a sender that waits for room on a full socket; one small runtime receives for every connection; serve_as starts connections with no thread each, serve_connection waits for one (tests).
4. Behind a store: flushes and followers' commit notices as tasks on that runtime (flushes through spawn_blocking; followers wake on the writer's commit event).
5. extent_store: open files per store bounded lower for extentd, documented with memory per store.
6. Test: 300 stores, forks of one base, opened through the daemon in one process: threads and open files counted (/proc/self), bounded.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified on GCE: serves_hundreds_of_forks_of_a_base (300 forks of a base, each reading it, open at once: the process's threads but the clients' 22 -> 32; 8 files a fork, the base's segment files shared, 64 at most; 91 KiB resident a fork); 2099 workspace tests, the Shuttle suites, 18 guest cases; macOS 2049. The test caught a thread per fork left after the pool: every fork of a base without a boot profile recorded one, with a thread waiting out its window; now one fork per base records. The daemon's limit of open files is raised as before (now fxdax::extentd::raise_open_files_limit, which the test calls too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd no longer spends threads per connection: requests run on a shared pool sized by the host (a connection's 8 at most), one small runtime receives every connection and works behind the stores, and every store's uploader and fetches share one runtime. Files per store are bounded (own files by Options::open_files; inherited segments' files shared per lineage) and memory per fork is what it changed (TASK-31); documented in extentd's module docs. Shown by a 300-fork test with threads, files and memory measured and bounded.
<!-- SECTION:FINAL_SUMMARY:END -->
