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
  its part cache. The rerun with a 4 GiB window and 6 GiB of daemon memory (r872,
  `FXDAX_LINUX_CHECKOUT_WINDOW`, `FXDAX_LINUX_CHECKOUT_EXTENTD_MEMORY`) is still to be done: gcloud's
  credentials expired before it ran.
- **The volume's metadata is small:** 18.9 MB of layers, 1,557-1,560 allocator records and a
  600-byte manifest, with or without DAX. The Lima run's 91,727 records without DAX don't reproduce.
- **Not explained:** after the checkout VM, the daemon's PSS is 937-959 MiB while its arenas hold
  2-6 MiB.
