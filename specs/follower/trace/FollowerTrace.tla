---------------------------- MODULE FollowerTrace ----------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* Trace validation (backlog doc-8 testing 0 and 3, TASK-42.1): a run of  *)
(* crates/follower's Shuttle tests, the real protocol over the spec's     *)
(* tiny filesystem, traced step by step (crates/follower/src/            *)
(* shuttle_trace.rs, FollowerTraceData), is a behavior of Follower: each  *)
(* step it took is one of the spec's actions, as its record says, with    *)
(* what it saw. TLC follows the behaviors the trace allows; the trace is  *)
(* one of the spec's if one of them reaches its end, which TraceMatched,  *)
(* an invariant that must be violated, says (scripts/check-tla.sh). A run *)
(* that took a step the spec doesn't allow, or saw what the spec doesn't, *)
(* ends TLC's search short of its end.                                   *)
(***************************************************************************)
EXTENDS Follower, FollowerTraceData

VARIABLE i

TraceInit == Init /\ i = 0

\* A reader's operation starting changes what the operation is, alone.
StartUnchanged ==
    UNCHANGED <<lastObs, copyExt, mappings, handles, graves, writerVars, storeVars,
                followerVars, errors>>

\* The step record e says the run took.
Step(e) ==
    CASE e.a = "Rename" -> Rename
      [] e.a = "Write" -> nextExt = e.e /\ Write(e.f)
      [] e.a = "Fetch" -> Fetch /\ target' = e.t
      [] e.a = "WaitBarrier" -> Wait
      [] e.a = "StartBatch" -> StartBatch
      [] e.a = "Invalidate" -> Invalidate(e.f)
      [] e.a = "Dropped" -> Dropped
      [] e.a = "ApplyOne" -> pending # <<>> /\ Head(pending) = <<e.k, e.v>> /\ ApplyOne
      [] e.a = "EndBatch" -> EndBatch
      [] e.a = "Advance" -> Advance
      [] e.a = "Release" -> ReleaseOf(<<e.e, e.c>>)
      [] e.a = "StartPair" -> StartPair(e.r) /\ StartUnchanged
      [] e.a = "StartFault" -> StartFault(e.r, e.f) /\ StartUnchanged
      [] e.a = "StartRead" -> StartRead(e.r, e.f) /\ StartUnchanged
      [] e.a = "StartOpen" -> StartOpen(e.r, e.f) /\ StartUnchanged
      [] e.a = "ReadKey" -> ReadKey(e.r) /\ vals'[e.r][Len(vals'[e.r])] = e.v
      [] e.a = "Found" -> Found(e.r)
      [] e.a = "Map" -> Map(e.r)
      [] e.a = "Copy" -> Copy(e.r)

TraceNext == i < Len(Trace) /\ Step(Trace[i + 1]) /\ i' = i + 1

TraceSpec == TraceInit /\ [][TraceNext]_<<vars, i>>

\* Violated once the whole trace is matched: what scripts/check-tla.sh expects.
TraceMatched == i < Len(Trace)

\* The files' first extents, as the Shuttle tests' tree has them.
TraceInitExtent == [f \in Files |-> IF f = "f1" THEN 1 ELSE 2]
=============================================================================
