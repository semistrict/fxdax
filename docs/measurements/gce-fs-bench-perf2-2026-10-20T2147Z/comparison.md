Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| seq-write | MiB/s | 107 (107–107) | 65.8 (65.6–70.3) | 1.63× |
| rand-read-cold | IOPS | 74,473 (73,802–75,502) | 128,000 (109,960–146,286) | 0.58× |
| seq-read-cold | MiB/s | 449 (447–452) | 856 (711–1,012) | 0.52× |
| seq-read-warm | MiB/s | 11,136 (11,100–11,225) | 11,050 (10,952–11,129) | 1.01× |
| rand-read-warm | IOPS | 883,400 (881,026–889,476) | 795,883 (793,077–804,935) | 1.11× |
| rand-write | IOPS | 8,681 (8,680–8,684) | 13,225 (11,816–14,421) | 0.66× |
| mmap-rand-read | IOPS | 805,780 (789,636–807,285) | 1,129,146 (1,086,730–1,135,123) | 0.71× |
| mmap-rand-write | IOPS | 8,683 (8,680–8,686) | 11,815 (11,813–11,816) | 0.73× |
| fsync-append | IOPS | 346 (346–346) | 397 (395–403) | 0.87× |
| create | ops/s | 38,256 (32,876–38,322) | 26,008 (25,917–26,741) | 1.47× |
| stat-cold | ops/s | 337,345 (156,704–339,949) | 187,727 (167,589–192,370) | 1.80× |
| stat-warm | ops/s | 1,136,021 (836,983–1,426,623) | 1,350,003 (1,282,407–1,462,110) | 0.84× |
| lookup-missing | ops/s | 431,831 (384,313–462,374) | 389,969 (323,697–483,048) | 1.11× |
| readdir | ops/s | 2,811,121 (957,756–3,065,275) | 2,354,559 (2,286,720–2,488,729) | 1.19× |
| read-small-cold | ops/s | 53,427 (53,270–54,265) | 93,041 (87,370–93,652) | 0.57× |
| chmod | ops/s | 608,947 (535,297–775,911) | 1,066,246 (848,308–1,192,140) | 0.57× |
| setxattr | ops/s | 513,959 (476,966–613,676) | 586,095 (304,839–638,476) | 0.88× |
| link | ops/s | 146,226 (143,750–151,638) | 92,417 (91,892–99,571) | 1.58× |
| rename | ops/s | 118,123 (116,220–124,038) | 74,433 (73,786–84,636) | 1.59× |
| rename-across | ops/s | 99,121 (95,945–106,344) | 66,167 (63,587–76,558) | 1.50× |
| unlink | ops/s | 355,910 (342,822–421,228) | 184,401 (156,381–272,587) | 1.93× |
| create-fsync | ops/s | 549 (543–599) | 743 (740–1,636) | 0.74× |
| mkdir-tree | ops/s | 42,823 (36,723–43,001) | 26,017 (24,660–27,044) | 1.65× |
| find-cold | ops/s | 316,529 (313,924–316,589) | 141,969 (133,571–167,531) | **2.23×** |
| rm-tree | ops/s | 103,745 (99,022–109,746) | 65,163 (64,105–69,742) | 1.59× |
| large-dir-create | ops/s | 110,694 (80,941–112,078) | 86,138 (56,692–90,732) | 1.29× |
| large-dir-stat-cold | ops/s | 260,067 (135,667–264,112) | 184,440 (178,921–186,423) | 1.41× |
| large-dir-unlink | ops/s | 208,764 (206,440–209,521) | 222,203 (214,114–227,969) | 0.94× |
| create-parallel | ops/s | 178,819 (176,955–180,951) | 108,598 (98,423–114,167) | 1.65× |
| stat-parallel | ops/s | 586,026 (584,979–587,266) | 507,123 (480,564–535,969) | 1.16× |
| unlink-parallel | ops/s | 312,389 (299,081–312,606) | 186,819 (185,247–197,161) | 1.67× |
| create-shared-dir | ops/s | 112,992 (111,780–113,117) | 85,043 (82,035–85,275) | 1.33× |
| unlink-shared-dir | ops/s | 282,058 (275,501–282,086) | 187,011 (177,057–200,222) | 1.51× |
| pgbench-init | s | 3.22 (3.19–3.31) | 4.18 (3.77–4.18) | 1.30× |
| pgbench-tps | tps | 1,771 (1,744–1,828) | 1,407 (1,399–1,438) | 1.26× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.13 median (-5.91–-2.39)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -6.69 median (-6.76–-4.91)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 144 | B | $0.000058 |
| get-range | 20 | B | $0.000008 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 176 | A | $0.000880 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 346 | | $0.000971 |

Uploaded 821.2 MiB (segments 821.2 MiB, extents 0 B, checkpoints 14.0 KiB, moves 0 B, other 39 B); downloaded 3.3 KiB.

Over 2× slower than ext4:
- find-cold on fxfs-guest-dax: 2.23× ext4-dax-pmem's
