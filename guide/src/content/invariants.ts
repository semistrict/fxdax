// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import type { Invariant } from "./types.ts";

// The claims that let the reader predict the code. Each is stated as the design docs and the
// code's own docs state it, and cites what enforces it and what shows it needed.

export const invariants: Invariant[] = [
  {
    id: "write-once",
    layer: "disk",
    claim:
      "An extent is written only from its first WRITE until the next COMMIT, which seals it. A " +
      "sealed extent never changes, and is freed whole.",
    why:
      "Everything above leans on this: a sealed extent can be cached once, mapped by any number " +
      "of VMs, uploaded once, and inherited by forks, with no invalidation anywhere.",
    enforcedBy: ["store", "es_write", "es_commit"],
    evidence: [{ kind: "doc", id: "doc-4" }, { kind: "doc", id: "doc-7" }],
    standing: "stated",
  },
  {
    id: "host-sees-extents",
    layer: "driver",
    claim:
      "The host sees extents and bytes, never files. The guest's Fxfs owns all metadata; the " +
      "device's requests name extents by (branch, sequence).",
    why:
      "The host stays a store of immutable pieces: no filesystem semantics to get right twice, and " +
      "a guest's metadata is as private as its data.",
    enforcedBy: ["device_trait", "extent_id", "extent_device"],
    evidence: [{ kind: "doc", id: "doc-2" }, { kind: "doc", id: "doc-4" }],
    standing: "stated",
  },
  {
    id: "sealed-only",
    layer: "daemon",
    claim:
      "A DAX window maps sealed extents or zeros, read-only. A MAP of an open extent fails EPERM; " +
      "open extents are written only by WRITE.",
    why:
      "Nothing a window maps can change, so a seal (COMMIT) changes nothing in any window. Writable " +
      "DAX broke exactly here: every fsync changed what mapped pages were.",
    enforcedBy: ["es_map", "es_commit"],
    evidence: [
      { kind: "doc", id: "doc-13", heading: "Invariants" },
      { kind: "mutation", spec: "dax-cow", name: "NoKeepUntilSealed" },
      { kind: "mutation", spec: "dax-cow", name: "NoRebindAfterSeal" },
      { kind: "deadEnd", file: "writable-dax.md" },
    ],
    standing: "checked",
  },
  {
    id: "three-kinds",
    layer: "daemon",
    claim:
      "A window page is one of three: a sealed extent's page, shared by every window that maps " +
      "it; zeros; or a private page, the VM's own, which no extent names. A store makes a page " +
      "private at the same window address; only the guest's MAP or UNMAP over it ends it.",
    why:
      "No store moves a page, so no guest PTE changes for a store, and the host can copy on write " +
      "without the guest's page tables knowing.",
    enforcedBy: ["window_make_private", "cow", "second_stage"],
    evidence: [{ kind: "doc", id: "doc-13", heading: "Invariants" }],
    standing: "stated",
  },
  {
    id: "dirty-implies-private",
    layer: "guest",
    claim:
      "Before the guest puts a page in its dirty set, it has the host make the page private " +
      "(PRIVATE). A store never makes the host copy a page.",
    why:
      "A fault's store lands after its iomap_end, maybe after a writeback moved the page's block " +
      "and a seal freed the old one: a copy made at the store would copy that freed block's zeros.",
    enforcedBy: ["guest_dirty", "guest_device_private", "server_make_private"],
    evidence: [
      { kind: "mutation", spec: "dax-cow", name: "NoPrivateAtIomap" },
      { kind: "mutant", file: "task-164.9-private-failure-ignored.patch" },
      { kind: "mutant", file: "task-164.4-writeback-unowned.patch" },
    ],
    standing: "checked",
  },
  {
    id: "exclusive-swap-rebind",
    layer: "guest",
    claim:
      "Writeback's swap of the dirty set, and a rebind (a private page mapped back to its sealed " +
      "extent), each hold the file's semaphore exclusively.",
    why:
      "No write(2) may straddle them: one that dirtied its page before the swap and copies its " +
      "bytes after the writeback's copy is in no set, and the next fsync writes nothing of it.",
    enforcedBy: ["guest_swap", "pages_swap", "guest_rebind_range"],
    evidence: [
      { kind: "doc", id: "doc-13", heading: "Invariants" },
      { kind: "mutation", spec: "dax-cow", name: "NoSwapExclusive" },
      { kind: "mutation", spec: "dax-cow", name: "NoRebindExclusive" },
      { kind: "mutant", file: "task-164.4-swap-shared.patch" },
      { kind: "mutant", file: "task-164.4-rebind-shared.patch" },
    ],
    standing: "checked",
  },
  {
    id: "private-max",
    layer: "vmm",
    claim:
      "A device holds at most private_max private pages, 1 GiB unless configured, whatever the " +
      "window's size. The guest counts its pages and waits for room; the host refuses a store past " +
      "the bound, and that VM alone stops.",
    why: "Memory for writes is bounded per VM, reserved against its tenant, without asking any other VM.",
    enforcedBy: ["vmm_private_max", "guest_dirty"],
    evidence: [
      { kind: "doc", id: "doc-13", heading: "Invariants" },
      { kind: "mutation", spec: "dax-cow", name: "NoCountPrivate" },
      { kind: "mutant", file: "task-164.4-private-uncounted.patch" },
    ],
    standing: "checked",
  },
  {
    id: "bytes-first",
    layer: "guest",
    claim:
      "Writeback commits the transaction naming a new extent only after the extent's bytes are " +
      "written. A crash shows a page's old data or its new, never what an extent held unwritten.",
    why: "A seal can come at any moment (another file's fsync, the journal); it must never make garbage durable.",
    enforcedBy: ["guest_write_back_keeping", "es_write", "es_commit"],
    evidence: [{ kind: "mutation", spec: "dax-cow", name: "NoBytesFirst" }],
    standing: "checked",
  },
  {
    id: "fsync-ordered",
    layer: "daemon",
    claim:
      "An fsync returns once its commit is ordered: in extentd, its data in the store's files. It " +
      "is durable on the host's disk within 50 ms at the median, 1 s at most; sync, syncfs, " +
      "unmount and freeze wait for durable, and with a bucket, for uploaded.",
    why:
      "doc-2's rule: a crash may lose a suffix of recent history, never a middle. fsync keeps its " +
      "ordering without a round trip to the disk.",
    enforcedBy: ["dev_commit_root", "es_commit_ordered", "flush_behind", "es_durable"],
    evidence: [{ kind: "doc", id: "doc-12" }, { kind: "doc", id: "doc-2" }],
    standing: "stated",
  },
  {
    id: "one-copy",
    layer: "memory",
    claim:
      "A sealed extent's pages are cached once per host, keyed by its ID, and shared by every " +
      "store, window and fork that maps it.",
    why:
      "Hundreds of forks of one base cost one copy of what they share: each VM pays only for what " +
      "it alone reaches, and for its private pages.",
    enforcedBy: ["extent_cache", "cache_cached", "cached_extent"],
    evidence: [{ kind: "doc", id: "doc-2" }, { kind: "doc", id: "doc-13" }],
    standing: "stated",
  },
  {
    id: "fork-inherits",
    layer: "bucket",
    claim:
      "An extent's ID is minted once, by its branch's writer. A fork is a new branch that " +
      "inherits its parent's extents as of a durable commit, without copying them.",
    why: "IDs never collide across writers or forks, so nothing inherited ever needs renaming or copying.",
    enforcedBy: ["extent_id", "es_fork", "fork_point", "store_commit_fork"],
    evidence: [
      { kind: "doc", id: "doc-7" },
      { kind: "mutation", spec: "bucket", name: "NoEpochSequences" },
    ],
    standing: "checked",
  },
  {
    id: "one-writer",
    layer: "bucket",
    claim:
      "A branch has one writer, holding its lease. Its segments are numbered without gaps, each " +
      "naming the one it follows; once a new writer's first segment (its fence) is in the bucket, " +
      "the old writer can add nothing.",
    why: "A host that lost its lease but still runs can't corrupt the branch: object storage's conditional writes do the fencing.",
    enforcedBy: ["upload", "acknowledge", "bucket"],
    evidence: [
      { kind: "doc", id: "doc-7" },
      { kind: "mutation", spec: "bucket", name: "NoFence" },
      { kind: "mutation", spec: "bucket", name: "NoPinsFences" },
      { kind: "mutation", spec: "bucket", name: "NoPinsOldFences" },
      { kind: "mutation", spec: "bucket", name: "NoPinsVoidStrays" },
      { kind: "mutation", spec: "bucket", name: "NoPinsLateStrays" },
      { kind: "mutation", spec: "bucket", name: "NoPinsTakings" },
      { kind: "mutation", spec: "bucket", name: "NoForksAfterLease" },
      { kind: "mutation", spec: "bucket", name: "NoContinuity" },
    ],
    standing: "checked",
  },
];
