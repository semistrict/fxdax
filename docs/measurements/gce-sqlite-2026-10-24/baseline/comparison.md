Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 3, ext4-dax-pmem 3, fxfs-guest 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-guest-kernel | ext4-dax-pmem | fxfs-guest | fxfs-guest-dax | ext4-dax-pmem vs ext4-guest-kernel | fxfs-guest vs ext4-dax-pmem | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|---|---|---|---|
| sqlite-pread-scan | MiB/s | 1,115 (502–2,709) | 183 (181–184) | 134 (131–145) | 784 (722–806) | **6.09×** | 1.37× | 0.23× |
| sqlite-pread-scan-warm | MiB/s | 3,067 (3,021–3,069) | 2,699 (2,303–2,790) | 3,042 (3,034–3,134) | 2,673 (2,667–2,676) | 1.14× | 0.89× | 1.01× |
| sqlite-mmap-scan | MiB/s | 1,381 (1,275–3,221) | 3,353 (3,307–3,387) | 943 (939–959) | 3,261 (3,261–3,424) | 0.41× | **3.55×** | 1.03× |
| sqlite-mmap-scan-warm | MiB/s | 3,758 (3,609–3,772) | 3,692 (3,645–3,758) | 3,528 (3,522–3,614) | 3,606 (3,591–3,690) | 1.02× | 1.05× | 1.02× |
| sqlite-delete-full-batch1 | txn/s | 106 (77.9–107) | 169 (148–183) | 511 (494–540) | 250 (243–250) | 0.63× | 0.33× | 0.67× |
| sqlite-delete-full-batch100 | txn/s | 48.9 (48.9–49.1) | 56.7 (56.3–58.9) | 132 (130–134) | 43.4 (42.5–43.4) | 0.86× | 0.43× | 1.31× |
| sqlite-wal-full-batch1 | txn/s | 295 (248–325) | 362 (334–367) | 1,612 (1,055–1,772) | 570 (560–591) | 0.81× | 0.22× | 0.64× |
| sqlite-wal-full-batch1-checkpoint | ms | 8.47 (6.95–10.7) | 14.2 (14.2–14.5) | 3.91 (3.88–4.23) | 19.1 (15.9–20.2) | 1.68× | 0.27× | 1.34× |
| sqlite-wal-full-batch100 | txn/s | 169 (168–170) | 114 (114–116) | 260 (235–277) | 96.5 (95.5–96.9) | 1.48× | 0.44× | 1.18× |
| sqlite-wal-full-batch100-checkpoint | ms | 19.4 (17.6–21.5) | 64.7 (62.2–68.6) | 15.6 (14.9–16.3) | 95.4 (90.2–103) | **3.33×** | 0.24× | 1.47× |
| sqlite-wal-normal-batch1 | txn/s | 28,715 (27,572–29,056) | 14,494 (13,653–14,796) | 51,405 (50,288–51,479) | 12,255 (11,533–12,308) | 1.98× | 0.28× | 1.18× |
| sqlite-wal-normal-batch1-checkpoint | ms | 13.3 (12.7–13.8) | 18.0 (17.6–19.9) | 6.10 (5.84–85.7) | 18.1 (15.3–18.3) | 1.35× | 0.34× | 1.00× |
| sqlite-wal-normal-batch100 | txn/s | 940 (910–980) | 268 (243–269) | 1,016 (1,005–1,043) | 177 (167–193) | **3.51×** | 0.26× | 1.51× |
| sqlite-wal-normal-batch100-checkpoint | ms | 406 (362–408) | 374 (372–394) | 174 (173–220) | 225 (130–318) | 0.92× | 0.46× | 0.60× |

- ext4-guest-kernel: mtime-ahead-of-guest-ms -3.06 median (-4.05–-0.64)
- ext4-guest-kernel: sqlite-delete-full-batch1-updates-per-second 106 median (77.9–107)
- ext4-guest-kernel: sqlite-delete-full-batch100-updates-per-second 4,894 median (4,890–4,910)
- ext4-guest-kernel: sqlite-mmap-scan-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-mmap-scan-warm-faults-per-mib 2.10 median (2.10–2.10)
- ext4-guest-kernel: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- ext4-guest-kernel: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-guest-kernel: sqlite-wal-full-batch1-updates-per-second 295 median (248–325)
- ext4-guest-kernel: sqlite-wal-full-batch100-updates-per-second 16,886 median (16,842–16,952)
- ext4-guest-kernel: sqlite-wal-normal-batch1-updates-per-second 28,716 median (27,572–29,056)
- ext4-guest-kernel: sqlite-wal-normal-batch100-updates-per-second 94,004 median (91,027–98,026)
- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.05 median (-5.09–-3.56)
- ext4-dax-pmem: sqlite-delete-full-batch1-updates-per-second 168 median (148–183)
- ext4-dax-pmem: sqlite-delete-full-batch100-updates-per-second 5,673 median (5,631–5,890)
- ext4-dax-pmem: sqlite-mmap-scan-faults-per-mib 3.70 median (3.70–3.70)
- ext4-dax-pmem: sqlite-mmap-scan-warm-faults-per-mib 3.60 median (3.60–3.60)
- ext4-dax-pmem: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- ext4-dax-pmem: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- ext4-dax-pmem: sqlite-wal-full-batch1-updates-per-second 362 median (334–367)
- ext4-dax-pmem: sqlite-wal-full-batch100-updates-per-second 11,403 median (11,362–11,575)
- ext4-dax-pmem: sqlite-wal-normal-batch1-updates-per-second 14,494 median (13,653–14,796)
- ext4-dax-pmem: sqlite-wal-normal-batch100-updates-per-second 26,775 median (24,333–26,932)
- fxfs-guest: mtime-ahead-of-guest-ms -3.65 median (-4.83–-3.30)
- fxfs-guest: sqlite-delete-full-batch1-updates-per-second 511 median (494–540)
- fxfs-guest: sqlite-delete-full-batch100-updates-per-second 13,225 median (13,041–13,357)
- fxfs-guest: sqlite-mmap-scan-faults-per-mib 9.60 median (9.60–9.60)
- fxfs-guest: sqlite-mmap-scan-warm-faults-per-mib 9.30 median (9.10–9.30)
- fxfs-guest: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest: sqlite-wal-full-batch1-updates-per-second 1,612 median (1,055–1,772)
- fxfs-guest: sqlite-wal-full-batch100-updates-per-second 26,043 median (23,532–27,709)
- fxfs-guest: sqlite-wal-normal-batch1-updates-per-second 51,405 median (50,288–51,479)
- fxfs-guest: sqlite-wal-normal-batch100-updates-per-second 101,607 median (100,472–104,281)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -5.42 median (-7.16–-3.64)
- fxfs-guest-dax: sqlite-delete-full-batch1-updates-per-second 250 median (243–250)
- fxfs-guest-dax: sqlite-delete-full-batch100-updates-per-second 4,337 median (4,252–4,342)
- fxfs-guest-dax: sqlite-mmap-scan-faults-per-mib 5.60 median (3.70–9.60)
- fxfs-guest-dax: sqlite-mmap-scan-warm-faults-per-mib 5.60 median (3.70–9.60)
- fxfs-guest-dax: sqlite-pread-scan-faults-per-mib 2.70 median (2.70–2.70)
- fxfs-guest-dax: sqlite-pread-scan-warm-faults-per-mib 2.60 median (2.60–2.60)
- fxfs-guest-dax: sqlite-wal-full-batch1-updates-per-second 570 median (560–591)
- fxfs-guest-dax: sqlite-wal-full-batch100-updates-per-second 9,646 median (9,555–9,694)
- fxfs-guest-dax: sqlite-wal-normal-batch1-updates-per-second 12,255 median (11,533–12,308)
- fxfs-guest-dax: sqlite-wal-normal-batch100-updates-per-second 17,709 median (16,736–19,265)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 153 | B | $0.000061 |
| get-range | 2 | B | $0.000001 |
| head | 0 | B | $0.000000 |
| list | 9 | A | $0.000045 |
| create | 698 | A | $0.003490 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 865 | | $0.003607 |

Uploaded 4.2 GiB (segments 3.9 GiB, extents 326.8 MiB, checkpoints 13.1 KiB, moves 0 B, other 66 B); downloaded 3.4 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 2 | 8,378.8 µs |
| read | 7,928 | 1,382.3 µs |
| write | 52,556 | 609.4 µs |
| free | 1,271 | 99.6 µs |
| map | 114,446 | 133.0 µs |
| unmap | 21,703 | 26.5 µs |
| commit | 83 | 10,633.6 µs |
| root | 2 | 1.4 µs |
| info | 8 | 1.1 µs |
| window | 1 | 10.7 µs |
| fault | 3 | 43.8 µs |
| transfer | 2 | 15.4 µs |
| free_many | 2,806 | 183.7 µs |

Commits: 13,084, 1,731.3 µs each: writing the open extents' 161.7 KiB 115.2 µs, committing the store 137.1 µs, sealing the windows' mappings 57.1 µs, waiting for object storage 1,421.6 µs.

Segments closed and framed: 145, 57.8 ms each.

Over 2× slower than ext4:
- sqlite-pread-scan on ext4-dax-pmem: 6.09× ext4-guest-kernel's
- sqlite-mmap-scan on fxfs-guest: 3.55× ext4-dax-pmem's
- sqlite-wal-full-batch100-checkpoint on ext4-dax-pmem: 3.33× ext4-guest-kernel's
- sqlite-wal-normal-batch100 on ext4-dax-pmem: 3.51× ext4-guest-kernel's
