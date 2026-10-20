Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| seq-write | MiB/s | 107 (107–107) | 70.1 (65.8–70.3) | 1.53× |
| rand-read-cold | IOPS | 75,502 (74,136–75,852) | 136,533 (109,960–153,121) | 0.55× |
| seq-read-cold | MiB/s | 452 (448–452) | 911 (707–1,058) | 0.50× |
| seq-read-warm | MiB/s | 11,179 (10,642–11,466) | 10,937 (10,923–11,217) | 1.02× |
| rand-read-warm | IOPS | 883,654 (883,351–885,227) | 792,203 (789,398–800,938) | 1.12× |
| rand-write | IOPS | 8,677 (7,669–8,685) | 11,816 (11,808–11,822) | 0.73× |
| mmap-rand-read | IOPS | 797,747 (792,125–806,979) | 1,129,825 (1,112,076–1,130,802) | 0.71× |
| mmap-rand-write | IOPS | 8,679 (7,669–8,683) | 11,821 (11,814–11,823) | 0.73× |
| fsync-append | IOPS | 346 (346–346) | 402 (402–403) | 0.86× |
| create | ops/s | 38,489 (32,624–38,561) | 26,114 (23,220–27,208) | 1.47× |
| stat-cold | ops/s | 340,110 (147,679–343,593) | 194,987 (136,201–197,078) | 1.74× |
| stat-warm | ops/s | 944,066 (840,258–1,161,245) | 1,339,996 (1,325,113–1,387,221) | 0.70× |
| lookup-missing | ops/s | 378,409 (369,945–463,238) | 401,773 (356,175–443,796) | 0.94× |
| readdir | ops/s | 933,450 (899,575–2,975,911) | 2,392,256 (2,195,166–2,445,652) | 0.39× |
| read-small-cold | ops/s | 54,071 (49,703–54,355) | 91,571 (91,271–94,665) | 0.59× |
| chmod | ops/s | 736,831 (637,150–831,584) | 1,259,881 (846,678–1,278,490) | 0.58× |
| setxattr | ops/s | 644,778 (610,680–650,714) | 611,266 (516,965–618,731) | 1.05× |
| link | ops/s | 151,159 (145,781–156,518) | 95,097 (91,016–101,589) | 1.59× |
| rename | ops/s | 123,545 (119,622–124,017) | 77,645 (72,249–78,673) | 1.59× |
| rename-across | ops/s | 104,589 (103,389–105,404) | 65,295 (64,660–70,858) | 1.60× |
| unlink | ops/s | 395,186 (350,690–413,910) | 181,729 (173,253–205,326) | **2.17×** |
| create-fsync | ops/s | 541 (510–550) | 1,745 (730–1,789) | 0.31× |
| mkdir-tree | ops/s | 42,744 (42,637–43,228) | 28,373 (8,506–28,702) | 1.51× |
| find-cold | ops/s | 312,203 (160,965–318,907) | 143,909 (133,001–169,241) | **2.17×** |
| rm-tree | ops/s | 99,516 (97,840–103,756) | 65,101 (64,318–66,474) | 1.53× |
| large-dir-create | ops/s | 111,066 (79,271–111,176) | 80,672 (59,353–91,725) | 1.38× |
| large-dir-stat-cold | ops/s | 241,540 (239,652–255,168) | 184,360 (179,079–188,064) | 1.31× |
| large-dir-unlink | ops/s | 204,098 (181,037–208,680) | 217,247 (187,139–223,933) | 0.94× |
| create-parallel | ops/s | 174,113 (172,232–174,996) | 111,995 (104,681–124,199) | 1.55× |
| stat-parallel | ops/s | 647,829 (617,445–704,557) | 486,787 (460,029–545,926) | 1.33× |
| unlink-parallel | ops/s | 305,859 (296,205–308,109) | 192,024 (162,393–193,506) | 1.59× |
| create-shared-dir | ops/s | 112,367 (111,648–112,496) | 88,909 (85,607–91,893) | 1.26× |
| unlink-shared-dir | ops/s | 286,382 (271,169–287,766) | 167,430 (140,339–177,115) | 1.71× |
| pgbench-init | s | 3.21 (3.20–3.38) | 4.11 (3.81–4.18) | 1.28× |
| pgbench-tps | tps | 1,692 (1,683–1,707) | 1,441 (1,428–1,444) | 1.17× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -3.72 median (-5.76–-3.71)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.95 median (-99.49–-4.81)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 141 | B | $0.000056 |
| get-range | 17 | B | $0.000007 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 171 | A | $0.000855 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 335 | | $0.000943 |

Uploaded 821.4 MiB (segments 821.3 MiB, extents 0 B, checkpoints 14.1 KiB, moves 0 B, other 39 B); downloaded 3.2 KiB.

Over 2× slower than ext4:
- unlink on fxfs-guest-dax: 2.17× ext4-dax-pmem's
- find-cold on fxfs-guest-dax: 2.17× ext4-dax-pmem's
