# `fxdaxctl stats --json` from a live VM (GCE r681, 2026-11-05)

TASK-111.4: what fxfs-guest-stats's host ran against the extent daemon while its VM ran, twice
(`stats-1.json`, `stats-2.json`). n2-standard-4, us-central1-a, release guest kernel, tree at
c82e2be51 plus the uncommitted change (the builds' `-dirty`).

The first snapshot: the daemon's, the VM process's and the guest's builds, one commit; the VM
process answered 0.96 ms after it was asked; its window maps 8 ranges, 8 MiB, 2,048 pages; its
second stage has 1 run installed of a budget of 522,240 (the host's vm.max_map_count, 1,048,576,
as the daemon told it, TASK-172); the guest's 70 counters and 10 histograms, `device_writes` 24 and
`window_map_requests` 4, each between what the guest read of its /proc/fs/fxfs/extents just before
asking and just after. The second, after the guest wrote and read another 8 MiB, counts its writes
since.
