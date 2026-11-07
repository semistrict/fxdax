# A Linux-sized checkout on Fxfs, with and without DAX, against ext4, and shared, 2026-11-07

TASK-136.1, for doc-10. The cases are `linux-checkout-ext4`, `linux-checkout-fxfs` and
`linux-checkout-fxfs-dax` (guest/tests/test_cases/src/test_linux_checkout.rs). They are in no
suite, so each runs by name:
`FXDAX_E2E_TEST="linux-checkout-ext4 linux-checkout-fxfs linux-checkout-fxfs-dax" scripts/test-gce.sh`.
The tables below are `scripts/linux-checkout-report.py r870/e2e`. The raw output is in
[r870/](linux-checkout-2026-11-07/r870/).

**Setup.** GCE r870 ran on one n2-standard-8 host in us-central1-b: Cascade Lake, 2.8 GHz, 8 vCPUs,
32 GB, Ubuntu's 7.0.0-1011-gcp, nested KVM. The three cases ran one after the other on that host.
The guest kernel is fxdax's: 6.18.54-121.166.amzn2023 with the series, x86_64, a release build made
on the host. The source was main 5fab013b0 plus this change, and Firecracker was 05dd8e3df. Each VM
had 4 vCPUs and 4 GiB. A case runs its own extent daemon with its default memory, 1 GiB of arenas,
and a file bucket on the host's disk. The Fxfs stores have 8 GiB of capacity, and the DAX window is
1 GiB. ext4 sits on a virtio-blk disk backed by a raw image that the host's page cache caches
(`cache_type: Writeback`).

**The checkout.** Linux v6.18 at one commit, cloned with `--depth 1` into a bare repo in the root
filesystem's image: 91,166 tracked files, 6,059 directories and 1.83 GB. Of that, `.git` is 296 MB
in one pack, and the working tree read whole is 1.54 GB.

**What each VM does.**
- `checkout` clones the repo with `--no-local --no-checkout` (copying the pack and indexing it), then
  runs `git reset --hard` to write the 91k files, then `sync`. Next come the publisher's index
  rules from doc-10. Then, with its caches dropped, it runs `git status` cold, warm, after a `sync`,
  and warm again. Last, it reads every working-tree file (not `.git`) cold, then warm.
- `reopened` is a fresh VM on the same disk or store that repeats the statuses and the read.
- On Fxfs, `checkout` then publishes the volume, and the shared-volume roles follow:
  - `sandbox-0..2` each run on a store of their own. Each attaches the generation, imports it, mounts
    it, runs the statuses (the first lstats every tracked file and hashes none, which the test checks
    with strace) and the read, then writes a byte to `MAINTAINERS` and creates a new file, each with
    fsync.
  - `hundreds` imports and mounts the volume 256 times.
  - `rebooted` boots `hundreds`' store again and mounts the volumes, one first and then the other 255.
- **Memory.** Each memory line is the host's reading at the moment the guest asks
  (`crate::measure`). "Guest used" is MemTotal minus MemAvailable inside the guest. "VM anon" is the
  guest RAM the VM process has touched, which the host keeps even after the guest drops its caches.
  "VM shmem" is the daemon's pages that the DAX window maps, shared between VMs.

## The checkout

|  | ext4 | fxfs | fxfs-dax |
|---|---|---|---|
| tracked-files | 91,166 files | 91,166 files | 91,166 files |
| volume-files | 91,192 files | 91,192 files | 91,192 files |
| volume-directories | 6,059 directories | 6,059 directories | 6,059 directories |
| volume-bytes | 1,831,268,657 bytes | 1,831,268,657 bytes | 1,831,268,657 bytes |
| git-bytes | 295,961,625 bytes | 295,961,625 bytes | 295,961,625 bytes |
| read-bytes | 1,535,304,277 bytes | 1,535,304,277 bytes | 1,535,304,277 bytes |
| manifest |  | 600 bytes | 600 bytes |
| volume-layers |  | 18,903,040 bytes | 18,907,136 bytes |
| volume-allocator-records |  | 1,557 records | 1,560 records |

## Checking it out, and opening it in a fresh VM

|  | ext4 checkout | fxfs checkout | fxfs-dax checkout | ext4 reopened | fxfs reopened | fxfs-dax reopened |
|---|---|---|---|---|---|---|
| mkfs | 435.8 ms |  |  |  |  |  |
| default-mount |  | 86.3 ms | 84.2 ms |  | 96.6 ms | 96.6 ms |
| mount | 58.1 ms |  |  | 77.1 ms | 22.3 ms | 22.6 ms |
| clone | 12.74 s | 11.79 s | 11.63 s |  |  |  |
| checkout | 16.80 s | 48.20 s | 47.64 s |  |  |  |
| sync | 4.06 s | 383.0 ms | 520.3 ms |  |  |  |
| status-cold | 364.5 ms | 696.3 ms | 802.9 ms | 684.9 ms | 833.2 ms | 1.02 s |
| status-warm | 126.4 ms | 178.9 ms | 177.0 ms | 175.8 ms | 165.0 ms | 166.0 ms |
| status-synced | 128.5 ms | 206.9 ms | 196.2 ms | 135.5 ms | 230.9 ms | 255.3 ms |
| status-warm-again | 126.8 ms | 172.1 ms | 167.9 ms | 157.6 ms | 223.6 ms | 148.6 ms |
| read-cold | 12.27 s | 28.46 s | 18.05 s | 19.51 s | 36.73 s | 18.71 s |
| read-warm | 586.4 ms | 611.6 ms | 26.75 s | 577.1 ms | 630.1 ms | 25.61 s |
| publish |  | 1.26 s | 796.1 ms |  |  |  |

## Shared, a volume over the published base

|  | fxfs | fxfs-dax |
|---|---|---|
| sandbox attach | 21.0 ms - 24.7 ms | 20.6 ms - 24.4 ms |
| sandbox import | 31.6 ms - 35.7 ms | 32.5 ms - 35.1 ms |
| sandbox mount | 1.5 ms - 1.8 ms | 1.5 ms - 1.6 ms |
| sandbox status-cold | 762.5 ms - 915.1 ms | 832.4 ms - 1.13 s |
| sandbox status-warm | 154.5 ms - 238.3 ms | 160.0 ms - 225.4 ms |
| sandbox status-synced | 175.6 ms - 200.6 ms | 183.0 ms - 271.7 ms |
| sandbox status-warm-again | 184.0 ms - 230.2 ms | 147.6 ms - 188.5 ms |
| sandbox read-cold | 35.71 s - 37.29 s | 37.61 s - 37.95 s |
| sandbox read-warm | 606.4 ms - 615.7 ms | 43.08 s - 44.02 s |
| sandbox first-overwrite | 2.5 ms - 2.9 ms | 7.7 ms - 9.5 ms |
| sandbox first-create | 1.8 ms - 2.4 ms | 1.9 ms - 2.4 ms |
| sandbox status-after-writes | 548.6 ms - 562.2 ms | 772.0 ms - 799.2 ms |
| hundreds import-median | 8.7 ms | 8.9 ms |
| hundreds import-max | 31.8 ms | 31.2 ms |
| hundreds mount-median | 0.4 ms | 0.4 ms |
| hundreds mount-max | 1.1 ms | 1.1 ms |
| hundreds status-cold | 783.5 ms | 854.2 ms |
| hundreds status-warm | 175.4 ms | 186.2 ms |
| hundreds status-synced | 177.0 ms | 206.0 ms |
| hundreds first-overwrite | 3.2 ms | 3.3 ms |
| hundreds first-create | 2.1 ms | 2.1 ms |
| hundreds unmount-all | 5.26 s | 5.15 s |
| rebooted default-mount | 121.9 ms | 129.1 ms |
| rebooted mount | 9.6 ms | 9.8 ms |
| rebooted status-cold | 760.4 ms | 839.2 ms |
| rebooted status-warm | 193.4 ms | 160.7 ms |
| rebooted mount-others-median | 8.8 ms | 8.4 ms |
| rebooted mount-others-max | 13.7 ms | 13.2 ms |
| rebooted unmount-all | 5.59 s | 4.94 s |

## Memory, MiB

| ext4: phase | guest used | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|
| checkout-booted-pinned | 190 | 233 | 0 | 8 | 0 |
| checkout-checked-out | 453 | 2,915 | 0 | 8 | 0 |
| checkout-checked-out-pinned | 284 | 2,915 | 0 | 8 | 0 |
| checkout-status | 371 | 2,916 | 0 | 8 | 0 |
| checkout-read | 362 | 2,925 | 0 | 8 | 0 |
| checkout-read-pinned | 340 | 2,925 | 0 | 8 | 0 |
| reopened-booted-pinned | 192 | 233 | 0 | 8 | 0 |
| reopened-mounted | 193 | 233 | 0 | 8 | 0 |
| reopened-status | 305 | 423 | 0 | 8 | 0 |
| reopened-read | 343 | 2,100 | 0 | 8 | 0 |
| reopened-read-pinned | 267 | 2,100 | 0 | 8 | 0 |

| fxfs: phase | guest used | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|
| checkout-booted-pinned | 178 | 244 | 0 | 18 | 4 |
| checkout-checked-out | 553 | 2,908 | 1 | 937 | 2 |
| checkout-checked-out-pinned | 359 | 2,908 | 1 | 937 | 2 |
| checkout-status | 448 | 2,909 | 1 | 1,108 | 136 |
| checkout-read | 479 | 2,918 | 1 | 2,054 | 1,024 |
| checkout-read-pinned | 420 | 2,918 | 1 | 2,054 | 1,024 |
| reopened-booted-pinned | 189 | 244 | 0 | 2,101 | 1,024 |
| reopened-mounted | 191 | 246 | 0 | 2,101 | 1,024 |
| reopened-status | 341 | 402 | 1 | 2,104 | 1,024 |
| reopened-read | 440 | 2,114 | 1 | 2,128 | 1,024 |
| reopened-read-pinned | 256 | 2,114 | 1 | 2,128 | 1,024 |
| sandbox-0-booted-pinned | 189 | 246 | 0 | 2,128 | 1,024 |
| sandbox-0-mounted | 190 | 248 | 0 | 2,128 | 1,024 |
| sandbox-0-status | 343 | 406 | 1 | 2,130 | 1,022 |
| sandbox-0-read | 438 | 2,134 | 1 | 2,151 | 1,024 |
| sandbox-0-read-pinned | 317 | 2,134 | 1 | 2,151 | 1,024 |
| sandbox-0-written-pinned | 361 | 2,138 | 1 | 2,162 | 1,024 |
| sandbox-1-booted-pinned | 211 | 246 | 0 | 2,161 | 1,024 |
| sandbox-1-mounted | 212 | 248 | 0 | 2,161 | 1,024 |
| sandbox-1-status | 362 | 413 | 1 | 2,162 | 1,024 |
| sandbox-1-read | 437 | 2,125 | 1 | 2,177 | 1,024 |
| sandbox-1-read-pinned | 278 | 2,125 | 1 | 2,177 | 1,024 |
| sandbox-1-written-pinned | 332 | 2,135 | 1 | 2,195 | 1,024 |
| sandbox-2-booted-pinned | 208 | 242 | 0 | 2,195 | 1,024 |
| sandbox-2-mounted | 210 | 244 | 0 | 2,195 | 1,024 |
| sandbox-2-status | 340 | 415 | 1 | 2,196 | 1,024 |
| sandbox-2-read | 441 | 2,134 | 1 | 2,212 | 1,024 |
| sandbox-2-read-pinned | 294 | 2,134 | 1 | 2,212 | 1,024 |
| sandbox-2-written-pinned | 329 | 2,141 | 1 | 2,221 | 1,024 |
| hundreds-booted-pinned | 192 | 245 | 0 | 2,220 | 1,024 |
| hundreds-mounted-pinned | 501 | 545 | 0 | 2,220 | 1,024 |
| hundreds-status | 648 | 712 | 1 | 2,221 | 1,024 |
| hundreds-written-pinned | 537 | 729 | 1 | 2,221 | 1,024 |
| rebooted-booted-pinned | 194 | 246 | 0 | 2,215 | 1,018 |
| rebooted-mounted | 195 | 247 | 0 | 2,215 | 1,018 |
| rebooted-status | 361 | 416 | 0 | 2,215 | 1,018 |
| rebooted-status-pinned | 236 | 426 | 0 | 2,215 | 1,018 |
| rebooted-mounted-all-pinned | 538 | 657 | 0 | 2,215 | 1,018 |

| fxfs-dax: phase | guest used | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|
| checkout-booted-pinned | 181 | 257 | 0 | 23 | 8 |
| checkout-checked-out | 555 | 2,909 | 1 | 959 | 6 |
| checkout-checked-out-pinned | 376 | 2,909 | 1 | 959 | 6 |
| checkout-status | 510 | 2,915 | 6 | 1,120 | 138 |
| checkout-read | 434 | 2,915 | 451 | 1,659 | 1,024 |
| checkout-read-pinned | 379 | 2,915 | 451 | 1,659 | 1,024 |
| reopened-booted-pinned | 177 | 254 | 0 | 2,152 | 1,024 |
| reopened-mounted | 178 | 256 | 0 | 2,152 | 1,024 |
| reopened-status | 363 | 429 | 51 | 2,104 | 1,024 |
| reopened-read | 378 | 511 | 451 | 1,729 | 1,024 |
| reopened-read-pinned | 306 | 511 | 451 | 1,729 | 1,024 |
| sandbox-0-booted-pinned | 188 | 253 | 0 | 2,179 | 1,024 |
| sandbox-0-mounted | 189 | 255 | 0 | 2,179 | 1,024 |
| sandbox-0-status | 361 | 426 | 5 | 2,152 | 1,000 |
| sandbox-0-read | 374 | 512 | 200 | 1,989 | 1,024 |
| sandbox-0-read-pinned | 272 | 512 | 200 | 1,989 | 1,024 |
| sandbox-0-written-pinned | 303 | 522 | 198 | 1,992 | 1,024 |
| sandbox-1-booted-pinned | 192 | 256 | 0 | 2,189 | 1,024 |
| sandbox-1-mounted | 193 | 258 | 0 | 2,189 | 1,024 |
| sandbox-1-status | 371 | 441 | 7 | 2,184 | 1,024 |
| sandbox-1-read | 376 | 530 | 200 | 2,007 | 1,024 |
| sandbox-1-read-pinned | 271 | 530 | 200 | 2,007 | 1,024 |
| sandbox-1-written-pinned | 324 | 539 | 198 | 2,024 | 1,024 |
| sandbox-2-booted-pinned | 196 | 257 | 0 | 2,221 | 1,024 |
| sandbox-2-mounted | 197 | 259 | 0 | 2,221 | 1,024 |
| sandbox-2-status | 362 | 437 | 7 | 2,216 | 1,024 |
| sandbox-2-read | 374 | 540 | 200 | 2,033 | 1,024 |
| sandbox-2-read-pinned | 271 | 540 | 200 | 2,033 | 1,024 |
| sandbox-2-written-pinned | 313 | 547 | 198 | 2,055 | 1,024 |
| hundreds-booted-pinned | 181 | 257 | 0 | 2,252 | 1,024 |
| hundreds-mounted-pinned | 494 | 558 | 0 | 2,253 | 1,024 |
| hundreds-status | 668 | 741 | 7 | 2,248 | 1,024 |
| hundreds-written-pinned | 546 | 751 | 7 | 2,248 | 1,024 |
| rebooted-booted-pinned | 206 | 258 | 0 | 2,252 | 1,022 |
| rebooted-mounted | 207 | 259 | 0 | 2,252 | 1,022 |
| rebooted-status | 370 | 431 | 6 | 2,246 | 1,022 |
| rebooted-status-pinned | 256 | 458 | 6 | 2,246 | 1,022 |
| rebooted-mounted-all-pinned | 552 | 710 | 6 | 2,246 | 1,022 |

## The host's disk, MiB

| ext4: after | bucket | ext4.img | extentd-cache |
|---|---|---|---|
| checkout | 0.0 | 2,195.2 | 0.0 |
| reopened | 0.0 | 2,195.2 | 0.0 |

| fxfs: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 726.9 | 0.1 | 0.2 | 705.9 |  |  |  |  |
| sandbox-0 | 1.6 | 728.6 | 0.1 | 0.2 | 706.3 | 0.1 |  |  |  |
| sandbox-1 | 3.2 | 730.2 | 0.1 | 0.2 | 706.4 | 0.1 | 0.1 |  |  |
| sandbox-2 | 4.9 | 731.8 | 0.1 | 0.2 | 706.6 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.1 | 735.9 | 0.1 | 0.2 | 706.8 | 0.1 | 0.1 | 0.1 | 0.2 |

| fxfs-dax: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 731.6 | 0.1 | 0.2 | 710.5 |  |  |  |  |
| sandbox-0 | 1.6 | 733.3 | 0.1 | 0.2 | 710.8 | 0.1 |  |  |  |
| sandbox-1 | 3.3 | 734.9 | 0.1 | 0.2 | 711.0 | 0.1 | 0.1 |  |  |
| sandbox-2 | 4.9 | 736.5 | 0.1 | 0.2 | 711.2 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.1 | 740.6 | 0.1 | 0.2 | 711.4 | 0.1 | 0.1 | 0.1 | 0.2 |

## What it shows

- **Writing the checkout costs Fxfs 2.3 times what it costs ext4.** Checkout plus sync takes 48.6 s
  and 48.2 s on Fxfs, against 20.9 s on ext4, whose sync takes 4.1 s of that. The clone itself, a
  300 MB pack written and indexed, is even: 11.6-12.7 s. This is the publisher's cost, paid once per
  generation. A sandbox doesn't pay it.
- **A `git status` costs Fxfs 1.2-2.3 times what it costs ext4.** Cold it takes 0.70-1.02 s against
  0.36-0.68 s, and warm 0.15-0.26 s against 0.13-0.18 s. A status after a `sync` costs no more. The
  30-40 s statuses and the 0.3-0.5 s first writes measured in Lima on 2026-11-01 (branch
  t3/fxfs-shared-repo-overlays) don't appear here: they came from that loaded Mac.
- **The overlay adds nothing a status or a write can see.** A sandbox's cold status takes
  0.76-1.13 s, the same as a fresh VM on the publisher's own store (0.83-1.02 s). Its first write to
  a file of the base takes 2.5-9.5 ms with fsync, and its first new file 1.8-2.4 ms.
- **The first open of a shared checkout is about 60 ms:** attach 21-25 ms, import 32-36 ms and mount
  1.5 ms. With 256 already mounted, an import's median is 9 ms and a mount's 0.4 ms. After a reboot,
  a volume mounts in 8-14 ms.
- **Disk per added checkout: about 1.7 MiB.** That is the sandbox's bucket growth (1.6 MiB) and its
  store directory (0.1 MiB), after a status, a full read and two small writes. The 256 imports and
  mounts of `hundreds` added 4.2 MiB. The base itself takes 706-711 MiB in its bucket, compressed
  from 2.1 GB raw (ext4's image holds 2.2 GB), plus as much again in the daemon's part cache.
- **Memory per checkout mounted and idle: 1.2 MiB of guest memory, and as much VM anon.** For 256
  mounted, that is 309-313 MiB of guest memory and 300 MiB of VM anon. Booted again, mounting all 256
  costs 1.16-1.18 MiB each. Unmounting 256 takes 5 s.
- **A status's metadata isn't shared.** A cold status grows the guest by 130-185 MiB and the VM's
  anon by 156-173 MiB, with or without DAX (ext4: 112 MiB and 190 MiB). Each sandbox pays this for
  itself, which confirms doc-10's open question "Metadata across VMs".
- **DAX is what keeps a read checkout out of the VM's memory.** Without DAX, reading the tree once
  grows the VM's anon to 2.1 GB, and it stays at that size after the guest drops its caches. ext4
  does the same. With DAX, anon stays at 0.5 GB and the window maps 0.2-0.45 GB of the daemon's
  shared pages. That is about 1.6 GB of host memory saved per sandbox that reads its checkout.
- **But with a 1 GiB window and 1 GiB of daemon memory, DAX never gets warm on this tree.** A second
  full read takes 26-44 s with DAX, against 0.6 s without, no faster than the first. The tree
  (1.5 GB) is larger than the window and the daemon's arenas (1 GiB each), so a read in order
  evicts each page before it comes round again. A cold read is slow on Fxfs either way: 18-38 s
  against ext4's 12-20 s. That is one reader thread, with what the daemon couldn't hold served from
  its part cache. r872, below, reran it with both larger than the tree.
- **The volume's metadata is small:** 18.9 MB of layers, 1,557-1,560 allocator records and a
  600-byte manifest, with or without DAX. The Lima run's 91,727 records without DAX don't reproduce.
- **Not explained:** after the checkout VM, the daemon's PSS is 937-959 MiB while its arenas hold
  2-6 MiB.

# r872: a window larger than the tree, against ext4 with DAX on virtio-pmem

TASK-227's target, set by the user: Fxfs DAX's warm reread of the tree as fast as ext4 with DAX on
Firecracker's virtio-pmem, or faster. r872 ran all four cases one after the other on one
n2-standard-8 in us-central1-b, a different host from r870's. The source was the same tree plus
`linux-checkout-ext4-pmem`. The settings were `FXDAX_LINUX_CHECKOUT_WINDOW=8589934592` (an 8 GiB
window) and `FXDAX_LINUX_CHECKOUT_EXTENTD_MEMORY=6442450944` (6 GiB of daemon memory). ext4-pmem
uses an 8 GiB image as a writable virtio-pmem device, mounted `dax=always`, which the case checks in
/proc/mounts. The raw output is in [r872/](linux-checkout-2026-11-07/r872/). Each figure is one run.

**The warm reread, the target:**

| | ext4, pmem, DAX | Fxfs, DAX | Fxfs / ext4-pmem | ext4, virtio-blk | Fxfs |
|---|---|---|---|---|---|
| Checkout VM | 729.0 ms | 800.6 ms | 1.10 | 762.7 ms | 857.6 ms |
| Fresh VM | 711.7 ms | 750.8 ms | 1.05 | 766.4 ms | 790.9 ms |
| Sandboxes over the shared base | | 759.3-781.0 ms | 1.07-1.10 | | 802 ms-1.03 s |

Fxfs DAX is 5-10% slower than ext4 on pmem. That is about as far as Fxfs without DAX is from
ext4 on virtio-blk (3-12%), and within the spread between VMs of one case: a warm status varies
by 50-100 ms between VMs. What lies between them is therefore most likely Fxfs's cost per file,
91k opens and their lookups, and not the DAX mapping, which is warm in both. This is inferred from
the non-DAX comparison; no profile was taken. With the window and the daemon's memory larger than
the tree, DAX gets warm: 0.75-0.80 s, against r870's 26-44 s with 1 GiB each.

**Other results:**
- **Cold, Fxfs DAX beats pmem when the daemon holds the base.** A fresh VM's cold read takes
  5.64 s against ext4-pmem's 8.81 s (ext4 on virtio-blk: 20.1 s). A sandbox over the shared base
  takes 34-35 s instead: its reads go through the attached generation, which the daemon's memory
  doesn't hold warm the way it holds the publisher's own store.
- **Host memory per VM after one read is comparable.** ext4-pmem: RSS 2,478 MiB, of it 588 MiB anon
  and the rest the image's pages, shared through the host's page cache. Fxfs DAX: RSS 2,318 MiB, of
  it 626 MiB anon and 843 MiB of the daemon's shared pages.
- Writing the checkout: ext4-pmem 26.8 + 0.5 s, ext4 19.5 + 3.1 s, Fxfs DAX 58.6 + 0.6 s.

### The checkout

|  | ext4 | ext4-pmem | fxfs | fxfs-dax |
|---|---|---|---|---|
| tracked-files | 91,166 files | 91,166 files | 91,166 files | 91,166 files |
| volume-files | 91,192 files | 91,192 files | 91,192 files | 91,192 files |
| volume-directories | 6,059 directories | 6,059 directories | 6,059 directories | 6,059 directories |
| volume-bytes | 1,831,129,545 bytes | 1,831,129,545 bytes | 1,831,129,545 bytes | 1,831,129,545 bytes |
| git-bytes | 295,822,513 bytes | 295,822,513 bytes | 295,822,513 bytes | 295,822,513 bytes |
| read-bytes | 1,535,304,277 bytes | 1,535,304,277 bytes | 1,535,304,277 bytes | 1,535,304,277 bytes |
| manifest |  |  | 600 bytes | 600 bytes |
| volume-layers |  |  | 18,907,136 bytes | 18,907,136 bytes |
| volume-allocator-records |  |  | 1,558 records | 1,557 records |

### Checking it out, and opening it in a fresh VM

|  | ext4 checkout | ext4-pmem checkout | fxfs checkout | fxfs-dax checkout | ext4 reopened | ext4-pmem reopened | fxfs reopened | fxfs-dax reopened |
|---|---|---|---|---|---|---|---|---|
| mkfs | 512.6 ms | 611.5 ms |  |  |  |  |  |  |
| default-mount |  |  | 89.5 ms | 85.9 ms |  |  | 104.3 ms | 102.7 ms |
| mount | 66.6 ms | 23.1 ms |  |  | 84.7 ms | 32.7 ms | 25.6 ms | 25.7 ms |
| clone | 13.98 s | 13.26 s | 13.39 s | 12.98 s |  |  |  |  |
| checkout | 19.49 s | 26.77 s | 59.22 s | 58.60 s |  |  |  |  |
| sync | 3.05 s | 489.3 ms | 871.6 ms | 609.2 ms |  |  |  |  |
| status-cold | 435.8 ms | 417.9 ms | 974.3 ms | 1.03 s | 819.9 ms | 661.7 ms | 933.3 ms | 1.06 s |
| status-warm | 180.1 ms | 182.3 ms | 224.8 ms | 230.4 ms | 286.4 ms | 227.7 ms | 283.4 ms | 210.7 ms |
| status-synced | 175.4 ms | 173.6 ms | 268.8 ms | 264.8 ms | 197.2 ms | 253.8 ms | 272.2 ms | 247.0 ms |
| status-warm-again | 178.0 ms | 173.7 ms | 238.5 ms | 227.3 ms | 255.8 ms | 181.9 ms | 213.1 ms | 273.6 ms |
| read-cold | 13.26 s | 8.66 s | 37.38 s | 15.59 s | 20.13 s | 8.81 s | 43.36 s | 5.64 s |
| read-warm | 762.7 ms | 729.0 ms | 857.6 ms | 800.6 ms | 766.4 ms | 711.7 ms | 790.9 ms | 750.8 ms |
| publish |  |  | 1.42 s | 1.57 s |  |  |  |  |

### Shared, a volume over the published base

|  | fxfs | fxfs-dax |
|---|---|---|
| sandbox attach | 25.5 ms - 32.2 ms | 27.3 ms - 29.9 ms |
| sandbox import | 31.3 ms - 32.8 ms | 33.4 ms - 43.5 ms |
| sandbox mount | 1.9 ms - 2.6 ms | 1.5 ms - 2.8 ms |
| sandbox status-cold | 906.0 ms - 1.10 s | 987.3 ms - 1.24 s |
| sandbox status-warm | 208.8 ms - 291.3 ms | 262.4 ms - 278.1 ms |
| sandbox status-synced | 275.8 ms - 291.4 ms | 246.2 ms - 262.2 ms |
| sandbox status-warm-again | 211.5 ms - 240.1 ms | 201.4 ms - 241.2 ms |
| sandbox read-cold | 41.86 s - 46.35 s | 33.95 s - 34.79 s |
| sandbox read-warm | 802.3 ms - 1.03 s | 759.3 ms - 781.0 ms |
| sandbox first-overwrite | 1.8 ms - 1.9 ms | 2.0 ms - 2.0 ms |
| sandbox first-create | 1.8 ms - 2.1 ms | 2.0 ms - 2.3 ms |
| sandbox status-after-writes | 646.6 ms - 660.3 ms | 566.0 ms - 575.2 ms |
| hundreds import-median | 9.8 ms | 10.2 ms |
| hundreds import-max | 32.3 ms | 32.2 ms |
| hundreds mount-median | 0.6 ms | 0.7 ms |
| hundreds mount-max | 1.2 ms | 1.5 ms |
| hundreds status-cold | 930.1 ms | 1.00 s |
| hundreds status-warm | 216.8 ms | 282.7 ms |
| hundreds status-synced | 248.9 ms | 243.7 ms |
| hundreds first-overwrite | 3.4 ms | 3.8 ms |
| hundreds first-create | 2.1 ms | 2.0 ms |
| hundreds unmount-all | 7.12 s | 7.23 s |
| rebooted default-mount | 122.7 ms | 123.2 ms |
| rebooted mount | 12.1 ms | 11.2 ms |
| rebooted status-cold | 937.4 ms | 980.6 ms |
| rebooted status-warm | 275.7 ms | 258.3 ms |
| rebooted mount-others-median | 10.3 ms | 10.1 ms |
| rebooted mount-others-max | 16.3 ms | 15.0 ms |
| rebooted unmount-all | 7.51 s | 7.26 s |

### Memory, MiB

| ext4: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 186 | 238 | 234 | 0 | 23 | 0 |
| checkout-checked-out | 425 | 2,924 | 2,920 | 0 | 23 | 0 |
| checkout-checked-out-pinned | 271 | 2,924 | 2,920 | 0 | 23 | 0 |
| checkout-status | 383 | 2,926 | 2,923 | 0 | 23 | 0 |
| checkout-read | 376 | 2,933 | 2,930 | 0 | 23 | 0 |
| checkout-read-pinned | 338 | 2,933 | 2,930 | 0 | 23 | 0 |
| reopened-booted-pinned | 191 | 243 | 240 | 0 | 23 | 0 |
| reopened-mounted | 195 | 243 | 240 | 0 | 23 | 0 |
| reopened-status | 314 | 444 | 440 | 0 | 23 | 0 |
| reopened-read | 340 | 2,113 | 2,110 | 0 | 23 | 0 |
| reopened-read-pinned | 218 | 2,113 | 2,110 | 0 | 23 | 0 |

| ext4-pmem: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 317 | 437 | 363 | 0 | 23 | 0 |
| checkout-checked-out | 484 | 3,135 | 976 | 0 | 23 | 0 |
| checkout-checked-out-pinned | 323 | 3,135 | 976 | 0 | 23 | 0 |
| checkout-status | 428 | 3,141 | 982 | 0 | 23 | 0 |
| checkout-read | 385 | 3,151 | 986 | 0 | 23 | 0 |
| checkout-read-pinned | 357 | 3,151 | 986 | 0 | 23 | 0 |
| reopened-booted-pinned | 325 | 368 | 362 | 0 | 23 | 0 |
| reopened-mounted | 322 | 368 | 362 | 0 | 23 | 0 |
| reopened-status | 422 | 651 | 549 | 0 | 23 | 0 |
| reopened-read | 378 | 2,478 | 588 | 0 | 23 | 0 |
| reopened-read-pinned | 352 | 2,478 | 588 | 0 | 23 | 0 |

| fxfs: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 190 | 244 | 241 | 0 | 33 | 4 |
| checkout-checked-out | 526 | 2,896 | 2,891 | 1 | 890 | 2 |
| checkout-checked-out-pinned | 339 | 2,896 | 2,891 | 1 | 890 | 2 |
| checkout-status | 429 | 2,898 | 2,892 | 1 | 1,039 | 126 |
| checkout-read | 464 | 2,904 | 2,899 | 1 | 2,771 | 1,814 |
| checkout-read-pinned | 375 | 2,904 | 2,899 | 1 | 2,771 | 1,814 |
| reopened-booted-pinned | 211 | 249 | 245 | 0 | 2,806 | 1,814 |
| reopened-mounted | 209 | 251 | 247 | 0 | 2,810 | 1,818 |
| reopened-status | 355 | 407 | 402 | 1 | 2,824 | 1,832 |
| reopened-read | 444 | 2,119 | 2,114 | 1 | 2,825 | 1,832 |
| reopened-read-pinned | 307 | 2,119 | 2,114 | 1 | 2,825 | 1,832 |
| sandbox-0-booted-pinned | 199 | 246 | 243 | 0 | 2,824 | 1,832 |
| sandbox-0-mounted | 197 | 249 | 245 | 0 | 2,830 | 1,838 |
| sandbox-0-status | 336 | 405 | 401 | 0 | 2,986 | 1,978 |
| sandbox-0-read | 436 | 2,130 | 2,126 | 1 | 4,697 | 3,666 |
| sandbox-0-read-pinned | 273 | 2,130 | 2,126 | 1 | 4,697 | 3,666 |
| sandbox-0-written-pinned | 316 | 2,141 | 2,137 | 1 | 4,713 | 3,666 |
| sandbox-1-booted-pinned | 202 | 251 | 248 | 0 | 4,712 | 3,666 |
| sandbox-1-mounted | 200 | 253 | 250 | 0 | 4,712 | 3,666 |
| sandbox-1-status | 344 | 420 | 416 | 0 | 4,712 | 3,666 |
| sandbox-1-read | 435 | 2,131 | 2,127 | 0 | 4,712 | 3,666 |
| sandbox-1-read-pinned | 295 | 2,131 | 2,127 | 0 | 4,712 | 3,666 |
| sandbox-1-written-pinned | 324 | 2,141 | 2,138 | 0 | 4,724 | 3,666 |
| sandbox-2-booted-pinned | 193 | 251 | 248 | 0 | 4,724 | 3,666 |
| sandbox-2-mounted | 199 | 253 | 250 | 0 | 4,724 | 3,666 |
| sandbox-2-status | 342 | 421 | 417 | 0 | 4,724 | 3,666 |
| sandbox-2-read | 441 | 2,139 | 2,135 | 0 | 4,726 | 3,666 |
| sandbox-2-read-pinned | 245 | 2,139 | 2,135 | 0 | 4,726 | 3,666 |
| sandbox-2-written-pinned | 263 | 2,146 | 2,141 | 0 | 4,740 | 3,666 |
| hundreds-booted-pinned | 197 | 249 | 245 | 0 | 4,739 | 3,666 |
| hundreds-mounted-pinned | 506 | 547 | 544 | 0 | 4,739 | 3,666 |
| hundreds-status | 662 | 722 | 718 | 0 | 4,740 | 3,666 |
| hundreds-written-pinned | 533 | 727 | 723 | 0 | 4,740 | 3,666 |
| rebooted-booted-pinned | 195 | 255 | 251 | 0 | 4,739 | 3,666 |
| rebooted-mounted | 201 | 256 | 252 | 0 | 4,739 | 3,666 |
| rebooted-status | 348 | 424 | 420 | 0 | 4,740 | 3,666 |
| rebooted-status-pinned | 245 | 453 | 449 | 0 | 4,740 | 3,666 |
| rebooted-mounted-all-pinned | 549 | 658 | 654 | 0 | 4,740 | 3,666 |

| fxfs-dax: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 187 | 343 | 340 | 0 | 37 | 8 |
| checkout-checked-out | 512 | 3,009 | 3,004 | 1 | 968 | 6 |
| checkout-checked-out-pinned | 313 | 3,009 | 3,004 | 1 | 968 | 6 |
| checkout-status | 434 | 3,022 | 3,007 | 6 | 1,113 | 136 |
| checkout-read | 434 | 4,696 | 3,009 | 842 | 2,002 | 1,830 |
| checkout-read-pinned | 443 | 4,696 | 3,009 | 842 | 2,002 | 1,830 |
| reopened-booted-pinned | 207 | 352 | 348 | 0 | 2,847 | 1,830 |
| reopened-mounted | 213 | 354 | 350 | 0 | 2,851 | 1,834 |
| reopened-status | 383 | 695 | 532 | 80 | 2,786 | 1,848 |
| reopened-read | 419 | 2,318 | 626 | 843 | 2,023 | 1,848 |
| reopened-read-pinned | 372 | 2,318 | 626 | 843 | 2,023 | 1,848 |
| sandbox-0-booted-pinned | 191 | 342 | 338 | 0 | 2,865 | 1,848 |
| sandbox-0-mounted | 191 | 344 | 341 | 0 | 2,867 | 1,850 |
| sandbox-0-status | 365 | 541 | 527 | 5 | 3,007 | 1,994 |
| sandbox-0-read | 412 | 2,323 | 638 | 841 | 3,888 | 3,688 |
| sandbox-0-read-pinned | 356 | 2,323 | 638 | 841 | 3,888 | 3,688 |
| sandbox-0-written-pinned | 394 | 2,328 | 643 | 841 | 3,904 | 3,688 |
| sandbox-1-booted-pinned | 206 | 342 | 338 | 0 | 4,744 | 3,688 |
| sandbox-1-mounted | 205 | 344 | 340 | 0 | 4,744 | 3,688 |
| sandbox-1-status | 361 | 530 | 513 | 7 | 4,738 | 3,688 |
| sandbox-1-read | 410 | 2,333 | 645 | 842 | 3,908 | 3,688 |
| sandbox-1-read-pinned | 351 | 2,333 | 645 | 842 | 3,908 | 3,688 |
| sandbox-1-written-pinned | 393 | 2,338 | 650 | 842 | 3,912 | 3,688 |
| sandbox-2-booted-pinned | 187 | 346 | 343 | 0 | 4,754 | 3,688 |
| sandbox-2-mounted | 196 | 349 | 345 | 0 | 4,755 | 3,688 |
| sandbox-2-status | 372 | 543 | 526 | 7 | 4,748 | 3,688 |
| sandbox-2-read | 411 | 2,337 | 649 | 842 | 3,915 | 3,688 |
| sandbox-2-read-pinned | 352 | 2,337 | 649 | 842 | 3,915 | 3,688 |
| sandbox-2-written-pinned | 400 | 2,344 | 657 | 842 | 3,925 | 3,688 |
| hundreds-booted-pinned | 199 | 341 | 338 | 0 | 4,767 | 3,688 |
| hundreds-mounted-pinned | 502 | 643 | 640 | 0 | 4,769 | 3,688 |
| hundreds-status | 674 | 833 | 816 | 7 | 4,762 | 3,688 |
| hundreds-written-pinned | 522 | 852 | 834 | 7 | 4,762 | 3,688 |
| rebooted-booted-pinned | 198 | 349 | 345 | 0 | 4,769 | 3,688 |
| rebooted-mounted | 196 | 350 | 346 | 0 | 4,769 | 3,688 |
| rebooted-status | 359 | 543 | 527 | 7 | 4,762 | 3,688 |
| rebooted-status-pinned | 256 | 550 | 533 | 7 | 4,762 | 3,688 |
| rebooted-mounted-all-pinned | 545 | 802 | 785 | 7 | 4,762 | 3,688 |

### The host's disk, MiB

| ext4: after | bucket | ext4.img | extentd-cache |
|---|---|---|---|
| checkout | 0.0 | 2,195.0 | 0.0 |
| reopened | 0.0 | 2,195.0 | 0.0 |

| ext4-pmem: after | bucket | ext4.img | extentd-cache |
|---|---|---|---|
| checkout | 0.0 | 2,161.9 | 0.0 |
| reopened | 0.0 | 2,231.1 | 0.0 |

| fxfs: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 722.3 | 0.1 | 0.2 | 700.6 |  |  |  |  |
| sandbox-0 | 1.6 | 724.0 | 0.1 | 0.2 | 701.0 | 0.1 |  |  |  |
| sandbox-1 | 3.2 | 725.6 | 0.1 | 0.2 | 701.1 | 0.1 | 0.1 |  |  |
| sandbox-2 | 4.9 | 727.2 | 0.1 | 0.2 | 701.3 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.0 | 731.3 | 0.1 | 0.2 | 701.5 | 0.1 | 0.1 | 0.1 | 0.2 |

| fxfs-dax: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 722.0 | 0.1 | 0.2 | 699.6 |  |  |  |  |
| sandbox-0 | 1.6 | 723.6 | 0.1 | 0.2 | 699.9 | 0.1 |  |  |  |
| sandbox-1 | 3.3 | 725.2 | 0.1 | 0.2 | 700.1 | 0.1 | 0.1 |  |  |
| sandbox-2 | 4.9 | 726.8 | 0.1 | 0.2 | 700.2 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.1 | 730.9 | 0.1 | 0.2 | 700.4 | 0.1 | 0.1 | 0.1 | 0.2 |
