// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The guest's past bugs (guest/mutants) that each DaxCow rule's absence stands for, where one
// exists: the same mistake in dax.c, put back, and the guest/sim scenario that catches it. The
// spec's own header says where none does (keepUntilSealed, rebindAfterSeal).

export const ruleMutants: Record<string, string[]> = {
  privateAtIomap: ["task-164.9-private-failure-ignored.patch", "task-164.4-writeback-unowned.patch"],
  protectByPageTables: ["task-164.4-protect-by-entries.patch", "task-164.4-wb-sync-none.patch"],
  swapExclusive: ["task-164.4-swap-shared.patch"],
  rebindExclusive: ["task-164.4-rebind-shared.patch"],
  countPrivate: ["task-164.4-private-uncounted.patch"],
};
