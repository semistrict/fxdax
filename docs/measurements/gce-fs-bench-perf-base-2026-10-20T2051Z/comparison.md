Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| seq-write | MiB/s | 107 (107–107) | 65.8 (65.6–70.3) | 1.63× |
| rand-read-cold | IOPS | 74,473 (74,136–74,813) | 182,044 (110,703–195,048) | 0.41× |
| seq-read-cold | MiB/s | 445 (445–454) | 1,280 (711–1,407) | 0.35× |
| seq-read-warm | MiB/s | 11,382 (11,096–11,642) | 11,107 (11,049–11,435) | 1.02× |
| rand-read-warm | IOPS | 882,565 (860,055–884,155) | 805,218 (802,581–805,255) | 1.10× |
| rand-write | IOPS | 8,686 (8,682–8,686) | 11,816 (11,813–11,818) | 0.74× |
| mmap-rand-read | IOPS | 811,041 (804,761–830,870) | 1,152,769 (1,090,537–1,154,374) | 0.70× |
| mmap-rand-write | IOPS | 8,683 (7,649–8,688) | 11,822 (11,817–13,211) | 0.73× |
| fsync-append | IOPS | 346 (346–346) | 402 (399–653) | 0.86× |
| create | ops/s | 38,312 (33,926–38,395) | 25,827 (20,815–26,830) | 1.48× |
| stat-cold | ops/s | 334,939 (153,426–336,121) | 184,273 (171,817–185,470) | 1.82× |
| stat-warm | ops/s | 877,968 (870,313–1,308,649) | 1,500,050 (1,337,546–1,566,860) | 0.59× |
| lookup-missing | ops/s | 365,041 (352,907–378,725) | 367,978 (367,241–377,343) | 0.99× |
| readdir | ops/s | 1,272,736 (1,099,269–3,075,182) | 2,490,896 (2,363,351–2,642,381) | 0.51× |
| read-small-cold | ops/s | 54,216 (47,957–55,288) | 90,765 (90,445–95,078) | 0.60× |
| chmod | ops/s | 778,667 (778,662–837,229) | 1,123,003 (903,260–1,375,344) | 0.69× |
| setxattr | ops/s | 626,288 (624,680–639,724) | 571,467 (220,533–652,725) | 1.10× |
| link | ops/s | 156,692 (152,789–157,159) | 94,726 (91,700–100,265) | 1.65× |
| rename | ops/s | 124,360 (122,959–124,636) | 78,090 (75,635–79,174) | 1.59× |
| rename-across | ops/s | 103,889 (103,255–107,075) | 70,809 (67,798–71,187) | 1.47× |
| unlink | ops/s | 438,079 (426,106–441,379) | 160,614 (152,965–162,843) | **2.73×** |
| create-fsync | ops/s | 565 (546–569) | 1,731 (771–1,818) | 0.33× |
| mkdir-tree | ops/s | 42,896 (42,784–42,937) | 26,460 (9,855–28,075) | 1.62× |
| find-cold | ops/s | 318,027 (314,774–320,742) | 159,567 (140,592–170,238) | 1.99× |
| rm-tree | ops/s | 101,227 (99,599–109,772) | 63,346 (62,501–67,875) | 1.60× |
| large-dir-create | ops/s | 110,970 (82,386–112,191) | 84,494 (58,071–87,644) | 1.31× |
| large-dir-stat-cold | ops/s | 255,337 (138,169–263,279) | 183,753 (127,418–185,137) | 1.39× |
| large-dir-unlink | ops/s | 211,342 (209,765–211,708) | 225,283 (181,557–249,101) | 0.94× |
| create-parallel | ops/s | 177,249 (174,128–178,691) | 109,337 (108,571–109,961) | 1.62× |
| stat-parallel | ops/s | 650,679 (625,774–699,970) | 572,308 (504,541–598,766) | 1.14× |
| unlink-parallel | ops/s | 308,102 (297,051–322,452) | 187,978 (186,655–188,592) | 1.64× |
| create-shared-dir | ops/s | 110,107 (109,329–110,191) | 87,170 (83,399–88,414) | 1.26× |
| unlink-shared-dir | ops/s | 289,125 (276,994–297,659) | 187,158 (153,726–199,988) | 1.54× |
| pgbench-init | s | 3.18 (3.16–3.27) | 4.14 (3.82–4.34) | 1.30× |
| pgbench-tps | tps | 1,708 (1,701–1,735) | 1,425 (1,360–1,443) | 1.20× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -5.09 median (-5.16–-2.25)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.75 median (-5.99–-3.95)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 140 | B | $0.000056 |
| get-range | 18 | B | $0.000007 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 168 | A | $0.000840 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 332 | | $0.000928 |

Uploaded 826.5 MiB (segments 826.5 MiB, extents 0 B, checkpoints 13.2 KiB, moves 0 B, other 39 B); downloaded 3.2 KiB.

Over 2× slower than ext4:
- unlink on fxfs-guest-dax: 2.73× ext4-dax-pmem's
