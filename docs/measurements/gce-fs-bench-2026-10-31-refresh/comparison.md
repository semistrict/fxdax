Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, ext4-dax-pmem 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | ext4-dax-pmem | fxfs-guest | fxfs-guest-dax | ext4-dax-pmem vs ext4-guest-kernel | fxfs-guest vs ext4-dax-pmem | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|---|---|---|---|
| seq-write | MiB/s | 135 (107–135) | 107 (107–107) | 184 (119–184) | 183 (182–184) | 1.26× | 0.58× | 0.58× |
| rand-read-cold | IOPS | 14,486 (13,452–15,128) | 73,471 (73,143–74,813) | 3,173 (3,135–3,216) | 75,502 (75,156–75,852) | 0.20× | **23.16×** | 0.97× |
| seq-read-cold | MiB/s | 3,325 (1,196–3,507) | 438 (432–454) | 1,237 (911–1,293) | 453 (451–453) | **7.58×** | 0.35× | 0.97× |
| seq-read-warm | MiB/s | 8,116 (7,847–8,182) | 11,042 (10,614–11,161) | 8,383 (8,321–8,468) | 11,076 (10,993–11,386) | 0.74× | 1.32× | 1.00× |
| rand-read-warm | IOPS | 925,410 (899,325–926,659) | 886,068 (879,749–888,917) | 938,884 (930,449–946,752) | 863,212 (858,168–867,583) | 1.04× | 0.94× | 1.03× |
| mmap-seq-read | MiB/s | 9,846 (9,846–9,846) | 10,240 (9,846–10,240) | 8,828 (8,828–9,143) | 9,846 (9,481–9,846) | 0.96× | 1.16× | 1.04× |
| rand-write | IOPS | 10,011 (8,685–11,805) | 8,683 (8,681–8,686) | 25,759 (25,751–25,767) | 17,928 (17,852–18,437) | 1.15× | 0.34× | 0.48× |
| mmap-rand-read | IOPS | 1,257,063 (1,243,593–1,260,113) | 796,095 (780,628–804,704) | 1,231,781 (1,226,556–1,241,063) | 1,096,374 (787,018–1,161,762) | 1.58× | 0.65× | 0.73× |
| mmap-rand-write | IOPS | 6,865 (6,211–8,664) | 8,683 (8,682–8,684) | 10,008 (10,006–11,714) | 6,129 (5,589–6,852) | 0.79× | 0.87× | 1.42× |
| fsync-append | IOPS | 225 (225–225) | 345 (345–346) | 400 (399–661) | 633 (392–638) | 0.65× | 0.86× | 0.55× |
| sqlite-pread-scan | MiB/s | 2,617 (778–2,710) | 283 (282–285) | 116 (115–120) | 284 (283–286) | **9.25×** | **2.45×** | 1.00× |
| sqlite-pread-scan-warm | MiB/s | 3,038 (2,977–3,055) | 2,797 (2,754–2,820) | 3,091 (3,014–3,103) | 2,826 (2,753–2,837) | 1.09× | 0.90× | 0.99× |
| sqlite-mmap-scan | MiB/s | 2,749 (2,670–2,788) | 2,776 (1,872–2,874) | 1,128 (1,126–1,142) | 2,786 (2,706–2,806) | 0.99× | **2.46×** | 1.00× |
| sqlite-mmap-scan-warm | MiB/s | 2,927 (2,896–2,999) | 2,928 (1,973–2,996) | 2,827 (2,804–2,857) | 2,934 (2,895–2,945) | 1.00× | 1.04× | 1.00× |
| create | ops/s | 96,625 (94,661–97,280) | 37,863 (33,036–37,889) | 78,631 (75,898–80,394) | 25,928 (25,452–26,292) | **2.55×** | 0.48× | 1.46× |
| stat-cold | ops/s | 330,910 (324,158–334,843) | 337,133 (145,831–337,573) | 193,384 (173,603–202,315) | 173,444 (157,095–201,709) | 0.98× | 1.74× | 1.94× |
| stat-warm | ops/s | 1,232,666 (1,218,108–1,309,634) | 743,846 (731,324–1,346,745) | 1,197,876 (691,085–1,243,552) | 1,351,292 (1,304,123–1,371,816) | 1.66× | 0.62× | 0.55× |
| lookup-missing | ops/s | 435,293 (409,056–436,395) | 451,153 (450,179–459,758) | 399,768 (351,505–465,609) | 443,171 (443,078–495,000) | 0.96× | 1.13× | 1.02× |
| readdir | ops/s | 3,046,909 (3,021,577–3,049,729) | 3,023,884 (998,991–3,063,711) | 3,464,499 (2,571,702–3,476,761) | 3,433,759 (3,182,266–3,525,602) | 1.01× | 0.87× | 0.88× |
| read-small-cold | ops/s | 15,491 (15,266–15,708) | 53,777 (53,460–54,283) | 5,418 (5,222–5,446) | 50,925 (50,827–51,829) | 0.29× | **9.93×** | 1.06× |
| chmod | ops/s | 480,437 (455,433–692,232) | 536,245 (534,276–835,559) | 345,142 (272,095–395,789) | 364,203 (343,788–396,267) | 0.90× | 1.55× | 1.47× |
| setxattr | ops/s | 553,118 (549,653–569,123) | 640,647 (627,519–645,499) | 605,880 (603,533–618,278) | 599,322 (249,087–683,005) | 0.86× | 1.06× | 1.07× |
| link | ops/s | 152,342 (141,833–155,092) | 153,378 (143,818–157,150) | 96,211 (86,286–96,562) | 97,652 (97,365–104,397) | 0.99× | 1.59× | 1.57× |
| rename | ops/s | 116,820 (111,955–118,087) | 121,627 (115,778–122,975) | 80,470 (68,014–81,035) | 80,058 (77,795–81,025) | 0.96× | 1.51× | 1.52× |
| rename-across | ops/s | 98,221 (96,882–99,394) | 100,675 (99,900–101,519) | 63,186 (58,366–69,624) | 69,251 (67,606–74,711) | 0.98× | 1.59× | 1.45× |
| unlink | ops/s | 403,992 (351,051–428,427) | 430,106 (402,366–439,716) | 335,741 (255,497–389,874) | 200,002 (165,552–221,050) | 0.94× | 1.28× | **2.15×** |
| create-fsync | ops/s | 352 (343–360) | 537 (533–556) | 2,322 (2,282–2,395) | 2,179 (2,106–2,202) | 0.66× | 0.23× | 0.25× |
| mkdir-tree | ops/s | 127,097 (126,790–127,693) | 42,731 (42,432–42,907) | 79,642 (64,405–80,515) | 27,100 (26,587–27,661) | **2.97×** | 0.54× | 1.58× |
| find-cold | ops/s | 318,485 (317,788–320,083) | 321,812 (289,548–322,358) | 177,759 (163,329–185,067) | 156,096 (145,985–165,082) | 0.99× | 1.81× | **2.06×** |
| rm-tree | ops/s | 106,739 (104,129–106,775) | 104,830 (102,388–107,041) | 74,916 (74,515–78,503) | 69,282 (67,803–69,893) | 1.02× | 1.40× | 1.51× |
| large-dir-create | ops/s | 111,418 (111,404–112,180) | 108,767 (79,752–111,885) | 92,001 (88,935–92,316) | 83,559 (55,413–90,498) | 1.02× | 1.18× | 1.30× |
| large-dir-stat-cold | ops/s | 261,968 (260,833–279,966) | 247,072 (137,876–259,485) | 176,371 (124,563–190,505) | 167,414 (124,811–179,707) | 1.06× | 1.40× | 1.48× |
| large-dir-unlink | ops/s | 209,278 (207,207–211,980) | 209,977 (209,877–212,093) | 208,698 (191,842–210,737) | 200,058 (155,890–224,042) | 1.00× | 1.01× | 1.05× |
| create-parallel | ops/s | 173,494 (171,015–180,417) | 180,435 (176,142–181,717) | 122,266 (118,514–126,949) | 114,640 (94,610–120,901) | 0.96× | 1.48× | 1.57× |
| stat-parallel | ops/s | 574,280 (567,590–623,815) | 581,788 (559,324–629,171) | 624,673 (588,294–681,827) | 570,716 (556,473–624,784) | 0.99× | 0.93× | 1.02× |
| unlink-parallel | ops/s | 305,864 (305,826–320,672) | 324,552 (287,440–327,661) | 183,066 (180,682–183,523) | 169,885 (163,478–181,859) | 0.94× | 1.77× | 1.91× |
| create-shared-dir | ops/s | 112,852 (112,457–117,782) | 113,047 (112,890–114,973) | 88,219 (84,425–92,615) | 84,469 (82,458–88,196) | 1.00× | 1.28× | 1.34× |
| unlink-shared-dir | ops/s | 275,288 (268,379–277,091) | 281,555 (279,085–295,235) | 155,005 (142,941–161,673) | 152,788 (150,607–159,633) | 0.98× | 1.82× | 1.84× |
| untar | s | 0.02 (0.01–0.02) | 0.02 (0.02–0.02) | 0.02 (0.01–0.02) | 0.02 (0.02–0.02) | 1.00× | 1.06× | 1.19× |
| make | s | 4.55 (4.55–4.67) | 4.58 (4.55–4.73) | 4.60 (4.57–4.65) | 4.61 (4.58–4.66) | 1.01× | 1.00× | 1.01× |
| build-total | s | 4.57 (4.57–4.69) | 4.60 (4.56–4.75) | 4.62 (4.60–4.66) | 4.63 (4.59–4.67) | 1.01× | 1.00× | 1.01× |

- ext4-guest-kernel: mmap-seq-read-faults-per-mib 6.60 median (6.60–6.60)
- ext4-guest-kernel: mtime-ahead-of-guest-ms -4.77 median (-5.03–-4.58)
- ext4-guest-kernel: sqlite-mmap-scan-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-mmap-scan-warm-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- ext4-guest-kernel: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: mmap-seq-read-faults-per-mib 5.60 median (5.60–5.60)
- ext4-dax-pmem: mtime-ahead-of-guest-ms -6.00 median (-6.46–-3.32)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–129)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–129)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest: mmap-seq-read-faults-per-mib 13.7 median (13.7–13.7)
- fxfs-guest: mtime-ahead-of-guest-ms -3.54 median (-5.64–-3.49)
- fxfs-guest: sqlite-mmap-scan-faults-per-mib 9.60 median (9.60–9.60)
- fxfs-guest: sqlite-mmap-scan-warm-faults-per-mib 9.30 median (9.30–9.30)
- fxfs-guest: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -2.89 median (-4.32–-2.66)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 115 | B | $0.000046 |
| get-range | 2 | B | $0.000001 |
| head | 0 | B | $0.000000 |
| list | 7 | A | $0.000035 |
| create | 2,696 | A | $0.013480 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 2,823 | | $0.013572 |

Uploaded 3.9 GiB (segments 2.1 GiB, extents 1.8 GiB, checkpoints 5.8 KiB, moves 0 B, other 66 B); downloaded 2.5 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 7,840.8 µs |
| read | 60,791 | 394.2 µs |
| write | 90,341 | 398.2 µs |
| free | 29 | 70.1 µs |
| map | 109,660 | 96.2 µs |
| unmap | 4,968 | 33.5 µs |
| commit | 50 | 17,377.0 µs |
| root | 2 | 0.9 µs |
| info | 8 | 1.1 µs |
| window | 1 | 9.9 µs |
| transfer | 2 | 15.1 µs |
| free_many | 199 | 3,606.4 µs |

Commits: 2,898, 11,247.1 µs each: writing the open extents' 702.9 KiB 329.6 µs, committing the store 2,270.8 µs, sealing the windows' mappings 68.6 µs, waiting for object storage 8,577.8 µs.

Segments closed and framed: 105, 58.2 ms each.

Over 2× slower than ext4:
- rand-read-cold on fxfs-guest: 23.16× ext4-dax-pmem's
- seq-read-cold on ext4-dax-pmem: 7.58× ext4-guest-kernel's
- sqlite-pread-scan on ext4-dax-pmem: 9.25× ext4-guest-kernel's
- sqlite-pread-scan on fxfs-guest: 2.45× ext4-dax-pmem's
- sqlite-mmap-scan on fxfs-guest: 2.46× ext4-dax-pmem's
- create on ext4-dax-pmem: 2.55× ext4-guest-kernel's
- read-small-cold on fxfs-guest: 9.93× ext4-dax-pmem's
- unlink on fxfs-guest-dax: 2.15× ext4-dax-pmem's
- mkdir-tree on ext4-dax-pmem: 2.97× ext4-guest-kernel's
- find-cold on fxfs-guest-dax: 2.06× ext4-dax-pmem's
