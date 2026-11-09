Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | fxfs-guest | fxfs-guest-dax | fxfs-guest vs ext4-guest-kernel | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|
| seq-write | MiB/s | 192 (122–194) | 141 (106–143) | 140 (107–143) | 1.36× | 1.37× |
| rand-read-cold | IOPS | 14,385 (13,430–14,721) | 5,074 (5,010–5,098) | 12,021 (11,678–12,291) | **2.83×** | 1.20× |
| seq-read-cold | MiB/s | 3,282 (1,306–3,657) | 918 (738–921) | 401 (399–404) | **3.58×** | **8.18×** |
| seq-read-warm | MiB/s | 8,493 (8,465–8,532) | 8,476 (8,403–8,634) | 11,279 (11,267–11,481) | 1.00× | 0.75× |
| rand-read-warm | IOPS | 939,613 (932,641–940,863) | 939,853 (936,298–943,501) | 855,648 (851,890–858,092) | 1.00× | 1.10× |
| mmap-seq-read | MiB/s | 9,846 (9,143–9,846) | 9,143 (8,828–9,143) | 10,240 (9,846–10,240) | 1.08× | 0.96× |
| rand-write | IOPS | 14,868 (10,009–15,004) | 75,502 (74,813–76,561) | 5,500 (4,988–5,729) | 0.20× | **2.70×** |
| mmap-rand-read | IOPS | 1,292,981 (1,284,010–1,293,529) | 1,255,961 (1,250,090–1,259,001) | 1,308,081 (1,307,797–1,311,491) | 1.03× | 0.99× |
| mmap-rand-write | IOPS | 8,623 (8,325–11,292) | 7,603 (7,550–7,806) | 59,796 (59,362–63,504) | 1.13× | 0.14× |
| fsync-append | IOPS | 385 (378–403) | 2,351 (1,675–2,379) | 2,938 (2,684–3,003) | 0.16× | 0.13× |
| sqlite-pread-scan | MiB/s | 1,710 (609–2,592) | 322 (296–329) | 164 (160–164) | **5.31×** | **10.43×** |
| sqlite-pread-scan-warm | MiB/s | 2,985 (2,790–2,994) | 3,081 (3,080–3,117) | 2,759 (2,724–2,778) | 0.97× | 1.08× |
| sqlite-mmap-scan | MiB/s | 2,560 (2,408–2,724) | 828 (827–835) | 2,909 (2,809–2,931) | **3.09×** | 0.88× |
| sqlite-mmap-scan-warm | MiB/s | 2,943 (2,902–2,987) | 2,873 (2,861–2,933) | 3,037 (3,002–3,037) | 1.02× | 0.97× |
| create | ops/s | 97,346 (87,709–97,939) | 81,173 (77,746–83,487) | 82,158 (77,280–82,390) | 1.20× | 1.18× |
| stat-cold | ops/s | 307,301 (215,316–327,161) | 195,532 (163,686–196,229) | 194,069 (193,089–194,723) | 1.57× | 1.58× |
| stat-warm | ops/s | 1,289,572 (1,285,090–1,381,623) | 1,336,395 (862,116–1,402,559) | 1,322,215 (1,291,708–1,395,350) | 0.96× | 0.98× |
| lookup-missing | ops/s | 438,739 (371,781–446,724) | 482,023 (463,842–519,751) | 475,697 (445,129–503,931) | 0.91× | 0.92× |
| readdir | ops/s | 2,984,264 (2,933,838–3,049,845) | 3,395,171 (3,003,436–3,501,364) | 3,489,963 (3,375,784–3,758,318) | 0.88× | 0.86× |
| read-small-cold | ops/s | 15,298 (15,246–15,317) | 6,672 (6,455–6,881) | 37,198 (37,135–37,948) | **2.29×** | 0.41× |
| chmod | ops/s | 733,679 (518,647–791,550) | 404,874 (320,147–422,779) | 455,170 (441,376–475,345) | 1.81× | 1.61× |
| setxattr | ops/s | 618,849 (604,044–631,473) | 175,622 (171,485–178,973) | 198,430 (177,809–203,895) | **3.52×** | **3.12×** |
| link | ops/s | 158,598 (157,840–159,451) | 97,898 (95,133–99,686) | 100,410 (100,085–105,053) | 1.62× | 1.58× |
| rename | ops/s | 118,934 (111,717–119,757) | 80,764 (77,064–81,040) | 79,727 (79,706–81,615) | 1.47× | 1.49× |
| rename-across | ops/s | 103,501 (103,262–105,068) | 67,677 (63,981–68,485) | 67,157 (65,929–67,813) | 1.53× | 1.54× |
| unlink | ops/s | 430,639 (428,918–434,352) | 179,104 (171,906–359,848) | 317,358 (187,826–329,184) | **2.40×** | 1.36× |
| create-fsync | ops/s | 361 (359–380) | 2,163 (1,996–2,422) | 2,488 (2,487–2,599) | 0.17× | 0.15× |

- ext4-guest-kernel: mmap-seq-read-faults-per-mib 6.60 median (6.60–6.60)
- ext4-guest-kernel: mtime-ahead-of-guest-ms -4.83 median (-6.00–-4.35)
- ext4-guest-kernel: sqlite-mmap-scan-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-mmap-scan-warm-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- ext4-guest-kernel: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest: mmap-seq-read-faults-per-mib 13.7 median (13.7–13.7)
- fxfs-guest: mtime-ahead-of-guest-ms -5.23 median (-6.75–-4.51)
- fxfs-guest: sqlite-mmap-scan-faults-per-mib 9.60 median (9.60–9.60)
- fxfs-guest: sqlite-mmap-scan-warm-faults-per-mib 9.30 median (9.30–9.30)
- fxfs-guest: sqlite-pread-scan-faults-per-mib 2.60 median (2.60–2.70)
- fxfs-guest: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.45 median (-5.54–-4.99)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 1.60 median (1.60–1.70)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 175 | B | $0.000070 |
| get-range | 4,689 | B | $0.001876 |
| head | 0 | B | $0.000000 |
| list | 12 | A | $0.000060 |
| create | 2,232 | A | $0.011160 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 7,111 | | $0.013176 |

Uploaded 4.4 GiB (segments 2.9 GiB, extents 1.5 GiB, checkpoints 47.5 KiB, moves 0 B, other 96 B); downloaded 3.0 GiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 8,332.1 µs |
| read | 60,718 | 96.2 µs |
| write | 60,274 | 724.4 µs |
| free | 68 | 11.0 µs |
| map | 1,686 | 17.3 µs |
| unmap | 783 | 62.7 µs |
| commit | 142 | 122,831.3 µs |
| root | 2 | 1.5 µs |
| info | 8 | 2.7 µs |
| window | 1 | 117.3 µs |
| fault | 12,806 | 334.2 µs |
| transfer | 2 | 10.8 µs |
| free_many | 222 | 1,982.9 µs |
| private | 49,152 | 7.2 µs |

Commits: 13,738, 3,334.4 µs each: committing the store 430.1 µs, waiting for object storage 2,904.1 µs.

Segments closed and framed: 181, 184.6 ms each.

Over 2× slower than ext4:
- rand-read-cold on fxfs-guest: 2.83× ext4-guest-kernel's
- seq-read-cold on fxfs-guest: 3.58× ext4-guest-kernel's
- seq-read-cold on fxfs-guest-dax: 8.18× ext4-guest-kernel's
- rand-write on fxfs-guest-dax: 2.70× ext4-guest-kernel's
- sqlite-pread-scan on fxfs-guest: 5.31× ext4-guest-kernel's
- sqlite-pread-scan on fxfs-guest-dax: 10.43× ext4-guest-kernel's
- sqlite-mmap-scan on fxfs-guest: 3.09× ext4-guest-kernel's
- read-small-cold on fxfs-guest: 2.29× ext4-guest-kernel's
- setxattr on fxfs-guest: 3.52× ext4-guest-kernel's
- setxattr on fxfs-guest-dax: 3.12× ext4-guest-kernel's
- unlink on fxfs-guest: 2.40× ext4-guest-kernel's
