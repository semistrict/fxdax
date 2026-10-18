------------------------------ MODULE DaxSeal ------------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* The guest Fxfs's DAX seal and relocation protocol (guest/linux/fs/fxfs: *)
(* dax.c, and the gate in device.c), at the level of one page of a file   *)
(* (TASK-44.2), as the guest kernel 6.18 runs it.                          *)
(*                                                                         *)
(* A page's data is at a location: a block of an extent, which the window *)
(* maps. Extents are open until a seal (a COMMIT, or a WRITE with a       *)
(* barrier) seals them, and a store to a sealed one is lost or corrupts   *)
(* (doc-4). A write to a page that isn't writable in the window moves it  *)
(* to a new block of an open extent (fxfs_dax_map_write), and marks it    *)
(* relocated, so that the next iomap of it says it is shared and the      *)
(* kernel replaces its DAX entry and user mapping, which say where it was *)
(* (fxfs_dax_cow).                                                         *)
(*                                                                         *)
(* Stores are made in the device's gate (fxfs_device_store_begin); a seal *)
(* closes it, waits for the stores in it, makes user mappings read-only   *)
(* and forgets which pages are writable (fxfs_dax_seal), seals, and opens *)
(* it. A write that moved a page checks no seal came between the move and *)
(* the gate (the seal count). A fault holds its page's DAX entry lock, so *)
(* it doesn't wait for the gate: it lets the entry go and waits for the   *)
(* seal instead (-EBUSY), or a write(2) in the gate waiting for the entry *)
(* would deadlock with the seal waiting for it.                            *)
(*                                                                         *)
(* Actors: a write(2) task, a task that faults the page (reads and        *)
(* writes) and stores through its mapping, and the sealer. On 6.18 a      *)
(* fault inserting a page where its DAX entry names another, or making    *)
(* writable a mapping of another page, fails (validate_page_before_insert,*)
(* insert_page_into_pte_locked): the store gets SIGBUS.                   *)
(*                                                                         *)
(* Each mechanism is guarded by a member of Rules; specs/dax/mutations    *)
(* remove one each, and must find the violation it prevents. NoFaultsOnly *)
(* is dce4f14's bug: any iomap, a write(2)'s too, forgetting the mark.    *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    MaxWrites,      \* the write(2)s of the page
    MaxFaults,      \* the faults of the page
    MaxStores,      \* the stores through its mapping
    MaxSeals,       \* the seals
    Rules           \* the protocol's rules in force

AllRules == {"faultsOnly", "shared", "wp", "forgetWritable", "sealsCheck", "gateWait",
             "tryGate"}

ASSUME Rules \subseteq AllRules

None == "none"
NoLoc == 0
NoPte == [kind |-> None, loc |-> NoLoc]

VARIABLES
    \* The page and the store.
    loc,        \* where the page's data is
    nextLoc,    \* the next new block
    data,       \* data[l]: what location l holds
    sealed,     \* the locations sealed
    fileData,   \* the page's latest data, as the last write(2) or store left it
    \* The window and the kernel.
    writable,   \* whether the window maps the page writably (its block is open)
    relocated,  \* the page's relocation mark (range->relocated)
    entryLoc,   \* where the page's DAX entry says it is, or NoLoc
    entryLocked,\* whether a fault holds the entry
    pte,        \* the user mapping: kind None, "ro" or "rw", and its location
    \* The gate, and the file's DAX semaphore.
    sealing, stores, seals,
    semW,       \* who holds the semaphore for writing, or None
    semR,       \* whether a reader holds it
    \* The write(2) task.
    wpc, wSeals, wShared, wLoc, writes,
    \* The faulting task.
    fpc, fKind, fSeals, fShared, fLoc, faults, userStores,
    \* The sealer.
    spc, sealsDone,
    errors

vars == <<loc, nextLoc, data, sealed, fileData, writable, relocated, entryLoc, entryLocked, pte,
          sealing, stores, seals, semW, semR, wpc, wSeals, wShared, wLoc, writes, fpc, fKind,
          fSeals, fShared, fLoc, faults, userStores, spc, sealsDone, errors>>

pageVars == <<loc, nextLoc, data, sealed, fileData>>
kernelVars == <<writable, relocated, entryLoc, entryLocked, pte>>
gateVars == <<sealing, stores, seals, semW, semR>>
writerVars == <<wpc, wSeals, wShared, wLoc, writes>>
faultVars == <<fpc, fKind, fSeals, fShared, fLoc, faults, userStores>>
sealerVars == <<spc, sealsDone>>

\* A store to location l of value v, which a sealed location can't take.
Store(l, v) ==
    /\ errors' = IF l \in sealed THEN errors \cup {"store into a sealed extent"} ELSE errors
    /\ data' = [data EXCEPT ![l] = v]
    /\ fileData' = v

\* The page moves to a new block of an open extent, its data with it (Fxfs, or a fault's copy of
\* the page it held), and is marked relocated: the window mapped it where it was.
Move ==
    /\ loc' = nextLoc
    /\ nextLoc' = nextLoc + 1
    /\ data' = data @@ (nextLoc :> data[loc])
    /\ relocated' = TRUE

\* fxfs_dax_cow for a write(2)'s iomap (not a fault's): if the page was relocated, it says shared,
\* so that the write replaces the page's entry and mapping, but leaves the mark for the faults
\* after, unless no DAX entry is of the page (faultsOnly); dce4f14's bug forgot it here.
\* Returns <<shared, the mark after>>.
WriteCow ==
    IF ~relocated \/ "shared" \notin Rules THEN <<FALSE, relocated>>
    ELSE IF "faultsOnly" \notin Rules THEN <<TRUE, FALSE>>
    ELSE IF entryLoc # NoLoc \/ entryLocked THEN <<TRUE, TRUE>>
    ELSE <<FALSE, FALSE>>

\* fxfs_dax_cow for a fault's iomap: it forgets the mark, and says shared if there was one.
FaultCow ==
    IF relocated /\ "shared" \in Rules THEN <<TRUE, FALSE>> ELSE <<FALSE, relocated>>

-----------------------------------------------------------------------------
(* The write(2) task: dax_iomap_rw over fxfs_dax_iomap_write. *)

WStart ==
    /\ wpc = "idle" /\ writes < MaxWrites
    /\ wpc' = "sem"
    /\ UNCHANGED <<pageVars, kernelVars, gateVars, wSeals, wShared, wLoc, writes, faultVars,
                   sealerVars, errors>>

WSem ==
    /\ wpc = "sem" /\ semW = None /\ ~semR
    /\ semW' = "writer"
    /\ wpc' = "gate"
    /\ UNCHANGED <<pageVars, kernelVars, sealing, stores, seals, semR, wSeals, wShared, wLoc,
                   writes, faultVars, sealerVars, errors>>

\* fxfs_device_store_begin: waits for a seal to end.
WGate ==
    /\ wpc = "gate" /\ ~sealing
    /\ stores' = stores + 1
    /\ wpc' = "check"
    /\ UNCHANGED <<pageVars, kernelVars, sealing, seals, semW, semR, wSeals, wShared, wLoc,
                   writes, faultVars, sealerVars, errors>>

\* Writable, the write takes the page as it is; else out of the gate, to move it.
WCheck ==
    /\ wpc = "check"
    /\ IF writable
       THEN /\ wpc' = "cow"
            /\ UNCHANGED stores
       ELSE /\ stores' = stores - 1
            /\ wpc' = "move"
    /\ UNCHANGED <<pageVars, kernelVars, sealing, seals, semW, semR, wSeals, wShared, wLoc,
                   writes, faultVars, sealerVars, errors>>

WMove ==
    /\ wpc = "move"
    /\ wSeals' = seals
    /\ Move
    /\ wpc' = "gate2"
    /\ UNCHANGED <<sealed, fileData, writable, entryLoc, entryLocked, pte, gateVars, wShared,
                   wLoc, writes, faultVars, sealerVars, errors>>

\* Back in the gate: the moved block is mapped writably, unless a seal came between (sealsCheck),
\* which sealed it: then again from the start.
WGate2 ==
    /\ wpc = "gate2" /\ ~sealing
    /\ IF "sealsCheck" \notin Rules \/ seals = wSeals
       THEN /\ stores' = stores + 1
            /\ writable' = TRUE
            /\ wpc' = "cow"
       ELSE /\ UNCHANGED <<stores, writable>>
            /\ wpc' = "gate"
    /\ UNCHANGED <<pageVars, relocated, entryLoc, entryLocked, pte, sealing, seals, semW, semR,
                   wSeals, wShared, wLoc, writes, faultVars, sealerVars, errors>>

\* The iomap: where the page is, shared or not; the semaphore goes.
WCow ==
    /\ wpc = "cow"
    /\ wShared' = WriteCow[1]
    /\ relocated' = WriteCow[2]
    /\ wLoc' = loc
    /\ semW' = None
    /\ wpc' = "inval"
    /\ UNCHANGED <<pageVars, writable, entryLoc, entryLocked, pte, sealing, stores, seals, semR,
                   wSeals, writes, faultVars, sealerVars, errors>>

\* dax_iomap_iter, shared: the page's DAX entry, once no fault holds it, and its user mapping go
\* (invalidate_inode_pages2_range).
WInval ==
    /\ wpc = "inval"
    /\ IF wShared
       THEN /\ ~entryLocked
            /\ entryLoc' = NoLoc
            /\ pte' = NoPte
       ELSE UNCHANGED <<entryLoc, pte>>
    /\ wpc' = "copy"
    /\ UNCHANGED <<pageVars, writable, relocated, entryLocked, gateVars, wSeals, wShared, wLoc,
                   writes, faultVars, sealerVars, errors>>

WCopy ==
    /\ wpc = "copy"
    /\ Store(wLoc, 100 + writes)
    /\ wpc' = "end"
    /\ UNCHANGED <<loc, nextLoc, sealed, kernelVars, gateVars, wSeals, wShared, wLoc, writes,
                   faultVars, sealerVars>>

\* fxfs_dax_iomap_end: out of the gate.
WEnd ==
    /\ wpc = "end"
    /\ stores' = stores - 1
    /\ writes' = writes + 1
    /\ wpc' = "idle"
    /\ UNCHANGED <<pageVars, kernelVars, sealing, seals, semW, semR, wSeals, wShared, wLoc,
                   faultVars, sealerVars, errors>>

WriterNext == WStart \/ WSem \/ WGate \/ WCheck \/ WMove \/ WGate2 \/ WCow \/ WInval \/ WCopy
              \/ WEnd

-----------------------------------------------------------------------------
(* The faulting task: dax_iomap_fault over fxfs_dax_iomap_read and _write, and stores through
   the mapping it makes. *)

\* A read fault, of an unmapped page; a write fault, of one not mapped writably.
FStart(kind) ==
    /\ fpc = "idle" /\ faults < MaxFaults
    /\ IF kind = "read" THEN pte.kind = None ELSE pte.kind # "rw"
    /\ fKind' = kind
    /\ fpc' = "entry"
    /\ UNCHANGED <<pageVars, kernelVars, gateVars, writerVars, fSeals, fShared, fLoc, faults,
                   userStores, sealerVars, errors>>

\* grab_mapping_entry: the page's DAX entry, locked, before the iomap.
FEntry ==
    /\ fpc = "entry" /\ ~entryLocked
    /\ entryLocked' = TRUE
    /\ fpc' = "sem"
    /\ UNCHANGED <<pageVars, writable, relocated, entryLoc, pte, gateVars, writerVars, fKind,
                   fSeals, fShared, fLoc, faults, userStores, sealerVars, errors>>

FSem ==
    /\ fpc = "sem"
    /\ IF fKind = "read"
       THEN /\ semW = None
            /\ semR' = TRUE
            /\ UNCHANGED semW
            /\ fpc' = "rmap"
       ELSE /\ semW = None /\ ~semR
            /\ semW' = "fault"
            /\ UNCHANGED semR
            /\ fpc' = "gate"
    /\ UNCHANGED <<pageVars, kernelVars, sealing, stores, seals, writerVars, fKind, fSeals,
                   fShared, fLoc, faults, userStores, sealerVars, errors>>

\* fxfs_dax_iomap_read: where the page is.
FReadMap ==
    /\ fpc = "rmap"
    /\ fShared' = FaultCow[1]
    /\ relocated' = FaultCow[2]
    /\ fLoc' = loc
    /\ semR' = FALSE
    /\ fpc' = "insert"
    /\ UNCHANGED <<pageVars, writable, entryLoc, entryLocked, pte, sealing, stores, seals, semW,
                   writerVars, fKind, fSeals, faults, userStores, sealerVars, errors>>

\* Lets the entry and the semaphore go, and waits for the seal (-EBUSY, fxfs_dax_do_fault).
Busy ==
    /\ semW' = None
    /\ entryLocked' = FALSE
    /\ fpc' = "waitseal"

\* The gate, holding the entry: tried (tryGate), -EBUSY while a seal is in progress.
FGate ==
    /\ fpc = "gate"
    /\ IF sealing
       THEN /\ "tryGate" \in Rules
            /\ Busy
            /\ UNCHANGED stores
       ELSE /\ stores' = stores + 1
            /\ fpc' = "wcheck"
            /\ UNCHANGED <<semW, entryLocked>>
    /\ UNCHANGED <<pageVars, writable, relocated, entryLoc, pte, sealing, seals, semR,
                   writerVars, fKind, fSeals, fShared, fLoc, faults, userStores, sealerVars,
                   errors>>

FWaitSeal ==
    /\ fpc = "waitseal" /\ ~sealing
    /\ fpc' = "entry"
    /\ UNCHANGED <<pageVars, kernelVars, gateVars, writerVars, fKind, fSeals, fShared, fLoc,
                   faults, userStores, sealerVars, errors>>

FCheck ==
    /\ fpc = "wcheck"
    /\ IF writable
       THEN /\ fpc' = "wcow"
            /\ UNCHANGED stores
       ELSE /\ stores' = stores - 1
            /\ fpc' = "wmove"
    /\ UNCHANGED <<pageVars, kernelVars, sealing, seals, semW, semR, writerVars, fKind, fSeals,
                   fShared, fLoc, faults, userStores, sealerVars, errors>>

FMove ==
    /\ fpc = "wmove"
    /\ fSeals' = seals
    /\ Move
    /\ fpc' = "wgate2"
    /\ UNCHANGED <<sealed, fileData, writable, entryLoc, entryLocked, pte, gateVars, writerVars,
                   fKind, fShared, fLoc, faults, userStores, sealerVars, errors>>

FGate2 ==
    /\ fpc = "wgate2"
    /\ IF sealing
       THEN /\ "tryGate" \in Rules
            /\ Busy
            /\ UNCHANGED <<stores, writable>>
       ELSE IF "sealsCheck" \notin Rules \/ seals = fSeals
            THEN /\ stores' = stores + 1
                 /\ writable' = TRUE
                 /\ fpc' = "wcow"
                 /\ UNCHANGED <<semW, entryLocked>>
            ELSE /\ fpc' = "gate"
                 /\ UNCHANGED <<stores, writable, semW, entryLocked>>
    /\ UNCHANGED <<pageVars, relocated, entryLoc, pte, sealing, seals, semR, writerVars, fKind,
                   fSeals, fShared, fLoc, faults, userStores, sealerVars, errors>>

FWriteCow ==
    /\ fpc = "wcow"
    /\ fShared' = FaultCow[1]
    /\ relocated' = FaultCow[2]
    /\ fLoc' = loc
    /\ semW' = None
    /\ fpc' = "insert"
    /\ UNCHANGED <<pageVars, writable, entryLoc, entryLocked, pte, sealing, stores, seals, semR,
                   writerVars, fKind, fSeals, faults, userStores, sealerVars, errors>>

\* dax_insert_entry and vmf_insert_page_mkwrite: shared, or over an empty entry, the entry is
\* replaced and the old mapping unmapped; else both stay. On 6.18 an entry or mapping of another
\* page than the iomap's is refused: SIGBUS.
FInsert ==
    /\ fpc = "insert"
    /\ LET replace == fShared \/ entryLoc = NoLoc
           before == IF replace THEN NoPte ELSE pte
           entry == IF replace THEN fLoc ELSE entryLoc
       IN /\ errors' = errors
                         \cup (IF entry # fLoc THEN {"SIGBUS: the entry names another page"}
                               ELSE {})
                         \cup (IF before.kind # None /\ before.loc # fLoc
                               THEN {"SIGBUS: the mapping maps another page"} ELSE {})
          /\ entryLoc' = entry
          /\ pte' = [kind |-> IF fKind = "write" THEN "rw" ELSE "ro", loc |-> fLoc]
    /\ fpc' = IF fKind = "write" THEN "fend" ELSE "unlock"
    /\ UNCHANGED <<pageVars, writable, relocated, entryLocked, gateVars, writerVars, fKind, fSeals,
                   fShared, fLoc, faults, userStores, sealerVars>>

FEnd ==
    /\ fpc = "fend"
    /\ stores' = stores - 1
    /\ fpc' = "unlock"
    /\ UNCHANGED <<pageVars, kernelVars, sealing, seals, semW, semR, writerVars, fKind, fSeals,
                   fShared, fLoc, faults, userStores, sealerVars, errors>>

FUnlock ==
    /\ fpc = "unlock"
    /\ entryLocked' = FALSE
    /\ faults' = faults + 1
    /\ fpc' = "idle"
    /\ UNCHANGED <<pageVars, writable, relocated, entryLoc, pte, gateVars, writerVars, fKind,
                   fSeals, fShared, fLoc, userStores, sealerVars, errors>>

\* A store through a writable mapping: no gate, no fault.
UserStore ==
    /\ pte.kind = "rw" /\ userStores < MaxStores
    /\ Store(pte.loc, 200 + userStores)
    /\ userStores' = userStores + 1
    /\ UNCHANGED <<loc, nextLoc, sealed, kernelVars, gateVars, writerVars, fpc, fKind, fSeals,
                   fShared, fLoc, faults, sealerVars>>

FaultNext ==
    \/ FStart("read") \/ FStart("write") \/ FEntry \/ FSem \/ FReadMap \/ FGate \/ FWaitSeal
    \/ FCheck \/ FMove \/ FGate2 \/ FWriteCow \/ FInsert \/ FEnd \/ FUnlock \/ UserStore

-----------------------------------------------------------------------------
(* The sealer: fxfs_device_seal_begin, fxfs_dax_seal, the request, fxfs_device_seal_end. *)

SBegin ==
    /\ spc = "idle" /\ sealsDone < MaxSeals /\ ~sealing
    /\ sealing' = TRUE
    /\ seals' = seals + 1
    /\ spc' = "wait"
    /\ UNCHANGED <<pageVars, kernelVars, stores, semW, semR, writerVars, faultVars, sealsDone,
                   errors>>

\* With no store in the gate (gateWait): user mappings made read-only (wp), the window's pages
\* forgotten writable (forgetWritable).
SSealer ==
    /\ spc = "wait"
    /\ "gateWait" \in Rules => stores = 0
    /\ pte' = IF "wp" \in Rules /\ pte.kind = "rw" THEN [pte EXCEPT !.kind = "ro"] ELSE pte
    /\ writable' = IF "forgetWritable" \in Rules THEN FALSE ELSE writable
    /\ spc' = "commit"
    /\ UNCHANGED <<pageVars, relocated, entryLoc, entryLocked, gateVars, writerVars, faultVars,
                   sealsDone, errors>>

\* The device seals the open extents: every block so far.
SCommit ==
    /\ spc = "commit"
    /\ sealed' = 1..(nextLoc - 1)
    /\ spc' = "end"
    /\ UNCHANGED <<loc, nextLoc, data, fileData, kernelVars, gateVars, writerVars, faultVars,
                   sealsDone, errors>>

SEnd ==
    /\ spc = "end"
    /\ sealing' = FALSE
    /\ seals' = seals + 1
    /\ sealsDone' = sealsDone + 1
    /\ spc' = "idle"
    /\ UNCHANGED <<pageVars, kernelVars, stores, semW, semR, writerVars, faultVars, errors>>

SealerNext == SBegin \/ SSealer \/ SCommit \/ SEnd

-----------------------------------------------------------------------------

Init ==
    /\ loc = 1
    /\ nextLoc = 2
    /\ data = (1 :> 0)
    /\ sealed = {1}
    /\ fileData = 0
    /\ writable = FALSE
    /\ relocated = FALSE
    /\ entryLoc = NoLoc
    /\ entryLocked = FALSE
    /\ pte = NoPte
    /\ sealing = FALSE
    /\ stores = 0
    /\ seals = 0
    /\ semW = None
    /\ semR = FALSE
    /\ wpc = "idle" /\ wSeals = 0 /\ wShared = FALSE /\ wLoc = NoLoc /\ writes = 0
    /\ fpc = "idle" /\ fKind = None /\ fSeals = 0 /\ fShared = FALSE /\ fLoc = NoLoc
    /\ faults = 0 /\ userStores = 0
    /\ spc = "idle" /\ sealsDone = 0
    /\ errors = {}

\* No task is in the middle of anything: the model may stop there, which isn't a deadlock. A
\* deadlock is a state where a task is under way and nothing can move.
Done == wpc = "idle" /\ fpc = "idle" /\ spc = "idle"

Next ==
    \/ WriterNext
    \/ FaultNext
    \/ SealerNext
    \/ Done /\ UNCHANGED vars

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* What must hold. *)

\* No fault fails: an entry or mapping of the page where it was is replaced before its fault
\* inserts the page where it is (validate_page_before_insert, insert_page_into_pte_locked).
NoSigbus == \A e \in errors : e \notin {"SIGBUS: the entry names another page",
                                         "SIGBUS: the mapping maps another page"}

\* No store, a write(2)'s or through a mapping, lands in a sealed extent.
NoSealedStores == "store into a sealed extent" \notin errors

\* With no write(2) nor fault of the page under way, its DAX entry and user mapping are of where
\* it is, so that loads through the mapping see its latest data.
Coherent ==
    (wpc = "idle" /\ fpc = "idle") =>
        /\ entryLoc \in {NoLoc, loc}
        /\ pte.kind # None => pte.loc = loc

\* And its data is the latest written: no move or store lost any.
DataKept == (wpc = "idle" /\ fpc = "idle") => data[loc] = fileData

=============================================================================
