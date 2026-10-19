------------------------------ MODULE Follower ------------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* Live read-only followers of a mounted Fxfs (backlog doc-8, TASK-42.8):  *)
(* the protocol between a writer VM, the store extentd serves it from, a   *)
(* follower VM's Fxfs and the readers in that VM, at the level of the      *)
(* protocol: what each step reads and writes, and in what order.           *)
(*                                                                         *)
(* The writer commits transactions: ordered mutations of keys of a tiny    *)
(* filesystem (a token renamed between two directories, so that a half-   *)
(* applied rename shows as the token in neither; files written, a new     *)
(* extent each time and the old one freed; files deleted). Each commit    *)
(* seals what it wrote (doc-4), so the store as of a commit is immutable; *)
(* its frees are kept while a follower's view may still read them.        *)
(*                                                                         *)
(* The follower fetches batches of commits, then, holding its fill barrier *)
(* exclusively (TASK-42.10), drops the DAX mappings of each file they      *)
(* change and applies them, mutation by mutation; lets the barrier go; and *)
(* advances its view, which lets the store release what the batch freed.  *)
(* It pins the extents of files deleted while a reader has them open (its *)
(* graveyard).                                                            *)
(*                                                                         *)
(* Readers' operations hold the barrier shared, from their start to their *)
(* end: pair reads of the token's directories, faults of files' data into *)
(* DAX mappings, read(2)s copying from a file's extent, and opens; and     *)
(* they load through the mappings they have, which takes no barrier. So   *)
(* no operation reads part of a batch, nor copies from an extent a batch  *)
(* freed, and no mapping a batch made stale outlives it: a reader's       *)
(* observations, across its operations and loads, never go back.          *)
(*                                                                         *)
(* Each mechanism is guarded by a member of Rules, so that a model with   *)
(* one rule removed (specs/follower/mutations) must find the violation it *)
(* prevents: a check that never fails proves nothing.                     *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Files,      \* the files, by name (strings)
    Readers,    \* the follower's readers
    MaxCommits, \* how many commits the writer makes, at most
    MaxOps,     \* how many operations each reader starts, at most
    Limit,      \* the frees a follower may hold kept before it is cut off
    Rules       \* the protocol's rules in force (all, but in a mutation)

AllRules == {"barrier", "invalidate", "keepFrees", "graveyard", "cutOff"}

ASSUME Rules \subseteq AllRules
ASSUME Limit >= 1 /\ MaxCommits >= 1 /\ MaxOps >= 1

Dirs == {"d1", "d2"}
Keys == Dirs \cup Files
NoExtent == 0
None == "none"

\* The extents the files start with: 1..Cardinality(Files).
InitExtent == CHOOSE g \in [Files -> 1..Cardinality(Files)] :
                  \A a, b \in Files : a # b => g[a] # g[b]
\* A directory's value is 1 if it holds the token; a file's, its extent, or NoExtent once deleted.
Tree0 == [k \in Keys |-> IF k = "d1" THEN 1
                         ELSE IF k = "d2" THEN 0
                         ELSE InitExtent[k]]
Extents == 1..(Cardinality(Files) + MaxCommits)

VARIABLES
    \* The writer and the store.
    hist,       \* hist[c + 1]: the writer's tree as of commit c (hist[1], the mount's)
    txs,        \* txs[c]: commit c's mutations, <<key, value>>, in order
    nextExt,    \* the next extent the writer mints
    kept,       \* frees kept for the follower's view: <<extent, commit>>
    released,   \* extents whose data is gone
    pinned,     \* extents the follower's graveyard keeps
    cutOff,     \* whether the follower fell too far behind, and was cut off
    \* The follower.
    ftree,      \* its tree
    applied,    \* the last commit it has applied
    view,       \* its view's commit: the store keeps frees after it
    fphase,     \* "idle", "fetched", "waiting", "dropping", "applying", "advance"
    pending,    \* the batch's mutations not yet applied
    target,     \* the commit the batch ends at
    changed,    \* the batch's files whose mappings are not yet dropped
    \* The readers.
    mappings,   \* DAX mappings: [r |-> reader, f |-> file, e |-> extent]
    handles,    \* handles[r]: the files r has open
    graves,     \* graves[r][f]: the extent the graveyard keeps for r's open, deleted f
    pc,         \* each reader's step: "idle" outside an operation, which holds the barrier
    op,         \* each reader's operation: "pair", "fault", "read", "open"
    opFile,     \* the file it is on
    vals,       \* the values read so far
    lo,         \* the commit applied as its operation began
    lastObs,    \* the earliest commit a reader's observations may all be of: they never go back
    copyExt,    \* the extent a fault maps, or a read(2) copies from
    ops,        \* the operations each reader has started
    errors      \* the violations found, by name

writerVars == <<hist, txs, nextExt>>
storeVars == <<kept, released, pinned, cutOff>>
followerVars == <<ftree, applied, view, fphase, pending, target, changed>>
readerVars == <<mappings, handles, graves, pc, op, opFile, vals, lo, lastObs, copyExt, ops>>
vars == <<writerVars, storeVars, followerVars, readerVars, errors>>

Latest == Len(hist) - 1
Tree == hist[Len(hist)]
TreeAt(c) == hist[c + 1]

RECURSIVE Apply(_, _)
Apply(t, ms) == IF ms = <<>> THEN t
                ELSE Apply([t EXCEPT ![Head(ms)[1]] = Head(ms)[2]], Tail(ms))

RECURSIVE Batch(_, _)
\* The mutations of commits from..to, in order.
Batch(from, to) == IF from > to THEN <<>> ELSE txs[from] \o Batch(from + 1, to)

\* The files the mutations ms change.
FilesOf(ms) == {ms[i][1] : i \in 1..Len(ms)} \cap Files

-----------------------------------------------------------------------------
(* The writer. It never looks at the follower: nothing here waits for it. *)

\* Commits the mutations ms, freeing the extents freed.
Commit(ms, freed, minted) ==
    /\ hist' = Append(hist, Apply(Tree, ms))
    /\ txs' = Append(txs, ms)
    /\ nextExt' = nextExt + minted
    /\ IF "keepFrees" \in Rules /\ ~cutOff
       THEN /\ kept' = kept \cup {<<e, Latest + 1>> : e \in freed}
            /\ UNCHANGED released
       ELSE /\ released' = released \cup freed
            /\ UNCHANGED kept
    /\ UNCHANGED <<pinned, cutOff, followerVars, readerVars, errors>>

\* The token moves to the other directory: cleared where it was, then set, so a follower
\* applying half of it has it in neither.
Rename ==
    LET from == IF Tree["d1"] = 1 THEN "d1" ELSE "d2"
        to == IF from = "d1" THEN "d2" ELSE "d1"
    IN Commit(<<<<from, 0>>, <<to, 1>>>>, {}, 0)

\* f's data is rewritten: Fxfs writes it in a new extent, and frees the old one.
Write(f) ==
    /\ Tree[f] # NoExtent
    /\ Commit(<<<<f, nextExt>>>>, {Tree[f]}, 1)

Delete(f) ==
    /\ Tree[f] # NoExtent
    /\ Commit(<<<<f, NoExtent>>>>, {Tree[f]}, 0)

WriterNext ==
    /\ Latest < MaxCommits
    /\ \/ Rename
       \/ \E f \in Files : Write(f) \/ Delete(f)

-----------------------------------------------------------------------------
(* The store (extentd). *)

\* A kept free goes once the follower's view is past it, unless the graveyard pins it.
ReleaseOf(kf) ==
    /\ kf \in kept
    /\ cutOff \/ kf[2] <= view
    /\ kf[1] \notin pinned
    /\ kept' = kept \ {kf}
    /\ released' = released \cup {kf[1]}
    /\ UNCHANGED <<pinned, cutOff, writerVars, followerVars, readerVars, errors>>

Release == \E kf \in kept : ReleaseOf(kf)

\* A follower holding too much kept is cut off: its view goes, and what it kept; its reads
\* fail from here on (extentd refuses them), so nothing it does matters any more.
CutOff ==
    /\ "cutOff" \in Rules
    /\ ~cutOff
    /\ Cardinality(kept) > Limit
    /\ cutOff' = TRUE
    /\ pinned' = {}
    /\ UNCHANGED <<kept, released, writerVars, followerVars, readerVars, errors>>

StoreNext == Release \/ CutOff

-----------------------------------------------------------------------------
(* The follower. *)

\* A batch is fetched: the commits made since the last, read before the follower takes its
\* barrier, as the implementation reads them (crates/follower's Follow::fetch); the writer may
\* commit more meanwhile, which the next batch takes.
Fetch ==
    /\ ~cutOff
    /\ fphase = "idle"
    /\ applied < Latest
    /\ target' = Latest
    /\ pending' = Batch(applied + 1, Latest)
    /\ changed' = FilesOf(Batch(applied + 1, Latest))
    /\ fphase' = "fetched"
    /\ UNCHANGED <<ftree, applied, view, writerVars, storeVars, readerVars, errors>>

\* A batch waiting for the fill barrier, or holding it until it has applied: no operation takes it
\* meanwhile (an rwsem's writer waiting goes before readers coming).
BatchHoldsBarrier == fphase \in {"waiting", "dropping", "applying"}

\* The batch fetched waits for the barrier.
Wait ==
    /\ ~cutOff
    /\ fphase = "fetched"
    /\ fphase' = "waiting"
    /\ UNCHANGED <<ftree, applied, view, pending, target, changed, writerVars, storeVars,
                   readerVars, errors>>

\* The batch takes the barrier, once no reader's operation holds it.
StartBatch ==
    /\ ~cutOff
    /\ fphase = "waiting"
    /\ \A r \in Readers : pc[r] = "idle"
    /\ fphase' = "dropping"
    /\ UNCHANGED <<ftree, applied, view, pending, target, changed, writerVars, storeVars,
                   readerVars, errors>>

\* A file the batch changes has its DAX mappings dropped, before the batch applies.
Invalidate(f) ==
    /\ ~cutOff
    /\ fphase = "dropping"
    /\ f \in changed
    /\ mappings' = IF "invalidate" \in Rules THEN {m \in mappings : m.f # f} ELSE mappings
    /\ changed' = changed \ {f}
    /\ UNCHANGED <<ftree, applied, view, fphase, pending, target, writerVars, storeVars,
                   handles, graves, pc, op, opFile, vals, lo, lastObs, copyExt, ops, errors>>

\* All dropped: the batch applies.
Dropped ==
    /\ ~cutOff
    /\ fphase = "dropping"
    /\ changed = {}
    /\ fphase' = "applying"
    /\ UNCHANGED <<ftree, applied, view, pending, target, changed, writerVars, storeVars,
                   readerVars, errors>>

\* One mutation applied. Before a file's deletion, the graveyard keeps its extent for each
\* reader with it open.
ApplyOne ==
    /\ ~cutOff
    /\ fphase = "applying"
    /\ pending # <<>>
    /\ LET key == Head(pending)[1]
           v == Head(pending)[2]
           holders == {r \in Readers : key \in handles[r]}
           grave == "graveyard" \in Rules /\ key \in Files /\ v = NoExtent /\ holders # {}
       IN /\ ftree' = [ftree EXCEPT ![key] = v]
          /\ pending' = Tail(pending)
          /\ IF grave
             THEN /\ pinned' = pinned \cup {ftree[key]}
                  /\ graves' = [r \in Readers |->
                                   IF r \in holders THEN [graves[r] EXCEPT ![key] = ftree[key]]
                                   ELSE graves[r]]
             ELSE UNCHANGED <<pinned, graves>>
    /\ UNCHANGED <<applied, view, fphase, target, changed, writerVars, kept, released, cutOff,
                   mappings, handles, pc, op, opFile, vals, lo, lastObs, copyExt, ops, errors>>

\* Applied: the batch lets the barrier go.
EndBatch ==
    /\ ~cutOff
    /\ fphase = "applying"
    /\ pending = <<>>
    /\ applied' = target
    /\ fphase' = "advance"
    /\ UNCHANGED <<ftree, view, pending, target, changed, writerVars, storeVars, readerVars,
                   errors>>

\* The view moves to the batch's commit: what the batch freed may go.
Advance ==
    /\ ~cutOff
    /\ fphase = "advance"
    /\ view' = applied
    /\ fphase' = "idle"
    /\ UNCHANGED <<ftree, applied, pending, target, changed, writerVars, storeVars, readerVars,
                   errors>>

FollowerNext ==
    Fetch \/ Wait \/ StartBatch \/ (\E f \in Files : Invalidate(f)) \/ Dropped \/ ApplyOne \/ EndBatch
    \/ Advance

-----------------------------------------------------------------------------
(* The readers. *)

\* The keys an operation reads.
OpKeys(r) == IF op[r] = "pair" THEN <<"d1", "d2">> ELSE <<opFile[r]>>

\* What extent a reader's open file f has: its data, or, deleted, the graveyard's copy.
FileExtent(r, f, v) == IF v # NoExtent THEN v ELSE graves[r][f]

Idle(r) == pc[r] = "idle"

\* An operation begins holding the fill barrier shared: not while a batch holds it, or waits for
\* it; without the barrier, whenever.
Start(r, o, f) ==
    /\ ~cutOff
    /\ Idle(r)
    /\ "barrier" \in Rules => ~BatchHoldsBarrier
    /\ ops[r] < MaxOps
    /\ op' = [op EXCEPT ![r] = o]
    /\ opFile' = [opFile EXCEPT ![r] = f]
    /\ ops' = [ops EXCEPT ![r] = ops[r] + 1]
    /\ vals' = [vals EXCEPT ![r] = <<>>]
    /\ lo' = [lo EXCEPT ![r] = applied]
    /\ pc' = [pc EXCEPT ![r] = "read"]

StartPair(r) == Start(r, "pair", None)

StartFault(r, f) == f \in handles[r] /\ Start(r, "fault", f)

StartRead(r, f) == f \in handles[r] /\ Start(r, "read", f)

StartOpen(r, f) == f \notin handles[r] /\ Start(r, "open", f)

StartOp(r) ==
    /\ \/ StartPair(r)
       \/ \E f \in Files : StartFault(r, f) \/ StartRead(r, f) \/ StartOpen(r, f)
    /\ UNCHANGED <<lastObs, copyExt, mappings, handles, graves, writerVars, storeVars,
                   followerVars, errors>>

\* Closing a file unmaps it, and lets the graveyard's copy go.
Close(r, f) ==
    /\ ~cutOff
    /\ Idle(r)
    /\ f \in handles[r]
    /\ handles' = [handles EXCEPT ![r] = handles[r] \ {f}]
    /\ mappings' = {m \in mappings : ~(m.r = r /\ m.f = f)}
    /\ graves' = [graves EXCEPT ![r][f] = NoExtent]
    /\ pinned' = IF graves[r][f] # NoExtent
                    /\ ~\E r2 \in Readers \ {r} : graves[r2][f] = graves[r][f]
                 THEN pinned \ {graves[r][f]} ELSE pinned
    /\ UNCHANGED <<kept, released, cutOff, writerVars, followerVars, pc, op, opFile, vals, lo,
                   lastObs, copyExt, ops, errors>>

\* One key read, a step of its own, so that a batch without the barrier may apply between two.
ReadKey(r) ==
    /\ ~cutOff
    /\ pc[r] = "read"
    /\ Len(vals[r]) < Len(OpKeys(r))
    /\ vals' = [vals EXCEPT ![r] = Append(vals[r], ftree[OpKeys(r)[Len(vals[r]) + 1]])]
    /\ UNCHANGED <<mappings, handles, graves, pc, op, opFile, lo, lastObs, copyExt, ops,
                   writerVars, storeVars, followerVars, errors>>

\* A reader observes that file f holds extent e, on the follower as of a commit from `from` on:
\* some commit from then to the one applied, no earlier than its observations so far may all be
\* of, has it so, or the reader went back.
Observe(r, f, e, from) ==
    LET seen == {c \in from..applied : TreeAt(c)[f] = e /\ c >= lastObs[r]}
    IN IF seen = {}
       THEN /\ errors' = errors \cup {"reader went back"}
            /\ UNCHANGED lastObs
       ELSE /\ lastObs' = [lastObs EXCEPT ![r] = CHOOSE c \in seen : \A d \in seen : c <= d]
            /\ UNCHANGED errors

\* What a fault or read(2) read of an open file: its extent, observed, or, deleted, the
\* graveyard's copy, which its reader reads as it was.
ObserveFile(r, v) ==
    IF v[1] # NoExtent THEN Observe(r, opFile[r], v[1], lo[r])
    ELSE /\ errors' = IF graves[r][opFile[r]] = NoExtent
                      THEN errors \cup {"open file deleted, unreadable"} ELSE errors
         /\ UNCHANGED lastObs

\* A pair read must see the tree of a commit applied while it ran, never older than the last.
PairSeen(r, v) ==
    LET seen == {c \in lo[r]..applied : <<TreeAt(c)["d1"], TreeAt(c)["d2"]>> = v
                                         /\ c >= lastObs[r]}
    IN IF seen = {}
       THEN /\ errors' = errors \cup {"pair read saw no committed prefix"}
            /\ UNCHANGED lastObs
       ELSE /\ lastObs' = [lastObs EXCEPT ![r] = CHOOSE c \in seen : \A d \in seen : c <= d]
            /\ UNCHANGED errors

\* What an operation read, done: a pair read and an open end; a fault maps what it read next,
\* a read(2) copies from it.
Found(r) ==
    /\ ~cutOff
    /\ pc[r] = "read"
    /\ Len(vals[r]) = Len(OpKeys(r))
    /\ LET v == vals[r]
       IN CASE op[r] = "pair" ->
                 /\ PairSeen(r, v)
                 /\ pc' = [pc EXCEPT ![r] = "idle"]
                 /\ UNCHANGED <<handles, copyExt>>
            [] op[r] = "fault" ->
                 /\ ObserveFile(r, v)
                 /\ copyExt' = [copyExt EXCEPT ![r] = FileExtent(r, opFile[r], v[1])]
                 /\ pc' = [pc EXCEPT ![r] = "map"]
                 /\ UNCHANGED handles
            [] op[r] = "read" ->
                 /\ ObserveFile(r, v)
                 /\ copyExt' = [copyExt EXCEPT ![r] = FileExtent(r, opFile[r], v[1])]
                 /\ pc' = [pc EXCEPT ![r] = "copy"]
                 /\ UNCHANGED handles
            [] op[r] = "open" ->
                 /\ handles' = IF v[1] # NoExtent
                               THEN [handles EXCEPT ![r] = handles[r] \cup {opFile[r]}]
                               ELSE handles
                 /\ pc' = [pc EXCEPT ![r] = "idle"]
                 /\ UNCHANGED <<lastObs, copyExt, errors>>
    /\ UNCHANGED <<mappings, graves, op, opFile, vals, lo, ops, writerVars, storeVars,
                   followerVars>>

\* A fault maps the extent it read, and its operation ends.
Map(r) ==
    /\ ~cutOff
    /\ pc[r] = "map"
    /\ mappings' = mappings \cup {[r |-> r, f |-> opFile[r], e |-> copyExt[r]]}
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<handles, graves, op, opFile, vals, lo, lastObs, copyExt, ops, writerVars,
                   storeVars, followerVars, errors>>

\* A read(2) copies from the extent it read, and its operation ends.
Copy(r) ==
    /\ ~cutOff
    /\ pc[r] = "copy"
    /\ errors' = IF copyExt[r] \in released THEN errors \cup {"read(2) of a released extent"}
                 ELSE errors
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<mappings, handles, graves, op, opFile, vals, lo, lastObs, copyExt, ops,
                   writerVars, storeVars, followerVars>>

\* A load through a DAX mapping the reader has: no fault, so no barrier: what it maps, observed.
\* Of a deleted file's graveyard copy, as it was.
Load(r, m) ==
    /\ ~cutOff
    /\ Idle(r)
    /\ ops[r] < MaxOps
    /\ m \in mappings
    /\ m.r = r
    /\ ops' = [ops EXCEPT ![r] = ops[r] + 1]
    /\ IF m.e = graves[r][m.f] THEN UNCHANGED <<errors, lastObs>> ELSE Observe(r, m.f, m.e, 0)
    /\ UNCHANGED <<mappings, handles, graves, pc, op, opFile, vals, lo, copyExt, writerVars,
                   storeVars, followerVars>>

ReaderStep(r) == ReadKey(r) \/ Found(r) \/ Map(r) \/ Copy(r)

-----------------------------------------------------------------------------

Init ==
    /\ hist = <<Tree0>>
    /\ txs = <<>>
    /\ nextExt = Cardinality(Files) + 1
    /\ kept = {}
    /\ released = {}
    /\ pinned = {}
    /\ cutOff = FALSE
    /\ ftree = Tree0
    /\ applied = 0
    /\ view = 0
    /\ fphase = "idle"
    /\ pending = <<>>
    /\ target = 0
    /\ changed = {}
    /\ mappings = {}
    /\ handles = [r \in Readers |-> {}]
    /\ graves = [r \in Readers |-> [f \in Files |-> NoExtent]]
    /\ pc = [r \in Readers |-> "idle"]
    /\ op = [r \in Readers |-> None]
    /\ opFile = [r \in Readers |-> None]
    /\ vals = [r \in Readers |-> <<>>]
    /\ lo = [r \in Readers |-> 0]
    /\ lastObs = [r \in Readers |-> 0]
    /\ copyExt = [r \in Readers |-> NoExtent]
    /\ ops = [r \in Readers |-> 0]
    /\ errors = {}

ReaderNext(r) ==
    ReaderStep(r) \/ StartOp(r) \/ (\E f \in Files : Close(r, f)) \/ (\E m \in mappings : Load(r, m))

Next ==
    \/ WriterNext
    \/ StoreNext
    \/ FollowerNext
    \/ \E r \in Readers : ReaderNext(r)

Spec == Init /\ [][Next]_vars

\* The follower, the store and readers once started go on; the writer, and readers starting
\* operations, needn't.
Fairness ==
    /\ WF_vars(FollowerNext)
    /\ WF_vars(StoreNext)
    /\ \A r \in Readers : WF_vars(ReaderStep(r))

LiveSpec == Spec /\ Fairness

\* A follower that hangs (its VM paused, its kernel stuck): only the store goes on.
StalledSpec == Spec /\ WF_vars(StoreNext)

\* Readers are interchangeable (for safety only: TLC's symmetry is unsound for liveness).
ReaderSymmetry == Permutations(Readers)

-----------------------------------------------------------------------------
(* What must hold. *)

TypeOK ==
    /\ hist \in Seq([Keys -> {0, 1} \cup Extents])
    /\ nextExt \in Extents \cup {Cardinality(Files) + MaxCommits + 1}
    /\ kept \subseteq Extents \X (1..MaxCommits)
    /\ released \subseteq Extents
    /\ pinned \subseteq Extents
    /\ cutOff \in BOOLEAN
    /\ applied \in 0..MaxCommits /\ view \in 0..applied
    /\ fphase \in {"idle", "fetched", "waiting", "dropping", "applying", "advance"}
    /\ changed \subseteq Files
    /\ pc \in [Readers -> {"idle", "read", "map", "copy"}]
    /\ \A r \in Readers : handles[r] \subseteq Files

\* What the actions' checks found, by kind.
\* A read sees whole transactions: a pair read saw a committed prefix, never going back.
NoTornReads == "pair read saw no committed prefix" \notin errors
\* No read(2) copies from an extent the store released.
NoReleasedReads == "read(2) of a released extent" \notin errors
\* A file deleted while a reader has it open reads to its end.
OpenFilesReadable == "open file deleted, unreadable" \notin errors
\* A reader's observations, across its operations and loads, never go back (TASK-42.10).
NoGoingBack == "reader went back" \notin errors
\* And no other kind.
NoErrors == errors = {}

\* No DAX mapping, while the follower is not cut off, maps an extent the store released.
MappingsLive == ~cutOff => \A m \in mappings : m.e \notin released

\* What the graveyard keeps stays.
GravesLive == ~cutOff => \A r \in Readers, f \in Files : graves[r][f] \notin released

\* Outside a batch holding the barrier, every mapping maps what the follower's tree says the file
\* holds, or, deleted, what the graveyard keeps for the reader.
NoStaleMappings ==
    (~cutOff /\ fphase \in {"idle", "fetched", "waiting", "advance"}) =>
        \A m \in mappings : \/ m.e = ftree[m.f]
                            \/ ftree[m.f] = NoExtent /\ m.e = graves[m.r][m.f]

\* Every pin is the graveyard's, for a reader with the deleted file open.
NoLeakedPins == \A e \in pinned : \E r \in Readers, f \in Files : graves[r][f] = e

\* The writer never waits for the follower: whatever the follower does, it may commit.
WriterNeverWaits == [](Latest < MaxCommits => ENABLED WriterNext)

\* The follower catches up with the writer once the writer stops, unless it is cut off.
FollowerCatchesUp == <>[](cutOff \/ (view = Latest /\ applied = Latest))

\* What the writer freed goes once the follower's view is past it, but for what the graveyard
\* pins (which goes as its file is closed: NoLeakedPins).
FreesGo ==
    \A e \in Extents, c \in 1..MaxCommits :
        (<<e, c>> \in kept /\ c <= view /\ e \notin pinned) ~> (<<e, c>> \notin kept)

\* Every operation begun ends.
ReadersFinish == \A r \in Readers : (pc[r] # "idle") ~> (pc[r] = "idle" \/ cutOff)

\* A follower holding more than Limit frees is cut off, rather than holding them on: even one
\* that hangs (StalledSpec), so that no follower can make the writer's store grow unbounded.
LagCutOff == (Cardinality(kept) > Limit) ~> (cutOff \/ Cardinality(kept) <= Limit)

=============================================================================
