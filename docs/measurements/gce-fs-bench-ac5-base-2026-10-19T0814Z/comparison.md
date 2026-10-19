Host: linux. VMM: firecracker. Iterations: fxfs-guest-dax 5.

| Workload | Unit | fxfs-guest-dax |
|---|---|---|
| create | ops/s | 25,946 (23,905–28,131) |
| stat-cold | ops/s | 301,772 (201,035–351,835) |
| stat-warm | ops/s | 1,403,442 (1,362,714–1,433,985) |
| lookup-missing | ops/s | 447,732 (368,243–555,177) |
| readdir | ops/s | 3,164,151 (2,676,677–3,346,743) |
| read-small-cold | ops/s | 127,210 (125,762–131,940) |
| chmod | ops/s | 1,117,280 (932,268–1,307,640) |
| setxattr | ops/s | 672,200 (222,613–683,512) |
| link | ops/s | 98,418 (88,525–105,965) |
| rename | ops/s | 77,933 (74,139–79,870) |
| rename-across | ops/s | 68,096 (58,495–72,315) |
| unlink | ops/s | 182,549 (166,495–248,904) |
| create-fsync | ops/s | 1,343 (1,078–1,674) |
| mkdir-tree | ops/s | 26,945 (25,778–27,670) |
| find-cold | ops/s | 249,869 (201,334–275,301) |
| rm-tree | ops/s | 65,247 (54,933–73,482) |
| large-dir-create | ops/s | 80,309 (66,765–93,058) |
| large-dir-stat-cold | ops/s | 287,669 (279,874–303,516) |
| large-dir-unlink | ops/s | 205,431 (184,564–235,856) |
| create-parallel | ops/s | 117,505 (101,978–128,043) |
| stat-parallel | ops/s | 632,422 (548,873–654,572) |
| unlink-parallel | ops/s | 201,369 (198,453–210,969) |
| create-shared-dir | ops/s | 86,838 (85,446–94,524) |
| unlink-shared-dir | ops/s | 193,381 (188,318–220,910) |

- fxfs-guest-dax: mtime-ahead-of-guest-ms -3.57 median (-5.71–-2.96)

