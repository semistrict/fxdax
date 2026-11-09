Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, ext4-pmem 3, ext4-dax-pmem 3, fxfs-guest 3, fxfs-guest-dax 3, fxfs-guest-dax-followed 3, fxfs-guest-dax-followed-idle 3.

| Workload | Unit | ext4-guest-kernel | ext4-pmem | ext4-dax-pmem | fxfs-guest | fxfs-guest-dax | fxfs-guest-dax-followed | fxfs-guest-dax-followed-idle | ext4-pmem vs ext4-guest-kernel | ext4-dax-pmem vs ext4-pmem | fxfs-guest vs ext4-dax-pmem | fxfs-guest-dax vs ext4-dax-pmem | fxfs-guest-dax-followed vs fxfs-guest-dax | fxfs-guest-dax-followed-idle vs fxfs-guest-dax |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| seq-write | MiB/s | 158 (120–190) | 124 (98.3–145) | 124 (124–127) | 141 (106–141) | 127 (106–141) | 126 (108–140) | 132 (106–141) | 1.27× | 1.00× | 0.88× | 0.98× | 1.01× | 0.96× |
| rand-read-cold | IOPS | 29,574 (28,593–31,447) | 65,275 (61,134–67,702) | 73,802 (73,471–75,502) | 6,019 (5,648–6,088) | 12,136 (11,433–12,145) | 80,709 (11,645–93,091) | 12,056 (11,872–12,282) | 0.45× | 0.88× | **12.26×** | **6.08×** | 0.15× | 1.01× |
| seq-read-cold | MiB/s | 3,657 (1,076–3,821) | 422 (320–422) | 447 (443–462) | 959 (740–970) | 398 (391–399) | 1,446 (399–1,480) | 394 (393–398) | **8.67×** | 0.94× | 0.47× | 1.12× | 0.28× | 1.01× |
| seq-read-warm | MiB/s | 8,142 (8,060–8,183) | 8,255 (8,243–8,311) | 11,014 (10,828–11,059) | 8,263 (8,232–8,383) | 10,688 (10,666–10,861) | 10,928 (10,800–11,000) | 11,029 (10,860–11,172) | 0.99× | 0.75× | 1.33× | 1.03× | 0.98× | 0.97× |
| rand-read-warm | IOPS | 922,428 (918,551–927,180) | 922,415 (844,362–930,997) | 866,230 (802,059–868,158) | 930,501 (921,786–934,082) | 838,474 (832,541–844,912) | 844,648 (839,747–846,632) | 851,288 (833,102–852,069) | 1.00× | 1.06× | 0.93× | 1.03× | 0.99× | 0.98× |
| mmap-seq-read | MiB/s | 9,846 (9,481–9,846) | 9,846 (9,481–9,846) | 9,846 (9,143–10,240) | 8,828 (8,533–8,828) | 9,846 (9,846–9,846) | 10,240 (9,846–10,240) | 9,846 (9,846–10,240) | 1.00× | 1.00× | 1.12× | 1.00× | 0.96× | 1.00× |
| rand-write | IOPS | 15,227 (14,800–15,284) | 11,522 (10,221–12,083) | 11,040 (10,033–11,146) | 76,561 (65,016–80,314) | 5,984 (5,854–6,123) | 6,423 (5,306–7,083) | 6,001 (5,990–6,438) | 1.32× | 1.04× | 0.14× | 1.85× | 0.93× | 1.00× |
| mmap-rand-read | IOPS | 1,264,994 (1,264,748–1,282,160) | 1,239,709 (1,230,012–1,243,268) | 780,821 (767,891–782,580) | 1,223,991 (1,222,725–1,229,111) | 1,281,399 (1,274,522–1,295,100) | 920,709 (776,693–1,288,196) | 1,288,952 (1,285,163–1,295,866) | 1.02× | 1.59× | 0.64× | 0.61× | 1.39× | 0.99× |
| mmap-rand-write | IOPS | 8,346 (8,159–10,758) | 7,904 (7,427–7,923) | 10,967 (10,076–11,048) | 8,131 (7,663–8,246) | 61,594 (46,811–64,504) | 63,504 (45,385–64,000) | 64,000 (55,165–64,251) | 1.06× | 0.72× | 1.35× | 0.18× | 0.97× | 0.96× |
| fsync-append | IOPS | 436 (435–442) | 534 (530–535) | 534 (530–535) | 2,065 (1,744–2,586) | 3,402 (3,025–3,612) | 1,981 (1,673–2,312) | 2,653 (2,646–2,999) | 0.82× | 1.00× | 0.26× | 0.16× | 1.72× | 1.28× |
| sqlite-pread-scan | MiB/s | 2,743 (1,450–2,773) | 313 (262–315) | 293 (285–293) | 338 (303–414) | 160 (159–178) | 171 (157–606) | 159 (157–163) | **8.75×** | 1.07× | 0.87× | 1.83× | 0.93× | 1.00× |
| sqlite-pread-scan-warm | MiB/s | 3,024 (3,022–3,045) | 2,916 (2,853–2,952) | 2,807 (2,807–2,822) | 3,042 (3,015–3,080) | 2,778 (2,753–2,787) | 2,810 (2,750–2,831) | 2,753 (2,749–2,754) | 1.04× | 1.04× | 0.92× | 1.01× | 0.99× | 1.01× |
| sqlite-mmap-scan | MiB/s | 2,613 (1,294–2,683) | 2,620 (2,433–2,642) | 2,879 (1,915–2,883) | 874 (873–884) | 2,875 (2,829–2,876) | 2,874 (2,816–2,964) | 2,858 (2,855–2,906) | 1.00× | 0.91× | **3.29×** | 1.00× | 1.00× | 1.01× |
| sqlite-mmap-scan-warm | MiB/s | 2,893 (2,890–2,995) | 2,734 (2,728–2,739) | 2,995 (2,024–2,999) | 2,884 (2,790–2,916) | 2,923 (2,855–2,980) | 2,950 (2,947–2,976) | 2,992 (2,960–2,997) | 1.06× | 0.91× | 1.04× | 1.02× | 0.99× | 0.98× |
| create | ops/s | 98,931 (94,780–99,026) | 98,559 (96,147–98,803) | 38,076 (33,821–38,702) | 79,141 (78,917–79,489) | 78,168 (74,598–78,188) | 77,999 (69,238–78,238) | 79,048 (76,471–80,809) | 1.00× | **2.59×** | 0.48× | 0.49× | 1.00× | 0.99× |
| stat-cold | ops/s | 331,119 (191,984–334,576) | 330,970 (325,248–336,468) | 323,571 (148,104–329,032) | 182,527 (173,614–196,635) | 177,938 (168,955–181,808) | 176,262 (170,592–183,493) | 169,497 (167,430–187,591) | 1.00× | 1.02× | 1.77× | 1.82× | 1.01× | 1.05× |
| stat-warm | ops/s | 839,859 (730,959–1,248,392) | 740,545 (728,820–1,359,113) | 1,163,024 (1,106,801–1,192,159) | 1,128,544 (849,201–1,159,897) | 844,564 (840,794–1,258,569) | 1,124,892 (984,760–1,180,063) | 832,953 (702,609–1,009,573) | 1.13× | 0.64× | 1.03× | 1.38× | 0.75× | 1.01× |
| lookup-missing | ops/s | 435,646 (381,147–438,466) | 439,386 (439,116–439,685) | 429,839 (301,799–433,017) | 437,502 (392,621–447,934) | 501,009 (458,021–502,761) | 483,590 (408,690–492,895) | 469,771 (408,612–486,947) | 0.99× | 1.02× | 0.98× | 0.86× | 1.04× | 1.07× |
| readdir | ops/s | 3,077,027 (2,925,666–3,080,577) | 2,828,610 (961,079–2,932,018) | 2,867,334 (2,743,970–2,998,002) | 3,004,036 (2,790,424–3,148,397) | 2,836,984 (2,775,207–3,091,548) | 3,026,877 (2,708,643–3,172,866) | 2,822,296 (2,605,554–3,030,895) | 1.09× | 0.99× | 0.95× | 1.01× | 0.94× | 1.01× |
| read-small-cold | ops/s | 25,971 (25,944–26,326) | 50,158 (49,672–51,854) | 53,187 (47,603–53,886) | 8,268 (8,212–8,618) | 36,415 (34,123–36,659) | 34,381 (23,261–35,297) | 35,404 (35,022–35,873) | 0.52× | 0.94× | **6.43×** | 1.46× | 1.06× | 1.03× |
| chmod | ops/s | 681,235 (680,562–681,262) | 717,679 (693,483–834,615) | 708,433 (686,171–728,624) | 330,078 (321,722–393,980) | 369,354 (346,419–415,645) | 362,178 (353,548–392,626) | 353,901 (346,460–387,108) | 0.95× | 1.01× | **2.15×** | 1.92× | 1.02× | 1.04× |
| setxattr | ops/s | 549,310 (541,464–559,707) | 571,171 (569,254–614,053) | 561,296 (549,350–590,414) | 172,666 (161,899–182,281) | 184,829 (165,427–194,057) | 168,147 (166,247–169,151) | 166,695 (165,842–174,150) | 0.96× | 1.02× | **3.25×** | **3.04×** | 1.10× | 1.11× |
| link | ops/s | 151,879 (140,444–154,990) | 157,291 (156,692–158,657) | 150,634 (149,788–155,459) | 92,667 (85,873–94,118) | 96,244 (88,809–97,118) | 93,609 (74,543–93,716) | 91,023 (87,635–94,118) | 0.97× | 1.04× | 1.63× | 1.57× | 1.03× | 1.06× |
| rename | ops/s | 119,062 (118,298–119,867) | 116,528 (110,151–117,726) | 116,135 (114,209–116,464) | 69,435 (61,274–75,306) | 77,334 (72,652–77,352) | 73,664 (57,001–76,187) | 72,621 (59,881–75,369) | 1.02× | 1.00× | 1.67× | 1.50× | 1.05× | 1.06× |
| rename-across | ops/s | 100,935 (100,234–101,156) | 100,417 (97,091–102,120) | 98,997 (98,237–100,025) | 63,358 (57,683–66,946) | 63,840 (56,465–65,056) | 51,264 (48,236–61,640) | 62,287 (60,577–65,902) | 1.01× | 1.01× | 1.56× | 1.55× | 1.25× | 1.02× |
| unlink | ops/s | 393,737 (380,461–394,618) | 406,741 (397,431–423,584) | 379,766 (338,205–401,081) | 232,094 (205,765–250,061) | 251,465 (200,851–252,804) | 201,744 (153,460–283,813) | 257,239 (196,533–259,039) | 0.97× | 1.07× | 1.64× | 1.51× | 1.25× | 0.98× |
| create-fsync | ops/s | 439 (420–445) | 552 (515–584) | 545 (517–573) | 2,739 (2,500–3,097) | 2,921 (2,900–3,032) | 2,005 (1,931–2,062) | 2,372 (2,102–2,444) | 0.79× | 1.01× | 0.20× | 0.19× | 1.46× | 1.23× |
| mkdir-tree | ops/s | 128,339 (110,324–128,470) | 129,718 (129,213–130,149) | 42,419 (36,298–43,450) | 76,432 (75,156–77,139) | 77,466 (77,355–79,638) | 75,556 (68,863–76,948) | 77,988 (71,046–78,290) | 0.99× | **3.06×** | 0.55× | 0.55× | 1.03× | 0.99× |
| find-cold | ops/s | 317,616 (315,481–318,991) | 317,716 (315,405–321,708) | 309,671 (138,586–312,644) | 162,116 (114,845–165,164) | 150,023 (143,907–150,071) | 151,893 (117,447–154,761) | 148,473 (123,518–154,967) | 1.00× | 1.03× | 1.91× | **2.06×** | 0.99× | 1.01× |
| rm-tree | ops/s | 107,091 (105,653–115,185) | 107,403 (103,003–111,286) | 109,344 (106,260–113,424) | 70,303 (68,190–70,653) | 66,905 (64,599–67,923) | 66,377 (53,425–69,550) | 67,218 (65,457–69,087) | 1.00× | 0.98× | 1.56× | 1.63× | 1.01× | 1.00× |
| large-dir-create | ops/s | 111,640 (111,526–111,702) | 110,619 (108,744–111,303) | 103,613 (82,862–110,065) | 88,679 (87,532–88,758) | 89,537 (88,818–89,676) | 87,662 (69,232–88,617) | 86,629 (86,584–89,313) | 1.01× | 1.07× | 1.17× | 1.16× | 1.02× | 1.03× |
| large-dir-stat-cold | ops/s | 265,906 (240,273–267,011) | 260,444 (257,204–279,421) | 252,528 (210,509–267,817) | 166,597 (119,851–175,041) | 160,355 (158,806–168,701) | 156,708 (136,898–168,807) | 163,828 (120,755–164,234) | 1.02× | 1.03× | 1.52× | 1.57× | 1.02× | 0.98× |
| large-dir-unlink | ops/s | 210,082 (208,852–210,538) | 207,517 (207,431–208,172) | 201,996 (200,853–203,052) | 181,498 (142,604–184,141) | 137,507 (128,408–188,103) | 174,204 (134,585–187,515) | 176,317 (137,417–189,186) | 1.01× | 1.03× | 1.11× | 1.47× | 0.79× | 0.78× |
| create-parallel | ops/s | 175,892 (171,979–184,608) | 179,842 (178,187–182,142) | 175,002 (174,751–175,182) | 116,981 (114,846–120,551) | 124,826 (124,094–125,206) | 124,184 (121,938–124,309) | 113,835 (112,408–121,532) | 0.98× | 1.03× | 1.50× | 1.40× | 1.01× | 1.10× |
| stat-parallel | ops/s | 616,689 (540,970–642,078) | 609,516 (555,295–625,736) | 591,723 (517,616–597,380) | 412,856 (321,231–445,529) | 476,069 (427,881–484,046) | 377,165 (357,611–415,921) | 425,312 (338,918–433,944) | 1.01× | 1.03× | 1.43× | 1.24× | 1.26× | 1.12× |
| unlink-parallel | ops/s | 305,192 (302,652–311,611) | 310,572 (310,569–318,234) | 291,427 (278,235–295,643) | 163,792 (155,031–165,781) | 168,582 (166,073–176,318) | 162,315 (155,325–168,471) | 173,755 (162,423–181,121) | 0.98× | 1.07× | 1.78× | 1.73× | 1.04× | 0.97× |
| create-shared-dir | ops/s | 112,714 (110,078–113,892) | 108,233 (106,549–115,337) | 108,224 (107,525–109,054) | 86,182 (76,704–88,978) | 83,604 (81,447–87,128) | 84,296 (78,905–88,862) | 89,022 (84,492–90,568) | 1.04× | 1.00× | 1.26× | 1.29× | 0.99× | 0.94× |
| unlink-shared-dir | ops/s | 272,191 (261,084–287,282) | 273,705 (271,776–280,982) | 269,540 (268,037–287,587) | 169,914 (150,921–174,613) | 154,313 (123,008–160,790) | 162,018 (128,149–165,522) | 156,965 (153,583–175,898) | 0.99× | 1.02× | 1.59× | 1.75× | 0.95× | 0.98× |
| untar | s | 0.01 (0.01–0.01) | 0.01 (0.01–0.01) | 0.01 (0.01–0.02) | 0.01 (0.01–0.01) | 0.01 (0.01–0.02) | 0.02 (0.01–0.02) | 0.01 (0.01–0.02) | 1.00× | 1.07× | 1.00× | 1.00× | 1.13× | 1.00× |
| make | s | 4.58 (4.57–4.60) | 4.63 (4.57–4.64) | 4.67 (4.64–4.72) | 4.61 (4.58–4.61) | 4.60 (4.59–4.64) | 4.59 (4.59–4.61) | 4.63 (4.58–4.63) | 1.01× | 1.01× | 0.99× | 0.98× | 1.00× | 1.01× |
| build-total | s | 4.60 (4.59–4.61) | 4.64 (4.59–4.65) | 4.69 (4.66–4.74) | 4.62 (4.59–4.63) | 4.62 (4.61–4.66) | 4.61 (4.61–4.62) | 4.65 (4.60–4.65) | 1.01× | 1.01× | 0.99× | 0.98× | 1.00× | 1.01× |

- ext4-guest-kernel: mmap-seq-read-faults-per-mib 6.60 median (6.60–6.60)
- ext4-guest-kernel: mtime-ahead-of-guest-ms -3.91 median (-4.21–-3.08)
- ext4-guest-kernel: sqlite-mmap-scan-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-mmap-scan-warm-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- ext4-guest-kernel: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-pmem: mmap-seq-read-faults-per-mib 6.30 median (6.30–6.30)
- ext4-pmem: mtime-ahead-of-guest-ms -6.41 median (-6.80–-4.01)
- ext4-pmem: sqlite-mmap-scan-faults-per-mib 13.3 median (13.3–13.3)
- ext4-pmem: sqlite-mmap-scan-warm-faults-per-mib 11.9 median (11.9–11.9)
- ext4-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- ext4-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.70)
- ext4-dax-pmem: mmap-seq-read-faults-per-mib 5.60 median (5.60–5.60)
- ext4-dax-pmem: mtime-ahead-of-guest-ms -1.86 median (-4.31–-0.76)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–129)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–129)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.60 median (2.60–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest: mmap-seq-read-faults-per-mib 13.7 median (13.7–13.8)
- fxfs-guest: mtime-ahead-of-guest-ms -5.84 median (-6.10–-5.18)
- fxfs-guest: sqlite-mmap-scan-faults-per-mib 9.60 median (9.60–9.60)
- fxfs-guest: sqlite-mmap-scan-warm-faults-per-mib 9.30 median (9.30–9.30)
- fxfs-guest: sqlite-pread-scan-faults-per-mib 2.60 median (2.60–2.70)
- fxfs-guest: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.65 median (-5.67–-3.53)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.70)
- fxfs-guest-dax-followed: follower1-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed: follower1-raced-reads 11,964,620 median (11,964,620–11,964,620)
- fxfs-guest-dax-followed: follower1-reads 755,186 median (755,186–755,186)
- fxfs-guest-dax-followed: follower2-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed: follower2-raced-reads 11,855,820 median (11,855,820–11,855,820)
- fxfs-guest-dax-followed: follower2-reads 752,696 median (752,696–752,696)
- fxfs-guest-dax-followed: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax-followed: mtime-ahead-of-guest-ms -5.07 median (-5.92–-1.29)
- fxfs-guest-dax-followed: sqlite-mmap-scan-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax-followed: sqlite-mmap-scan-warm-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax-followed: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax-followed: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax-followed-idle: follower1-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax-followed-idle: mtime-ahead-of-guest-ms -6.42 median (-6.57–-5.49)
- fxfs-guest-dax-followed-idle: sqlite-mmap-scan-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax-followed-idle: sqlite-mmap-scan-warm-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax-followed-idle: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax-followed-idle: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.70)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 434 | B | $0.000174 |
| get-range | 9,434 | B | $0.003774 |
| head | 0 | B | $0.000000 |
| list | 20 | A | $0.000100 |
| create | 4,207 | A | $0.021035 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 14,098 | | $0.025092 |

Uploaded 7.6 GiB (segments 4.8 GiB, extents 2.9 GiB, checkpoints 38.0 KiB, moves 0 B, other 180 B); downloaded 5.6 GiB.

Followers: at most 2 at once, at most 2841 commits and 50,770 ms behind their writer, keeping at most 1.3 GiB of its frees.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 8 | 4,104.9 µs |
| read | 127,264 | 61.2 µs |
| write | 160,317 | 519.9 µs |
| free | 124 | 13.0 µs |
| map | 147,309 | 58.9 µs |
| unmap | 3,740 | 97.2 µs |
| commit | 368 | 87,802.4 µs |
| root | 16 | 1.7 µs |
| info | 32 | 2.8 µs |
| window | 7 | 28.1 µs |
| fault | 106,925 | 218.6 µs |
| advance | 2,855 | 3.1 µs |
| let_go | 2,853 | 2.8 µs |
| pin | 1,816 | 6.9 µs |
| unpin | 1,164 | 0.6 µs |
| transfer | 8 | 14.0 µs |
| free_many | 563 | 1,720.4 µs |
| private | 147,456 | 7.9 µs |

Commits: 27,582, 3,229.7 µs each: committing the store 427.5 µs, waiting for object storage 2,801.8 µs.

Segments closed and framed: 448, 108.2 ms each.

Over 2× slower than ext4:
- rand-read-cold on fxfs-guest: 12.26× ext4-dax-pmem's
- rand-read-cold on fxfs-guest-dax: 6.08× ext4-dax-pmem's
- seq-read-cold on ext4-pmem: 8.67× ext4-guest-kernel's
- sqlite-pread-scan on ext4-pmem: 8.75× ext4-guest-kernel's
- sqlite-mmap-scan on fxfs-guest: 3.29× ext4-dax-pmem's
- create on ext4-dax-pmem: 2.59× ext4-pmem's
- read-small-cold on fxfs-guest: 6.43× ext4-dax-pmem's
- chmod on fxfs-guest: 2.15× ext4-dax-pmem's
- setxattr on fxfs-guest: 3.25× ext4-dax-pmem's
- setxattr on fxfs-guest-dax: 3.04× ext4-dax-pmem's
- mkdir-tree on ext4-dax-pmem: 3.06× ext4-pmem's
- find-cold on fxfs-guest-dax: 2.06× ext4-dax-pmem's
