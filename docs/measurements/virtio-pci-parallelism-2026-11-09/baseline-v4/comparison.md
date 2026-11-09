Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | fxfs-guest | fxfs-guest-dax | fxfs-guest vs ext4-guest-kernel | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|
| scale-rand-read-cold-j1 | IOPS | 26,597 (26,597–30,118) | 3,677 (3,638–3,821) | 5,565 (5,159–5,611) | **7.23×** | **4.78×** |
| scale-rand-read-cold-j1-p50 | us | 35.6 (30.3–35.6) | 119 (115–124) | 13.9 (13.6–14.7) | **3.35×** | 0.39× |
| scale-rand-read-cold-j1-p99 | us | 60.7 (58.6–62.7) | 4,686 (4,686–4,882) | 4,686 (4,686–4,817) | **77.23×** | **77.23×** |
| scale-rand-read-warm-j1 | IOPS | 964,650 (960,847–965,369) | 977,268 (972,292–983,019) | 910,700 (900,060–915,444) | 0.99× | 1.06× |
| scale-rand-read-warm-j1-p50 | us | 0.74 (0.74–0.74) | 0.72 (0.72–0.73) | 0.80 (0.80–0.81) | 0.98× | 1.08× |
| scale-rand-read-warm-j1-p99 | us | 1.01 (1.01–1.03) | 0.99 (0.98–1.00) | 1.02 (1.01–1.03) | 0.98× | 1.01× |
| scale-rand-read-cold-j2 | IOPS | 49,349 (43,574–54,613) | 7,050 (6,704–7,474) | 10,064 (9,615–10,584) | **7.00×** | **4.90×** |
| scale-rand-read-cold-j2-p50 | us | 35.1 (33.5–38.7) | 126 (123–130) | 12.5 (12.4–12.5) | **3.61×** | 0.36× |
| scale-rand-read-cold-j2-p99 | us | 74.2 (61.7–82.4) | 5,407 (4,948–5,472) | 5,276 (5,145–5,407) | **72.83×** | **71.06×** |
| scale-rand-read-warm-j2 | IOPS | 1,852,015 (1,843,861–1,856,599) | 1,854,872 (1,847,705–1,867,908) | 1,494,188 (1,491,007–1,494,860) | 1.00× | 1.24× |
| scale-rand-read-warm-j2-p50 | us | 0.77 (0.77–0.78) | 0.78 (0.76–0.78) | 1.00 (1.00–1.01) | 1.01× | 1.30× |
| scale-rand-read-warm-j2-p99 | us | 1.08 (1.08–1.10) | 1.06 (1.06–1.06) | 1.43 (1.43–1.43) | 0.99× | 1.33× |
| scale-rand-read-cold-j4 | IOPS | 59,362 (43,574–73,802) | 12,820 (12,564–13,256) | 22,693 (20,635–23,814) | **4.63×** | **2.62×** |
| scale-rand-read-cold-j4-p50 | us | 55.6 (44.3–101) | 157 (153–161) | 12.4 (12.2–12.6) | **2.82×** | 0.22× |
| scale-rand-read-cold-j4-p99 | us | 148 (119–148) | 5,014 (4,882–5,079) | 3,817 (2,146–4,620) | **33.77×** | **25.71×** |
| scale-rand-read-warm-j4 | IOPS | 3,639,420 (3,615,221–3,656,541) | 3,649,801 (3,647,796–3,654,549) | 2,614,388 (2,611,395–2,620,755) | 1.00× | 1.39× |
| scale-rand-read-warm-j4-p50 | us | 0.80 (0.80–0.80) | 0.79 (0.79–0.80) | 1.14 (1.14–1.14) | 0.99× | 1.44× |
| scale-rand-read-warm-j4-p99 | us | 1.10 (1.08–1.11) | 1.11 (1.11–1.11) | 2.04 (2.04–2.04) | 1.01× | 1.86× |
| scale-rand-read-cold-j8 | IOPS | 65,799 (38,191–66,602) | 18,876 (18,492–19,051) | 36,818 (34,638–37,751) | **3.49×** | 1.79× |
| scale-rand-read-cold-j8-p50 | us | 123 (122–237) | 202 (202–210) | 12.9 (12.7–13.0) | 1.63× | 0.10× |
| scale-rand-read-cold-j8-p99 | us | 171 (163–285) | 7,111 (6,193–7,766) | 2,441 (2,441–5,079) | **41.58×** | **14.28×** |
| scale-rand-read-warm-j8 | IOPS | 3,533,358 (3,504,490–3,539,061) | 3,528,471 (3,400,823–3,553,910) | 2,596,357 (2,584,158–2,596,997) | 1.00× | 1.36× |
| scale-rand-read-warm-j8-p50 | us | 0.83 (0.82–0.84) | 0.82 (0.82–0.84) | 1.16 (1.16–1.16) | 0.99× | 1.40× |
| scale-rand-read-warm-j8-p99 | us | 1.18 (1.16–1.19) | 1.19 (1.18–1.30) | 2.06 (2.06–2.06) | 1.01× | 1.76× |
| scale-rand-read-cold-qd16 | IOPS | 66,065 (49,951–66,065) | 30,567 (29,257–30,567) | 682,667 (512,000–682,667) | **2.16×** | 0.10× |
| scale-rand-read-cold-qd16-p50 | us | 220 (218–293) | 481 (481–502) | 19.8 (15.7–19.8) | **2.19×** | 0.09× |
| scale-rand-read-cold-qd16-p99 | us | 253 (251–334) | 1,171 (1,106–1,221) | 60.2 (53.0–67.1) | **4.63×** | 0.24× |
| scale-rand-write-j1 | IOPS | 22,756 (21,787–23,273) | 64,000 (64,000–73,143) | 6,024 (5,851–6,059) | 0.36× | **3.78×** |
| scale-rand-write-j1-p50 | us | 1.34 (1.34–1.35) | 1.38 (1.38–1.38) | 148 (148–153) | 1.04× | **111.14×** |
| scale-rand-write-j1-p99 | us | 4.26 (3.76–14.4) | 2.35 (2.13–2.35) | 206 (206–210) | 0.55× | **48.36×** |
| scale-fsync-append-j1 | IOPS | 452 (429–469) | 2,893 (2,473–3,141) | 3,261 (2,876–3,261) | 0.16× | 0.14× |
| scale-fsync-append-j1-p50 | us | 5.86 (5.47–6.94) | 3.02 (2.86–3.18) | 3.28 (3.22–3.28) | 0.52× | 0.56× |
| scale-fsync-append-j1-p99 | us | 15.7 (15.6–23.9) | 10.9 (7.65–19.1) | 8.03 (7.07–18.8) | 0.70× | 0.51× |
| scale-fsync-append-j1-fsync-p50 | us | 2,179 (2,146–2,277) | 289 (289–305) | 289 (289–293) | 0.13× | 0.13× |
| scale-fsync-append-j1-fsync-p99 | us | 2,998 (2,671–3,424) | 889 (449–2,040) | 494 (444–1,745) | 0.30× | 0.16× |
| scale-create-j1 | ops/s | 106,763 (68,221–107,210) | 85,759 (75,712–87,533) | 86,364 (69,730–86,635) | 1.24× | 1.24× |
| scale-stat-cold-j1 | ops/s | 241,114 (215,325–244,161) | 162,702 (143,336–170,504) | 155,386 (113,170–159,367) | 1.48× | 1.55× |
| scale-unlink-j1 | ops/s | 213,229 (209,437–220,202) | 214,679 (195,974–227,923) | 176,365 (158,111–217,914) | 0.99× | 1.21× |
| scale-rand-write-j2 | IOPS | 43,574 (43,574–43,574) | 107,789 (107,789–113,778) | 11,571 (11,253–11,838) | 0.40× | **3.77×** |
| scale-rand-write-j2-p50 | us | 1.62 (1.59–2.16) | 1.82 (1.75–2.22) | 71.2 (69.1–119) | 1.12× | **43.82×** |
| scale-rand-write-j2-p99 | us | 6.30 (4.90–21.1) | 7.20 (5.47–7.39) | 259 (255–268) | 1.14× | **41.10×** |
| scale-fsync-append-j2 | IOPS | 484 (471–526) | 3,765 (3,357–3,779) | 3,314 (3,190–3,580) | 0.13× | 0.15× |
| scale-fsync-append-j2-p50 | us | 52.0 (47.4–52.5) | 4.13 (3.86–4.19) | 4.26 (4.26–4.58) | 0.08× | 0.08× |
| scale-fsync-append-j2-p99 | us | 97.8 (92.7–116) | 18.8 (18.6–19.3) | 27.8 (25.7–28.5) | 0.19× | 0.28× |
| scale-fsync-append-j2-fsync-p50 | us | 4,227 (4,178–4,227) | 494 (489–502) | 514 (510–528) | 0.12× | 0.12× |
| scale-fsync-append-j2-fsync-p99 | us | 5,276 (5,079–5,669) | 1,303 (1,139–2,474) | 2,867 (1,516–3,162) | 0.25× | 0.54× |
| scale-create-j2 | ops/s | 144,510 (122,503–173,879) | 132,116 (110,164–133,758) | 117,851 (100,769–127,628) | 1.09× | 1.23× |
| scale-stat-cold-j2 | ops/s | 339,567 (325,706–387,749) | 235,845 (204,970–239,160) | 231,796 (223,973–233,733) | 1.44× | 1.46× |
| scale-unlink-j2 | ops/s | 300,808 (296,682–318,516) | 194,013 (192,110–235,828) | 227,408 (206,183–231,196) | 1.55× | 1.32× |
| scale-rand-write-j4 | IOPS | 43,574 (37,578–47,628) | 157,538 (141,241–163,840) | 10,240 (10,214–10,266) | 0.28× | **4.26×** |
| scale-rand-write-j4-p50 | us | 1.91 (1.58–1.91) | 2.16 (1.83–2.19) | 338 (334–342) | 1.13× | **176.74×** |
| scale-rand-write-j4-p99 | us | 7.46 (7.14–15.3) | 20.9 (12.7–21.6) | 602 (594–635) | **2.80×** | **80.76×** |
| scale-fsync-append-j4 | IOPS | 884 (862–885) | 5,292 (4,842–5,333) | 4,935 (4,582–4,995) | 0.17× | 0.18× |
| scale-fsync-append-j4-p50 | us | 60.7 (60.2–62.7) | 4.32 (3.98–4.38) | 4.64 (4.45–4.70) | 0.07× | 0.08× |
| scale-fsync-append-j4-p99 | us | 119 (114–121) | 40.2 (37.6–43.3) | 35.1 (35.1–37.1) | 0.34× | 0.29× |
| scale-fsync-append-j4-fsync-p50 | us | 4,424 (4,358–4,555) | 602 (586–610) | 627 (618–635) | 0.14× | 0.14× |
| scale-fsync-append-j4-fsync-p99 | us | 6,128 (5,472–7,045) | 3,031 (2,474–4,817) | 2,638 (2,638–4,686) | 0.49× | 0.43× |
| scale-create-j4 | ops/s | 210,871 (180,616–251,817) | 133,139 (129,240–140,105) | 130,009 (123,555–142,282) | 1.58× | 1.62× |
| scale-stat-cold-j4 | ops/s | 410,023 (385,553–415,979) | 309,202 (293,622–312,385) | 327,259 (285,439–335,933) | 1.33× | 1.25× |
| scale-unlink-j4 | ops/s | 413,114 (349,252–413,789) | 212,404 (211,692–213,597) | 208,888 (190,646–221,848) | 1.94× | 1.98× |
| scale-rand-write-j8 | IOPS | 40,157 (39,767–44,765) | 151,704 (128,000–154,566) | 10,291 (10,139–10,357) | 0.26× | **3.90×** |
| scale-rand-write-j8-p50 | us | 1.61 (1.58–1.96) | 1.94 (1.93–2.16) | 725 (717–725) | 1.21× | **450.87×** |
| scale-rand-write-j8-p99 | us | 8.26 (6.88–15.2) | 20.4 (11.1–77.3) | 1,106 (1,106–1,122) | **2.47×** | **133.95×** |
| scale-fsync-append-j8 | IOPS | 1,661 (1,551–1,702) | 5,745 (5,729–5,777) | 5,340 (5,178–5,354) | 0.29× | 0.31× |
| scale-fsync-append-j8-p50 | us | 71.2 (69.1–76.3) | 4.13 (4.05–4.19) | 4.38 (4.32–4.51) | 0.06× | 0.06× |
| scale-fsync-append-j8-p99 | us | 148 (131–161) | 113 (103–124) | 115 (95.7–116) | 0.76× | 0.78× |
| scale-fsync-append-j8-fsync-p50 | us | 4,686 (4,555–4,817) | 766 (758–782) | 848 (840–856) | 0.16× | 0.18× |
| scale-fsync-append-j8-fsync-p99 | us | 6,128 (5,865–6,390) | 9,765 (9,241–10,158) | 8,716 (8,028–9,503) | 1.59× | 1.42× |
| scale-create-j8 | ops/s | 240,328 (206,682–270,682) | 128,532 (128,052–132,870) | 119,116 (112,322–120,507) | 1.87× | **2.02×** |
| scale-stat-cold-j8 | ops/s | 409,192 (362,701–413,201) | 284,267 (221,038–325,178) | 286,121 (263,281–287,585) | 1.44× | 1.43× |
| scale-unlink-j8 | ops/s | 390,787 (359,317–427,766) | 208,279 (205,388–228,564) | 202,421 (198,012–211,450) | 1.88× | 1.93× |

- ext4-guest-kernel: mtime-ahead-of-guest-ms -5.61 median (-6.97–-4.28)
- fxfs-guest: mtime-ahead-of-guest-ms -3.31 median (-3.63–-3.18)
- fxfs-guest: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j8-cross-cpu-pct 99.6 median (99.5–100)
- fxfs-guest: scale-create-j8-interrupts-per-request 0.29 median (0.29–0.46)
- fxfs-guest: scale-create-j8-queues-used 3.00 median (1.00–4.00)
- fxfs-guest: scale-fsync-append-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j1-interrupts-per-request 1.00 median (0.99–1.00)
- fxfs-guest: scale-fsync-append-j1-queues-used 4.00 median (3.00–4.00)
- fxfs-guest: scale-fsync-append-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j2-interrupts-per-request 1.00 median (0.99–1.00)
- fxfs-guest: scale-fsync-append-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-fsync-append-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j4-interrupts-per-request 1.00 median (0.99–1.00)
- fxfs-guest: scale-fsync-append-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-fsync-append-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j8-interrupts-per-request 0.99 median (0.99–0.99)
- fxfs-guest: scale-fsync-append-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j1-queues-used 3.00 median (3.00–3.00)
- fxfs-guest: scale-rand-read-cold-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j4-interrupts-per-request 0.99 median (0.99–0.99)
- fxfs-guest: scale-rand-read-cold-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j8-interrupts-per-request 0.94 median (0.93–0.94)
- fxfs-guest: scale-rand-read-cold-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-qd16-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-qd16-interrupts-per-request 0.94 median (0.92–0.95)
- fxfs-guest: scale-rand-read-cold-qd16-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-write-j1-interrupts-per-request 0.53 median (0.51–0.63)
- fxfs-guest: scale-rand-write-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-write-j2-interrupts-per-request 0.40 median (0.34–0.86)
- fxfs-guest: scale-rand-write-j2-queues-used 4.00 median (3.00–4.00)
- fxfs-guest: scale-rand-write-j4-cross-cpu-pct 100 median (99.4–100)
- fxfs-guest: scale-rand-write-j4-interrupts-per-request 0.37 median (0.34–0.45)
- fxfs-guest: scale-rand-write-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-write-j8-cross-cpu-pct 99.2 median (99.2–99.3)
- fxfs-guest: scale-rand-write-j8-interrupts-per-request 0.45 median (0.37–0.57)
- fxfs-guest: scale-rand-write-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-stat-cold-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j4-cross-cpu-pct 83.3 median (83.3–83.3)
- fxfs-guest: scale-unlink-j4-interrupts-per-request 0.83 median (0.83–0.83)
- fxfs-guest: scale-unlink-j4-queues-used 0.00 median (0.00–3.00)
- fxfs-guest: scale-unlink-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.43 median (-4.62–-3.58)
- fxfs-guest-dax: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-create-j8-interrupts-per-request 0.36 median (0.24–0.41)
- fxfs-guest-dax: scale-create-j8-queues-used 4.00 median (1.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j1-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j2-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest-dax: scale-fsync-append-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j4-interrupts-per-request 1.00 median (0.99–1.00)
- fxfs-guest-dax: scale-fsync-append-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j8-interrupts-per-request 0.99 median (0.99–0.99)
- fxfs-guest-dax: scale-fsync-append-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-cross-cpu-pct 100 median (96.9–100)
- fxfs-guest-dax: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (0.97–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-read-cold-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-queues-used 2.00 median (2.00–3.00)
- fxfs-guest-dax: scale-rand-read-cold-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-read-cold-j8-interrupts-per-request 0.99 median (0.99–0.99)
- fxfs-guest-dax: scale-rand-read-cold-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-read-cold-qd16-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j1-interrupts-per-request 0.98 median (0.98–0.99)
- fxfs-guest-dax: scale-rand-write-j1-queues-used 3.00 median (2.00–3.00)
- fxfs-guest-dax: scale-rand-write-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j2-interrupts-per-request 0.96 median (0.95–0.96)
- fxfs-guest-dax: scale-rand-write-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-write-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j4-interrupts-per-request 0.96 median (0.96–0.97)
- fxfs-guest-dax: scale-rand-write-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-write-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j8-interrupts-per-request 0.94 median (0.94–0.95)
- fxfs-guest-dax: scale-rand-write-j8-queues-used 4.00 median (4.00–4.00)
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
| get | 227 | B | $0.000091 |
| get-range | 5,251 | B | $0.002100 |
| head | 0 | B | $0.000000 |
| list | 12 | A | $0.000060 |
| create | 1,863 | A | $0.009315 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 7,356 | | $0.011576 |

Uploaded 3.9 GiB (segments 2.5 GiB, extents 1.4 GiB, checkpoints 73.4 KiB, moves 0 B, other 96 B); downloaded 3.1 GiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 7,879.5 µs |
| read | 177,019 | 61.0 µs |
| write | 96,963 | 720.3 µs |
| free | 71 | 10.2 µs |
| map | 1,536 | 20.8 µs |
| unmap | 768 | 77.1 µs |
| commit | 232 | 145,890.5 µs |
| root | 2 | 1.4 µs |
| info | 8 | 2.3 µs |
| window | 1 | 123.3 µs |
| fault | 18,930 | 373.5 µs |
| transfer | 2 | 10.0 µs |
| free_many | 396 | 1,132.1 µs |
| private | 35,931 | 9.5 µs |

Commits: 28,882, 1,439.7 µs each: committing the store 270.2 µs, waiting for object storage 1,169.3 µs.

Segments closed and framed: 250, 109.1 ms each.

Over 2× slower than ext4:
- scale-rand-read-cold-j1 on fxfs-guest: 7.23× ext4-guest-kernel's
- scale-rand-read-cold-j1 on fxfs-guest-dax: 4.78× ext4-guest-kernel's
- scale-rand-read-cold-j1-p50 on fxfs-guest: 3.35× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest: 77.23× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest-dax: 77.23× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest: 7.00× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest-dax: 4.90× ext4-guest-kernel's
- scale-rand-read-cold-j2-p50 on fxfs-guest: 3.61× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest: 72.83× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest-dax: 71.06× ext4-guest-kernel's
- scale-rand-read-cold-j4 on fxfs-guest: 4.63× ext4-guest-kernel's
- scale-rand-read-cold-j4 on fxfs-guest-dax: 2.62× ext4-guest-kernel's
- scale-rand-read-cold-j4-p50 on fxfs-guest: 2.82× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest: 33.77× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest-dax: 25.71× ext4-guest-kernel's
- scale-rand-read-cold-j8 on fxfs-guest: 3.49× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest: 41.58× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest-dax: 14.28× ext4-guest-kernel's
- scale-rand-read-cold-qd16 on fxfs-guest: 2.16× ext4-guest-kernel's
- scale-rand-read-cold-qd16-p50 on fxfs-guest: 2.19× ext4-guest-kernel's
- scale-rand-read-cold-qd16-p99 on fxfs-guest: 4.63× ext4-guest-kernel's
- scale-rand-write-j1 on fxfs-guest-dax: 3.78× ext4-guest-kernel's
- scale-rand-write-j1-p50 on fxfs-guest-dax: 111.14× ext4-guest-kernel's
- scale-rand-write-j1-p99 on fxfs-guest-dax: 48.36× ext4-guest-kernel's
- scale-rand-write-j2 on fxfs-guest-dax: 3.77× ext4-guest-kernel's
- scale-rand-write-j2-p50 on fxfs-guest-dax: 43.82× ext4-guest-kernel's
- scale-rand-write-j2-p99 on fxfs-guest-dax: 41.10× ext4-guest-kernel's
- scale-rand-write-j4 on fxfs-guest-dax: 4.26× ext4-guest-kernel's
- scale-rand-write-j4-p50 on fxfs-guest-dax: 176.74× ext4-guest-kernel's
- scale-rand-write-j4-p99 on fxfs-guest: 2.80× ext4-guest-kernel's
- scale-rand-write-j4-p99 on fxfs-guest-dax: 80.76× ext4-guest-kernel's
- scale-rand-write-j8 on fxfs-guest-dax: 3.90× ext4-guest-kernel's
- scale-rand-write-j8-p50 on fxfs-guest-dax: 450.87× ext4-guest-kernel's
- scale-rand-write-j8-p99 on fxfs-guest: 2.47× ext4-guest-kernel's
- scale-rand-write-j8-p99 on fxfs-guest-dax: 133.95× ext4-guest-kernel's
- scale-create-j8 on fxfs-guest-dax: 2.02× ext4-guest-kernel's
