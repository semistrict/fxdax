# virtio-PCI's parallelism: queues per vCPU, completions on the submitting CPU (TASK-276)

Every fxdax VM runs on virtio-PCI since TASK-273, whose MSI-X gives each queue a vector of its own.
This measures how fxdax's filesystems scale with jobs and vCPUs on it, finds where they stop, and
measures what TASK-276 changed against that, on one GCE host. What landed: the extent device offers
a request queue per vCPU (Firecracker, fxdax-v1.16.1-r4), and the guest's driver puts each CPU's
requests on the queue whose interrupt is affine to that CPU. What was tried and dropped: KVM's
prefault of private pages moved off the VM process's receiving thread.

**Host.** One c3-standard-8 (8 vCPUs, Sapphire Rapids, nested KVM), `fxdax-keep-c3-standard-8-us-east5-b-1`,
for every bench run; quick checks and guest rounds on `fxdax-keep-n2-standard-4-us-east4-a-1`.
With 8-vCPU guests the host is oversubscribed: the guest's 8 vCPUs share it with the VMM and
extentd, and every case, ext4's too, falls at 8 vCPUs.

**Workloads.** The fs bench's scaling group (`guest/tests/test_cases/src/test_fs_bench.rs`,
`FXDAX_BENCH_GROUPS=scaling`), at 1, 2, 4 and 8 jobs (`FXDAX_BENCH_JOBS`), each job on a file or
directory of its own: 4 KiB random reads cold (8 MiB a job) and warm (2 s), one job 16 deep through
io_uring, 4 KiB random writes with a final fsync, 4 KiB fsynced appends, and creates, cold stats and
unlinks of 2000 files a thread. fio's IOPS with its completion latency p50 and p99 (and its fsyncs'),
and for each workload the extent device's requests per queue and the share of requests completed on
a CPU other than the one that queued them (the driver's `completions` attribute's `cross`). Cases:
`fxfs-guest` (Fxfs, no DAX), `fxfs-guest-dax`, and `ext4-guest-kernel` (ext4 on virtio-blk), the
control. VMs of 2, 4 and 8 vCPUs (`FXDAX_BENCH_VCPUS`).

**Method.** The baseline's scaling is three runs of three iterations each (`baseline-v2`, `-v4`, `-v8`
beside this file), medians. Before-and-after runs hours apart could not attribute 10-25%
changes: the unchanged ext4 control moved up to 27% between them. So every comparison here is an
interleaved A/B on the one host (`scripts/bench-ab-gce.sh`): each side built there once, then three
rounds of two iterations each, A B, B A, A B, six iterations a side, against the prebuilt
artifacts. `scripts/bench-ab.py --control ext4-guest-kernel` calls a difference real only where the
sides' ranges don't overlap and it is larger than the ext4 control's own B-vs-A ratio for the same
workload; otherwise it is within drift. Each A/B's full table is its `ab-*/ab.md`.

## Baseline: where it stops

IOPS (creates: ops/s), thousands, medians of three, main before TASK-276:

| Workload | Case | 2 vCPUs: j1 → j2 → j4 → j8 | 4 vCPUs | 8 vCPUs |
|---|---|---|---|---|
| rand-read-cold | ext4/blk | 29.7 → 56.1 → 75.2 → 89.0 | 26.6 → 49.3 → 59.4 → 65.8 | 26.3 → 53.9 → 38.8 → 47.8 |
| rand-read-cold | fxfs | 4.1 → 7.0 → 13.6 → 20.5 | 3.7 → 7.0 → 12.8 → 18.9 | 3.6 → 7.0 → 10.6 → 12.8 |
| rand-read-cold | fxfs-dax | 5.6 → 9.9 → 22.8 → 25.4 | 5.6 → 10.1 → 22.7 → 36.8 | 5.8 → 11.2 → 23.5 → 35.5 |
| rand-write | ext4/blk | 23.3 → 42.7 → 43.1 → 43.6 | 22.8 → 43.6 → 43.6 → 40.2 | 22.3 → 44.5 → 44.0 → 32.5 |
| rand-write | fxfs | 68.3 → 107.8 → 120.5 → 107.8 | 64.0 → 107.8 → 157.5 → 151.7 | 64.0 → 102.4 → 151.7 → 128.0 |
| rand-write | fxfs-dax | 6.3 → 12.0 → 12.3 → 11.3 | 6.0 → 11.6 → 10.2 → 10.3 | 6.2 → 11.8 → 10.8 → 10.3 |
| fsync-append | ext4/blk | 0.4 → 0.5 → 0.9 → 1.7 | 0.5 → 0.5 → 0.9 → 1.7 | 0.4 → 0.6 → 0.9 → 1.6 |
| fsync-append | fxfs | 3.9 → 4.1 → 4.8 → 4.7 | 2.9 → 3.8 → 5.3 → 5.7 | 2.9 → 3.1 → 3.8 → 4.0 |
| fsync-append | fxfs-dax | 3.2 → 3.9 → 4.6 → 4.6 | 3.3 → 3.3 → 4.9 → 5.3 | 2.5 → 3.0 → 3.5 → 3.8 |
| create | ext4/blk | 106 → 158 → 158 → 162 | 107 → 145 → 211 → 240 | 106 → 163 → 209 → 311 |
| create | fxfs | 84 → 132 → 118 → 106 | 86 → 132 → 133 → 129 | 71 → 105 → 128 → 107 |
| create | fxfs-dax | 87 → 128 → 102 → 117 | 86 → 118 → 130 → 119 | 70 → 135 → 124 → 108 |

What stops them, from the bench's own counters and a whole-host profile (`profile/`, 4 vCPUs, 8
jobs, `FXDAX_BENCH_PERF=record-all`):

- **Every completion on the wrong CPU.** 100% of fxfs requests, at every vCPU and job count,
  completed on a CPU other than their submitter's: the driver sent CPU i's requests to queue i and
  asked for no interrupt affinity, and the transport put queue i's vector, after the
  configuration's, on CPU i+1. Each completion woke its caller across CPUs.
- **DAX random writes: ~10-12k IOPS from 2 jobs on, any vCPU count**, latency linear in jobs (p50
  140 µs at 1 job, 725 µs at 8). Each 4 KiB write is a `PRIVATE`, and the VM process applies every
  change of the DAX window on its one receiving thread, in order; that thread is saturated: 47% in
  `mmap` of private memory over an installed page, 13% `fallocate`, both mostly
  `smp_call_function_many_cond` (41%): TLB-shootdown IPIs to the vCPUs and KVM's remote flushes,
  which nested virtualization makes dear; 7.6% KVM's prefault of the pages; 7% `recvmsg`. `mmap`
  holds the process's `mmap_lock` for write, so more threads would not overlap them. TASK-276.6.
- **Non-DAX cold reads are latency-bound, not serialized**: each 4 KiB read one `READ` of 150-190
  µs at every job count (extentd's own read 157 µs), so throughput grows with jobs. The receiving
  thread spends a third of its time in copy-on-write faults of guest pages the host never wrote as
  it copies replies in (`wp_page_copy` → TLB shootdown), not yet a limit (TASK-276.7).
- **The daemon connection doesn't serialize**: the client keeps up to 4096 requests in flight,
  extentd serves 8 of a connection's at once; neither saturates.
- **ext4 on single-queue virtio-blk** scales its cold reads to 48-89k IOPS and stays above fxfs
  everywhere: no single-queue ceiling that matters to fxdax, so no virtio-blk or virtio-net
  multiqueue (TASK-276.5, skipped).
- **fsync-append ~4-5k IOPS** at every job and vCPU count, fsync p99 12-17 ms at 8 jobs: Fxfs's
  commits, outside the device's queues.
- **Fxfs creates stop at ~105-135k/s from 2 jobs on** while ext4 scales to 311k at 8 vCPUs; they
  send the device almost nothing. A guest kernel profile (`create-profile/`) puts 8 creating threads
  21% parked inside `fxfs_create`, with 13% of the CPUs idle, and the work in the commit path
  (`Transaction::commit` 27%, `Journal::commit` 17%): every create's transaction through the
  journal one at a time. TASK-288.

## Each change alone

Interleaved A/Bs on one build where a guest kernel argument turns a change back
(`virtio_extent.affine=0`, `virtio_extent.max_queues=4`), or two builds where it can't.

**Interrupt affinity** (`ab-affine-v4`, 4 vCPUs, `affine=0` against the default). Cross-CPU
completions 100% → 0%. Real, past the control's drift:

| Workload | affine=0 | affine | |
|---|---|---|---|
| fxfs-guest-dax rand-write j1 | 5,937 IOPS | 6,896 | +16% |
| fxfs-guest-dax rand-write j2 | 11.2k | 13.0k | +17% |
| fxfs-guest-dax rand-write j4 | 10.3k (p50 338 µs) | 11.8k (285 µs) | +14% |
| fxfs-guest rand-read-cold p50 j1 / j2 | 120 / 128 µs | 106 / 114 µs | −11 to −12% |
| fsync p50, fxfs and fxfs-dax, j1-j8 | | | −8 to −12% |
| fxfs-guest rand-read-cold-qd16 p99 | 1,163 µs | 1,401 µs | **+20%** (TASK-276.8) |

The one regression: one job keeping 16 cold reads in flight through io_uring, its p99 up 20% (its
IOPS 29.7k → 27.7k, within drift). Every other throughput within drift.

**A queue per vCPU** (`ab-queues-v8`, 8 vCPUs, the driver using 4 of the device's 8 queues
against all 8; the device's own count isn't a guest argument, and 4 used of 8 is what the old
device's 4 were). Queues used at 8 jobs 4 → 8. Real: fsync p50 −6 to −15% (fxfs-guest j1,
fxfs-guest-dax j4, j8); every throughput within drift. On this host, oversubscribed at 8 vCPUs, the
queues add little beyond affinity.

**Prefault behind the receiving thread** (`ab-prefault-v4`, two builds, fxfs-guest-dax and the
control, 4 vCPUs): no IOPS past drift, the inline prefault 1.08x ahead at 1-2 jobs, and random
writes' p99 at 8 jobs worse, 1,073 → 1,155 µs (real): the guest's store reached the page before the
prefault and paid the nested fault itself. Dropped, not landed (TASK-276.4).

**All three, before the drop** (`ab-final-v4`, `ab-final-v8`: main before TASK-276 against the
three changes, on r2): 12 real differences at 4 vCPUs and 20 at 8, every one an improvement, none a
regression past drift; the fxfs-guest random writes' −19/−25% that runs hours apart had shown
were within drift (8 jobs: 134-158k against 99-158k).

## What landed

`ab-landed-v4`: the tree that landed (Firecracker fxdax-v1.16.1-r4: TASK-277's r3 and the queue
per vCPU; the driver's affinity; no prefault change) against main before it with only the bench
added, both on r3, 4 vCPUs, three interleaved rounds of two iterations, ext4 the control. 17 real
differences, every one an improvement; no regression past drift.

| Workload | main | landed | |
|---|---|---|---|
| fxfs-guest-dax rand-write j1 | 6,206 IOPS (p50 141 µs) | 7,065 (123 µs) | +14% |
| fxfs-guest-dax rand-write j2 | 11.6k | 13.4k | +15% |
| fxfs-guest-dax rand-write j4 | 10.5k (p50 330 µs) | 12.4k (272 µs) | +18% |
| fxfs-guest-dax rand-write j8 | 10.5k (p50 709 µs) | 10.9k (680 µs) | +4% |
| fxfs-guest rand-read-cold p50 j1 / j2 / j4 | 118 / 129 / 159 µs | 106 / 112 / 134 µs | −11 to −15% |
| fxfs-guest fsync-append j1 | 3,106 IOPS | 3,569 | +15% |
| fsync p50, j1 / j8 (fxfs) | 291 / 795 µs | 256 / 680 µs | −12 / −14% |
| fsync p50, j8 (fxfs-dax) | 836 µs | 729 µs | −13% |
| fxfs-guest fsync-append j8 p99 | 121 µs | 76 µs | −37% |

Within drift: every cold-read IOPS (fxfs-guest j8 18.8k → 19.7k, fxfs-dax j8 36.3k → 36.5k), the
16-deep io_uring read (30.8k → 29.7k), creates, stats and unlinks, and fxfs-guest's random writes,
whose j8 median fell 159k → 104k with ranges of 111-164k against 93-161k: bimodal, as this
workload is (buffered writes, then the fsync at the end), and the earlier A/Bs had it 1.15x slower
at 4 vCPUs and 0.95x at 8, no direction.

So the DAX write ceiling moved by about a sixth at 2-4 jobs and not at 8: what holds it at 10-11k
is the receiving thread's TLB shootdowns (TASK-276.6), which neither queues nor affinity touch.
Reads' and fsyncs' latency fell by a tenth to a seventh, completions now waking their submitter on
its own CPU.

## Tests

On the landed tree (fxdax b199aea26, the code as it landed, Firecracker fxdax-v1.16.1-r4): the vmm crate's suite with
KVM, 774 of 775 (TASK-271's `test_set_tsc`, upstream's, red on every GCE host); the eleven guest
cases on the PCI paths; `fxfs-guest-snapshot-dax` ten rounds, r3's vsock reset with the new queue
counts. Before the rebase, on the same changes: the release round, the workspace 2,418 of 2,418 and
every guest suite, the scripts part 244 of 247 (TASK-279: the host's tree has no `.git`, and a
test loses rustup's toolchain); the debug round 15 of 16 suites, the one TASK-257's flaky free-space
check, which fails on main's kernels too.
