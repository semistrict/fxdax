# fxdax against ext4 on virtio-blk, on GCE, 2026-10-10

`scripts/bench-gce.sh` (TASK-25): libkrun guests run the same workloads on fxdax's virtio-fs
device with DAX and without, on ext4 over a virtio-blk disk, and on libkrun's own virtio-fs device
passing a host directory through, which measures the transport fxdax's device shares without
fxdax. The run took 481 s, building libkrun and the tests from scratch. The measurements, the
guests' logs, the host and the source measured are in
[gce-fs-bench-2026-10-10/](gce-fs-bench-2026-10-10/).

## Setup

- Host: GCE c3-standard-8 (Intel Sapphire Rapids, 8 vCPUs, 32 GiB) in us-east4-b, with nested
  virtualization, Ubuntu 26.04 (Linux 7.0); the images on the boot disk's ext4 (pd-balanced).
- Guests: libkrun with libkrunfw 5.6.2's kernel, 2 vCPUs and 2 GiB, an Alpine 3.22 root
  filesystem (fio, gcc, make, e2fsprogs) over libkrun's virtio-fs.
- ext4: an 8 GiB sparse raw image through `krun_add_disk3` with libkrun's recommended defaults:
  the host's page cache, and flushes honoured (`KRUN_SYNC_RELAXED`). The guest makes it with
  `mkfs.ext4` and mounts it with the defaults.
- fxdax: an 8 GiB sparse image through `krun_add_virtiofs_fxdax`, with a 2 GiB DAX window
  (`dax=always`) or none (`dax=never`). fxdax's in-process kernel has its default 1 GiB, which
  libkrun cannot change, so the data file is 256 MiB, which both it and the guest cache whole.
- Passthrough: `krun_add_virtiofs` of a directory on the same filesystem, `dax=never`.
- Workloads, 3 iterations each, in order (`test_fs_bench.rs`): fio on one 256 MiB file
  (sequential 1 MiB writes with a final fsync; 4 KiB random reads cold, after `drop_caches`;
  sequential reads cold and warm; random reads warm for 2 s; 16 MiB of 4 KiB random writes with a
  final fsync; mmap random reads for 2 s and 16 MiB of mmap random writes with a final fsync),
  256 fsynced 4 KiB appends, then creating, stating (cold), listing and removing 2,000 files, and
  untarring and building Lua 5.4.7 with `make -j2`. fio's reads report its own rates; writes are
  timed from starting fio to its exit, so they include the final fsync. fio's own cache
  invalidation is off: `drop_caches` makes the cold reads cold.

## Results

The ratio is how many times slower than ext4 each is: its median time over ext4's, or ext4's
median rate over its. Medians, with the spread from minimum to maximum.

| Workload | Unit | ext4-virtio-blk | fxdax-dax | fxdax-nodax | virtiofs-passthrough | fxdax-dax vs ext4 | fxdax-nodax vs ext4 | virtiofs-passthrough vs ext4 |
|---|---|---|---|---|---|---|---|---|
| seq-write | MiB/s | 107 (96.3–119) | 34.6 (33.5–41.6) | 58.2 (58.2–70.0) | 119 (96.3–119) | **3.09×** | 1.84× | 0.90× |
| rand-read-cold | IOPS | 15,284 (15,227–15,515) | 8,943 (8,715–8,982) | 9,503 (7,685–9,503) | 9,638 (8,982–10,114) | 1.71× | 1.61× | 1.59× |
| seq-read-cold | MiB/s | 3,160 (2,909–3,368) | 172 (171–173) | 1,641 (320–1,695) | 1,590 (1,561–1,882) | **18.35×** | 1.93× | 1.99× |
| seq-read-warm | MiB/s | 6,919 (6,737–7,111) | 9,846 (9,481–9,846) | 6,919 (6,737–7,111) | 1,673 (1,542–1,695) | 0.70× | 1.00× | **4.14×** |
| rand-read-warm | IOPS | 795,273 (793,731–798,309) | 862,344 (860,893–865,104) | 814,983 (799,035–819,706) | 9,133 (9,056–10,062) | 0.92× | 0.98× | **87.08×** |
| rand-write | IOPS | 6,363 (6,361–6,363) | 5,453 (4,574–5,461) | 4,584 (4,577–4,584) | 4,578 (3,581–4,583) | 1.17× | 1.39× | 1.39× |
| mmap-rand-read | IOPS | 1,202,111 (1,184,322–1,202,963) | 13,395 (13,172–13,949) | 9,135 (9,002–9,345) | 8,288 (7,829–9,174) | **89.74×** | **131.59×** | **145.04×** |
| mmap-rand-write | IOPS | 6,348 (6,338–6,354) | 1,548 (1,542–1,709) | 6,342 (6,301–6,346) | 3,578 (3,574–4,576) | **4.10×** | 1.00× | 1.77× |
| fsync-append | IOPS | 224 (224–224) | 397 (395–398) | 398 (398–398) | 399 (398–399) | 0.56× | 0.56× | 0.56× |
| create | ops/s | 75,454 (74,938–75,942) | 1,211 (1,181–1,258) | 1,184 (1,181–1,238) | 1,469 (1,449–1,647) | **62.31×** | **63.74×** | **51.37×** |
| stat-cold | ops/s | 355,470 (353,772–358,362) | 2,640 (2,621–2,669) | 2,572 (2,568–2,602) | 10,122 (10,108–10,220) | **134.64×** | **138.23×** | **35.12×** |
| readdir | ops/s | 2,488,420 (2,290,597–2,524,166) | 314,497 (300,688–342,920) | 361,513 (315,990–366,762) | 358,982 (355,410–363,300) | **7.91×** | **6.88×** | **6.93×** |
| unlink | ops/s | 77,921 (77,637–82,193) | 2,755 (2,735–2,855) | 2,835 (2,815–2,908) | 3,917 (3,894–3,974) | **28.28×** | **27.48×** | **19.89×** |
| untar | s | 0.14 (0.13–0.16) | 0.25 (0.25–0.26) | 0.24 (0.24–0.26) | 0.21 (0.20–0.22) | 1.85× | 1.78× | 1.51× |
| make | s | 7.24 (7.16–7.49) | 7.79 (7.56–7.89) | 7.68 (7.54–7.90) | 7.59 (7.54–7.88) | 1.08× | 1.06× | 1.05× |
| build-total | s | 7.38 (7.29–7.65) | 8.04 (7.81–8.15) | 7.93 (7.78–8.15) | 7.79 (7.73–8.10) | 1.09× | 1.08× | 1.06× |

- ext4-virtio-blk: mtime-ahead-of-guest-ms -4.38 median (-5.28–-2.79)
- fxdax-dax: mtime-ahead-of-guest-ms 169 median (169–170)
- fxdax-nodax: mtime-ahead-of-guest-ms 170 median (170–170)
- virtiofs-passthrough: mtime-ahead-of-guest-ms 170 median (170–170)

## Findings

- **The transport.** Passthrough is as slow as fxdax on creating (51×), listing (7×) and removing
  (20×) files and on mmap reads (145×): a metadata operation over virtio-fs takes 0.1 to 0.7 ms here, under
  nested KVM, against microseconds for ext4's metadata in the guest's own caches. fxdax adds to it
  on cold `stat`s (2.6k against passthrough's 10k per second) and creates (1.2k against 1.5k).
- **DAX reads from a cold window** run at 172 MiB/s, against 1.6 GiB/s without DAX: each page of
  the window the guest first touches is a fault that fxdax resolves page by page.
- **DAX writes**: sequential writes at 35 MiB/s, against 58 without DAX and 107 on ext4, and mmap
  writes at a quarter of ext4's rate.
- **mmap reads** through virtio-fs, with DAX or without, are 90 to 145× slower than ext4's from
  its page cache.
- **Warm sequential reads** take about 40 ms, which fio's millisecond runtimes quantize: ext4's
  and fxdax's without DAX come out the same.
- **Warm reads** match or beat ext4's: from the guest's page cache without DAX, and straight from
  the window with it.
- **fsync** is faster than ext4's on every virtio-fs backend (398 against 224 per second); ext4's
  journal commits pass through virtio-blk's flushes.
- **Timestamps.** Files on virtio-fs are stamped by the host's clock, which is 170 ms ahead of
  the guest's here, while ext4 stamps them with the guest's. `make` warns of clock skew, and a
  build's generated files can look out of date (a GNU make `configure` failed once in an earlier
  run).

Gaps over 2×:

- seq-write on fxdax-dax: 3.09×
- seq-read-cold on fxdax-dax: 18.35×
- seq-read-warm on virtiofs-passthrough: 4.14×
- rand-read-warm on virtiofs-passthrough: 87.08×
- mmap-rand-read on fxdax-dax: 89.74×
- mmap-rand-read on fxdax-nodax: 131.59×
- mmap-rand-read on virtiofs-passthrough: 145.04×
- mmap-rand-write on fxdax-dax: 4.10×
- create on fxdax-dax: 62.31×
- create on fxdax-nodax: 63.74×
- create on virtiofs-passthrough: 51.37×
- stat-cold on fxdax-dax: 134.64×
- stat-cold on fxdax-nodax: 138.23×
- stat-cold on virtiofs-passthrough: 35.12×
- readdir on fxdax-dax: 7.91×
- readdir on fxdax-nodax: 6.88×
- readdir on virtiofs-passthrough: 6.93×
- unlink on fxdax-dax: 28.28×
- unlink on fxdax-nodax: 27.48×
- unlink on virtiofs-passthrough: 19.89×
