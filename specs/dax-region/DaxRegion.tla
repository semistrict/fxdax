------------------------------ MODULE DaxRegion ------------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* A region of the guest Fxfs's DAX window (guest/linux/fs/fxfs/dax.c):   *)
(* the window's mapping of an extent, which a reader and a writer share   *)
(* (TASK-44.14, TASK-99). The guest believes the region writable or not   *)
(* (region->writable) and mapped or not (region->mapped); the host maps   *)
(* it read-only or writably, as the MAP that mapped it said.              *)
(*                                                                         *)
(* A reader that took the extent for sealed maps the region outside the  *)
(* gate: under the region's lock, it reads whether the region is writable,*)
(* sends the MAP, whose reply comes later, and sets mapped (map_to). A    *)
(* writer, in the gate, makes the region writable unless it is mapped     *)
(* already (region_open), maps it as far as it needs unless it is mapped  *)
(* (map_to), and, if the region is writable, has the guest store to it.   *)
(*                                                                         *)
(* TASK-99: region_open looked at mapped without the region's lock, while *)
(* a reader's read-only MAP was in flight under it: the region made       *)
(* writable, the MAP ended read-only and set mapped, the writer's map_to  *)
(* found it mapped and sent nothing, and the guest stored to pages the    *)
(* host had read-only, which the host refused until the VM stopped.       *)
(*                                                                         *)
(* Its rule, openUnderLock: region_open under the region's lock, which    *)
(* specs/dax-region/mutations removes, and must find the store the host   *)
(* refuses. With it, map_to's reading writable twice, for the MAP's length *)
(* and for the MAP, as the code did, is harmless: nothing changes it while *)
(* map_to holds the lock (checked with a rule of its own, which held      *)
(* removed). map_to reads it once now all the same.                       *)
(***************************************************************************)
EXTENDS Naturals, TLC

CONSTANT Rules

AllRules == {"openUnderLock"}

ASSUME Rules \subseteq AllRules

VARIABLES
    writable,   \* region->writable: what the guest believes of it
    mapped,     \* region->mapped: whether the guest takes it for mapped
    host,       \* the host's mapping of it: "none", "ro" or "rw"
    lock,       \* who holds the region's lock: "reader", "writer" or "none"
    rpc, rWritable,     \* the reader, and what it read of writable
    wpc, wWritable      \* the writer, and what it read of writable

vars == <<writable, mapped, host, lock, rpc, rWritable, wpc, wWritable>>

Init ==
    /\ writable = FALSE
    /\ mapped = FALSE
    /\ host = "none"
    /\ lock = "none"
    /\ rpc = "lock" /\ rWritable = FALSE
    /\ wpc = "open" /\ wWritable = FALSE

Prot(w) == IF w THEN "rw" ELSE "ro"

(* The reader: map_to, outside the gate, for a read-only mapping it may find writable. *)
ReaderLock ==
    /\ rpc = "lock" /\ lock = "none"
    /\ lock' = "reader"
    /\ rpc' = IF mapped THEN "unlock" ELSE "read"
    /\ UNCHANGED <<writable, mapped, host, rWritable, wpc, wWritable>>

ReaderRead ==
    /\ rpc = "read"
    /\ rWritable' = writable
    /\ rpc' = "map"
    /\ UNCHANGED <<writable, mapped, host, lock, wpc, wWritable>>

(* The MAP, sent and answered: as writable as map_to read. *)
ReaderMap ==
    /\ rpc = "map"
    /\ host' = Prot(rWritable)
    /\ rpc' = "mapped"
    /\ UNCHANGED <<writable, mapped, lock, rWritable, wpc, wWritable>>

ReaderMapped ==
    /\ rpc = "mapped"
    /\ mapped' = TRUE
    /\ rpc' = "unlock"
    /\ UNCHANGED <<writable, host, lock, rWritable, wpc, wWritable>>

ReaderUnlock ==
    /\ rpc = "unlock"
    /\ lock' = "none"
    /\ rpc' = "done"
    /\ UNCHANGED <<writable, mapped, host, rWritable, wpc, wWritable>>

(* The writer: region_open, under the region's lock with openUnderLock, else under the regions *)
(* spinlock alone, which a MAP doesn't hold.                                                     *)
WriterOpen ==
    /\ wpc = "open"
    /\ "openUnderLock" \notin Rules \/ lock = "none"
    /\ writable' = (writable \/ ~mapped)
    /\ wpc' = "lock"
    /\ UNCHANGED <<mapped, host, lock, rpc, rWritable, wWritable>>

(* map_to, as reach calls it: nothing if the region is mapped; else a MAP as writable as read. *)
WriterLock ==
    /\ wpc = "lock" /\ lock = "none"
    /\ lock' = "writer"
    /\ wpc' = IF mapped THEN "unlock" ELSE "read"
    /\ UNCHANGED <<writable, mapped, host, rpc, rWritable, wWritable>>

WriterRead ==
    /\ wpc = "read"
    /\ wWritable' = writable
    /\ wpc' = "map"
    /\ UNCHANGED <<writable, mapped, host, lock, rpc, rWritable>>

WriterMap ==
    /\ wpc = "map"
    /\ host' = Prot(wWritable)
    /\ mapped' = TRUE
    /\ wpc' = "unlock"
    /\ UNCHANGED <<writable, lock, rpc, rWritable, wWritable>>

WriterUnlock ==
    /\ wpc = "unlock"
    /\ lock' = "none"
    /\ wpc' = IF writable THEN "store" ELSE "done"
    /\ UNCHANGED <<writable, mapped, host, rpc, rWritable, wWritable>>

(* The guest stores to the region, which it took for writable: and is done. *)
WriterStore ==
    /\ wpc = "store"
    /\ wpc' = "stored"
    /\ UNCHANGED <<writable, mapped, host, lock, rpc, rWritable, wWritable>>

Done ==
    /\ rpc = "done" /\ wpc \in {"done", "stored"}
    /\ UNCHANGED vars

Next ==
    \/ ReaderLock \/ ReaderRead \/ ReaderMap \/ ReaderMapped \/ ReaderUnlock
    \/ WriterOpen \/ WriterLock \/ WriterRead \/ WriterMap \/ WriterUnlock \/ WriterStore
    \/ Done

Spec == Init /\ [][Next]_vars

(* A store the guest makes is to pages the host maps writably: else the host refuses it, and the *)
(* vCPU's KVM_RUN fails with EFAULT until its VM stops (TASK-99).                                *)
NoRefusedStores == wpc = "stored" => host = "rw"
=============================================================================
