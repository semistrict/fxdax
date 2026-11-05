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
(* own, an extent freed, the branch's next sequence number, and a fork;    *)
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
(* stamped by the chain; an upload creates its extent of its own, then     *)
(* puts it in flight; an acknowledgement takes the WAL's prefix created,   *)
(* after a read of the lease; then the children of its forks get their     *)
(* records. A host crashes keeping its WALs or losing them. Placements     *)
(* give hosts fencing tokens. Branches are deleted by their placements     *)
(* (the lease ended, then the record, the children recorded), failed forks *)
(* by their hosts (delete_fork), points forked by restores (the child's    *)
(* record, the point read, the record made); other branches take forks'    *)
(* children's IDs. One collector runs passes: the listing and the records, *)
(* the live branches recovered, the records again, then compaction and     *)
(* deletion.                                                               *)
(*                                                                         *)
(* Each rule is a member of Rules; specs/bucket/mutations removes one      *)
(* each, and must find the violation it prevents (each mutation's model    *)
(* says which, and how). The first 26 are the fields of extent_remote's    *)
(* `Rules` (crates/extent_remote/src/lib.rs); then one the code holds      *)
(* with no field (TASK-178), and one it lacks (TASK-183); the last two     *)
(* doc-14's, which TASK-165.5 adds:                                        *)
(*                                                                         *)
(* - acksAfterLease (NoAcksAfterLease): a writer acknowledges a segment    *)
(*   only once a read of its lease after the creates shows it still its.   *)
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
(*   what it inherited lost, if one has come since (TASK-177).             *)
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
(*   again after reading states with checkpoints of their own.             *)
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
(*   (TASK-173).                                                           *)
(* - deletionsDoneByOthers (NoDeletionsDoneByOthers): a deletion whose     *)
(*   recovery of the branch finds what it needs lost, the record deleted   *)
(*   since, is done (TASK-178; branch::delete, with no field of Rules).    *)
(* - deletionsFence (NoDeletionsFence): a deletion's recovery creates an   *)
(*   empty segment at the end it found, as a writer taking over does, so   *)
(*   that every deletion of the branch finds the same forks. Not in the    *)
(*   code (TASK-183): a second deletion, recovering past a fork's segment  *)
(*   landed after the first marked the branch deleted, records the child   *)
(*   of a fork never acknowledged, whose origin a pass, reading the        *)
(*   branch deleted and no record of the child, deletes.                   *)
(* - walFollows (NoWalFollows): a WAL is put after the branch only if the  *)
(*   branch's last commit is one of the WAL's or the commit the WAL        *)
(*   started from.                                                         *)
(* - walAfterFence (NoWalAfterFence): that last commit is the branch's     *)
(*   after the fence, which may have applied the WAL's own segments,       *)
(*   landed since recovery.                                                *)
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
(* - No record is dropped: a deleted root's stays too, as a deleted fork's *)
(*   does (TASK-174). What TASK-177 found lost, a fork's deleted parent's  *)
(*   record, is here that parent's segments to the fork, which the         *)
(*   collector lets go of for the same reason: the fork's checkpoint.      *)
(*                                                                         *)
(* Steps are what the code does between two requests, or merged: a        *)
(* writer's taking reads the record, takes the lease and reads the record  *)
(* again in one step, the first read of it from before the branch's        *)
(* deletion if "staleTake" is in Can; the writers' and the collector's     *)
(* recoveries read the bucket at once, the deletions' in two steps, the    *)
(* checkpoints listed, then the rest. A checkpoint's create lands after    *)
(* its writer took its moves, in a step of its own, while its writer's     *)
(* session lasts. Passes overlap if "overlap" is in Can, a later one       *)
(* taking the collector's lease; a pass reads the records once each, as    *)
(* listings find them, until one finds none new, then once again each      *)
(* after the states (recordsRelisted).                                     *)
(*                                                                         *)
(* Can bounds what happens: each kind of step needs its name in it, so     *)
(* that a model checks some at a time and a mutation only what it needs.   *)
(* Moves go two hops at most: a segment's, then those moves'. Not          *)
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
    Rules           \* the protocol's rules in force (all, but in a mutation)

AllRules == {"acksAfterLease", "fence", "continuity", "epochSequences", "recordsAfterAck",
             "candidatesFirst", "newestCheckpoint", "forksFromStates", "orphansAfterSuccessor",
             "tokens", "deleteTokens", "footers", "forkClaimsIds", "forkClaimsLanded",
             "pointsTakeNoWriter", "forksCheckPoints", "forksPending", "collectorAbandons",
             "recordsRelisted", "recordsOwned", "extentKeysNameSegments", "waysFromSeals",
             "movesTakenFirst", "leasesByTakings", "passesFencedStop", "recoveriesByOrigins",
             "deletionsDoneByOthers", "deletionsFence", "walFollows", "walAfterFence"}

AllCan == {"commit", "mint", "seg", "own", "free", "fork", "point", "restore", "unfork",
           "contest", "delete", "deletePoints", "place", "checkpoint", "collect", "compact",
           "compactMoves", "crash", "disk", "staleTake", "overlap"}

ASSUME Rules \subseteq AllRules
ASSUME Can \subseteq AllCan
ASSUME Root \in Branches

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
    \* The hosts.
    ses,        \* ses[h][b]: h's writer of b, or None
    wal,        \* wal[h][b]: h's WAL of b, a sequence of [s, o]: kept by a crash, or lost
    tok,        \* tok[h][b]: the fencing token of h's placement of b
    proc,       \* proc[h]: h's deletion, failed fork's deletion or restore, or None
    \* The collector.
    gc,         \* gc[i]: pass i, whose lease epoch is i: the collector's lease is the last's
    \* What happened.
    commits,    \* commits[t]: [branch, parent, seal, free]: its branch, the commit before it, what
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
    \* Bounds.
    gen, takes, passes, places, crashes, mints

bucketVars == <<seg, own, ckpt, rec, lease, moves, inflight>>
hostVars == <<ses, wal, tok, proc>>
truthVars == <<commits, nextTag, acked, minted, twice, makers, doomed, taken, placed, errors>>
countVars == <<gen, takes, passes, places, crashes, mints>>
vars == <<bucketVars, hostVars, gc, truthVars, countVars>>

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
\* next sequence number, its forks, the highest epoch applied, the last segment read, and,
\* not in the bucket's bytes, the commits applied and whether one was applied twice.
Fresh == [ext |-> Nothing, head |-> 0, last |-> <<>>, next |-> <<0, 0>>, forks |-> {},
          epoch |-> 0, through |-> -1, hist |-> {}, dup |-> FALSE]

\* A fork's child's state before its segments: what it inherits.
Inherit(st) == [Fresh EXCEPT !.ext = st.ext, !.head = st.head, !.hist = st.hist]

\* Whether a reader applies segment o after st: it follows the last applied (continuity).
Follows(st, o) == o.after = st.last \/ ~R("continuity")

\* st, segment s of b, whose object is o, applied: unless it is void.
Apply(st, b, s, o) ==
    IF ~Follows(st, o) THEN [st EXCEPT !.through = s]
    ELSE LET e1 == IF o.ext # None THEN Put(st.ext, o.ext, <<"seg", b, s>>) ELSE st.ext
             e2 == IF o.own # None THEN Put(e1, o.own, <<"own", b, o.epoch, s>>) ELSE e1
             e3 == IF o.free # None /\ o.free \in DOMAIN e2 THEN Drop(e2, o.free) ELSE e2
         IN [st EXCEPT !.ext = e3,
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

\* Whether the object d hops from segment s of b names x: the segment's footer, or the moves' map.
Names(b, s, d, x) ==
    IF d = 0 THEN seg[b][s] # None /\ seg[b][s].ext = x
    ELSE moves[b][s][d] # None /\ x \in moves[b][s][d]

\* Whether a reader of extent x at loc reads x's bytes: where the footer of the object at its key
\* says it is, or, that object gone or naming it not, where the map of the moves of it says, one
\* hop at a time (fetch's read); without footers, a segment where loc says, whatever it holds.
Readable(x, loc) ==
    IF loc[1] = "own" THEN [b |-> loc[2], e |-> loc[3], s |-> loc[4], x |-> x] \in own
    ELSE LET b == loc[2]
             s == loc[3]
             d == Depth(loc)
         IN IF d = 0 /\ seg[b][s] # None /\ ~R("footers") THEN seg[b][s].ext = x
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
             sealed == IF c.seal = None THEN held ELSE held \cup {c.seal}
         IN MadeBy(Tail(p), IF c.free = None THEN sealed ELSE sealed \ {c.free})

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

\* Whether a writer whose state is st has forks acknowledged whose children it hasn't recorded.
Unrecorded(st, recorded) == \E f \in st.forks : f.child \notin recorded

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
                 up |-> {}, done |-> {}, open |-> {}, recorded |-> {}, ckd |-> -1, ckp |-> None]
       IN /\ lease' = [lease EXCEPT ![b] = [epoch |-> e, token |-> tok[h][b], holder |-> gen + 1]]
          /\ ses' = [ses EXCEPT ![h][b] = IF rec[b].deleted THEN [s EXCEPT !.pc = "stopped"]
                                          ELSE Started(s, b)]
    /\ taken' = [taken EXCEPT ![b] = IF tok[h][b] > @ THEN tok[h][b] ELSE @]
    /\ gen' = gen + 1
    /\ takes' = takes + 1
    /\ UNCHANGED <<seg, own, ckpt, rec, moves, inflight, wal, tok, proc, gc>>
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

\* Whether the fence of a writer whose session is s goes on: it has not created its fence, or
\* not filled the gaps to the last segment listed.
Fencing(s) == ~s.fenced \/ s.seq <= s.top

\* h's writer of b, whose session s fenced the branch, opened: the lease read again
\* (epochSequences), and the chain, minting from the epoch's range. Then its WAL put after the
\* branch's end (wal::rebase), by commits: the WAL's commits to the branch's last, if it is one
\* of them, acknowledged; the rest restamped after the fence, if the branch's last commit is one
\* of the WAL's or the commit the WAL started from (walFollows), as the branch is after the fence
\* (walAfterFence); otherwise the WAL diverged, and is dropped. The commit the WAL started from,
\* its first segment's commit's parent, the WAL keeps.
Opened(h, b, s) ==
    LET floor == IF R("epochSequences") THEN <<s.epoch, 0>> ELSE <<0, 0>>
        next == Max2(s.st.next, floor)
        ws == wal[h][b]
        head == IF R("walAfterFence") THEN s.st.head ELSE s.pre
        base == IF ws = <<>> THEN 0 ELSE commits[ws[1].o.commit].parent
        at == {i \in DOMAIN ws : ws[i].o.commit = head}
        k == IF at = {} THEN 0 ELSE Max(at)
        A == SubSeq(ws, 1, k)
        Rs == SubSeq(ws, k + 1, Len(ws))
        keep == Rs # <<>> /\ (at # {} \/ head = base \/ ~R("walFollows"))
        new == IF keep THEN Restamp(Rs, s.seq, s.epoch, s.st.last, next) ELSE <<>>
    IN IF (R("epochSequences") /\ ~Leased(s, b)) \/ s.seq + Len(new) > Segs
       THEN /\ ses' = [ses EXCEPT ![h][b] = [s EXCEPT !.pc = "stopped"]]
            /\ UNCHANGED <<wal, acked>>
       ELSE /\ wal' = [wal EXCEPT ![h][b] = new]
            /\ acked' = [acked EXCEPT ![b] = @ \cup CommitsOf(A)]
            /\ ses' = [ses EXCEPT ![h][b] =
                         [s EXCEPT !.pc = IF Unrecorded(s.st, {}) THEN "record" ELSE "ready",
                                   !.seq = s.seq + Len(new),
                                   !.last = IF new = <<>> THEN s.st.last
                                            ELSE <<new[Len(new)].s, s.epoch>>,
                                   !.next = IF new = <<>> THEN next ELSE new[Len(new)].o.next]]

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
             /\ UNCHANGED <<wal, acked>>
          ELSE IF Fencing(s2) THEN
             /\ ses' = [ses EXCEPT ![h][b] = s2]
             /\ UNCHANGED <<wal, acked>>
          ELSE Opened(h, b, s2)
       /\ UNCHANGED <<bucketVars, tok, proc, gc, countVars>>
       /\ UNCHANGED <<commits, nextTag, minted, twice, makers, doomed, taken, placed, errors>>

\* The fence: an empty segment of the writer's at the branch's end, what landed there first
\* applied before it, and the gaps to the last listed filled; then the writer opened.
Fence(h, b) ==
    LET s == ses[h][b]
        q == s.seq
        f == [epoch |-> s.epoch, after |-> s.st.last, commit |-> 0, ext |-> None, own |-> None,
              free |-> None, next |-> s.st.next, fork |-> None]
        o == IF seg[b][q] = None THEN f ELSE seg[b][q]
        s2 == [s EXCEPT !.fenced = (@ \/ seg[b][q] = None), !.st = Apply(s.st, b, q, o),
                        !.seq = q + 1]
    IN /\ s # None /\ s.pc = "fence" /\ q < Segs
       /\ IF seg[b][q] # None /\ seg[b][q].epoch > s.epoch THEN
             /\ Stopped(h, b)
             /\ UNCHANGED <<seg, wal, acked>>
          ELSE /\ seg' = [seg EXCEPT ![b][q] = o]
               /\ IF Fencing(s2) THEN ses' = [ses EXCEPT ![h][b] = s2] /\ UNCHANGED <<wal, acked>>
                  ELSE Opened(h, b, s2)
       /\ UNCHANGED <<own, ckpt, rec, lease, moves, inflight, tok, proc, gc, countVars>>
       /\ UNCHANGED <<commits, nextTag, minted, twice, makers, doomed, taken, placed, errors>>

\* The children of the forks acknowledged get their records (record_forks), one at a time.
RecordForks(h, b) ==
    LET s == ses[h][b]
        F == {f \in s.st.forks : f.child \notin s.recorded}
        f == FirstFork(F)
        r == RecordFork(rec, b, f)
    IN /\ s # None /\ s.pc = "record"
       /\ IF F = {} THEN
             /\ ses' = [ses EXCEPT ![h][b].pc = "ready"]
             /\ UNCHANGED <<rec, makers>>
          ELSE
             /\ rec' = [rec EXCEPT ![f.child] = r.rec]
             /\ makers' = IF r.made THEN [makers EXCEPT ![f.child] = @ \cup {f.owner}]
                          ELSE makers
             /\ ses' = [ses EXCEPT ![h][b].recorded = @ \cup {f.child}]
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
\* free an extent its guest holds, or fork the branch (a point, or not) to a child with no record.
Ops(h, b) ==
    LET s == ses[h][b]
        v == View(h, b)
        op(k, x, c, p) == [k |-> k, x |-> x, c |-> c, point |-> p]
        kinds == (IF "fork" \in Can THEN {FALSE} ELSE {})
                 \cup (IF "point" \in Can THEN {TRUE} ELSE {})
    IN (IF "commit" \in Can THEN {op("plain", None, 0, FALSE)} ELSE {})
       \cup {op("seg", x, 0, FALSE) : x \in IF "seg" \in Can THEN s.open ELSE {}}
       \cup {op("own", x, 0, FALSE) : x \in IF "own" \in Can THEN s.open ELSE {}}
       \cup {op("free", x, 0, FALSE) : x \in IF "free" \in Can THEN DOMAIN v.ext ELSE {}}
       \cup {op("fork", None, c, p) : c \in {c \in Branches \ {b, Root} : rec[c] = None},
                                      p \in kinds}

\* A commit: a segment of the WAL, stamped by the chain.
Commit(h, b) ==
    LET s == ses[h][b]
        t == nextTag
    IN /\ Ready(h, b) /\ t <= MaxCommits /\ s.seq < Segs
       /\ \E op \in Ops(h, b) :
            LET f == [child |-> op.c, owner |-> <<"fork", t>>, point |-> op.point]
                o == [epoch |-> s.epoch, after |-> s.last, commit |-> t,
                      ext |-> IF op.k = "seg" THEN op.x ELSE None,
                      own |-> IF op.k = "own" THEN op.x ELSE None,
                      free |-> IF op.k = "free" THEN op.x ELSE None,
                      next |-> s.next,
                      fork |-> IF op.k = "fork" THEN f ELSE None]
            IN /\ wal' = [wal EXCEPT ![h][b] = Append(@, [s |-> s.seq, o |-> o])]
               /\ ses' = [ses EXCEPT ![h][b] =
                            [s EXCEPT !.seq = @ + 1, !.last = <<s.seq, s.epoch>>,
                                      !.open = @ \ {op.x}]]
               /\ commits' = [commits EXCEPT ![t] = [branch |-> b, parent |-> View(h, b).head,
                                                      seal |-> IF op.k \in {"seg", "own"}
                                                               THEN op.x ELSE None,
                                                      free |-> o.free]]
               \* Without recordsAfterAck, the child's record at once.
               /\ rec' = IF op.k = "fork" /\ ~R("recordsAfterAck")
                         THEN [rec EXCEPT ![op.c] = Record(IF op.point THEN "point" ELSE "fork",
                                                           <<b, s.seq>>, 0, f.owner, t)]
                         ELSE rec
       /\ nextTag' = t + 1
       /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, tok, proc, gc>>
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
       /\ UNCHANGED <<seg, ckpt, rec, lease, moves, wal, tok, proc, gc, truthVars, countVars>>

\* A create lands, whenever, though its writer has gone: where nothing is, or its own is; else
\* its writer, if it still is, stops (fenced, or displaced).
Land(m) ==
    LET s == ses[m.h][m.b]
        mine == s # None /\ s.gen = m.g /\ s.pc # "stopped"
    IN /\ inflight' = inflight \ {m}
       /\ IF seg[m.b][m.s] = None \/ seg[m.b][m.s] = m.o THEN
             /\ seg' = [seg EXCEPT ![m.b][m.s] = m.o]
             /\ ses' = IF mine THEN [ses EXCEPT ![m.h][m.b].done = @ \cup {m.s}] ELSE ses
          ELSE
             /\ ses' = IF mine THEN [ses EXCEPT ![m.h][m.b].pc = "stopped"] ELSE ses
             /\ UNCHANGED seg
       /\ UNCHANGED <<own, ckpt, rec, lease, moves, wal, tok, proc, gc, truthVars, countVars>>

\* The acknowledgement of the WAL's prefix created, once a read of the lease shows it the
\* writer's (acksAfterLease): its commits told durable, its forks' children to be recorded.
Ack(h, b) ==
    LET s == ses[h][b]
        ws == wal[h][b]
        n == Max({k \in 0..Len(ws) : \A i \in 1..k : ws[i].s \in s.done})
        P == SubSeq(ws, 1, n)
    IN /\ Ready(h, b) /\ n > 0
       /\ IF R("acksAfterLease") /\ ~Leased(s, b) THEN
             Stopped(h, b) /\ UNCHANGED <<wal, acked>>
          ELSE /\ wal' = [wal EXCEPT ![h][b] = SubSeq(ws, n + 1, Len(ws))]
               /\ ses' = [ses EXCEPT ![h][b] =
                            [s EXCEPT !.st = ApplyAll(s.st, b, P),
                                      !.pc = IF Unrecorded(ApplyAll(s.st, b, P), s.recorded)
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
    /\ UNCHANGED <<seg, own, rec, lease, moves, inflight, wal, tok, proc, gc, truthVars, countVars>>

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

\* A host crashes: its writers and its other work stop; its WALs stay, or the disk is lost.
Crash(h) ==
    /\ "crash" \in Can /\ crashes < MaxCrashes
    /\ \E lose \in IF "disk" \in Can THEN {FALSE, TRUE} ELSE {FALSE} :
          wal' = [wal EXCEPT ![h] = [b \in Branches |-> IF lose THEN <<>> ELSE @[b]]]
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
    /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, hostVars, gc, countVars>>
    /\ UNCHANGED <<commits, nextTag, acked, minted, twice, doomed, taken, placed, errors>>

-----------------------------------------------------------------------------
(* Deletions, failed forks' deletions, and forks of points: a host's work, one at a time. *)

\* A deletion of b, asked on h: the lease ended first. Asked by b's last placement, or one as
\* late, it may delete b's making.
StartDelete(h, b) ==
    /\ proc[h] = None
    /\ rec[b] # None /\ ~rec[b].deleted
    /\ "delete" \in Can \/ ("deletePoints" \in Can /\ rec[b].kind = "point")
    /\ proc' = [proc EXCEPT ![h] = [op |-> "delete", b |-> b, pc |-> "end", c |-> 0, p |-> 0,
                                    at |-> 0, o |-> <<>>, live |-> FALSE, tk |-> tok[h][b],
                                    ck |-> -1]]
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
                                            tk |-> tok[h][w.o.fork.child], ck |-> -1]]
            /\ doomed' = doomed \cup {w.o.fork.owner}
       /\ UNCHANGED <<bucketVars, ses, wal, tok, gc, countVars>>
       /\ UNCHANGED <<commits, nextTag, acked, minted, twice, makers, taken, placed, errors>>

\* A fork of point P to C (fork_point): C's record, saying it is being made (forksPending), of
\* the origin the point names. From a host's copy of P, stale perhaps.
StartRestore(h, c, p) ==
    /\ "restore" \in Can /\ proc[h] = None
    /\ rec[p] # None /\ rec[p].kind = "point" /\ rec[c] = None /\ c \notin {Root, p}
    /\ rec' = [rec EXCEPT ![c] = Record(IF R("forksPending") THEN "forking" ELSE "fork",
                                        rec[p].origin, p, <<"restore", c, h>>, rec[p].base)]
    /\ proc' = [proc EXCEPT ![h] = [op |-> "restore", b |-> c, pc |-> "check", c |-> c, p |-> p,
                                    at |-> 0, o |-> <<"restore", c, h>>, live |-> FALSE,
                                    tk |-> tok[h][c], ck |-> -1]]
    /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, ses, wal, tok, gc, truthVars,
                   countVars>>

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
            [] q.op = "delete" /\ q.pc = "recover" ->
                 \* Its recovery, starting again with the checkpoints listed again; then an
                 \* empty segment of the final epoch created where it found the first missing
                 \* (deletionsFence; a point, which no writer has, needs none), that read and the
                 \* create one step; its forks' children recorded, and its record deleted. Finding
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
                       LET all == RecordAll(rec, makers, b, got.st.forks) IN
                       /\ rec' = [all.rec EXCEPT ![b].deleted = TRUE]
                       /\ makers' = all.makers
                       /\ Done(h)
                       /\ UNCHANGED errors
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
                                    next |-> got.st.next, fork |-> None]]
                           ELSE seg
                 /\ UNCHANGED <<own, ckpt, lease, moves, inflight>>
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
            [] q.op = "restore" /\ q.pc = "check" ->
                 \* The point's record, read after the child's (forksCheckPoints): deleted, the
                 \* fork fails, and its host deletes the child.
                 /\ IF R("forksCheckPoints") /\ (rec[q.p] = None \/ rec[q.p].deleted)
                    THEN Deleting(h)
                    ELSE proc' = [proc EXCEPT ![h].pc = "make"]
                 /\ UNCHANGED <<seg, own, ckpt, rec, lease, moves, inflight, makers, errors>>
            [] q.op = "restore" /\ q.pc = "make" ->
                 \* No longer being made, with a condition: unless deleted, given up on.
                 /\ IF rec[b] # None /\ ~rec[b].deleted THEN
                       /\ rec' = [rec EXCEPT ![b].kind = IF @ = "forking" THEN "fork" ELSE @]
                       /\ makers' = [makers EXCEPT ![b] = @ \cup {q.o}]
                    ELSE UNCHANGED <<rec, makers>>
                 /\ Done(h)
                 /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, errors>>
       /\ doomed' = IF q.op = "restore" /\ q.pc = "check" /\ R("forksCheckPoints")
                       /\ (rec[q.p] = None \/ rec[q.p].deleted)
                    THEN doomed \cup {q.o} ELSE doomed
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
GcIdle == [pc |-> "idle", objs |-> {}, recs |-> NoRecs, live |-> Nothing, ck |-> FALSE]

\* Every object's key.
Listing ==
    {k \in {<<"seg", b, s>> : b \in Branches, s \in Seqs} : seg[k[2]][k[3]] # None}
    \cup {<<"own", o.b, o.e, o.s, o.x>> : o \in own}
    \cup {k \in {<<"ckpt", b, s>> : b \in Branches, s \in Seqs} : ckpt[k[2]][k[3]] # None}
    \cup {<<"rec", b>> : b \in {b \in Branches : rec[b] # None}}
    \cup {<<"lease", b>> : b \in {b \in Branches : lease[b] # None}}
    \cup {k \in {<<"moves", b, s, d>> : b \in Branches, s \in Seqs, d \in 1..Hops} :
            moves[k[2]][k[3]][k[4]] # None}

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
\* another origin, a restore's that took the ID (TASK-173). A failure that loses nothing
\* (deleted since, a claim no writer has) is skipped, the claim given up on; any other loses the
\* pass.
RecoverAll(queue, recs, acc) ==
    IF queue = {} \/ acc.lost THEN acc
    ELSE LET q == CHOOSE q \in queue : TRUE
             b == q[1]
             rest == queue \ {q}
             r == Recover(b, q[2])
             kids == {<<f.child, <<b, f.at>>>> :
                        f \in {f \in r.st.forks : recs[f.child] = None /\ R("forksFromStates")}}
         IN IF Recovered(acc, q) THEN RecoverAll(rest, recs, acc)
            ELSE IF r.ok THEN
               RecoverAll(rest \cup kids, recs,
                          [acc EXCEPT !.live = Put(@, q, [ext |-> r.st.ext, end |-> r.end,
                                                          epoch |-> r.st.epoch, next |-> r.st.next,
                                                          touched |-> r.touched,
                                                          behind |-> Behind(b, r.ck)]),
                                      !.ck = @ \/ r.ck >= 0])
            ELSE IF rec[b] = None \/ rec[b].deleted THEN RecoverAll(rest, recs, acc)
            ELSE IF rec[b].kind = "claim" /\ Unwritten(b)
                 THEN RecoverAll(rest, recs, [acc EXCEPT !.claims = @ \cup {b}])
            ELSE [acc EXCEPT !.lost = TRUE]

\* Whether a later pass took the collector's lease from pass i.
Fenced(i) == passes # i

\* A pass's start, taking the collector's lease, once the others are done (but with "overlap"):
\* the listing (candidatesFirst), then the records.
GcStart ==
    LET i == passes + 1 IN
    /\ "collect" \in Can /\ passes < MaxPasses
    /\ "overlap" \in Can \/ \A j \in DOMAIN gc : gc[j].pc = "idle"
    /\ gc' = [gc EXCEPT ![i] = [GcIdle EXCEPT !.pc = "records", !.recs = rec,
                                               !.objs = IF R("candidatesFirst") THEN Listing
                                                        ELSE {}]]
    /\ passes' = i
    /\ UNCHANGED <<bucketVars, hostVars, truthVars, gen, takes, places, crashes, mints>>

\* The records of the branches pass i's listing has not read, which a listing finds.
Unread(i) == {b \in Branches : gc[i].recs[b] = None /\ rec[b] # None}

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
GcRecover(i) ==
    LET g == gc[i]
        gone(p) == g.recs[p] = None \/ g.recs[p].deleted
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
                                        lost |-> FALSE])
    IN /\ g.pc = "recover"
       /\ IF acc.lost THEN
             /\ errors' = IF R("passesFencedStop") /\ Fenced(i) THEN errors
                          ELSE errors \cup {"a pass lost what a live branch needs"}
             /\ gc' = [gc EXCEPT ![i] = GcIdle]
             /\ rec' = rec1
          ELSE
             /\ rec' = [b \in Branches |->
                         IF b \in acc.claims THEN [rec1[b] EXCEPT !.deleted = TRUE] ELSE rec1[b]]
             /\ gc' = [gc EXCEPT ![i] = [g EXCEPT !.recs = recs, !.live = acc.live,
                                                 !.pc = IF acc.ck /\ R("recordsRelisted")
                                                        THEN "relist" ELSE "finish"]]
             /\ UNCHANGED errors
       /\ UNCHANGED <<seg, own, ckpt, lease, moves, inflight, hostVars, countVars>>
       /\ UNCHANGED <<commits, nextTag, acked, minted, twice, makers, doomed, taken, placed>>

\* The records listed again, after the states (recordsRelisted): new ones recovered too.
GcRelist(i) ==
    /\ gc[i].pc = "relist"
    /\ gc' = [gc EXCEPT ![i].recs = rec,
                        ![i].pc = IF \E b \in Branches : rec[b] # None /\ gc[i].recs[b] = None
                                  THEN "records" ELSE "finish"]
    /\ UNCHANGED <<bucketVars, hostVars, truthVars, countVars>>

\* Compaction, then deletion, unless a later pass took the lease: of what was listed, what the
\* live branches do not need.
GcFinish(i) ==
    LET g == gc[i]
        objs == IF R("candidatesFirst") THEN g.objs ELSE Listing
        L == g.live
        recs == g.recs
        touched == UNION {L[k].touched : k \in DOMAIN L}
        locs == UNION {{<<x, L[k].ext[x]>> : x \in DOMAIN L[k].ext} : k \in DOMAIN L}
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
              [] k[1] = "own" -> k \notin named /\ ~mayLandOwn(k[2], k[3], k[4], k[5])
              \* Moves go once no state's way goes through them, and they are moved in turn, or
              \* what they are of is gone.
              [] k[1] = "moves" ->
                   /\ k \notin hops
                   /\ IF k \in named THEN Depth(k) < Hops /\ next(k) \in objs
                      ELSE Key(k[2], k[3], Depth(k) - 1) \notin objs
              \* No record: a deleted root's stays too, as a deleted fork's does, which a slow
              \* writer of a parent naming its ID as a fork's child would make again
              \* (extent_remote's Record::may_drop drops it, TASK-174).
              [] k[1] = "rec" -> FALSE
              [] k[1] = "lease" -> ~(recs[k[2]] # None /\ ~recs[k[2]].deleted)
        D == IF Fenced(i) THEN {} ELSE {k \in objs : doomedKey(k)}
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
       /\ gc' = [gc EXCEPT ![i] = GcIdle]
       /\ UNCHANGED <<rec, inflight, hostVars, truthVars, countVars>>

GcNext ==
    \/ GcStart
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
    /\ ses = [h \in Hosts |-> [b \in Branches |-> None]]
    /\ wal = [h \in Hosts |-> [b \in Branches |-> <<>>]]
    /\ tok = [h \in Hosts |-> [b \in Branches |-> 0]]
    /\ proc = [h \in Hosts |-> None]
    /\ gc = [i \in 1..MaxPasses |-> GcIdle]
    /\ commits = [t \in 1..MaxCommits |-> [branch |-> 0, parent |-> 0, seal |-> None,
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

Next == WriterNext \/ HostNext \/ GcNext

Spec == Init /\ [][Next]_vars

\* Hosts are alike.
HostPerms == Permutations(Hosts)

-----------------------------------------------------------------------------
(* What must hold. *)

\* The branches whose records say they live: roots, forks' children, points.
LiveRecs == {b \in Branches : rec[b] # None /\ ~rec[b].deleted
                              /\ rec[b].kind \in {"root", "fork", "point"}}

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

\* Nothing failed that must not: a pass of the collector, a deletion, a WAL rebased.
NoErrors == errors = {}
=============================================================================
