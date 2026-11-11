Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | fxfs-guest | fxfs-guest-dax | fxfs-guest vs ext4-guest-kernel | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|
| scale-rand-read-cold-j1 | IOPS | 42,667 (40,157–43,574) | 6,804 (6,041–7,186) | 8,715 (8,533–9,143) | **6.27×** | **4.90×** |
| scale-rand-read-cold-j1-p50 | us | 19.1 (18.3–20.1) | 52.0 (52.0–55.0) | 4.70 (4.70–4.83) | **2.72×** | 0.25× |
| scale-rand-read-cold-j1-p99 | us | 54.5 (50.4–67.1) | 2,900 (2,769–3,555) | 3,097 (2,572–3,228) | **53.18×** | **56.79×** |
| scale-rand-read-warm-j1 | IOPS | 929,302 (826,418–942,854) | 749,499 (735,909–778,457) | 637,694 (613,994–650,578) | 1.24× | 1.46× |
| scale-rand-read-warm-j1-p50 | us | 0.88 (0.86–0.97) | 1.13 (1.06–1.14) | 1.34 (1.32–1.34) | 1.29× | 1.53× |
| scale-rand-read-warm-j1-p99 | us | 1.40 (1.34–1.86) | 1.72 (1.70–1.72) | 1.83 (1.77–2.35) | 1.23× | 1.31× |
| scale-rand-read-cold-j2 | IOPS | 51,200 (7,969–61,134) | 11,409 (11,040–11,473) | 14,577 (14,577–16,926) | **4.49×** | **3.51×** |
| scale-rand-read-cold-j2-p50 | us | 34.6 (31.4–45.8) | 66.0 (59.1–69.1) | 4.38 (4.08–4.64) | 1.91× | 0.13× |
| scale-rand-read-cold-j2-p99 | us | 202 (87.6–5,538) | 3,490 (3,359–3,686) | 3,555 (3,097–3,588) | **17.30×** | **17.62×** |
| scale-rand-read-warm-j2 | IOPS | 1,784,179 (1,610,320–1,793,727) | 1,497,010 (1,493,503–1,556,708) | 1,259,792 (1,061,581–1,262,919) | 1.19× | 1.42× |
| scale-rand-read-warm-j2-p50 | us | 0.93 (0.92–1.00) | 1.13 (1.08–1.13) | 1.35 (1.35–1.61) | 1.21× | 1.45× |
| scale-rand-read-warm-j2-p99 | us | 1.45 (1.32–1.86) | 1.66 (1.56–1.74) | 1.96 (1.85–2.26) | 1.14× | 1.35× |
| scale-rand-read-cold-j4 | IOPS | 63,015 (18,327–65,016) | 13,107 (12,450–14,173) | 31,752 (27,959–32,508) | **4.81×** | 1.98× |
| scale-rand-read-cold-j4-p50 | us | 60.2 (59.6–78.3) | 121 (111–142) | 4.32 (4.19–4.38) | **2.02×** | 0.07× |
| scale-rand-read-cold-j4-p99 | us | 130 (112–758) | 5,472 (5,407–5,603) | 3,064 (2,769–3,195) | **42.25×** | **23.65×** |
| scale-rand-read-warm-j4 | IOPS | 3,346,090 (3,150,419–3,348,642) | 2,874,778 (2,575,854–2,896,922) | 2,466,357 (2,194,489–2,491,987) | 1.16× | 1.36× |
| scale-rand-read-warm-j4-p50 | us | 1.00 (1.00–1.02) | 1.16 (1.13–1.27) | 1.37 (1.37–1.51) | 1.16× | 1.37× |
| scale-rand-read-warm-j4-p99 | us | 1.42 (1.40–1.86) | 1.83 (1.75–1.91) | 1.98 (1.86–2.22) | 1.29× | 1.40× |
| scale-rand-read-cold-j8 | IOPS | 51,200 (15,298–52,345) | 15,017 (14,161–15,241) | 33,574 (32,444–37,926) | **3.41×** | 1.53× |
| scale-rand-read-cold-j8-p50 | us | 153 (151–234) | 230 (206–232) | 4.38 (4.26–4.64) | 1.51× | 0.03× |
| scale-rand-read-cold-j8-p99 | us | 281 (259–2,671) | 9,372 (9,110–9,896) | 5,472 (4,555–5,538) | **33.40×** | **19.50×** |
| scale-rand-read-warm-j8 | IOPS | 4,721,830 (4,576,139–4,728,918) | 4,352,169 (4,275,870–4,372,548) | 3,636,231 (3,476,199–3,647,848) | 1.08× | 1.30× |
| scale-rand-read-warm-j8-p50 | us | 1.38 (1.38–1.42) | 1.50 (1.50–1.53) | 1.82 (1.82–1.82) | 1.08× | 1.31× |
| scale-rand-read-warm-j8-p99 | us | 1.93 (1.91–2.02) | 2.06 (2.04–2.10) | 2.45 (2.45–4.38) | 1.07× | 1.27× |
| scale-rand-read-cold-qd16 | IOPS | 89,043 (85,333–93,091) | 32,000 (32,000–32,508) | 512,000 (512,000–512,000) | **2.78×** | 0.17× |
| scale-rand-read-cold-qd16-p50 | us | 138 (111–138) | 453 (444–457) | 17.3 (17.0–17.8) | **3.27×** | 0.12× |
| scale-rand-read-cold-qd16-p99 | us | 635 (408–651) | 1,204 (1,090–1,253) | 89.6 (83.5–123) | 1.90× | 0.14× |
| scale-rand-write-j1 | IOPS | 36,571 (29,257–44,522) | 60,235 (56,889–68,267) | 12,800 (12,642–13,299) | 0.61× | **2.86×** |
| scale-rand-write-j1-p50 | us | 2.54 (1.78–2.70) | 1.80 (1.77–1.85) | 61.7 (60.7–62.7) | 0.71× | **24.25×** |
| scale-rand-write-j1-p99 | us | 8.26 (4.64–8.90) | 4.64 (4.32–4.64) | 110 (109–130) | 0.56× | **13.33×** |
| scale-rand-write-unread-j1 | IOPS | 27,676 (17,655–40,960) | 64,000 (51,200–78,769) | 11,636 (10,240–12,190) | 0.43× | **2.38×** |
| scale-rand-write-unread-j1-p50 | us | 3.22 (2.16–5.86) | 2.22 (2.22–2.35) | 45.3 (43.8–45.3) | 0.69× | **14.09×** |
| scale-rand-write-unread-j1-p99 | us | 7.71 (6.43–16.2) | 10.6 (6.37–12.5) | 416 (375–489) | 1.37× | **53.91×** |
| scale-fsync-append-j1 | IOPS | 552 (122–566) | 5,020 (4,163–5,505) | 3,580 (2,829–3,879) | 0.11× | 0.15× |
| scale-fsync-append-j1-p50 | us | 14.5 (7.58–30.6) | 4.90 (4.58–5.09) | 5.73 (5.34–7.26) | 0.34× | 0.39× |
| scale-fsync-append-j1-p99 | us | 35.1 (25.7–77.3) | 9.28 (7.97–16.2) | 21.4 (14.7–21.4) | 0.26× | 0.61× |
| scale-fsync-append-j1-fsync-p50 | us | 1,729 (1,614–7,569) | 175 (167–183) | 210 (196–255) | 0.10× | 0.12× |
| scale-fsync-append-j1-fsync-p99 | us | 6,980 (2,245–13,435) | 342 (313–1,044) | 1,090 (473–1,450) | 0.05× | 0.16× |
| scale-create-j1 | ops/s | 60,096 (50,710–61,234) | 54,923 (52,606–57,119) | 53,809 (51,609–54,269) | 1.09× | 1.12× |
| scale-stat-cold-j1 | ops/s | 161,767 (158,867–165,415) | 113,236 (98,757–123,437) | 110,794 (110,470–116,143) | 1.43× | 1.46× |
| scale-unlink-j1 | ops/s | 121,748 (121,067–137,317) | 188,118 (137,505–204,564) | 178,783 (153,507–189,526) | 0.65× | 0.68× |
| scale-rand-write-j2 | IOPS | 31,508 (29,681–48,762) | 107,789 (107,789–128,000) | 24,094 (23,540–24,381) | 0.29× | 1.31× |
| scale-rand-write-j2-p50 | us | 2.38 (2.19–3.63) | 2.26 (2.26–2.32) | 47.9 (44.3–48.4) | 0.95× | **20.08×** |
| scale-rand-write-j2-p99 | us | 11.3 (10.6–14.7) | 11.2 (9.15–11.5) | 132 (131–142) | 0.99× | **11.66×** |
| scale-rand-write-unread-j2 | IOPS | 31,508 (21,787–47,628) | 120,471 (113,778–128,000) | 21,113 (18,286–21,333) | 0.26× | 1.49× |
| scale-rand-write-unread-j2-p50 | us | 2.22 (2.13–3.28) | 2.26 (2.22–2.29) | 47.9 (47.9–53.0) | 1.01× | **21.53×** |
| scale-rand-write-unread-j2-p99 | us | 10.8 (10.4–20.4) | 11.2 (11.1–11.5) | 659 (578–725) | 1.04× | **60.97×** |
| scale-fsync-append-j2 | IOPS | 615 (171–869) | 5,069 (4,063–5,120) | 4,016 (3,908–5,535) | 0.12× | 0.15× |
| scale-fsync-append-j2-p50 | us | 64.8 (15.4–80.4) | 5.79 (5.73–5.98) | 6.18 (6.05–6.88) | 0.09× | 0.10× |
| scale-fsync-append-j2-p99 | us | 259 (107–301) | 17.0 (16.1–17.3) | 21.4 (18.6–24.4) | 0.07× | 0.08× |
| scale-fsync-append-j2-fsync-p50 | us | 3,162 (1,729–13,042) | 309 (305–322) | 293 (281–317) | 0.10× | 0.09× |
| scale-fsync-append-j2-fsync-p99 | us | 3,850 (3,621–25,559) | 1,974 (1,057–2,310) | 2,007 (1,434–2,933) | 0.51× | 0.52× |
| scale-create-j2 | ops/s | 101,334 (98,810–103,618) | 71,687 (68,754–90,611) | 89,798 (79,288–93,708) | 1.41× | 1.13× |
| scale-stat-cold-j2 | ops/s | 313,196 (310,978–334,637) | 229,203 (213,378–234,713) | 217,346 (205,446–222,625) | 1.37× | 1.44× |
| scale-unlink-j2 | ops/s | 246,212 (236,287–260,039) | 205,803 (198,275–215,153) | 200,685 (187,616–204,272) | 1.20× | 1.23× |
| scale-rand-write-j4 | IOPS | 50,568 (29,681–53,195) | 163,840 (163,840–178,087) | 33,574 (31,267–34,712) | 0.31× | 1.51× |
| scale-rand-write-j4-p50 | us | 2.64 (2.51–2.93) | 2.67 (2.64–2.77) | 91.6 (84.5–94.7) | 1.01× | **34.72×** |
| scale-rand-write-j4-p99 | us | 12.7 (8.90–14.0) | 12.1 (9.92–12.6) | 247 (239–285) | 0.95× | **19.38×** |
| scale-rand-write-unread-j4 | IOPS | 63,015 (17,732–64,000) | 146,286 (128,000–157,538) | 19,412 (19,230–20,177) | 0.43× | **3.25×** |
| scale-rand-write-unread-j4-p50 | us | 2.61 (2.48–3.47) | 2.70 (2.70–2.77) | 78.3 (77.3–79.4) | 1.04× | **30.04×** |
| scale-rand-write-unread-j4-p99 | us | 10.4 (8.77–17.0) | 13.5 (12.7–15.0) | 4,424 (4,227–4,882) | 1.29× | **424.05×** |
| scale-fsync-append-j4 | IOPS | 1,143 (515–1,236) | 5,936 (5,432–6,006) | 5,721 (4,911–5,885) | 0.19× | 0.20× |
| scale-fsync-append-j4-p50 | us | 72.2 (70.1–86.5) | 7.33 (6.75–7.33) | 7.46 (7.39–7.52) | 0.10× | 0.10× |
| scale-fsync-append-j4-p99 | us | 241 (142–289) | 18.3 (17.3–19.6) | 18.0 (17.3–20.1) | 0.08× | 0.07× |
| scale-fsync-append-j4-fsync-p50 | us | 3,129 (3,129–4,293) | 510 (494–537) | 545 (528–545) | 0.16× | 0.17× |
| scale-fsync-append-j4-fsync-p99 | us | 7,438 (3,654–13,828) | 3,817 (3,129–7,111) | 3,326 (3,064–5,997) | 0.51× | 0.45× |
| scale-create-j4 | ops/s | 170,329 (161,764–172,966) | 118,500 (113,008–120,427) | 117,990 (116,946–125,576) | 1.44× | 1.44× |
| scale-stat-cold-j4 | ops/s | 532,418 (486,950–557,005) | 379,671 (257,265–423,473) | 345,734 (308,538–403,793) | 1.40× | 1.54× |
| scale-unlink-j4 | ops/s | 413,531 (383,474–503,470) | 201,298 (193,353–209,649) | 202,053 (199,858–202,339) | **2.05×** | **2.05×** |
| scale-rand-write-j8 | IOPS | 45,011 (18,164–51,200) | 146,286 (122,269–170,667) | 26,006 (24,165–26,597) | 0.31× | 1.73× |
| scale-rand-write-j8-p50 | us | 2.70 (2.64–4.05) | 2.86 (2.86–2.90) | 253 (251–259) | 1.06× | **93.54×** |
| scale-rand-write-j8-p99 | us | 12.9 (9.92–18.3) | 16.2 (15.7–19.3) | 569 (457–602) | 1.26× | **44.26×** |
| scale-rand-write-unread-j8 | IOPS | 46,022 (18,703–48,188) | 138,847 (136,533–190,512) | 15,398 (14,787–16,351) | 0.33× | **2.99×** |
| scale-rand-write-unread-j8-p50 | us | 3.06 (2.80–4.32) | 2.80 (2.80–2.83) | 181 (181–194) | 0.92× | **59.31×** |
| scale-rand-write-unread-j8-p99 | us | 13.2 (9.41–17.3) | 14.9 (14.0–17.0) | 13,828 (13,304–14,352) | 1.13× | **1043.79×** |
| scale-fsync-append-j8 | IOPS | 2,210 (888–2,326) | 5,278 (5,114–5,777) | 5,333 (3,073–5,543) | 0.42× | 0.41× |
| scale-fsync-append-j8-p50 | us | 76.3 (62.2–84.5) | 7.58 (7.01–7.78) | 7.84 (7.65–7.90) | 0.10× | 0.10× |
| scale-fsync-append-j8-p99 | us | 285 (142–317) | 22.4 (19.1–23.2) | 23.7 (23.2–27.8) | 0.08× | 0.08× |
| scale-fsync-append-j8-fsync-p50 | us | 3,359 (3,293–13,435) | 791 (766–815) | 848 (815–856) | 0.24× | 0.25× |
| scale-fsync-append-j8-fsync-p99 | us | 5,276 (3,850–15,925) | 9,241 (9,241–10,158) | 8,716 (7,176–9,634) | 1.75× | 1.65× |
| scale-create-j8 | ops/s | 241,254 (239,611–249,495) | 110,157 (102,722–114,872) | 109,864 (105,450–117,920) | **2.19×** | **2.20×** |
| scale-stat-cold-j8 | ops/s | 698,344 (677,928–863,311) | 472,417 (424,720–483,891) | 545,032 (514,606–616,457) | 1.48× | 1.28× |
| scale-unlink-j8 | ops/s | 697,606 (649,322–713,257) | 180,711 (175,238–188,959) | 175,467 (173,702–181,115) | **3.86×** | **3.98×** |

- ext4-guest-kernel: mtime-ahead-of-guest-ms -6.01 median (-6.29–-3.82)
- fxfs-guest: mtime-ahead-of-guest-ms -4.47 median (-4.75–-3.40)
- fxfs-guest: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-create-j4-queues-used 0.00 median (0.00–4.00)
- fxfs-guest: scale-create-j8-cross-cpu-pct 0.40 median (0.20–0.40)
- fxfs-guest: scale-create-j8-interrupts-per-request 0.24 median (0.20–0.35)
- fxfs-guest: scale-create-j8-queues-used 2.00 median (1.00–4.00)
- fxfs-guest: scale-fsync-append-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j1-queues-used 4.00 median (3.00–5.00)
- fxfs-guest: scale-fsync-append-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j2-queues-used 5.00 median (5.00–6.00)
- fxfs-guest: scale-fsync-append-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j4-queues-used 8.00 median (8.00–8.00)
- fxfs-guest: scale-fsync-append-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j8-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j8-queues-used 8.00 median (8.00–8.00)
- fxfs-guest: scale-rand-read-cold-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j2-queues-used 5.00 median (4.00–7.00)
- fxfs-guest: scale-rand-read-cold-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j4-queues-used 8.00 median (8.00–8.00)
- fxfs-guest: scale-rand-read-cold-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j8-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest: scale-rand-read-cold-j8-queues-used 8.00 median (8.00–8.00)
- fxfs-guest: scale-rand-read-cold-qd16-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-qd16-interrupts-per-request 0.86 median (0.86–0.87)
- fxfs-guest: scale-rand-read-cold-qd16-queues-used 8.00 median (8.00–8.00)
- fxfs-guest: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j1-interrupts-per-request 0.68 median (0.49–0.76)
- fxfs-guest: scale-rand-write-j1-queues-used 3.00 median (3.00–3.00)
- fxfs-guest: scale-rand-write-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j2-interrupts-per-request 0.63 median (0.50–0.76)
- fxfs-guest: scale-rand-write-j2-queues-used 4.00 median (4.00–5.00)
- fxfs-guest: scale-rand-write-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j4-interrupts-per-request 0.53 median (0.42–0.57)
- fxfs-guest: scale-rand-write-j4-queues-used 6.00 median (6.00–8.00)
- fxfs-guest: scale-rand-write-j8-cross-cpu-pct 0.30 median (0.30–1.10)
- fxfs-guest: scale-rand-write-j8-interrupts-per-request 0.46 median (0.46–0.53)
- fxfs-guest: scale-rand-write-j8-queues-used 8.00 median (8.00–8.00)
- fxfs-guest: scale-rand-write-unread-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-unread-j1-interrupts-per-request 0.78 median (0.52–0.84)
- fxfs-guest: scale-rand-write-unread-j1-queues-used 3.00 median (2.00–4.00)
- fxfs-guest: scale-rand-write-unread-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-unread-j2-interrupts-per-request 0.62 median (0.56–0.74)
- fxfs-guest: scale-rand-write-unread-j2-queues-used 4.00 median (3.00–5.00)
- fxfs-guest: scale-rand-write-unread-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-unread-j4-interrupts-per-request 0.50 median (0.49–0.57)
- fxfs-guest: scale-rand-write-unread-j4-queues-used 7.00 median (7.00–8.00)
- fxfs-guest: scale-rand-write-unread-j8-cross-cpu-pct 0.40 median (0.40–0.40)
- fxfs-guest: scale-rand-write-unread-j8-interrupts-per-request 0.52 median (0.49–0.57)
- fxfs-guest: scale-rand-write-unread-j8-queues-used 8.00 median (8.00–8.00)
- fxfs-guest: scale-stat-cold-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.05 median (-5.70–-1.58)
- fxfs-guest-dax: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-create-j4-queues-used 0.00 median (0.00–3.00)
- fxfs-guest-dax: scale-create-j8-cross-cpu-pct 0.00 median (0.00–1.00)
- fxfs-guest-dax: scale-create-j8-interrupts-per-request 0.38 median (0.23–0.40)
- fxfs-guest-dax: scale-create-j8-queues-used 1.00 median (1.00–8.00)
- fxfs-guest-dax: scale-fsync-append-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j1-interrupts-per-request 1.00 median (0.99–1.00)
- fxfs-guest-dax: scale-fsync-append-j1-queues-used 5.00 median (4.00–6.00)
- fxfs-guest-dax: scale-fsync-append-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j2-queues-used 7.00 median (7.00–7.00)
- fxfs-guest-dax: scale-fsync-append-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j4-queues-used 8.00 median (7.00–8.00)
- fxfs-guest-dax: scale-fsync-append-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j8-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j8-queues-used 8.00 median (8.00–8.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-read-cold-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j8-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-read-cold-qd16-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j1-interrupts-per-request 0.04 median (0.03–0.04)
- fxfs-guest-dax: scale-rand-write-j1-queues-used 3.00 median (3.00–4.00)
- fxfs-guest-dax: scale-rand-write-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j2-interrupts-per-request 0.09 median (0.09–0.10)
- fxfs-guest-dax: scale-rand-write-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-write-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j4-interrupts-per-request 0.33 median (0.20–0.48)
- fxfs-guest-dax: scale-rand-write-j4-queues-used 7.00 median (7.00–8.00)
- fxfs-guest-dax: scale-rand-write-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j8-interrupts-per-request 0.97 median (0.97–0.97)
- fxfs-guest-dax: scale-rand-write-j8-queues-used 8.00 median (8.00–8.00)
- fxfs-guest-dax: scale-rand-write-unread-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j1-interrupts-per-request 0.07 median (0.07–0.08)
- fxfs-guest-dax: scale-rand-write-unread-j1-queues-used 3.00 median (2.00–4.00)
- fxfs-guest-dax: scale-rand-write-unread-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j2-interrupts-per-request 0.08 median (0.08–0.09)
- fxfs-guest-dax: scale-rand-write-unread-j2-queues-used 4.00 median (4.00–5.00)
- fxfs-guest-dax: scale-rand-write-unread-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j4-interrupts-per-request 0.23 median (0.21–0.26)
- fxfs-guest-dax: scale-rand-write-unread-j4-queues-used 7.00 median (6.00–8.00)
- fxfs-guest-dax: scale-rand-write-unread-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j8-interrupts-per-request 0.98 median (0.97–0.98)
- fxfs-guest-dax: scale-rand-write-unread-j8-queues-used 8.00 median (8.00–8.00)
- fxfs-guest-dax: scale-stat-cold-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-stat-cold-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-stat-cold-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-stat-cold-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-unlink-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-unlink-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-unlink-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-unlink-j8-queues-used 0.00 median (0.00–0.00)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 238 | B | $0.000095 |
| get-range | 9,072 | B | $0.003629 |
| head | 0 | B | $0.000000 |
| list | 16 | A | $0.000080 |
| create | 3,776 | A | $0.018880 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 13,105 | | $0.022694 |

Uploaded 6.2 GiB (segments 3.9 GiB, extents 2.3 GiB, checkpoints 230.4 KiB, moves 0 B, other 96 B); downloaded 5.7 GiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 20,636.4 µs |
| read | 177,184 | 80.0 µs |
| write | 106,406 | 558.4 µs |
| free | 71 | 22.4 µs |
| map | 2,256 | 46.4 µs |
| unmap | 1,128 | 107.6 µs |
| commit | 212 | 59,982.2 µs |
| root | 2 | 41.0 µs |
| info | 8 | 6.1 µs |
| window | 1 | 782.9 µs |
| fault | 17,352 | 404.1 µs |
| transfer | 2 | 13.1 µs |
| free_many | 434 | 1,728.5 µs |
| private | 81,978 | 95.5 µs |

Commits: 29,223, 2,246.9 µs each: committing the store 432.0 µs, waiting for object storage 1,814.8 µs.

Segments closed and framed: 287, 30.9 ms each.

Over 2× slower than ext4:
- scale-rand-read-cold-j1 on fxfs-guest: 6.27× ext4-guest-kernel's
- scale-rand-read-cold-j1 on fxfs-guest-dax: 4.90× ext4-guest-kernel's
- scale-rand-read-cold-j1-p50 on fxfs-guest: 2.72× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest: 53.18× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest-dax: 56.79× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest: 4.49× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest-dax: 3.51× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest: 17.30× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest-dax: 17.62× ext4-guest-kernel's
- scale-rand-read-cold-j4 on fxfs-guest: 4.81× ext4-guest-kernel's
- scale-rand-read-cold-j4-p50 on fxfs-guest: 2.02× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest: 42.25× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest-dax: 23.65× ext4-guest-kernel's
- scale-rand-read-cold-j8 on fxfs-guest: 3.41× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest: 33.40× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest-dax: 19.50× ext4-guest-kernel's
- scale-rand-read-cold-qd16 on fxfs-guest: 2.78× ext4-guest-kernel's
- scale-rand-read-cold-qd16-p50 on fxfs-guest: 3.27× ext4-guest-kernel's
- scale-rand-write-j1 on fxfs-guest-dax: 2.86× ext4-guest-kernel's
- scale-rand-write-j1-p50 on fxfs-guest-dax: 24.25× ext4-guest-kernel's
- scale-rand-write-j1-p99 on fxfs-guest-dax: 13.33× ext4-guest-kernel's
- scale-rand-write-unread-j1 on fxfs-guest-dax: 2.38× ext4-guest-kernel's
- scale-rand-write-unread-j1-p50 on fxfs-guest-dax: 14.09× ext4-guest-kernel's
- scale-rand-write-unread-j1-p99 on fxfs-guest-dax: 53.91× ext4-guest-kernel's
- scale-rand-write-j2-p50 on fxfs-guest-dax: 20.08× ext4-guest-kernel's
- scale-rand-write-j2-p99 on fxfs-guest-dax: 11.66× ext4-guest-kernel's
- scale-rand-write-unread-j2-p50 on fxfs-guest-dax: 21.53× ext4-guest-kernel's
- scale-rand-write-unread-j2-p99 on fxfs-guest-dax: 60.97× ext4-guest-kernel's
- scale-rand-write-j4-p50 on fxfs-guest-dax: 34.72× ext4-guest-kernel's
- scale-rand-write-j4-p99 on fxfs-guest-dax: 19.38× ext4-guest-kernel's
- scale-rand-write-unread-j4 on fxfs-guest-dax: 3.25× ext4-guest-kernel's
- scale-rand-write-unread-j4-p50 on fxfs-guest-dax: 30.04× ext4-guest-kernel's
- scale-rand-write-unread-j4-p99 on fxfs-guest-dax: 424.05× ext4-guest-kernel's
- scale-unlink-j4 on fxfs-guest: 2.05× ext4-guest-kernel's
- scale-unlink-j4 on fxfs-guest-dax: 2.05× ext4-guest-kernel's
- scale-rand-write-j8-p50 on fxfs-guest-dax: 93.54× ext4-guest-kernel's
- scale-rand-write-j8-p99 on fxfs-guest-dax: 44.26× ext4-guest-kernel's
- scale-rand-write-unread-j8 on fxfs-guest-dax: 2.99× ext4-guest-kernel's
- scale-rand-write-unread-j8-p50 on fxfs-guest-dax: 59.31× ext4-guest-kernel's
- scale-rand-write-unread-j8-p99 on fxfs-guest-dax: 1043.79× ext4-guest-kernel's
- scale-create-j8 on fxfs-guest: 2.19× ext4-guest-kernel's
- scale-create-j8 on fxfs-guest-dax: 2.20× ext4-guest-kernel's
- scale-unlink-j8 on fxfs-guest: 3.86× ext4-guest-kernel's
- scale-unlink-j8 on fxfs-guest-dax: 3.98× ext4-guest-kernel's
