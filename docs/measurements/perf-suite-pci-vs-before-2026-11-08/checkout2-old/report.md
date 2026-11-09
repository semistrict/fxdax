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
| volume-allocator-records |  | 1,559 records |

## Checking it out, and opening it in a fresh VM

|  | ext4 checkout | fxfs checkout | ext4 reopened | fxfs reopened |
|---|---|---|---|---|
| mkfs | 431.4 ms |  |  |  |
| default-mount |  | 58.2 ms |  | 70.4 ms |
| mount | 48.8 ms |  | 50.6 ms | 19.1 ms |
| clone | 10.41 s | 10.07 s |  |  |
| checkout | 15.99 s | 37.05 s |  |  |
| sync | 2.04 s | 447.5 ms |  |  |
| status-cold | 317.1 ms | 751.2 ms | 536.3 ms | 669.6 ms |
| status-warm | 129.4 ms | 112.5 ms | 159.7 ms | 109.6 ms |
| status-synced | 126.1 ms | 129.9 ms | 159.5 ms | 174.8 ms |
| status-warm-again | 128.6 ms | 101.4 ms | 139.0 ms | 100.5 ms |
| read-cold | 7.54 s | 21.45 s | 13.32 s | 24.05 s |
| read-warm | 571.9 ms | 536.6 ms | 570.5 ms | 572.4 ms |
| publish |  | 1.22 s |  |  |

## Shared, a volume over the published base

|  | fxfs |
|---|---|
| sandbox attach | 16.6 ms - 18.8 ms |
| sandbox import | 19.7 ms - 20.6 ms |
| sandbox mount | 0.9 ms - 1.2 ms |
| sandbox status-cold | 676.2 ms - 737.6 ms |
| sandbox status-warm | 107.2 ms - 143.4 ms |
| sandbox status-synced | 164.0 ms - 179.9 ms |
| sandbox status-warm-again | 104.6 ms - 143.7 ms |
| sandbox read-cold | 23.74 s - 24.20 s |
| sandbox read-warm | 538.4 ms - 559.4 ms |
| sandbox first-overwrite | 1.1 ms - 1.2 ms |
| sandbox first-create | 1.2 ms - 1.7 ms |
| sandbox status-after-writes | 468.7 ms - 486.6 ms |
| hundreds import-median | 6.4 ms |
| hundreds import-max | 19.6 ms |
| hundreds mount-median | 0.3 ms |
| hundreds mount-max | 0.8 ms |
| hundreds status-cold | 672.1 ms |
| hundreds status-warm | 151.7 ms |
| hundreds status-synced | 132.3 ms |
| hundreds first-overwrite | 2.1 ms |
| hundreds first-create | 1.6 ms |
| hundreds unmount-all | 3.62 s |
| rebooted default-mount | 89.8 ms |
| rebooted mount | 7.8 ms |
| rebooted status-cold | 674.0 ms |
| rebooted status-warm | 113.9 ms |
| rebooted mount-others-median | 6.2 ms |
| rebooted mount-others-max | 9.7 ms |
| rebooted unmount-all | 3.97 s |

## Memory, MiB

| ext4: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 189 | 244 | 241 | 0 | 29 | 0 |
| checkout-checked-out | 426 | 2,912 | 2,908 | 0 | 29 | 0 |
| checkout-checked-out-pinned | 288 | 2,912 | 2,908 | 0 | 29 | 0 |
| checkout-status | 387 | 2,914 | 2,911 | 0 | 29 | 0 |
| checkout-read | 367 | 2,919 | 2,916 | 0 | 29 | 0 |
| checkout-read-pinned | 299 | 2,919 | 2,916 | 0 | 29 | 0 |
| reopened-booted-pinned | 184 | 241 | 238 | 0 | 29 | 0 |
| reopened-mounted | 184 | 241 | 238 | 0 | 29 | 0 |
| reopened-status | 311 | 443 | 439 | 0 | 29 | 0 |
| reopened-read | 344 | 2,108 | 2,105 | 0 | 29 | 0 |
| reopened-read-pinned | 303 | 2,108 | 2,105 | 0 | 29 | 0 |

| fxfs: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 210 | 248 | 245 | 0 | 40 | 4 |
| checkout-checked-out | 538 | 2,925 | 2,920 | 1 | 1,030 | 2 |
| checkout-checked-out-pinned | 384 | 2,925 | 2,920 | 1 | 1,030 | 2 |
| checkout-status | 499 | 2,928 | 2,923 | 1 | 1,196 | 136 |
| checkout-read | 490 | 2,937 | 2,932 | 1 | 2,917 | 1,830 |
| checkout-read-pinned | 422 | 2,937 | 2,932 | 1 | 2,917 | 1,830 |
| reopened-booted-pinned | 203 | 255 | 251 | 0 | 2,946 | 1,830 |
| reopened-mounted | 204 | 257 | 253 | 0 | 2,949 | 1,832 |
| reopened-status | 362 | 423 | 418 | 1 | 2,965 | 1,848 |
| reopened-read | 450 | 2,125 | 2,120 | 1 | 2,964 | 1,848 |
| reopened-read-pinned | 310 | 2,125 | 2,120 | 1 | 2,964 | 1,848 |
| sandbox-0-booted-pinned | 183 | 252 | 249 | 0 | 2,963 | 1,848 |
| sandbox-0-mounted | 185 | 255 | 251 | 0 | 2,969 | 1,852 |
| sandbox-0-status | 351 | 430 | 425 | 1 | 3,015 | 1,898 |
| sandbox-0-read | 439 | 2,140 | 2,136 | 1 | 2,977 | 1,860 |
| sandbox-0-read-pinned | 322 | 2,140 | 2,136 | 1 | 2,977 | 1,860 |
| sandbox-0-written-pinned | 348 | 2,148 | 2,143 | 1 | 2,977 | 1,860 |
| sandbox-1-booted-pinned | 188 | 256 | 253 | 0 | 2,993 | 1,860 |
| sandbox-1-mounted | 194 | 258 | 255 | 0 | 2,993 | 1,860 |
| sandbox-1-status | 363 | 432 | 428 | 0 | 2,993 | 1,860 |
| sandbox-1-read | 442 | 2,146 | 2,142 | 0 | 2,994 | 1,860 |
| sandbox-1-read-pinned | 326 | 2,146 | 2,142 | 0 | 2,994 | 1,860 |
| sandbox-1-written-pinned | 366 | 2,151 | 2,147 | 0 | 2,994 | 1,860 |
| sandbox-2-booted-pinned | 198 | 258 | 254 | 0 | 3,011 | 1,860 |
| sandbox-2-mounted | 203 | 260 | 256 | 0 | 3,011 | 1,860 |
| sandbox-2-status | 343 | 428 | 424 | 0 | 3,012 | 1,860 |
| sandbox-2-read | 451 | 2,150 | 2,146 | 0 | 3,012 | 1,860 |
| sandbox-2-read-pinned | 329 | 2,150 | 2,146 | 0 | 3,012 | 1,860 |
| sandbox-2-written-pinned | 371 | 2,157 | 2,153 | 0 | 3,012 | 1,860 |
| hundreds-booted-pinned | 192 | 251 | 248 | 0 | 3,029 | 1,860 |
| hundreds-mounted-pinned | 507 | 559 | 556 | 0 | 3,029 | 1,860 |
| hundreds-status | 676 | 729 | 725 | 0 | 3,029 | 1,860 |
| hundreds-written-pinned | 526 | 750 | 746 | 0 | 3,029 | 1,860 |
| rebooted-booted-pinned | 197 | 256 | 253 | 0 | 3,029 | 1,860 |
| rebooted-mounted | 198 | 257 | 254 | 0 | 3,029 | 1,860 |
| rebooted-status | 348 | 415 | 411 | 0 | 3,029 | 1,860 |
| rebooted-status-pinned | 225 | 442 | 438 | 0 | 3,029 | 1,860 |
| rebooted-mounted-all-pinned | 512 | 624 | 620 | 0 | 3,029 | 1,860 |

## The host's disk, MiB

| ext4: after | bucket | ext4.img | extentd-cache |
|---|---|---|---|
| checkout | 0.0 | 2,195.0 | 0.0 |
| reopened | 0.0 | 2,195.0 | 0.0 |

| fxfs: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 735.8 | 0.1 | 0.1 | 713.8 |  |  |  |  |
| sandbox-0 | 1.7 | 737.5 | 0.1 | 0.1 | 714.1 | 0.1 |  |  |  |
| sandbox-1 | 3.3 | 739.2 | 0.1 | 0.1 | 714.3 | 0.1 | 0.1 |  |  |
| sandbox-2 | 5.0 | 740.8 | 0.1 | 0.1 | 714.5 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.2 | 744.9 | 0.1 | 0.1 | 714.7 | 0.1 | 0.1 | 0.1 | 0.1 |
