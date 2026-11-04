------------------------------ MODULE DaxCow ------------------------------
(***************************************************************************)
(* Copyright 2026 The fxdax Authors. All rights reserved.                  *)
(* Use of this source code is governed by a BSD-style license that can be *)
(* found in the LICENSE file.                                              *)
(*                                                                         *)
(* Copy-on-write DAX (backlog doc-13, TASK-164): the guest Fxfs's window  *)
(* maps sealed extents, or zeros, read-only; a window page the guest      *)
(* dirties, the host makes the VM's own, a private copy of it at the same *)
(* address (the device's DAX feature), before any store reaches it.       *)
(* Fxfs allocates the blocks at writeback, as the page cache's path       *)
(* does, and commits after their bytes: guest/linux/fs/fxfs/dax.c, DAX    *)
(* as it is since writable DAX went (dead-ends/writable-dax.md).          *)
(*                                                                         *)
(* A file of a few pages (Pages), one task storing to them, one doing the *)
(* rest, and the host:                                                     *)
(*                                                                         *)
(* - the storer: a fault, whose iomap_begin (IOMAP_WRITE) has the host   *)
(*   make the page private (PRIVATE), puts it in the file's dirty set,     *)
(*   counts it private, and maps it writable; the fault holds the page's  *)
(*   DAX entry past its iomap_end until it returns, after which the store,*)
(*   through the PTE, if it is writable still, or a fault again. Or a     *)
(*   write(2), whose copy into the window is between its iomap_begin and  *)
(*   iomap_end, holding the file's semaphore shared. Before either, a page*)
(*   not counted waits, holding nothing, for the budget to have room for  *)
(*   it (private_max).                                                     *)
(* - the other task: writeback (->writepages), fsync (writeback, then the *)
(*   seal that makes it durable), rebinding (a private page MAPped back to*)
(*   its extent's page, freeing it), and punching a page (truncate).      *)
(*   Writeback swaps the dirty set out for an empty one (the writeback    *)
(*   set), write-protects the PTEs (by the page tables), copies the pages *)
(*   from the window, WRITEs them to a new extent, commits the            *)
(*   transaction naming it, and keeps the pages until the seal after that *)
(*   commit, which seals the extent. The extent that held a page's block  *)
(*   before dies at that seal (Fxfs frees it after the flush), and a      *)
(*   window page mapping it reads zeros from then on.                      *)
(* - the host: copies on write, at a PRIVATE or at a store to a page not  *)
(*   private, refusing either past private_max, which stops the VM; seals *)
(*   (a COMMIT: the journal's, or any fsync's) at any time; and refuses a *)
(*   MAP of an open extent (EPERM).                                        *)
(*                                                                         *)
(* Data are versions: each store writes the next, a punch writes zeros as *)
(* the next, and a page's copy holds every store before it. A store is to *)
(* part of a page: one the host copies a freed block's zeros for leaves   *)
(* the rest of the page lost (Torn). `durable` is what a crash leaves:    *)
(* Fxfs's file as of the last seal.                                        *)
(*                                                                         *)
(* The rules, each a member of Rules; specs/dax-cow/mutations removes one *)
(* each, and must find the violation it prevents:                          *)
(*                                                                         *)
(* - privateAtIomap: the iomap that dirties a page has the host make it   *)
(*   private first (PRIVATE), so that dirty implies private (the invariant*)
(*   DirtyImpliesPrivate), and no store ever has the host copy a window   *)
(*   page: the store of a fault comes after its iomap_end, maybe after a  *)
(*   writeback took the page, moved its block, and the seal after freed   *)
(*   the old one, whose zeros the host would copy (guest/sim's seed 2802; *)
(*   TASK-164.9, dax.c's fxfs_dax_cow_dirty).                              *)
(* - protectByPageTables: writeback write-protects every PTE of its pages, *)
(*   by the page tables; dax_writeback_mapping_range, by the DAX entries, *)
(*   passes by an entry a fault holds past its iomap_end, whose PTE stays *)
(*   writable, and a store through it after the writeback's copy is in no *)
(*   set (guest/sim's seed 22).                                            *)
(* - swapFirst: writeback swaps the dirty set before it write-protects, so*)
(*   that a store after the write-protect faults into the new dirty set.  *)
(* - swapExclusive: the swap holds the semaphore exclusively, so that no  *)
(*   iomap between its iomap_begin, which dirtied its page, and its copy  *)
(*   or PTE straddles it: a write(2) copying after the swap and the       *)
(*   writeback's copy loses its bytes. Doc-13 has the swap under the      *)
(*   semaphore shared, which this model finds (NoSwapExclusive.cfg): a     *)
(*   page put in the dirty set at iomap_end, after the copy, as the page  *)
(*   cache's write_end dirties a folio, would do as well.                  *)
(* - rebindExclusive: a rebind holds the semaphore exclusively, so that no*)
(*   iomap dirties a page between its look and its MAP.                    *)
(* - rebindAfterSeal: a rebind takes no page of the writeback set, whose   *)
(*   extent the seal after its commit hasn't sealed.                        *)
(* - keepUntilSealed: writeback keeps its pages in the writeback set until*)
(*   that seal, so that a rebind finding a page in neither set finds its   *)
(*   extent sealed. Dax.c keeps these two otherwise: a rebind maps only    *)
(*   what Fxfs says is sealed, and leaves a page private on the host's     *)
(*   EPERM (fxfs_dax_cow_map), so guest/sim has no mutant of them.          *)
(* - countPrivate: iomap_begin counts its page private, which the budget  *)
(*   waits on, so that the guest never asks the host for a private page   *)
(*   past private_max.                                                     *)
(* - bytesFirst: writeback commits after its bytes are written (Fxfs's     *)
(*   fxfs_write_many), so that a crash shows old or new data, never what   *)
(*   a new extent held unwritten.                                           *)
(*                                                                         *)
(* Writeback keeps punches and rebinds out from its swap to its commit,    *)
(* which doc-13 doesn't say: else one meets a copy whose commit is to come, *)
(* which undoes it. Dax.c has a lock for it, `wb_lock`, which they take    *)
(* before the semaphore; here writeback holds the semaphore shared to the  *)
(* commit, which they take exclusively. The kernel's DAX entries are      *)
(* modelled only as a fault holds its page's (EntryHeld), which nothing of*)
(* the protocol waits for: a fault trylocks the semaphore, holding its     *)
(* entry, as dax.c's do since TASK-79.12. Their dirty tags aren't: no rule *)
(* reads them (dax.c cleans them for the kernel's sake alone, and a range's*)
(* release, which needs them clean, is the kernel's, outside this model).  *)
(***************************************************************************)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Pages,          \* the file's pages
    PrivateMax,     \* the device's private_max
    MaxStores,      \* the stores, faults' and write(2)s'
    MaxPunches,     \* the punches
    Rules           \* the protocol's rules in force

AllRules == {"privateAtIomap", "protectByPageTables", "swapFirst", "swapExclusive",
             "rebindExclusive", "rebindAfterSeal", "keepUntilSealed", "countPrivate",
             "bytesFirst"}

ASSUME Rules \subseteq AllRules
ASSUME PrivateMax \in Nat \ {0}

\* What a new extent holds where nothing was written to it.
Garbage == -1
\* What a freed block reads as, through the window: zeros.
Zeroed == -2
\* A page the host copied a freed block's zeros for, then a store changed a part of.
Torn == -3

VARIABLES
    \* The host.
    win,        \* win[p]: the window's page: [kind: "extent", "zeros", "private", "doomed"
                \* (an extent's block Fxfs replaced, freed at the next seal) or "freed", data]
    refused,    \* whether the host refused a store or a PRIVATE, which stops the VM
    \* Fxfs.
    file,       \* file[p]: the file's committed block: [state: "hole", "open" or "sealed", data]
    durable,    \* durable[p]: what a crash shows: file[p]'s data as of the last seal
    \* What happened.
    latest,     \* latest[p]: the version of the last store or punch of p that completed
    next,       \* the next version
    errors,     \* what the host refused that it shouldn't have been asked
    \* The guest.
    pte,        \* pte[p]: the user's mapping of p: "none", "ro" or "rw"
    dirty,      \* the file's dirty set
    wb,         \* the writeback set, from the swap to the commit
    sealing,    \* the writeback set, from the commit to the seal (keepUntilSealed)
    counted,    \* the pages the guest counts private
    semX,       \* who holds the file's semaphore exclusively: "none", "S" or "W"
    semR,       \* who holds it shared
    \* The storer.
    spc, sPage, sWay, stores,
    \* The other task.
    wpc, wOp, wPage, floor, buf, punches

hostVars == <<win, refused>>
fxfsVars == <<file, durable>>
ghostVars == <<latest, next, errors>>
guestVars == <<pte, dirty, wb, sealing, counted, semX, semR>>
sVars == <<spc, sPage, sWay, stores>>
wVars == <<wpc, wOp, wPage, floor, buf, punches>>
vars == <<hostVars, fxfsVars, ghostVars, guestVars, sVars, wVars>>

\* The pages the host holds private.
HostPrivate == Cardinality({p \in Pages : win[p].kind = "private"})

\* Whether the host takes a store to p: a private page's, or a copy within private_max.
CanStore(p) == win[p].kind = "private" \/ HostPrivate < PrivateMax

\* A store of the next version to part of p, through the window: the host copies on write, what
\* the window maps there (a freed block's zeros, the rest of the page lost), or refuses.
Store(p) ==
    IF CanStore(p)
    THEN /\ win' = [win EXCEPT ![p] = [kind |-> "private",
                                       data |-> IF win[p].kind = "freed" THEN Torn ELSE next]]
         /\ latest' = [latest EXCEPT ![p] = next]
         /\ next' = next + 1
         /\ UNCHANGED refused
    ELSE /\ refused' = TRUE
         /\ UNCHANGED <<win, latest, next>>

\* PRIVATE of p: the host copies it now, what the window maps there, within private_max.
MadePrivate(p) ==
    IF win[p].kind = "private" THEN win[p]
    ELSE [kind |-> "private", data |-> win[p].data]

\* Whether the storer's fault holds p's DAX entry: from before its iomap_begin until it returns.
EntryHeld(p) == sWay = "fault" /\ sPage = p /\ spc \in {"lock", "begin", "unlock", "entry"}

\* Writeback's write-protect: every writable PTE read-only, by the page tables; or, by the DAX
\* entries, as dax_writeback_mapping_range does, every one but that of an entry a fault holds.
WriteProtected ==
    [p \in Pages |-> IF pte[p] = "rw" /\ ("protectByPageTables" \in Rules \/ ~EntryHeld(p))
                     THEN "ro" ELSE pte[p]]

\* A seal: the open extents sealed, the commits so far durable, the writeback set let go of, and
\* the extents the commits replaced freed: a window page mapping one reads zeros.
SealEffect ==
    /\ file' = [p \in Pages |->
                   IF file[p].state = "open" THEN [file[p] EXCEPT !.state = "sealed"]
                   ELSE file[p]]
    /\ durable' = [p \in Pages |-> file[p].data]
    /\ sealing' = {}
    /\ win' = [p \in Pages |-> IF win[p].kind = "doomed" THEN [kind |-> "freed", data |-> Zeroed]
                               ELSE win[p]]

-----------------------------------------------------------------------------
(* The storer. *)

\* A store to a page: through its PTE if it is writable, or a fault; or a write(2).
SStart ==
    /\ spc = "idle" /\ stores < MaxStores
    /\ \E p \in Pages, way \in {"fault", "write"} :
         /\ sPage' = p
         /\ sWay' = way
         /\ spc' = IF way = "fault" /\ pte[p] = "rw" THEN "store" ELSE "throttle"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, guestVars, stores, wVars>>

\* The budget: a page not counted waits, holding nothing, until there is room for it.
SThrottle ==
    /\ spc = "throttle"
    /\ sPage \in counted \/ Cardinality(counted) < PrivateMax
    /\ spc' = "lock"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, guestVars, sPage, sWay, stores, wVars>>

SLock ==
    /\ spc = "lock" /\ semX = "none"
    /\ semR' = semR \cup {"S"}
    /\ spc' = "begin"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, pte, dirty, wb, sealing, counted, semX, sPage,
                   sWay, stores, wVars>>

\* fxfs_dax_iomap_begin, IOMAP_WRITE (fxfs_dax_cow_dirty): the page made private on the host
\* (privateAtIomap), unless the host refuses, then dirty, counted private (countPrivate); a fault
\* maps it writable before its iomap_end.
SBegin ==
    /\ spc = "begin"
    /\ IF "privateAtIomap" \in Rules /\ ~CanStore(sPage)
       THEN /\ refused' = TRUE
            /\ spc' = "stopped"
            /\ UNCHANGED <<win, pte, dirty, counted>>
       ELSE /\ win' = IF "privateAtIomap" \in Rules
                      THEN [win EXCEPT ![sPage] = MadePrivate(sPage)] ELSE win
            /\ dirty' = dirty \cup {sPage}
            /\ counted' = IF "countPrivate" \in Rules THEN counted \cup {sPage} ELSE counted
            /\ pte' = IF sWay = "fault" THEN [pte EXCEPT ![sPage] = "rw"] ELSE pte
            /\ spc' = IF sWay = "fault" THEN "unlock" ELSE "copy"
            /\ UNCHANGED refused
    /\ UNCHANGED <<fxfsVars, ghostVars, wb, sealing, semX, semR, sPage, sWay, stores, wVars>>

\* A write(2)'s copy into the window, between its iomap_begin and iomap_end.
SCopy ==
    /\ spc = "copy"
    /\ Store(sPage)
    /\ stores' = stores + 1
    /\ spc' = IF CanStore(sPage) THEN "unlock" ELSE "stopped"
    /\ UNCHANGED <<fxfsVars, errors, guestVars, sPage, sWay, wVars>>

\* fxfs_dax_iomap_end; a fault holds its page's DAX entry until it returns (SReturn).
SUnlock ==
    /\ spc = "unlock"
    /\ semR' = semR \ {"S"}
    /\ IF sWay = "fault"
       THEN /\ spc' = "entry"
            /\ UNCHANGED <<sPage, sWay>>
       ELSE /\ spc' = "idle"
            /\ sPage' = 0
            /\ sWay' = "none"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, pte, dirty, wb, sealing, counted, semX, stores,
                   wVars>>

\* The fault returns, its page's DAX entry let go of, and the store is retried.
SReturn ==
    /\ spc = "entry"
    /\ spc' = "store"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, guestVars, sPage, sWay, stores, wVars>>

\* The store through the PTE, writable still; or, write-protected or unmapped since, a fault again.
SStore ==
    /\ spc = "store"
    /\ IF pte[sPage] = "rw"
       THEN /\ Store(sPage)
            /\ stores' = stores + 1
            /\ spc' = IF CanStore(sPage) THEN "idle" ELSE "stopped"
            /\ sPage' = 0
            /\ sWay' = "none"
       ELSE /\ spc' = "throttle"
            /\ UNCHANGED <<win, refused, latest, next, stores, sPage, sWay>>
    /\ UNCHANGED <<fxfsVars, errors, guestVars, wVars>>

StorerNext == SStart \/ SThrottle \/ SLock \/ SBegin \/ SCopy \/ SUnlock \/ SReturn \/ SStore

-----------------------------------------------------------------------------
(* The other task: writeback, fsync, rebinding, punching. *)

WStart ==
    /\ wpc = "idle"
    /\ \E op \in {"writeback", "fsync", "rebind", "punch"} :
         /\ op = "punch" => punches < MaxPunches
         /\ op = "rebind" => HostPrivate > 0
         /\ wOp' = op
         \* An fsync keeps every store that completed before it began.
         /\ floor' = IF op = "fsync" THEN latest ELSE floor
         /\ wpc' = CASE op \in {"writeback", "fsync"} ->
                           (IF "swapFirst" \in Rules THEN "swapLock" ELSE "wpFirst")
                     [] op = "rebind" -> "rbLock"
                     [] op = "punch" -> "ptLock"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, guestVars, sVars, wPage, buf, punches>>

\* Without swapFirst: the write-protect before the swap.
WWpFirst ==
    /\ wpc = "wpFirst"
    /\ pte' = WriteProtected
    /\ wpc' = "swapLock"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, dirty, wb, sealing, counted, semX, semR, sVars,
                   wOp, wPage, floor, buf, punches>>

WSwapLock ==
    /\ wpc = "swapLock" /\ semX = "none"
    /\ IF "swapExclusive" \in Rules
       THEN /\ semR = {}
            /\ semX' = "W"
            /\ UNCHANGED semR
       ELSE /\ semR' = semR \cup {"W"}
            /\ UNCHANGED semX
    /\ wpc' = "swap"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, pte, dirty, wb, sealing, counted, sVars, wOp,
                   wPage, floor, buf, punches>>

\* The dirty set swapped out for an empty one; the semaphore downgraded, held shared to the commit.
WSwap ==
    /\ wpc = "swap"
    /\ wb' = dirty
    /\ dirty' = {}
    /\ semX' = "none"
    /\ semR' = semR \cup {"W"}
    /\ wpc' = IF "swapFirst" \in Rules THEN "wp" ELSE "copy"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, pte, sealing, counted, sVars, wOp, wPage, floor,
                   buf, punches>>

WWp ==
    /\ wpc = "wp"
    /\ pte' = WriteProtected
    /\ wpc' = "copy"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, dirty, wb, sealing, counted, semX, semR, sVars,
                   wOp, wPage, floor, buf, punches>>

\* fxfs_write_many's copy of the pages, from the window into its transfer buffer.
WCopy ==
    /\ wpc = "copy"
    /\ buf' = [p \in Pages |-> IF p \in wb THEN win[p].data ELSE buf[p]]
    /\ wpc' = IF "bytesFirst" \in Rules THEN "write" ELSE "commit"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, guestVars, sVars, wOp, wPage, floor, punches>>

\* Its WRITE of them to a new extent: before the commit (bytesFirst), or after it.
WWrite ==
    /\ wpc = "write"
    /\ IF "bytesFirst" \in Rules
       THEN /\ wpc' = "commit"
            /\ UNCHANGED <<file, wb>>
       ELSE /\ file' = [p \in Pages |->
                           IF p \in wb THEN [file[p] EXCEPT !.data = buf[p]] ELSE file[p]]
            /\ wb' = {}
            /\ wpc' = "unlock"
    /\ UNCHANGED <<hostVars, durable, ghostVars, pte, dirty, sealing, counted, semX, semR, sVars,
                   wOp, wPage, floor, buf, punches>>

\* The transaction naming the new extent, open; its pages kept until it is sealed. The blocks it
\* replaces die at the next seal: a window page mapping one, not private, is doomed.
WCommit ==
    /\ wpc = "commit"
    /\ file' = [p \in Pages |->
                   IF p \in wb
                   THEN [state |-> "open",
                         data |-> IF "bytesFirst" \in Rules THEN buf[p] ELSE Garbage]
                   ELSE file[p]]
    /\ win' = [p \in Pages |->
                  IF p \in wb /\ win[p].kind = "extent" THEN [win[p] EXCEPT !.kind = "doomed"]
                  ELSE win[p]]
    /\ sealing' = IF "keepUntilSealed" \in Rules THEN sealing \cup wb ELSE sealing
    /\ wb' = IF "bytesFirst" \in Rules THEN {} ELSE wb
    /\ wpc' = IF "bytesFirst" \in Rules THEN "unlock" ELSE "write"
    /\ UNCHANGED <<refused, durable, ghostVars, pte, dirty, counted, semX, semR, sVars, wOp,
                   wPage, floor, buf, punches>>

WUnlock ==
    /\ wpc = "unlock"
    /\ semR' = semR \ {"W"}
    /\ semX' = IF semX = "W" THEN "none" ELSE semX
    /\ wpc' = IF wOp = "fsync" THEN "fsSeal" ELSE "idle"
    /\ wPage' = 0
    /\ buf' = [p \in Pages |-> 0]
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, pte, dirty, wb, sealing, counted, sVars, wOp,
                   floor, punches>>

\* An fsync's seal (fxfs_fsync), after its writeback.
WFsSeal ==
    /\ wpc = "fsSeal"
    /\ SealEffect
    /\ wpc' = "fsDone"
    /\ UNCHANGED <<refused, ghostVars, pte, dirty, wb, counted, semX, semR, sVars, wOp, wPage,
                   floor, buf, punches>>

\* The fsync returns.
WFsDone ==
    /\ wpc = "fsDone"
    /\ wpc' = "idle"
    /\ floor' = [p \in Pages |-> 0]
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, guestVars, sVars, wOp, wPage, buf, punches>>

WRbLock ==
    /\ wpc = "rbLock" /\ semX = "none"
    /\ IF "rebindExclusive" \in Rules
       THEN /\ semR = {}
            /\ semX' = "W"
            /\ UNCHANGED semR
       ELSE /\ semR' = semR \cup {"W"}
            /\ UNCHANGED semX
    /\ wpc' = "rbLook"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, pte, dirty, wb, sealing, counted, sVars, wOp,
                   wPage, floor, buf, punches>>

\* A private page in neither set (rebindAfterSeal: nor waiting for its seal), if there is one.
WRbLook ==
    /\ wpc = "rbLook"
    /\ LET candidates == {p \in Pages :
                             /\ win[p].kind = "private"
                             /\ p \notin dirty \cup wb
                             /\ "rebindAfterSeal" \in Rules => p \notin sealing}
       IN IF candidates = {}
          THEN /\ wpc' = "unlock"
               /\ UNCHANGED wPage
          ELSE \E p \in candidates :
                 /\ wPage' = p
                 /\ wpc' = "rbMap"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, guestVars, sVars, wOp, floor, buf, punches>>

\* The MAP of the file's block over the private page, which the host frees: zeros for a hole.
WRbMap ==
    /\ wpc = "rbMap"
    /\ errors' = IF file[wPage].state = "open" THEN errors \cup {"a MAP of an open extent"}
                 ELSE errors
    /\ win' = [win EXCEPT ![wPage] =
                  [kind |-> IF file[wPage].state = "hole" THEN "zeros" ELSE "extent",
                   data |-> file[wPage].data]]
    /\ counted' = counted \ {wPage}
    /\ wpc' = "unlock"
    /\ UNCHANGED <<refused, fxfsVars, latest, next, pte, dirty, wb, sealing, semX, semR, sVars,
                   wOp, wPage, floor, buf, punches>>

WPtLock ==
    /\ wpc = "ptLock" /\ semX = "none" /\ semR = {}
    /\ semX' = "W"
    /\ wpc' = "punch"
    /\ UNCHANGED <<hostVars, fxfsVars, ghostVars, pte, dirty, wb, sealing, counted, semR, sVars,
                   wOp, wPage, floor, buf, punches>>

\* A punch of a page: out of both sets, unmapped, MAP_ZEROS over it, a hole in Fxfs.
WPunch ==
    /\ wpc = "punch"
    /\ \E p \in Pages :
         /\ dirty' = dirty \ {p}
         /\ sealing' = sealing \ {p}
         /\ counted' = counted \ {p}
         /\ pte' = [pte EXCEPT ![p] = "none"]
         /\ win' = [win EXCEPT ![p] = [kind |-> "zeros", data |-> next]]
         /\ file' = [file EXCEPT ![p] = [state |-> "hole", data |-> next]]
         /\ latest' = [latest EXCEPT ![p] = next]
    /\ next' = next + 1
    /\ punches' = punches + 1
    /\ wpc' = "unlock"
    /\ UNCHANGED <<refused, durable, errors, wb, semX, semR, sVars, wOp, wPage, floor, buf>>

OtherNext ==
    \/ WStart \/ WWpFirst \/ WSwapLock \/ WSwap \/ WWp \/ WCopy \/ WWrite \/ WCommit \/ WUnlock
    \/ WFsSeal \/ WFsDone \/ WRbLock \/ WRbLook \/ WRbMap \/ WPtLock \/ WPunch

-----------------------------------------------------------------------------
(* The host's seals: the journal's, or another file's fsync's, whenever something is open. *)

Seal ==
    /\ \E p \in Pages : file[p].state = "open"
    /\ SealEffect
    /\ UNCHANGED <<refused, ghostVars, pte, dirty, wb, counted, semX, semR, sVars, wVars>>

-----------------------------------------------------------------------------

Init ==
    \* Each page a sealed extent's, as a file read in has, or a hole: the window maps them so.
    /\ \E holes \in SUBSET Pages :
         /\ file = [p \in Pages |->
                       [state |-> IF p \in holes THEN "hole" ELSE "sealed", data |-> 0]]
         /\ win = [p \in Pages |->
                      [kind |-> IF p \in holes THEN "zeros" ELSE "extent", data |-> 0]]
    /\ refused = FALSE
    /\ durable = [p \in Pages |-> 0]
    /\ latest = [p \in Pages |-> 0]
    /\ next = 1
    /\ errors = {}
    /\ pte = [p \in Pages |-> "none"]
    /\ dirty = {} /\ wb = {} /\ sealing = {} /\ counted = {}
    /\ semX = "none" /\ semR = {}
    /\ spc = "idle" /\ sPage = 0 /\ sWay = "none" /\ stores = 0
    /\ wpc = "idle" /\ wOp = "none" /\ wPage = 0 /\ punches = 0
    /\ floor = [p \in Pages |-> 0]
    /\ buf = [p \in Pages |-> 0]

Next == StorerNext \/ OtherNext \/ Seal

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* What must hold. *)

\* An fsync that returned has every store that completed before it began in what a crash shows: the
\* extent its commit names, or a later one.
FsyncKeepsStores == wpc = "fsDone" => \A p \in Pages : durable[p] >= floor[p]

\* The window shows every page's last store: no rebind, nor anything else, discarded one.
NoLostStore == \A p \in Pages : win[p].data = latest[p]

\* A page in neither set is in Fxfs as the window shows it: writeback has nothing of it to write.
CleanInFxfs == \A p \in Pages : p \notin dirty \cup wb => file[p].data = win[p].data

\* Nothing maps an open extent: the host refuses such a MAP (EPERM).
NoOpenMap == "a MAP of an open extent" \notin errors

\* A crash shows each page old or new, as a commit left it, never a new extent's unwritten block.
CrashOldOrNew == \A p \in Pages : durable[p] # Garbage

\* The host never refuses a store, nor a PRIVATE: the guest keeps within private_max.
NoRefusal == ~refused

\* A dirty page, or one being written back, is private on the host: a store to it reaches no
\* extent's block, which the writeback moves, and the seal after it frees (privateAtIomap).
DirtyImpliesPrivate == \A p \in dirty \cup wb : win[p].kind = "private"

\* At most private_max private pages: the host's to keep, by refusing.
PrivateWithinMax == HostPrivate <= PrivateMax
=============================================================================
