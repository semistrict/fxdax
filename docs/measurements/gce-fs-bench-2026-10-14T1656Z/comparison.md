Host: linux. Iterations: ext4-virtio-blk 3, ext4-guest-kernel 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-virtio-blk | ext4-guest-kernel | fxfs-guest-dax | ext4-guest-kernel vs ext4-virtio-blk | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|
| seq-write | MiB/s | 107 (96.3–107) | 107 (96.3–119) | 81.4 (81.1–88.1) | 1.00× | 1.31× |
| rand-read-cold | IOPS | 14,124 (13,170–14,173) | 13,980 (13,003–14,272) | 3,251 (3,248–3,259) | 1.01× | **4.30×** |
| seq-read-cold | MiB/s | 2,753 (2,723–3,413) | 3,200 (2,943–3,241) | 211 (207–211) | 0.86× | **15.18×** |
| seq-read-warm | MiB/s | 7,632 (7,093–7,638) | 7,633 (7,298–7,712) | 10,586 (10,354–10,614) | 1.00× | 0.72× |
| rand-read-warm | IOPS | 786,663 (771,074–829,580) | 816,197 (811,585–830,797) | 823,638 (820,704–830,675) | 0.96× | 0.99× |
| rand-write | IOPS | 6,372 (6,367–10,316) | 6,371 (6,330–6,373) | 1,548 (1,303–1,548) | 1.00× | **4.12×** |
| mmap-rand-read | IOPS | 1,197,283 (1,192,063–1,221,300) | 1,209,985 (1,194,540–1,225,308) | 1,096,065 (591,920–1,099,739) | 0.99× | 1.10× |
| mmap-rand-write | IOPS | 6,371 (6,336–6,382) | 6,380 (6,372–6,385) | 1,414 (1,301–1,416) | 1.00× | **4.51×** |
| fsync-append | IOPS | 287 (287–288) | 287 (287–287) | 221 (198–224) | 1.00× | 1.30× |
| create | ops/s | 76,154 (65,346–76,430) | 75,287 (72,175–75,582) | 753 (699–816) | 1.01× | **99.93×** |
| stat-cold | ops/s | 351,847 (259,303–353,628) | 358,464 (310,122–359,337) | 129,380 (125,968–130,190) | 0.98× | **2.77×** |
| stat-warm | ops/s | 1,396,276 (1,299,254–1,431,366) | 1,297,701 (1,237,079–1,341,035) | 1,265,315 (1,222,991–1,541,206) | 1.08× | 1.03× |
| lookup-missing | ops/s | 486,678 (478,457–488,227) | 495,598 (494,710–496,307) | 385,074 (356,373–398,041) | 0.98× | 1.29× |
| readdir | ops/s | 2,636,856 (2,548,527–3,022,034) | 2,439,652 (2,355,344–2,621,122) | 1,306,805 (1,200,183–1,401,174) | 1.08× | 1.87× |
| read-small-cold | ops/s | 13,366 (13,224–13,581) | 13,238 (13,134–13,252) | 2,466 (2,204–2,890) | 1.01× | **5.37×** |
| chmod | ops/s | 781,656 (743,137–782,300) | 719,306 (716,848–738,252) | 110,615 (96,175–122,404) | 1.09× | **6.50×** |
| setxattr | ops/s | 598,738 (580,142–621,340) | 530,995 (525,408–562,131) | 147,862 (119,661–151,272) | 1.13× | **3.59×** |
| link | ops/s | 114,790 (113,784–115,842) | 114,971 (114,243–115,047) | 39,171 (33,037–43,758) | 1.00× | **2.94×** |
| rename | ops/s | 76,947 (76,507–77,273) | 77,851 (76,660–78,017) | 25,220 (25,055–26,832) | 0.99× | **3.09×** |
| rename-across | ops/s | 66,886 (66,723–67,890) | 68,986 (68,501–69,068) | 21,938 (20,323–22,096) | 0.97× | **3.14×** |
| unlink | ops/s | 194,030 (192,272–195,883) | 197,197 (190,463–198,960) | 36,435 (26,080–39,268) | 0.98× | **5.41×** |
| create-fsync | ops/s | 389 (379–412) | 411 (387–425) | 217 (202–239) | 0.95× | 1.90× |
| mkdir-tree | ops/s | 91,627 (89,968–93,824) | 92,191 (89,268–92,336) | 996 (841–1,034) | 0.99× | **92.60×** |
| find-cold | ops/s | 325,706 (323,888–328,485) | 324,738 (324,732–327,407) | 127,265 (109,086–131,049) | 1.00× | **2.55×** |
| rm-tree | ops/s | 59,915 (58,812–60,323) | 59,773 (59,133–60,203) | 16,593 (15,966–19,321) | 1.00× | **3.60×** |
| large-dir-create | ops/s | 76,683 (66,735–79,853) | 77,373 (65,903–79,578) | 33,806 (29,816–35,262) | 0.99× | **2.29×** |
| large-dir-stat-cold | ops/s | 253,866 (210,594–279,758) | 219,604 (215,651–241,944) | 100,637 (99,634–104,848) | 1.16× | **2.18×** |
| large-dir-unlink | ops/s | 124,086 (123,857–124,523) | 123,365 (122,213–124,051) | 22,069 (21,580–23,053) | 1.01× | **5.59×** |
| create-parallel | ops/s | 125,699 (124,205–125,715) | 134,987 (129,081–136,253) | 38,106 (32,090–43,653) | 0.93× | **3.54×** |
| stat-parallel | ops/s | 646,784 (607,016–673,416) | 635,010 (625,394–668,667) | 632,344 (515,515–661,450) | 1.02× | 1.00× |
| unlink-parallel | ops/s | 207,279 (205,616–207,319) | 187,678 (187,408–188,487) | 23,248 (21,067–24,054) | 1.10× | **8.07×** |
| create-shared-dir | ops/s | 86,140 (85,475–86,589) | 85,874 (84,903–86,791) | 29,600 (26,282–33,358) | 1.00× | **2.90×** |
| unlink-shared-dir | ops/s | 162,298 (160,525–169,441) | 165,677 (159,393–168,722) | 19,477 (18,728–19,700) | 0.98× | **8.51×** |
| untar | s | 0.09 (0.09–0.10) | 0.10 (0.09–0.10) | 0.18 (0.18–0.21) | 1.01× | 1.95× |
| make | s | 6.76 (6.75–6.95) | 6.98 (6.93–7.43) | 7.54 (7.40–7.68) | 1.03× | 1.08× |
| build-total | s | 6.86 (6.84–7.04) | 7.07 (7.02–7.53) | 7.74 (7.58–7.89) | 1.03× | 1.09× |

- ext4-virtio-blk: mtime-ahead-of-guest-ms -3.49 median (-3.96–-2.93)
- ext4-guest-kernel: mtime-ahead-of-guest-ms -4.45 median (-6.22–-4.15)
- build failed in 1 of fxfs-guest-dax's iterations, which have no measurement.
- fxfs-guest-dax: mtime-ahead-of-guest-ms -11.55 median (-11.98–-5.12)

Over 2× slower than ext4:
- rand-read-cold on fxfs-guest-dax: 4.30× ext4-guest-kernel's
- seq-read-cold on fxfs-guest-dax: 15.18× ext4-guest-kernel's
- rand-write on fxfs-guest-dax: 4.12× ext4-guest-kernel's
- mmap-rand-write on fxfs-guest-dax: 4.51× ext4-guest-kernel's
- create on fxfs-guest-dax: 99.93× ext4-guest-kernel's
- stat-cold on fxfs-guest-dax: 2.77× ext4-guest-kernel's
- read-small-cold on fxfs-guest-dax: 5.37× ext4-guest-kernel's
- chmod on fxfs-guest-dax: 6.50× ext4-guest-kernel's
- setxattr on fxfs-guest-dax: 3.59× ext4-guest-kernel's
- link on fxfs-guest-dax: 2.94× ext4-guest-kernel's
- rename on fxfs-guest-dax: 3.09× ext4-guest-kernel's
- rename-across on fxfs-guest-dax: 3.14× ext4-guest-kernel's
- unlink on fxfs-guest-dax: 5.41× ext4-guest-kernel's
- mkdir-tree on fxfs-guest-dax: 92.60× ext4-guest-kernel's
- find-cold on fxfs-guest-dax: 2.55× ext4-guest-kernel's
- rm-tree on fxfs-guest-dax: 3.60× ext4-guest-kernel's
- large-dir-create on fxfs-guest-dax: 2.29× ext4-guest-kernel's
- large-dir-stat-cold on fxfs-guest-dax: 2.18× ext4-guest-kernel's
- large-dir-unlink on fxfs-guest-dax: 5.59× ext4-guest-kernel's
- create-parallel on fxfs-guest-dax: 3.54× ext4-guest-kernel's
- unlink-parallel on fxfs-guest-dax: 8.07× ext4-guest-kernel's
- create-shared-dir on fxfs-guest-dax: 2.90× ext4-guest-kernel's
- unlink-shared-dir on fxfs-guest-dax: 8.51× ext4-guest-kernel's
