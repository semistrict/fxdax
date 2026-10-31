# UNMAP mapping zeros in place of what it unmaps, for followers' late window faults

**Tried:** 2026-10-18, TASK-42.9, 77897a5, later reverted.
**Idea:** A follower's guest touched a window page 3-7 ms after its own UNMAP and the VMM aborted on
ENOENT. With zeros there, the access finds something to map, as when an extent is freed.
**Result:** Torture passed and no reader saw a zero page, but it was harmless, not understood. Ruled
out one GCE run at a time before the cause was found: async page faults, vCPU races, the /proc/self/
mem reader, DAX regions, two ranges owning space, stale TLBs, surviving PTEs, two inodes.
**Why it's dead:** The accesses were x86 KVM's own: its async page fault re-faults when the page is
ready, even without the guest's PV async PF, after the guest had dropped the range. The no-kvmapf
run ruled nothing out, as it turns off only the PV kind. The fix withholds unmapped pages, so the
re-fault fails with EFAULT, which KVM drops, and gives them back on the next map (63356ac, e1207b8,
dc20010). Withholding alone failed 10 of 10 until the window told its stage about new maps.
**Reopen if:** Nothing. Remember that no-kvmapf doesn't disable KVM's async page faults.
