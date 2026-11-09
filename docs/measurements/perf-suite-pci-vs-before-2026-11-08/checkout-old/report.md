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
| volume-layers |  | 18,898,944 bytes | 18,898,944 bytes |
| volume-allocator-records |  | 1,558 records | 1,557 records |

## Checking it out, and opening it in a fresh VM

|  | ext4 checkout | fxfs checkout | fxfs-dax checkout | ext4 reopened | fxfs reopened | fxfs-dax reopened |
|---|---|---|---|---|---|---|
| mkfs | 436.1 ms |  |  |  |  |  |
| default-mount |  | 56.8 ms | 58.8 ms |  | 69.4 ms | 67.5 ms |
| mount | 50.2 ms |  |  | 55.4 ms | 18.8 ms | 20.0 ms |
| clone | 10.39 s | 10.01 s | 10.05 s |  |  |  |
| checkout | 16.27 s | 36.99 s | 36.91 s |  |  |  |
| sync | 1.51 s | 449.7 ms | 445.2 ms |  |  |  |
| status-cold | 335.8 ms | 761.8 ms | 813.3 ms | 515.3 ms | 673.8 ms | 782.2 ms |
| status-warm | 133.9 ms | 111.8 ms | 113.4 ms | 169.1 ms | 111.1 ms | 163.6 ms |
| status-synced | 131.9 ms | 139.2 ms | 133.5 ms | 151.2 ms | 130.8 ms | 142.8 ms |
| status-warm-again | 131.2 ms | 101.0 ms | 103.6 ms | 162.9 ms | 105.5 ms | 113.3 ms |
| read-cold | 7.94 s | 20.77 s | 10.63 s | 12.46 s | 23.64 s | 4.33 s |
| read-warm | 577.9 ms | 542.0 ms | 525.8 ms | 572.8 ms | 537.9 ms | 490.1 ms |
| publish |  | 1.24 s | 1.25 s |  |  |  |

## Shared, a volume over the published base

|  | fxfs | fxfs-dax |
|---|---|---|
| sandbox attach | 18.1 ms - 20.1 ms | 18.9 ms - 20.5 ms |
| sandbox import | 19.6 ms - 21.3 ms | 19.5 ms - 21.9 ms |
| sandbox mount | 0.5 ms - 1.3 ms | 1.1 ms - 1.6 ms |
| sandbox status-cold | 666.2 ms - 727.4 ms | 794.8 ms - 857.2 ms |
| sandbox status-warm | 110.3 ms - 158.8 ms | 113.3 ms - 167.8 ms |
| sandbox status-synced | 176.3 ms - 190.3 ms | 142.2 ms - 197.8 ms |
| sandbox status-warm-again | 103.5 ms - 150.6 ms | 112.4 ms - 147.8 ms |
| sandbox read-cold | 23.17 s - 23.55 s | 5.07 s - 5.60 s |
| sandbox read-warm | 544.5 ms - 546.8 ms | 490.1 ms - 495.0 ms |
| sandbox first-overwrite | 1.2 ms - 1.2 ms | 2.7 ms - 2.9 ms |
| sandbox first-create | 1.3 ms - 1.4 ms | 1.4 ms - 1.8 ms |
| sandbox status-after-writes | 469.9 ms - 485.1 ms | 445.1 ms - 457.4 ms |
| hundreds import-median | 6.2 ms | 6.3 ms |
| hundreds import-max | 20.3 ms | 19.5 ms |
| hundreds mount-median | 0.3 ms | 0.3 ms |
| hundreds mount-max | 0.8 ms | 0.9 ms |
| hundreds status-cold | 672.4 ms | 815.5 ms |
| hundreds status-warm | 117.9 ms | 160.3 ms |
| hundreds status-synced | 157.3 ms | 153.7 ms |
| hundreds first-overwrite | 2.2 ms | 3.5 ms |
| hundreds first-create | 1.4 ms | 1.6 ms |
| hundreds unmount-all | 3.75 s | 3.66 s |
| rebooted default-mount | 85.8 ms | 88.4 ms |
| rebooted mount | 7.1 ms | 8.1 ms |
| rebooted status-cold | 651.0 ms | 801.4 ms |
| rebooted status-warm | 159.4 ms | 153.1 ms |
| rebooted mount-others-median | 6.3 ms | 6.4 ms |
| rebooted mount-others-max | 9.8 ms | 7.3 ms |
| rebooted unmount-all | 3.67 s | 3.77 s |

## Memory, MiB

| ext4: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 183 | 245 | 242 | 0 | 29 | 0 |
| checkout-checked-out | 424 | 2,918 | 2,915 | 0 | 29 | 0 |
| checkout-checked-out-pinned | 277 | 2,918 | 2,915 | 0 | 29 | 0 |
| checkout-status | 399 | 2,918 | 2,915 | 0 | 29 | 0 |
| checkout-read | 367 | 2,923 | 2,919 | 0 | 29 | 0 |
| checkout-read-pinned | 310 | 2,923 | 2,919 | 0 | 29 | 0 |
| reopened-booted-pinned | 164 | 244 | 241 | 0 | 29 | 0 |
| reopened-mounted | 164 | 244 | 241 | 0 | 29 | 0 |
| reopened-status | 309 | 428 | 424 | 0 | 29 | 0 |
| reopened-read | 352 | 2,115 | 2,111 | 0 | 29 | 0 |
| reopened-read-pinned | 263 | 2,115 | 2,111 | 0 | 29 | 0 |

| fxfs: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 189 | 251 | 248 | 0 | 40 | 4 |
| checkout-checked-out | 585 | 2,920 | 2,914 | 1 | 1,035 | 2 |
| checkout-checked-out-pinned | 396 | 2,920 | 2,914 | 1 | 1,035 | 2 |
| checkout-status | 496 | 2,922 | 2,917 | 1 | 1,196 | 134 |
| checkout-read | 466 | 2,925 | 2,919 | 1 | 2,889 | 1,812 |
| checkout-read-pinned | 398 | 2,925 | 2,919 | 1 | 2,889 | 1,812 |
| reopened-booted-pinned | 197 | 252 | 249 | 0 | 2,947 | 1,812 |
| reopened-mounted | 199 | 255 | 251 | 0 | 2,951 | 1,816 |
| reopened-status | 359 | 422 | 418 | 1 | 2,965 | 1,830 |
| reopened-read | 449 | 2,131 | 2,127 | 1 | 2,963 | 1,830 |
| reopened-read-pinned | 324 | 2,131 | 2,127 | 1 | 2,963 | 1,830 |
| sandbox-0-booted-pinned | 208 | 257 | 254 | 0 | 2,962 | 1,830 |
| sandbox-0-mounted | 213 | 259 | 256 | 0 | 2,967 | 1,834 |
| sandbox-0-status | 355 | 422 | 418 | 1 | 3,029 | 1,896 |
| sandbox-0-read | 454 | 2,142 | 2,137 | 1 | 2,981 | 1,846 |
| sandbox-0-read-pinned | 322 | 2,142 | 2,137 | 1 | 2,981 | 1,846 |
| sandbox-0-written-pinned | 320 | 2,146 | 2,141 | 1 | 2,981 | 1,846 |
| sandbox-1-booted-pinned | 193 | 257 | 253 | 0 | 2,999 | 1,846 |
| sandbox-1-mounted | 196 | 259 | 256 | 0 | 2,999 | 1,846 |
| sandbox-1-status | 356 | 433 | 429 | 0 | 3,000 | 1,846 |
| sandbox-1-read | 440 | 2,145 | 2,140 | 1 | 3,000 | 1,846 |
| sandbox-1-read-pinned | 333 | 2,145 | 2,140 | 1 | 3,000 | 1,846 |
| sandbox-1-written-pinned | 369 | 2,150 | 2,145 | 1 | 3,000 | 1,846 |
| sandbox-2-booted-pinned | 183 | 256 | 253 | 0 | 3,015 | 1,846 |
| sandbox-2-mounted | 184 | 258 | 255 | 0 | 3,016 | 1,846 |
| sandbox-2-status | 364 | 428 | 424 | 0 | 3,016 | 1,846 |
| sandbox-2-read | 447 | 2,139 | 2,135 | 0 | 3,016 | 1,846 |
| sandbox-2-read-pinned | 338 | 2,139 | 2,135 | 0 | 3,016 | 1,846 |
| sandbox-2-written-pinned | 367 | 2,146 | 2,142 | 0 | 3,016 | 1,846 |
| hundreds-booted-pinned | 196 | 250 | 247 | 0 | 3,032 | 1,846 |
| hundreds-mounted-pinned | 511 | 556 | 552 | 0 | 3,032 | 1,846 |
| hundreds-status | 676 | 740 | 736 | 0 | 3,032 | 1,846 |
| hundreds-written-pinned | 541 | 756 | 752 | 0 | 3,032 | 1,846 |
| rebooted-booted-pinned | 186 | 255 | 251 | 0 | 3,032 | 1,846 |
| rebooted-mounted | 187 | 256 | 252 | 0 | 3,032 | 1,846 |
| rebooted-status | 338 | 417 | 413 | 0 | 3,032 | 1,846 |
| rebooted-status-pinned | 235 | 454 | 450 | 0 | 3,032 | 1,846 |
| rebooted-mounted-all-pinned | 537 | 656 | 652 | 0 | 3,032 | 1,846 |

| fxfs-dax: phase | guest used | VM RSS | VM anon | VM shmem | extentd PSS | extentd arenas |
|---|---|---|---|---|---|---|
| checkout-booted-pinned | 195 | 248 | 245 | 0 | 44 | 8 |
| checkout-checked-out | 563 | 2,918 | 2,913 | 1 | 1,033 | 6 |
| checkout-checked-out-pinned | 369 | 2,918 | 2,913 | 1 | 1,033 | 6 |
| checkout-status | 511 | 2,937 | 2,922 | 6 | 1,190 | 130 |
| checkout-read | 482 | 4,629 | 2,942 | 842 | 2,076 | 1,834 |
| checkout-read-pinned | 466 | 4,629 | 2,942 | 842 | 2,076 | 1,834 |
| reopened-booted-pinned | 196 | 252 | 249 | 0 | 2,961 | 1,834 |
| reopened-mounted | 197 | 255 | 251 | 0 | 2,965 | 1,838 |
| reopened-status | 362 | 589 | 428 | 79 | 2,902 | 1,852 |
| reopened-read | 422 | 2,239 | 547 | 843 | 2,137 | 1,852 |
| reopened-read-pinned | 363 | 2,239 | 547 | 843 | 2,137 | 1,852 |
| sandbox-0-booted-pinned | 196 | 253 | 250 | 0 | 2,979 | 1,852 |
| sandbox-0-mounted | 198 | 255 | 252 | 0 | 2,981 | 1,852 |
| sandbox-0-status | 375 | 596 | 428 | 78 | 2,963 | 1,912 |
| sandbox-0-read | 417 | 2,707 | 574 | 843 | 2,141 | 1,856 |
| sandbox-0-read-pinned | 359 | 2,707 | 574 | 843 | 2,141 | 1,856 |
| sandbox-0-written-pinned | 410 | 2,713 | 581 | 843 | 2,141 | 1,856 |
| sandbox-1-booted-pinned | 201 | 256 | 252 | 0 | 2,989 | 1,856 |
| sandbox-1-mounted | 202 | 258 | 254 | 0 | 2,989 | 1,856 |
| sandbox-1-status | 374 | 604 | 436 | 78 | 2,911 | 1,856 |
| sandbox-1-read | 439 | 2,712 | 579 | 843 | 2,146 | 1,856 |
| sandbox-1-read-pinned | 384 | 2,712 | 579 | 843 | 2,146 | 1,856 |
| sandbox-1-written-pinned | 403 | 2,721 | 589 | 843 | 2,146 | 1,856 |
| sandbox-2-booted-pinned | 195 | 248 | 244 | 0 | 2,990 | 1,856 |
| sandbox-2-mounted | 196 | 250 | 247 | 0 | 2,991 | 1,856 |
| sandbox-2-status | 357 | 599 | 431 | 78 | 2,913 | 1,856 |
| sandbox-2-read | 430 | 2,728 | 595 | 843 | 2,148 | 1,856 |
| sandbox-2-read-pinned | 379 | 2,728 | 595 | 843 | 2,148 | 1,856 |
| sandbox-2-written-pinned | 415 | 2,741 | 609 | 843 | 2,148 | 1,856 |
| hundreds-booted-pinned | 202 | 251 | 248 | 0 | 2,991 | 1,856 |
| hundreds-mounted-pinned | 507 | 554 | 551 | 0 | 2,991 | 1,856 |
| hundreds-status | 685 | 916 | 748 | 78 | 2,914 | 1,856 |
| hundreds-written-pinned | 526 | 937 | 768 | 79 | 2,913 | 1,856 |
| rebooted-booted-pinned | 196 | 259 | 255 | 0 | 2,992 | 1,856 |
| rebooted-mounted | 201 | 260 | 256 | 0 | 2,992 | 1,856 |
| rebooted-status | 377 | 600 | 432 | 78 | 2,914 | 1,856 |
| rebooted-status-pinned | 220 | 622 | 454 | 78 | 2,914 | 1,856 |
| rebooted-mounted-all-pinned | 537 | 846 | 678 | 78 | 2,914 | 1,856 |

## The host's disk, MiB

| ext4: after | bucket | ext4.img | extentd-cache |
|---|---|---|---|
| checkout | 0.0 | 2,195.0 | 0.0 |
| reopened | 0.0 | 2,195.0 | 0.0 |

| fxfs: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 733.9 | 0.1 | 0.1 | 711.0 |  |  |  |  |
| sandbox-0 | 1.7 | 735.5 | 0.1 | 0.1 | 711.4 | 0.1 |  |  |  |
| sandbox-1 | 3.3 | 737.2 | 0.1 | 0.1 | 711.5 | 0.1 | 0.1 |  |  |
| sandbox-2 | 5.0 | 738.8 | 0.1 | 0.1 | 711.7 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.2 | 742.9 | 0.1 | 0.1 | 711.9 | 0.1 | 0.1 | 0.1 | 0.1 |

| fxfs-dax: after | bucket | extentd-cache | generation | publisher | publisher.bucket | sandbox-0 | sandbox-1 | sandbox-2 | hundreds |
|---|---|---|---|---|---|---|---|---|---|
| checkout | 0.0 | 738.3 | 0.1 | 0.1 | 714.3 |  |  |  |  |
| sandbox-0 | 1.7 | 740.0 | 0.1 | 0.1 | 714.6 | 0.1 |  |  |  |
| sandbox-1 | 3.3 | 741.6 | 0.1 | 0.1 | 714.8 | 0.1 | 0.1 |  |  |
| sandbox-2 | 5.0 | 743.2 | 0.1 | 0.1 | 715.0 | 0.1 | 0.1 | 0.1 |  |
| hundreds | 9.2 | 747.3 | 0.1 | 0.1 | 715.2 | 0.1 | 0.1 | 0.1 | 0.1 |
