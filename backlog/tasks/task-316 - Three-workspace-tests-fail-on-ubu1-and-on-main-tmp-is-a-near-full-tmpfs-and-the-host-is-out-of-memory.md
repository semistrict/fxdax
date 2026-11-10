---
id: TASK-316
title: >-
  Three workspace tests fail on ubu1, and on main: /tmp is a near-full tmpfs and
  the host is out of memory
status: To Do
assignee: []
created_date: '2026-11-10 09:05'
updated_date: '2026-11-10 09:08'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 723000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen 2026-11-10 running cargo nextest --workspace on ubu1 (TASK-249/314 branch, then origin/main f36453323 in its own worktree: same three failures). ubu1's /tmp is a 14 GB tmpfs with ~4.0 GiB free (other agents' runs), its 27 GB of RAM used and 7 GB of swap full.
- extent_store::store durable_commits_keep_the_open_segments_pages_cached_on_the_host: '0 of the open segment's 8 pages cached' (store.rs:1540): the store in a tempdir on tmpfs, whose pages no page-cache check finds as on a disk filesystem.
- fxdax extentd::tests::imports_read_what_they_made_from_the_hosts_cache: 29 ranged GETs, not 3 (tests.rs ~6050): the part cache under /tmp sits below its 4 GiB free-space floor (CACHE_FLOOR) and evicts what the import put there.
- fxdax dax::uffd::tests::windows_larger_than_the_map_count_read_whole: SIGKILL, the OOM killer (dmesg: 7.8 GB shmem-rss).
Also flaky under the load (passed on rerun): extent_store::host directories_are_taken_one_at_a_time ('taken while held'), fxdax extentd::tests::serves_hundreds_of_forks_of_a_base (thread count).
Either the tests say what they need of the host (a disk filesystem for TMPDIR, free space over the cache floor, memory) and skip or fail saying so, or ubu1's test setup gives them a TMPDIR on its disk and runs memory-heavy tests alone.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each of the three either passes on ubu1 or fails naming the host's shortfall (tmpfs TMPDIR, free space under the cache floor, memory)
- [ ] #2 The workspace passes on ubu1 with TMPDIR on its disk
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
TMPDIR on ubu1's disk (ZFS, ~/src/t249-tmp): imports_read_what_they_made_from_the_hosts_cache, directories_are_taken_one_at_a_time and serves_hundreds_of_forks_of_a_base pass; durable_commits_keep_the_open_segments_pages_cached_on_the_host still fails '0 of 8 pages cached': ZFS keeps file data in its ARC, not the page cache mincore reads, so the test needs a page-cache filesystem (ext4, xfs) or must say it has none.
<!-- SECTION:NOTES:END -->
