Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 3.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| seq-write | MiB/s | 70.1 (65.8–70.3) |
| rand-read-cold | IOPS | 144,991 (113,778–218,453) |
| seq-read-cold | MiB/s | 962 (729–1,571) |
| seq-read-warm | MiB/s | 11,227 (11,221–11,324) |
| rand-read-warm | IOPS | 813,636 (811,467–816,023) |
| rand-write | IOPS | 11,821 (11,821–14,415) |
| mmap-rand-read | IOPS | 1,143,156 (1,126,359–1,155,126) |
| mmap-rand-write | IOPS | 11,818 (11,817–13,205) |
| fsync-append | IOPS | 402 (402–638) |
| create | ops/s | 27,281 (23,076–27,441) |
| stat-cold | ops/s | 270,982 (187,214–299,233) |
| stat-warm | ops/s | 1,391,346 (1,322,500–1,399,920) |
| lookup-missing | ops/s | 399,581 (346,178–490,016) |
| readdir | ops/s | 2,128,343 (2,126,836–2,264,972) |
| read-small-cold | ops/s | 117,095 (115,943–117,974) |
| chmod | ops/s | 1,225,422 (858,911–1,447,867) |
| setxattr | ops/s | 619,965 (306,418–622,398) |
| link | ops/s | 96,588 (92,061–97,145) |
| rename | ops/s | 79,577 (72,317–84,105) |
| rename-across | ops/s | 72,901 (62,097–76,317) |
| unlink | ops/s | 191,616 (177,541–202,263) |
| create-fsync | ops/s | 854 (717–1,656) |
| mkdir-tree | ops/s | 27,457 (27,027–28,344) |
| find-cold | ops/s | 189,724 (183,826–226,680) |
| rm-tree | ops/s | 62,190 (61,522–67,417) |
| large-dir-create | ops/s | 81,341 (60,583–89,693) |
| large-dir-stat-cold | ops/s | 278,725 (188,719–279,607) |
| large-dir-unlink | ops/s | 186,295 (183,265–223,719) |
| create-parallel | ops/s | 118,468 (117,664–122,661) |
| stat-parallel | ops/s | 543,381 (384,194–604,588) |
| unlink-parallel | ops/s | 196,324 (176,209–198,044) |
| create-shared-dir | ops/s | 89,651 (84,433–90,734) |
| unlink-shared-dir | ops/s | 174,614 (164,091–178,360) |
| untar | s | 0.03 (0.02–0.05) |
| make | s | 4.54 (4.51–4.62) |
| build-total | s | 4.56 (4.54–4.67) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -1.31 median (-1.73–-1.30)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 43 | B | $0.000017 |
| get-range | 0 | B | $0.000000 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 48 | A | $0.000240 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 97 | | $0.000282 |

Uploaded 704.4 MiB (segments 704.4 MiB, extents 0 B, checkpoints 0 B, moves 0 B, other 39 B); downloaded 918 B.

