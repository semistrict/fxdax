Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| seq-write | MiB/s | 107 (107–107) | 70.1 (65.8–70.3) | 1.53× |
| rand-read-cold | IOPS | 76,561 (76,205–78,019) | 135,405 (112,219–167,184) | 0.57× |
| seq-read-cold | MiB/s | 463 (454–464) | 908 (719–1,128) | 0.51× |
| seq-read-warm | MiB/s | 11,422 (11,084–11,532) | 11,096 (10,908–11,586) | 1.03× |
| rand-read-warm | IOPS | 888,169 (883,077–888,564) | 807,202 (804,868–807,332) | 1.10× |
| rand-write | IOPS | 8,677 (7,669–8,686) | 13,223 (13,218–14,428) | 0.66× |
| mmap-rand-read | IOPS | 809,052 (807,190–828,744) | 1,136,322 (1,122,231–1,153,249) | 0.71× |
| mmap-rand-write | IOPS | 8,680 (7,668–8,684) | 11,822 (11,815–13,205) | 0.73× |
| fsync-append | IOPS | 346 (345–346) | 402 (402–661) | 0.86× |
| create | ops/s | 39,057 (37,039–39,271) | 25,143 (21,426–26,522) | 1.55× |
| stat-cold | ops/s | 347,297 (223,966–347,798) | 192,330 (140,530–197,840) | 1.81× |
| stat-warm | ops/s | 850,080 (848,663–1,397,357) | 1,440,550 (806,087–1,460,031) | 0.59× |
| lookup-missing | ops/s | 392,290 (359,037–396,398) | 385,925 (321,757–430,156) | 1.02× |
| readdir | ops/s | 1,300,132 (1,258,337–3,097,356) | 2,455,524 (2,324,595–2,470,588) | 0.53× |
| read-small-cold | ops/s | 54,119 (52,987–55,065) | 93,345 (91,354–95,195) | 0.58× |
| chmod | ops/s | 826,715 (758,194–861,701) | 1,290,999 (1,236,415–1,355,453) | 0.64× |
| setxattr | ops/s | 631,896 (611,888–641,159) | 385,211 (218,767–549,085) | 1.64× |
| link | ops/s | 155,483 (155,012–157,015) | 97,842 (91,680–98,667) | 1.59× |
| rename | ops/s | 124,165 (123,206–125,480) | 77,646 (73,699–80,432) | 1.60× |
| rename-across | ops/s | 104,660 (104,604–108,409) | 69,079 (59,784–70,522) | 1.52× |
| unlink | ops/s | 436,487 (430,570–443,205) | 223,838 (166,392–276,469) | 1.95× |
| create-fsync | ops/s | 568 (567–572) | 1,722 (765–1,826) | 0.33× |
| mkdir-tree | ops/s | 43,794 (43,643–43,863) | 26,506 (9,928–28,303) | 1.65× |
| find-cold | ops/s | 323,891 (241,357–325,199) | 154,086 (101,864–177,628) | **2.10×** |
| rm-tree | ops/s | 102,388 (99,362–106,141) | 63,257 (62,823–70,194) | 1.62× |
| large-dir-create | ops/s | 110,920 (80,386–112,783) | 85,666 (57,824–87,437) | 1.29× |
| large-dir-stat-cold | ops/s | 272,641 (264,883–276,881) | 191,913 (191,057–191,989) | 1.42× |
| large-dir-unlink | ops/s | 219,422 (216,776–221,958) | 239,470 (203,534–251,555) | 0.92× |
| create-parallel | ops/s | 177,566 (175,454–183,799) | 115,799 (109,024–131,060) | 1.53× |
| stat-parallel | ops/s | 669,859 (609,325–676,432) | 478,442 (471,084–489,104) | 1.40× |
| unlink-parallel | ops/s | 309,293 (299,479–321,343) | 190,354 (177,534–194,756) | 1.62× |
| create-shared-dir | ops/s | 112,895 (107,918–113,516) | 84,086 (80,974–91,108) | 1.34× |
| unlink-shared-dir | ops/s | 283,415 (271,089–290,812) | 183,965 (171,027–185,624) | 1.54× |
| pgbench-init | s | 3.19 (3.09–3.29) | 3.83 (3.80–4.03) | 1.20× |
| pgbench-tps | tps | 1,925 (1,853–1,941) | 1,465 (1,463–1,476) | 1.31× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.08 median (-6.30–-3.61)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.25 median (-4.57–-3.77)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 133 | B | $0.000053 |
| get-range | 30 | B | $0.000012 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 162 | A | $0.000810 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 331 | | $0.000900 |

Uploaded 822.4 MiB (segments 822.4 MiB, extents 0 B, checkpoints 13.4 KiB, moves 0 B, other 39 B); downloaded 3.0 KiB.

Over 2× slower than ext4:
- find-cold on fxfs-guest-dax: 2.10× ext4-dax-pmem's
