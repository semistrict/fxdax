Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | fxfs-guest | fxfs-guest-dax | fxfs-guest vs ext4-guest-kernel | fxfs-guest-dax vs ext4-guest-kernel |
|---|---|---|---|---|---|---|
| seq-write | MiB/s | 185 (122–193) | 140 (106–142) | 141 (106–144) | 1.31× | 1.31× |
| rand-read-cold | IOPS | 30,118 (26,684–30,510) | 5,777 (5,715–6,443) | 12,245 (11,703–12,319) | **5.21×** | **2.46×** |
| seq-read-cold | MiB/s | 3,122 (1,113–3,241) | 914 (771–966) | 398 (398–400) | **3.41×** | **7.84×** |
| seq-read-warm | MiB/s | 8,545 (8,463–8,608) | 8,480 (8,441–8,541) | 11,291 (10,849–11,399) | 1.01× | 0.76× |
| rand-read-warm | IOPS | 940,022 (936,757–945,038) | 939,788 (937,853–940,935) | 850,026 (844,668–851,325) | 1.00× | 1.11× |
| mmap-seq-read | MiB/s | 9,846 (9,846–10,240) | 8,828 (8,533–8,828) | 10,240 (10,240–10,240) | 1.12× | 0.96× |
| rand-write | IOPS | 14,538 (10,829–14,854) | 76,920 (76,205–79,150) | 6,302 (6,134–6,448) | 0.19× | **2.31×** |
| mmap-rand-read | IOPS | 1,296,175 (1,275,948–1,296,405) | 1,241,382 (1,225,273–1,254,283) | 1,304,129 (1,298,150–1,311,910) | 1.04× | 0.99× |
| mmap-rand-write | IOPS | 9,996 (8,055–11,386) | 7,522 (6,639–8,020) | 60,907 (60,235–64,759) | 1.33× | 0.16× |
| fsync-append | IOPS | 411 (398–432) | 2,619 (2,044–3,266) | 3,631 (3,501–3,765) | 0.16× | 0.11× |
| sqlite-pread-scan | MiB/s | 2,771 (757–2,775) | 352 (317–356) | 162 (160–163) | **7.88×** | **17.08×** |
| sqlite-pread-scan-warm | MiB/s | 3,020 (2,982–3,045) | 3,042 (3,028–3,078) | 2,756 (2,752–2,805) | 0.99× | 1.10× |
| sqlite-mmap-scan | MiB/s | 2,562 (2,196–2,751) | 887 (850–896) | 2,973 (2,850–2,979) | **2.89×** | 0.86× |
| sqlite-mmap-scan-warm | MiB/s | 2,971 (2,958–3,046) | 2,842 (2,810–2,852) | 3,030 (3,025–3,063) | 1.05× | 0.98× |
| create | ops/s | 96,591 (95,820–98,437) | 81,304 (77,553–81,400) | 79,931 (69,755–83,572) | 1.19× | 1.21× |
| stat-cold | ops/s | 332,226 (323,664–339,088) | 191,427 (183,955–201,776) | 183,224 (173,603–187,246) | 1.74× | 1.81× |
| stat-warm | ops/s | 1,303,203 (1,291,124–1,350,869) | 1,343,859 (1,314,892–1,350,618) | 1,399,670 (1,370,479–1,399,796) | 0.97× | 0.93× |
| lookup-missing | ops/s | 452,298 (450,453–456,039) | 472,504 (459,132–486,278) | 477,189 (460,718–489,089) | 0.96× | 0.95× |
| readdir | ops/s | 3,021,070 (2,895,760–3,025,668) | 3,646,800 (3,333,239–3,710,210) | 3,590,426 (3,436,408–3,619,569) | 0.83× | 0.84× |
| read-small-cold | ops/s | 24,125 (23,779–26,466) | 8,354 (7,864–8,386) | 37,879 (37,128–37,967) | **2.89×** | 0.64× |
| chmod | ops/s | 747,546 (697,380–778,417) | 389,161 (262,974–410,848) | 467,101 (441,845–472,606) | 1.92× | 1.60× |
| setxattr | ops/s | 569,485 (556,744–620,376) | 178,782 (171,018–200,483) | 201,635 (193,928–208,056) | **3.19×** | **2.82×** |
| link | ops/s | 152,847 (144,851–154,042) | 100,213 (97,511–103,907) | 101,364 (94,725–102,334) | 1.53× | 1.51× |
| rename | ops/s | 118,349 (117,854–120,964) | 80,924 (79,870–82,180) | 79,748 (78,785–80,817) | 1.46× | 1.48× |
| rename-across | ops/s | 104,435 (103,555–104,820) | 64,782 (55,038–67,406) | 66,439 (65,989–66,616) | 1.61× | 1.57× |
| unlink | ops/s | 422,363 (403,987–436,780) | 328,762 (266,509–355,144) | 321,017 (264,813–333,105) | 1.28× | 1.32× |
| create-fsync | ops/s | 396 (367–446) | 2,830 (2,414–2,967) | 2,847 (2,819–2,923) | 0.14× | 0.14× |

- ext4-guest-kernel: mmap-seq-read-faults-per-mib 6.60 median (6.60–6.60)
- ext4-guest-kernel: mtime-ahead-of-guest-ms -4.24 median (-5.52–-2.05)
- ext4-guest-kernel: sqlite-mmap-scan-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-mmap-scan-warm-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- ext4-guest-kernel: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest: mmap-seq-read-faults-per-mib 13.7 median (13.7–13.7)
- fxfs-guest: mtime-ahead-of-guest-ms -2.96 median (-3.20–-2.16)
- fxfs-guest: sqlite-mmap-scan-faults-per-mib 9.60 median (9.60–9.60)
- fxfs-guest: sqlite-mmap-scan-warm-faults-per-mib 9.30 median (9.30–9.30)
- fxfs-guest: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: mmap-seq-read-faults-per-mib 5.50 median (5.50–5.50)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -6.23 median (-6.48–-4.96)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 1.70 median (1.70–1.70)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.60–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 173 | B | $0.000069 |
| get-range | 4,686 | B | $0.001874 |
| head | 0 | B | $0.000000 |
| list | 12 | A | $0.000060 |
| create | 2,244 | A | $0.011220 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 7,118 | | $0.013234 |

Uploaded 4.4 GiB (segments 2.9 GiB, extents 1.5 GiB, checkpoints 46.9 KiB, moves 0 B, other 96 B); downloaded 2.9 GiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 8,760.2 µs |
| read | 60,666 | 93.2 µs |
| write | 60,292 | 716.7 µs |
| free | 56 | 10.3 µs |
| map | 1,696 | 17.3 µs |
| unmap | 783 | 62.8 µs |
| commit | 140 | 125,144.6 µs |
| root | 2 | 1.5 µs |
| info | 8 | 2.7 µs |
| window | 1 | 90.7 µs |
| fault | 12,768 | 337.0 µs |
| transfer | 2 | 8.7 µs |
| free_many | 227 | 1,890.5 µs |
| private | 49,152 | 7.1 µs |

Commits: 13,733, 3,303.2 µs each: committing the store 421.8 µs, waiting for object storage 2,881.2 µs.

Segments closed and framed: 182, 177.5 ms each.

Over 2× slower than ext4:
- rand-read-cold on fxfs-guest: 5.21× ext4-guest-kernel's
- rand-read-cold on fxfs-guest-dax: 2.46× ext4-guest-kernel's
- seq-read-cold on fxfs-guest: 3.41× ext4-guest-kernel's
- seq-read-cold on fxfs-guest-dax: 7.84× ext4-guest-kernel's
- rand-write on fxfs-guest-dax: 2.31× ext4-guest-kernel's
- sqlite-pread-scan on fxfs-guest: 7.88× ext4-guest-kernel's
- sqlite-pread-scan on fxfs-guest-dax: 17.08× ext4-guest-kernel's
- sqlite-mmap-scan on fxfs-guest: 2.89× ext4-guest-kernel's
- read-small-cold on fxfs-guest: 2.89× ext4-guest-kernel's
- setxattr on fxfs-guest: 3.19× ext4-guest-kernel's
- setxattr on fxfs-guest-dax: 2.82× ext4-guest-kernel's
