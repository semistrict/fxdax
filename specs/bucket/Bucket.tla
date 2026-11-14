------------------------------ MODULE Bucket ------------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be  *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* Branches in object storage (backlog doc-7, crates/extent_remote) with   *)
(* the host's write-ahead log of doc-14 (TASK-165.2): at the level of the  *)
(* protocol, what each step reads and writes in the bucket, and in what    *)
(* order. The simulation (extent_remote's sim) samples interleavings, and  *)
(* found TASK-79.18 and 79.3 only after the fact; TLC exhausts small ones. *)
(*                                                                         *)
(* The bucket holds, per branch: segments, dense and create-only, each a   *)
(* footer naming the segment it follows (`after`), its writer's epoch, one *)
(* commit (or none: a fence), an extent sealed into it or stored on its    *)
(* own, an extent freed, the branch's next sequence number, a fork, and    *)
(* the run of a commit it goes on with, which a later segment ends;        *)
(* extents stored on their own, at keys naming their writer's epoch and    *)
(* the segment that names them; checkpoints, a state as of a segment;      *)
(* records; leases, with their placement's fencing token; and the          *)
(* collector's moves. A segment's create is in flight from its upload      *)
(* until it lands, in any order, though its writer has gone.               *)
(*                                                                         *)
(* Hosts run writers. Opening a branch takes its lease and starts its      *)
(* recovery (the newest checkpoint, or the origin's state); recovery reads *)
(* the segments after to the first missing, then the checkpoints again;    *)
(* the fence, a step a create, applies what landed at the end first and    *)
(* fills gaps; then the writer reads its lease and mints from its epoch's  *)
(* range, and puts its WAL after the branch's end (doc-14, Opening a       *)
(* store: wal::rebase), by commits: the WAL's commits to the branch's      *)
(* last, if it is one of them, are acknowledged, and the rest restamped    *)
(* after the fence, if the branch's last commit is one of the WAL's or the *)
(* commit the WAL started from; otherwise the WAL diverged, and is         *)
(* dropped, as `fxdaxctl take` drops it. A commit is a segment of the WAL, *)
(* stamped by the chain; or, spanning segments (TASK-92.41), segments      *)
(* going on with it, a run of it in each, then the one it ends in. Readers *)
(* hold such runs until that one, and drop them at a segment neither going *)
(* on with the commit nor ending it, a fence; writers acknowledge, and     *)
(* WALs land, only between commits. An upload creates its extent of its    *)
(* own, then puts it in flight; an acknowledgement takes the WAL's prefix  *)
(* created, after a read of the lease if it holds a fork; then the         *)
(* children of its forks get their records. The collector keeps the        *)
(* fences of live branches. A host crashes keeping its WALs, to their      *)
(* last durable commits with "cut" (local recovery keeping commits whole), *)
(* or losing them. Placements give hosts fencing tokens. Branches are      *)
(* deleted by their placements (the lease ended, then the record, the      *)
(* children recorded), failed forks by their hosts (delete_fork), points   *)
(* forked by restores (the child's record, the point read, the record      *)
(* made); other branches take forks' children's IDs. Every branch is a     *)
(* volume of a cell (CellOf), whose prefix holds its objects. The          *)
(* collector runs passes per cell (TASK-395), each under its cell's        *)
(* collector lease, at once with other cells' and with writers: the cell's *)
(* listing and records, its live branches recovered, the states at their   *)
(* fork points of its branches' forks to other cells kept, its records and *)
(* references listed again, then compaction and deletion within the cell,  *)
(* and the references of its deleted children retired once nothing in it   *)
(* lives. A fork to another cell writes a reference under its parent's     *)
(* cell, naming the parent, the fork point and the child, which keeps the  *)
(* parent's whole state at the fork point there while it stands, whatever  *)
(* the child frees of it since: the collector is conservative.             *)
(*                                                                         *)
(* Each rule is a member of Rules; specs/bucket/mutations removes one      *)
(* each, and must find the violation it prevents (each mutation's model    *)
(* says which, and how). The first 33 are the fields of extent_remote's    *)
(* `Rules` (crates/extent_remote/src/lib.rs); then one the code holds with *)
(* no field (TASK-178); then two doc-14's, its last two fields too         *)
(* (TASK-165.5, crates/extent_remote/src/wal.rs); then five those of       *)
(* commits spanning segments, which the code holds with no field           *)
(* (TASK-92.41, TASK-149.45); then six the per-cell collector's           *)
(* (TASK-395.1), fields of Rules since TASK-395.3 and 395.4; then two its   *)
(* references' retirement's (TASK-395.10), fields too; the last, that a    *)
(* retirement is marked (TASK-395.13, TASK-395.14), a field too:           *)
(*                                                                         *)
(* - pinsFences, pinsOldFences, pinsVoidStrays, pinsLateStrays,            *)
(*   pinsTakings (NoPinsFences, NoPinsOldFences, NoPinsVoidStrays,         *)
(*   NoPinsLateStrays, NoPinsTakings): the cases of the collector's one    *)
(*   function of what it keeps of a live branch whatever covers it, Pinned *)
(*   (collector::pinned, TASK-388): every position a writer of the branch, *)
(*   taken over or taking it over, may still create at. A writer           *)
(*   acknowledges what it created with no read of its lease, its creates   *)
(*   its fencing (TASK-371), so CreatesNeverFreed states the safety they   *)
(*   keep once: the first position a writer may still create at was never  *)
(*   freed. Its fences (TASK-371), of every epoch; its strays, void or     *)
(*   late (TASK-383); and, while a writer takes it over, its lease of an   *)
(*   epoch past its state's, everything (TASK-371). Without any, a         *)
(*   writer's create lands where the collector freed, below a later        *)
(*   writer's fence, and is acknowledged: a commit no history holds.       *)
(* - forksAfterLease (NoForksAfterLease): a writer acknowledges a segment  *)
(*   holding a fork only once a read of its lease after the creates shows  *)
(*   it still its: a deleted branch's fences go, and a writer of it taken  *)
(*   over would record the child of a fork landed where its deletion's     *)
(*   fence was, of segments the collector let go of.                       *)
(* - fence (NoFence): a writer taking over creates an empty segment at the *)
(*   end it found.                                                         *)
(* - continuity (NoContinuity): readers skip a segment that does not       *)
(*   follow the last applied.                                              *)
(* - epochSequences (NoEpochSequences): a writer mints from its epoch's    *)
(*   range, having read its lease after recovering and fencing.            *)
(* - recordsAfterAck (NoRecordsAfterAck): a fork's child gets its record   *)
(*   once the fork's segment is acknowledged.                              *)
(* - candidatesFirst (NoCandidatesFirst): the collector lists before it    *)
(*   reads records and states.                                             *)
(* - newestCheckpoint (NoNewestCheckpoint, NoNewestCheckpointInherited): a *)
(*   recovery that finds a segment missing lists the checkpoints again,    *)
(*   and starts again from a newer one; one that listed none and finds     *)
(*   what it inherited lost, if one has come since (TASK-177). A writer's  *)
(*   recovery, pinsTakings covers too (TASK-371): its mutation removes     *)
(*   both.                                                                 *)
(* - forksFromStates (NoForksFromStates): the collector finds forks in     *)
(*   their parents' states, not only by their records.                     *)
(* - orphansAfterSuccessor (NoOrphansAfterSuccessor): an earlier writer's  *)
(*   extent goes once a later one's segment is in the branch, not its      *)
(*   lease.                                                                *)
(* - tokens (NoTokens): a lease is taken only with a token as high.        *)
(* - deleteTokens (NoDeleteTokens): a branch is deleted only with a token  *)
(*   as high as its lease's.                                               *)
(* - footers (NoFooters): readers read an extent where the footer of the   *)
(*   object at its key says, not where their state says (TASK-79.18).      *)
(* - forkClaimsIds (NoForkClaimsIds): a failed fork's deletion takes its   *)
(*   child's ID with a record before it ends the child's lease. The code's *)
(*   rule is of forks made nowhere in the bucket (create_forked), which    *)
(*   doc-14 removes; here every fork has an origin, and the rule is what   *)
(*   it says: without it, the deletion of a child with no record deletes   *)
(*   it directly (TASK-79.82).                                             *)
(* - forkClaimsLanded (NoForkClaimsLanded): that record lives only if the  *)
(*   fork's parent holds the fork's segment, and says it is a claim.       *)
(* - pointsTakeNoWriter (NoPointsTakeNoWriter): no writer takes a point.   *)
(* - forksCheckPoints (NoForksCheckPoints): a fork of a point reads the    *)
(*   point's record after it creates its child's.                          *)
(* - forksPending (NoForksPending): that record says the fork is being     *)
(*   made until then.                                                      *)
(* - collectorAbandons (NoCollectorAbandons): the collector gives up on    *)
(*   forks being made of points deleted.                                   *)
(* - recordsRelisted (NoRecordsRelisted): the collector lists the records  *)
(*   again after reading states with checkpoints of their own. Since       *)
(*   TASK-395.1, a clone's reference, listed after the states, finds it    *)
(*   too: its mutation removes references with it.                         *)
(* - recordsOwned (NoRecordsOwned): records name their makings' owners.    *)
(* - extentKeysNameSegments (NoExtentKeysNameSegments): the collector      *)
(*   keeps an extent of its own while the segment its key names may land,  *)
(*   not while the branch's next sequence number is below it (TASK-79.3).  *)
(* - waysFromSeals (NoWaysFromSeals): the collector keeps every moves on   *)
(*   the way to an extent from where it was sealed, unless the state       *)
(*   naming it is from a checkpoint of the lease's epoch, read after the   *)
(*   checkpoints were listed (TASK-175).                                   *)
(* - movesTakenFirst (NoMovesTakenFirst): a writer names what its          *)
(*   checkpoint moves where it went before it creates the checkpoint, only *)
(*   forward along the way (TASK-175).                                     *)
(* - leasesByTakings (NoLeasesByTakings): a writer finds the lease still   *)
(*   its by its epoch and its holder, the taking's, not by its epoch       *)
(*   alone (TASK-179).                                                     *)
(* - passesFencedStop (NoPassesFencedStop): a pass that finds what a live  *)
(*   branch needs lost stops, without failing, if a later pass took the    *)
(*   collector's lease (TASK-180).                                         *)
(* - recoveriesByOrigins (NoRecoveriesByOrigins): the collector recovers a *)
(*   branch by each origin it is named by, its record's and its parent's   *)
(*   state's, not once: a child recovered by the state, that fork refused, *)
(*   may have a record of a restore's origin, found on relisting           *)
(*   (TASK-173). Since TASK-395.1, the restore's reference keeps its       *)
(*   origin's state at the fork point too: its mutation removes references *)
(*   with it.                                                              *)
(* - recordsKept (NoRecordsKept): the collector drops no record, a deleted *)
(*   root's neither: a parent's writer slow to record a fork whose child's *)
(*   ID a root took, and was deleted, would make it again (TASK-174).      *)
(* - deletionsFence (NoDeletionsFence): a deletion's recovery creates an   *)
(*   empty segment of the final epoch at the end it found, as a writer     *)
(*   taking over does, so that every deletion of the branch finds the same *)
(*   forks: without it, a second deletion, recovering past a fork's        *)
(*   segment landed after the first marked the branch deleted, records the *)
(*   child of a fork never acknowledged, whose origin a pass, reading the  *)
(*   branch deleted and no record of the child, deletes (TASK-183).        *)
(* - deletionsReread (NoDeletionsReread): a deletion reads the record      *)
(*   again once its recovery is fenced, before recording the children of   *)
(*   the forks it found: deleted since, it is done, as the deletion that   *)
(*   marked it recorded those before its own fence. A pass deletes that    *)
(*   fence, and the extents a writer taken over stored on their own ahead  *)
(*   of its segments: one of them, of a fork, landing where the fence was, *)
(*   the later deletion would record the fork's child, which names an      *)
(*   extent the pass deleted (TASK-380).                                   *)
(* - deletionsDoneByOthers (NoDeletionsDoneByOthers): a deletion whose     *)
(*   recovery of the branch finds what it needs lost, the record deleted   *)
(*   since, is done (TASK-178; branch::delete, with no field of Rules).    *)
(* - walFollows (NoWalFollows): a WAL is put after the branch only if the  *)
(*   branch's last commit is one of the WAL's or the commit the WAL        *)
(*   started from.                                                         *)
(* - walAfterFence (NoWalAfterFence): that last commit is the branch's     *)
(*   after the fence, which may have applied the WAL's own segments,       *)
(*   landed since recovery.                                                *)
(* - spansHeld (NoSpansHeld): readers hold the runs of a commit a segment  *)
(*   goes on with (Contents::continued) until a segment ends it, then seal *)
(*   them with it (State::apply_contents, State::begun).                   *)
(* - spansFenced (NoSpansFenced): they drop them at a segment applied that *)
(*   neither goes on with the commit nor ends it, a later writer's fence:  *)
(*   the commit, never ended, never was.                                   *)
(* - acksBetweenCommits (NoAcksBetweenCommits): a writer acknowledges      *)
(*   segments only to the last going on with no commit                     *)
(*   (Writer::acknowledge), so that its WAL keeps a commit whole until the *)
(*   bucket has it whole.                                                  *)
(* - walBetweenCommits (NoWalBetweenCommits): a WAL is put after the       *)
(*   branch only between commits: the segments taken out as landed end     *)
(*   every commit they go on with (Wal::rebase's `between`).               *)
(* - walContinues (NoWalContinues): a WAL whose segment ending the         *)
(*   branch's last commit and going on with the next landed alone, those   *)
(*   after it not, is put after the branch too: that segment restamped     *)
(*   after the fence with its continuation alone, then the rest            *)
(*   (Wal::rebase, TASK-345).                                              *)
(* - wholeCommitsKept (NoWholeCommitsKept): a host's local recovery keeps  *)
(*   a commit spanning segments only if consecutive segments end it, and   *)
(*   closes the first again without it otherwise (extent_store's           *)
(*   wal::recover): a store whose log ends inside a commit refuses to      *)
(*   open.                                                                 *)
(* - references (NoReferences): a fork of a branch to a child in another   *)
(* cell, a writable fork or a point's clone, has a reference under the     *)
(* parent's cell, which that cell's passes keep the parent's state at the  *)
(* fork point for: its pass lists nothing of the child's cell, and the     *)
(* parent deleted, would delete what the child inherits.                   *)
(*                                                                         *)
(* - referencesFirst (NoReferencesFirst): a writable fork's reference is   *)
(* written before its child's record, by whoever writes the record: its    *)
(* writer, the next, or the parent's deletion, before that marks the       *)
(* parent deleted. Without it, a pass reading the parent deleted finds no  *)
(* reference.                                                              *)
(*                                                                         *)
(* - clonesReferFirst (NoClonesReferFirst): a point's clone writes its     *)
(* reference before its record and before it reads the point's record: one *)
(* that read the point live then wrote it before the point's deletion,     *)
(* which a pass reading the point deleted, or finding its reference        *)
(* retired, read after.                                                    *)
(*                                                                         *)
(* - referencesHeld (NoReferencesHeld): a reference is retired, by the     *)
(* pass of its child's cell, only once that cell has no live branch and no *)
(* reference of its own: the child's forks inherit what it did, kept by    *)
(* its reference.                                                          *)
(*                                                                         *)
(* - referencesRelisted (NoReferencesRelisted): a pass lists its cell's    *)
(* references after the states, as it lists the records again: listed with *)
(* the candidates, it misses a clone's of a point a checkpoint dropped     *)
(* since.                                                                  *)
(*                                                                         *)
(* - forkPointsKept (NoForkPointsKept): a pass keeps, of the parent's      *)
(* state at a referenced fork point, what recovering it reads, not only    *)
(* the objects holding its extents: a child with no checkpoint of its own  *)
(* recovers through it.                                                    *)
(*                                                                         *)
(* - clonesRecordFirst (NoClonesRecordFirst): a point's clone creates its  *)
(* record, being made, before its reference (and reads the point's after   *)
(* both, clonesReferFirst): a reference always has a record naming its     *)
(* origin, by which its child's cell's pass retires it, whatever comes of  *)
(* the clone. Written first, a host dying between leaves a reference no    *)
(* record names, which no pass retires (ReferencesNamed).                  *)
(*                                                                         *)
(* - referencesTakenBack (NoReferencesTakenBack): whoever finds the record *)
(* of a writable fork's child naming another origin, or none, deletes the  *)
(* fork's reference: as it writes the record and finds another's there,   *)
(* or as a later writer or the parent's deletion records the fork again.   *)
(* Without it, a reference whose child's ID another branch took between    *)
(* its writer's read of the child's record and its create stays for ever  *)
(* (ReferencesNamed).                                                      *)
(*                                                                         *)
(* - retirementsMarked (NoRetirementsMarked): a reference retired, or      *)
(* taken back, is marked, a mark kept for good under its parent's cell     *)
(* beside it, before it is deleted; a pass deletes the references of its   *)
(* cell it lists marked. A writer of the reference whose read of the       *)
(* child's record came before the retirement, none there, writes it after: *)
(* one a stale recorder wrote after another's take-back, dying before its  *)
(* create, which no record names (TASK-395.13); one written again after    *)
(* its child's cell's pass retired it, which that cell's passes, skipping  *)
(* it, the cell dead and unchanged, never retire again (TASK-395.14). Its  *)
(* write changes its parent's cell's listing, whose next pass deletes it,  *)
(* by its mark (ReferencesRetiredMarked). A writer finding the child's     *)
(* record live and its own writes none: its maker's came first             *)
(* (referencesFirst), and stands.                                          *)
(*                                                                         *)
(* Where this differs from doc-14 and the code, by what TLC found:         *)
(*                                                                         *)
(* - doc-14 put the WAL after the branch by segments: WAL segments whose   *)
(*   footers recovery read were acknowledged, and the first other one had  *)
(*   to follow the fence's `after`. A WAL whose landed segments a later    *)
(*   writer's checkpoint covers is then declared diverged (recovery reads  *)
(*   no footer a checkpoint covers), and so is one the branch moved past   *)
(*   by a fence alone (a takeover that crashed): the store refuses to      *)
(*   open, its guest's commits dropped by `take`, though the branch holds  *)
(*   them or nothing came after them. By commits, as doc-7's recovery on   *)
(*   the same host was, neither diverges.                                  *)
(* - What TASK-177 found lost, a fork's deleted parent's record, which no *)
(*   record is now (recordsKept), is here that parent's segments to the    *)
(*   fork, which the collector lets go of for the same reason: the fork's  *)
(*   checkpoint.                                                           *)
(*                                                                         *)
(* Steps are what the code does between two requests, or merged: a         *)
(* writer's taking reads the record, takes the lease and reads the record  *)
(* again in one step, the first read of it from before the branch's        *)
(* deletion if "staleTake" is in Can; the writers' and the collector's     *)
(* recoveries read the bucket at once, the deletions' in two steps, the    *)
(* checkpoints listed, then the rest, its fence and the record read again; *)
(* their forks' children are recorded in a step after. (The code's writer, *)
(* once it holds the lease, reads the record, recovers and lists the       *)
(* segments at once, and takes the lease from a read of it made as it      *)
(* first read the record, conditionally: TASK-165.11.) A checkpoint's      *)
(* create lands after its writer took its moves, in a step of its own,     *)
(* while its writer's session lasts. Passes overlap if "overlap" is in     *)
(* Can, of any cells, a later one of a cell taking its collector lease;    *)
(* MaxPasses bounds them all. A pass reads its cell's records once each,   *)
(* as listings find them, until one finds none new, then once again each   *)
(* after the states (recordsRelisted), when it lists its references too.   *)
(* Reads of other cells' objects (a parent's state, a point's record) are  *)
(* a pass's GETs; it lists only its own cell, and deletes only there, but  *)
(* for the references it retires, whose keys its deleted children's        *)
(* records give. Listings are atomic, as before.                           *)
(*                                                                         *)
(* Can bounds what happens: each kind of step needs its name in it, so     *)
(* that a model checks some at a time and a mutation only what it needs.   *)
(* Moves go two hops at most: a segment's, then those moves'. A commit     *)
(* spans three segments at most, two going on with it, each a run of one   *)
(* extent, the one ending it holding nothing else (the code's may hold its *)
(* frees and more). A segment ends a commit and goes on with the next only *)
(* with "spanOn" in Can (BucketSpans has it): without walContinues,        *)
(* Wal::rebase finds a WAL diverged whose segment doing so landed alone,   *)
(* though no other writer moved the branch on (TASK-345). Not              *)
(* modelled: the collector's grace period (correctness does not depend on  *)
(* it, doc-7) and its compaction threshold (it compacts any object it may, *)
(* by choice), and creates failing.                                        *)
(***************************************************************************)
EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS
    Hosts,          \* the hosts
    Branches,       \* branch IDs
    Root,           \* the branch the bucket starts with
    Segs,           \* segments per branch: 0..Segs-1
    MaxTakes,       \* lease takings, all branches'
    MaxCommits,     \* commits
    MaxMints,       \* extent IDs minted
    MaxPasses,      \* passes of the collector
    MaxPlaces,      \* placements
    MaxCrashes,     \* host crashes
    Can,            \* the kinds of step the model takes
    None,           \* a model value: nothing
    Rules,          \* the protocol's rules in force (all, but in a mutation)
    CellOf          \* CellOf[b]: the cell b is a volume of, whose prefix holds its objects

AllRules == {"pinsFences", "pinsOldFences", "pinsVoidStrays", "pinsLateStrays", "pinsTakings",
             "forksAfterLease", "fence", "continuity", "epochSequences", "recordsAfterAck",
             "candidatesFirst", "newestCheckpoint", "forksFromStates", "orphansAfterSuccessor",
             "tokens", "deleteTokens", "footers", "forkClaimsIds", "forkClaimsLanded",
             "pointsTakeNoWriter", "forksCheckPoints", "forksPending", "collectorAbandons",
             "recordsRelisted", "recordsOwned", "extentKeysNameSegments", "waysFromSeals",
             "movesTakenFirst", "leasesByTakings", "passesFencedStop", "recoveriesByOrigins",
             "recordsKept", "deletionsFence", "deletionsReread", "deletionsDoneByOthers",
             "walFollows",
             "walAfterFence", "spansHeld", "spansFenced", "acksBetweenCommits",
             "walBetweenCommits", "wholeCommitsKept", "walContinues",
             "references", "referencesFirst", "clonesReferFirst", "referencesHeld",
             "referencesRelisted", "forkPointsKept", "clonesRecordFirst", "referencesTakenBack",
             "retirementsMarked"}

AllCan == {"commit", "mint", "seg", "own", "free", "fork", "point", "restore", "unfork",
           "contest", "delete", "deletePoints", "place", "checkpoint", "collect", "compact",
           "compactMoves", "crash", "disk", "staleTake", "overlap", "span", "spanOn", "cut",
           "forkOwn", "deleteRoot", "landInOrder", "forkNext"}

ASSUME Rules \subseteq AllRules
ASSUME Can \subseteq AllCan
ASSUME Root \in Branches
ASSUME CellOf \in [Branches -> Nat \ {0}]

\* The maps of branches to cells a model's CellOf may be: one cell, which is the whole-bucket
\* collector before TASK-395; a cell each, as hosts make them (a store, a fork, a point and a
\* restore each the one volume of a new cell); and the root sharing its cell with branch 2, the
\* others a cell each, for forks within a cell and across cells in one model.
OneCell == [b \in Branches |-> 1]
CellEach == [b \in Branches |-> b]
Pair == [b \in Branches |-> IF b <= 2 THEN 1 ELSE b]

Cells == {CellOf[b] : b \in Branches}
CellBranches(c) == {b \in Branches : CellOf[b] = c}
\* Whether a fork of p to c crosses cells: c's cell's collector reads nothing of p's.
Cross(p, c) == CellOf[p] # CellOf[c]

Seqs == 0..(Segs - 1)
\* The lease epoch of a deletion's, which no writer takes.
Final == MaxTakes + 1

R(rule) == rule \in Rules

VARIABLES
    \* The bucket.
    seg,        \* seg[b][s]: segment s of b, or None
    own,        \* the extents stored on their own: [b, e, s, x], at the key of writer e's segment s
    ckpt,       \* ckpt[b][s]: b's checkpoint as of segment s, or None
    rec,        \* rec[b]: b's record, or None
    lease,      \* lease[b]: [epoch, token, holder], or None: the holder its taking's, unique
    moves,      \* moves[b][s][d]: the extents the collector moved out of segment s of b, at d = 1,
                \* or out of those moves, at d = 2; or None
    inflight,   \* the segments' creates in flight: [h, b, g, s, o]
    refs,       \* the references of forks across cells, [p, at, c]: under p's cell, naming p, the
                \* fork point at, and the child c
    marked,     \* the marks of the references retired or taken back, [p, at, c], under p's cell,
                \* kept (retirementsMarked): a reference marked is garbage, written again or not
    retired,    \* what happened: the references retired or taken back (kept with the bucket's
                \* variables, as it changes with them)
    \* The hosts.
    ses,        \* ses[h][b]: h's writer of b, or None
    wal,        \* wal[h][b]: h's WAL of b, a sequence of [s, o]: kept by a crash, or lost
    tok,        \* tok[h][b]: the fencing token of h's placement of b
    proc,       \* proc[h]: h's deletion, failed fork's deletion or restore, or None
    \* The collector.
    gc,         \* gc[i]: pass i, of the cell gc[i].cell, whose collector lease epoch is i: a cell's
                \* collector lease is its last pass's
    \* What happened.
    commits,    \* commits[t]: [branch, parent, seals, free]: its branch, the commit before it, what
                \* it sealed and freed
    nextTag,
    acked,      \* acked[b]: the commits a writer of b acknowledged
    minted,     \* the extent IDs minted
    twice,      \* whether an ID was minted twice
    makers,     \* makers[b]: the makings of b told they made it
    doomed,     \* the makings a deletion of was asked by whoever may
    taken,      \* taken[b]: the highest token that took b's lease, -1 if none did
    placed,     \* placed[b]: the last placement's token
    errors,     \* what failed that must not
    freed,      \* the segments' places the collector freed, <<b, s>>: CreatesNeverFreed's history
    \* Bounds.
    gen, takes, passes, places, crashes, mints

bucketVars == <<seg, own, ckpt, rec, lease, moves, inflight, refs, marked, retired>>
hostVars == <<ses, wal, tok, proc>>
truthVars == <<commits, nextTag, acked, minted, twice, makers, doomed, taken, placed, errors>>
\* (freed is no action's: Next keeps it.)
countVars == <<gen, takes, passes, places, crashes, mints>>
vars == <<bucketVars, hostVars, gc, truthVars, countVars, freed>>

-----------------------------------------------------------------------------
(* Values. *)

Max(S) == CHOOSE x \in S : \A y \in S : x >= y
Min(S) == CHOOSE x \in S : \A y \in S : x <= y
\* Pairs <<epoch, n>>, ordered as sequence numbers are: the epoch's range, then n.
GE2(a, b) == a[1] > b[1] \/ (a[1] = b[1] /\ a[2] >= b[2])
Max2(a, b) == IF GE2(a, b) THEN a ELSE b

Put(f, x, v) == [y \in DOMAIN f \cup {x} |-> IF y = x THEN v ELSE f[y]]
Drop(f, x) == [y \in DOMAIN f \ {x} |-> f[y]]
Nothing == [x \in {} |-> None]

\* What a record writes for the owner `o` of its making: nothing, without recordsOwned.
Owner(o) == IF R("recordsOwned") THEN o ELSE <<"anyone">>

\* A record: its kind ("root", "fork", "point", "forking", "claim"), its origin <<parent, at>>,
\* the point a fork being made is of, its owner and whether deleted; and, not in the bucket's
\* bytes, the commit its branch starts from and the making it is of.
Record(kind, origin, point, o, base) ==
    [kind |-> kind, origin |-> origin, point |-> point, owner |-> Owner(o), deleted |-> FALSE,
     base |-> base, maker |-> o]

\* Whether r is the record of the child of the fork whose owner is o.
ChildOf(r, o) == r.kind # "root" /\ r.owner = Owner(o)

\* A state: its extents' locations, its last commit, the last segment applied <<s, epoch>>, its
\* next sequence number, its forks, the highest epoch applied, the last segment read, the runs
\* of the commit the segments applied last went on with (State::begun: [commit, runs], runs the
\* locations of its extents; or None), its fences, the segments applied that began an epoch
\* (State::fences), its strays (State::strays), the segments read that are neither, void, or
\* late, following the last applied of an earlier epoch, and, not in the bucket's bytes, the
\* commits applied and whether one was applied twice.
Fresh == [ext |-> Nothing, head |-> 0, last |-> <<>>, next |-> <<0, 0>>, forks |-> {},
          epoch |-> 0, through |-> -1, begun |-> None, fences |-> {}, voids |-> {}, lates |-> {},
          hist |-> {}, dup |-> FALSE]

\* A fork's child's state before its segments: what it inherits (State::inheriting: no runs
\* begun).
Inherit(st) == [Fresh EXCEPT !.ext = st.ext, !.head = st.head, !.hist = st.hist]

\* Whether a reader applies segment o after st: it follows the last applied (continuity).
Follows(st, o) == o.after = st.last \/ ~R("continuity")

\* Whether segment o, following one that went on with commit t, ends it (Contents::ends): it
\* names it. The code's names a commit of t's writer, t or a later one, the last of those its
\* segment ends; here a segment ends one commit at most, and a commit's segments are its writer's,
\* one after another, so that the one after t's last continuation names t.
Ends(o, t) == o.commit = t
\* Whether it goes on with t, ending no commit (Contents::continues).
Continues(o, t) == o.commit = 0 /\ o.cont # None /\ o.cont.commit = t

\* The locations f, those of g sealed over them.
Over(f, g) == [x \in DOMAIN f \cup DOMAIN g |-> IF x \in DOMAIN g THEN g[x] ELSE f[x]]

\* st, segment s of b, whose object is o, applied: unless it is void. First the runs of the commit
\* the segments before went on with: sealed, if o ends it; held on, if o goes on with it; else
\* dropped (spansFenced), its writer's, never acknowledged, a later writer fenced. Then o's own;
\* then, if o goes on with a commit, its run held until a segment ends it (spansHeld)
\* (State::apply_contents).
Apply(st, b, s, o) ==
    IF ~Follows(st, o) THEN [st EXCEPT !.through = s, !.voids = @ \cup {s}]
    ELSE LET bg == st.begun
             ended == bg # None /\ Ends(o, bg.commit)
             held == IF bg = None \/ ended THEN None
                     ELSE IF Continues(o, bg.commit) \/ ~R("spansFenced") THEN bg
                     ELSE None
             e0 == IF ended THEN Over(st.ext, bg.runs) ELSE st.ext
             e1 == IF o.ext # None THEN Put(e0, o.ext, <<"seg", b, s>>) ELSE e0
             e2 == IF o.own # None THEN Put(e1, o.own, <<"own", b, o.epoch, s>>) ELSE e1
             e3 == IF o.free # None /\ o.free \in DOMAIN e2 THEN Drop(e2, o.free) ELSE e2
             run == IF o.cont = None THEN Nothing ELSE Put(Nothing, o.cont.ext, <<"seg", b, s>>)
             \* Without spansHeld, a continuation's run sealed at once.
             e4 == IF R("spansHeld") THEN e3 ELSE Over(e3, run)
             begun == IF o.cont = None \/ ~R("spansHeld") THEN held
                      ELSE IF held = None THEN [commit |-> o.cont.commit, runs |-> run]
                      ELSE [held EXCEPT !.runs = Over(@, run)]
         IN [st EXCEPT !.ext = e4,
                       !.begun = begun,
                       !.head = IF o.commit # 0 THEN o.commit ELSE st.head,
                       !.hist = IF o.commit # 0 THEN st.hist \cup {o.commit} ELSE st.hist,
                       !.dup = st.dup \/ o.commit \in st.hist,
                       !.last = <<s, o.epoch>>,
                       !.next = Max2(st.next, o.next),
                       !.forks = IF o.fork = None THEN st.forks
                                 ELSE st.forks \cup {[child |-> o.fork.child, at |-> s,
                                                      owner |-> o.fork.owner,
                                                      point |-> o.fork.point]},
                       !.epoch = IF o.epoch > st.epoch THEN o.epoch ELSE st.epoch,
                       !.fences = IF o.epoch > st.epoch THEN st.fences \cup {s} ELSE st.fences,
                       !.lates = IF o.epoch < st.epoch THEN st.lates \cup {s} ELSE st.lates,
                       !.through = s]

RECURSIVE ApplyAll(_, _, _)
\* st, the WAL ws applied.
ApplyAll(st, b, ws) ==
    IF ws = <<>> THEN st ELSE ApplyAll(Apply(st, b, ws[1].s, ws[1].o), b, Tail(ws))

CommitsOf(ws) == {ws[i].o.commit : i \in DOMAIN ws} \ {0}

-----------------------------------------------------------------------------
(* Recovery, as one read of the bucket: what invariants and the collector see. *)

CkSeqs(b) == {s \in Seqs : ckpt[b][s] # None}

RECURSIVE FollowTo(_, _, _, _, _)
\* st, b's segments from s to at applied, every one there, and what that read.
FollowTo(b, st, s, at, t) ==
    IF s > at THEN [ok |-> TRUE, st |-> st, touched |-> t]
    ELSE IF seg[b][s] = None THEN [ok |-> FALSE, st |-> st, touched |-> t]
    ELSE FollowTo(b, Apply(st, b, s, seg[b][s]), s + 1, at, t \cup {<<"seg", b, s>>})

RECURSIVE StateAt(_, _)
\* b's state as of its segment at: its newest checkpoint to there, or its origin's, then its
\* segments to there; or a failure, something gone.
StateAt(b, at) ==
    LET cs == {s \in CkSeqs(b) : s <= at} IN
    IF cs # {} THEN FollowTo(b, ckpt[b][Max(cs)], Max(cs) + 1, at, {<<"ckpt", b, Max(cs)>>})
    ELSE IF rec[b] = None THEN [ok |-> FALSE, st |-> Fresh, touched |-> {}]
    ELSE IF rec[b].origin = <<>> THEN FollowTo(b, Fresh, 0, at, {<<"rec", b>>})
    ELSE LET p == StateAt(rec[b].origin[1], rec[b].origin[2]) IN
         IF ~p.ok THEN p
         ELSE FollowTo(b, Inherit(p.st), 0, at, p.touched \cup {<<"rec", b>>})

RECURSIVE FollowEnd(_, _, _, _)
\* st, b's segments from s applied to the first missing, and what that read.
FollowEnd(b, st, s, t) ==
    IF s >= Segs \/ seg[b][s] = None
    THEN [ok |-> TRUE, st |-> st, end |-> s, touched |-> t]
    ELSE FollowEnd(b, Apply(st, b, s, seg[b][s]), s + 1, t \cup {<<"seg", b, s>>})

Lost == [ok |-> FALSE, st |-> Fresh, end |-> 0, touched |-> {}, ck |-> -1]

\* b's state, forked from origin: its newest checkpoint, or its origin's state, then its segments.
Recover(b, origin) ==
    LET cs == CkSeqs(b)
        with(r, ck) == [ok |-> r.ok, st |-> r.st, end |-> r.end, touched |-> r.touched, ck |-> ck]
    IN IF cs # {} THEN with(FollowEnd(b, ckpt[b][Max(cs)], Max(cs) + 1,
                                      {<<"ckpt", b, Max(cs)>>}), Max(cs))
       ELSE IF origin = <<>> THEN with(FollowEnd(b, Fresh, 0, {}), -1)
       ELSE LET p == StateAt(origin[1], origin[2]) IN
            IF ~p.ok THEN Lost ELSE with(FollowEnd(b, Inherit(p.st), 0, p.touched), -1)

\* b's newest checkpoint, -1 if it has none.
Newest(b) == IF CkSeqs(b) = {} THEN -1 ELSE Max(CkSeqs(b))

\* A recovery of b, forked from origin, that listed its checkpoints, the newest ck (-1 if none),
\* going on: from it, or from its origin's state, then its segments. It starts again if the
\* checkpoint has gone, or, listing the checkpoints again (newestCheckpoint), a newer one has come,
\* which the collector, going by it, may have let what it read go for: segments, or what it
\* inherited (TASK-177).
Recovering(b, origin, ck) ==
    LET newer == R("newestCheckpoint") /\ Newest(b) # ck
        again == [restart |-> TRUE, ok |-> FALSE, st |-> Fresh, end |-> 0]
        from(st, s) == LET r == FollowEnd(b, st, s, {}) IN
                       IF newer THEN again
                       ELSE [restart |-> FALSE, ok |-> TRUE, st |-> r.st, end |-> r.end]
    IN IF ck >= 0 THEN IF ckpt[b][ck] = None THEN again ELSE from(ckpt[b][ck], ck + 1)
       ELSE IF origin = <<>> THEN from(Fresh, 0)
       ELSE LET p == StateAt(origin[1], origin[2]) IN
            IF p.ok THEN from(Inherit(p.st), 0)
            ELSE IF newer THEN again
            ELSE [restart |-> FALSE, ok |-> FALSE, st |-> Fresh, end |-> 0]

\* Whether b holds its segment at, as a fork's recovery from there needs.
Holds(b, at) == StateAt(b, at).ok

\* Whether no writer has b: no lease, or its deletion's.
Unwritten(b) == lease[b] = None \/ lease[b].epoch = Final

LeaseEpoch(b) == IF lease[b] = None THEN 0 ELSE lease[b].epoch

\* Whether b's lease is the taking of the writer whose session is s: its epoch, and its holder.
Taking(s, b) == lease[b] # None /\ lease[b].epoch = s.epoch /\ lease[b].holder = s.gen

\* Whether that writer finds b's lease still its, reading it: by its taking (leasesByTakings), or
\* by its epoch alone, which a lease removed and taken again repeats (TASK-179).
Leased(s, b) == IF R("leasesByTakings") THEN Taking(s, b) ELSE LeaseEpoch(b) = s.epoch

\* How many times the collector moves what was in a segment: into its moves, then those moves'.
Hops == 2

\* The key of segment s of b (d = 0), or of the moves d hops from it, and its depth.
Key(b, s, d) == IF d = 0 THEN <<"seg", b, s>> ELSE <<"moves", b, s, d>>
Depth(loc) == IF loc[1] = "seg" THEN 0 ELSE loc[4]

\* Whether segment o's footer names a run of x in its data: of a commit it ends, or of the one it
\* goes on with (Contents::runs).
Runs(o, x) == o.ext = x \/ (o.cont # None /\ o.cont.ext = x)

\* Whether the object d hops from segment s of b names x: the segment's footer, or the moves' map.
Names(b, s, d, x) ==
    IF d = 0 THEN seg[b][s] # None /\ Runs(seg[b][s], x)
    ELSE moves[b][s][d] # None /\ x \in moves[b][s][d]

\* Whether a reader of extent x at loc reads x's bytes: where the footer of the object at its key
\* says it is, or, that object gone or naming it not, where the map of the moves of it says, one
\* hop at a time (fetch's read); without footers, a segment where loc says, whatever it holds.
Readable(x, loc) ==
    IF loc[1] = "own" THEN [b |-> loc[2], e |-> loc[3], s |-> loc[4], x |-> x] \in own
    ELSE LET b == loc[2]
             s == loc[3]
             d == Depth(loc)
         IN IF d = 0 /\ seg[b][s] # None /\ ~R("footers") THEN Runs(seg[b][s], x)
            ELSE Names(b, s, d, x) \/ (d < Hops /\ Names(b, s, d + 1, x))

RECURSIVE Followed(_, _, _, _)
\* How many hops from segment s of b x is, following the moves' maps on from d.
Followed(x, b, s, d) == IF d < Hops /\ Names(b, s, d + 1, x) THEN Followed(x, b, s, d + 1) ELSE d

\* Where a checkpoint records x, at loc, to be: where the collector moved it.
Moved(x, loc) == IF loc[1] = "own" THEN loc
                 ELSE LET d == Followed(x, loc[2], loc[3], Depth(loc)) IN Key(loc[2], loc[3], d)

\* Whether loc2 is further along an extent's way than loc (layout::further): moves of it, or of
\* those moves.
Further(loc, loc2) == /\ loc[1] # "own" /\ loc2[1] = "moves"
                      /\ loc2[2] = loc[2] /\ loc2[3] = loc[3] /\ Depth(loc2) > Depth(loc)

\* The state st, its extents named where the state ck names them if that is further along their
\* ways (Writer::take_moves).
TakeMoves(st, ck) ==
    [st EXCEPT !.ext = [x \in DOMAIN st.ext |->
                          IF x \in DOMAIN ck.ext /\ Further(st.ext[x], ck.ext[x]) THEN ck.ext[x]
                          ELSE st.ext[x]]]

-----------------------------------------------------------------------------
(* The truth. *)

RECURSIVE Anc(_)
\* The commits up to t.
Anc(t) == IF t = 0 THEN {} ELSE {t} \cup Anc(commits[t].parent)

RECURSIVE OwnStart(_, _)
\* Where the path of b's own commits to t starts: the commit before the first.
OwnStart(b, t) == IF t # 0 /\ commits[t].branch = b THEN OwnStart(b, commits[t].parent) ELSE t

RECURSIVE Path(_)
\* The commits to t, oldest first.
Path(t) == IF t = 0 THEN <<>> ELSE Append(Path(commits[t].parent), t)

RECURSIVE MadeBy(_, _)
\* The extents the commits of the path p leave.
MadeBy(p, held) ==
    IF p = <<>> THEN held
    ELSE LET c == commits[p[1]]
         IN MadeBy(Tail(p), IF c.free = None THEN held \cup c.seals
                            ELSE (held \cup c.seals) \ {c.free})

Made == UNION {makers[b] : b \in Branches}

\* A writer's guest's state: what it acknowledged, then its WAL.
View(h, b) == ApplyAll(ses[h][b].st, b, wal[h][b])

-----------------------------------------------------------------------------
(* Records. *)

\* The record of the child of parent's fork f, the records being rs, given by whoever gives it
\* (record_fork): its writer, the next, or its parent's deletion. Returns the record and whether
\* the fork stands, the record its making's; the fork's commit is what the child starts from.
RecordFork(rs, parent, f) ==
    LET r == rs[f.child]
        new == Record(IF f.point THEN "point" ELSE "fork", <<parent, f.at>>, 0, f.owner,
                      f.owner[2])
    IN IF r = None THEN [rec |-> new, made |-> TRUE]
       ELSE [rec |-> r, made |-> ~r.deleted /\ ChildOf(r, f.owner)]

\* The fork in F made first.
FirstFork(F) == CHOOSE f \in F : \A g \in F : f.at <= g.at

\* Whether b's record says it lives: a root, a fork's child, a point.
LiveRec(b) == rec[b] # None /\ ~rec[b].deleted /\ rec[b].kind \in {"root", "fork", "point"}

\* The reference of the fork of p at its segment at to c, under p's cell (TASK-395).
RefOf(p, at, c) == [p |-> p, at |-> at, c |-> c]

\* Whether parent's fork f needs a reference: its child is in another cell (references).
NeedsRef(parent, f) == R("references") /\ Cross(parent, f.child)

\* The references of the forks F of parent.
RefsOf(parent, F) == {RefOf(parent, f.at, f.child) : f \in {f \in F : NeedsRef(parent, f)}}

\* The references under cell c.
RefsUnder(c) == {r \in refs : CellOf[r.p] = c}

\* Whether the record r is of the child of parent's fork f, live: its writers write its reference
\* (again), having read it so (record_fork reads the child's record first, TASK-395.3).
Refers(r, f) == r = None \/ (ChildOf(r, f.owner) /\ ~r.deleted)

\* The reference of the fork of p at at to c taken back, the record r of c found naming another
\* origin, or none (referencesTakenBack): no record will name it.
TakenBack(p, at, c, r) ==
    IF R("references") /\ R("referencesTakenBack") /\ r # None /\ r.origin # <<p, at>>
    THEN {RefOf(p, at, c)} ELSE {}

\* The marks of the references Rs retired or taken back (retirementsMarked): kept for good under
\* their parents' cells, so that one a stale writer writes again after, which no recorder nor
\* pass of its child's cell would look at again, its parent's cell's next pass deletes.
Marks(Rs) == IF R("retirementsMarked") THEN Rs ELSE {}

\* What writers of the records of parent's forks F to other cells write of their references, and
\* take back, reading the records as they are: without referencesFirst, once recorded.
RefsWritten(parent, F) ==
    {RefOf(parent, f.at, f.child) : f \in {f \in F : NeedsRef(parent, f) /\ Refers(rec[f.child], f)}}
\* The forks of F whose references their writers write, reading their children's records as they
\* are, with referencesFirst: none there. One there of the child, live, its maker's reference
\* came before it, and stands (ReferencesStand): written again, it might land after its
\* retirement, the child deleted since the read (TASK-395.14).
ToRefer(parent, F) == {f \in F : NeedsRef(parent, f) /\ rec[f.child] = None}
\* Those taken back by the records rs: as recorded, each fork's record found or made in turn.
RefsTakenBackIn(rs, parent, F) ==
    UNION {TakenBack(parent, f.at, f.child, rs[f.child]) : f \in {f \in F : NeedsRef(parent, f)}}
RefsTakenBack(parent, F) == RefsTakenBackIn(rec, parent, F)

RECURSIVE RecordAll(_, _, _, _)
\* The records rs, the forks F of parent recorded, one after another; and the makings told.
RecordAll(rs, ms, parent, F) ==
    IF F = {} THEN [rec |-> rs, makers |-> ms]
    ELSE LET f == FirstFork(F)
             r == RecordFork(rs, parent, f)
         IN RecordAll([rs EXCEPT ![f.child] = r.rec],
                      IF r.made THEN [ms EXCEPT ![f.child] = @ \cup {f.owner}] ELSE ms,
                      parent, F \ {f})

-----------------------------------------------------------------------------
(* Writers. *)

Stopped(h, b) == ses' = [ses EXCEPT ![h][b].pc = "stopped"]

\* Whether the writer of b whose session is s, its state st, has forks acknowledged whose children
\* it hasn't recorded, or, in other cells, written the references of.
Unrecorded(b, st, s) ==
    \E f \in st.forks : f.child \notin s.recorded \/ (NeedsRef(b, f) /\ f.child \notin s.reffed)

\* s, the session of a writer of b, its recovery started: from the newest checkpoint, or its
\* origin's state; stopped, what that needs gone.
Started(s, b) ==
    LET cs == CkSeqs(b)
        p == StateAt(s.origin[1], s.origin[2])
    IN IF cs # {} THEN [s EXCEPT !.pc = "read", !.st = ckpt[b][Max(cs)], !.at = Max(cs) + 1,
                                 !.ck = Max(cs)]
       ELSE IF s.origin = <<>> THEN [s EXCEPT !.pc = "read", !.st = Fresh, !.at = 0, !.ck = -1]
       ELSE IF p.ok THEN [s EXCEPT !.pc = "read", !.st = Inherit(p.st), !.at = 0, !.ck = -1]
       ELSE [s EXCEPT !.pc = "stopped"]

\* h takes b's lease, as a writer of b, of its placement's token, its taking's holder, once it
\* read b's record; then, the record read again, its recovery starts, or, deleted since, it stops
\* (Writer::take). With "staleTake", the first read is from before b's deletion, whose lease the
\* collector may have removed since: taken again, at epoch 1 (TASK-179).
Open(h, b) ==
    /\ takes < MaxTakes
    /\ IF ses[h][b] = None THEN TRUE ELSE ses[h][b].pc = "stopped"
    /\ rec[b] # None
    /\ ~rec[b].deleted \/ ("staleTake" \in Can /\ rec[b].maker \in makers[b])
    /\ IF rec[b].kind = "point" THEN ~R("pointsTakeNoWriter") ELSE rec[b].kind \in {"root", "fork"}
    \* Not deleted's; nor, its token higher, a later placement's (tokens).
    /\ IF lease[b] = None THEN TRUE
       ELSE lease[b].epoch # Final /\ (R("tokens") => lease[b].token <= tok[h][b])
    /\ LET e == IF lease[b] = None THEN 1 ELSE lease[b].epoch + 1
           s == [gen |-> gen + 1, pc |-> "start", epoch |-> e, token |-> tok[h][b],
                 origin |-> rec[b].origin, st |-> Fresh, ck |-> -1, at |-> 0, pre |-> 0,
                 fenced |-> FALSE, top |-> -1, seq |-> 0, last |-> <<>>, next |-> <<0, 0>>,
                 up |-> {}, done |-> {}, open |-> {}, recorded |-> {}, reffed |-> {},
                 reading |-> {}, ckd |-> -1,
                 ckp |-> None, durable |-> None]
       IN /\ lease' = [lease EXCEPT ![b] = [epoch |-> e, token |-> tok[h][b], holder |-> gen + 1]]
          /\ ses' = [ses EXCEPT ![h][b] = IF rec[b].deleted THEN [s EXCEPT !.pc = "stopped"]
                                          ELSE Started(s, b)]
    /\ taken' = [taken EXCEPT ![b] = IF tok[h][b] > @ THEN tok[h][b] ELSE @]
    /\ gen' = gen + 1
    /\ takes' = takes + 1
    /\ UNCHANGED <<seg, own, ckpt, rec, moves, inflight, refs, marked, retired, wal, tok, proc, gc>>
    /\ UNCHANGED <<commits, nextTag, acked, minted, twice, makers, doomed, placed, errors>>
    /\ UNCHANGED <<passes, places, crashes, mints>>

\* Recovery started again, a newer checkpoint come (newestCheckpoint).
RecoverStart(h, b) ==
    /\ ses[h][b] # None /\ ses[h][b].pc = "start"
    /\ ses' = [ses EXCEPT ![h][b] = Started(ses[h][b], b)]
    /\ UNCHANGED <<bucketVars, wal, tok, proc, gc, truthVars, countVars>>

RECURSIVE Restamp(_, _, _, _, _)
\* The WAL's segments R restamped on the chain of epoch e from segment q, after last, minting
\* on from next: their data unchanged, new footers.
Restamp(Rs, q, e, last, next) ==
    IF Rs = <<>> THEN <<>>
    ELSE LET o == [Rs[1].o EXCEPT !.epoch = e, !.after = last, !.next = Max2(@, next)]
         IN <<[s |-> q, o |-> o]>> \o Restamp(Tail(Rs), q + 1, e, <<q, e>>, o.next)

\* The first commit segment o is of: the one it ends, or the one it goes on with.
SegCommit(o) == IF o.commit # 0 THEN o.commit ELSE o.cont.commit

\* The guest's last commit as of the first k segments of the WAL ws, whose base is base
\* (Wal::heads): the last any of them makes, or base.
HeadAt(ws, base, k) ==
    LET C == {i \in 1..k : ws[i].o.commit # 0} IN IF C = {} THEN base ELSE ws[Max(C)].o.commit

\* Whether the WAL's first k segments end every commit they go on with (walBetweenCommits).
Between(ws, k) == k = 0 \/ ws[k].o.cont = None \/ ~R("walBetweenCommits")

\* Whether the fence of a writer whose session is s goes on: it has not created its fence, or
\* not filled the gaps to the last segment listed.
Fencing(s) == ~s.fenced \/ s.seq <= s.top

\* h's writer of b, whose session s fenced the branch, opened: the lease read again
\* (epochSequences), and the chain, minting from the epoch's range. Then its WAL put after the
\* branch's end (wal::rebase), by commits: the WAL's commits to the branch's last, if it is one
\* of them, acknowledged; the rest restamped after the fence, if the branch's last commit is one
\* of the WAL's or the commit the WAL started from (walFollows), as the branch is after the fence
\* (walAfterFence); otherwise the WAL diverged, and is dropped. The commit the WAL started from,
\* its first segment's commit's parent, the WAL keeps. The store keeps the restamped footers in
\* memory alone (TASK-165.9): a crash after finds them stamped as before, which the next opening
\* restamps by the same commits, its own chain, and Max2 with a later floor, to what this would
\* have: so the WAL here holds them restamped.
\*
\* By commits, as Wal::heads has them: the WAL's first k segments are taken out if the guest's
\* last commit as of them is the branch's, and they end no commit begun, unless no such k but 0
\* is, the WAL's base (walBetweenCommits): segments going on with a commit are no branch's end,
\* the fence having left their commit unended. If none is, the branch's last commit may be the
\* one a segment of the WAL's ends going on with the next, landed alone (walContinues): the
\* segments before it are taken out, and it is restamped with its continuation alone, its commit
\* and what that sealed, freed and forked dropped, the branch holding them, then the rest
\* (TASK-345). The WAL's segments replayed over the state after the fence must end every commit
\* they go on with, or the store refuses to open (store.rs: "log ends inside a commit"), its
\* local recovery having kept a commit begun.
Opened(h, b, s) ==
    LET floor == IF R("epochSequences") THEN <<s.epoch, 0>> ELSE <<0, 0>>
        next == Max2(s.st.next, floor)
        ws == wal[h][b]
        head == IF R("walAfterFence") THEN s.st.head ELSE s.pre
        base == IF ws = <<>> THEN 0 ELSE commits[SegCommit(ws[1].o)].parent
        at == {k \in 0..Len(ws) : HeadAt(ws, base, k) = head /\ Between(ws, k)}
        on == {k \in 1..Len(ws) : ws[k].o.commit = head /\ ws[k].o.cont # None}
        cont == at = {} /\ on # {} /\ R("walFollows") /\ R("walContinues")
        k == IF at # {} THEN Max(at) ELSE IF cont THEN Max(on) - 1 ELSE 0
        A == SubSeq(ws, 1, IF cont THEN k + 1 ELSE k)
        Rest == SubSeq(ws, k + 1, Len(ws))
        Rs == IF cont
              THEN <<[Rest[1] EXCEPT !.o.commit = 0, !.o.ext = None, !.o.own = None,
                                     !.o.free = None, !.o.fork = None]>> \o Tail(Rest)
              ELSE Rest
        keep == Rs # <<>> /\ (at # {} \/ cont \/ ~R("walFollows"))
        new == IF keep THEN Restamp(Rs, s.seq, s.epoch, s.st.last, next) ELSE <<>>
    IN IF (R("epochSequences") /\ ~Leased(s, b)) \/ s.seq + Len(new) > Segs
       THEN /\ ses' = [ses EXCEPT ![h][b] = [s EXCEPT !.pc = "stopped"]]
            /\ UNCHANGED <<wal, acked, errors>>
       ELSE IF ApplyAll(s.st, b, new).begun # None
       THEN /\ ses' = [ses EXCEPT ![h][b] = [s EXCEPT !.pc = "stopped"]]
            /\ errors' = errors \cup {"a store's log ends inside a commit"}
            /\ UNCHANGED <<wal, acked>>
       ELSE /\ wal' = [wal EXCEPT ![h][b] = new]
            /\ acked' = [acked EXCEPT ![b] = @ \cup CommitsOf(A)]
            /\ ses' = [ses EXCEPT ![h][b] =
                         [s EXCEPT !.pc = IF Unrecorded(b, s.st, s) THEN "record" ELSE "ready",
                                   !.durable = {new[i].s : i \in DOMAIN new},
                                   !.seq = s.seq + Len(new),
                                   !.last = IF new = <<>> THEN s.st.last
                                            ELSE <<new[Len(new)].s, s.epoch>>,
                                   !.next = IF new = <<>> THEN next ELSE new[Len(new)].o.next]]
            \* Diverged, the WAL dropped, though no other writer's commit moved the branch on: its
            \* last is one of the WAL's, or the WAL's base.
            /\ errors' = IF Rs # <<>> /\ ~keep /\ head \in CommitsOf(ws) \cup {base}
                         THEN errors \cup {"a WAL diverged from a branch holding its commits"}
                         ELSE errors

\* Recovery's segments, to the first missing; then the checkpoints, listed again
\* (newestCheckpoint); then the segments, listed, where the fence goes.
RecoverRead(h, b) ==
    LET s == ses[h][b]
        r == FollowEnd(b, s.st, s.at, {})
        cs == CkSeqs(b)
        newest == IF cs = {} THEN -1 ELSE Max(cs)
        listed == {q \in Seqs : seg[b][q] # None}
        s2 == [s EXCEPT !.pc = "fence", !.st = r.st, !.seq = r.end, !.pre = r.st.head,
                        !.top = IF listed = {} THEN -1 ELSE Max(listed), !.fenced = ~R("fence")]
    IN /\ s # None /\ s.pc = "read"
       /\ IF R("newestCheckpoint") /\ newest # s.ck THEN
             /\ ses' = [ses EXCEPT ![h][b].pc = "start"]
             /\ UNCHANGED <<wal, acked, errors>>
          ELSE IF Fencing(s2) THEN
             /\ ses' = [ses EXCEPT ![h][b] = s2]
             /\ UNCHANGED <<wal, acked, errors>>
          ELSE Opened(h, b, s2)
       /\ UNCHANGED <<bucketVars, tok, proc, gc, countVars>>
       /\ UNCHANGED <<commits, nextTag, minted, twice, makers, doomed, taken, placed>>

\* The fence: an empty segment of the writer's at the branch's end, what landed there first
\* applied before it, and the gaps to the last listed filled; then the writer opened.
Fence(h, b) ==
    LET s == ses[h][b]
        q == s.seq
        f == [epoch |-> s.epoch, after |-> s.st.last, commit |-> 0, ext |-> None, own |-> None,
              free |-> None, next |-> s.st.next, fork |-> None, cont |-> None]
        o == IF seg[b][q] = None THEN f ELSE seg[b][q]
        s2 == [s EXCEPT !.fenced = (@ \/ seg[b][q] = None), !.st = Apply(s.st, b, q, o),
                        !.seq = q + 1]
    IN /\ s # None /\ s.pc = "fence" /\ q < Segs
       /\ IF seg[b][q] # None /\ seg[b][q].epoch > s.epoch THEN
             /\ Stopped(h, b)
             /\ UNCHANGED <<seg, wal, acked, errors>>
          ELSE /\ seg' = [seg EXCEPT ![b][q] = o]
               /\ IF Fencing(s2) THEN
                     ses' = [ses EXCEPT ![h][b] = s2] /\ UNCHANGED <<wal, acked, errors>>
                  ELSE Opened(h, b, s2)
       /\ UNCHANGED <<own, ckpt, rec, lease, moves, inflight, refs, marked, retired, tok, proc, gc,
                      countVars>>
       /\ UNCHANGED <<commits, nextTag, minted, twice, makers, doomed, taken, placed>>

\* The children of the forks acknowledged get their records (record_forks), one at a time; those
\* in other cells first their references (referencesFirst), or, without the rule, after. With
\* the rule, the child's record read, then the reference written, as the code's requests are
\* (record_fork): a recorder whose read is stale writes after another's retirement or take-back.
RecordForks(h, b) ==
    LET s == ses[h][b]
        F == {f \in s.st.forks : f.child \notin s.recorded
                                 \/ (NeedsRef(b, f) /\ f.child \notin s.reffed)}
        f == FirstFork(F)
        r == RecordFork(rec, b, f)
        refer == /\ NeedsRef(b, f) /\ f.child \notin s.reffed
                 /\ R("referencesFirst") \/ f.child \in s.recorded
        back == RefsTakenBack(b, {f})
    IN /\ s # None /\ s.pc = "record"
       /\ IF F = {} THEN
             /\ ses' = [ses EXCEPT ![h][b].pc = "ready"]
             /\ UNCHANGED <<rec, makers, refs, marked, retired>>
          ELSE IF refer /\ ~R("referencesFirst") THEN
             \* Without the rule, once recorded: its reference written, as its record reads.
             /\ refs' = (refs \ back) \cup RefsWritten(b, {f})
             /\ retired' = retired \cup back
             /\ marked' = marked \cup Marks(back)
             /\ ses' = [ses EXCEPT ![h][b].reffed = @ \cup {f.child}]
             /\ UNCHANGED <<rec, makers>>
          ELSE IF refer /\ f.child \in s.reading THEN
             \* Its reference written, its record read none.
             /\ refs' = refs \cup RefsOf(b, {f})
             /\ ses' = [ses EXCEPT ![h][b].reffed = @ \cup {f.child},
                                    ![h][b].reading = @ \ {f.child}]
             /\ UNCHANGED <<rec, makers, marked, retired>>
          ELSE IF refer THEN
             \* The child's record read: none, its reference to be written; another's, or
             \* deleted, taken back; its own, live, its reference its maker's (ToRefer).
             /\ ses' = IF ToRefer(b, {f}) # {}
                       THEN [ses EXCEPT ![h][b].reading = @ \cup {f.child}]
                       ELSE [ses EXCEPT ![h][b].reffed = @ \cup {f.child}]
             /\ refs' = refs \ back
             /\ retired' = retired \cup back
             /\ marked' = marked \cup Marks(back)
             /\ UNCHANGED <<rec, makers>>
          ELSE
             \* The record, unless one is there: another's, the reference taken back.
             /\ rec' = [rec EXCEPT ![f.child] = r.rec]
             /\ makers' = IF r.made THEN [makers EXCEPT ![f.child] = @ \cup {f.owner}]
                          ELSE makers
             /\ ses' = [ses EXCEPT ![h][b].recorded = @ \cup {f.child}]
             /\ refs' = refs \ back
             /\ retired' = retired \cup back
             /\ marked' = marked \cup Marks(back)
       /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, wal, tok, proc, gc>>
       /\ UNCHANGED <<commits, nextTag, acked, minted, twice, doomed, taken, placed, errors>>
       /\ UNCHANGED countVars

Ready(h, b) == ses[h][b] # None /\ ses[h][b].pc = "ready"

\* The guest mints an extent ID, from the chain's next sequence number.
Mint(h, b) ==
    LET s == ses[h][b]
        x == <<b, s.next[1], s.next[2]>>
    IN /\ "mint" \in Can /\ Ready(h, b) /\ mints < MaxMints
       /\ twice' = (twice \/ x \in minted)
       /\ minted' = minted \cup {x}
       /\ ses' = [ses EXCEPT ![h][b].open = @ \cup {x}, ![h][b].next = <<@[1], @[2] + 1>>]
       /\ mints' = mints + 1
       /\ UNCHANGED <<bucketVars, wal, tok, proc, gc>>
       /\ UNCHANGED <<commits, nextTag, acked, makers, doomed, taken, placed, errors>>
       /\ UNCHANGED <<gen, takes, passes, places, crashes>>

\* What a commit does, besides committing: seal an open extent into its segment or on its own,
\* free an extent its guest holds, fork the branch (a point, or not) to a child with no record,
\* with "forkOwn" sealing an open extent on its own too, in the fork's segment (a store's segment
\* holds the commits before the fork it ends, their extents stored on their own among them:
\* TASK-380), or seal one or two open extents where they were written, in segments filled since the last
\* commit that go on with it, one each, the commit ending in the next (a commit spanning
\* segments: TASK-92.41, segments::Phase::Full). With "spanOn", the first of those segments may
\* be the WAL's last, ending the commit before, not uploaded: it filled after that commit, before
\* this one (Segments::record: "the first alone may hold commits, those made before it filled").
Ops(h, b) ==
    LET s == ses[h][b]
        v == View(h, b)
        op(k, x, c, p) == [k |-> k, x |-> x, c |-> c, point |-> p, xs |-> <<>>, on |-> FALSE]
        span(xs, on) == [k |-> "span", x |-> None, c |-> 0, point |-> FALSE, xs |-> xs, on |-> on]
        ws == wal[h][b]
        ons == {FALSE} \cup (IF /\ "spanOn" \in Can /\ ws # <<>>
                                 /\ ws[Len(ws)].s \notin s.up /\ ws[Len(ws)].o.cont = None
                              THEN {TRUE} ELSE {})
        kinds == (IF "fork" \in Can THEN {FALSE} ELSE {})
                 \cup (IF "point" \in Can THEN {TRUE} ELSE {})
        open == IF "span" \in Can THEN s.open ELSE {}
        \* The IDs a fork's child may take: any with no record; with "forkNext", the least, as
        \* an allocator gives them out, for models whose choice of ID would only multiply states.
        free == {c \in Branches \ {b, Root} : rec[c] = None}
    IN (IF "commit" \in Can THEN {op("plain", None, 0, FALSE)} ELSE {})
       \cup {op("seg", x, 0, FALSE) : x \in IF "seg" \in Can THEN s.open ELSE {}}
       \cup {op("own", x, 0, FALSE) : x \in IF "own" \in Can THEN s.open ELSE {}}
       \cup {op("free", x, 0, FALSE) : x \in IF "free" \in Can THEN DOMAIN v.ext ELSE {}}
       \cup {op("fork", x, c, p) : x \in {None} \cup (IF "forkOwn" \in Can THEN s.open ELSE {}),
                                   c \in IF "forkNext" \in Can /\ free # {} THEN {Min(free)}
                                         ELSE free,
                                   p \in kinds}
       \cup {span(<<x>>, on) : x \in open, on \in ons}
       \cup {span(<<x, y>>, on) : <<x, y>> \in {p \in open \X open : p[1] # p[2]}, on \in ons}

\* A commit: segments of the WAL, stamped by the chain: those going on with it, if it spans
\* segments (Contents::continued, each naming the commit and its run there), the first perhaps
\* the WAL's last, then the one it ends in.
Commit(h, b) ==
    LET s == ses[h][b]
        t == nextTag
        ws == wal[h][b]
    IN /\ Ready(h, b) /\ t <= MaxCommits
       /\ \E op \in Ops(h, b) :
            LET on == IF op.on THEN 1 ELSE 0
                n == Len(op.xs) - on
                f == [child |-> op.c, owner |-> <<"fork", t>>, point |-> op.point]
                cont(i) == [commit |-> t, ext |-> op.xs[i]]
                part(i) == [s |-> s.seq + i - 1,
                            o |-> [epoch |-> s.epoch,
                                   after |-> IF i = 1 THEN s.last ELSE <<s.seq + i - 2, s.epoch>>,
                                   commit |-> 0, ext |-> None, own |-> None, free |-> None,
                                   next |-> s.next, fork |-> None, cont |-> cont(i + on)]]
                before == IF op.on THEN [ws EXCEPT ![Len(ws)].o.cont = cont(1)] ELSE ws
                o == [epoch |-> s.epoch,
                      after |-> IF n = 0 THEN s.last ELSE <<s.seq + n - 1, s.epoch>>,
                      commit |-> t,
                      ext |-> IF op.k = "seg" THEN op.x ELSE None,
                      own |-> IF op.k \in {"own", "fork"} THEN op.x ELSE None,
                      free |-> IF op.k = "free" THEN op.x ELSE None,
                      next |-> s.next,
                      fork |-> IF op.k = "fork" THEN f ELSE None,
                      cont |-> None]
                xs == {op.xs[i] : i \in DOMAIN op.xs}
            IN /\ s.seq + n < Segs
               /\ wal' = [wal EXCEPT ![h][b] = before \o [i \in 1..n |-> part(i)]
                                                 \o <<[s |-> s.seq + n, o |-> o]>>]
               /\ ses' = [ses EXCEPT ![h][b] =
                            [s EXCEPT !.seq = @ + n + 1, !.last = <<s.seq + n, s.epoch>>,
                                      !.open = @ \ ({op.x} \cup xs)]]
               /\ commits' = [commits EXCEPT ![t] = [branch |-> b, parent |-> View(h, b).head,
                                                      seals |-> IF op.k \in {"seg", "own"}
                                                                   \/ op.x # None
                                                                THEN {op.x} ELSE xs,
                                                      free |-> o.free]]
               \* Without recordsAfterAck, the child's record at once, and its reference.
               /\ rec' = IF op.k = "fork" /\ ~R("recordsAfterAck")
                         THEN [rec EXCEPT ![op.c] = Record(IF op.point THEN "point" ELSE "fork",
                                                           <<b, s.seq>>, 0, f.owner, t)]
                         ELSE rec
               /\ refs' = IF op.k = "fork" /\ ~R("recordsAfterAck")
                          THEN refs \cup RefsOf(b, {[child |-> op.c, at |-> s.seq]})
                          ELSE refs
       /\ nextTag' = t + 1
       /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, marked, retired, tok, proc, gc>>
       /\ UNCHANGED <<acked, minted, twice, makers, doomed, taken, placed, errors, countVars>>

\* The upload of the WAL's next segment: its extent of its own created, then it in flight.
Upload(h, b) ==
    LET s == ses[h][b]
        ws == wal[h][b]
        I == {i \in DOMAIN ws : ws[i].s \notin s.up}
        w == ws[Min(I)]
    IN /\ Ready(h, b) /\ I # {}
       /\ own' = IF w.o.own = None THEN own
                 ELSE own \cup {[b |-> b, e |-> w.o.epoch, s |-> w.s, x |-> w.o.own]}
       /\ inflight' = inflight \cup {[h |-> h, b |-> b, g |-> s.gen, s |-> w.s, o |-> w.o]}
       /\ ses' = [ses EXCEPT ![h][b].up = @ \cup {w.s}]
       /\ UNCHANGED <<seg, ckpt, rec, lease, moves, refs, marked, retired, wal, tok, proc, gc,
                      truthVars, countVars>>

\* A create lands, whenever, though its writer has gone: where nothing is, or its own is; else
\* its writer, if it still is, stops (fenced, or displaced). With "landInOrder", a writer's creates
\* land in the order it made them, the earliest first, landing or failing: a model of more
\* writers kept to a size TLC exhausts (BucketTakeovers), BucketStrays landing them in any order.
Land(m) ==
    LET s == ses[m.h][m.b]
        mine == s # None /\ s.gen = m.g /\ s.pc # "stopped"
    IN /\ "landInOrder" \notin Can
          \/ \A n \in inflight : n.h = m.h /\ n.b = m.b /\ n.g = m.g => n.s >= m.s
       /\ inflight' = inflight \ {m}
       /\ IF seg[m.b][m.s] = None \/ seg[m.b][m.s] = m.o THEN
             /\ seg' = [seg EXCEPT ![m.b][m.s] = m.o]
             /\ ses' = IF mine THEN [ses EXCEPT ![m.h][m.b].done = @ \cup {m.s}] ELSE ses
          ELSE
             /\ ses' = IF mine THEN [ses EXCEPT ![m.h][m.b].pc = "stopped"] ELSE ses
             /\ UNCHANGED seg
       /\ UNCHANGED <<own, ckpt, rec, lease, moves, refs, marked, retired, wal, tok, proc, gc,
                      truthVars, countVars>>

\* The acknowledgement of the WAL's prefix created, its creates the fencing (Pinned); if it
\* holds a fork, once a read of the lease shows it the writer's (forksAfterLease): its commits
\* told durable, its forks' children to be recorded. To
\* the last segment going on with no commit (acksBetweenCommits, Writer::acknowledge): a commit
\* spanning segments is acknowledged with the one it ends in, so that the state acknowledged,
\* which checkpoints hold, holds no commit begun, and the WAL keeps it whole.
Ack(h, b) ==
    LET s == ses[h][b]
        ws == wal[h][b]
        n == Max({k \in 0..Len(ws) : /\ \A i \in 1..k : ws[i].s \in s.done
                                     /\ k = 0 \/ ws[k].o.cont = None \/ ~R("acksBetweenCommits")})
        P == SubSeq(ws, 1, n)
    IN /\ Ready(h, b) /\ n > 0
       /\ IF /\ R("forksAfterLease") /\ ~Leased(s, b)
             /\ \E i \in DOMAIN P : P[i].o.fork # None THEN
             Stopped(h, b) /\ UNCHANGED <<wal, acked>>
          ELSE /\ wal' = [wal EXCEPT ![h][b] = SubSeq(ws, n + 1, Len(ws))]
               /\ ses' = [ses EXCEPT ![h][b] =
                            [s EXCEPT !.st = ApplyAll(s.st, b, P),
                                      !.pc = IF Unrecorded(b, ApplyAll(s.st, b, P), s)
                                             THEN "record" ELSE "ready"]]
               /\ acked' = [acked EXCEPT ![b] = @ \cup CommitsOf(P)]
       /\ UNCHANGED <<bucketVars, tok, proc, gc, countVars>>
       /\ UNCHANGED <<commits, nextTag, minted, twice, makers, doomed, taken, placed, errors>>

\* A checkpoint as of the last segment acknowledged: the extents the collector moved where they
\* went, the forks whose children were deleted dropped; its create then in flight. The writer
\* names those extents where they went now, before it creates it (movesTakenFirst), only forward
\* along their ways; without the rule, once it is created.
Checkpoint(h, b) ==
    LET s == ses[h][b]
        q == s.st.through
        st == [s.st EXCEPT !.ext = [x \in DOMAIN s.st.ext |-> Moved(x, s.st.ext[x])],
                           !.forks = {f \in s.st.forks :
                                         ~(rec[f.child] # None /\ rec[f.child].deleted)}]
        kept == [s.st EXCEPT !.forks = st.forks]
    IN /\ "checkpoint" \in Can /\ Ready(h, b) /\ q >= 0 /\ q > s.ckd /\ s.ckp = None
       /\ ses' = [ses EXCEPT ![h][b].st = IF R("movesTakenFirst") THEN TakeMoves(kept, st)
                                         ELSE kept,
                             ![h][b].ckd = q,
                             ![h][b].ckp = [q |-> q, st |-> st, landed |-> FALSE]]
       /\ UNCHANGED <<bucketVars, wal, tok, proc, gc, truthVars, countVars>>

\* The checkpoint's create lands, whenever while its writer's session lasts, though it stopped;
\* without movesTakenFirst, the writer names what it moved where it went once it is answered.
CheckpointLand(h, b) ==
    LET s == ses[h][b] IN
    /\ s # None /\ s.ckp # None /\ ~s.ckp.landed
    /\ ckpt' = IF ckpt[b][s.ckp.q] = None THEN [ckpt EXCEPT ![b][s.ckp.q] = s.ckp.st] ELSE ckpt
    /\ ses' = [ses EXCEPT ![h][b].ckp = IF R("movesTakenFirst") THEN None
                                      ELSE [@ EXCEPT !.landed = TRUE]]
    /\ UNCHANGED <<seg, own, rec, lease, moves, inflight, refs, marked, retired, wal, tok, proc, gc,
                   truthVars, countVars>>

\* The create answered, the writer names what the checkpoint moved where it went, only forward
\* along their ways (without movesTakenFirst).
CheckpointTaken(h, b) ==
    LET s == ses[h][b] IN
    /\ s # None /\ s.ckp # None /\ s.ckp.landed
    /\ ses' = [ses EXCEPT ![h][b].st = TakeMoves(s.st, s.ckp.st), ![h][b].ckp = None]
    /\ UNCHANGED <<bucketVars, wal, tok, proc, gc, truthVars, countVars>>

WriterNext ==
    \E h \in Hosts, b \in Branches :
        \/ Open(h, b) \/ RecoverStart(h, b) \/ RecoverRead(h, b) \/ Fence(h, b)
        \/ RecordForks(h, b) \/ Mint(h, b) \/ Commit(h, b) \/ Upload(h, b)
        \/ Ack(h, b) \/ Checkpoint(h, b) \/ CheckpointLand(h, b) \/ CheckpointTaken(h, b)

-----------------------------------------------------------------------------
(* Hosts and placements. *)

\* The lengths a crash may cut h's WAL of b to, with "cut": its commits to the last a flush made
\* durable, at least those of every segment uploaded, which waited for theirs (uploads.rs's tag:
\* a segment going on with a commit, for that commit), and those durable before the writer's
\* session (its WAL as it opened; all of it, if it has not). Local recovery (wal::recover) keeps a
\* commit spanning segments only whole, if consecutive segments end it, and closes the first
\* again without it otherwise (wholeCommitsKept): the WAL is cut between commits, as a flush
\* appends a commit's records to its segments' logs one after another, a crash stopping it, or
\* their checks against the raw data, at any of them.
Cuts(h, b) ==
    LET ws == wal[h][b]
        s == ses[h][b]
        up == IF s = None \/ s.durable = None THEN Len(ws)
              ELSE Max({0} \cup {i \in DOMAIN ws : ws[i].s \in s.up \cup s.durable})
        whole(k) == k = 0 \/ ws[k].o.cont = None
        lo == Min({k \in up..Len(ws) : whole(k)} \cup {Len(ws)})
    IN IF "cut" \in Can
       THEN {k \in lo..Len(ws) : whole(k) \/ ~R("wholeCommitsKept")}
       ELSE {Len(ws)}

\* A host crashes: its writers and its other work stop; its WALs stay, or the disk is lost; with
\* "cut", each to its last durable commit.
Crash(h) ==
    /\ "crash" \in Can /\ crashes < MaxCrashes
    /\ \E lose \in IF "disk" \in Can THEN {FALSE, TRUE} ELSE {FALSE},
          cut \in [Branches -> 0..Segs] :
          /\ \A b \in Branches : cut[b] \in Cuts(h, b)
          /\ wal' = [wal EXCEPT ![h] = [b \in Branches |-> IF lose THEN <<>>
                                                           ELSE SubSeq(@[b], 1, cut[b])]]
    /\ ses' = [ses EXCEPT ![h] = [b \in Branches |-> None]]
    /\ proc' = [proc EXCEPT ![h] = None]
    /\ crashes' = crashes + 1
    /\ UNCHANGED <<bucketVars, tok, gc, truthVars, gen, takes, passes, places, mints>>

\* b placed on h: a fencing token above every earlier placement's.
Place(h, b) ==
    /\ "place" \in Can /\ places < MaxPlaces /\ rec[b] # None
    /\ tok' = [tok EXCEPT ![h][b] = placed[b] + 1]
    /\ placed' = [placed EXCEPT ![b] = @ + 1]
    /\ places' = places + 1
    /\ UNCHANGED <<bucketVars, ses, wal, proc, gc, gen, takes, passes, crashes, mints>>
    /\ UNCHANGED <<commits, nextTag, acked, minted, twice, makers, doomed, taken, errors>>

\* Another branch takes the ID c, as a guest may give any (TASK-79.82).
Contest(c) ==
    /\ "contest" \in Can /\ c # Root /\ rec[c] = None
    /\ rec' = [rec EXCEPT ![c] = Record("root", <<>>, 0, <<"root", c>>, 0)]
    /\ makers' = [makers EXCEPT ![c] = @ \cup {<<"root", c>>}]
    /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, refs, marked, retired, hostVars, gc,
                   countVars>>
    /\ UNCHANGED <<commits, nextTag, acked, minted, twice, doomed, taken, placed, errors>>

-----------------------------------------------------------------------------
(* Deletions, failed forks' deletions, and forks of points: a host's work, one at a time. *)

\* A deletion of b, asked on h: the lease ended first. Asked by b's last placement, or one as
\* late, it may delete b's making. With "deleteRoot" alone, of the root alone.
StartDelete(h, b) ==
    /\ proc[h] = None
    /\ rec[b] # None /\ ~rec[b].deleted
    /\ \/ "delete" \in Can
       \/ "deletePoints" \in Can /\ rec[b].kind = "point"
       \/ "deleteRoot" \in Can /\ b = Root
    /\ proc' = [proc EXCEPT ![h] = [op |-> "delete", b |-> b, pc |-> "end", c |-> 0, p |-> 0,
                                    at |-> 0, o |-> <<>>, live |-> FALSE, tk |-> tok[h][b],
                                    ck |-> -1, fs |-> {}, ws |-> {}, og |-> <<>>, bs |-> 0]]
    /\ doomed' = IF tok[h][b] >= taken[b] THEN doomed \cup {rec[b].maker} ELSE doomed
    /\ UNCHANGED <<bucketVars, ses, wal, tok, gc, countVars>>
    /\ UNCHANGED <<commits, nextTag, acked, minted, twice, makers, taken, placed, errors>>

\* The deletion of a fork that failed, its writer stopped before acknowledging it
\* (delete_fork): of the child, if its record is the fork's or it has none.
StartUnfork(h, b) ==
    LET s == ses[h][b]
        F == {i \in DOMAIN wal[h][b] : wal[h][b][i].o.fork # None}
    IN /\ "unfork" \in Can /\ proc[h] = None /\ s # None /\ s.pc = "stopped"
       /\ \E i \in F :
            LET w == wal[h][b][i] IN
            /\ proc' = [proc EXCEPT ![h] = [op |-> "unfork", b |-> w.o.fork.child, pc |-> "read",
                                            c |-> w.o.fork.child, p |-> b, at |-> w.s,
                                            o |-> w.o.fork.owner, live |-> FALSE,
                                            tk |-> tok[h][w.o.fork.child], ck |-> -1,
                                            fs |-> {}, ws |-> {}, og |-> <<>>, bs |-> 0]]
            /\ doomed' = doomed \cup {w.o.fork.owner}
       /\ UNCHANGED <<bucketVars, ses, wal, tok, gc, countVars>>
       /\ UNCHANGED <<commits, nextTag, acked, minted, twice, makers, taken, placed, errors>>

\* The reference of the fork of a point, the restore q, to its child: under the cell of the
\* origin the point names, whatever the child's cell. One in that cell too: the cell's pass reads
\* its records before the states and its references after, and the point's own reference, in
\* another cell, may be retired between, its point deleted, while the child's record came after
\* the records were read (BucketOverlap found it).
CloneRefs(q) == IF R("references") THEN {RefOf(q.og[1], q.og[2], q.c)} ELSE {}

\* The record of the child of the restore q, being made (forksPending).
Forking(q) == Record(IF R("forksPending") THEN "forking" ELSE "fork", q.og, q.p, q.o, q.bs)

\* A fork of point P to C (fork_point), of the origin P names: first C's record, saying it is
\* being made (forksPending), then its reference (clonesRecordFirst), before it reads P's
\* (clonesReferFirst); without clonesRecordFirst, the reference first. From a host's copy of P,
\* stale perhaps.
StartRestore(h, c, p) ==
    LET q == [op |-> "restore", b |-> c, pc |-> "check", c |-> c, p |-> p, at |-> 0,
              o |-> <<"restore", c, h>>, live |-> FALSE, tk |-> tok[h][c], ck |-> -1, fs |-> {},
              ws |-> {}, og |-> rec[p].origin, bs |-> rec[p].base]
        refers == R("clonesReferFirst") /\ CloneRefs(q) # {}
        first == refers /\ ~R("clonesRecordFirst")
    IN /\ "restore" \in Can /\ proc[h] = None
       /\ rec[p] # None /\ rec[p].kind = "point" /\ rec[c] = None /\ c \notin {Root, p}
       /\ IF first THEN
             /\ refs' = refs \cup CloneRefs(q)
             /\ proc' = [proc EXCEPT ![h] = [q EXCEPT !.pc = "fork"]]
             /\ UNCHANGED rec
          ELSE
             /\ rec' = [rec EXCEPT ![c] = Forking(q)]
             /\ proc' = [proc EXCEPT ![h] = [q EXCEPT !.pc = IF refers THEN "refer" ELSE "check"]]
             /\ UNCHANGED refs
       /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, marked, retired, ses, wal, tok, gc,
                      truthVars, countVars>>

Done(h) == proc' = [proc EXCEPT ![h] = None]
\* A deletion of the branch proc[h] names, after a failed fork's or restore's.
Deleting(h) == proc' = [proc EXCEPT ![h] = [@ EXCEPT !.op = "delete", !.pc = "end"]]

ProcStep(h) ==
    LET q == proc[h]
        b == q.b
    IN /\ q # None
       /\ CASE q.op = "delete" /\ q.pc = "end" ->
                 \* end_lease, unless a later placement took the branch (deleteTokens).
                 /\ IF lease[b] # None /\ lease[b].epoch # Final /\ R("deleteTokens")
                       /\ lease[b].token > q.tk
                    THEN Done(h) /\ UNCHANGED lease
                    ELSE /\ lease' = [lease EXCEPT ![b] = [epoch |-> Final, token |-> 0,
                                                           holder |-> 0]]
                         /\ proc' = [proc EXCEPT ![h].pc = "mark"]
                 /\ UNCHANGED <<seg, own, ckpt, rec, moves, inflight, makers, errors>>
            [] q.op = "delete" /\ q.pc = "mark" ->
                 \* Its record, deleted: made so if it has none; else, unless it is deleted or
                 \* being made, its checkpoints listed, to recover it.
                 LET r == rec[b] IN
                 /\ IF r = None THEN
                       /\ rec' = [rec EXCEPT ![b] =
                                    [Record("fork", <<>>, 0, <<"deletion", h>>, 0)
                                         EXCEPT !.deleted = TRUE]]
                       /\ Done(h)
                    ELSE IF r.deleted THEN UNCHANGED rec /\ Done(h)
                    ELSE IF r.kind = "forking" THEN
                       /\ rec' = [rec EXCEPT ![b].deleted = TRUE]
                       /\ Done(h)
                    ELSE /\ proc' = [proc EXCEPT ![h].pc = "recover", ![h].ck = Newest(b)]
                         /\ UNCHANGED rec
                 /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, makers, errors>>
            [] q.op = "delete" /\ q.pc = "recover" /\ rec[b] = None ->
                 \* Its record dropped since, as only a deleted root's is, without recordsKept:
                 \* done, as by another deletion.
                 /\ Done(h)
                 /\ UNCHANGED <<seg, own, ckpt, rec, lease, moves, inflight, makers, errors>>
            [] q.op = "delete" /\ q.pc = "recover" /\ rec[b] # None ->
                 \* Its recovery, starting again with the checkpoints listed again; then an
                 \* empty segment of the final epoch created where it found the first missing
                 \* (deletionsFence; a point, which no writer has, needs none), and its record
                 \* read again, that read, the create and the read one step: deleted since, by
                 \* another deletion, it is done (deletionsReread); else its forks are recorded
                 \* next, those in other cells their references first (referencesFirst). Finding
                 \* what it needs lost: done if the record was deleted since
                 \* (deletionsDoneByOthers), or a claim no writer has.
                 LET got == Recovering(b, rec[b].origin, q.ck)
                     fence == /\ R("deletionsFence") /\ got.ok /\ got.end < Segs
                              /\ rec[b].kind # "point"
                 IN
                 /\ IF got.restart THEN
                       /\ proc' = [proc EXCEPT ![h].ck = Newest(b)]
                       /\ UNCHANGED <<rec, makers, errors>>
                    ELSE IF got.ok THEN
                       /\ IF R("deletionsReread") /\ rec[b].deleted THEN Done(h)
                          ELSE proc' = [proc EXCEPT ![h].pc = IF /\ R("referencesFirst")
                                                                 /\ RefsOf(b, got.st.forks) # {}
                                                              THEN "refs" ELSE "record",
                                                    ![h].fs = got.st.forks]
                       /\ UNCHANGED <<rec, makers, errors>>
                    ELSE IF R("deletionsDoneByOthers") /\ rec[b].deleted THEN
                       Done(h) /\ UNCHANGED <<rec, makers, errors>>
                    ELSE IF rec[b].kind = "claim" /\ Unwritten(b) THEN
                       /\ rec' = [rec EXCEPT ![b].deleted = TRUE]
                       /\ Done(h)
                       /\ UNCHANGED <<makers, errors>>
                    ELSE /\ errors' = errors \cup {"a deletion lost what it needs"}
                         /\ Done(h)
                         /\ UNCHANGED <<rec, makers>>
                 /\ seg' = IF fence
                           THEN [seg EXCEPT ![b][got.end] =
                                   [epoch |-> Final, after |-> got.st.last, commit |-> 0,
                                    ext |-> None, own |-> None, free |-> None,
                                    next |-> got.st.next, fork |-> None, cont |-> None]]
                           ELSE seg
                 /\ UNCHANGED <<own, ckpt, lease, moves, inflight>>
            [] q.op = "delete" /\ q.pc = "refs" ->
                 \* The references of the forks its recovery found to other cells' children,
                 \* before their records (referencesFirst): the children's records read, those
                 \* of other origins or none taken back, and those with none to be written, in a
                 \* step after (record_fork). Without the rule, after its own record is marked
                 \* deleted, in the one step.
                 /\ IF ~R("referencesFirst") THEN Done(h)
                    ELSE IF ToRefer(b, q.fs) # {} THEN
                       proc' = [proc EXCEPT ![h].pc = "write", ![h].ws = ToRefer(b, q.fs)]
                    ELSE proc' = [proc EXCEPT ![h].pc = "record"]
                 /\ UNCHANGED <<seg, own, ckpt, rec, lease, moves, inflight, makers, errors>>
            [] q.op = "delete" /\ q.pc = "write" ->
                 \* Those references written, as their records read none.
                 /\ proc' = [proc EXCEPT ![h].pc = "record", ![h].ws = {}]
                 /\ UNCHANGED <<seg, own, ckpt, rec, lease, moves, inflight, makers, errors>>
            [] q.op = "delete" /\ q.pc = "record" ->
                 \* The children of the forks its recovery found recorded, and its record
                 \* deleted, unless the collector dropped it (without recordsKept).
                 LET all == RecordAll(rec, makers, b, q.fs) IN
                 /\ rec' = IF all.rec[b] = None THEN all.rec
                           ELSE [all.rec EXCEPT ![b].deleted = TRUE]
                 /\ makers' = all.makers
                 /\ IF ~R("referencesFirst") /\ RefsOf(b, q.fs) # {}
                    THEN proc' = [proc EXCEPT ![h].pc = "refs"]
                    ELSE Done(h)
                 /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, errors>>
            [] q.op = "unfork" /\ q.pc = "read" ->
                 /\ IF rec[b] # None THEN
                       IF ~rec[b].deleted /\ ChildOf(rec[b], q.o) THEN Deleting(h) ELSE Done(h)
                    ELSE IF ~R("forkClaimsIds") THEN Deleting(h)
                    ELSE proc' = [proc EXCEPT ![h].pc = "claim"]
                 /\ UNCHANGED <<seg, own, ckpt, rec, lease, moves, inflight, makers, errors>>
            [] q.op = "unfork" /\ q.pc = "claim" ->
                 \* The child's ID taken with a record: live, a claim, only if its parent holds
                 \* the fork's segment (forkClaimsLanded).
                 LET live == ~R("forkClaimsLanded") \/ Holds(q.p, q.at)
                     kind == IF R("forkClaimsLanded") /\ live THEN "claim" ELSE "fork"
                 IN IF rec[b] = None THEN
                       /\ rec' = [rec EXCEPT ![b] = [Record(kind, <<q.p, q.at>>, 0, q.o, q.o[2])
                                                        EXCEPT !.deleted = ~live]]
                       /\ proc' = [proc EXCEPT ![h].pc = "end", ![h].live = live,
                                               ![h].op = "unforkEnd"]
                       /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, makers, errors>>
                    ELSE /\ IF ~rec[b].deleted /\ ChildOf(rec[b], q.o) THEN Deleting(h)
                            ELSE Done(h)
                         /\ UNCHANGED <<seg, own, ckpt, rec, lease, moves, inflight, makers,
                                        errors>>
            [] q.op = "unforkEnd" /\ q.pc = "end" ->
                 /\ IF lease[b] # None /\ lease[b].epoch # Final /\ R("deleteTokens")
                       /\ lease[b].token > q.tk
                    THEN Done(h) /\ UNCHANGED lease
                    ELSE /\ lease' = [lease EXCEPT ![b] = [epoch |-> Final, token |-> 0,
                                                           holder |-> 0]]
                         /\ proc' = [proc EXCEPT ![h].pc = "mark"]
                 /\ UNCHANGED <<seg, own, ckpt, rec, moves, inflight, makers, errors>>
            [] q.op = "unforkEnd" /\ q.pc = "mark" ->
                 /\ rec' = IF q.live /\ ~rec[b].deleted THEN [rec EXCEPT ![b].deleted = TRUE]
                           ELSE rec
                 /\ Done(h)
                 /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, makers, errors>>
            [] q.op = "restore" /\ q.pc = "refer" ->
                 \* Its reference, its record made (clonesRecordFirst), before the point's read.
                 /\ proc' = [proc EXCEPT ![h].pc = "check"]
                 /\ UNCHANGED <<seg, own, ckpt, rec, lease, moves, inflight, makers, errors>>
            [] q.op = "restore" /\ q.pc = "fork" ->
                 \* Without clonesRecordFirst: the child's record, its reference made, unless its ID
                 \* was taken since: the reference then taken back (referencesTakenBack), or left
                 \* for good.
                 /\ IF rec[b] = None THEN /\ rec' = [rec EXCEPT ![b] = Forking(q)]
                                          /\ proc' = [proc EXCEPT ![h].pc = "check"]
                    ELSE Done(h) /\ UNCHANGED rec
                 /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, makers, errors>>
            [] q.op = "restore" /\ q.pc = "check" ->
                 \* The point's record, read after the child's (forksCheckPoints): deleted, the
                 \* fork fails, and its host deletes the child.
                 /\ IF R("forksCheckPoints") /\ (rec[q.p] = None \/ rec[q.p].deleted)
                    THEN Deleting(h)
                    ELSE proc' = [proc EXCEPT ![h].pc = "make"]
                 /\ UNCHANGED <<seg, own, ckpt, rec, lease, moves, inflight, makers, errors>>
            [] q.op = "restore" /\ q.pc = "make" ->
                 \* No longer being made, with a condition: unless deleted, given up on. Without
                 \* clonesReferFirst, its reference made now, after the point's read.
                 /\ IF rec[b] # None /\ ~rec[b].deleted THEN
                       /\ rec' = [rec EXCEPT ![b].kind = IF @ = "forking" THEN "fork" ELSE @]
                       /\ makers' = [makers EXCEPT ![b] = @ \cup {q.o}]
                    ELSE UNCHANGED <<rec, makers>>
                 /\ Done(h)
                 /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, errors>>
       /\ doomed' = IF q.op = "restore" /\ q.pc = "check" /\ R("forksCheckPoints")
                       /\ (rec[q.p] = None \/ rec[q.p].deleted)
                    THEN doomed \cup {q.o} ELSE doomed
       \* A deletion's references of the forks it found, as their records read (record_fork):
       \* taken back, those records another's (referencesTakenBack), or written, in a step after,
       \* those with none; and taken back as it records the children; a clone's. Taken back,
       \* marked (retirementsMarked).
       /\ LET back == CASE q.op = "delete" /\ q.pc = "refs" -> RefsTakenBack(b, q.fs)
                         [] q.op = "delete" /\ q.pc = "record" ->
                             RefsTakenBackIn(RecordAll(rec, makers, b, q.fs).rec, b, q.fs)
                         [] q.op = "restore" /\ q.pc = "fork" /\ rec[b] # None ->
                             TakenBack(q.og[1], q.og[2], q.c, rec[b])
                         [] OTHER -> {}
          IN /\ refs' = (refs \ back)
                       \cup CASE q.op = "delete" /\ q.pc = "refs" /\ ~R("referencesFirst") ->
                                  RefsWritten(b, q.fs)
                               [] q.op = "delete" /\ q.pc = "write" -> RefsOf(b, q.ws)
                               [] q.op = "restore" /\ q.pc = "refer" -> CloneRefs(q)
                               [] (/\ q.op = "restore" /\ q.pc = "make"
                                   /\ ~R("clonesReferFirst") /\ rec[b] # None
                                   /\ ~rec[b].deleted) -> CloneRefs(q)
                               [] OTHER -> {}
             /\ retired' = retired \cup back
             /\ marked' = marked \cup Marks(back)
       /\ UNCHANGED <<ses, wal, tok, gc, countVars>>
       /\ UNCHANGED <<commits, nextTag, acked, minted, twice, taken, placed>>

HostNext ==
    \/ \E h \in Hosts : Crash(h) \/ ProcStep(h)
    \/ \E h \in Hosts, b \in Branches : Place(h, b) \/ StartDelete(h, b) \/ StartUnfork(h, b)
    \/ \E h \in Hosts, c, p \in Branches : StartRestore(h, c, p)
    \/ \E c \in Branches : Contest(c)
    \/ \E m \in inflight : Land(m)

-----------------------------------------------------------------------------
(* The collector. *)

NoRecs == [b \in Branches |-> None]
\* A pass: its cell, what it listed, the records it read, the branches it recovered, whether one
\* was from a checkpoint, the references it listed, and the fork points' states it keeps, each
\* [ext, touched]. Idle, a pass keeps its cell: its epoch is that cell's collector lease's.
GcIdle == [pc |-> "idle", cell |-> 0, objs |-> {}, recs |-> NoRecs, live |-> Nothing,
           ck |-> FALSE, refs |-> {}, pts |-> {}, stale |-> {}]
Idle(g) == [GcIdle EXCEPT !.cell = g.cell]

\* Every object's key under cell c's prefix but references, which are no pass's candidates: the
\* child's cell's pass retires them.
Listing(c) ==
    LET B == CellBranches(c) IN
    {k \in {<<"seg", b, s>> : b \in B, s \in Seqs} : seg[k[2]][k[3]] # None}
    \cup {<<"own", o.b, o.e, o.s, o.x>> : o \in {o \in own : o.b \in B}}
    \cup {k \in {<<"ckpt", b, s>> : b \in B, s \in Seqs} : ckpt[k[2]][k[3]] # None}
    \cup {<<"rec", b>> : b \in {b \in B : rec[b] # None}}
    \cup {<<"lease", b>> : b \in {b \in B : lease[b] # None}}
    \cup {k \in {<<"moves", b, s, d>> : b \in B, s \in Seqs, d \in 1..Hops} :
            moves[k[2]][k[3]][k[4]] # None}

\* The state of p at its fork point at, as a pass keeps it for a fork to another cell: the extents
\* it names, and what recovering it reads (forkPointsKept); without the rule, the extents alone.
ForkPoint(p, at) ==
    LET r == StateAt(p, at) IN
    [ok |-> r.ok, ext |-> r.st.ext, touched |-> IF R("forkPointsKept") THEN r.touched ELSE {}]
Kept(fp) == [ext |-> fp.ext, touched |-> fp.touched]

\* The fork points of the references Rs: the states kept, and whether one is lost that a live
\* child of theirs needs. A child not live (deleted, being made of a deleted point, a claim, or
\* no record yet, or another's, its reference to be named or taken back: ReferencesNamed) loses
\* nothing.
RefPoints(Rs) ==
    LET P == {<<r.p, r.at>> : r \in Rs}
        lost(pt) == /\ ~ForkPoint(pt[1], pt[2]).ok
                    /\ \E r \in Rs : <<r.p, r.at>> = pt /\ LiveRec(r.c)
    IN [pts |-> {Kept(ForkPoint(pt[1], pt[2])) : pt \in {pt \in P : ForkPoint(pt[1], pt[2]).ok}},
        lost |-> \E pt \in P : lost(pt)]

\* Whether the writer of b may name the extents of its state, recovered from its checkpoint ck
\* (-1 if none), anywhere on their ways from where they were sealed (waysFromSeals): unless the
\* checkpoint is of the lease's epoch, read after the checkpoints were listed (collector::behind).
\* A fork's inherited extents, which a fork with no checkpoint holds, always.
Behind(b, ck) == R("waysFromSeals") /\ (ck < 0 \/ ckpt[b][ck].epoch # LeaseEpoch(b))

\* Whether acc holds a recovery of q, <<b, origin>>: by that origin (recoveriesByOrigins), or,
\* without the rule, by any.
Recovered(acc, q) ==
    IF R("recoveriesByOrigins") THEN q \in DOMAIN acc.live
    ELSE \E k \in DOMAIN acc.live : k[1] = q[1]

RECURSIVE RecoverAll(_, _, _)
\* The live branches recovered into acc, those queued, <<b, origin>>, each once by each origin
\* (recoveriesByOrigins), and the children of their forks with no record read (forksFromStates):
\* a child recovered by its parent's state is recovered again by a record found after naming
\* another origin, a restore's that took the ID (TASK-173). Of the forks to other cells, the
\* parent's state at each fork point is kept instead (forksFromStates too): the child's cell's
\* pass recovers the child. A failure that loses nothing (deleted since, a claim no writer has) is
\* skipped, the claim given up on; any other loses the pass.
RecoverAll(queue, recs, acc) ==
    IF queue = {} \/ acc.lost THEN acc
    ELSE LET q == CHOOSE q \in queue : TRUE
             b == q[1]
             rest == queue \ {q}
             r == Recover(b, q[2])
             found == IF R("forksFromStates") THEN r.st.forks ELSE {}
             kids == {<<f.child, <<b, f.at>>>> :
                        f \in {f \in found : ~Cross(b, f.child) /\ recs[f.child] = None}}
             fps == {ForkPoint(b, f.at) : f \in {f \in found : Cross(b, f.child)}}
         IN IF Recovered(acc, q) THEN RecoverAll(rest, recs, acc)
            ELSE IF r.ok /\ \E fp \in fps : ~fp.ok THEN [acc EXCEPT !.lost = TRUE]
            ELSE IF r.ok THEN
               RecoverAll(rest \cup kids, recs,
                          [acc EXCEPT !.live = Put(@, q, [ext |-> r.st.ext, end |-> r.end,
                                                          epoch |-> r.st.epoch, next |-> r.st.next,
                                                          touched |-> r.touched,
                                                          fences |-> r.st.fences,
                                                          voids |-> r.st.voids,
                                                          lates |-> r.st.lates,
                                                          taking |-> LeaseEpoch(b) > r.st.epoch,
                                                          behind |-> Behind(b, r.ck)]),
                                      !.ck = @ \/ r.ck >= 0,
                                      !.pts = @ \cup {Kept(fp) : fp \in fps}])
            ELSE IF rec[b] = None \/ rec[b].deleted THEN RecoverAll(rest, recs, acc)
            ELSE IF rec[b].kind = "claim" /\ Unwritten(b)
                 THEN RecoverAll(rest, recs, [acc EXCEPT !.claims = @ \cup {b}])
            ELSE [acc EXCEPT !.lost = TRUE]

\* Whether a later pass of its cell took the cell's collector lease from pass i.
Fenced(i) == \E j \in (i + 1)..passes : gc[j].cell = gc[i].cell

\* The records of cell c's branches.
CellRecs(c) == [b \in Branches |-> IF CellOf[b] = c THEN rec[b] ELSE None]

\* A pass's start over cell c, taking the cell's collector lease, once the others are done (but
\* with "overlap", which lets passes of any cells, c's or others', run at once; a model without it
\* checks the cells' passes one after another, kept to a size TLC exhausts): the listing of c's
\* prefix (candidatesFirst), then its records. Without referencesRelisted, its references listed
\* now, and not again. The listing holds the cell's references and marks too: those of the
\* references it lists marked, retired or taken back, written again by a stale writer since, are
\* garbage, which it deletes (retirementsMarked).
GcStart(c) ==
    LET i == passes + 1 IN
    /\ "collect" \in Can /\ passes < MaxPasses
    /\ "overlap" \in Can \/ \A j \in DOMAIN gc : gc[j].pc = "idle"
    /\ gc' = [gc EXCEPT ![i] = [GcIdle EXCEPT !.pc = "records", !.cell = c, !.recs = CellRecs(c),
                                               !.objs = IF R("candidatesFirst") THEN Listing(c)
                                                        ELSE {},
                                               !.refs = IF R("referencesRelisted") THEN {}
                                                        ELSE RefsUnder(c),
                                               !.stale = RefsUnder(c) \cap marked]]
    /\ passes' = i
    /\ UNCHANGED <<bucketVars, hostVars, truthVars, gen, takes, places, crashes, mints>>

\* The records of the cell's branches pass i's listing has not read, which a listing finds.
Unread(i) == {b \in CellBranches(gc[i].cell) : gc[i].recs[b] = None /\ rec[b] # None}

\* The records listed again, until a listing finds none new (Collector::records): those new read
\* now, the others as they were read before.
GcRecords(i) ==
    /\ gc[i].pc = "records"
    /\ gc' = [gc EXCEPT ![i].recs = [b \in Branches |-> IF b \in Unread(i) THEN rec[b]
                                                       ELSE @[b]],
                        ![i].pc = IF Unread(i) = {} THEN "recover" ELSE "records"]
    /\ UNCHANGED <<bucketVars, hostVars, truthVars, countVars>>

\* The forks being made of points deleted given up on (collectorAbandons), then the live
\* branches recovered. Finding what one needs lost, a pass a later one took the lease from
\* stops (passesFencedStop): what it read may be older than what that pass, deleting since, read.
\* A point of another cell's is read now.
GcRecover(i) ==
    LET g == gc[i]
        pointRec(p) == IF CellOf[p] = g.cell THEN g.recs[p] ELSE rec[p]
        gone(p) == pointRec(p) = None \/ pointRec(p).deleted
        ab == IF R("collectorAbandons")
              THEN {b \in Branches : g.recs[b] # None /\ ~g.recs[b].deleted
                                     /\ g.recs[b].kind = "forking" /\ gone(g.recs[b].point)}
              ELSE {}
        rec1 == [b \in Branches |-> IF b \in ab /\ rec[b] # None /\ rec[b].kind = "forking"
                                       THEN [rec[b] EXCEPT !.deleted = TRUE] ELSE rec[b]]
        recs == [b \in Branches |-> IF b \in ab THEN rec1[b] ELSE g.recs[b]]
        queue == {<<b, recs[b].origin>> :
                    b \in {b \in Branches : recs[b] # None /\ ~recs[b].deleted}}
        acc == RecoverAll(queue, recs, [live |-> g.live, ck |-> FALSE, claims |-> {},
                                        lost |-> FALSE, pts |-> g.pts])
    IN /\ g.pc = "recover"
       /\ IF acc.lost THEN
             /\ errors' = IF R("passesFencedStop") /\ Fenced(i) THEN errors
                          ELSE errors \cup {"a pass lost what a live branch needs"}
             /\ gc' = [gc EXCEPT ![i] = Idle(g)]
             /\ rec' = rec1
          ELSE
             /\ rec' = [b \in Branches |->
                         IF b \in acc.claims THEN [rec1[b] EXCEPT !.deleted = TRUE] ELSE rec1[b]]
             /\ gc' = [gc EXCEPT ![i] = [g EXCEPT !.recs = recs, !.live = acc.live,
                                                 !.pts = acc.pts, !.ck = acc.ck,
                                                 !.pc = "relist"]]
             /\ UNCHANGED errors
       /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, refs, marked, retired, hostVars,
                      countVars>>
       /\ UNCHANGED <<commits, nextTag, acked, minted, twice, makers, doomed, taken, placed>>

\* After the states: the cell's records listed again (recordsRelisted), while what it recovered
\* has a checkpoint of its own, new ones recovered too; and its references listed
\* (referencesRelisted), each fork point's state kept. A reference written after this listing
\* is of a fork whose parent's state at its fork point this pass kept, or never listed: a
\* writable fork's child is recorded, with its reference first, before its parent's deletion
\* marks it, which the pass read live, its state naming the fork, or deleted, after; and a
\* point's clone writes its reference before it reads the point's record, so that one reading
\* it live made its reference before the point's deletion, before the pass read the point
\* deleted, or its own reference retired by its cell's pass, after.
GcRelist(i) ==
    LET g == gc[i]
        recs == IF g.ck /\ R("recordsRelisted") THEN CellRecs(g.cell) ELSE g.recs
        listed == IF R("referencesRelisted") THEN RefsUnder(g.cell) ELSE g.refs
        kept == RefPoints(listed)
    IN /\ g.pc = "relist"
       /\ IF \E b \in Branches : recs[b] # None /\ g.recs[b] = None THEN
             /\ gc' = [gc EXCEPT ![i].recs = recs, ![i].pc = "records"]
             /\ UNCHANGED errors
          ELSE IF kept.lost THEN
             /\ errors' = IF R("passesFencedStop") /\ Fenced(i) THEN errors
                          ELSE errors \cup {"a pass lost what a live branch needs"}
             /\ gc' = [gc EXCEPT ![i] = Idle(g)]
          ELSE
             /\ gc' = [gc EXCEPT ![i] = [g EXCEPT !.recs = recs, !.refs = listed,
                                                 !.pts = @ \cup kept.pts, !.pc = "finish"]]
             /\ UNCHANGED errors
       /\ UNCHANGED <<bucketVars, hostVars, countVars>>
       /\ UNCHANGED <<commits, nextTag, acked, minted, twice, makers, doomed, taken, placed>>

\* Whether segment s of a branch stays whatever covers it, by e, what a pass took of a recovery
\* of it (collector::pinned, TASK-388): every position a writer of it, taken over or taking it
\* over, may still create at. While a writer takes it over, every segment: it may have read the
\* branch before the last writer's segment landed and fences where it found the end (pinsTakings);
\* its fences, of every epoch (pinsFences; but the last's alone without pinsOldFences); its void
\* strays (pinsVoidStrays) and its late ones (pinsLateStrays).
Pinned(e, s) ==
    \/ R("pinsTakings") /\ e.taking
    \/ R("pinsFences") /\ s \in e.fences /\ (R("pinsOldFences") \/ s = Max(e.fences))
    \/ R("pinsVoidStrays") /\ s \in e.voids
    \/ R("pinsLateStrays") /\ s \in e.lates

\* Compaction, then deletion, unless a later pass of the cell took its lease: of what was listed,
\* what the live branches and the fork points kept do not need. A fork point's state is behind
\* (waysFromSeals): the child names what it inherited where it read it. Then the references of
\* the cell's deleted children retired, under their parents' cells, once the cell has no live
\* branch and no reference of its own (referencesHeld): no branch anywhere inherits from them,
\* a fork of one in the cell by its record, in another cell by its reference. Without the rule,
\* once the child is deleted.
GcFinish(i) ==
    LET g == gc[i]
        objs == IF R("candidatesFirst") THEN g.objs ELSE Listing(g.cell)
        L == g.live
        recs == g.recs
        touched == UNION {L[k].touched : k \in DOMAIN L} \cup UNION {p.touched : p \in g.pts}
        locs == UNION {{<<x, L[k].ext[x]>> : x \in DOMAIN L[k].ext} : k \in DOMAIN L}
                \cup UNION {{<<x, p.ext[x]>> : x \in DOMAIN p.ext} : p \in g.pts}
        \* The keys on the way to each extent named, from where a state says it is: the
        \* objects listed, and the moves listed of each, whose maps say it moved there.
        reach(l, d) == LET d0 == Depth(l[2]) IN
                       d >= d0 /\ \A e \in (d0 + 1)..d :
                           Key(l[2][2], l[2][3], e) \in objs /\ Names(l[2][2], l[2][3], e, l[1])
        chained == {l \in locs : l[2][1] # "own"}
        namedAt(k) == {l[1] : l \in {l \in chained : \E d \in 0..Hops :
                                         reach(l, d) /\ Key(l[2][2], l[2][3], d) = k}}
        \* And the moves on the ways there from where they were sealed, of the states whose
        \* writers may be behind them (waysFromSeals).
        behind == UNION {{L[k].ext[x] : x \in DOMAIN L[k].ext} :
                            k \in {k \in DOMAIN L : L[k].behind}}
                  \cup (IF R("waysFromSeals")
                        THEN UNION {{p.ext[x] : x \in DOMAIN p.ext} : p \in g.pts} ELSE {})
        hops == {Key(l[2][2], l[2][3], d + 1) :
                   <<l, d>> \in {<<l, d>> \in chained \X (0..(Hops - 1)) :
                                    reach(l, d) /\ Key(l[2][2], l[2][3], d + 1) \in objs}}
                \cup ({Key(l[2], l[3], d) : <<l, d>> \in {<<l, d>> \in behind \X (1..Hops) :
                                                            l[1] = "moves" /\ d <= Depth(l)}}
                     \cap objs)
        named == {Key(l[2][2], l[2][3], d) :
                    <<l, d>> \in {<<l, d>> \in chained \X (0..Hops) :
                                     reach(l, d) /\ Key(l[2][2], l[2][3], d) \in objs}}
                 \cup ({<<"own", l[2][2], l[2][3], l[2][4], l[1]>> :
                          l \in {l \in locs : l[2][1] = "own"}} \cap objs)
        \* By any recovery of b's.
        mayLandSeg(b, s) == \E k \in DOMAIN L : k[1] = b /\ s >= L[k].end
        \* Pinned by any recovery of b's.
        pinnedAt(b, s) == \E k \in DOMAIN L : k[1] = b /\ Pinned(L[k], s)
        mayLandOwn(b, e, s, x) ==
            \E k \in DOMAIN L :
               /\ k[1] = b
               /\ LET cur == IF R("orphansAfterSuccessor") THEN L[k].epoch ELSE LeaseEpoch(b)
                      ahead == IF R("extentKeysNameSegments") THEN s >= L[k].end
                               ELSE GE2(<<x[2], x[3]>>, L[k].next)
                  IN e >= cur /\ (e # L[k].epoch \/ ahead)
        \* Segments no recovery reads, which may not land; moves, but the last hop's
        \* (compactMoves): each into its moves, if it has none.
        next(k) == Key(k[2], k[3], Depth(k) + 1)
        compactable == {k \in named :
                          /\ k[1] # "own"
                          /\ IF k[1] = "seg"
                             THEN /\ "compact" \in Can /\ seg[k[2]][k[3]] # None
                                  /\ k \notin touched /\ ~mayLandSeg(k[2], k[3])
                             ELSE /\ "compactMoves" \in Can /\ Depth(k) < Hops
                                  /\ moves[k[2]][k[3]][Depth(k)] # None
                          /\ next(k) \notin objs /\ moves[k[2]][k[3]][Depth(k) + 1] = None}
        doomedKey(k) ==
            CASE k[1] = "ckpt" -> k \notin touched
              [] k[1] = "seg" -> ~mayLandSeg(k[2], k[3]) /\ k \notin touched
                                 /\ (k \notin named \/ next(k) \in objs)
                                 /\ ~pinnedAt(k[2], k[3])
              [] k[1] = "own" -> k \notin named /\ ~mayLandOwn(k[2], k[3], k[4], k[5])
              \* Moves go once no state's way goes through them, and they are moved in turn, or
              \* what they are of is gone.
              [] k[1] = "moves" ->
                   /\ k \notin hops
                   /\ IF k \in named THEN Depth(k) < Hops /\ next(k) \in objs
                      ELSE Key(k[2], k[3], Depth(k) - 1) \notin objs
              \* No record (recordsKept): a deleted root's stays too, as a deleted fork's does,
              \* which a slow writer of a parent naming its ID as a fork's child would make again
              \* (TASK-174). Without the rule, a deleted root's goes once no recovery read it.
              [] k[1] = "rec" -> /\ ~R("recordsKept")
                                 /\ recs[k[2]] # None /\ recs[k[2]].deleted
                                 /\ recs[k[2]].kind = "root" /\ k \notin touched
              [] k[1] = "lease" -> ~(recs[k[2]] # None /\ ~recs[k[2]].deleted)
        D == IF Fenced(i) THEN {} ELSE {k \in objs : doomedKey(k)}
        dead == /\ \A b \in CellBranches(g.cell) : recs[b] = None \/ recs[b].deleted
                /\ g.refs = {}
        \* Each deleted child's reference, of an origin in another cell, its key by the child's
        \* record: the origin it names, never listed, marked (retirementsMarked), whether it is
        \* there or not (collector's retired).
        deleted == {c \in CellBranches(g.cell) : /\ recs[c] # None /\ recs[c].deleted
                                                 /\ recs[c].origin # <<>>
                                                 /\ Cross(recs[c].origin[1], c)}
        retiring == IF Fenced(i) \/ (R("referencesHeld") /\ ~dead) THEN {}
                    ELSE {RefOf(recs[c].origin[1], recs[c].origin[2], c) : c \in deleted}
        \* The cell's own references listed marked: retired before, or taken back.
        stale == IF Fenced(i) THEN {} ELSE g.stale
    IN /\ g.pc = "finish"
       /\ \E C \in SUBSET compactable :
             moves' = [b \in Branches |-> [s \in Seqs |-> [d \in 1..Hops |->
                         IF Key(b, s, d) \in D THEN None
                         ELSE IF Key(b, s, d - 1) \in C THEN namedAt(Key(b, s, d - 1))
                         ELSE moves[b][s][d]]]]
       /\ seg' = [b \in Branches |-> [s \in Seqs |-> IF <<"seg", b, s>> \in D THEN None
                                                     ELSE seg[b][s]]]
       /\ own' = {o \in own : <<"own", o.b, o.e, o.s, o.x>> \notin D}
       /\ ckpt' = [b \in Branches |-> [s \in Seqs |-> IF <<"ckpt", b, s>> \in D THEN None
                                                      ELSE ckpt[b][s]]]
       /\ lease' = [b \in Branches |-> IF <<"lease", b>> \in D THEN None ELSE lease[b]]
       /\ rec' = [b \in Branches |-> IF <<"rec", b>> \in D THEN None ELSE rec[b]]
       /\ refs' = refs \ (retiring \cup stale)
       /\ retired' = retired \cup retiring
       /\ marked' = marked \cup Marks(retiring)
       /\ gc' = [gc EXCEPT ![i] = Idle(g)]
       /\ UNCHANGED <<inflight, hostVars, truthVars, countVars>>

GcNext ==
    \/ \E c \in Cells : GcStart(c)
    \/ \E i \in DOMAIN gc : GcRecords(i) \/ GcRecover(i) \/ GcRelist(i) \/ GcFinish(i)

-----------------------------------------------------------------------------

Init ==
    /\ seg = [b \in Branches |-> [s \in Seqs |-> None]]
    /\ own = {}
    /\ ckpt = [b \in Branches |-> [s \in Seqs |-> None]]
    /\ rec = [b \in Branches |-> IF b = Root THEN Record("root", <<>>, 0, <<"root", Root>>, 0)
                                 ELSE None]
    /\ lease = [b \in Branches |-> None]
    /\ moves = [b \in Branches |-> [s \in Seqs |-> [d \in 1..Hops |-> None]]]
    /\ inflight = {}
    /\ refs = {}
    /\ marked = {}
    /\ retired = {}
    /\ ses = [h \in Hosts |-> [b \in Branches |-> None]]
    /\ wal = [h \in Hosts |-> [b \in Branches |-> <<>>]]
    /\ tok = [h \in Hosts |-> [b \in Branches |-> 0]]
    /\ proc = [h \in Hosts |-> None]
    /\ gc = [i \in 1..MaxPasses |-> GcIdle]
    /\ commits = [t \in 1..MaxCommits |-> [branch |-> 0, parent |-> 0, seals |-> {},
                                             free |-> None]]
    /\ nextTag = 1
    /\ acked = [b \in Branches |-> {}]
    /\ minted = {}
    /\ twice = FALSE
    /\ makers = [b \in Branches |-> IF b = Root THEN {<<"root", Root>>} ELSE {}]
    /\ doomed = {}
    /\ taken = [b \in Branches |-> -1]
    /\ placed = [b \in Branches |-> 0]
    /\ errors = {}
    /\ gen = 0 /\ takes = 0 /\ passes = 0 /\ places = 0 /\ crashes = 0 /\ mints = 0
    /\ freed = {}

\* Every step, and what it freed: the places of segments there before it and gone after.
Next ==
    /\ WriterNext \/ HostNext \/ GcNext
    /\ freed' = freed \cup {<<b, s>> \in Branches \X Seqs : seg[b][s] # None /\ seg'[b][s] = None}

Spec == Init /\ [][Next]_vars

\* Hosts are alike.
HostPerms == Permutations(Hosts)

-----------------------------------------------------------------------------
(* What must hold. *)

\* The branches whose records say they live: roots, forks' children, points.
LiveRecs == {b \in Branches : LiveRec(b)}

\* Nothing a live branch needs is deleted: it recovers, every extent it names reads back as
\* itself, and a lease a writer took is there; and the writer whose lease it is reads every
\* extent its guest holds where its state acknowledged says it is.
NothingNeededGone ==
    /\ \A b \in LiveRecs :
          LET r == Recover(b, rec[b].origin) IN
          /\ r.ok
          /\ \A x \in DOMAIN r.st.ext : Readable(x, r.st.ext[x])
          /\ taken[b] >= 0 => lease[b] # None
    /\ \A h \in Hosts, b \in LiveRecs :
          (Ready(h, b) /\ Taking(ses[h][b], b)) =>
              \A x \in DOMAIN ses[h][b].st.ext \cap DOMAIN View(h, b).ext :
                  Readable(x, ses[h][b].st.ext[x])

\* Each live branch's history is a path of its own commits from the commit it started from,
\* every one applied once, holding what they made.
HistoriesOwn ==
    \A b \in LiveRecs :
        LET r == Recover(b, rec[b].origin) IN
        r.ok => /\ OwnStart(b, r.st.head) = rec[b].base
                /\ ~r.st.dup
                /\ r.st.hist = Anc(r.st.head)
                /\ DOMAIN r.st.ext = MadeBy(Path(r.st.head), {})

\* No acknowledged commit is lost: each is in its branch's history, and in the guest's of the
\* writer whose lease it is.
NoAckedLost ==
    /\ \A b \in LiveRecs :
          LET r == Recover(b, rec[b].origin) IN r.ok => acked[b] \subseteq Anc(r.st.head)
    /\ \A h \in Hosts, b \in LiveRecs :
          (Ready(h, b) /\ Taking(ses[h][b], b)) =>
              acked[b] \subseteq Anc(View(h, b).head)

\* No extent ID is minted twice.
NoIdTwice == ~twice

\* Nothing writes a point: no segment, checkpoint or writer's lease of one.
PointsUnwritten ==
    \A b \in Branches : rec[b] # None /\ rec[b].kind = "point" =>
        /\ \A s \in Seqs : seg[b][s] = None /\ ckpt[b][s] = None
        /\ lease[b] = None \/ lease[b].epoch = Final

\* No placement takes a branch back from a later one.
LeasesCurrent == \A b \in Branches : lease[b] # None /\ lease[b].epoch # Final =>
                                         lease[b].token >= taken[b]

\* A making told it made a branch loses it only to a deletion of it asked by who may.
DeletedOnlyByDeletes ==
    \A b \in Branches : rec[b] # None /\ rec[b].deleted /\ rec[b].maker \in Made =>
                            rec[b].maker \in doomed

\* One making, not deleted, is told it made each branch.
OneMaker == \A b \in Branches : Cardinality(makers[b] \ doomed) <= 1

\* Nothing failed that must not: a pass of the collector, a deletion, a WAL rebased, a store opened
\* (its log ending inside a commit), a WAL diverging from a branch whose last commit is its own.
NoErrors == errors = {}

\* The places h's writer of b may still create at, and acknowledge what it creates: its creates in
\* flight; once opened, the segments of its WAL it has yet to upload (stamped by its chain only
\* then, Opened); and, fencing, its fence's, while the lease is still its (a writer fencing that
\* finds it another's after stops, Opened). None once it stopped.
Creatable(h, b) ==
    LET s == ses[h][b] IN
    IF s = None \/ s.pc = "stopped" THEN {}
    ELSE {m.s : m \in {m \in inflight : m.h = h /\ m.b = b /\ m.g = s.gen}}
         \cup (IF s.pc \in {"ready", "record"}
                THEN {wal[h][b][i].s : i \in DOMAIN wal[h][b]} \ s.up ELSE {})
         \cup (IF s.pc = "fence" /\ (Leased(s, b) \/ ~R("epochSequences")) THEN {s.seq}
                ELSE {})

\* The safety of a writer's creates as its fencing (TASK-371), once (TASK-388): the first place a
\* writer of a live branch, taken over or taking it over, may still create at was never freed. A
\* writer creates its segments in order and acknowledges them in order, so it acknowledges
\* nothing past the first create that fails; where the collector freed a place, its create lands,
\* below a later writer's fence, or where an earlier writer's acknowledged segment was, and it
\* or its successor acknowledges what no history holds. The collector's pins keep it (Pinned).
CreatesNeverFreed ==
    \A h \in Hosts, b \in Branches :
        LET C == Creatable(h, b) IN
        C # {} /\ rec[b] # None /\ ~rec[b].deleted => <<b, Min(C)>> \notin freed

\* Every reference is named by its child's record, its origin the reference's fork point, which
\* the child's cell's pass retires it by once the child is deleted (TASK-395.10); or is of a fork
\* whose records' writers are yet to name or take it back: a live parent's state names the fork,
\* whose next writer or deletion records it again; a writer or a deletion of it records it now; a
\* clone of a point is being made; or it is marked, retired or taken back before, a stale writer's,
\* which its parent's cell's next pass deletes (retirementsMarked). Any other no pass retires, for
\* ever.
RefPending(r) ==
    LET at(F) == \E f \in F : f.child = r.c /\ f.at = r.at IN
    \/ /\ rec[r.p] # None /\ ~rec[r.p].deleted
       /\ LET st == Recover(r.p, rec[r.p].origin) IN st.ok /\ at(st.st.forks)
    \/ \E h \in Hosts : ses[h][r.p] # None /\ ses[h][r.p].pc = "record" /\ at(ses[h][r.p].st.forks)
    \/ \E h \in Hosts : /\ proc[h] # None /\ proc[h].op = "delete" /\ proc[h].b = r.p
                       /\ proc[h].pc \in {"refs", "write", "record"} /\ at(proc[h].fs)
    \/ \E h \in Hosts : /\ proc[h] # None /\ proc[h].op = "restore" /\ proc[h].c = r.c
                       /\ proc[h].og = <<r.p, r.at>>
    \/ r \in marked
ReferencesNamed ==
    \A r \in refs : (rec[r.c] # None /\ rec[r.c].origin = <<r.p, r.at>>) \/ RefPending(r)

\* A reference standing after its retirement or take-back is marked (TASK-395.13, TASK-395.14):
\* a stale writer wrote it again, its read of the child's record from before, none there, the
\* child deleted or its ID taken since. No record will name it, nor will its child's cell's
\* passes retire it again, skipping the dead cell, whose listing it changes not, nor will a
\* recorder take it back, the fork recorded; its parent's cell's listing it changes, and that
\* cell's next pass deletes it, by its mark.
ReferencesRetiredMarked == \A r \in refs \cap retired : r \in marked

\* Every live branch forked from another cell, and every point's clone, has its reference under
\* its origin's cell, naming its fork point (TASK-395): written before its record, retired only
\* once it is deleted.
ReferencesStand ==
    \A b \in LiveRecs :
        rec[b].origin # <<>> /\ (Cross(rec[b].origin[1], b) \/ rec[b].point # 0) =>
            RefOf(rec[b].origin[1], rec[b].origin[2], b) \in refs
=============================================================================
