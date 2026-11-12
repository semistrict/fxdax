# Qualification A/B: LangSmith's pin 96c7808b8 against main 0ee1bdb58 (TASK-330.4)

`scripts/bench-ab-host.sh` with `FXDAX_AB_SUITE=qualify` on ubu1: each side's extent daemon,
Firecracker and guest kernel built from its commit, driven by one harness (main's runner and
guest agent, `FXDAX_E2E_HARNESS`); each round `scripts/qualify.sh`: the sandboxes' long mode
(LangSmith's 16 VMs, eight at a time, 8-vCPU builders, 28 vCPUs on 8 CPUs, two extent daemons
and RustFS on one ext4 disk, in the 16 GiB a measurement has) and the bucket soak (1,800 s,
RustFS through a proxy that slows, fills and takes the bucket out of service). The rounds ran
alone on the bench's CPUs (`fxjob --bench`), the builds before them as ordinary work.

Two runs: run4 (2 rounds, pin then main, then main then pin) and run5 (1 round, pin then
main). Each round's measurements and failed checks are in `runN/SIDE-rK/bench-*.json`, what the
cases said in `said.txt`, each driver's own comparison in `runN/ab.md`.

## Which rounds count

`fxjob --bench` isolates CPU, cache and memory bandwidth, not the disk (TASK-375):

- **run4 round 1 (pin-r1, main-r1): contaminated.** TASK-369's loader rewrote 1 GiB with fsync in
  a loop, ~186 MB/s, on the same NVMe, 01:54:30-02:45:16 EST: all of pin-r1 (01:54-02:27) and
  main-r1's sandboxes (02:27-02:44) and its soak's first minute; TASK-374's `drop_caches` (~02:50)
  in main-r1's soak too. Their timings (framings, latencies, durations, CPU, upload catch-up)
  don't count; their memory and correctness results do, and agree with the clean rounds.
- **run4 round 2 (main-r2 03:14-03:58, pin-r2 03:58-04:30) and run5 round 1: clean** of known
  load (TASK-374's loaders blocked on their go pipe until 04:30:13; step 146 caps work's writes at
  32 MB/s during a measurement from then on).

`ab-clean.md` compares those two rounds a side, one in each order.

## What moved, clean rounds

| | pin 96c7808b8 | main | main vs pin |
|---|---|---|---|
| Soak: daemon's disk writes, for the guests' 15,360 MiB | 30.25 GiB | 15.20 GiB | 0.50x |
| Soak: daemon's peak memory (VmHWM) | 4,359 MiB | 1,671 MiB | 0.38x |
| Soak: everything uploaded after the bucket answered | 50.1 s (25-75) | 21.7 s (20.9-22.5) | 0.43x |
| Soak: daemon's CPU | 249 s | 386 s | 1.55x |
| Soak: bucket failures said (full, out of service) | none | every phase | |
| Sandboxes long mode | stopped every round, memory out (daemons 4.3-5.6 GB) | 16 VMs run | |
| Sandboxes: stores' writes per guest byte | not reached | 1.20x | |
| Sandboxes: framings past 1 s | not reached | 4-12 a host, slowest 1.2-4.7 s | |

- **TASK-310's write amplification falls**: the pin's daemon writes every guest byte twice, raw
  and framed (2.0x the guests' bytes), main's once (1.0x), on the same soak.
- **TASK-317's memory**: the pin's daemon grows past what LangSmith's shape has room for: its
  sandboxes ran 16 GiB out of memory every round (15.4 GiB held past reclaim, its daemons
  4.3-5.6 GB), where main's daemons peak at 3.1 GB; in the soak 4.4 GB against 1.7 GB.
- **TASK-314**: the pin says nothing of a full or failing bucket; main counts every failure.
- Main's daemon spends half again the pin's CPU in the soak: what framing in place, durable
  together, and the budget's sampling cost; not a check's.
- **Main fails the framing check still** (any framing past 1 s), TASK-310's remainder: 4-12
  framings a host past it in the clean rounds, the slowest 1.2-4.7 s.
- run5's main-r1 lost two git VMs to the harness, not fxdax: git's automatic gc, detached, held
  its lock as the role's own `git gc` ran ("gc is already running"); fixed by running it
  undetached (`gc.autoDetach=false`). Its other 14 VMs, and its bytes, stand.

The pin has none of TASK-330.2's budgets (`budgets.unreported`): what it costs is its kernel's
count, which the cases read for both sides alike.
