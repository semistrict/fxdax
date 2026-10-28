----------------------------- MODULE ExtentOpen -----------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* One extent of the host's extent store (crates/fxdax/src/extents.rs,    *)
(* ExtentStore), as the guest's writes, MAPs and commits reach it at once  *)
(* (TASK-119.20). The store holds the extent not at all, open (written     *)
(* since the last commit, not mapped), or sealed. Mapped, it is open in   *)
(* memory, a VMO, which the window maps (`open`): opened from what the   *)
(* store holds of it. Data are the writes' names: the bytes each wrote.   *)
(*                                                                         *)
(* Each a step, under the lock of the open extents (ExtentStore::open):   *)
(* - write: to the extent's memory if it is open; else to the store, which *)
(*   refuses a sealed extent (EPERM);                                       *)
(* - MAP (map_open): an extent open in memory is mapped as it is; else one *)
(*   the store holds open is opened from it, read-only or writably; one it *)
(*   doesn't hold is opened empty, writably (may_open), and not read-only  *)
(*   (ENOENT); a sealed one isn't opened (read-only, the cached extent's   *)
(*   pages are mapped; writably, EPERM);                                   *)
(* - commit (commit_and_seal): the store takes each open extent's memory, *)
(*   and seals every extent it holds open, in memory or not.               *)
(* The guest stores through a writable mapping to the extent's memory,    *)
(* while it is open: its seal protocol stores nothing once a seal begins. *)
(*                                                                         *)
(* TASK-121's last loss: write checked the extent wasn't open, let go of   *)
(* the lock, then wrote the store; a writable MAP between opened the       *)
(* extent from the store without the write, and the commit wrote that     *)
(* memory over it. Its rule, writeUnderLock: write's store write under the *)
(* lock, one step with its check, which specs/extent-open/mutations       *)
(* removes, and must find the write lost.                                  *)
(***************************************************************************)
EXTENDS Naturals, TLC

CONSTANTS
    Rules,      \* the rules in force
    Writes,     \* the writes' names: each a writer's write, once
    Commits     \* how many commits the committer makes

AllRules == {"writeUnderLock"}

ASSUME Rules \subseteq AllRules
ASSUME Commits \in Nat

\* The guest's store through its writable mapping.
Stored == "stored"

VARIABLES
    stored,     \* what the store holds of the extent: "none", "open" or "sealed"
    storeData,  \* the writes the store holds of it
    isOpen,     \* whether it is open in memory (the open extents' map has it)
    memory,     \* its memory's writes, while open
    wpc,        \* each write's step: "start", "store" (checked, lock let go), "done" or "failed"
    rwpc,       \* the writable MAP's step: "start", "mapped", "done" or "failed"
    ropc,       \* the read-only MAP's: "start", "mapped" or "failed"
    commits,    \* commits made
    returned    \* the writes that returned, and the guest's store once made

vars == <<stored, storeData, isOpen, memory, wpc, rwpc, ropc, commits, returned>>

Init ==
    /\ stored = "none"
    /\ storeData = {}
    /\ isOpen = FALSE
    /\ memory = {}
    /\ wpc = [w \in Writes |-> "start"]
    /\ rwpc = "start"
    /\ ropc = "start"
    /\ commits = 0
    /\ returned = {}

(* The store's write: it refuses a sealed extent. *)
StoreWrite(w) ==
    IF stored = "sealed"
    THEN /\ wpc' = [wpc EXCEPT ![w] = "failed"]
         /\ UNCHANGED <<stored, storeData, returned>>
    ELSE /\ stored' = "open"
         /\ storeData' = storeData \cup {w}
         /\ wpc' = [wpc EXCEPT ![w] = "done"]
         /\ returned' = returned \cup {w}

(* ExtentStore::write: to the memory of an extent open; else to the store, in the same step *)
(* with writeUnderLock, else in a step of its own, the lock let go.                         *)
Write(w) ==
    /\ wpc[w] = "start"
    /\ IF isOpen
       THEN /\ memory' = memory \cup {w}
            /\ wpc' = [wpc EXCEPT ![w] = "done"]
            /\ returned' = returned \cup {w}
            /\ UNCHANGED <<stored, storeData>>
       ELSE IF "writeUnderLock" \in Rules
            THEN /\ StoreWrite(w)
                 /\ UNCHANGED memory
            ELSE /\ wpc' = [wpc EXCEPT ![w] = "store"]
                 /\ UNCHANGED <<stored, storeData, memory, returned>>
    /\ UNCHANGED <<isOpen, rwpc, ropc, commits>>

WriteStore(w) ==
    /\ wpc[w] = "store"
    /\ StoreWrite(w)
    /\ UNCHANGED <<isOpen, memory, rwpc, ropc, commits>>

(* map_open: the MAP's next step, `pc'`, and the extent opened, or not. *)
Map(writable, pc) ==
    /\ pc = "start"
    /\ CASE isOpen ->
                /\ pc' = "mapped"
                /\ UNCHANGED <<isOpen, memory>>
         [] stored = "open" ->
                /\ isOpen' = TRUE
                /\ memory' = storeData
                /\ pc' = "mapped"
         [] stored = "none" /\ writable ->
                /\ isOpen' = TRUE
                /\ memory' = {}
                /\ pc' = "mapped"
         [] stored = "sealed" /\ ~writable ->
                /\ pc' = "mapped"
                /\ UNCHANGED <<isOpen, memory>>
         [] OTHER ->
                /\ pc' = "failed"
                /\ UNCHANGED <<isOpen, memory>>

MapWritable ==
    /\ Map(TRUE, rwpc)
    /\ UNCHANGED <<stored, storeData, wpc, ropc, commits, returned>>

MapReadOnly ==
    /\ Map(FALSE, ropc)
    /\ UNCHANGED <<stored, storeData, wpc, rwpc, commits, returned>>

(* The guest's store through the writable mapping, while the extent is open. *)
GuestStore ==
    /\ rwpc = "mapped" /\ isOpen
    /\ memory' = memory \cup {Stored}
    /\ returned' = returned \cup {Stored}
    /\ rwpc' = "done"
    /\ UNCHANGED <<stored, storeData, isOpen, wpc, ropc, commits>>

(* Sealed meanwhile, the mapping is read-only: the guest stores nothing. *)
GuestSealed ==
    /\ rwpc = "mapped" /\ ~isOpen
    /\ rwpc' = "done"
    /\ UNCHANGED <<stored, storeData, isOpen, memory, wpc, ropc, commits, returned>>

(* commit_and_seal: the memory of the extent open written, and the extent sealed. *)
Commit ==
    /\ commits < Commits
    /\ commits' = commits + 1
    /\ IF isOpen
       THEN /\ storeData' = memory
            /\ stored' = "sealed"
       ELSE /\ stored' = IF stored = "open" THEN "sealed" ELSE stored
            /\ UNCHANGED storeData
    /\ isOpen' = FALSE
    /\ memory' = {}
    /\ UNCHANGED <<wpc, rwpc, ropc, returned>>

Finished ==
    /\ \A w \in Writes : wpc[w] \in {"done", "failed"}
    /\ rwpc \in {"done", "failed"}
    /\ ropc \in {"mapped", "failed"}
    /\ commits = Commits

Done ==
    /\ Finished
    /\ UNCHANGED vars

Next ==
    \/ \E w \in Writes : Write(w) \/ WriteStore(w)
    \/ MapWritable \/ MapReadOnly
    \/ GuestStore \/ GuestSealed
    \/ Commit
    \/ Done

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ stored \in {"none", "open", "sealed"}
    /\ storeData \subseteq Writes \cup {Stored}
    /\ memory \subseteq Writes \cup {Stored}
    /\ returned \subseteq Writes \cup {Stored}

(* What a read of the extent reads: its memory while open, else what the store holds. *)
Contents == IF isOpen THEN memory ELSE storeData

(* Every write that returned, and the guest's store, is in what the extent reads, and so in what *)
(* the commits after write: none is lost.                                                        *)
NoLostWrite == returned \subseteq Contents
=============================================================================
