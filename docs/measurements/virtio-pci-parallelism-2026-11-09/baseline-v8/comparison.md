Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | fxfs-guest | fxfs-guest-dax | fxfs-guest vs ext4-guest-kernel | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|
| scale-rand-read-cold-j1 | IOPS | 26,256 (24,976–30,118) | 3,568 (3,413–3,954) | 5,769 (5,146–6,006) | **7.36×** | **4.55×** |
| scale-rand-read-cold-j1-p50 | us | 36.1 (31.4–41.2) | 124 (117–130) | 14.0 (13.5–14.9) | **3.45×** | 0.39× |
| scale-rand-read-cold-j1-p99 | us | 59.6 (50.9–88.6) | 4,882 (4,686–4,882) | 4,817 (4,620–4,817) | **81.85×** | **80.76×** |
| scale-rand-read-warm-j1 | IOPS | 972,698 (966,843–988,142) | 981,561 (976,027–993,195) | 917,987 (915,512–919,250) | 0.99× | 1.06× |
| scale-rand-read-warm-j1-p50 | us | 0.73 (0.72–0.74) | 0.72 (0.72–0.72) | 0.79 (0.79–0.79) | 0.99× | 1.08× |
| scale-rand-read-warm-j1-p99 | us | 1.00 (0.98–1.01) | 0.99 (0.97–1.00) | 1.01 (1.00–1.01) | 0.98× | 1.01× |
| scale-rand-read-cold-j2 | IOPS | 53,895 (52,513–55,351) | 6,990 (6,639–7,014) | 11,191 (10,064–12,375) | **7.71×** | **4.82×** |
| scale-rand-read-cold-j2-p50 | us | 34.6 (33.5–34.6) | 130 (129–136) | 12.5 (12.2–12.6) | **3.75×** | 0.36× |
| scale-rand-read-cold-j2-p99 | us | 59.6 (58.6–66.0) | 5,603 (4,948–5,734) | 4,882 (2,703–5,276) | **93.94×** | **81.85×** |
| scale-rand-read-warm-j2 | IOPS | 1,845,314 (1,844,743–1,860,079) | 1,855,823 (1,854,289–1,855,957) | 1,456,769 (1,420,858–1,459,228) | 0.99× | 1.27× |
| scale-rand-read-warm-j2-p50 | us | 0.77 (0.77–0.78) | 0.77 (0.77–0.78) | 1.05 (1.05–1.08) | 1.00× | 1.36× |
| scale-rand-read-warm-j2-p99 | us | 1.08 (1.06–1.08) | 1.05 (1.05–1.06) | 1.46 (1.46–1.51) | 0.97× | 1.36× |
| scale-rand-read-cold-j4 | IOPS | 38,825 (38,280–57,690) | 10,557 (10,039–10,708) | 23,540 (22,383–23,814) | **3.68×** | 1.65× |
| scale-rand-read-cold-j4-p50 | us | 114 (64.8–118) | 210 (206–210) | 12.7 (12.5–12.7) | 1.84× | 0.11× |
| scale-rand-read-cold-j4-p99 | us | 159 (130–161) | 5,079 (2,474–5,800) | 4,555 (2,179–4,620) | **32.00×** | **28.70×** |
| scale-rand-read-warm-j4 | IOPS | 3,650,057 (3,625,857–3,650,409) | 3,625,988 (3,546,409–3,627,691) | 2,487,211 (2,475,344–2,503,620) | 1.01× | 1.47× |
| scale-rand-read-warm-j4-p50 | us | 0.79 (0.79–0.80) | 0.80 (0.80–0.80) | 1.19 (1.18–1.19) | 1.01× | 1.51× |
| scale-rand-read-warm-j4-p99 | us | 1.14 (1.13–1.14) | 1.14 (1.11–1.19) | 2.26 (2.22–2.26) | 1.00× | 1.97× |
| scale-rand-read-cold-j8 | IOPS | 47,767 (46,545–49,498) | 12,810 (12,507–13,139) | 35,463 (35,463–35,617) | **3.73×** | 1.35× |
| scale-rand-read-cold-j8-p50 | us | 163 (157–165) | 309 (305–309) | 12.7 (12.7–13.1) | 1.90× | 0.08× |
| scale-rand-read-cold-j8-p99 | us | 243 (234–261) | 5,145 (4,620–6,324) | 5,210 (2,474–5,276) | **21.20×** | **21.47×** |
| scale-rand-read-warm-j8 | IOPS | 4,561,216 (4,530,337–4,569,836) | 4,541,306 (4,509,891–4,578,420) | 2,621,589 (2,609,957–2,631,143) | 1.00× | 1.74× |
| scale-rand-read-warm-j8-p50 | us | 1.14 (1.14–1.16) | 1.16 (1.14–1.16) | 2.48 (2.45–2.48) | 1.01× | **2.17×** |
| scale-rand-read-warm-j8-p99 | us | 1.64 (1.64–1.66) | 1.66 (1.62–1.66) | 5.09 (5.09–5.15) | 1.01× | **3.10×** |
| scale-rand-read-cold-qd16 | IOPS | 58,514 (56,889–64,000) | 24,094 (24,094–25,284) | 682,667 (682,667–682,667) | **2.43×** | 0.09× |
| scale-rand-read-cold-qd16-p50 | us | 222 (222–276) | 545 (528–553) | 15.9 (15.3–15.9) | **2.45×** | 0.07× |
| scale-rand-read-cold-qd16-p99 | us | 334 (285–553) | 2,179 (2,073–2,179) | 76.3 (51.5–86.5) | **6.53×** | 0.23× |
| scale-rand-write-j1 | IOPS | 22,261 (21,333–22,261) | 64,000 (64,000–64,000) | 6,169 (6,095–6,206) | 0.35× | **3.61×** |
| scale-rand-write-j1-p50 | us | 1.35 (1.34–1.40) | 1.37 (1.35–1.40) | 142 (140–146) | 1.01× | **105.28×** |
| scale-rand-write-j1-p99 | us | 4.32 (4.13–16.3) | 2.45 (2.26–2.48) | 218 (210–222) | 0.57× | **50.49×** |
| scale-fsync-append-j1 | IOPS | 441 (429–444) | 2,893 (2,639–3,066) | 2,547 (2,510–2,599) | 0.15× | 0.17× |
| scale-fsync-append-j1-p50 | us | 6.75 (5.60–7.52) | 3.15 (2.74–3.28) | 3.15 (3.09–3.22) | 0.47× | 0.47× |
| scale-fsync-append-j1-p99 | us | 17.0 (16.1–43.8) | 9.79 (9.02–19.3) | 14.8 (13.6–16.2) | 0.58× | 0.87× |
| scale-fsync-append-j1-fsync-p50 | us | 2,245 (2,245–2,310) | 309 (305–309) | 334 (326–334) | 0.14× | 0.15× |
| scale-fsync-append-j1-fsync-p99 | us | 2,703 (2,671–2,802) | 938 (569–1,090) | 1,647 (1,647–1,745) | 0.35× | 0.61× |
| scale-create-j1 | ops/s | 106,199 (96,812–107,035) | 71,117 (67,668–87,086) | 69,461 (64,215–86,387) | 1.49× | 1.53× |
| scale-stat-cold-j1 | ops/s | 248,755 (201,229–253,621) | 153,062 (114,008–164,292) | 157,619 (114,126–157,808) | 1.63× | 1.58× |
| scale-unlink-j1 | ops/s | 212,516 (168,571–215,282) | 211,074 (188,514–216,893) | 206,025 (180,722–210,009) | 1.01× | 1.03× |
| scale-rand-write-j2 | IOPS | 44,522 (44,522–45,511) | 102,400 (89,043–107,789) | 11,770 (11,703–12,047) | 0.43× | **3.78×** |
| scale-rand-write-j2-p50 | us | 1.70 (1.53–2.10) | 1.69 (1.67–1.70) | 83.5 (69.1–130) | 0.99× | **48.98×** |
| scale-rand-write-j2-p99 | us | 8.77 (6.11–29.3) | 6.43 (6.18–12.6) | 272 (272–281) | 0.73× | **31.07×** |
| scale-fsync-append-j2 | IOPS | 617 (466–909) | 3,141 (3,122–3,507) | 3,021 (2,977–3,048) | 0.20× | 0.20× |
| scale-fsync-append-j2-p50 | us | 10.8 (5.66–47.9) | 3.73 (3.25–3.86) | 3.89 (3.47–3.92) | 0.34× | 0.36× |
| scale-fsync-append-j2-p99 | us | 85.5 (14.9–97.8) | 13.4 (12.7–14.1) | 12.6 (10.9–18.0) | 0.16× | 0.15× |
| scale-fsync-append-j2-fsync-p50 | us | 2,540 (2,179–4,227) | 553 (528–553) | 586 (569–594) | 0.22× | 0.23× |
| scale-fsync-append-j2-fsync-p99 | us | 4,948 (2,736–5,865) | 1,401 (1,057–2,703) | 2,408 (2,277–2,572) | 0.28× | 0.49× |
| scale-create-j2 | ops/s | 162,936 (144,551–172,903) | 104,596 (94,909–124,614) | 135,156 (93,997–135,770) | 1.56× | 1.21× |
| scale-stat-cold-j2 | ops/s | 324,165 (303,199–368,188) | 234,128 (215,880–239,710) | 226,254 (209,172–235,114) | 1.38× | 1.43× |
| scale-unlink-j2 | ops/s | 301,041 (258,190–315,946) | 183,642 (171,412–194,732) | 190,607 (182,407–191,585) | 1.64× | 1.58× |
| scale-rand-write-j4 | IOPS | 44,043 (42,667–48,188) | 151,704 (146,286–157,538) | 10,779 (10,779–10,865) | 0.29× | **4.09×** |
| scale-rand-write-j4-p50 | us | 1.91 (1.54–2.19) | 2.13 (1.82–2.22) | 317 (309–322) | 1.11× | **166.03×** |
| scale-rand-write-j4-p99 | us | 7.46 (6.62–26.8) | 20.9 (8.38–21.4) | 569 (553–586) | **2.80×** | **76.36×** |
| scale-fsync-append-j4 | IOPS | 931 (908–939) | 3,835 (3,690–3,835) | 3,537 (3,537–3,587) | 0.24× | 0.26× |
| scale-fsync-append-j4-p50 | us | 60.2 (59.1–61.7) | 5.79 (5.73–5.86) | 5.92 (5.86–5.92) | 0.10× | 0.10× |
| scale-fsync-append-j4-p99 | us | 127 (118–142) | 28.8 (27.0–31.6) | 34.6 (31.9–35.1) | 0.23× | 0.27× |
| scale-fsync-append-j4-fsync-p50 | us | 4,227 (4,227–4,293) | 815 (799–831) | 831 (823–831) | 0.19× | 0.20× |
| scale-fsync-append-j4-fsync-p99 | us | 6,062 (5,079–6,062) | 4,293 (3,588–4,489) | 4,358 (3,916–4,424) | 0.71× | 0.72× |
| scale-create-j4 | ops/s | 208,762 (177,493–239,338) | 127,993 (127,120–132,647) | 123,947 (114,536–132,807) | 1.63× | 1.68× |
| scale-stat-cold-j4 | ops/s | 385,792 (375,335–420,208) | 310,573 (284,589–311,679) | 288,855 (285,351–322,030) | 1.24× | 1.34× |
| scale-unlink-j4 | ops/s | 430,890 (393,644–435,123) | 170,618 (167,148–177,683) | 170,967 (168,434–172,591) | **2.53×** | **2.52×** |
| scale-rand-write-j8 | IOPS | 32,508 (24,023–32,900) | 128,000 (126,031–134,295) | 10,279 (10,279–10,330) | 0.25× | **3.16×** |
| scale-rand-write-j8-p50 | us | 2.16 (2.16–2.16) | 2.42 (2.35–2.42) | 700 (700–700) | 1.12× | **324.27×** |
| scale-rand-write-j8-p99 | us | 10.0 (9.41–21.6) | 59.1 (58.1–60.2) | 1,319 (1,221–1,450) | **5.89×** | **131.26×** |
| scale-fsync-append-j8 | IOPS | 1,611 (1,602–1,636) | 3,981 (3,950–4,059) | 3,810 (3,789–3,810) | 0.40× | 0.42× |
| scale-fsync-append-j8-p50 | us | 74.2 (70.1–74.2) | 6.30 (6.30–6.37) | 6.30 (6.30–6.37) | 0.08× | 0.08× |
| scale-fsync-append-j8-p99 | us | 165 (155–194) | 34.0 (32.6–35.1) | 40.7 (33.5–42.8) | 0.21× | 0.25× |
| scale-fsync-append-j8-fsync-p50 | us | 4,817 (4,751–4,882) | 1,139 (1,139–1,188) | 1,204 (1,155–1,221) | 0.24× | 0.25× |
| scale-fsync-append-j8-fsync-p99 | us | 6,128 (5,669–6,717) | 12,648 (11,600–12,648) | 12,780 (12,648–13,042) | **2.06×** | **2.09×** |
| scale-create-j8 | ops/s | 311,041 (227,399–318,990) | 107,263 (99,696–107,775) | 108,324 (103,066–108,729) | **2.90×** | **2.87×** |
| scale-stat-cold-j8 | ops/s | 405,671 (405,131–411,679) | 342,008 (341,988–354,835) | 373,012 (364,942–376,494) | 1.19× | 1.09× |
| scale-unlink-j8 | ops/s | 412,398 (391,784–436,667) | 154,437 (153,749–158,031) | 152,142 (145,819–153,461) | **2.67×** | **2.71×** |

- ext4-guest-kernel: mtime-ahead-of-guest-ms -5.07 median (-6.27–-3.78)
- fxfs-guest: mtime-ahead-of-guest-ms -5.04 median (-6.07–-2.94)
- fxfs-guest: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j8-cross-cpu-pct 99.6 median (99.6–100)
- fxfs-guest: scale-create-j8-interrupts-per-request 0.18 median (0.18–0.20)
- fxfs-guest: scale-create-j8-queues-used 2.00 median (2.00–3.00)
- fxfs-guest: scale-fsync-append-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j1-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest: scale-fsync-append-j1-queues-used 4.00 median (2.00–4.00)
- fxfs-guest: scale-fsync-append-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j2-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest: scale-fsync-append-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-fsync-append-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j4-interrupts-per-request 0.98 median (0.96–0.99)
- fxfs-guest: scale-fsync-append-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-fsync-append-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j8-interrupts-per-request 0.96 median (0.96–0.96)
- fxfs-guest: scale-fsync-append-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j1-queues-used 2.00 median (2.00–3.00)
- fxfs-guest: scale-rand-read-cold-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j2-queues-used 3.00 median (3.00–4.00)
- fxfs-guest: scale-rand-read-cold-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j4-interrupts-per-request 0.93 median (0.93–0.94)
- fxfs-guest: scale-rand-read-cold-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j8-interrupts-per-request 0.82 median (0.82–0.82)
- fxfs-guest: scale-rand-read-cold-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-cold-qd16-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-qd16-interrupts-per-request 0.65 median (0.64–0.65)
- fxfs-guest: scale-rand-read-cold-qd16-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-write-j1-interrupts-per-request 0.49 median (0.32–0.53)
- fxfs-guest: scale-rand-write-j1-queues-used 2.00 median (2.00–3.00)
- fxfs-guest: scale-rand-write-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-write-j2-interrupts-per-request 0.39 median (0.35–0.47)
- fxfs-guest: scale-rand-write-j2-queues-used 3.00 median (3.00–4.00)
- fxfs-guest: scale-rand-write-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-write-j4-interrupts-per-request 0.40 median (0.39–0.41)
- fxfs-guest: scale-rand-write-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-rand-write-j8-cross-cpu-pct 99.7 median (99.0–100)
- fxfs-guest: scale-rand-write-j8-interrupts-per-request 0.43 median (0.38–0.48)
- fxfs-guest: scale-rand-write-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest: scale-stat-cold-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.95 median (-6.41–-4.01)
- fxfs-guest-dax: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j8-cross-cpu-pct 100 median (99.6–100)
- fxfs-guest-dax: scale-create-j8-interrupts-per-request 0.14 median (0.11–0.23)
- fxfs-guest-dax: scale-create-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-fsync-append-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j1-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest-dax: scale-fsync-append-j1-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j2-interrupts-per-request 0.99 median (0.97–0.99)
- fxfs-guest-dax: scale-fsync-append-j2-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j4-interrupts-per-request 0.97 median (0.96–1.00)
- fxfs-guest-dax: scale-fsync-append-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-fsync-append-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j8-interrupts-per-request 0.96 median (0.95–0.96)
- fxfs-guest-dax: scale-fsync-append-j8-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-cross-cpu-pct 100 median (71.9–100)
- fxfs-guest-dax: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (0.72–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-read-cold-j4-interrupts-per-request 1.00 median (0.94–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-queues-used 2.00 median (1.00–2.00)
- fxfs-guest-dax: scale-rand-read-cold-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-read-cold-j8-interrupts-per-request 0.98 median (0.97–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j8-queues-used 3.00 median (3.00–4.00)
- fxfs-guest-dax: scale-rand-read-cold-qd16-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j1-interrupts-per-request 0.98 median (0.98–0.99)
- fxfs-guest-dax: scale-rand-write-j1-queues-used 2.00 median (1.00–2.00)
- fxfs-guest-dax: scale-rand-write-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j2-interrupts-per-request 0.95 median (0.95–0.96)
- fxfs-guest-dax: scale-rand-write-j2-queues-used 3.00 median (3.00–4.00)
- fxfs-guest-dax: scale-rand-write-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j4-interrupts-per-request 0.96 median (0.77–0.96)
- fxfs-guest-dax: scale-rand-write-j4-queues-used 4.00 median (4.00–4.00)
- fxfs-guest-dax: scale-rand-write-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j8-interrupts-per-request 0.84 median (0.83–0.85)
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
| get | 236 | B | $0.000094 |
| get-range | 5,197 | B | $0.002079 |
| head | 0 | B | $0.000000 |
| list | 12 | A | $0.000060 |
| create | 2,348 | A | $0.011740 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 7,796 | | $0.013983 |

Uploaded 3.7 GiB (segments 2.2 GiB, extents 1.4 GiB, checkpoints 87.0 KiB, moves 0 B, other 96 B); downloaded 3.0 GiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 7,814.4 µs |
| read | 177,064 | 64.2 µs |
| write | 98,696 | 563.3 µs |
| free | 64 | 10.7 µs |
| map | 1,536 | 20.7 µs |
| unmap | 768 | 78.9 µs |
| commit | 227 | 156,092.7 µs |
| root | 2 | 1.6 µs |
| info | 8 | 2.4 µs |
| window | 1 | 150.3 µs |
| fault | 18,372 | 388.3 µs |
| transfer | 2 | 10.5 µs |
| free_many | 420 | 1,086.1 µs |
| private | 35,931 | 10.4 µs |

Commits: 30,217, 1,448.8 µs each: committing the store 246.8 µs, waiting for object storage 1,201.7 µs.

Segments closed and framed: 249, 126.5 ms each.

Over 2× slower than ext4:
- scale-rand-read-cold-j1 on fxfs-guest: 7.36× ext4-guest-kernel's
- scale-rand-read-cold-j1 on fxfs-guest-dax: 4.55× ext4-guest-kernel's
- scale-rand-read-cold-j1-p50 on fxfs-guest: 3.45× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest: 81.85× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest-dax: 80.76× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest: 7.71× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest-dax: 4.82× ext4-guest-kernel's
- scale-rand-read-cold-j2-p50 on fxfs-guest: 3.75× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest: 93.94× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest-dax: 81.85× ext4-guest-kernel's
- scale-rand-read-cold-j4 on fxfs-guest: 3.68× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest: 32.00× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest-dax: 28.70× ext4-guest-kernel's
- scale-rand-read-cold-j8 on fxfs-guest: 3.73× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest: 21.20× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest-dax: 21.47× ext4-guest-kernel's
- scale-rand-read-warm-j8-p50 on fxfs-guest-dax: 2.17× ext4-guest-kernel's
- scale-rand-read-warm-j8-p99 on fxfs-guest-dax: 3.10× ext4-guest-kernel's
- scale-rand-read-cold-qd16 on fxfs-guest: 2.43× ext4-guest-kernel's
- scale-rand-read-cold-qd16-p50 on fxfs-guest: 2.45× ext4-guest-kernel's
- scale-rand-read-cold-qd16-p99 on fxfs-guest: 6.53× ext4-guest-kernel's
- scale-rand-write-j1 on fxfs-guest-dax: 3.61× ext4-guest-kernel's
- scale-rand-write-j1-p50 on fxfs-guest-dax: 105.28× ext4-guest-kernel's
- scale-rand-write-j1-p99 on fxfs-guest-dax: 50.49× ext4-guest-kernel's
- scale-rand-write-j2 on fxfs-guest-dax: 3.78× ext4-guest-kernel's
- scale-rand-write-j2-p50 on fxfs-guest-dax: 48.98× ext4-guest-kernel's
- scale-rand-write-j2-p99 on fxfs-guest-dax: 31.07× ext4-guest-kernel's
- scale-rand-write-j4 on fxfs-guest-dax: 4.09× ext4-guest-kernel's
- scale-rand-write-j4-p50 on fxfs-guest-dax: 166.03× ext4-guest-kernel's
- scale-rand-write-j4-p99 on fxfs-guest: 2.80× ext4-guest-kernel's
- scale-rand-write-j4-p99 on fxfs-guest-dax: 76.36× ext4-guest-kernel's
- scale-unlink-j4 on fxfs-guest: 2.53× ext4-guest-kernel's
- scale-unlink-j4 on fxfs-guest-dax: 2.52× ext4-guest-kernel's
- scale-rand-write-j8 on fxfs-guest-dax: 3.16× ext4-guest-kernel's
- scale-rand-write-j8-p50 on fxfs-guest-dax: 324.27× ext4-guest-kernel's
- scale-rand-write-j8-p99 on fxfs-guest: 5.89× ext4-guest-kernel's
- scale-rand-write-j8-p99 on fxfs-guest-dax: 131.26× ext4-guest-kernel's
- scale-fsync-append-j8-fsync-p99 on fxfs-guest: 2.06× ext4-guest-kernel's
- scale-fsync-append-j8-fsync-p99 on fxfs-guest-dax: 2.09× ext4-guest-kernel's
- scale-create-j8 on fxfs-guest: 2.90× ext4-guest-kernel's
- scale-create-j8 on fxfs-guest-dax: 2.87× ext4-guest-kernel's
- scale-unlink-j8 on fxfs-guest: 2.67× ext4-guest-kernel's
- scale-unlink-j8 on fxfs-guest-dax: 2.71× ext4-guest-kernel's
