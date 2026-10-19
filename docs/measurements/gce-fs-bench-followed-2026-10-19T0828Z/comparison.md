Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3, fxfs-guest-dax-followed 3, fxfs-guest-dax-followed-idle 3.

| Workload | Unit | fxfs-guest-dax | fxfs-guest-dax-followed | fxfs-guest-dax-followed-idle | fxfs-guest-dax-followed vs fxfs-guest-dax | fxfs-guest-dax-followed-idle vs fxfs-guest-dax |
|---|---|---|---|---|---|---|
| seq-write | MiB/s | 65.8 (65.6–65.8) | 58.1 (52.3–61.8) | 61.8 (58.1–65.8) | 1.13× | 1.06× |
| rand-read-cold | IOPS | 167,184 (109,960–192,753) | 204,800 (126,031–207,392) | 212,779 (212,779–215,579) | 0.82× | 0.79× |
| seq-read-cold | MiB/s | 1,174 (705–1,399) | 1,446 (955–1,463) | 1,590 (1,533–1,600) | 0.81× | 0.74× |
| seq-read-warm | MiB/s | 11,488 (11,148–11,497) | 10,712 (10,190–11,257) | 11,292 (11,141–11,397) | 1.07× | 1.02× |
| rand-read-warm | IOPS | 817,220 (816,673–817,373) | 808,416 (770,220–813,683) | 816,914 (816,760–817,847) | 1.01× | 1.00× |
| rand-write | IOPS | 11,826 (11,816–14,430) | 14,400 (11,815–14,421) | 11,815 (11,801–11,818) | 0.82× | 1.00× |
| mmap-rand-read | IOPS | 1,164,856 (1,135,224–1,166,009) | 1,086,338 (1,083,916–1,149,448) | 1,171,410 (1,166,273–1,172,839) | 1.07× | 0.99× |
| mmap-rand-write | IOPS | 11,821 (11,817–11,822) | 11,805 (11,799–11,812) | 11,817 (11,817–11,818) | 1.00× | 1.00× |
| fsync-append | IOPS | 402 (402–662) | 402 (401–402) | 402 (393–402) | 1.00× | 1.00× |
| create | ops/s | 24,266 (23,313–26,780) | 19,864 (19,645–21,904) | 24,122 (23,431–24,930) | 1.22× | 1.01× |
| stat-cold | ops/s | 303,623 (196,411–313,746) | 228,006 (162,354–250,841) | 252,202 (176,147–299,963) | 1.33× | 1.20× |
| stat-warm | ops/s | 1,094,675 (1,062,561–1,139,362) | 843,310 (655,212–963,824) | 1,299,969 (1,283,742–1,406,083) | 1.30× | 0.84× |
| lookup-missing | ops/s | 410,562 (368,715–428,255) | 290,256 (267,384–374,005) | 418,829 (376,626–438,716) | 1.41× | 0.98× |
| readdir | ops/s | 2,193,945 (2,100,077–2,361,771) | 1,949,101 (1,646,273–2,029,013) | 1,967,005 (1,957,503–2,118,159) | 1.13× | 1.12× |
| read-small-cold | ops/s | 119,909 (119,042–125,067) | 98,872 (91,139–103,054) | 117,345 (117,311–120,221) | 1.21× | 1.02× |
| chmod | ops/s | 1,320,120 (1,271,647–1,575,708) | 977,484 (840,348–1,250,579) | 1,354,038 (1,073,556–1,428,079) | 1.35× | 0.97× |
| setxattr | ops/s | 578,511 (222,929–631,160) | 500,671 (323,871–585,232) | 628,905 (382,721–633,007) | 1.16× | 0.92× |
| link | ops/s | 96,007 (90,891–101,230) | 83,391 (72,676–84,468) | 99,832 (96,278–102,474) | 1.15× | 0.96× |
| rename | ops/s | 80,072 (75,319–80,433) | 64,381 (60,015–69,519) | 83,720 (79,156–85,674) | 1.24× | 0.96× |
| rename-across | ops/s | 72,496 (65,906–73,674) | 62,042 (49,602–62,944) | 75,506 (72,094–77,169) | 1.17× | 0.96× |
| unlink | ops/s | 205,603 (162,651–219,143) | 174,288 (129,755–273,756) | 170,080 (125,901–213,325) | 1.18× | 1.21× |
| create-fsync | ops/s | 1,652 (1,170–1,765) | 1,236 (1,183–1,355) | 1,053 (823–1,058) | 1.34× | 1.57× |
| mkdir-tree | ops/s | 27,470 (19,082–27,593) | 22,232 (15,878–23,300) | 24,576 (24,434–25,566) | 1.24× | 1.12× |
| find-cold | ops/s | 236,511 (182,418–244,386) | 176,295 (164,839–212,639) | 226,060 (146,017–230,433) | 1.34× | 1.05× |
| rm-tree | ops/s | 64,629 (59,632–67,873) | 53,170 (48,906–53,887) | 58,239 (58,180–58,451) | 1.22× | 1.11× |
| large-dir-create | ops/s | 89,446 (68,056–91,581) | 76,670 (55,070–80,933) | 84,092 (61,173–84,470) | 1.17× | 1.06× |
| large-dir-stat-cold | ops/s | 309,045 (300,612–314,542) | 266,035 (257,309–269,433) | 268,448 (266,902–292,767) | 1.16× | 1.15× |
| large-dir-unlink | ops/s | 220,638 (198,380–238,930) | 194,396 (193,485–196,452) | 215,548 (173,893–228,864) | 1.13× | 1.02× |
| create-parallel | ops/s | 120,401 (108,119–122,826) | 100,976 (97,665–101,493) | 112,479 (107,631–117,757) | 1.19× | 1.07× |
| stat-parallel | ops/s | 510,208 (473,285–602,821) | 506,871 (474,450–540,097) | 445,936 (419,434–568,286) | 1.01× | 1.14× |
| unlink-parallel | ops/s | 203,707 (196,619–215,129) | 170,871 (167,754–182,478) | 199,495 (190,437–200,652) | 1.19× | 1.02× |
| create-shared-dir | ops/s | 90,256 (89,617–90,279) | 76,674 (75,368–84,811) | 85,534 (78,933–92,100) | 1.18× | 1.06× |
| unlink-shared-dir | ops/s | 185,867 (151,105–222,467) | 151,523 (146,422–169,916) | 198,155 (193,067–208,358) | 1.23× | 0.94× |
| pgbench-init | s | 4.01 (3.71–4.06) | 4.34 (4.25–4.92) | 4.25 (4.22–4.48) | 1.08× | 1.06× |
| pgbench-tps | tps | 1,610 (1,589–1,622) | 1,259 (1,213–1,295) | 1,117 (1,110–1,144) | 1.28× | 1.44× |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -6.16 median (-6.64–-3.01)
- fxfs-guest-dax-followed: follower1-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed: follower1-raced-reads 567 median (567–567)
- fxfs-guest-dax-followed: follower1-reads 326,671 median (326,671–326,671)
- fxfs-guest-dax-followed: follower2-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed: follower2-raced-reads 602 median (602–602)
- fxfs-guest-dax-followed: follower2-reads 318,207 median (318,207–318,207)
- fxfs-guest-dax-followed: mtime-ahead-of-guest-ms -4.02 median (-4.79–-3.59)
- fxfs-guest-dax-followed-idle: follower1-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower1-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-failed-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-raced-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: follower2-reads 0.00 median (0.00–0.00)
- fxfs-guest-dax-followed-idle: mtime-ahead-of-guest-ms -1.71 median (-3.41–-1.50)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 2,585 | B | $0.001034 |
| get-range | 5,972 | B | $0.002389 |
| head | 0 | B | $0.000000 |
| list | 9 | A | $0.000045 |
| create | 2,623 | A | $0.013115 |
| update | 6 | A | $0.000030 |
| overwrite | 0 | A | $0.000000 |
| delete | 3 | free | $0.000000 |
| all | 11,198 | | $0.016613 |

Uploaded 2.4 GiB (segments 2.4 GiB, extents 0 B, checkpoints 778.5 KiB, moves 0 B, other 117 B); downloaded 60.3 KiB.

Followers: at most 2 at once, at most 881 commits and 5,163 ms behind their writer, keeping at most 432.5 MiB of its frees.

Every workload is within 2× of ext4.
