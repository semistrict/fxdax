Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, ext4-pmem 3, ext4-dax-pmem 3, fxfs-guest 3, fxfs-guest-dax 3, fxfs-guest-dax-followed 3, fxfs-guest-dax-followed-idle 3.

| Workload | Unit | ext4-guest-kernel | ext4-pmem | ext4-dax-pmem | fxfs-guest | fxfs-guest-dax | fxfs-guest-dax-followed | fxfs-guest-dax-followed-idle | ext4-pmem vs ext4-guest-kernel | ext4-dax-pmem vs ext4-pmem | fxfs-guest vs ext4-dax-pmem | fxfs-guest-dax vs ext4-dax-pmem | fxfs-guest-dax-followed vs fxfs-guest-dax | fxfs-guest-dax-followed-idle vs fxfs-guest-dax |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| seq-write | MiB/s | 164 (120–184) | 123 (98.9–149) | 123 (122–126) | 140 (106–141) | 131 (106–140) | 135 (106–140) | 128 (106–139) | 1.33× | 1.00× | 0.88× | 0.94× | 0.97× | 1.02× |
| rand-read-cold | IOPS | 14,359 (13,885–14,616) | 65,016 (63,751–65,799) | 72,818 (72,496–74,813) | 5,061 (4,749–5,172) | 11,916 (11,138–11,924) | 75,852 (11,830–91,531) | 11,661 (11,473–12,038) | 0.22× | 0.89× | **14.39×** | **6.11×** | 0.16× | 1.02× |
| seq-read-cold | MiB/s | 3,122 (1,180–3,606) | 412 (308–420) | 449 (444–454) | 859 (686–871) | 393 (390–393) | 1,463 (398–1,497) | 396 (393–396) | **7.57×** | 0.92× | 0.52× | 1.14× | 0.27× | 0.99× |
| seq-read-warm | MiB/s | 8,089 (8,075–8,173) | 8,277 (8,238–8,288) | 11,017 (10,909–11,145) | 8,306 (8,305–8,382) | 10,905 (10,729–11,048) | 10,861 (10,679–10,909) | 11,089 (10,913–11,113) | 0.98× | 0.75× | 1.33× | 1.01× | 1.00× | 0.98× |
| rand-read-warm | IOPS | 918,774 (917,668–924,002) | 926,348 (924,367–930,306) | 870,944 (867,575–872,825) | 929,967 (926,953–930,575) | 835,467 (832,433–842,313) | 835,197 (830,947–841,104) | 839,818 (837,947–848,132) | 0.99× | 1.06× | 0.94× | 1.04× | 1.00× | 0.99× |
| mmap-seq-read | MiB/s | 9,846 (9,846–10,240) | 9,846 (9,481–9,846) | 10,240 (1,842–10,240) | 8,828 (8,828–9,143) | 9,846 (9,846–10,240) | 9,846 (9,846–9,846) | 9,846 (9,481–10,240) | 1.00× | 0.96× | 1.16× | 1.04× | 1.00× | 1.00× |
| rand-write | IOPS | 14,525 (10,422–14,589) | 11,620 (10,989–11,787) | 11,093 (9,471–13,496) | 72,496 (67,148–75,852) | 4,917 (4,872–4,947) | 4,992 (4,793–5,472) | 5,043 (4,991–5,060) | 1.25× | 1.05× | 0.15× | **2.26×** | 0.98× | 0.98× |
| mmap-rand-read | IOPS | 1,271,101 (1,262,934–1,274,017) | 1,226,590 (1,223,649–1,235,701) | 810,845 (784,879–867,558) | 1,229,986 (1,228,247–1,238,284) | 1,291,007 (1,286,269–1,291,159) | 776,851 (768,463–1,280,364) | 1,290,003 (1,276,901–1,294,684) | 1.04× | 1.51× | 0.66× | 0.63× | 1.66× | 1.00× |
| mmap-rand-write | IOPS | 7,699 (7,667–11,362) | 7,288 (6,204–7,866) | 11,078 (10,937–13,364) | 8,167 (7,703–8,605) | 59,362 (45,011–62,061) | 61,363 (45,011–64,000) | 59,796 (57,087–61,134) | 1.06× | 0.66× | 1.36× | 0.19× | 0.97× | 0.99× |
| fsync-append | IOPS | 390 (387–395) | 535 (530–536) | 533 (530–533) | 2,069 (1,473–2,325) | 2,643 (2,476–2,745) | 1,533 (1,438–2,346) | 2,052 (1,990–2,238) | 0.73× | 1.00× | 0.26× | 0.20× | 1.72× | 1.29× |
| sqlite-pread-scan | MiB/s | 2,661 (2,189–2,737) | 302 (293–310) | 283 (282–290) | 317 (299–350) | 161 (158–177) | 181 (158–566) | 159 (156–179) | **8.82×** | 1.07× | 0.89× | 1.76× | 0.89× | 1.01× |
| sqlite-pread-scan-warm | MiB/s | 2,986 (2,979–2,998) | 2,944 (2,912–2,981) | 2,815 (2,767–2,818) | 3,001 (2,988–3,076) | 2,789 (2,771–2,806) | 2,778 (2,763–2,789) | 2,799 (2,731–2,812) | 1.01× | 1.05× | 0.94× | 1.01× | 1.00× | 1.00× |
| sqlite-mmap-scan | MiB/s | 2,574 (2,098–2,697) | 2,606 (2,572–2,647) | 2,823 (1,887–2,845) | 789 (628–813) | 2,805 (2,794–2,874) | 2,862 (2,811–2,898) | 2,838 (2,787–2,864) | 0.99× | 0.92× | **3.58×** | 1.01× | 0.98× | 0.99× |
| sqlite-mmap-scan-warm | MiB/s | 2,942 (2,936–2,973) | 2,754 (2,647–2,760) | 2,902 (1,988–3,007) | 2,822 (1,819–2,892) | 2,977 (2,951–3,003) | 2,976 (2,974–3,024) | 3,005 (2,985–3,072) | 1.07× | 0.95× | 1.03× | 0.97× | 1.00× | 0.99× |
| create | ops/s | 97,659 (89,957–98,288) | 97,621 (93,710–98,914) | 37,706 (31,559–38,561) | 79,280 (74,120–79,614) | 79,262 (79,003–81,207) | 80,484 (79,410–80,736) | 80,275 (74,414–80,779) | 1.00× | **2.59×** | 0.48× | 0.48× | 0.98× | 0.99× |
| stat-cold | ops/s | 335,380 (221,297–337,625) | 328,491 (318,058–333,679) | 325,379 (151,118–330,367) | 176,958 (175,257–178,837) | 179,844 (175,246–182,488) | 172,121 (141,672–177,082) | 180,513 (178,802–180,985) | 1.02× | 1.01× | 1.84× | 1.81× | 1.04× | 1.00× |
| stat-warm | ops/s | 1,327,162 (1,099,847–1,358,714) | 1,112,178 (739,166–1,237,608) | 1,099,707 (648,651–1,245,804) | 1,089,993 (947,437–1,108,326) | 1,192,398 (1,157,941–1,200,907) | 1,082,330 (1,023,904–1,215,672) | 1,117,917 (798,608–1,211,851) | 1.19× | 1.01× | 1.01× | 0.92× | 1.10× | 1.07× |
| lookup-missing | ops/s | 455,904 (452,295–456,328) | 427,129 (387,916–429,383) | 428,794 (299,845–441,710) | 439,504 (385,770–447,202) | 502,095 (463,013–504,150) | 474,324 (451,094–500,165) | 433,346 (415,787–491,297) | 1.07× | 1.00× | 0.98× | 0.85× | 1.06× | 1.16× |
| readdir | ops/s | 2,919,538 (2,860,461–3,027,418) | 2,859,144 (934,535–3,118,242) | 2,973,867 (2,796,307–2,975,318) | 2,892,272 (2,845,209–3,106,521) | 3,037,506 (2,938,804–3,150,504) | 2,838,990 (2,388,827–3,158,794) | 2,930,622 (2,753,983–3,216,184) | 1.02× | 0.96× | 1.03× | 0.98× | 1.07× | 1.04× |
| read-small-cold | ops/s | 14,791 (11,554–15,275) | 50,426 (48,691–51,006) | 52,693 (52,240–54,440) | 6,359 (6,334–6,403) | 36,026 (34,565–36,411) | 33,946 (23,178–34,943) | 35,598 (34,757–36,545) | 0.29× | 0.96× | **8.29×** | 1.46× | 1.06× | 1.01× |
| chmod | ops/s | 681,293 (676,420–711,776) | 722,791 (705,513–783,353) | 717,517 (465,050–746,847) | 336,516 (335,882–394,876) | 376,006 (178,095–431,913) | 351,476 (337,440–357,280) | 379,958 (350,989–412,819) | 0.94× | 1.01× | **2.13×** | 1.91× | 1.07× | 0.99× |
| setxattr | ops/s | 551,469 (535,773–567,151) | 558,590 (542,453–594,994) | 575,228 (569,104–598,430) | 162,409 (161,426–176,289) | 171,463 (161,643–195,601) | 164,650 (160,032–174,045) | 175,127 (170,251–187,981) | 0.99× | 0.97× | **3.54×** | **3.35×** | 1.04× | 0.98× |
| link | ops/s | 150,818 (148,898–155,092) | 154,303 (148,506–154,439) | 152,268 (138,833–154,424) | 90,826 (86,937–90,868) | 92,292 (89,158–99,731) | 89,928 (81,700–90,768) | 95,380 (92,938–96,416) | 0.98× | 1.01× | 1.68× | 1.65× | 1.03× | 0.97× |
| rename | ops/s | 120,445 (119,287–121,067) | 115,945 (112,963–117,090) | 117,937 (111,567–119,803) | 70,824 (58,505–74,536) | 72,917 (71,099–77,851) | 72,012 (52,177–73,028) | 75,608 (73,132–75,987) | 1.04× | 0.98× | 1.67× | 1.62× | 1.01× | 0.96× |
| rename-across | ops/s | 96,989 (93,915–98,825) | 99,797 (95,601–102,651) | 97,938 (97,442–99,022) | 61,581 (59,570–64,529) | 59,886 (57,375–62,871) | 52,546 (50,099–58,742) | 63,199 (60,914–66,135) | 0.97× | 1.02× | 1.59× | 1.64× | 1.14× | 0.95× |
| unlink | ops/s | 374,831 (374,302–416,786) | 368,490 (353,609–393,970) | 383,918 (364,296–390,754) | 265,855 (193,217–293,296) | 263,481 (153,625–268,413) | 208,234 (200,534–234,270) | 230,727 (210,992–249,546) | 1.02× | 0.96× | 1.44× | 1.46× | 1.27× | 1.14× |
| create-fsync | ops/s | 367 (358–379) | 522 (504–529) | 536 (526–546) | 2,112 (2,033–2,423) | 2,343 (2,285–2,353) | 2,156 (1,439–2,221) | 1,824 (1,760–1,919) | 0.70× | 0.97× | 0.25× | 0.23× | 1.09× | 1.28× |
| mkdir-tree | ops/s | 126,092 (116,401–126,799) | 129,440 (128,567–130,481) | 41,560 (40,168–41,850) | 76,271 (75,447–77,171) | 77,465 (76,940–78,328) | 77,091 (72,316–77,753) | 76,075 (63,876–77,579) | 0.97× | **3.11×** | 0.54× | 0.54× | 1.00× | 1.02× |
| find-cold | ops/s | 309,101 (222,316–311,683) | 314,906 (314,806–318,044) | 309,886 (299,965–312,147) | 154,512 (113,851–172,035) | 154,284 (136,425–159,873) | 145,926 (119,286–158,019) | 151,630 (135,337–153,541) | 0.98× | 1.02× | **2.01×** | **2.01×** | 1.06× | 1.02× |
| rm-tree | ops/s | 101,116 (98,956–103,053) | 103,311 (102,318–104,578) | 100,378 (99,510–100,732) | 69,369 (65,968–69,463) | 66,847 (64,526–67,809) | 62,657 (62,396–66,572) | 64,725 (54,318–67,418) | 0.98× | 1.03× | 1.45× | 1.50× | 1.07× | 1.03× |
| large-dir-create | ops/s | 111,344 (107,838–111,609) | 110,159 (105,962–110,740) | 108,920 (77,821–109,751) | 87,949 (84,264–88,810) | 87,509 (83,765–88,358) | 87,929 (86,621–88,693) | 86,459 (86,409–89,602) | 1.01× | 1.01× | 1.24× | 1.24× | 1.00× | 1.01× |
| large-dir-stat-cold | ops/s | 263,162 (259,379–276,656) | 272,170 (260,969–278,355) | 229,421 (139,240–246,714) | 169,937 (119,851–172,528) | 166,159 (160,435–167,552) | 166,970 (158,541–167,259) | 167,664 (158,753–168,517) | 0.97× | 1.19× | 1.35× | 1.38× | 1.00× | 0.99× |
| large-dir-unlink | ops/s | 210,191 (207,532–212,605) | 207,621 (207,567–209,029) | 206,964 (206,505–207,164) | 159,004 (156,736–187,199) | 179,751 (134,929–184,339) | 170,478 (126,747–174,586) | 145,240 (131,748–188,293) | 1.01× | 1.00× | 1.30× | 1.15× | 1.05× | 1.24× |
| create-parallel | ops/s | 179,183 (175,525–180,602) | 175,643 (174,270–177,616) | 172,625 (129,563–181,139) | 119,215 (115,076–119,918) | 119,859 (117,183–126,291) | 119,649 (118,873–125,870) | 121,503 (115,535–123,524) | 1.02× | 1.02× | 1.45× | 1.44× | 1.00× | 0.99× |
| stat-parallel | ops/s | 554,482 (535,586–642,376) | 544,215 (532,219–606,701) | 532,089 (526,114–589,555) | 393,993 (265,670–416,070) | 404,616 (371,473–469,195) | 465,022 (352,953–485,127) | 461,135 (425,079–477,738) | 1.02× | 1.02× | 1.35× | 1.32× | 0.87× | 0.88× |
| unlink-parallel | ops/s | 301,997 (300,238–327,293) | 312,518 (296,990–322,373) | 300,443 (299,579–310,907) | 172,764 (162,721–173,018) | 170,885 (151,098–183,344) | 167,700 (158,541–169,136) | 163,911 (153,161–164,785) | 0.97× | 1.04× | 1.74× | 1.76× | 1.02× | 1.04× |
| create-shared-dir | ops/s | 110,516 (109,560–112,563) | 112,553 (110,482–113,606) | 108,281 (105,662–110,595) | 82,070 (78,887–88,990) | 86,111 (83,002–87,155) | 83,578 (75,731–83,915) | 85,266 (83,639–89,579) | 0.98× | 1.04× | 1.32× | 1.26× | 1.03× | 1.01× |
| unlink-shared-dir | ops/s | 281,263 (268,198–286,326) | 279,894 (270,729–281,750) | 279,158 (268,149–287,312) | 152,739 (143,694–153,356) | 137,808 (135,366–157,740) | 152,689 (125,287–156,924) | 146,653 (119,714–148,315) | 1.00× | 1.00× | 1.83× | **2.03×** | 0.90× | 0.94× |
| untar | s | 0.01 (0.01–0.02) | 0.01 (0.01–0.02) | 0.02 (0.02–0.02) | 0.02 (0.02–0.02) | 0.02 (0.02–0.02) | 0.02 (0.02–0.02) | 0.02 (0.02–0.03) | 1.00× | 1.13× | 0.94× | 0.94× | 1.00× | 1.25× |
| make | s | 4.60 (4.60–4.65) | 4.63 (4.62–4.63) | 4.65 (4.63–4.72) | 4.66 (4.64–4.67) | 4.63 (4.63–4.67) | 4.64 (4.64–4.65) | 4.81 (4.64–4.86) | 1.01× | 1.00× | 1.00× | 1.00× | 1.00× | 1.04× |
| build-total | s | 4.62 (4.62–4.67) | 4.64 (4.63–4.65) | 4.67 (4.64–4.74) | 4.67 (4.66–4.68) | 4.64 (4.64–4.69) | 4.66 (4.65–4.66) | 4.83 (4.66–4.88) | 1.01× | 1.00× | 1.00× | 1.00× | 1.00× | 1.04× |

- ext4-guest-kernel: mmap-seq-read-faults-per-mib 6.60 median (6.60–6.60)
- ext4-guest-kernel: mtime-ahead-of-guest-ms -5.57 median (-6.71–-5.13)
- ext4-guest-kernel: sqlite-mmap-scan-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-mmap-scan-warm-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- ext4-guest-kernel: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-pmem: mmap-seq-read-faults-per-mib 6.30 median (6.30–6.30)
- ext4-pmem: mtime-ahead-of-guest-ms -2.36 median (-3.52–-0.18)
- ext4-pmem: sqlite-mmap-scan-faults-per-mib 13.3 median (13.3–13.3)
- ext4-pmem: sqlite-mmap-scan-warm-faults-per-mib 11.9 median (11.9–11.9)
- ext4-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- ext4-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: mmap-seq-read-faults-per-mib 5.60 median (5.60–5.60)
- ext4-dax-pmem: mtime-ahead-of-guest-ms -6.45 median (-7.54–-5.74)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–129)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.70 median (3.60–129)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.60 median (2.60–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest: mmap-seq-read-faults-per-mib 13.7 median (13.7–13.7)
- fxfs-guest: mtime-ahead-of-guest-ms -5.41 median (-5.68–-3.98)
- fxfs-guest: sqlite-mmap-scan-faults-per-mib 9.60 median (9.60–9.60)
- fxfs-guest: sqlite-mmap-scan-warm-faults-per-mib 9.30 median (9.30–9.30)
- fxfs-guest: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.08 median (-5.90–-4.13)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax-followed: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax-followed: mtime-ahead-of-guest-ms -4.52 median (-5.48–-3.25)
- fxfs-guest-dax-followed: sqlite-mmap-scan-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax-followed: sqlite-mmap-scan-warm-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax-followed: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax-followed: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax-followed-idle: follower1-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax-followed-idle: mtime-ahead-of-guest-ms -3.85 median (-4.36–-1.56)
- fxfs-guest-dax-followed-idle: sqlite-mmap-scan-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax-followed-idle: sqlite-mmap-scan-warm-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax-followed-idle: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax-followed-idle: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 440 | B | $0.000176 |
| get-range | 9,231 | B | $0.003692 |
| head | 0 | B | $0.000000 |
| list | 20 | A | $0.000100 |
| create | 4,358 | A | $0.021790 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 14,052 | | $0.025768 |

Uploaded 7.8 GiB (segments 5.0 GiB, extents 2.8 GiB, checkpoints 37.7 KiB, moves 0 B, other 180 B); downloaded 5.6 GiB.

Followers: at most 2 at once, at most 385 commits and 619 ms behind their writer, keeping at most 384.0 MiB of its frees.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 8 | 4,023.7 µs |
| read | 127,730 | 63.6 µs |
| write | 160,310 | 527.6 µs |
| free | 125 | 12.3 µs |
| map | 149,580 | 60.9 µs |
| unmap | 3,809 | 101.1 µs |
| commit | 370 | 86,929.0 µs |
| root | 16 | 1.7 µs |
| info | 32 | 2.8 µs |
| window | 7 | 25.5 µs |
| fault | 96,673 | 244.2 µs |
| advance | 3,656 | 3.6 µs |
| let_go | 3,655 | 3.2 µs |
| pin | 5,913 | 2.1 µs |
| unpin | 5,261 | 2.1 µs |
| transfer | 8 | 12.9 µs |
| free_many | 552 | 1,743.1 µs |
| private | 147,456 | 7.9 µs |

Commits: 27,579, 3,268.9 µs each: committing the store 430.6 µs, waiting for object storage 2,838.0 µs.

Segments closed and framed: 451, 108.6 ms each.

Over 2× slower than ext4:
- rand-read-cold on fxfs-guest: 14.39× ext4-dax-pmem's
- rand-read-cold on fxfs-guest-dax: 6.11× ext4-dax-pmem's
- seq-read-cold on ext4-pmem: 7.57× ext4-guest-kernel's
- rand-write on fxfs-guest-dax: 2.26× ext4-dax-pmem's
- sqlite-pread-scan on ext4-pmem: 8.82× ext4-guest-kernel's
- sqlite-mmap-scan on fxfs-guest: 3.58× ext4-dax-pmem's
- create on ext4-dax-pmem: 2.59× ext4-pmem's
- read-small-cold on fxfs-guest: 8.29× ext4-dax-pmem's
- chmod on fxfs-guest: 2.13× ext4-dax-pmem's
- setxattr on fxfs-guest: 3.54× ext4-dax-pmem's
- setxattr on fxfs-guest-dax: 3.35× ext4-dax-pmem's
- mkdir-tree on ext4-dax-pmem: 3.11× ext4-pmem's
- find-cold on fxfs-guest: 2.01× ext4-dax-pmem's
- find-cold on fxfs-guest-dax: 2.01× ext4-dax-pmem's
- unlink-shared-dir on fxfs-guest-dax: 2.03× ext4-dax-pmem's
