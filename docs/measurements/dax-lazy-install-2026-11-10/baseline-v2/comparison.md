Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | fxfs-guest | fxfs-guest-dax | fxfs-guest vs ext4-guest-kernel | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|
| scale-rand-read-cold-j1 | IOPS | 40,157 (35,310–41,796) | 7,787 (7,288–7,847) | 8,192 (8,031–8,225) | **5.16×** | **4.90×** |
| scale-rand-read-cold-j1-p50 | us | 19.6 (19.3–20.4) | 43.8 (40.2–44.8) | 4.32 (4.26–4.51) | **2.24×** | 0.22× |
| scale-rand-read-cold-j1-p99 | us | 134 (37.6–214) | 2,671 (2,638–2,867) | 2,671 (2,540–3,031) | **19.91×** | **19.91×** |
| scale-rand-read-warm-j1 | IOPS | 843,015 (835,475–857,904) | 820,447 (797,660–859,639) | 723,594 (712,637–723,618) | 1.03× | 1.17× |
| scale-rand-read-warm-j1-p50 | us | 0.98 (0.96–1.00) | 1.02 (0.97–1.05) | 1.18 (1.18–1.21) | 1.04× | 1.20× |
| scale-rand-read-warm-j1-p99 | us | 1.45 (1.37–1.53) | 1.48 (1.35–1.53) | 1.58 (1.51–1.58) | 1.02× | 1.09× |
| scale-rand-read-cold-j2 | IOPS | 57,690 (56,889–60,235) | 13,128 (12,337–13,299) | 16,319 (15,515–17,504) | **4.39×** | **3.54×** |
| scale-rand-read-cold-j2-p50 | us | 29.8 (29.3–31.6) | 50.9 (45.8–51.5) | 3.82 (3.82–4.02) | 1.71× | 0.13× |
| scale-rand-read-cold-j2-p99 | us | 61.2 (58.6–81.4) | 2,834 (2,703–2,933) | 2,933 (2,834–2,966) | **46.33×** | **47.93×** |
| scale-rand-read-warm-j2 | IOPS | 1,472,437 (1,228,216–1,604,023) | 1,599,352 (1,175,328–1,710,477) | 1,413,036 (1,143,466–1,429,578) | 0.92× | 1.04× |
| scale-rand-read-warm-j2-p50 | us | 1.10 (1.05–1.32) | 1.05 (0.97–1.40) | 1.21 (1.19–1.53) | 0.96× | 1.10× |
| scale-rand-read-warm-j2-p99 | us | 1.74 (1.58–1.77) | 1.48 (1.46–1.88) | 1.69 (1.50–1.98) | 0.85× | 0.97× |
| scale-rand-read-cold-j4 | IOPS | 61,134 (61,134–62,061) | 20,078 (18,124–21,059) | 28,544 (27,769–33,992) | **3.04×** | **2.14×** |
| scale-rand-read-cold-j4-p50 | us | 63.7 (62.7–64.3) | 72.2 (71.2–76.3) | 3.92 (3.89–3.95) | 1.13× | 0.06× |
| scale-rand-read-cold-j4-p99 | us | 98.8 (95.7–108) | 3,555 (2,998–4,014) | 2,703 (2,638–2,867) | **35.98×** | **27.36×** |
| scale-rand-read-warm-j4 | IOPS | 1,617,471 (1,596,111–1,649,818) | 1,552,802 (1,540,664–1,621,572) | 1,392,828 (1,373,238–1,406,117) | 1.04× | 1.16× |
| scale-rand-read-warm-j4-p50 | us | 1.03 (1.00–1.03) | 1.08 (1.02–1.10) | 1.24 (1.22–1.26) | 1.05× | 1.20× |
| scale-rand-read-warm-j4-p99 | us | 1.53 (1.48–1.74) | 1.50 (1.45–1.62) | 1.80 (1.61–1.85) | 0.98× | 1.18× |
| scale-rand-read-cold-j8 | IOPS | 61,363 (51,361–63,504) | 23,953 (19,458–26,903) | 31,752 (31,208–35,617) | **2.56×** | 1.93× |
| scale-rand-read-cold-j8-p50 | us | 129 (125–153) | 120 (112–126) | 3.86 (3.82–3.95) | 0.94× | 0.03× |
| scale-rand-read-cold-j8-p99 | us | 183 (177–367) | 6,128 (5,341–7,373) | 4,817 (4,620–5,341) | **33.43×** | **26.28×** |
| scale-rand-read-warm-j8 | IOPS | 1,591,454 (1,560,909–1,613,102) | 1,567,352 (1,554,817–1,605,695) | 1,417,073 (1,383,602–1,430,605) | 1.02× | 1.12× |
| scale-rand-read-warm-j8-p50 | us | 1.05 (1.03–1.08) | 1.08 (1.05–1.08) | 1.21 (1.21–1.24) | 1.03× | 1.15× |
| scale-rand-read-warm-j8-p99 | us | 1.53 (1.53–1.56) | 1.56 (1.53–1.61) | 1.61 (1.53–1.64) | 1.02× | 1.05× |
| scale-rand-read-cold-qd16 | IOPS | 102,400 (89,043–107,789) | 40,960 (34,133–43,574) | 682,667 (409,600–682,667) | **2.50×** | 0.15× |
| scale-rand-read-cold-qd16-p50 | us | 122 (119–144) | 362 (326–440) | 16.5 (16.2–24.7) | **2.96×** | 0.13× |
| scale-rand-read-cold-qd16-p99 | us | 494 (461–510) | 848 (791–963) | 105 (89.6–155) | 1.72× | 0.21× |
| scale-rand-write-j1 | IOPS | 32,000 (19,321–46,545) | 60,235 (60,235–60,235) | 15,284 (14,423–17,356) | 0.53× | **2.09×** |
| scale-rand-write-j1-p50 | us | 1.83 (1.74–1.83) | 1.74 (1.66–2.32) | 49.4 (43.8–50.9) | 0.95× | **26.97×** |
| scale-rand-write-j1-p99 | us | 5.28 (5.15–5.34) | 4.26 (4.19–6.56) | 89.6 (80.4–110) | 0.81× | **16.97×** |
| scale-rand-write-unread-j1 | IOPS | 31,030 (28,444–44,522) | 68,267 (56,889–68,267) | 13,128 (10,557–13,653) | 0.45× | **2.36×** |
| scale-rand-write-unread-j1-p50 | us | 2.22 (2.16–2.22) | 2.22 (2.16–3.12) | 37.1 (34.6–37.1) | 1.00× | **16.69×** |
| scale-rand-write-unread-j1-p99 | us | 8.90 (7.90–11.2) | 9.92 (5.79–10.0) | 379 (379–602) | 1.12× | **42.59×** |
| scale-fsync-append-j1 | IOPS | 145 (145–648) | 4,531 (4,197–6,169) | 5,885 (4,876–6,400) | 0.03× | 0.02× |
| scale-fsync-append-j1-p50 | us | 20.9 (8.51–25.7) | 4.45 (4.08–4.96) | 4.13 (3.95–4.26) | 0.21× | 0.20× |
| scale-fsync-append-j1-p99 | us | 35.1 (27.8–48.9) | 15.3 (12.2–18.6) | 9.54 (9.41–14.1) | 0.44× | 0.27× |
| scale-fsync-append-j1-fsync-p50 | us | 6,849 (1,516–6,849) | 142 (142–161) | 146 (138–146) | 0.02× | 0.02× |
| scale-fsync-append-j1-fsync-p99 | us | 7,307 (1,745–7,569) | 1,004 (367–2,834) | 281 (272–946) | 0.14× | 0.04× |
| scale-create-j1 | ops/s | 56,717 (52,959–59,848) | 59,139 (57,266–63,812) | 62,856 (59,356–65,539) | 0.96× | 0.90× |
| scale-stat-cold-j1 | ops/s | 157,489 (154,985–167,751) | 130,844 (130,230–143,740) | 125,133 (116,420–127,404) | 1.20× | 1.26× |
| scale-unlink-j1 | ops/s | 127,381 (124,292–128,111) | 163,554 (127,625–176,699) | 179,109 (176,256–180,419) | 0.78× | 0.71× |
| scale-rand-write-j2 | IOPS | 25,924 (20,687–46,545) | 120,471 (102,400–128,000) | 31,508 (30,118–33,032) | 0.22× | 0.82× |
| scale-rand-write-j2-p50 | us | 2.29 (2.26–2.70) | 2.26 (2.22–3.89) | 40.7 (39.7–40.7) | 0.99× | **17.79×** |
| scale-rand-write-j2-p99 | us | 7.58 (6.18–9.92) | 10.6 (9.28–10.8) | 90.6 (81.4–118) | 1.39× | **11.95×** |
| scale-rand-write-unread-j2 | IOPS | 47,628 (40,960–56,889) | 120,471 (120,471–157,538) | 24,976 (23,273–25,284) | 0.40× | 1.91× |
| scale-rand-write-unread-j2-p50 | us | 2.26 (2.19–2.32) | 2.16 (2.16–2.22) | 40.2 (39.7–41.2) | 0.96× | **17.82×** |
| scale-rand-write-unread-j2-p99 | us | 8.90 (5.73–10.3) | 7.97 (6.75–10.0) | 684 (569–750) | 0.90× | **76.89×** |
| scale-fsync-append-j2 | IOPS | 668 (662–669) | 5,919 (4,830–6,360) | 6,522 (4,900–7,585) | 0.11× | 0.10× |
| scale-fsync-append-j2-p50 | us | 48.9 (47.4–49.9) | 4.90 (4.83–5.15) | 4.70 (4.64–4.83) | 0.10× | 0.10× |
| scale-fsync-append-j2-p99 | us | 130 (106–208) | 20.6 (17.0–22.1) | 14.7 (10.9–14.8) | 0.16× | 0.11× |
| scale-fsync-append-j2-fsync-p50 | us | 2,933 (2,900–2,933) | 253 (251–259) | 247 (245–247) | 0.09× | 0.08× |
| scale-fsync-append-j2-fsync-p99 | us | 3,195 (3,162–4,882) | 1,925 (1,090–3,916) | 987 (477–2,933) | 0.60× | 0.31× |
| scale-create-j2 | ops/s | 97,326 (89,876–98,326) | 93,218 (66,651–111,179) | 109,779 (84,075–120,018) | 1.04× | 0.89× |
| scale-stat-cold-j2 | ops/s | 295,979 (262,227–328,385) | 215,756 (186,291–235,388) | 248,212 (229,722–251,366) | 1.37× | 1.19× |
| scale-unlink-j2 | ops/s | 223,541 (208,239–223,961) | 176,758 (158,011–180,926) | 154,311 (144,373–183,119) | 1.26× | 1.45× |
| scale-rand-write-j4 | IOPS | 47,628 (45,011–48,188) | 132,129 (124,121–141,241) | 35,310 (35,009–35,310) | 0.36× | 1.35× |
| scale-rand-write-j4-p50 | us | 2.35 (2.32–2.70) | 2.22 (2.16–2.61) | 51.5 (47.9–53.5) | 0.95× | **21.88×** |
| scale-rand-write-j4-p99 | us | 7.46 (7.20–8.51) | 7.39 (5.92–8.38) | 477 (436–586) | 0.99× | **64.00×** |
| scale-rand-write-unread-j4 | IOPS | 60,235 (2,000–61,134) | 136,533 (110,703–163,840) | 20,078 (19,140–20,078) | 0.44× | **3.00×** |
| scale-rand-write-unread-j4-p50 | us | 2.22 (2.16–2.29) | 2.22 (2.13–2.22) | 41.2 (40.2–43.3) | 1.00× | **18.53×** |
| scale-rand-write-unread-j4-p99 | us | 8.16 (7.90–8.90) | 7.07 (7.01–7.26) | 5,341 (4,948–5,734) | 0.87× | **654.56×** |
| scale-fsync-append-j4 | IOPS | 1,247 (658–1,317) | 6,522 (5,505–7,161) | 7,136 (6,522–7,211) | 0.19× | 0.17× |
| scale-fsync-append-j4-p50 | us | 55.0 (49.9–58.6) | 5.09 (4.83–5.28) | 4.77 (4.58–4.77) | 0.09× | 0.09× |
| scale-fsync-append-j4-p99 | us | 171 (138–387) | 53.5 (44.8–56.6) | 39.7 (22.9–46.8) | 0.31× | 0.23× |
| scale-fsync-append-j4-fsync-p50 | us | 2,966 (2,933–3,064) | 317 (313–334) | 317 (317–326) | 0.11× | 0.11× |
| scale-fsync-append-j4-fsync-p99 | us | 13,959 (3,260–71,827) | 4,620 (3,129–12,124) | 5,341 (4,555–8,094) | 0.33× | 0.38× |
| scale-create-j4 | ops/s | 97,614 (86,992–110,206) | 94,863 (87,020–100,010) | 112,263 (109,567–117,335) | 1.03× | 0.87× |
| scale-stat-cold-j4 | ops/s | 289,359 (259,897–294,982) | 219,017 (216,023–224,914) | 220,553 (219,026–225,890) | 1.32× | 1.31× |
| scale-unlink-j4 | ops/s | 233,759 (200,478–234,823) | 174,727 (142,948–179,718) | 180,581 (175,500–182,038) | 1.34× | 1.29× |
| scale-rand-write-j8 | IOPS | 30,682 (30,007–39,196) | 109,227 (96,376–132,129) | 36,248 (34,860–36,571) | 0.28× | 0.85× |
| scale-rand-write-j8-p50 | us | 2.26 (2.19–2.29) | 2.29 (2.19–2.29) | 167 (165–169) | 1.01× | **73.99×** |
| scale-rand-write-j8-p99 | us | 7.84 (6.24–8.90) | 6.30 (6.11–8.03) | 412 (322–477) | 0.80× | **52.51×** |
| scale-rand-write-unread-j8 | IOPS | 7,161 (6,666–57,690) | 130,032 (120,471–134,295) | 18,450 (17,770–19,787) | 0.06× | 0.39× |
| scale-rand-write-unread-j8-p50 | us | 2.13 (2.13–2.45) | 2.22 (2.16–2.67) | 111 (41.7–117) | 1.05× | **52.21×** |
| scale-rand-write-unread-j8-p99 | us | 6.94 (6.11–6.94) | 6.94 (6.88–9.54) | 14,877 (11,600–15,270) | 1.00× | **2142.38×** |
| scale-fsync-append-j8 | IOPS | 507 (419–778) | 6,311 (5,761–6,827) | 6,931 (6,748–7,461) | 0.08× | 0.07× |
| scale-fsync-append-j8-p50 | us | 148 (121–214) | 5.15 (5.02–5.41) | 4.83 (4.45–4.96) | 0.03× | 0.03× |
| scale-fsync-append-j8-p99 | us | 938 (358–1,417) | 91.6 (74.2–134) | 84.5 (74.2–92.7) | 0.10× | 0.09× |
| scale-fsync-append-j8-fsync-p50 | us | 13,435 (13,304–15,270) | 395 (383–416) | 375 (350–395) | 0.03× | 0.03× |
| scale-fsync-append-j8-fsync-p99 | us | 33,817 (27,132–41,157) | 14,615 (13,435–15,663) | 11,731 (11,076–13,435) | 0.43× | 0.35× |
| scale-create-j8 | ops/s | 117,133 (106,305–117,628) | 99,617 (95,266–101,638) | 104,795 (99,291–108,407) | 1.18× | 1.12× |
| scale-stat-cold-j8 | ops/s | 290,689 (273,053–302,928) | 179,363 (165,135–232,870) | 223,585 (174,568–240,366) | 1.62× | 1.30× |
| scale-unlink-j8 | ops/s | 222,605 (218,907–250,955) | 178,717 (169,094–183,932) | 179,799 (170,044–184,964) | 1.25× | 1.24× |

- ext4-guest-kernel: mtime-ahead-of-guest-ms -1.30 median (-1.74–-1.16)
- fxfs-guest: mtime-ahead-of-guest-ms -3.48 median (-4.30–-1.89)
- fxfs-guest: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j2-interrupts-per-request 0.68 median (0.68–0.68)
- fxfs-guest: scale-create-j2-queues-used 0.00 median (0.00–2.00)
- fxfs-guest: scale-create-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j8-interrupts-per-request 0.58 median (0.56–0.61)
- fxfs-guest: scale-create-j8-queues-used 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j1-cross-cpu-pct 0.00 median (0.00–0.10)
- fxfs-guest: scale-fsync-append-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-fsync-append-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-fsync-append-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j4-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest: scale-fsync-append-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-fsync-append-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j8-interrupts-per-request 0.99 median (0.99–0.99)
- fxfs-guest: scale-fsync-append-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j4-interrupts-per-request 0.99 median (0.99–0.99)
- fxfs-guest: scale-rand-read-cold-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j8-interrupts-per-request 0.94 median (0.93–0.94)
- fxfs-guest: scale-rand-read-cold-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-qd16-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-qd16-interrupts-per-request 0.99 median (0.98–1.00)
- fxfs-guest: scale-rand-read-cold-qd16-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j1-interrupts-per-request 0.74 median (0.66–0.80)
- fxfs-guest: scale-rand-write-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j2-interrupts-per-request 0.69 median (0.69–0.74)
- fxfs-guest: scale-rand-write-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-j4-cross-cpu-pct 0.60 median (0.00–0.60)
- fxfs-guest: scale-rand-write-j4-interrupts-per-request 0.64 median (0.59–0.77)
- fxfs-guest: scale-rand-write-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-j8-cross-cpu-pct 0.30 median (0.00–0.30)
- fxfs-guest: scale-rand-write-j8-interrupts-per-request 0.70 median (0.60–0.71)
- fxfs-guest: scale-rand-write-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-unread-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-unread-j1-interrupts-per-request 0.74 median (0.67–0.76)
- fxfs-guest: scale-rand-write-unread-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-unread-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-unread-j2-interrupts-per-request 0.78 median (0.64–0.83)
- fxfs-guest: scale-rand-write-unread-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-unread-j4-cross-cpu-pct 0.80 median (0.00–1.60)
- fxfs-guest: scale-rand-write-unread-j4-interrupts-per-request 0.62 median (0.61–0.74)
- fxfs-guest: scale-rand-write-unread-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-unread-j8-cross-cpu-pct 0.40 median (0.40–0.80)
- fxfs-guest: scale-rand-write-unread-j8-interrupts-per-request 0.71 median (0.68–0.75)
- fxfs-guest: scale-rand-write-unread-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-stat-cold-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-unlink-j4-queues-used 0.00 median (0.00–1.00)
- fxfs-guest: scale-unlink-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.03 median (-4.62–-2.76)
- fxfs-guest-dax: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j2-interrupts-per-request 0.62 median (0.62–0.62)
- fxfs-guest-dax: scale-create-j2-queues-used 0.00 median (0.00–2.00)
- fxfs-guest-dax: scale-create-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j8-interrupts-per-request 0.71 median (0.62–0.74)
- fxfs-guest-dax: scale-create-j8-queues-used 2.00 median (1.00–2.00)
- fxfs-guest-dax: scale-fsync-append-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-fsync-append-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-fsync-append-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j4-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest-dax: scale-fsync-append-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-fsync-append-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j8-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest-dax: scale-fsync-append-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-interrupts-per-request 0.56 median (0.25–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-interrupts-per-request 1.00 median (0.89–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-read-cold-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j8-interrupts-per-request 0.85 median (0.52–0.93)
- fxfs-guest-dax: scale-rand-read-cold-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-read-cold-qd16-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j1-interrupts-per-request 0.04 median (0.04–0.05)
- fxfs-guest-dax: scale-rand-write-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j2-interrupts-per-request 0.09 median (0.08–0.11)
- fxfs-guest-dax: scale-rand-write-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j4-interrupts-per-request 0.07 median (0.07–0.08)
- fxfs-guest-dax: scale-rand-write-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j8-interrupts-per-request 0.96 median (0.96–0.97)
- fxfs-guest-dax: scale-rand-write-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-unread-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j1-interrupts-per-request 0.08 median (0.07–0.11)
- fxfs-guest-dax: scale-rand-write-unread-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-unread-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j2-interrupts-per-request 0.08 median (0.08–0.09)
- fxfs-guest-dax: scale-rand-write-unread-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-unread-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j4-interrupts-per-request 0.13 median (0.12–0.15)
- fxfs-guest-dax: scale-rand-write-unread-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-unread-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j8-interrupts-per-request 0.57 median (0.21–0.58)
- fxfs-guest-dax: scale-rand-write-unread-j8-queues-used 2.00 median (2.00–2.00)
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
| get | 247 | B | $0.000099 |
| get-range | 10,178 | B | $0.004071 |
| head | 0 | B | $0.000000 |
| list | 16 | A | $0.000080 |
| create | 2,219 | A | $0.011095 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 12,663 | | $0.015355 |

Uploaded 6.4 GiB (segments 4.7 GiB, extents 1.7 GiB, checkpoints 149.0 KiB, moves 0 B, other 96 B); downloaded 7.2 GiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 25,095.7 µs |
| read | 177,260 | 56.7 µs |
| write | 110,640 | 267.1 µs |
| free | 103 | 33.4 µs |
| map | 2,256 | 52.7 µs |
| unmap | 1,128 | 86.8 µs |
| commit | 222 | 107,456.2 µs |
| root | 2 | 57.3 µs |
| info | 8 | 6.0 µs |
| window | 1 | 2,362.8 µs |
| fault | 18,886 | 256.8 µs |
| transfer | 2 | 12.8 µs |
| free_many | 482 | 1,368.1 µs |
| private | 81,978 | 83.5 µs |

Commits: 34,528, 1,271.9 µs each: committing the store 344.3 µs, waiting for object storage 927.5 µs.

Segments closed and framed: 299, 37.2 ms each.

Over 2× slower than ext4:
- scale-rand-read-cold-j1 on fxfs-guest: 5.16× ext4-guest-kernel's
- scale-rand-read-cold-j1 on fxfs-guest-dax: 4.90× ext4-guest-kernel's
- scale-rand-read-cold-j1-p50 on fxfs-guest: 2.24× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest: 19.91× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest-dax: 19.91× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest: 4.39× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest-dax: 3.54× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest: 46.33× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest-dax: 47.93× ext4-guest-kernel's
- scale-rand-read-cold-j4 on fxfs-guest: 3.04× ext4-guest-kernel's
- scale-rand-read-cold-j4 on fxfs-guest-dax: 2.14× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest: 35.98× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest-dax: 27.36× ext4-guest-kernel's
- scale-rand-read-cold-j8 on fxfs-guest: 2.56× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest: 33.43× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest-dax: 26.28× ext4-guest-kernel's
- scale-rand-read-cold-qd16 on fxfs-guest: 2.50× ext4-guest-kernel's
- scale-rand-read-cold-qd16-p50 on fxfs-guest: 2.96× ext4-guest-kernel's
- scale-rand-write-j1 on fxfs-guest-dax: 2.09× ext4-guest-kernel's
- scale-rand-write-j1-p50 on fxfs-guest-dax: 26.97× ext4-guest-kernel's
- scale-rand-write-j1-p99 on fxfs-guest-dax: 16.97× ext4-guest-kernel's
- scale-rand-write-unread-j1 on fxfs-guest-dax: 2.36× ext4-guest-kernel's
- scale-rand-write-unread-j1-p50 on fxfs-guest-dax: 16.69× ext4-guest-kernel's
- scale-rand-write-unread-j1-p99 on fxfs-guest-dax: 42.59× ext4-guest-kernel's
- scale-rand-write-j2-p50 on fxfs-guest-dax: 17.79× ext4-guest-kernel's
- scale-rand-write-j2-p99 on fxfs-guest-dax: 11.95× ext4-guest-kernel's
- scale-rand-write-unread-j2-p50 on fxfs-guest-dax: 17.82× ext4-guest-kernel's
- scale-rand-write-unread-j2-p99 on fxfs-guest-dax: 76.89× ext4-guest-kernel's
- scale-rand-write-j4-p50 on fxfs-guest-dax: 21.88× ext4-guest-kernel's
- scale-rand-write-j4-p99 on fxfs-guest-dax: 64.00× ext4-guest-kernel's
- scale-rand-write-unread-j4 on fxfs-guest-dax: 3.00× ext4-guest-kernel's
- scale-rand-write-unread-j4-p50 on fxfs-guest-dax: 18.53× ext4-guest-kernel's
- scale-rand-write-unread-j4-p99 on fxfs-guest-dax: 654.56× ext4-guest-kernel's
- scale-rand-write-j8-p50 on fxfs-guest-dax: 73.99× ext4-guest-kernel's
- scale-rand-write-j8-p99 on fxfs-guest-dax: 52.51× ext4-guest-kernel's
- scale-rand-write-unread-j8-p50 on fxfs-guest-dax: 52.21× ext4-guest-kernel's
- scale-rand-write-unread-j8-p99 on fxfs-guest-dax: 2142.38× ext4-guest-kernel's
