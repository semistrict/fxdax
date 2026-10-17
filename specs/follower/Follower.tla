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
(* The follower applies batches of commits mutation by mutation, inside a  *)
(* seqcount and holding its mutex; then invalidates each changed file's   *)
(* mappings under its invalidate_lock, held exclusively; waits for an     *)
(* SRCU grace period; and advances its view, which lets the store release *)
(* what the batch freed. It pins the extents of files deleted while a     *)
(* reader has them open (its graveyard).                                  *)
(*                                                                         *)
(* Readers read keys inside the seqcount, retrying when it moved, and     *)
(* falling back to the mutex after K retries; fault files' data into DAX  *)
(* mappings holding the file's invalidate_lock shared; read(2) inside the *)
(* SRCU read section; open and close files.                               *)
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
    K,          \* the retries of a read before it falls back to the mutex
    Limit,      \* the frees a follower may hold kept before it is cut off
    Rules       \* the protocol's rules in force (all, but in a mutation)

AllRules == {"validate", "fallback", "invalidate", "grace", "graceFirst",
             "keepFrees", "graveyard", "cutOff"}

ASSUME Rules \subseteq AllRules
ASSUME K >= 1 /\ Limit >= 1 /\ MaxCommits >= 1 /\ MaxOps >= 1

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
    fphase,     \* "idle", "applying", "invalidating", "grace", "advance"
    pending,    \* the batch's mutations not yet applied
    target,     \* the commit the batch ends at
    changed,    \* the batch's files not yet invalidated
    seq,        \* the seqcount: odd while a batch applies
    mutex,      \* None, "f" (the follower, applying) or a reader (falling back)
    graceWait,  \* the readers in the SRCU read section when the grace period began
    \* The readers.
    ilock,      \* ilock[f]: readers holding f's invalidate_lock shared (faults)
    srcu,       \* readers inside the SRCU read section
    mappings,   \* DAX mappings: [r |-> reader, f |-> file, e |-> extent]
    handles,    \* handles[r]: the files r has open
    graves,     \* graves[r][f]: the extent the graveyard keeps for r's open, deleted f
    pc,         \* each reader's step
    op,         \* each reader's operation: "pair", "fault", "read", "open"
    opFile,     \* the file it is on
    s,          \* the seqcount as its read began
    retries,    \* the read's retries so far
    vals,       \* the values read so far
    lo,         \* the commit applied as its read began
    lastObs,    \* the last commit a pair read saw: they never go back
    copyExt,    \* the extent a read(2) copies from
    ops,        \* the operations each reader has started
    errors      \* the violations found, by name

writerVars == <<hist, txs, nextExt>>
storeVars == <<kept, released, pinned, cutOff>>
followerVars == <<ftree, applied, view, fphase, pending, target, changed, seq, mutex,
                  graceWait>>
\* The follower's, but its grace period's wait, which readers leaving the SRCU section shrink.
followerVarsButGrace == <<ftree, applied, view, fphase, pending, target, changed, seq, mutex>>
readerVars == <<ilock, srcu, mappings, handles, graves, pc, op, opFile, s, retries, vals,
                lo, lastObs, copyExt, ops>>
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
Release ==
    \E kf \in kept :
        /\ cutOff \/ kf[2] <= view
        /\ kf[1] \notin pinned
        /\ kept' = kept \ {kf}
        /\ released' = released \cup {kf[1]}
        /\ UNCHANGED <<pinned, cutOff, writerVars, followerVars, readerVars, errors>>

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

\* A batch begins: the commits made since the last, under the mutex, the seqcount odd.
StartBatch ==
    /\ ~cutOff
    /\ fphase = "idle"
    /\ applied < Latest
    /\ mutex = None
    /\ target' = Latest
    /\ pending' = Batch(applied + 1, Latest)
    /\ seq' = seq + 1
    /\ mutex' = "f"
    /\ fphase' = "applying"
    /\ UNCHANGED <<ftree, applied, view, changed, graceWait, writerVars, storeVars,
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
          /\ changed' = IF key \in Files THEN changed \cup {key} ELSE changed
          /\ pending' = Tail(pending)
          /\ IF grave
             THEN /\ pinned' = pinned \cup {ftree[key]}
                  /\ graves' = [r \in Readers |->
                                   IF r \in holders THEN [graves[r] EXCEPT ![key] = ftree[key]]
                                   ELSE graves[r]]
             ELSE UNCHANGED <<pinned, graves>>
    /\ UNCHANGED <<applied, view, fphase, target, seq, mutex, graceWait, writerVars, kept,
                   released, cutOff, ilock, srcu, mappings, handles, pc, op, opFile, s,
                   retries, vals, lo, lastObs, copyExt, ops, errors>>

EndBatch ==
    /\ ~cutOff
    /\ fphase = "applying"
    /\ pending = <<>>
    /\ seq' = seq + 1
    /\ mutex' = None
    /\ applied' = target
    /\ fphase' = "invalidating"
    /\ UNCHANGED <<ftree, view, pending, target, changed, graceWait, writerVars, storeVars,
                   readerVars, errors>>

\* A changed file's DAX mappings go, with its invalidate_lock held exclusively: no fault of
\* it is under way.
Invalidate(f) ==
    /\ ~cutOff
    /\ fphase = "invalidating"
    /\ f \in changed
    /\ ilock[f] = {}
    /\ mappings' = IF "invalidate" \in Rules THEN {m \in mappings : m.f # f} ELSE mappings
    /\ changed' = changed \ {f}
    /\ UNCHANGED <<ftree, applied, view, fphase, pending, target, seq, mutex, graceWait,
                   writerVars, storeVars, ilock, srcu, handles, graves, pc, op, opFile, s,
                   retries, vals, lo, lastObs, copyExt, ops, errors>>

\* The grace period begins: it waits for the readers in the SRCU read section now.
Invalidated ==
    /\ ~cutOff
    /\ fphase = "invalidating"
    /\ changed = {}
    /\ IF "grace" \in Rules
       THEN /\ fphase' = "grace"
            /\ graceWait' = srcu
       ELSE /\ fphase' = "advance"
            /\ UNCHANGED graceWait
    /\ UNCHANGED <<ftree, applied, view, pending, target, changed, seq, mutex, writerVars,
                   storeVars, readerVars, errors>>

GraceOver ==
    /\ ~cutOff
    /\ fphase = "grace"
    /\ graceWait = {}
    /\ fphase' = "advance"
    /\ UNCHANGED <<ftree, applied, view, pending, target, changed, seq, mutex, graceWait,
                   writerVars, storeVars, readerVars, errors>>

\* The view moves to the batch's commit: what the batch freed may go.
Advance ==
    /\ ~cutOff
    /\ fphase = "advance"
    /\ view' = applied
    /\ fphase' = "idle"
    /\ UNCHANGED <<ftree, applied, pending, target, changed, seq, mutex, graceWait,
                   writerVars, storeVars, readerVars, errors>>

FollowerNext ==
    StartBatch \/ ApplyOne \/ EndBatch \/ (\E f \in Files : Invalidate(f)) \/ Invalidated
    \/ GraceOver \/ Advance

-----------------------------------------------------------------------------
(* The readers. *)

\* The keys an operation reads.
OpKeys(r) == IF op[r] = "pair" THEN <<"d1", "d2">> ELSE <<opFile[r]>>

\* What extent a reader's open file f has: its data, or, deleted, the graveyard's copy.
FileExtent(r, f, v) == IF v # NoExtent THEN v ELSE graves[r][f]

Idle(r) == pc[r] = "idle"

Start(r, o, f, extra) ==
    /\ ~cutOff
    /\ Idle(r)
    /\ ops[r] < MaxOps
    /\ op' = [op EXCEPT ![r] = o]
    /\ opFile' = [opFile EXCEPT ![r] = f]
    /\ retries' = [retries EXCEPT ![r] = 0]
    /\ ops' = [ops EXCEPT ![r] = ops[r] + 1]
    /\ pc' = [pc EXCEPT ![r] = "begin"]
    /\ extra

StartPair(r) ==
    Start(r, "pair", None, UNCHANGED <<ilock, srcu>>)

\* A fault holds the file's invalidate_lock shared from before its read to its mapping.
StartFault(r, f) ==
    /\ f \in handles[r]
    /\ Start(r, "fault", f, /\ ilock' = [ilock EXCEPT ![f] = ilock[f] \cup {r}]
                            /\ UNCHANGED srcu)

\* A read(2) enters the SRCU read section before it reads which extent to copy from
\* (graceFirst); its bug, after.
StartRead(r, f) ==
    /\ f \in handles[r]
    /\ Start(r, "read", f, /\ srcu' = IF "graceFirst" \in Rules THEN srcu \cup {r} ELSE srcu
                           /\ UNCHANGED ilock)

StartOpen(r, f) ==
    /\ f \notin handles[r]
    /\ Start(r, "open", f, UNCHANGED <<ilock, srcu>>)

StartOp(r) ==
    /\ \/ StartPair(r)
       \/ \E f \in Files : StartFault(r, f) \/ StartRead(r, f) \/ StartOpen(r, f)
    /\ UNCHANGED <<s, vals, lo, lastObs, copyExt, mappings, handles, graves, writerVars,
                   storeVars, followerVars, errors>>

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
    /\ UNCHANGED <<kept, released, cutOff, writerVars, followerVars, ilock, srcu, pc, op,
                   opFile, s, retries, vals, lo, lastObs, copyExt, ops, errors>>

\* A read begins once no batch is applying (read_seqbegin waits for an even count).
Begin(r) ==
    /\ ~cutOff
    /\ pc[r] = "begin"
    /\ seq % 2 = 0
    /\ s' = [s EXCEPT ![r] = seq]
    /\ vals' = [vals EXCEPT ![r] = <<>>]
    /\ lo' = [lo EXCEPT ![r] = applied]
    /\ pc' = [pc EXCEPT ![r] = "read"]
    /\ UNCHANGED <<ilock, srcu, mappings, handles, graves, op, opFile, retries, lastObs,
                   copyExt, ops, writerVars, storeVars, followerVars, errors>>

\* One key read, a step of its own, so that a batch may apply between two.
ReadKey(r) ==
    /\ ~cutOff
    /\ pc[r] = "read"
    /\ Len(vals[r]) < Len(OpKeys(r))
    /\ vals' = [vals EXCEPT ![r] = Append(vals[r], ftree[OpKeys(r)[Len(vals[r]) + 1]])]
    /\ UNCHANGED <<ilock, srcu, mappings, handles, graves, pc, op, opFile, s, retries, lo,
                   lastObs, copyExt, ops, writerVars, storeVars, followerVars, errors>>

\* What a read found, done: each operation goes on with the values.
\* A pair read must see the tree of a commit applied while it ran, never older than the last.
PairSeen(r, v) ==
    LET seen == {c \in lo[r]..applied : <<TreeAt(c)["d1"], TreeAt(c)["d2"]>> = v
                                         /\ c >= lastObs[r]}
    IN IF seen = {}
       THEN /\ errors' = errors \cup {"pair read saw no committed prefix"}
            /\ UNCHANGED lastObs
       ELSE /\ lastObs' = [lastObs EXCEPT ![r] = CHOOSE c \in seen : \A d \in seen : c <= d]
            /\ UNCHANGED errors

Found(r, v) ==
    CASE op[r] = "pair" ->
           /\ PairSeen(r, v)
           /\ pc' = [pc EXCEPT ![r] = "idle"]
           /\ UNCHANGED <<handles, copyExt, srcu>>
      [] op[r] = "fault" ->
           LET e == FileExtent(r, opFile[r], v[1])
           IN /\ errors' = IF e = NoExtent THEN errors \cup {"open file deleted, unreadable"}
                           ELSE errors
              /\ copyExt' = [copyExt EXCEPT ![r] = e]
              /\ pc' = [pc EXCEPT ![r] = "map"]
              /\ UNCHANGED <<lastObs, handles, srcu>>
      \* In the SRCU read section already (graceFirst), or entering it only now: its bug.
      [] op[r] = "read" ->
           LET e == FileExtent(r, opFile[r], v[1])
           IN /\ errors' = IF e = NoExtent THEN errors \cup {"open file deleted, unreadable"}
                           ELSE errors
              /\ copyExt' = [copyExt EXCEPT ![r] = e]
              /\ pc' = [pc EXCEPT ![r] = IF "graceFirst" \in Rules THEN "copy" ELSE "enter"]
              /\ UNCHANGED <<lastObs, handles, srcu>>
      [] op[r] = "open" ->
           /\ handles' = IF v[1] # NoExtent
                         THEN [handles EXCEPT ![r] = handles[r] \cup {opFile[r]}]
                         ELSE handles
           /\ pc' = [pc EXCEPT ![r] = "idle"]
           /\ UNCHANGED <<lastObs, copyExt, srcu, errors>>

\* The read is checked against the seqcount: unchanged, it stands; changed, it is retried,
\* or after K retries, done again holding the mutex.
Validate(r) ==
    /\ ~cutOff
    /\ pc[r] = "read"
    /\ Len(vals[r]) = Len(OpKeys(r))
    /\ IF "validate" \notin Rules \/ seq = s[r]
       THEN /\ Found(r, vals[r])
            /\ UNCHANGED retries
       ELSE /\ retries' = [retries EXCEPT ![r] = retries[r] + 1]
            /\ pc' = [pc EXCEPT ![r] = IF "fallback" \in Rules /\ retries[r] + 1 >= K
                                       THEN "locked" ELSE "begin"]
            /\ UNCHANGED <<handles, copyExt, srcu, lastObs, errors>>
    /\ UNCHANGED <<ilock, mappings, graves, op, opFile, s, vals, lo, ops, writerVars,
                   storeVars, followerVars>>

\* Holding the mutex, no batch applies: the read is whole. (Taking it, reading, and letting
\* it go are one step here, as nothing the follower does can come between them.)
LockedRead(r) ==
    /\ ~cutOff
    /\ pc[r] = "locked"
    /\ mutex = None
    /\ lo' = [lo EXCEPT ![r] = applied]
    /\ LET v == [i \in 1..Len(OpKeys(r)) |-> ftree[OpKeys(r)[i]]]
       IN /\ vals' = [vals EXCEPT ![r] = v]
          /\ Found(r, v)
    /\ UNCHANGED <<ilock, mappings, graves, op, opFile, s, retries, ops, writerVars,
                   storeVars, followerVars>>

\* A fault maps the extent it read, then lets the file's invalidate_lock go.
Map(r) ==
    /\ ~cutOff
    /\ pc[r] = "map"
    /\ mappings' = mappings \cup {[r |-> r, f |-> opFile[r], e |-> copyExt[r]]}
    /\ ilock' = [ilock EXCEPT ![opFile[r]] = ilock[opFile[r]] \ {r}]
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<srcu, handles, graves, op, opFile, s, retries, vals, lo, lastObs, copyExt,
                   ops, writerVars, storeVars, followerVars, errors>>

\* A read(2) that read its extent before entering the SRCU read section enters it now (the bug
\* graceFirst prevents: a grace period that began meanwhile doesn't wait for it).
Enter(r) ==
    /\ ~cutOff
    /\ pc[r] = "enter"
    /\ srcu' = srcu \cup {r}
    /\ pc' = [pc EXCEPT ![r] = "copy"]
    /\ UNCHANGED <<ilock, mappings, handles, graves, op, opFile, s, retries, vals, lo,
                   lastObs, copyExt, ops, writerVars, storeVars, followerVars, errors>>

\* A read(2) copies from the extent it read, then leaves the SRCU read section, and so the
\* wait of a grace period that began while it was in it.
Copy(r) ==
    /\ ~cutOff
    /\ pc[r] = "copy"
    /\ errors' = IF copyExt[r] \in released THEN errors \cup {"read(2) of a released extent"}
                 ELSE errors
    /\ srcu' = srcu \ {r}
    /\ graceWait' = graceWait \ {r}
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<ilock, mappings, handles, graves, op, opFile, s, retries, vals, lo,
                   lastObs, copyExt, ops, writerVars, storeVars, followerVarsButGrace>>

ReaderStep(r) ==
    \/ Begin(r) \/ ReadKey(r) \/ Validate(r) \/ LockedRead(r) \/ Map(r) \/ Enter(r) \/ Copy(r)

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
    /\ seq = 0
    /\ mutex = None
    /\ graceWait = {}
    /\ ilock = [f \in Files |-> {}]
    /\ srcu = {}
    /\ mappings = {}
    /\ handles = [r \in Readers |-> {}]
    /\ graves = [r \in Readers |-> [f \in Files |-> NoExtent]]
    /\ pc = [r \in Readers |-> "idle"]
    /\ op = [r \in Readers |-> None]
    /\ opFile = [r \in Readers |-> None]
    /\ s = [r \in Readers |-> 0]
    /\ retries = [r \in Readers |-> 0]
    /\ vals = [r \in Readers |-> <<>>]
    /\ lo = [r \in Readers |-> 0]
    /\ lastObs = [r \in Readers |-> 0]
    /\ copyExt = [r \in Readers |-> NoExtent]
    /\ ops = [r \in Readers |-> 0]
    /\ errors = {}

ReaderNext(r) == ReaderStep(r) \/ StartOp(r) \/ \E f \in Files : Close(r, f)

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
    /\ fphase \in {"idle", "applying", "invalidating", "grace", "advance"}
    /\ mutex \in {None, "f"} \cup Readers
    /\ srcu \subseteq Readers /\ graceWait \subseteq Readers
    /\ \A r \in Readers : handles[r] \subseteq Files

\* What the actions' checks found, by kind.
\* A read sees whole transactions: a pair read saw a committed prefix, never going back.
NoTornReads == "pair read saw no committed prefix" \notin errors
\* No read(2) copies from an extent the store released.
NoReleasedReads == "read(2) of a released extent" \notin errors
\* A file deleted while a reader has it open reads to its end.
OpenFilesReadable == "open file deleted, unreadable" \notin errors
\* And no other kind.
NoErrors == errors = {}

\* No DAX mapping, while the follower is not cut off, maps an extent the store released.
MappingsLive == ~cutOff => \A m \in mappings : m.e \notin released

\* What the graveyard keeps stays.
GravesLive == ~cutOff => \A r \in Readers, f \in Files : graves[r][f] \notin released

\* Once a batch's invalidation is done, every mapping maps what the follower's tree says the
\* file holds, or, deleted, what the graveyard keeps for the reader.
NoStaleMappings ==
    (~cutOff /\ fphase \in {"idle", "grace", "advance"}) =>
        \A m \in mappings : \/ m.e = ftree[m.f]
                            \/ ftree[m.f] = NoExtent /\ m.e = graves[m.r][m.f]

\* A busy writer can't starve a reader: it retries K times at most, then takes the mutex.
BoundedRetries == \A r \in Readers : retries[r] <= K

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

\* Every read, fault and read(2) begun ends.
ReadersFinish == \A r \in Readers : (pc[r] # "idle") ~> (pc[r] = "idle" \/ cutOff)

\* A follower holding more than Limit frees is cut off, rather than holding them on: even one
\* that hangs (StalledSpec), so that no follower can make the writer's store grow unbounded.
LagCutOff == (Cardinality(kept) > Limit) ~> (cutOff \/ Cardinality(kept) <= Limit)

=============================================================================
