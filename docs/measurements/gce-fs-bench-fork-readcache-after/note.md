After TASK-32: as gce-fs-bench-fork-readcache-before, on the same host (c3-standard-8), with READs of sealed extents from the extent cache. The case forks a base whose daemon served it (warm), 5 iterations.

Before and after, medians (ranges in comparison.md):

| | before | after |
|---|---:|---:|
| bytes READ from the store, per fork | 4,771,840 | 4,096 |
| page-ins, per fork | 0 | 1 (700 in the first fork, which reads the base's metadata into the cache) |
| the guest's time in READs (758–770 of them) | 146 ms | 105 ms |
| fork-mount | 0.37 s | 0.35 s |
| fork-read-usr | 0.62 s | 0.62 s |
| fork-pgbench-select | 39,532 tps | 40,769 tps |

Forks after the first read the base's metadata, which the guest's Fxfs READs (it maps only files' data), from memory: nothing from the store but an open extent. Mount is a little faster (its range overlaps the one before); reading /usr, whose data the window maps, is not changed.
