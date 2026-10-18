Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax-fork-warm 5.

| Workload | Unit | fxfs-guest-dax-fork-warm |
|---|---|---|
| base-copy-usr | s | 2.90 (2.90–2.90) |
| base-pgbench-init | s | 12.6 (12.6–12.6) |
| fork-mount | s | 0.37 (0.35–0.38) |
| fork-read-usr | s | 0.62 (0.61–0.62) |
| fork-pg-start | s | 0.25 (0.25–0.25) |
| fork-pgbench-select | tps | 39,532 (38,721–41,072) |

- fxfs-guest-dax-fork-warm: extentd-fault-ns 1,635,394 median (1,585,648–1,669,229)
- fxfs-guest-dax-fork-warm: extentd-faults 246 median (246–246)
- fxfs-guest-dax-fork-warm: extentd-fetched-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-map-ns 45,342,423 median (44,857,942–47,392,281)
- fxfs-guest-dax-fork-warm: extentd-maps 387 median (387–387)
- fxfs-guest-dax-fork-warm: extentd-page-in-bytes 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-page-ins 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-prefetches 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: extentd-read-bytes 4,771,840 median (4,771,840–4,771,840)
- fxfs-guest-dax-fork-warm: extentd-read-stored-bytes 4,771,840 median (4,771,840–4,771,840)
- fxfs-guest-dax-fork-warm: extentd-reads 758 median (758–758)
- fxfs-guest-dax-fork-warm: extentd-unpacks 16.0 median (16.0–16.0)
- fxfs-guest-dax-fork-warm: guest-advance-ms 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-advance-requests 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-commit-ms 11.3 median (10.3–13.0)
- fxfs-guest-dax-fork-warm: guest-commit-requests 3.00 median (3.00–3.00)
- fxfs-guest-dax-fork-warm: guest-free-ms 333 median (293–377)
- fxfs-guest-dax-fork-warm: guest-free-requests 2,169 median (2,169–2,169)
- fxfs-guest-dax-fork-warm: guest-freeze-ms 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-freeze-requests 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-info-ms 0.57 median (0.56–0.61)
- fxfs-guest-dax-fork-warm: guest-info-requests 3.00 median (3.00–3.00)
- fxfs-guest-dax-fork-warm: guest-let_go-ms 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-let_go-requests 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-map-ms 265 median (264–269)
- fxfs-guest-dax-fork-warm: guest-map-requests 386 median (386–386)
- fxfs-guest-dax-fork-warm: guest-pin-ms 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-pin-requests 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-read-ms 146 median (136–163)
- fxfs-guest-dax-fork-warm: guest-read-requests 758 median (758–758)
- fxfs-guest-dax-fork-warm: guest-resized-ms 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-resized-requests 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-root-ms 0.54 median (0.46–1.86)
- fxfs-guest-dax-fork-warm: guest-root-requests 4.00 median (4.00–4.00)
- fxfs-guest-dax-fork-warm: guest-unmap-ms 5.83 median (5.03–7.55)
- fxfs-guest-dax-fork-warm: guest-unmap-requests 25.0 median (25.0–25.0)
- fxfs-guest-dax-fork-warm: guest-unpin-ms 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-unpin-requests 0.00 median (0.00–0.00)
- fxfs-guest-dax-fork-warm: guest-write-ms 15.4 median (14.4–18.9)
- fxfs-guest-dax-fork-warm: guest-write-requests 53.0 median (53.0–53.0)

Object storage: the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 172 | B | $0.000069 |
| get-range | 192 | B | $0.000077 |
| head | 0 | B | $0.000000 |
| list | 68 | A | $0.000340 |
| create | 111 | A | $0.000555 |
| update | 30 | A | $0.000150 |
| overwrite | 0 | A | $0.000000 |
| delete | 12 | free | $0.000000 |
| all | 585 | | $0.001191 |

Uploaded 191.6 MiB (segments 191.5 MiB, extents 0 B, checkpoints 90.1 KiB, moves 0 B, other 530 B); downloaded 202.7 KiB.

