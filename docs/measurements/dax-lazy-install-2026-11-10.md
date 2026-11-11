# DAX random writes on bare metal, and a MAP that names pages (TASK-276.6, TASK-276.7)

TASK-276 measured on GCE's nested KVM, where DAX random 4 KiB writes stopped at 10-12k IOPS from 2
jobs on, the VM process's receiving thread saturated by TLB shootdowns: 47% `mmap` of a private
page over a page installed, 13% `fallocate`. This measures the same bench on bare metal first, then
what TASK-276.6 changed: a `MAP` names pages, their first touch installs them, and a private page
is filled as its memory object's and emptied once nothing maps it (doc-13, The host).

**Host.** ubu1: AMD Ryzen 9 8945HS (8 cores, 16 threads), 27 GiB, KVM on bare metal, Linux 7.0.
Every run a measurement job (`fxjob --bench`): CPUs 4-7 and 12-15 to itself, other agents' jobs
held to the rest meanwhile. The cases' stores and results on ZFS (/home), extentd's bucket on tmpfs:
fsyncs and cold reads from the store pay ZFS, not the ext4 or xfs of production hosts.

**Workloads.** TASK-276's scaling group (`FXDAX_BENCH_GROUPS=scaling`), fxfs-guest-dax, fxfs-guest
and ext4 on virtio-blk, the control, and a workload added here, `scale-rand-write-unread`: the same
random writes to files the guest read only through its page cache as it made them, so that extentd's
memory holds their extents, and never through its DAX window, each write a page's first touch
there, as a VM's of a database the host's memory holds (another VM's, a base's), opened cold and
written at random. The old `scale-rand-write` writes pages `rand-read-warm` read whole before it.

## Bare metal, main (`baseline-v2`, `-v4`, `-v8`)

Main (f36453323) with the bench, prebuilt, three iterations, medians; fxfs-guest-dax random writes,
IOPS, 1 / 2 / 4 / 8 jobs:

| vCPUs | read pages | (unread, first variant) |
|---|---|---|
| 2 | 15.3k / 31.5k / 35.3k / 36.2k | 13.1k / 25.0k / 20.1k / 18.5k |
| 4 | 15.8k / 30.1k / 41.4k / 36.4k | 14.2k / 23.5k / 22.9k / 22.7k |
| 8 | 12.8k / 24.1k / 33.6k / 26.0k | 11.6k / 21.1k / 19.4k / 15.4k |

(The baseline's unread files were not in extentd's memory, so each PRIVATE brought its page in from
the store: its plateau and its 4-11 ms p99s are ZFS's reads. The variant was then changed to warm
extentd's memory first, as above; the A/Bs below use it.)

So on bare metal the writes scale past 12k with jobs on main already: the 10-12k plateau was
nested virtualization's price for the shootdowns. What stops them at 33-41k is not the receiving
thread: perf (999 Hz) puts it at 26% of a CPU at most in any second of fxfs-guest-dax's case at 4
vCPUs and 8 jobs, 54% under fxfs-guest's cold reads.

`profile/stage-syscalls-main.txt` (`scripts/dax-stage-syscalls.bt`, the bench's CPUs), the same
case: the second of `scale-rand-write-j8` made 8,192 PRIVATEs, 8,495 `mmap`s of private memory over
pages installed (116 ms in all, 14 µs each), 13,154 cross-CPU calls and 8,192 KVM remote flushes;
no `fallocate`. The 12,801 `fallocate`s came in one second at the case's end, with 5,654 `PROT_NONE`
`mmap`s: the unlinks' UNMAPs of the private pages, each a hole punched. GCE's 13% `fallocate` was
these punches, not allocation.

## The change against main (`ab-landed-v4`)

Interleaved A/B (`scripts/lib/bench-ab-linux-host.sh` on a provisioned host, `FXDAX_HOST_PROVISIONED=1`):
main 7e1740cb7 with the bench (83c212a7b, base) against the branch as it lands (9e338efa2, lazy,
Firecracker 323b5a5af), each built once, 3 rounds of 2 iterations, A B, B A, A B, 4 vCPUs, ext4 the
control (`scripts/bench-ab.py`: real where the ranges don't overlap and past the control's own drift).
11 real differences, every one an improvement, all of fxfs-guest-dax's writes to unread pages:

| Jobs | base IOPS (p50, p99) | lazy IOPS (p50, p99) | |
|---|---|---|---|
| 1 | 13.2k (46.6 µs) | 17.1k (36.1 µs) | +29% |
| 2 | 24.4k (51.7 µs, 601 µs) | 34.7k (39.9 µs, 111 µs) | +42% |
| 4 | 25.4k (105 µs, 1,155 µs) | 42.1k (64.9 µs, 230 µs) | +66% |
| 8 | 25.4k (211 µs, 1,991 µs) | 41.9k (124 µs, 525 µs) | +65% |

The writes to pages read before are within drift (1-8 jobs: 15.0k / 29.9k / 39.0k / 35.6k against
15.2k / 28.6k / 40.6k / 34.0k), as expected: their pages are installed by the reads, so a PRIVATE
still replaces one, a shootdown each. Every other workload of the three cases is within drift.

Why (`profile/stage-syscalls-check-*.txt`, one iteration a side at 8 jobs): main's unread writes
took 8,380 `mmap`s over pages installed and 8,184 KVM flushes in their second, MAP_AHEAD having
installed what a write's MAP mapped; the branch's took 134 `mmap`s and no KVM flush, each private
page filled in place of nothing. The case's closing UNMAPs, 12,801 punches each side: 32,883
cross-CPU calls and 13,122 KVM flushes on main, 6 and 321 on the branch, each private page punched
once nothing mapped it.

An earlier A/B of the same change, before the rebase onto 7e1740cb7 (5,120 s, base f36453323), had
9 real differences, all improvements, the same: unread writes +41% / +56% / +65% at 2 / 4 / 8 jobs.

## TASK-276.7: the reply path's copy-on-write faults (`ab-init-on-alloc-v4`)

fxfs-guest's cold reads on bare metal: 8.2k / 13.4k / 19.8k / 25.3k IOPS at 1-8 jobs, 4 vCPUs (ext4
on virtio-blk 23.0k / 53.9k / 52.5k / 48.5k). The receiving thread, over the case: `wp_page_copy`
12.5% (the copy-on-write faults TASK-276's GCE profile found), `munmap` 18% (large payloads,
TASK-324), `memcpy` 21%, `recvmsg` 21%, waiting 12%; 54% of a CPU at most in any second.

An interleaved A/B on one build, the guest booted with `init_on_alloc=1`, writing every page it
allocates, against the default (fxfs-guest, ext4 the control, 4 vCPUs, 3 rounds x 2): no real
difference anywhere (cold reads at 8 jobs 19.9k either way). A profile of it left `wp_page_copy`
at 11.7% of the thread: the faulted pages are not pages fresh to the guest, the zero page it read,
so populating guest memory writable would not remove them, and would make each VM's RAM resident
at boot. Which pages they are is TASK-340. Not done.

## Tests

On the landed tree: the guest e2e suite, fxfs-guest-snapshot-dax and fxfs-guest-pgbench-dax-forked,
18 of 18 (ubu1, its own build and kernel); the VMM's tests (`test_config_space_as_upstream` among
them) and the Shuttle suites; the uffd units 34 of 34; specs/dax-cow's 14 models; the workspace and
scripts on macOS (2,204 and 260); `scripts/firecracker-series-check.sh` both ways.
