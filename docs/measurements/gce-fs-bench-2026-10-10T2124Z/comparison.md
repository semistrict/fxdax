Host: linux. Iterations: ext4-virtio-blk 3, fxdax-dax 3, fxdax-nodax 3, virtiofs-passthrough 3.

| Workload | Unit | ext4-virtio-blk | fxdax-dax | fxdax-nodax | virtiofs-passthrough | fxdax-dax vs ext4 | fxdax-nodax vs ext4 | virtiofs-passthrough vs ext4 |
|---|---|---|---|---|---|---|---|---|
| seq-write | MiB/s | 107 (96.2–119) | 58.2 (55.0–81.1) | 58.2 (58.2–70.0) | 119 (96.3–119) | 1.83× | 1.83× | 0.90× |
| rand-read-cold | IOPS | 13,932 (13,838–14,949) | 32,508 (31,752–36,248) | 9,288 (8,943–9,823) | 10,089 (9,438–10,139) | 0.43× | 1.50× | 1.38× |
| seq-read-cold | MiB/s | 3,556 (3,325–3,556) | 865 (848–905) | 1,652 (320–1,684) | 1,753 (1,695–1,766) | **4.11×** | **2.15×** | **2.03×** |
| seq-read-warm | MiB/s | 7,448 (7,435–7,500) | 10,252 (10,211–10,523) | 7,107 (7,102–7,546) | 6,777 (6,439–6,840) | 0.73× | 1.05× | 1.10× |
| rand-read-warm | IOPS | 804,898 (798,804–809,466) | 848,112 (845,365–848,551) | 804,263 (801,133–811,634) | 10,116 (10,062–10,168) | 0.95× | 1.00× | **79.57×** |
| rand-write | IOPS | 6,314 (6,309–6,335) | 4,568 (4,563–4,575) | 4,557 (3,574–4,573) | 4,576 (4,564–4,576) | 1.38× | 1.39× | 1.38× |
| mmap-rand-read | IOPS | 1,196,965 (1,195,493–1,199,748) | 1,107,129 (1,103,922–1,108,724) | 9,632 (8,952–9,661) | 10,035 (10,024–10,041) | 1.08× | **124.27×** | **119.27×** |
| mmap-rand-write | IOPS | 6,364 (6,350–10,392) | 4,579 (4,568–5,462) | 6,358 (6,346–6,362) | 4,580 (3,578–4,582) | 1.39× | 1.00× | 1.39× |
| fsync-append | IOPS | 224 (224–224) | 397 (395–398) | 398 (397–398) | 398 (398–399) | 0.56× | 0.56× | 0.56× |
| create | ops/s | 74,132 (65,356–74,407) | 1,545 (1,420–1,867) | 1,547 (1,488–1,591) | 1,476 (1,391–1,530) | **47.99×** | **47.91×** | **50.22×** |
| stat-cold | ops/s | 337,602 (241,526–339,340) | 5,004 (4,957–5,040) | 4,958 (4,929–5,281) | 10,079 (9,739–10,168) | **67.46×** | **68.09×** | **33.49×** |
| readdir | ops/s | 2,917,021 (2,907,966–3,037,123) | 389,582 (284,390–404,471) | 326,256 (314,886–356,116) | 368,172 (350,769–371,890) | **7.49×** | **8.94×** | **7.92×** |
| unlink | ops/s | 78,056 (78,002–78,344) | 3,457 (3,367–3,532) | 3,523 (3,473–3,561) | 4,147 (4,062–4,355) | **22.58×** | **22.16×** | **18.82×** |
| untar | s | 0.13 (0.13–0.13) | 0.20 (0.20–0.21) | 0.21 (0.20–0.22) | 0.19 (0.19–0.20) | 1.61× | 1.68× | 1.53× |
| make | s | 7.25 (7.05–7.47) | 7.58 (7.57–7.65) | 7.55 (7.44–7.63) | 7.56 (7.27–7.70) | 1.05× | 1.04× | 1.04× |
| build-total | s | 7.38 (7.18–7.60) | 7.79 (7.77–7.86) | 7.77 (7.65–7.83) | 7.75 (7.46–7.89) | 1.06× | 1.05× | 1.05× |

- ext4-virtio-blk: mtime-ahead-of-guest-ms -4.88 median (-6.32–-4.46)
- fxdax-dax: mtime-ahead-of-guest-ms 0.12 median (0.09–0.12)
- fxdax-nodax: mtime-ahead-of-guest-ms 0.10 median (0.08–0.13)
- virtiofs-passthrough: mtime-ahead-of-guest-ms -0.33 median (-0.63–-0.32)

Over 2× slower than ext4:
- seq-read-cold on fxdax-dax: 4.11×
- seq-read-cold on fxdax-nodax: 2.15×
- seq-read-cold on virtiofs-passthrough: 2.03×
- rand-read-warm on virtiofs-passthrough: 79.57×
- mmap-rand-read on fxdax-nodax: 124.27×
- mmap-rand-read on virtiofs-passthrough: 119.27×
- create on fxdax-dax: 47.99×
- create on fxdax-nodax: 47.91×
- create on virtiofs-passthrough: 50.22×
- stat-cold on fxdax-dax: 67.46×
- stat-cold on fxdax-nodax: 68.09×
- stat-cold on virtiofs-passthrough: 33.49×
- readdir on fxdax-dax: 7.49×
- readdir on fxdax-nodax: 8.94×
- readdir on virtiofs-passthrough: 7.92×
- unlink on fxdax-dax: 22.58×
- unlink on fxdax-nodax: 22.16×
- unlink on virtiofs-passthrough: 18.82×
