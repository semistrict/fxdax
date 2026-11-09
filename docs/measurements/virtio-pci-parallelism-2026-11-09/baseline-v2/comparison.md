Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | fxfs-guest | fxfs-guest-dax | fxfs-guest vs ext4-guest-kernel | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|
| scale-rand-read-cold-j1 | IOPS | 29,681 (28,055–30,118) | 4,088 (3,599–4,258) | 5,580 (5,535–6,244) | **7.26×** | **5.32×** |
| scale-rand-read-cold-j1-p50 | us | 31.9 (31.4–31.9) | 105 (102–113) | 13.8 (13.5–15.4) | **3.29×** | 0.43× |
| scale-rand-read-cold-j1-p99 | us | 52.5 (51.5–61.7) | 4,620 (4,555–5,014) | 4,555 (4,358–4,686) | **88.04×** | **86.79×** |
| scale-rand-read-warm-j1 | IOPS | 967,489 (926,460–980,278) | 976,306 (956,211–990,073) | 909,821 (895,427–910,571) | 0.99× | 1.06× |
| scale-rand-read-warm-j1-p50 | us | 0.73 (0.72–0.78) | 0.72 (0.72–0.75) | 0.80 (0.80–0.82) | 0.99× | 1.09× |
| scale-rand-read-warm-j1-p99 | us | 1.01 (0.97–1.06) | 0.99 (0.96–1.01) | 1.03 (1.03–1.05) | 0.98× | 1.02× |
| scale-rand-read-cold-j2 | IOPS | 56,110 (53,895–56,889) | 7,038 (6,966–7,953) | 9,870 (9,776–10,530) | **7.97×** | **5.68×** |
| scale-rand-read-cold-j2-p50 | us | 33.5 (33.5–33.5) | 117 (111–118) | 12.5 (12.2–12.6) | **3.50×** | 0.37× |
| scale-rand-read-cold-j2-p99 | us | 66.0 (59.1–95.7) | 5,210 (3,785–5,341) | 4,882 (4,686–5,341) | **78.88×** | **73.92×** |
| scale-rand-read-warm-j2 | IOPS | 1,854,637 (1,848,061–1,866,148) | 1,859,589 (1,858,642–1,878,145) | 1,478,483 (1,472,718–1,492,950) | 1.00× | 1.25× |
| scale-rand-read-warm-j2-p50 | us | 0.77 (0.76–0.78) | 0.77 (0.76–0.77) | 1.01 (1.00–1.02) | 1.00× | 1.31× |
| scale-rand-read-warm-j2-p99 | us | 1.06 (1.06–1.06) | 1.05 (1.03–1.05) | 1.43 (1.43–1.46) | 0.98× | 1.35× |
| scale-rand-read-cold-j4 | IOPS | 75,156 (70,621–75,852) | 13,608 (13,474–13,699) | 22,819 (22,505–23,339) | **5.52×** | **3.29×** |
| scale-rand-read-cold-j4-p50 | us | 44.3 (43.8–45.8) | 148 (140–153) | 12.5 (12.5–12.7) | **3.35×** | 0.28× |
| scale-rand-read-cold-j4-p99 | us | 127 (117–148) | 4,751 (4,620–4,817) | 4,358 (3,916–4,555) | **37.27×** | **34.18×** |
| scale-rand-read-warm-j4 | IOPS | 1,811,600 (1,797,542–1,812,871) | 1,799,963 (1,798,989–1,818,053) | 1,476,181 (1,469,731–1,476,803) | 1.01× | 1.23× |
| scale-rand-read-warm-j4-p50 | us | 0.80 (0.80–0.80) | 0.80 (0.80–0.80) | 1.03 (1.02–1.03) | 1.01× | 1.30× |
| scale-rand-read-warm-j4-p99 | us | 1.13 (1.11–1.13) | 1.13 (1.10–1.13) | 1.45 (1.45–1.45) | 1.00× | 1.28× |
| scale-rand-read-cold-j8 | IOPS | 89,043 (78,392–90,022) | 20,454 (20,005–20,845) | 25,441 (24,165–25,721) | **4.35×** | **3.50×** |
| scale-rand-read-cold-j8-p50 | us | 65.3 (59.1–67.1) | 218 (200–220) | 12.7 (12.6–12.9) | **3.34×** | 0.20× |
| scale-rand-read-cold-j8-p99 | us | 272 (261–362) | 5,145 (2,073–6,128) | 6,128 (5,669–6,717) | **18.89×** | **22.50×** |
| scale-rand-read-warm-j8 | IOPS | 1,754,414 (1,740,448–1,763,153) | 1,757,048 (1,754,195–1,757,712) | 1,468,880 (1,462,027–1,472,495) | 1.00× | 1.19× |
| scale-rand-read-warm-j8-p50 | us | 0.83 (0.82–0.84) | 0.83 (0.83–0.84) | 1.03 (1.03–1.03) | 1.00× | 1.25× |
| scale-rand-read-warm-j8-p99 | us | 1.19 (1.19–1.22) | 1.18 (1.18–1.19) | 1.48 (1.46–1.48) | 0.99× | 1.24× |
| scale-rand-read-cold-qd16 | IOPS | 66,065 (66,065–68,267) | 24,094 (23,273–24,976) | 682,667 (682,667–682,667) | **2.74×** | 0.10× |
| scale-rand-read-cold-qd16-p50 | us | 220 (214–220) | 627 (618–659) | 15.6 (15.4–16.2) | **2.85×** | 0.07× |
| scale-rand-read-cold-qd16-p99 | us | 261 (255–268) | 1,286 (1,237–1,319) | 64.3 (59.6–73.2) | **4.93×** | 0.25× |
| scale-rand-write-j1 | IOPS | 23,273 (22,261–23,814) | 68,267 (64,000–73,143) | 6,282 (6,132–6,321) | 0.34× | **3.70×** |
| scale-rand-write-j1-p50 | us | 1.32 (1.32–1.35) | 1.50 (1.43–1.54) | 140 (140–144) | 1.13× | **106.28×** |
| scale-rand-write-j1-p99 | us | 4.05 (3.54–4.19) | 2.29 (2.26–2.86) | 206 (198–206) | 0.57× | **50.85×** |
| scale-fsync-append-j1 | IOPS | 446 (443–447) | 3,879 (3,531–3,908) | 3,241 (3,180–3,346) | 0.11× | 0.14× |
| scale-fsync-append-j1-p50 | us | 6.11 (4.70–7.26) | 3.31 (2.83–3.34) | 3.02 (2.99–3.22) | 0.54× | 0.49× |
| scale-fsync-append-j1-p99 | us | 23.2 (19.8–45.8) | 7.33 (6.56–9.02) | 12.0 (10.2–14.5) | 0.32× | 0.52× |
| scale-fsync-append-j1-fsync-p50 | us | 2,212 (2,179–2,245) | 245 (243–245) | 257 (249–261) | 0.11× | 0.12× |
| scale-fsync-append-j1-fsync-p99 | us | 2,933 (2,802–3,326) | 412 (395–799) | 1,466 (766–1,499) | 0.14× | 0.50× |
| scale-create-j1 | ops/s | 106,336 (64,785–107,175) | 84,045 (79,090–84,961) | 86,468 (84,472–86,517) | 1.27× | 1.23× |
| scale-stat-cold-j1 | ops/s | 255,241 (195,392–256,675) | 161,705 (122,837–167,329) | 159,354 (158,008–172,447) | 1.58× | 1.60× |
| scale-unlink-j1 | ops/s | 215,503 (203,216–220,132) | 145,653 (69,102–152,517) | 165,417 (138,441–194,366) | 1.48× | 1.30× |
| scale-rand-write-j2 | IOPS | 42,667 (41,796–44,522) | 107,789 (85,333–107,789) | 11,977 (11,571–12,263) | 0.40× | **3.56×** |
| scale-rand-write-j2-p50 | us | 1.59 (1.58–1.64) | 1.66 (1.64–1.85) | 86.5 (37.6–132) | 1.04× | **54.35×** |
| scale-rand-write-j2-p99 | us | 5.28 (5.02–5.92) | 5.79 (4.96–7.26) | 237 (234–241) | 1.10× | **44.80×** |
| scale-fsync-append-j2 | IOPS | 469 (445–479) | 4,129 (3,864–4,267) | 3,894 (3,835–4,180) | 0.11× | 0.12× |
| scale-fsync-append-j2-p50 | us | 45.3 (42.8–48.9) | 3.76 (3.66–3.89) | 3.98 (3.89–4.05) | 0.08× | 0.09× |
| scale-fsync-append-j2-p99 | us | 87.6 (76.3–95.7) | 31.1 (30.8–49.9) | 29.6 (27.3–30.8) | 0.36× | 0.34× |
| scale-fsync-append-j2-fsync-p50 | us | 4,227 (4,112–4,358) | 440 (440–444) | 444 (436–444) | 0.10× | 0.11× |
| scale-fsync-append-j2-fsync-p99 | us | 5,014 (4,817–6,062) | 1,548 (1,171–2,114) | 2,146 (2,073–2,408) | 0.31× | 0.43× |
| scale-create-j2 | ops/s | 158,431 (110,506–173,128) | 132,178 (130,484–135,048) | 127,743 (117,543–133,066) | 1.20× | 1.24× |
| scale-stat-cold-j2 | ops/s | 385,124 (328,006–388,207) | 219,521 (218,301–221,429) | 230,016 (224,774–233,312) | 1.75× | 1.67× |
| scale-unlink-j2 | ops/s | 306,127 (293,251–307,498) | 210,870 (156,572–212,372) | 191,102 (176,036–202,110) | 1.45× | 1.60× |
| scale-rand-write-j4 | IOPS | 43,116 (41,796–44,522) | 120,471 (110,703–136,533) | 12,263 (12,227–12,300) | 0.36× | **3.52×** |
| scale-rand-write-j4-p50 | us | 1.64 (1.59–1.64) | 1.80 (1.78–1.86) | 281 (276–285) | 1.10× | **171.08×** |
| scale-rand-write-j4-p99 | us | 5.02 (4.77–14.5) | 8.16 (6.18–8.26) | 506 (494–506) | 1.62× | **100.69×** |
| scale-fsync-append-j4 | IOPS | 891 (870–917) | 4,796 (4,665–4,983) | 4,613 (4,613–4,686) | 0.19× | 0.19× |
| scale-fsync-append-j4-p50 | us | 58.6 (54.0–65.3) | 3.82 (3.79–3.92) | 3.79 (3.73–3.89) | 0.07× | 0.06× |
| scale-fsync-append-j4-p99 | us | 115 (107–121) | 46.8 (35.6–53.5) | 56.6 (44.8–58.1) | 0.41× | 0.49× |
| scale-fsync-append-j4-fsync-p50 | us | 4,358 (4,293–4,489) | 514 (510–518) | 494 (485–502) | 0.12× | 0.11× |
| scale-fsync-append-j4-fsync-p99 | us | 5,997 (5,210–5,997) | 5,865 (4,882–5,997) | 5,800 (4,948–5,800) | 0.98× | 0.97× |
| scale-create-j4 | ops/s | 157,469 (155,998–159,675) | 117,475 (98,352–124,702) | 101,690 (99,734–115,967) | 1.34× | 1.55× |
| scale-stat-cold-j4 | ops/s | 336,832 (333,250–366,949) | 224,137 (172,143–227,511) | 198,557 (198,340–202,552) | 1.50× | 1.70× |
| scale-unlink-j4 | ops/s | 289,126 (269,757–296,157) | 181,239 (171,986–184,424) | 187,013 (184,253–190,891) | 1.60× | 1.55× |
| scale-rand-write-j8 | IOPS | 43,574 (43,116–45,260) | 107,789 (103,696–126,031) | 11,299 (11,070–11,346) | 0.40× | **3.86×** |
| scale-rand-write-j8-p50 | us | 1.62 (1.61–1.66) | 1.82 (1.78–1.86) | 627 (618–643) | 1.12× | **385.89×** |
| scale-rand-write-j8-p99 | us | 6.56 (6.18–7.26) | 7.39 (5.60–8.38) | 1,319 (1,303–1,401) | 1.13× | **201.05×** |
| scale-fsync-append-j8 | IOPS | 1,670 (1,649–1,739) | 4,676 (4,639–4,842) | 4,582 (4,536–4,655) | 0.36× | 0.36× |
| scale-fsync-append-j8-p50 | us | 73.2 (65.3–76.3) | 3.79 (3.76–3.79) | 3.86 (3.73–3.86) | 0.05× | 0.05× |
| scale-fsync-append-j8-p99 | us | 173 (138–175) | 95.7 (65.3–110) | 58.6 (52.0–58.6) | 0.55× | 0.34× |
| scale-fsync-append-j8-fsync-p50 | us | 4,686 (4,489–4,686) | 553 (545–561) | 545 (545–569) | 0.12× | 0.12× |
| scale-fsync-append-j8-fsync-p99 | us | 6,193 (5,472–6,259) | 16,450 (15,794–17,695) | 17,170 (16,908–18,743) | **2.66×** | **2.77×** |
| scale-create-j8 | ops/s | 162,023 (156,747–166,856) | 106,298 (106,170–113,922) | 117,136 (116,155–120,613) | 1.52× | 1.38× |
| scale-stat-cold-j8 | ops/s | 318,577 (317,806–320,670) | 198,455 (190,136–208,636) | 214,000 (202,464–214,963) | 1.61× | 1.49× |
| scale-unlink-j8 | ops/s | 270,486 (270,267–279,136) | 179,477 (167,263–185,884) | 180,796 (180,744–180,883) | 1.51× | 1.50× |

- ext4-guest-kernel: mtime-ahead-of-guest-ms -3.93 median (-4.84–-2.63)
- fxfs-guest: mtime-ahead-of-guest-ms -2.33 median (-3.17–-1.81)
- fxfs-guest: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-create-j4-cross-cpu-pct 90.0 median (90.0–90.0)
- fxfs-guest: scale-create-j4-interrupts-per-request 0.90 median (0.90–0.90)
- fxfs-guest: scale-create-j4-queues-used 0.00 median (0.00–2.00)
- fxfs-guest: scale-create-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-create-j8-interrupts-per-request 0.29 median (0.25–0.39)
- fxfs-guest: scale-create-j8-queues-used 1.00 median (1.00–2.00)
- fxfs-guest: scale-fsync-append-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-fsync-append-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-fsync-append-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j2-interrupts-per-request 0.99 median (0.99–0.99)
- fxfs-guest: scale-fsync-append-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-fsync-append-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j4-interrupts-per-request 0.98 median (0.98–0.98)
- fxfs-guest: scale-fsync-append-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-fsync-append-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-fsync-append-j8-interrupts-per-request 0.99 median (0.98–0.99)
- fxfs-guest: scale-fsync-append-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-rand-read-cold-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (0.99–1.00)
- fxfs-guest: scale-rand-read-cold-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j4-interrupts-per-request 0.96 median (0.95–0.96)
- fxfs-guest: scale-rand-read-cold-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-j8-interrupts-per-request 0.91 median (0.90–0.92)
- fxfs-guest: scale-rand-read-cold-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-cold-qd16-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-read-cold-qd16-interrupts-per-request 0.95 median (0.95–0.97)
- fxfs-guest: scale-rand-read-cold-qd16-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-rand-write-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-write-j1-interrupts-per-request 0.42 median (0.40–0.46)
- fxfs-guest: scale-rand-write-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-rand-write-j2-interrupts-per-request 0.47 median (0.42–0.47)
- fxfs-guest: scale-rand-write-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-j4-cross-cpu-pct 98.9 median (97.7–99.0)
- fxfs-guest: scale-rand-write-j4-interrupts-per-request 0.44 median (0.42–0.50)
- fxfs-guest: scale-rand-write-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-rand-write-j8-cross-cpu-pct 99.5 median (98.6–99.7)
- fxfs-guest: scale-rand-write-j8-interrupts-per-request 0.48 median (0.41–0.50)
- fxfs-guest: scale-rand-write-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest: scale-stat-cold-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest: scale-stat-cold-j1-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest: scale-stat-cold-j1-queues-used 0.00 median (0.00–1.00)
- fxfs-guest: scale-stat-cold-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-stat-cold-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest: scale-unlink-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -4.55 median (-6.40–-3.74)
- fxfs-guest-dax: scale-create-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-create-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-create-j4-interrupts-per-request 0.44 median (0.36–1.00)
- fxfs-guest-dax: scale-create-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-create-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-create-j8-interrupts-per-request 0.29 median (0.25–0.30)
- fxfs-guest-dax: scale-create-j8-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-fsync-append-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j1-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest-dax: scale-fsync-append-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-fsync-append-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j2-interrupts-per-request 0.99 median (0.99–1.00)
- fxfs-guest-dax: scale-fsync-append-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-fsync-append-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j4-interrupts-per-request 0.99 median (0.99–0.99)
- fxfs-guest-dax: scale-fsync-append-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-fsync-append-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-fsync-append-j8-interrupts-per-request 0.99 median (0.98–0.99)
- fxfs-guest-dax: scale-fsync-append-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-cross-cpu-pct 100 median (87.5–100)
- fxfs-guest-dax: scale-rand-read-cold-j1-interrupts-per-request 1.00 median (0.88–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j1-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-read-cold-j2-interrupts-per-request 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j2-queues-used 1.00 median (1.00–1.00)
- fxfs-guest-dax: scale-rand-read-cold-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-read-cold-j4-interrupts-per-request 0.98 median (0.98–0.98)
- fxfs-guest-dax: scale-rand-read-cold-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-read-cold-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-read-cold-j8-interrupts-per-request 0.80 median (0.77–0.81)
- fxfs-guest-dax: scale-rand-read-cold-j8-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-read-cold-qd16-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j1-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j2-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j4-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-read-warm-j8-queues-used 0.00 median (0.00–0.00)
- fxfs-guest-dax: scale-rand-write-j1-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j1-interrupts-per-request 0.98 median (0.98–0.98)
- fxfs-guest-dax: scale-rand-write-j1-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-j2-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j2-interrupts-per-request 0.96 median (0.95–0.96)
- fxfs-guest-dax: scale-rand-write-j2-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-j4-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j4-interrupts-per-request 0.79 median (0.78–0.79)
- fxfs-guest-dax: scale-rand-write-j4-queues-used 2.00 median (2.00–2.00)
- fxfs-guest-dax: scale-rand-write-j8-cross-cpu-pct 100 median (100–100)
- fxfs-guest-dax: scale-rand-write-j8-interrupts-per-request 0.85 median (0.84–0.85)
- fxfs-guest-dax: scale-rand-write-j8-queues-used 2.00 median (2.00–2.00)
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
| get | 239 | B | $0.000096 |
| get-range | 5,273 | B | $0.002109 |
| head | 0 | B | $0.000000 |
| list | 12 | A | $0.000060 |
| create | 2,371 | A | $0.011855 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 7,898 | | $0.014130 |

Uploaded 3.8 GiB (segments 2.4 GiB, extents 1.5 GiB, checkpoints 65.2 KiB, moves 0 B, other 96 B); downloaded 3.0 GiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 7,804.3 µs |
| read | 177,137 | 51.3 µs |
| write | 103,652 | 430.8 µs |
| free | 74 | 10.3 µs |
| map | 1,536 | 19.8 µs |
| unmap | 768 | 72.4 µs |
| commit | 216 | 164,122.6 µs |
| root | 2 | 1.5 µs |
| info | 8 | 2.0 µs |
| window | 1 | 117.4 µs |
| fault | 18,660 | 346.2 µs |
| transfer | 2 | 9.7 µs |
| free_many | 471 | 1,005.4 µs |
| private | 35,931 | 8.6 µs |

Commits: 36,224, 1,233.0 µs each: committing the store 259.0 µs, waiting for object storage 973.7 µs.

Segments closed and framed: 251, 127.3 ms each.

Over 2× slower than ext4:
- scale-rand-read-cold-j1 on fxfs-guest: 7.26× ext4-guest-kernel's
- scale-rand-read-cold-j1 on fxfs-guest-dax: 5.32× ext4-guest-kernel's
- scale-rand-read-cold-j1-p50 on fxfs-guest: 3.29× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest: 88.04× ext4-guest-kernel's
- scale-rand-read-cold-j1-p99 on fxfs-guest-dax: 86.79× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest: 7.97× ext4-guest-kernel's
- scale-rand-read-cold-j2 on fxfs-guest-dax: 5.68× ext4-guest-kernel's
- scale-rand-read-cold-j2-p50 on fxfs-guest: 3.50× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest: 78.88× ext4-guest-kernel's
- scale-rand-read-cold-j2-p99 on fxfs-guest-dax: 73.92× ext4-guest-kernel's
- scale-rand-read-cold-j4 on fxfs-guest: 5.52× ext4-guest-kernel's
- scale-rand-read-cold-j4 on fxfs-guest-dax: 3.29× ext4-guest-kernel's
- scale-rand-read-cold-j4-p50 on fxfs-guest: 3.35× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest: 37.27× ext4-guest-kernel's
- scale-rand-read-cold-j4-p99 on fxfs-guest-dax: 34.18× ext4-guest-kernel's
- scale-rand-read-cold-j8 on fxfs-guest: 4.35× ext4-guest-kernel's
- scale-rand-read-cold-j8 on fxfs-guest-dax: 3.50× ext4-guest-kernel's
- scale-rand-read-cold-j8-p50 on fxfs-guest: 3.34× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest: 18.89× ext4-guest-kernel's
- scale-rand-read-cold-j8-p99 on fxfs-guest-dax: 22.50× ext4-guest-kernel's
- scale-rand-read-cold-qd16 on fxfs-guest: 2.74× ext4-guest-kernel's
- scale-rand-read-cold-qd16-p50 on fxfs-guest: 2.85× ext4-guest-kernel's
- scale-rand-read-cold-qd16-p99 on fxfs-guest: 4.93× ext4-guest-kernel's
- scale-rand-write-j1 on fxfs-guest-dax: 3.70× ext4-guest-kernel's
- scale-rand-write-j1-p50 on fxfs-guest-dax: 106.28× ext4-guest-kernel's
- scale-rand-write-j1-p99 on fxfs-guest-dax: 50.85× ext4-guest-kernel's
- scale-rand-write-j2 on fxfs-guest-dax: 3.56× ext4-guest-kernel's
- scale-rand-write-j2-p50 on fxfs-guest-dax: 54.35× ext4-guest-kernel's
- scale-rand-write-j2-p99 on fxfs-guest-dax: 44.80× ext4-guest-kernel's
- scale-rand-write-j4 on fxfs-guest-dax: 3.52× ext4-guest-kernel's
- scale-rand-write-j4-p50 on fxfs-guest-dax: 171.08× ext4-guest-kernel's
- scale-rand-write-j4-p99 on fxfs-guest-dax: 100.69× ext4-guest-kernel's
- scale-rand-write-j8 on fxfs-guest-dax: 3.86× ext4-guest-kernel's
- scale-rand-write-j8-p50 on fxfs-guest-dax: 385.89× ext4-guest-kernel's
- scale-rand-write-j8-p99 on fxfs-guest-dax: 201.05× ext4-guest-kernel's
- scale-fsync-append-j8-fsync-p99 on fxfs-guest: 2.66× ext4-guest-kernel's
- scale-fsync-append-j8-fsync-p99 on fxfs-guest-dax: 2.77× ext4-guest-kernel's
