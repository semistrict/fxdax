------------------------------ MODULE Journal ------------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* Fxfs's journal, its objects' flushes, the device's flushes and the     *)
(* super-block (crates/fxfs/src/object_store/journal.rs, write_super_block;*)
(* object_manager.rs, journal_file_offsets and apply_mutation; TASK-149.11)*)
(* : that a super-block claims only what its journal flushed. The bug of  *)
(* 2026-10-31 broke it: the offsets read after the device's flush, an     *)
(* object's flush ending meanwhile, its EndFlush past the super-block's   *)
(* checkpoint, the super-block said the object's mutations before it were *)
(* in its layers, which the journal, cut there, didn't say (TASK-149.5,   *)
(* dead-ends/theory-commits-racing-a-super-block-lose-the-journal.md).    *)
(*                                                                         *)
(* The journal is a sequence of records, the i-th at offset i: a commit's *)
(* mutation of an object; an object's flush's BeginFlush, whose layer     *)
(* holds the object's mutations before it; and its EndFlush, naming the   *)
(* BeginFlush, which commits the layer. What is durable is a prefix, as   *)
(* far as a device flush took it. Each object keeps the offset of its     *)
(* earliest mutation in no layer (journal_checkpoints: Current, Old while *)
(* it flushes, Both if it has more since): `old` and `cur` here, 0 for    *)
(* none. A super-block names a checkpoint, the offset its journal goes on *)
(* from (super_block_journal_file_offset), and each object's earliest     *)
(* offset, from which replay applies its mutations, or the checkpoint if  *)
(* it names none. Replay after a cut applies an object's EndFlush within  *)
(* what is durable, its layer then the object's, and each mutation from   *)
(* the object's offset on.                                                *)
(*                                                                         *)
(* write_super_block pads the journal to a block, the checkpoint; flushes *)
(* the device as far, at least; and saves the super-block. Commits and    *)
(* objects' flushes go on meanwhile. Its rules, each a member of Rules;   *)
(* specs/journal/mutations removes one each, and must find the violation  *)
(* it prevents:                                                            *)
(*                                                                         *)
(* - offsetsAtPad: the objects' offsets are read with the checkpoint,     *)
(*   under the journal's writer lock, before the flush (NoLostMutation).  *)
(* - flushBeforeSave: the super-block is saved once the journal is        *)
(*   durable to its checkpoint (SuperBlockFlushed).                       *)
(***************************************************************************)
EXTENDS Naturals, Sequences

CONSTANTS
    Objects,    \* the objects (stores, the allocator) whose mutations the journal holds
    MaxLen,     \* the journal's records at most
    Rules       \* the rules in force

AllRules == {"offsetsAtPad", "flushBeforeSave"}

ASSUME Rules \subseteq AllRules
ASSUME MaxLen \in Nat

\* No offset: records' offsets start at 1.
None == 0

VARIABLES
    journal,    \* the records written, in order
    durable,    \* how many of them a device flush made durable
    old,        \* old[o]: the earliest offset of the mutations o's flush is writing, or None
    cur,        \* cur[o]: the earliest offset of o's mutations since (or since its last flush)
    began,      \* began[o]: the offset of o's flush's BeginFlush, or None
    sb,         \* the super-block on the device: [ckpt, off]
    pc,         \* write_super_block's step: "idle", "padded" or "flushed"
    padCkpt,    \* its checkpoint
    padOff      \* the objects' offsets it read

vars == <<journal, durable, old, cur, began, sb, pc, padCkpt, padOff>>

Max(a, b) == IF a > b THEN a ELSE b

Mut(o) == [kind |-> "mut", obj |-> o, upTo |-> None]
Begin(o) == [kind |-> "begin", obj |-> o, upTo |-> None]
End(o, at) == [kind |-> "end", obj |-> o, upTo |-> at]

\* The next record's offset.
At == Len(journal) + 1
Room == Len(journal) < MaxLen

\* journal_file_offsets: each object's earliest offset, the old one first.
Offsets == [o \in Objects |-> IF old[o] # None THEN old[o] ELSE cur[o]]

TypeOK ==
    /\ durable \in 0..Len(journal)
    /\ old \in [Objects -> 0..MaxLen]
    /\ cur \in [Objects -> 0..MaxLen]
    /\ began \in [Objects -> 0..MaxLen]
    /\ pc \in {"idle", "padded", "flushed"}

Init ==
    /\ journal = <<>>
    /\ durable = 0
    /\ old = [o \in Objects |-> None]
    /\ cur = [o \in Objects |-> None]
    /\ began = [o \in Objects |-> None]
    /\ sb = [ckpt |-> 1, off |-> [o \in Objects |-> None]]
    /\ pc = "idle"
    /\ padCkpt = 1
    /\ padOff = [o \in Objects |-> None]

\* A commit's mutation of `o`: its first since o's last flush began is o's current offset.
Commit(o) ==
    /\ Room
    /\ journal' = Append(journal, Mut(o))
    /\ cur' = IF cur[o] = None THEN [cur EXCEPT ![o] = At] ELSE cur
    /\ UNCHANGED <<durable, old, began, sb, pc, padCkpt, padOff>>

\* o's flush begins (BeginFlush): its layer takes o's mutations so far, whose offset is now old.
BeginFlush(o) ==
    /\ Room
    /\ old[o] = None
    /\ cur[o] # None
    /\ journal' = Append(journal, Begin(o))
    /\ old' = [old EXCEPT ![o] = cur[o]]
    /\ cur' = [cur EXCEPT ![o] = None]
    /\ began' = [began EXCEPT ![o] = At]
    /\ UNCHANGED <<durable, sb, pc, padCkpt, padOff>>

\* o's flush ends (EndFlush), its layer written: the old offset goes.
EndFlush(o) ==
    /\ Room
    /\ old[o] # None
    /\ journal' = Append(journal, End(o, began[o]))
    /\ old' = [old EXCEPT ![o] = None]
    /\ began' = [began EXCEPT ![o] = None]
    /\ UNCHANGED <<durable, cur, sb, pc, padCkpt, padOff>>

\* A device flush, as a sync's or the journal's own, makes more of what was written durable.
DeviceFlush ==
    /\ \E d \in (durable + 1)..Len(journal): durable' = d
    /\ UNCHANGED <<journal, old, cur, began, sb, pc, padCkpt, padOff>>

\* write_super_block pads the journal, under the writer lock: the checkpoint is where the next
\* record goes, and, offsetsAtPad, the objects' offsets as of it.
Pad ==
    /\ pc = "idle"
    /\ pc' = "padded"
    /\ padCkpt' = At
    /\ padOff' = IF "offsetsAtPad" \in Rules THEN Offsets ELSE padOff
    /\ UNCHANGED <<journal, durable, old, cur, began, sb>>

\* It flushes the device, as far as the checkpoint at least, records written since maybe too; read
\* now, not offsetsAtPad, the offsets are of objects whose flushes may have ended since.
Flush ==
    /\ pc = "padded"
    /\ \E d \in Max(durable, padCkpt - 1)..Len(journal): durable' = d
    /\ padOff' = IF "offsetsAtPad" \in Rules THEN padOff ELSE Offsets
    /\ pc' = "flushed"
    /\ UNCHANGED <<journal, old, cur, began, sb, padCkpt>>

\* It saves the super-block, once flushed, if flushBeforeSave.
Save ==
    /\ \/ pc = "flushed"
       \/ pc = "padded" /\ "flushBeforeSave" \notin Rules
    /\ sb' = [ckpt |-> padCkpt, off |-> padOff]
    /\ pc' = "idle"
    /\ UNCHANGED <<journal, durable, old, cur, began, padCkpt, padOff>>

Next ==
    \/ \E o \in Objects: Commit(o) \/ BeginFlush(o) \/ EndFlush(o)
    \/ DeviceFlush
    \/ Pad
    \/ Flush
    \/ Save

Spec == Init /\ [][Next]_vars

\* After a cut, replay: o's layer holds its mutations before the BeginFlush the last durable
\* EndFlush of o names (none before 1); replay applies o's mutations from its offset on.
LayerBelow(o) ==
    LET ends == {i \in 1..durable : journal[i].kind = "end" /\ journal[i].obj = o}
        upTos == {journal[i].upTo : i \in ends}
    IN IF ends = {} THEN 1 ELSE CHOOSE u \in upTos : \A v \in upTos : v <= u

ReplayFrom(o) == IF sb.off[o] = None THEN sb.ckpt ELSE sb.off[o]

\* A cut now loses no durable mutation: each is in its object's layer, or replayed.
NoLostMutation ==
    \A o \in Objects : \A i \in 1..durable :
        journal[i] = Mut(o) => i < LayerBelow(o) \/ i >= ReplayFrom(o)

\* The super-block's journal is durable as far as its checkpoint.
SuperBlockFlushed == sb.ckpt - 1 <= durable

=============================================================================
