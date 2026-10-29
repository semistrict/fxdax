----------------------------- MODULE DaxEvacuate -----------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* A page of a file of the guest Fxfs's DAX window, as writes move its    *)
(* block to staging extents, seals seal them, evacuations map it back in  *)
(* its range's own space, prunes let regions go, and truncation empties   *)
(* it (guest/linux/fs/fxfs/dax.c; TASK-119.7).                            *)
(*                                                                         *)
(* Fxfs holds the page's block in an extent: the file's own, at first,    *)
(* or the staging extent a write moved it to. The window shows the page  *)
(* at one of two places, as the range's `moved` says: the range's own    *)
(* space, or its slot in the region of the staging extent it moved to.   *)
(* The host maps an extent's block at each place, as the guest's MAPs     *)
(* said, whose replies come in any order: a store through a place goes   *)
(* to the block the host maps there.                                      *)
(*                                                                         *)
(* A write, holding the file's semaphore, has Fxfs move the block to the  *)
(* staging extent open, maps the region's slot to it, waits for the MAP,  *)
(* takes the page as there (`moved`), and stores. A seal, with no store   *)
(* in the gate, seals the staging extent, and the next opens. An          *)
(* evacuation, of a page in a sealed extent's region, gathers it, maps    *)
(* its block in its range's own space, and places it there (`moved` 0).  *)
(* A prune lets a sealed region's mapping go. A truncation empties the    *)
(* page: a hole, read as zeros.                                           *)
(*                                                                         *)
(* Its rules, each of which specs/dax-evacuate/mutations removes, and     *)
(* must find an invariant broken:                                         *)
(*  - evacuationLocked: an evacuation holds the file's semaphore from     *)
(*    gathering its runs to placing them, so that no write moves the page *)
(*    meanwhile, whose newer block the placing would take the place of    *)
(*    with the gathered one (TASK-101, mutant evacuation_unlocked);       *)
(*  - placeAfterReply: it places a run once the MAP of it was answered,   *)
(*    not while the own space still maps what it did;                     *)
(*  - pruneKeepsMoved: a prune keeps the regions a page is in (TASK-128,  *)
(*    mutants prune_in_use, prune_misnumbers).                           *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Rules, Staging, MaxWrites

AllRules == {"evacuationLocked", "placeAfterReply", "pruneKeepsMoved"}

ASSUME Rules \subseteq AllRules
ASSUME Staging \subseteq 1..99 /\ Staging # {}
ASSUME MaxWrites \in Nat

(* Numbers all, as TLC compares no number with a string: the range's own space, and the file's own *)
(* extent's block, are 0; a staging extent, its block and its region's slot are its number.       *)
Own == 0
Base == 0
(* The host maps nothing at a place; Fxfs holds no block of the page, a hole. *)
None == 100
Hole == 101

(* Where the window shows the page: the range's own space, or a staging extent's region's slot. *)
Places == {Own} \cup Staging
(* The blocks the host may map at a place: the file's own extent's, or a staging extent's. *)
Blocks == {Base} \cup Staging
Values == 0..MaxWrites

VARIABLES
    fxfs,       \* where Fxfs holds the page's block: Base, a staging extent, or Hole
    data,       \* each block's bytes: the value of the write stored to it last
    latest,     \* the page's bytes as written last (0, at first, and once a hole)
    moved,      \* the range's moved entry: Own, or the staging extent's region
    host,       \* what the host maps at each place: a block, or None
    sealed,     \* each staging extent's being sealed
    open,       \* the staging extent open, which writes move blocks to, or 0 once none is
    pending,    \* the MAPs sent, whose replies have not come: [place, block]
    sem,        \* who holds the file's semaphore: "none", "writer", "evacuation" or "truncate"
    wpc, wplace, writes,    \* the writer, the place it maps, and how many writes it made
    epc, erun   \* the evacuation, and the staging extent of the run it gathered

vars == <<fxfs, data, latest, moved, host, sealed, open, pending, sem, wpc, wplace, writes,
          epc, erun>>

Init ==
    /\ fxfs = Base
    /\ data = [b \in Blocks |-> 0]
    /\ latest = 0
    /\ moved = Own
    /\ host = [p \in Places |-> IF p = Own THEN Base ELSE None]
    /\ sealed = [e \in Staging |-> FALSE]
    /\ open = CHOOSE e \in Staging : \A f \in Staging : e <= f
    /\ pending = {}
    /\ sem = "none"
    /\ wpc = "idle" /\ wplace = 0 /\ writes = 0
    /\ epc = "idle" /\ erun = 0

(* The place the guest shows the page at, as the range's moved entry says: the one it holds. *)
Shown == moved

(* The page's bytes through `place`: the block's the host maps there, or zeros. *)
At(place) == IF host[place] = None THEN 0 ELSE data[host[place]]

(* A MAP's reply, whichever of those pending comes. *)
Reply ==
    \E m \in pending :
        /\ host' = [host EXCEPT ![m.place] = m.block]
        /\ pending' = pending \ {m}
        /\ UNCHANGED <<fxfs, data, latest, moved, sealed, open, sem, wpc, wplace, writes, epc,
                       erun>>

(* A write: Fxfs moves the block to the staging extent open, and the region's slot is mapped. *)
WriteBegin ==
    /\ wpc = "idle" /\ sem = "none" /\ open # 0 /\ writes < MaxWrites
    /\ sem' = "writer"
    /\ fxfs' = open
    /\ wplace' = open
    /\ pending' = pending \cup {[place |-> open, block |-> open]}
    /\ writes' = writes + 1
    /\ wpc' = "mapping"
    /\ UNCHANGED <<data, latest, moved, host, sealed, open, epc, erun>>

(* Its MAP answered, it takes the page as in the region, and stores, in the gate. *)
WriteMapped ==
    /\ wpc = "mapping"
    /\ \A m \in pending : m.place # wplace
    /\ moved' = wplace
    /\ wpc' = "store"
    /\ UNCHANGED <<fxfs, data, latest, host, sealed, open, pending, sem, wplace, writes, epc,
                   erun>>

WriteStore ==
    /\ wpc = "store"
    /\ data' = IF host[wplace] = None THEN data ELSE [data EXCEPT ![host[wplace]] = writes]
    /\ latest' = writes
    /\ sem' = "none"
    /\ wpc' = "idle"
    /\ UNCHANGED <<fxfs, moved, host, sealed, open, pending, wplace, writes, epc, erun>>

(* A seal, no store in the gate: the staging extent open is sealed, and the next opens. *)
Seal ==
    /\ open # 0 /\ wpc \notin {"store"}
    /\ sealed' = [sealed EXCEPT ![open] = TRUE]
    /\ open' = IF \E e \in Staging : e > open
               THEN CHOOSE e \in Staging : e > open /\ \A f \in Staging : f > open => e <= f
               ELSE 0
    /\ UNCHANGED <<fxfs, data, latest, moved, host, pending, sem, wpc, wplace, writes, epc,
                   erun>>

(* An evacuation gathers the page's run in a sealed region, holding the file's semaphore. *)
EvacGather ==
    /\ epc = "idle" /\ sem = "none"
    /\ moved # Own /\ sealed[moved]
    /\ erun' = moved
    /\ epc' = "gathered"
    /\ sem' = "evacuation"
    /\ UNCHANGED <<fxfs, data, latest, moved, host, sealed, open, pending, wpc, wplace, writes>>

(* It maps the run's block in the range's own space: without evacuationLocked, having let the *)
(* semaphore go after gathering, as evacuation_unlocked did.                                 *)
EvacMap ==
    /\ epc = "gathered"
    /\ pending' = pending \cup {[place |-> Own, block |-> erun]}
    /\ sem' = IF "evacuationLocked" \in Rules THEN sem ELSE "none"
    /\ epc' = "mapped"
    /\ UNCHANGED <<fxfs, data, latest, moved, host, sealed, open, wpc, wplace, writes, erun>>

(* It places the run in the own space, once its MAP is answered with placeAfterReply, taking  *)
(* the semaphore again if it let it go, and lets it go.                                      *)
EvacPlace ==
    /\ epc = "mapped"
    /\ "placeAfterReply" \in Rules => \A m \in pending : m.place # Own
    /\ "evacuationLocked" \in Rules \/ sem = "none"
    /\ moved' = Own
    /\ sem' = "none"
    /\ epc' = "idle"
    /\ UNCHANGED <<fxfs, data, latest, host, sealed, open, pending, wpc, wplace, writes, erun>>

(* A prune lets a sealed region's mapping go: with pruneKeepsMoved, unless the page is in it. *)
Prune ==
    \E r \in Staging :
        /\ sealed[r] /\ host[r] # None /\ sem = "none"
        /\ "pruneKeepsMoved" \in Rules => moved # r
        /\ host' = [host EXCEPT ![r] = None]
        /\ UNCHANGED <<fxfs, data, latest, moved, sealed, open, pending, sem, wpc, wplace,
                       writes, epc, erun>>

(* A truncation empties the page, holding the semaphore: a hole, unmapped where it was shown. *)
Truncate ==
    /\ sem = "none" /\ wpc = "idle" /\ epc = "idle" /\ fxfs # Hole
    /\ fxfs' = Hole
    /\ latest' = 0
    /\ moved' = Own
    /\ host' = [host EXCEPT ![Own] = None, ![Shown] = None]
    /\ UNCHANGED <<data, sealed, open, pending, sem, wpc, wplace, writes, epc, erun>>

(* Nothing under way: the page as it is, for good. *)
Quiet ==
    /\ wpc = "idle" /\ epc = "idle" /\ pending = {}
    /\ UNCHANGED vars

Next ==
    \/ Reply \/ WriteBegin \/ WriteMapped \/ WriteStore \/ Seal
    \/ EvacGather \/ EvacMap \/ EvacPlace \/ Prune \/ Truncate \/ Quiet

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ fxfs \in Blocks \cup {Hole}
    /\ data \in [Blocks -> Values]
    /\ latest \in Values
    /\ moved \in Places
    /\ host \in [Places -> Blocks \cup {None}]
    /\ sem \in {"none", "writer", "evacuation", "truncate"}

(* With the semaphore free, the guest reads at the place it shows the page what was written last. *)
ViewConsistent == sem = "none" => At(Shown) = latest

(* With the semaphore free, Fxfs's block holds what was written last: what a remount reads. *)
Durable == (sem = "none" /\ fxfs # Hole) => data[fxfs] = latest
=============================================================================
