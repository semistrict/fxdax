# Wrong theory: the harness's in-memory bucket starving the host causes the decay across VMs

**Tried:** 2026-10-25/26, TASK-108.1, r120 (cac2afe77, n2-standard-8, 32 GB) and r125
(n2-standard-16, 64 GB).
**Idea:** r120 showed the VMM's host time per window change growing 84.6 -> 123.4 us (+46%) over 5
VMs as host free memory fell 12.0 -> 2.4 GB. The bucket is in extentd's memory (memory:///), which
grew ~0.7 GB a VM, so the sandbox team's real bucket on 64 GB hosts would show none of it.
**Result:** r125, with free memory plenty: 66.7 -> 76.9 -> 84.8 -> 91.8 -> 96.3 us, tps first runs
1216 -> 1105. The decay held.
**Why it's dead:** Not host memory. r126 found stage maps per VM growing 1.23M -> 3.45M, runs
averaging 9.5 -> 3.1 pages: extentd's physical memory fragmenting as it recycles at its cap.
**Reopen if:** Nothing. Fragmentation is shown, and fixes to it moved the decay (r130, r134, and the
buddy allocator in TASK-131.1).
