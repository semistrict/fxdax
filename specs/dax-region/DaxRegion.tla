------------------------------ MODULE DaxRegion ------------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* A region of the guest Fxfs's DAX window (guest/linux/fs/fxfs/dax.c):   *)
(* the window's mapping of an extent, which a reader and a writer might   *)
(* share (TASK-44.14, TASK-99, TASK-153.3). The guest believes the region *)
(* writable or not (region->writable) and mapped or not (region->mapped); *)
(* the host maps it read-only or writably, as the MAP that mapped it said.*)
(*                                                                         *)
(* An extent goes open, then sealed, and never back (`OpenExtents` in     *)
(* crates/storage_device). A writer that found it open, in the gate,      *)
(* which keeps a seal out, makes the region writable (region_open), maps  *)
(* it as far as it needs unless it is mapped (map_to), and has the guest  *)
(* store to it. A reader that found it sealed maps the region outside the *)
(* gate: under the region's lock, it reads whether the region is writable,*)
(* sends the MAP, whose reply comes later, and sets mapped (map_to). So   *)
(* they don't meet, for two reasons, each a rule here:                    *)
(*                                                                         *)
(* - oneWay: nothing has an extent read as open after sealed. A device   *)
(*   write did (TASK-99): the region mapped read-only for a reader of an  *)
(*   extent read as sealed, a writer's map_to found it mapped and sent    *)
(*   nothing, and the guest stored to pages the host had read-only, which *)
(*   the host refused until the VM stopped.                               *)
(* - sharedApart: blocks of an open extent a clone shares, which read as  *)
(*   not writable though the extent is open, don't go through its region. *)
(*   They did: a reader outside the gate sent a writable MAP, across a    *)
(*   seal (TASK-44.21, TASK-150.11).                                      *)
(*                                                                         *)
(* Region_open needs no lock of the region's for it, nor looks at mapped:  *)
(* TASK-99's fix, openUnderLock here, went with what made it needed.      *)
(* specs/dax-region/mutations removes each, and must find what it stops.  *)
(***************************************************************************)
EXTENDS Naturals, TLC

CONSTANT Rules

AllRules == {"oneWay", "sharedApart"}

ASSUME Rules \subseteq AllRules

VARIABLES
    extent,     \* the extent, as the guest knows it: "open" or "sealed"
    writable,   \* region->writable: what the guest believes of it
    mapped,     \* region->mapped: whether the guest takes it for mapped
    host,       \* the host's mapping of it: "none", "ro" or "rw"
    lock,       \* who holds the region's lock: "reader", "writer" or "none"
    rpc, rWritable,     \* the reader, and what it read of writable
    wpc, wWritable      \* the writer, and what it read of writable

vars == <<extent, writable, mapped, host, lock, rpc, rWritable, wpc, wWritable>>

Init ==
    /\ extent = "open"
    /\ writable = FALSE
    /\ mapped = FALSE
    /\ host = "none"
    /\ lock = "none"
    /\ rpc = "look" /\ rWritable = FALSE
    /\ wpc = "look" /\ wWritable = FALSE

Prot(w) == IF w THEN "rw" ELSE "ro"

(* The writer is in the gate from when it finds the extent open until it has stored. *)
WriterInGate == wpc \in {"open", "lock", "read", "map", "unlock", "store"}

(* A seal: with no store in progress, the sealer forgets the region was writable, then the     *)
(* extent is forgotten, and the host maps it read-only.                                          *)
Seal ==
    /\ extent = "open" /\ ~WriterInGate
    /\ extent' = "sealed"
    /\ writable' = FALSE
    /\ host' = IF host = "rw" THEN "ro" ELSE host
    /\ UNCHANGED <<mapped, lock, rpc, rWritable, wpc, wWritable>>

(* What oneWay rules out: a device write to an extent a seal forgot read as opening it. *)
Reopen ==
    /\ "oneWay" \notin Rules
    /\ extent = "sealed"
    /\ extent' = "open"
    /\ UNCHANGED <<writable, mapped, host, lock, rpc, rWritable, wpc, wWritable>>

(* The reader: maps a sealed extent's region. Shared blocks of an open one, without sharedApart. *)
ReaderLook ==
    /\ rpc = "look"
    /\ extent = "sealed" \/ "sharedApart" \notin Rules
    /\ rpc' = "lock"
    /\ UNCHANGED <<extent, writable, mapped, host, lock, rWritable, wpc, wWritable>>

ReaderLock ==
    /\ rpc = "lock" /\ lock = "none"
    /\ lock' = "reader"
    /\ rpc' = IF mapped THEN "unlock" ELSE "read"
    /\ UNCHANGED <<extent, writable, mapped, host, rWritable, wpc, wWritable>>

ReaderRead ==
    /\ rpc = "read"
    /\ rWritable' = writable
    /\ rpc' = "map"
    /\ UNCHANGED <<extent, writable, mapped, host, lock, wpc, wWritable>>

(* The MAP, sent and answered: as writable as map_to read. *)
ReaderMap ==
    /\ rpc = "map"
    /\ host' = Prot(rWritable)
    /\ rpc' = "mapped"
    /\ UNCHANGED <<extent, writable, mapped, lock, rWritable, wpc, wWritable>>

ReaderMapped ==
    /\ rpc = "mapped"
    /\ mapped' = TRUE
    /\ rpc' = "unlock"
    /\ UNCHANGED <<extent, writable, host, lock, rWritable, wpc, wWritable>>

ReaderUnlock ==
    /\ rpc = "unlock"
    /\ lock' = "none"
    /\ rpc' = "done"
    /\ UNCHANGED <<extent, writable, mapped, host, rWritable, wpc, wWritable>>

(* The writer: finds the extent open, in the gate, or sealed, and moves its blocks elsewhere. *)
WriterLook ==
    /\ wpc = "look"
    /\ wpc' = IF extent = "open" THEN "open" ELSE "done"
    /\ UNCHANGED <<extent, writable, mapped, host, lock, rpc, rWritable, wWritable>>

(* region_open: nothing having mapped it read-only, whatever is in flight. *)
WriterOpen ==
    /\ wpc = "open"
    /\ writable' = TRUE
    /\ wpc' = "lock"
    /\ UNCHANGED <<extent, mapped, host, lock, rpc, rWritable, wWritable>>

(* map_to, as reach calls it: nothing if the region is mapped; else a MAP as writable as read. *)
WriterLock ==
    /\ wpc = "lock" /\ lock = "none"
    /\ lock' = "writer"
    /\ wpc' = IF mapped THEN "unlock" ELSE "read"
    /\ UNCHANGED <<extent, writable, mapped, host, rpc, rWritable, wWritable>>

WriterRead ==
    /\ wpc = "read"
    /\ wWritable' = writable
    /\ wpc' = "map"
    /\ UNCHANGED <<extent, writable, mapped, host, lock, rpc, rWritable>>

WriterMap ==
    /\ wpc = "map"
    /\ host' = Prot(wWritable)
    /\ mapped' = TRUE
    /\ wpc' = "unlock"
    /\ UNCHANGED <<extent, writable, lock, rpc, rWritable, wWritable>>

WriterUnlock ==
    /\ wpc = "unlock"
    /\ lock' = "none"
    /\ wpc' = "store"
    /\ UNCHANGED <<extent, writable, mapped, host, rpc, rWritable, wWritable>>

(* The guest stores to the region, which it took for writable: and is done. *)
WriterStore ==
    /\ wpc = "store"
    /\ wpc' = "stored"
    /\ UNCHANGED <<extent, writable, mapped, host, lock, rpc, rWritable, wWritable>>

Done ==
    /\ rpc = "done" /\ wpc \in {"done", "stored"}
    /\ UNCHANGED vars

Next ==
    \/ Seal \/ Reopen
    \/ ReaderLook \/ ReaderLock \/ ReaderRead \/ ReaderMap \/ ReaderMapped \/ ReaderUnlock
    \/ WriterLook \/ WriterOpen \/ WriterLock \/ WriterRead \/ WriterMap \/ WriterUnlock
    \/ WriterStore
    \/ Done

Spec == Init /\ [][Next]_vars

(* A store the guest makes, in the gate, is to pages the host maps writably: else the host refuses *)
(* it, and the vCPU's KVM_RUN fails with EFAULT until its VM stops (TASK-99).                      *)
NoRefusedStores == wpc = "store" => host = "rw"

(* A reader is outside the gate, which a seal may cross: its MAP is read-only (TASK-44.21). *)
NoWritableMapOutsideTheGate == rpc = "mapped" => ~rWritable

(* TASK-99's race: a reader's MAP in flight, having read the region as it was, as a writer opens it. *)
NoMapInFlightUnderAnOpen == rpc \in {"map", "mapped"} => ~writable
=============================================================================
