# Wrong theory: the stage's trim is most of the per-MAP cost, and raising vm.max_map_count fixes it

**Tried:** 2026-10-24 to 26, TASK-91.16. r66 (profile), r68, r142 (FXDAX_MAX_MAP_COUNT, 4f193124c).
**Idea:** r66 showed TLB shootdowns from `leave_out` hottest. `leave_out` also runs for trim(), past
a budget of half vm.max_map_count, and the guest held 28-30k regions against ~30.7k, so trim's churn
looked like the cost and like the 8 GiB window's decay.
**Result:** r68: 53k mappings at 65530 and 113k at 1048576; `leave_out` 6.6% then 7.4% of samples;
tps 928/844/849/828/814 against 955/890/884/881/881 (+3-8%). r142, one host: equal, 1119/1051/1025/
984/989 against 1134/1048/1024/967/980; leave-outs 542.7k / 543.0k, KVM remote flushes 643k / 640k.
**Why it's dead:** Trim takes nothing away. Leave-outs come from the guest's UNMAPs (~265k) and from
remaps whose maps leave installed pages uncovered (~278k). The decay is the same at either setting.
**Reopen if:** A workload holds far more window runs than half vm.max_map_count, so trim really
runs. The +3-8% of r68 did not repeat in r142.
