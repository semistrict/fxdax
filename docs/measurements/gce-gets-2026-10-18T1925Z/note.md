Ranged GETs from GCS by part size and GETs in flight, from a c3-standard-8 host (Sapphire Rapids, 8 vCPUs) in us-east4, the bucket's region (gs://echophase-fxdax-bench): `scripts/gets-gce.sh`, `fxdax-gets` (TASK-28.11; doc-7, decision 9). The table is gets.md: an object of 512 MiB, random parts of each size, 10 s each.

What it shows:

- **A GET costs about 20 ms whatever its size up to 1 MiB** (p50 19–23 ms with up to 32 in flight), then more: 26 ms at 2 MiB, 33 ms at 4 MiB, with one in flight. Smaller parts save a page-in no time.
- **Throughput grows with GETs in flight until the host's network is full**, about 2.2 GiB/s (4 MiB parts, 64 in flight), and then only latency grows. With 1 MiB parts: 1.3 GiB/s at 32 in flight (p50 23 ms), 1.6 GiB/s at 64 (p50 36 ms), 1.7 GiB/s at 128 (p50 63 ms).

Chosen settings, unchanged:

- **Parts of 1 MiB** (`extent_frame::PART`): a demand page-in, which fetches the part its page is in, waits about the floor of a GET (21 ms), and a part streams at 47 MiB/s alone. 2 MiB parts would add 5 ms to every demand page-in for more throughput only under load; 256 KiB ones save 2 ms and take four GETs, billed each, for what one takes.
- **64 GETs in flight, 64 MiB, per host** (`fxdax::remote`'s budget): 1.6 GiB/s of 1 MiB parts, three quarters of what the host's network takes, with demand reads, which go first, waiting no more than 36 ms at the median when it is full; 128 would add 5% for twice the latency.
