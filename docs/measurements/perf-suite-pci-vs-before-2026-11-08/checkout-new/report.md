## The checkout

|  | ext4 | fxfs | fxfs-dax |
|---|---|---|---|
| tracked-files | 91,166 files | 91,166 files | 91,166 files |
| volume-files | 91,192 files | 91,192 files | 91,192 files |
| volume-directories | 6,059 directories | 6,059 directories | 6,059 directories |
| volume-bytes | 1,831,129,545 bytes | 1,831,129,545 bytes | 1,831,129,545 bytes |
| git-bytes | 295,822,513 bytes | 295,822,513 bytes | 295,822,513 bytes |
| read-bytes | 1,535,304,277 bytes | 1,535,304,277 bytes | 1,535,304,277 bytes |
| manifest |  | 600 bytes | 600 bytes |
| volume-layers |  | 18,894,848 bytes | 18,907,136 bytes |
| volume-allocator-records |  | 1,558 records | 1,558 records |

## Checking it out, and opening it in a fresh VM

|  | ext4 checkout | fxfs checkout | fxfs-dax checkout | ext4 reopened | fxfs reopened | fxfs-dax reopened |
|---|---|---|---|---|---|---|
| mkfs | 376.5 ms |  |  |  |  |  |
| default-mount |  | 57.1 ms | 58.2 ms |  | 70.0 ms | 70.7 ms |
| mount | 24.6 ms |  |  | 33.1 ms | 17.7 ms | 15.8 ms |
| clone | 10.11 s | 10.07 s | 9.94 s |  |  |  |
| checkout | 13.13 s | 34.46 s | 33.32 s |  |  |  |
| sync | 4.58 s | 461.8 ms | 475.2 ms |  |  |  |
| status-cold | 290.2 ms | 707.6 ms | 777.6 ms | 482.4 ms | 680.0 ms | 770.4 ms |
| status-warm | 148.4 ms | 105.6 ms | 106.5 ms | 186.3 ms | 125.2 ms | 153.9 ms |
| status-synced | 133.4 ms | 131.3 ms | 138.2 ms | 191.8 ms | 175.9 ms | 137.6 ms |
| status-warm-again | 127.6 ms | 103.6 ms | 102.8 ms | 187.3 ms | 107.3 ms | 130.6 ms |
| read-cold | 4.94 s | 16.85 s | 10.19 s | 9.45 s | 19.91 s | 4.22 s |
| read-warm | 596.2 ms | 544.9 ms | 491.1 ms | 567.0 ms | 841.6 ms | 489.0 ms |
| publish |  | 1.18 s | 1.19 s |  |  |  |

## Shared, a volume over the published base

|  | fxfs | fxfs-dax |
|---|---|---|
| sandbox attach | 17.4 ms - 19.8 ms | 17.6 ms - 19.1 ms |
| sandbox import | 18.3 ms - 21.4 ms | 18.7 ms - 20.5 ms |
| sandbox mount | 0.4 ms - 1.2 ms | 0.7 ms - 1.6 ms |
| sandbox status-cold | 657.1 ms - 725.4 ms | 771.8 ms - 830.6 ms |
| sandbox status-warm | 114.8 ms - 122.8 ms | 149.2 ms - 163.1 ms |
| sandbox status-synced | 161.9 ms - 190.3 ms | 129.7 ms - 174.9 ms |
| sandbox status-warm-again | 103.0 ms - 137.5 ms | 110.6 ms - 143.2 ms |
| sandbox read-cold | 19.79 s - 20.72 s | 4.98 s - 5.51 s |
| sandbox read-warm | 545.4 ms - 576.5 ms | 488.3 ms - 490.7 ms |
| sandbox first-overwrite | 1.0 ms - 1.0 ms | 2.4 ms - 2.6 ms |
| sandbox first-create | 1.3 ms - 1.5 ms | 0.6 ms - 1.5 ms |
| sandbox status-after-writes | 466.4 ms - 467.3 ms | 433.9 ms - 451.8 ms |
| hundreds import-median | 5.9 ms | 5.9 ms |
| hundreds import-max | 18.4 ms | 19.4 ms |
| hundreds mount-median | 0.3 ms | 0.3 ms |
| hundreds mount-max | 0.9 ms | 0.8 ms |
| hundreds status-cold | 665.7 ms | 774.9 ms |
| hundreds status-warm | 144.8 ms | 147.7 ms |
| hundreds status-synced | 139.7 ms | 137.3 ms |
| hundreds first-overwrite | 2.0 ms | 2.3 ms |
| hundreds first-create | 1.4 ms | 1.7 ms |
| hundreds unmount-all | 3.66 s | 3.78 s |
| rebooted default-mount | 84.8 ms | 83.3 ms |
| rebooted mount | 6.9 ms | 7.2 ms |
| rebooted status-cold | 665.8 ms | 775.2 ms |
| rebooted status-warm | 116.8 ms | 167.2 ms |
| rebooted mount-others-median | 6.0 ms | 6.0 ms |
| rebooted mount-others-max | 10.0 ms | 9.1 ms |
| rebooted unmount-all | 5.19 s | 3.73 s |

## Memory, MiB

| ext4: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 175 | 242 | 239 | 0 | 29 | 0 |
| checkout-checked-out | 415 | 2,914 | 2,911 | 0 | 29 | 0 |
| checkout-checked-out-pinned | 291 | 2,914 | 2,911 | 0 | 29 | 0 |
| checkout-status | 402 | 2,919 | 2,915 | 0 | 29 | 0 |
| checkout-read | 379 | 2,940 | 2,937 | 0 | 29 | 0 |
| checkout-read-pinned | 362 | 2,940 | 2,937 | 0 | 29 | 0 |
| reopened-booted-pinned | 178 | 245 | 241 | 0 | 29 | 0 |
| reopened-mounted | 178 | 245 | 241 | 0 | 29 | 0 |
| reopened-status | 326 | 438 | 435 | 0 | 29 | 0 |
| reopened-read | 334 | 2,115 | 2,112 | 0 | 29 | 0 |
| reopened-read-pinned | 282 | 2,116 | 2,112 | 0 | 29 | 0 |

| fxfs: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 208 | 253 | 250 | 0 | 40 | 4 |
| checkout-checked-out | 537 | 2,920 | 2,914 | 1 | 1,066 | 6 |
| checkout-checked-out-pinned | 328 | 2,920 | 2,914 | 1 | 1,066 | 6 |
| checkout-status | 459 | 2,925 | 2,920 | 1 | 1,234 | 144 |
| checkout-read | 470 | 2,932 | 2,927 | 1 | 2,941 | 1,830 |
| checkout-read-pinned | 367 | 2,932 | 2,927 | 1 | 2,941 | 1,830 |
| reopened-booted-pinned | 195 | 253 | 249 | 0 | 2,980 | 1,830 |
| reopened-mounted | 196 | 255 | 251 | 0 | 2,982 | 1,832 |
| reopened-status | 369 | 432 | 428 | 1 | 2,996 | 1,846 |
| reopened-read | 446 | 2,126 | 2,121 | 1 | 2,996 | 1,846 |
| reopened-read-pinned | 303 | 2,126 | 2,121 | 1 | 2,996 | 1,846 |
| sandbox-0-booted-pinned | 195 | 258 | 255 | 0 | 2,995 | 1,846 |
| sandbox-0-mounted | 200 | 261 | 257 | 0 | 3,002 | 1,852 |
| sandbox-0-status | 379 | 432 | 428 | 1 | 3,042 | 1,892 |
| sandbox-0-read | 438 | 2,141 | 2,137 | 1 | 3,006 | 1,854 |
| sandbox-0-read-pinned | 294 | 2,141 | 2,137 | 1 | 3,006 | 1,854 |
| sandbox-0-written-pinned | 315 | 2,151 | 2,147 | 1 | 3,006 | 1,854 |
| sandbox-1-booted-pinned | 183 | 252 | 248 | 0 | 3,009 | 1,854 |
| sandbox-1-mounted | 185 | 254 | 250 | 0 | 3,009 | 1,854 |
| sandbox-1-status | 355 | 422 | 419 | 0 | 3,009 | 1,854 |
| sandbox-1-read | 439 | 2,141 | 2,137 | 0 | 3,009 | 1,854 |
| sandbox-1-read-pinned | 324 | 2,141 | 2,137 | 0 | 3,009 | 1,854 |
| sandbox-1-written-pinned | 337 | 2,149 | 2,145 | 0 | 3,009 | 1,854 |
| sandbox-2-booted-pinned | 184 | 253 | 249 | 0 | 3,009 | 1,854 |
| sandbox-2-mounted | 185 | 255 | 251 | 0 | 3,009 | 1,854 |
| sandbox-2-status | 330 | 413 | 409 | 0 | 3,009 | 1,854 |
| sandbox-2-read | 442 | 2,144 | 2,141 | 0 | 3,010 | 1,854 |
| sandbox-2-read-pinned | 321 | 2,144 | 2,141 | 0 | 3,010 | 1,854 |
| sandbox-2-written-pinned | 341 | 2,146 | 2,142 | 0 | 3,010 | 1,854 |
| hundreds-booted-pinned | 190 | 255 | 251 | 0 | 3,025 | 1,854 |
| hundreds-mounted-pinned | 509 | 552 | 549 | 0 | 3,025 | 1,854 |
| hundreds-status | 649 | 736 | 732 | 0 | 3,026 | 1,854 |
| hundreds-written-pinned | 536 | 742 | 738 | 0 | 3,026 | 1,854 |
| rebooted-booted-pinned | 187 | 257 | 253 | 0 | 3,026 | 1,854 |
| rebooted-mounted | 188 | 258 | 254 | 0 | 3,026 | 1,854 |
| rebooted-status | 375 | 434 | 430 | 0 | 3,026 | 1,854 |
| rebooted-status-pinned | 261 | 447 | 443 | 0 | 3,026 | 1,854 |
| rebooted-mounted-all-pinned | 560 | 677 | 673 | 0 | 3,026 | 1,854 |

| fxfs-dax: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 220 | 258 | 255 | 0 | 44 | 8 |
| checkout-checked-out | 567 | 2,916 | 2,911 | 1 | 1,116 | 10 |
| checkout-checked-out-pinned | 379 | 2,916 | 2,911 | 1 | 1,116 | 10 |
| checkout-status | 511 | 2,938 | 2,923 | 6 | 1,265 | 136 |
| checkout-read | 470 | 4,630 | 2,942 | 842 | 2,138 | 1,824 |
| checkout-read-pinned | 455 | 4,630 | 2,942 | 842 | 2,138 | 1,824 |
| reopened-booted-pinned | 200 | 260 | 257 | 0 | 3,004 | 1,824 |
| reopened-mounted | 202 | 261 | 258 | 0 | 3,006 | 1,826 |
| reopened-status | 374 | 586 | 431 | 76 | 2,948 | 1,842 |
| reopened-read | 428 | 2,243 | 551 | 843 | 2,181 | 1,842 |
| reopened-read-pinned | 378 | 2,243 | 551 | 843 | 2,181 | 1,842 |
| sandbox-0-booted-pinned | 205 | 251 | 248 | 0 | 3,023 | 1,842 |
| sandbox-0-mounted | 207 | 253 | 250 | 0 | 3,023 | 1,842 |
| sandbox-0-status | 382 | 597 | 436 | 76 | 3,010 | 1,904 |
| sandbox-0-read | 421 | 2,717 | 582 | 843 | 2,184 | 1,846 |
| sandbox-0-read-pinned | 374 | 2,717 | 582 | 843 | 2,184 | 1,846 |
| sandbox-0-written-pinned | 384 | 2,725 | 591 | 843 | 2,184 | 1,846 |
| sandbox-1-booted-pinned | 185 | 251 | 248 | 0 | 3,044 | 1,846 |
| sandbox-1-mounted | 186 | 253 | 250 | 0 | 3,044 | 1,846 |
| sandbox-1-status | 366 | 601 | 442 | 76 | 2,969 | 1,846 |
| sandbox-1-read | 420 | 2,712 | 578 | 843 | 2,202 | 1,846 |
| sandbox-1-read-pinned | 364 | 2,712 | 578 | 843 | 2,202 | 1,846 |
| sandbox-1-written-pinned | 390 | 2,718 | 584 | 843 | 2,202 | 1,846 |
| sandbox-2-booted-pinned | 184 | 253 | 250 | 0 | 3,044 | 1,846 |
| sandbox-2-mounted | 185 | 255 | 252 | 0 | 3,044 | 1,846 |
| sandbox-2-status | 376 | 593 | 433 | 76 | 2,969 | 1,846 |
| sandbox-2-read | 412 | 2,716 | 581 | 843 | 2,202 | 1,846 |
| sandbox-2-read-pinned | 360 | 2,716 | 581 | 843 | 2,202 | 1,846 |
| sandbox-2-written-pinned | 414 | 2,725 | 591 | 843 | 2,202 | 1,846 |
| hundreds-booted-pinned | 202 | 255 | 251 | 0 | 3,060 | 1,846 |
| hundreds-mounted-pinned | 522 | 560 | 556 | 0 | 3,061 | 1,846 |
| hundreds-status | 692 | 907 | 747 | 76 | 2,986 | 1,846 |
| hundreds-written-pinned | 530 | 913 | 752 | 76 | 2,985 | 1,846 |
| rebooted-booted-pinned | 196 | 258 | 254 | 0 | 3,061 | 1,846 |
| rebooted-mounted | 198 | 259 | 255 | 0 | 3,061 | 1,846 |
| rebooted-status | 371 | 598 | 438 | 76 | 2,986 | 1,846 |
| rebooted-status-pinned | 277 | 623 | 463 | 76 | 2,986 | 1,846 |
| rebooted-mounted-all-pinned | 575 | 870 | 710 | 76 | 2,987 | 1,846 |

## The host's disk, MiB

| ext4: after | bucket | ext4.img | extentd-cache |
|---|---|---|---|
| checkout | 0.0 | 2,195.1 | 0.0 |
| reopened | 0.0 | 2,195.1 | 0.0 |

| fxfs: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 744.1 | 0.1 | 0.1 | 722.5 |  |  |  |  |
| sandbox-0 | 1.7 | 745.8 | 0.1 | 0.1 | 722.8 | 0.1 |  |  |  |
| sandbox-1 | 3.3 | 747.4 | 0.1 | 0.1 | 723.0 | 0.1 | 0.1 |  |  |
| sandbox-2 | 5.0 | 749.0 | 0.1 | 0.1 | 723.2 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.1 | 753.1 | 0.1 | 0.1 | 723.4 | 0.1 | 0.1 | 0.1 | 0.1 |

| fxfs-dax: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 744.4 | 0.1 | 0.1 | 723.1 |  |  |  |  |
| sandbox-0 | 1.7 | 746.0 | 0.1 | 0.1 | 723.5 | 0.1 |  |  |  |
| sandbox-1 | 3.3 | 747.7 | 0.1 | 0.1 | 723.7 | 0.1 | 0.1 |  |  |
| sandbox-2 | 5.0 | 749.3 | 0.1 | 0.1 | 723.9 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.2 | 753.4 | 0.1 | 0.1 | 724.1 | 0.1 | 0.1 | 0.1 | 0.1 |
