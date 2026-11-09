## The checkout

|  | ext4 | fxfs |
|---|---|---|
| tracked-files | 91,166 files | 91,166 files |
| volume-files | 91,192 files | 91,192 files |
| volume-directories | 6,059 directories | 6,059 directories |
| volume-bytes | 1,831,129,545 bytes | 1,831,129,545 bytes |
| git-bytes | 295,822,513 bytes | 295,822,513 bytes |
| read-bytes | 1,535,304,277 bytes | 1,535,304,277 bytes |
| manifest |  | 600 bytes |
| volume-layers |  | 18,898,944 bytes |
| volume-allocator-records |  | 1,560 records |

## Checking it out, and opening it in a fresh VM

|  | ext4 checkout | fxfs checkout | ext4 reopened | fxfs reopened |
|---|---|---|---|---|
| mkfs | 365.6 ms |  |  |  |
| default-mount |  | 57.7 ms |  | 67.1 ms |
| mount | 27.9 ms |  | 32.3 ms | 19.1 ms |
| clone | 10.57 s | 10.06 s |  |  |
| checkout | 15.42 s | 34.34 s |  |  |
| sync | 1.88 s | 516.9 ms |  |  |
| status-cold | 296.0 ms | 741.5 ms | 468.0 ms | 685.3 ms |
| status-warm | 139.0 ms | 106.5 ms | 142.6 ms | 117.4 ms |
| status-synced | 134.7 ms | 137.6 ms | 146.9 ms | 171.8 ms |
| status-warm-again | 138.9 ms | 101.8 ms | 140.0 ms | 149.9 ms |
| read-cold | 4.09 s | 16.98 s | 10.41 s | 20.65 s |
| read-warm | 592.5 ms | 547.1 ms | 577.1 ms | 627.1 ms |
| publish |  | 1.24 s |  |  |

## Shared, a volume over the published base

|  | fxfs |
|---|---|
| sandbox attach | 17.0 ms - 19.4 ms |
| sandbox import | 19.6 ms - 19.9 ms |
| sandbox mount | 0.5 ms - 1.0 ms |
| sandbox status-cold | 673.0 ms - 724.7 ms |
| sandbox status-warm | 114.5 ms - 157.4 ms |
| sandbox status-synced | 141.6 ms - 197.4 ms |
| sandbox status-warm-again | 113.6 ms - 152.3 ms |
| sandbox read-cold | 20.09 s - 20.70 s |
| sandbox read-warm | 542.8 ms - 562.6 ms |
| sandbox first-overwrite | 1.0 ms - 1.2 ms |
| sandbox first-create | 1.3 ms - 2.8 ms |
| sandbox status-after-writes | 472.7 ms - 479.5 ms |
| hundreds import-median | 5.9 ms |
| hundreds import-max | 20.5 ms |
| hundreds mount-median | 0.3 ms |
| hundreds mount-max | 0.8 ms |
| hundreds status-cold | 682.8 ms |
| hundreds status-warm | 145.6 ms |
| hundreds status-synced | 147.9 ms |
| hundreds first-overwrite | 1.5 ms |
| hundreds first-create | 1.3 ms |
| hundreds unmount-all | 3.80 s |
| rebooted default-mount | 85.2 ms |
| rebooted mount | 7.5 ms |
| rebooted status-cold | 674.2 ms |
| rebooted status-warm | 119.9 ms |
| rebooted mount-others-median | 6.0 ms |
| rebooted mount-others-max | 9.4 ms |
| rebooted unmount-all | 3.69 s |

## Memory, MiB

| ext4: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 195 | 243 | 240 | 0 | 29 | 0 |
| checkout-checked-out | 430 | 2,923 | 2,920 | 0 | 29 | 0 |
| checkout-checked-out-pinned | 314 | 2,923 | 2,920 | 0 | 29 | 0 |
| checkout-status | 412 | 2,925 | 2,921 | 0 | 29 | 0 |
| checkout-read | 376 | 2,938 | 2,935 | 0 | 29 | 0 |
| checkout-read-pinned | 356 | 2,938 | 2,935 | 0 | 29 | 0 |
| reopened-booted-pinned | 184 | 243 | 239 | 0 | 29 | 0 |
| reopened-mounted | 184 | 243 | 239 | 0 | 29 | 0 |
| reopened-status | 325 | 431 | 428 | 0 | 29 | 0 |
| reopened-read | 332 | 2,102 | 2,099 | 0 | 29 | 0 |
| reopened-read-pinned | 254 | 2,102 | 2,099 | 0 | 29 | 0 |

| fxfs: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 187 | 252 | 249 | 0 | 40 | 4 |
| checkout-checked-out | 565 | 2,917 | 2,912 | 1 | 1,125 | 2 |
| checkout-checked-out-pinned | 352 | 2,917 | 2,912 | 1 | 1,125 | 2 |
| checkout-status | 473 | 2,919 | 2,914 | 1 | 1,291 | 130 |
| checkout-read | 476 | 2,924 | 2,918 | 1 | 2,990 | 1,816 |
| checkout-read-pinned | 365 | 2,924 | 2,918 | 1 | 2,990 | 1,816 |
| reopened-booted-pinned | 187 | 255 | 252 | 0 | 3,049 | 1,816 |
| reopened-mounted | 188 | 257 | 254 | 0 | 3,054 | 1,820 |
| reopened-status | 348 | 433 | 429 | 1 | 3,068 | 1,834 |
| reopened-read | 453 | 2,134 | 2,129 | 1 | 3,066 | 1,834 |
| reopened-read-pinned | 369 | 2,134 | 2,129 | 1 | 3,066 | 1,834 |
| sandbox-0-booted-pinned | 197 | 257 | 254 | 0 | 3,065 | 1,834 |
| sandbox-0-mounted | 199 | 259 | 256 | 0 | 3,070 | 1,838 |
| sandbox-0-status | 349 | 425 | 421 | 0 | 3,116 | 1,884 |
| sandbox-0-read | 434 | 2,144 | 2,140 | 0 | 3,087 | 1,854 |
| sandbox-0-read-pinned | 312 | 2,144 | 2,140 | 0 | 3,087 | 1,854 |
| sandbox-0-written-pinned | 343 | 2,149 | 2,145 | 0 | 3,087 | 1,854 |
| sandbox-1-booted-pinned | 184 | 250 | 246 | 0 | 3,099 | 1,854 |
| sandbox-1-mounted | 186 | 252 | 248 | 0 | 3,099 | 1,854 |
| sandbox-1-status | 367 | 433 | 429 | 0 | 3,099 | 1,854 |
| sandbox-1-read | 451 | 2,147 | 2,143 | 0 | 3,099 | 1,854 |
| sandbox-1-read-pinned | 386 | 2,147 | 2,143 | 0 | 3,099 | 1,854 |
| sandbox-1-written-pinned | 390 | 2,158 | 2,153 | 0 | 3,099 | 1,854 |
| sandbox-2-booted-pinned | 185 | 254 | 251 | 0 | 3,113 | 1,854 |
| sandbox-2-mounted | 186 | 256 | 253 | 0 | 3,113 | 1,854 |
| sandbox-2-status | 372 | 429 | 425 | 0 | 3,114 | 1,854 |
| sandbox-2-read | 447 | 2,143 | 2,139 | 0 | 3,114 | 1,854 |
| sandbox-2-read-pinned | 356 | 2,143 | 2,139 | 0 | 3,114 | 1,854 |
| sandbox-2-written-pinned | 355 | 2,149 | 2,144 | 0 | 3,114 | 1,854 |
| hundreds-booted-pinned | 195 | 258 | 255 | 0 | 3,129 | 1,854 |
| hundreds-mounted-pinned | 511 | 556 | 553 | 0 | 3,129 | 1,854 |
| hundreds-status | 653 | 724 | 720 | 0 | 3,129 | 1,854 |
| hundreds-written-pinned | 519 | 739 | 735 | 0 | 3,129 | 1,854 |
| rebooted-booted-pinned | 200 | 259 | 256 | 0 | 3,129 | 1,854 |
| rebooted-mounted | 201 | 260 | 257 | 0 | 3,129 | 1,854 |
| rebooted-status | 355 | 435 | 431 | 0 | 3,129 | 1,854 |
| rebooted-status-pinned | 245 | 445 | 441 | 0 | 3,129 | 1,854 |
| rebooted-mounted-all-pinned | 533 | 670 | 666 | 0 | 3,129 | 1,854 |

## The host's disk, MiB

| ext4: after | bucket | ext4.img | extentd-cache |
|---|---|---|---|
| checkout | 0.0 | 2,185.7 | 0.0 |
| reopened | 0.0 | 2,185.7 | 0.0 |

| fxfs: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 741.8 | 0.1 | 0.1 | 720.1 |  |  |  |  |
| sandbox-0 | 1.7 | 743.5 | 0.1 | 0.1 | 720.5 | 0.1 |  |  |  |
| sandbox-1 | 3.3 | 745.1 | 0.1 | 0.1 | 720.7 | 0.1 | 0.1 |  |  |
| sandbox-2 | 4.9 | 746.7 | 0.1 | 0.1 | 720.8 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.1 | 750.8 | 0.1 | 0.1 | 721.0 | 0.1 | 0.1 | 0.1 | 0.1 |
