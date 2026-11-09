Host: linux. VMM: firecracker. Iterations: ext4-guest-kernel 5.

| Workload | Unit | ext4-guest-kernel |
|---|---|---|
| create | ops/s | 74,494 (35,387–100,284) |
| stat-cold | ops/s | 337,143 (150,164–339,020) |
| stat-warm | ops/s | 1,301,474 (1,014,230–1,489,876) |
| lookup-missing | ops/s | 419,172 (350,650–451,834) |
| readdir | ops/s | 3,066,079 (2,931,911–3,121,591) |
| read-small-cold | ops/s | 14,501 (12,879–16,295) |
| chmod | ops/s | 537,461 (454,382–685,169) |
| setxattr | ops/s | 550,247 (543,974–608,652) |
| link | ops/s | 150,285 (139,998–157,288) |
| rename | ops/s | 116,742 (113,437–122,168) |
| rename-across | ops/s | 102,186 (98,534–102,456) |
| unlink | ops/s | 405,852 (362,420–407,935) |
| create-fsync | ops/s | 376 (359–378) |
| mkdir-tree | ops/s | 119,951 (106,475–130,120) |
| find-cold | ops/s | 316,441 (294,845–321,547) |
| rm-tree | ops/s | 107,722 (103,443–112,128) |
| large-dir-create | ops/s | 109,671 (71,137–111,448) |
| large-dir-stat-cold | ops/s | 258,599 (163,849–266,434) |
| large-dir-unlink | ops/s | 210,865 (206,407–211,834) |
| create-parallel | ops/s | 174,497 (165,088–176,906) |
| stat-parallel | ops/s | 627,590 (574,663–649,995) |
| unlink-parallel | ops/s | 299,275 (287,681–311,882) |
| create-shared-dir | ops/s | 108,520 (107,162–110,668) |
| unlink-shared-dir | ops/s | 280,457 (272,679–287,435) |

- ext4-guest-kernel: mtime-ahead-of-guest-ms -3.86 median (-5.20–-3.19)

