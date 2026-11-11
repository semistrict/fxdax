Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | fxfs-guest | fxfs-guest-dax | fxfs-guest vs ext4-guest-kernel | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|
| scale-rand-read-cold-j1 | IOPS | 23,011 (15,284–23,814) | 8,192 (8,159–8,291) | 10,139 (10,139–11,130) | **2.81×** | **2.27×** |
| scale-rand-read-cold-j1-p50 | us | 19.3 (17.5–22.7) | 42.2 (41.7–44.8) | 4.13 (4.08–4.13) | **2.19×** | 0.21× |
| scale-rand-read-cold-j1-p99 | us | 268 (259–313) | 2,179 (2,146–2,408) | 2,441 (2,376–2,441) | **8.12×** | **9.10×** |
| scale-rand-read-warm-j1 | IOPS | 952,624 (935,564–956,086) | 971,097 (953,816–978,288) | 759,610 (757,388–761,183) | 0.98× | 1.25× |
| scale-rand-read-warm-j1-p50 | us | 0.86 (0.85–0.87) | 0.84 (0.84–0.85) | 1.13 (1.11–1.13) | 0.98× | 1.31× |
| scale-rand-read-warm-j1-p99 | us | 1.18 (1.16–1.29) | 1.13 (1.13–1.18) | 1.37 (1.34–1.40) | 0.96× | 1.16× |
| scale-rand-read-cold-j2 | IOPS | 53,895 (28,444–75,852) | 13,430 (12,962–13,563) | 18,124 (17,965–18,124) | **4.01×** | **2.97×** |
| scale-rand-read-cold-j2-p50 | us | 26.8 (21.6–36.1) | 49.4 (47.4–52.0) | 3.79 (3.76–3.79) | 1.85× | 0.14× |
| scale-rand-read-cold-j2-p99 | us | 224 (56.1–330) | 2,867 (2,834–2,900) | 2,736 (2,671–2,769) | **12.79×** | **12.20×** |
| scale-rand-read-warm-j2 | IOPS | 1,756,060 (1,675,591–1,791,287) | 1,801,695 (1,790,642–1,814,316) | 1,448,070 (1,444,245–1,451,238) | 0.97× | 1.21× |
| scale-rand-read-warm-j2-p50 | us | 0.93 (0.93–0.96) | 0.92 (0.92–0.93) | 1.18 (1.16–1.18) | 0.99× | 1.26× |
| scale-rand-read-warm-j2-p99 | us | 1.32 (1.29–3.98) | 1.27 (1.26–1.29) | 1.53 (1.48–1.53) | 0.96× | 1.16× |
| scale-rand-read-cold-j4 | IOPS | 52,513 (31,508–62,061) | 19,835 (18,534–21,558) | 32,000 (31,030–32,379) | **2.65×** | 1.64× |
| scale-rand-read-cold-j4-p50 | us | 61.2 (60.2–69.1) | 64.3 (56.6–65.3) | 3.92 (3.92–3.95) | 1.05× | 0.06× |
| scale-rand-read-cold-j4-p99 | us | 293 (134–494) | 3,719 (3,293–3,719) | 2,638 (2,605–2,671) | **12.70×** | **9.01×** |
| scale-rand-read-warm-j4 | IOPS | 3,363,521 (3,306,552–3,409,294) | 3,437,256 (3,382,379–3,440,455) | 2,793,169 (2,786,992–2,813,697) | 0.98× | 1.20× |
| scale-rand-read-warm-j4-p50 | us | 1.00 (0.97–1.01) | 0.97 (0.96–0.98) | 1.24 (1.22–1.24) | 0.98× | 1.24× |
| scale-rand-read-warm-j4-p99 | us | 1.37 (1.35–1.45) | 1.35 (1.34–1.37) | 1.58 (1.56–1.62) | 0.99× | 1.15× |
| scale-rand-read-cold-j8 | IOPS | 48,473 (38,642–57,087) | 25,323 (23,273–26,089) | 41,063 (40,554–42,336) | 1.91× | 1.18× |
| scale-rand-read-cold-j8-p50 | us | 127 (124–140) | 93.7 (89.6–96.8) | 4.05 (3.89–4.08) | 0.73× | 0.03× |
| scale-rand-read-cold-j8-p99 | us | 514 (346–627) | 5,931 (5,669–6,193) | 4,293 (3,981–4,293) | **11.54×** | **8.35×** |
| scale-rand-read-warm-j8 | IOPS | 3,289,335 (3,251,378–3,308,624) | 3,320,917 (3,300,081–3,359,132) | 2,785,690 (2,775,700–2,793,257) | 0.99× | 1.18× |
| scale-rand-read-warm-j8-p50 | us | 1.02 (1.01–1.03) | 1.01 (1.00–1.01) | 1.24 (1.24–1.24) | 0.99× | 1.22× |
| scale-rand-read-warm-j8-p99 | us | 1.43 (1.43–1.54) | 1.42 (1.40–1.45) | 1.61 (1.61–1.69) | 0.99× | 1.12× |
| scale-rand-read-cold-qd16 | IOPS | 93,091 (89,043–97,524) | 45,511 (44,522–51,200) | 512,000 (409,600–682,667) | **2.05×** | 0.18× |
| scale-rand-read-cold-qd16-p50 | us | 120 (115–122) | 301 (268–309) | 25.0 (16.5–34.6) | **2.50×** | 0.21× |
| scale-rand-read-cold-qd16-p99 | us | 610 (481–823) | 881 (831–1,036) | 72.2 (68.1–122) | 1.44× | 0.12× |
| scale-rand-write-j1 | IOPS | 46,545 (40,960–46,545) | 53,895 (51,200–68,267) | 15,754 (15,515–16,000) | 0.86× | **2.95×** |
| scale-rand-write-j1-p50 | us | 2.77 (2.13–2.90) | 2.32 (2.26–2.90) | 47.9 (47.4–47.9) | 0.84× | **17.29×** |
| scale-rand-write-j1-p99 | us | 7.39 (6.50–7.65) | 6.37 (4.38–6.43) | 96.8 (91.6–163) | 0.86× | **13.09×** |
| scale-rand-write-unread-j1 | IOPS | 46,545 (42,667–46,545) | 60,235 (36,571–60,235) | 14,222 (11,130–14,423) | 0.77× | **3.27×** |
| scale-rand-write-unread-j1-p50 | us | 3.31 (2.61–3.50) | 3.12 (3.06–3.25) | 36.6 (35.6–36.6) | 0.94× | **11.05×** |
| scale-rand-write-unread-j1-p99 | us | 11.6 (8.16–16.5) | 11.6 (7.39–16.1) | 375 (346–510) | 1.00× | **32.35×** |
| scale-fsync-append-j1 | IOPS | 586 (569–601) | 5,953 (5,278–6,481) | 4,376 (4,129–5,069) | 0.10× | 0.13× |
| scale-fsync-append-j1-p50 | us | 7.90 (7.33–9.02) | 3.79 (3.70–3.89) | 4.19 (3.98–4.26) | 0.48× | 0.53× |
| scale-fsync-append-j1-p99 | us | 33.5 (30.1–36.6) | 11.5 (8.77–14.0) | 13.9 (10.0–17.5) | 0.34× | 0.41× |
| scale-fsync-append-j1-fsync-p50 | us | 1,647 (1,614–1,712) | 142 (136–142) | 151 (142–151) | 0.09× | 0.09× |
| scale-fsync-append-j1-fsync-p99 | us | 2,212 (2,146–2,310) | 297 (261–791) | 1,319 (889–1,827) | 0.13× | 0.60× |
| scale-create-j1 | ops/s | 52,332 (50,178–57,048) | 62,570 (59,977–63,667) | 59,200 (58,797–61,288) | 0.84× | 0.88× |
| scale-stat-cold-j1 | ops/s | 157,948 (155,352–187,288) | 131,777 (126,433–133,984) | 124,320 (115,154–127,125) | 1.20× | 1.27× |
| scale-unlink-j1 | ops/s | 132,415 (132,066–134,789) | 242,736 (207,778–248,110) | 189,774 (177,195–191,343) | 0.55× | 0.70× |
| scale-rand-write-j2 | IOPS | 48,762 (43,574–49,951) | 113,778 (102,400–113,778) | 30,118 (29,257–31,508) | 0.43× | 1.62× |
| scale-rand-write-j2-p50 | us | 3.54 (3.50–3.57) | 3.15 (3.09–3.60) | 40.2 (39.7–40.7) | 0.89× | **11.37×** |
| scale-rand-write-j2-p99 | us | 11.8 (10.4–12.0) | 9.28 (6.94–11.6) | 130 (122–132) | 0.78× | **10.94×** |
| scale-rand-write-unread-j2 | IOPS | 56,889 (55,351–58,514) | 128,000 (113,778–128,000) | 23,540 (23,273–23,814) | 0.44× | **2.42×** |
| scale-rand-write-unread-j2-p50 | us | 3.50 (3.47–3.50) | 3.44 (3.09–3.54) | 39.7 (39.2–40.7) | 0.98× | **11.32×** |
| scale-rand-write-unread-j2-p99 | us | 11.5 (10.9–13.1) | 9.41 (8.90–15.2) | 618 (602–618) | 0.82× | **53.99×** |
| scale-fsync-append-j2 | IOPS | 613 (611–618) | 5,535 (5,333–7,211) | 5,198 (5,095–6,827) | 0.11× | 0.12× |
| scale-fsync-append-j2-p50 | us | 64.3 (63.2–66.0) | 4.77 (4.51–4.77) | 4.96 (4.83–5.02) | 0.07× | 0.08× |
| scale-fsync-append-j2-p99 | us | 243 (120–249) | 12.0 (11.7–12.7) | 13.6 (11.2–15.2) | 0.05× | 0.06× |
| scale-fsync-append-j2-fsync-p50 | us | 3,129 (3,129–3,129) | 222 (214–226) | 234 (230–241) | 0.07× | 0.07× |
| scale-fsync-append-j2-fsync-p99 | us | 3,981 (3,752–4,489) | 1,516 (946–1,614) | 2,867 (1,270–3,228) | 0.38× | 0.72× |
| scale-create-j2 | ops/s | 99,530 (97,368–115,968) | 112,748 (107,768–122,614) | 115,052 (114,601–115,800) | 0.88× | 0.87× |
| scale-stat-cold-j2 | ops/s | 343,367 (302,578–348,905) | 254,253 (216,346–260,818) | 226,361 (223,697–241,247) | 1.35× | 1.52× |
| scale-unlink-j2 | ops/s | 257,196 (233,439–264,728) | 272,788 (234,015–285,630) | 237,928 (228,246–255,735) | 0.94× | 1.08× |
| scale-rand-write-j4 | IOPS | 48,188 (47,628–50,568) | 170,667 (170,667–178,087) | 41,374 (41,374–42,227) | 0.28× | 1.16× |
| scale-rand-write-j4-p50 | us | 3.57 (3.54–3.63) | 3.63 (3.12–4.08) | 71.2 (70.1–73.2) | 1.02× | **19.95×** |
| scale-rand-write-j4-p99 | us | 12.1 (9.92–17.3) | 10.6 (9.02–14.5) | 232 (189–264) | 0.87× | **19.22×** |
| scale-rand-write-unread-j4 | IOPS | 64,000 (57,690–65,016) | 178,087 (170,667–204,800) | 22,883 (21,333–26,256) | 0.36× | **2.80×** |
| scale-rand-write-unread-j4-p50 | us | 3.34 (3.09–3.95) | 3.54 (3.22–3.63) | 64.3 (63.7–64.3) | 1.06× | **19.22×** |
| scale-rand-write-unread-j4-p99 | us | 13.4 (10.7–16.8) | 12.0 (11.1–13.4) | 4,424 (3,654–5,997) | 0.89× | **330.72×** |
| scale-fsync-append-j4 | IOPS | 1,213 (1,198–1,246) | 9,660 (8,904–9,846) | 9,184 (9,102–9,225) | 0.13× | 0.13× |
| scale-fsync-append-j4-p50 | us | 64.8 (63.7–64.8) | 5.15 (5.09–5.22) | 5.22 (5.15–5.28) | 0.08× | 0.08× |
| scale-fsync-append-j4-p99 | us | 241 (110–255) | 13.8 (13.4–14.0) | 13.5 (12.9–14.9) | 0.06× | 0.06× |
| scale-fsync-append-j4-fsync-p50 | us | 3,195 (3,097–3,260) | 305 (301–309) | 334 (326–334) | 0.10× | 0.10× |
| scale-fsync-append-j4-fsync-p99 | us | 3,752 (3,523–3,916) | 2,245 (2,179–4,489) | 2,376 (2,073–3,490) | 0.60× | 0.63× |
| scale-create-j4 | ops/s | 179,724 (176,804–187,007) | 136,506 (128,852–159,652) | 146,804 (141,820–159,830) | 1.32× | 1.22× |
| scale-stat-cold-j4 | ops/s | 538,598 (494,960–543,118) | 355,910 (347,222–400,399) | 338,015 (253,246–395,681) | 1.51× | 1.59× |
| scale-unlink-j4 | ops/s | 447,444 (343,518–488,118) | 242,157 (226,053–266,550) | 262,541 (252,456–288,430) | 1.85× | 1.70× |
| scale-rand-write-j8 | IOPS | 51,848 (51,200–55,351) | 157,538 (128,000–174,298) | 36,409 (36,409–38,102) | 0.33× | 1.42× |
| scale-rand-write-j8-p50 | us | 3.34 (3.34–3.41) | 3.15 (3.09–3.47) | 181 (171–183) | 0.94× | **54.20×** |
| scale-rand-write-j8-p99 | us | 10.2 (9.92–12.0) | 18.6 (16.5–20.6) | 403 (362–436) | 1.82× | **39.65×** |
| scale-rand-write-unread-j8 | IOPS | 25,206 (21,787–31,752) | 195,048 (167,184–195,048) | 22,693 (20,480–22,693) | 0.13× | 1.11× |
| scale-rand-write-unread-j8-p50 | us | 3.15 (3.12–3.18) | 3.06 (2.90–3.60) | 117 (117–117) | 0.97× | **37.20×** |
| scale-rand-write-unread-j8-p99 | us | 9.41 (8.38–9.79) | 23.2 (9.92–39.7) | 10,945 (9,765–12,386) | **2.46×** | **1163.32×** |
| scale-fsync-append-j8 | IOPS | 2,427 (2,388–2,482) | 9,438 (9,394–9,990) | 8,963 (8,866–9,022) | 0.26× | 0.27× |
| scale-fsync-append-j8-p50 | us | 71.2 (68.1–71.2) | 5.22 (5.09–5.22) | 5.34 (5.28–5.41) | 0.07× | 0.08× |
| scale-fsync-append-j8-p99 | us | 222 (140–226) | 77.3 (69.1–77.3) | 71.2 (67.1–74.2) | 0.35× | 0.32× |
| scale-fsync-append-j8-fsync-p50 | us | 3,195 (3,129–3,260) | 440 (420–444) | 528 (514–528) | 0.14× | 0.17× |
| scale-fsync-append-j8-fsync-p99 | us | 3,785 (3,588–3,817) | 5,734 (5,734–7,045) | 5,407 (4,293–5,472) | 1.52× | 1.43× |
| scale-create-j8 | ops/s | 199,711 (194,379–206,226) | 138,920 (135,403–146,779) | 148,107 (143,436–149,504) | 1.44× | 1.35× |
| scale-stat-cold-j8 | ops/s | 449,006 (438,839–471,648) | 420,637 (405,186–423,003) | 357,932 (342,198–453,553) | 1.07× | 1.25× |
| scale-unlink-j8 | ops/s | 373,834 (358,228–383,233) | 250,287 (225,447–267,773) | 219,731 (215,685–234,671) | 1.49× | 1.70× |

- ext4-guest-kernel: mtime-ahead-of-guest-ms -3.15 median (-3.71–-2.88)
- fxfs-guest: mtime-ahead-of-guest-ms -5.33 median (-5.75–-4.89)
- fxfs-guest: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-create-j4-queues-used 0.00 median (0.00–4.00)
- fxfs-guest: scale-create-j8-cross-cpu-pct 0.40 median (0.00–0.40)
- fxfs-guest: scale-create-j8-interrupts-per-request 0.54 median (0.47–0.54)
- fxfs-guest: scale-create-j8-queues-used 2.00 median (1.00–2.00)
- fxfs-guest: scale-fsync-append-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j1-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-fsync-append-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-fsync-append-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j4-interrupts-per-request 1.00 median (0.99–1.00)
- fxfs-guest: scale-fsync-append-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-fsync-append-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-fsync-append-j8-interrupts-per-request 1.00 median (0.99–1.00)
- fxfs-guest: scale-fsync-append-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j1-queues-used 3.00 median (2.00–3.00)
- fxfs-guest: scale-rand-read-cold-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-j8-interrupts-per-request 0.98 median (0.97–0.98)
- fxfs-guest: scale-rand-read-cold-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-qd16-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-cold-qd16-interrupts-per-request 0.97 median (0.97–0.98)
- fxfs-guest: scale-rand-read-cold-qd16-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j1-interrupts-per-request 0.69 median (0.63–0.82)
- fxfs-guest: scale-rand-write-j1-queues-used 3.00 median (2.00–3.00)
- fxfs-guest: scale-rand-write-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j2-interrupts-per-request 0.41 median (0.41–0.58)
- fxfs-guest: scale-rand-write-j2-queues-used 4.00 median (3.00–4.00)
- fxfs-guest: scale-rand-write-j4-cross-cpu-pct 0.00 median (0.00–0.60)
- fxfs-guest: scale-rand-write-j4-interrupts-per-request 0.54 median (0.47–0.56)
- fxfs-guest: scale-rand-write-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-write-j8-cross-cpu-pct 1.60 median (0.50–1.60)
- fxfs-guest: scale-rand-write-j8-interrupts-per-request 0.57 median (0.47–0.64)
- fxfs-guest: scale-rand-write-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-write-unread-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-unread-j1-interrupts-per-request 0.84 median (0.71–0.94)
- fxfs-guest: scale-rand-write-unread-j1-queues-used 3.00 median (2.00–4.00)
- fxfs-guest: scale-rand-write-unread-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-unread-j2-interrupts-per-request 0.72 median (0.69–0.81)
- fxfs-guest: scale-rand-write-unread-j2-queues-used 3.00 median (3.00–4.00)
- fxfs-guest: scale-rand-write-unread-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-unread-j4-interrupts-per-request 0.59 median (0.51–0.64)
- fxfs-guest: scale-rand-write-unread-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-write-unread-j8-cross-cpu-pct 2.40 median (0.00–2.40)
- fxfs-guest: scale-rand-write-unread-j8-interrupts-per-request 0.61 median (0.56–0.67)
- fxfs-guest: scale-rand-write-unread-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-stat-cold-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j8-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-unlink-j8-queues-used 0.00 median (0.00–2.00)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.02 median (-4.04–-2.14)
- fxfs-guest-dax: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-create-j4-queues-used 0.00 median (0.00–2.00)
- fxfs-guest-dax: scale-create-j8-cross-cpu-pct 0.40 median (0.00–1.20)
- fxfs-guest-dax: scale-create-j8-interrupts-per-request 0.57 median (0.45–0.60)
- fxfs-guest-dax: scale-create-j8-queues-used 2.00 median (1.00–2.00)
- fxfs-guest-dax: scale-fsync-append-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j1-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j4-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-fsync-append-j8-interrupts-per-request 0.99 median (0.99–0.99)
- fxfs-guest-dax: scale-fsync-append-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-interrupts-per-request 0.53 median (0.09–0.78)
- fxfs-guest-dax: scale-rand-read-cold-j1-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-interrupts-per-request 0.62 median (0.56–0.94)
- fxfs-guest-dax: scale-rand-read-cold-j2-queues-used 1.00 median (1.00–2.00)
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
- fxfs-guest-dax: scale-rand-write-j1-interrupts-per-request 0.05 median (0.03–0.07)
- fxfs-guest-dax: scale-rand-write-j1-queues-used 3.00 median (2.00–4.00)
- fxfs-guest-dax: scale-rand-write-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j2-interrupts-per-request 0.09 median (0.08–0.09)
- fxfs-guest-dax: scale-rand-write-j2-queues-used 3.00 median (3.00–3.00)
- fxfs-guest-dax: scale-rand-write-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j4-interrupts-per-request 0.12 median (0.12–0.15)
- fxfs-guest-dax: scale-rand-write-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-write-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j8-interrupts-per-request 0.98 median (0.97–0.98)
- fxfs-guest-dax: scale-rand-write-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-write-unread-j1-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j1-interrupts-per-request 0.07 median (0.07–0.07)
- fxfs-guest-dax: scale-rand-write-unread-j1-queues-used 3.00 median (3.00–3.00)
- fxfs-guest-dax: scale-rand-write-unread-j2-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j2-interrupts-per-request 0.08 median (0.08–0.09)
- fxfs-guest-dax: scale-rand-write-unread-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-write-unread-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j4-interrupts-per-request 0.13 median (0.12–0.15)
- fxfs-guest-dax: scale-rand-write-unread-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-write-unread-j8-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-unread-j8-interrupts-per-request 0.86 median (0.75–0.91)
- fxfs-guest-dax: scale-rand-write-unread-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-stat-cold-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-stat-cold-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-stat-cold-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-stat-cold-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-unlink-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-unlink-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-unlink-j4-cross-cpu-pct 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-unlink-j4-interrupts-per-request 0.77 median (0.77–0.77)
- fxfs-guest-dax: scale-unlink-j4-queues-used 0.00 median (0.00–3.00)
- fxfs-guest-dax: scale-unlink-j8-queues-used 0.00 median (0.00–0.00)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 247 | B | $0.000099 |
| get-range | 9,506 | B | $0.003802 |
| head | 0 | B | $0.000000 |
| list | 16 | A | $0.000080 |
| create | 2,828 | A | $0.014140 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 12,600 | | $0.018131 |

Uploaded 6.8 GiB (segments 4.8 GiB, extents 2.0 GiB, checkpoints 175.6 KiB, moves 0 B, other 96 B); downloaded 6.2 GiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 26,778.1 µs |
| read | 177,136 | 55.6 µs |
| write | 103,952 | 196.8 µs |
| free | 71 | 7.5 µs |
| map | 2,256 | 22.3 µs |
| unmap | 1,128 | 85.2 µs |
| commit | 208 | 39,489.2 µs |
| root | 2 | 1.8 µs |
| info | 8 | 2.1 µs |
| window | 1 | 89.2 µs |
| fault | 18,120 | 332.8 µs |
| transfer | 2 | 11.7 µs |
| free_many | 420 | 1,200.4 µs |
| private | 81,978 | 75.6 µs |

Commits: 26,584, 875.3 µs each: committing the store 314.0 µs, waiting for object storage 561.1 µs.

Segments closed and framed: 291, 30.7 ms each.

Over 2× slower than ext4:
- scale-rand-read-cold-j1 on fxfs-guest: 2.81× ext4-guest-kernel's
- scale-rand-read-cold-j1 on fxfs-guest-dax: 2.27× ext4-guest-kernel's
- scale-rand-read-cold-j1-p50 on fxfs-guest: 2.19× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest: 8.12× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest-dax: 9.10× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest: 4.01× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest-dax: 2.97× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest: 12.79× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest-dax: 12.20× ext4-guest-kernel's
- scale-rand-read-cold-j4 on fxfs-guest: 2.65× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest: 12.70× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest-dax: 9.01× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest: 11.54× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest-dax: 8.35× ext4-guest-kernel's
- scale-rand-read-cold-qd16 on fxfs-guest: 2.05× ext4-guest-kernel's
- scale-rand-read-cold-qd16-p50 on fxfs-guest: 2.50× ext4-guest-kernel's
- scale-rand-write-j1 on fxfs-guest-dax: 2.95× ext4-guest-kernel's
- scale-rand-write-j1-p50 on fxfs-guest-dax: 17.29× ext4-guest-kernel's
- scale-rand-write-j1-p99 on fxfs-guest-dax: 13.09× ext4-guest-kernel's
- scale-rand-write-unread-j1 on fxfs-guest-dax: 3.27× ext4-guest-kernel's
- scale-rand-write-unread-j1-p50 on fxfs-guest-dax: 11.05× ext4-guest-kernel's
- scale-rand-write-unread-j1-p99 on fxfs-guest-dax: 32.35× ext4-guest-kernel's
- scale-rand-write-j2-p50 on fxfs-guest-dax: 11.37× ext4-guest-kernel's
- scale-rand-write-j2-p99 on fxfs-guest-dax: 10.94× ext4-guest-kernel's
- scale-rand-write-unread-j2 on fxfs-guest-dax: 2.42× ext4-guest-kernel's
- scale-rand-write-unread-j2-p50 on fxfs-guest-dax: 11.32× ext4-guest-kernel's
- scale-rand-write-unread-j2-p99 on fxfs-guest-dax: 53.99× ext4-guest-kernel's
- scale-rand-write-j4-p50 on fxfs-guest-dax: 19.95× ext4-guest-kernel's
- scale-rand-write-j4-p99 on fxfs-guest-dax: 19.22× ext4-guest-kernel's
- scale-rand-write-unread-j4 on fxfs-guest-dax: 2.80× ext4-guest-kernel's
- scale-rand-write-unread-j4-p50 on fxfs-guest-dax: 19.22× ext4-guest-kernel's
- scale-rand-write-unread-j4-p99 on fxfs-guest-dax: 330.72× ext4-guest-kernel's
- scale-rand-write-j8-p50 on fxfs-guest-dax: 54.20× ext4-guest-kernel's
- scale-rand-write-j8-p99 on fxfs-guest-dax: 39.65× ext4-guest-kernel's
- scale-rand-write-unread-j8-p50 on fxfs-guest-dax: 37.20× ext4-guest-kernel's
- scale-rand-write-unread-j8-p99 on fxfs-guest: 2.46× ext4-guest-kernel's
- scale-rand-write-unread-j8-p99 on fxfs-guest-dax: 1163.32× ext4-guest-kernel's
