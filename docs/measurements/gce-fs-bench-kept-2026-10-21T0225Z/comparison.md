Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 3, fxfs-guest-dax 3.

| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| pgbench-init | s | 3.29 (3.16–3.31) | 4.07 (3.97–4.22) | 1.24× |
| pgbench-tps | tps | 2,025 (1,991–2,056) | 1,359 (1,324–1,373) | 1.49× |

- ext4-dax-pmem: mtime-ahead-of-guest-ms -4.33 median (-5.19–-2.73)
- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.72 median (-109.64–-2.97)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 98 | B | $0.000039 |
| get-range | 32 | B | $0.000013 |
| head | 0 | B | $0.000000 |
| list | 3 | A | $0.000015 |
| create | 118 | A | $0.000590 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 254 | | $0.000667 |

Uploaded 122.1 MiB (segments 122.1 MiB, extents 0 B, checkpoints 16.0 KiB, moves 0 B, other 39 B); downloaded 2.2 KiB.

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| open | 1 | 1,197,578.2 µs |
| read | 976 | 518.3 µs |
| write | 65,660 | 179.8 µs |
| free | 121,345 | 47.9 µs |
| map | 76,883 | 32.1 µs |
| unmap | 59,819 | 23.4 µs |
| commit | 129 | 106,541.3 µs |
| root | 1 | 0.8 µs |
| info | 4 | 0.8 µs |
| window | 1 | 34.4 µs |
| transfer | 1 | 9.3 µs |

Commits: 59,156, 411.4 µs each: writing the open extents' 29.2 KiB 30.3 µs, committing the store 106.5 µs, sealing the windows' mappings 40.7 µs, waiting for object storage 233.5 µs.

Every workload is within 2× of ext4.
