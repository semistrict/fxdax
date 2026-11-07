// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TASK-149.17: a compromised VM process, of one tenant, checked against what must stay true of
//! the tenants beside it, as each of its sequences ends: not that the daemon only panics nothing and
//! serves on ([`super::whatever_a_vm_process_says_harms_no_other`], TASK-77), but what it serves the
//! others, and what it holds.
//!
//! Three tenants share one daemon: mallory, whose VM processes are hostile; alice, granted too,
//! whose one VM process uses the protocol as it allows, beside mallory's, its every result checked
//! against a model of its store and window; and the default tenant's bob, whose store waits. A case
//! is one to three of mallory's connections at once, each saying anything, of any kind, flags and
//! words, naming alice's and bob's branches, extents and stores, paths that lead out of its
//! directory, forks to their branches, transfer areas of any make, storms of `PRIVATE`s and
//! `UNMAP`s, acknowledgements forged, early or never sent, and hanging up as it likes, mid-request
//! or with its framing broken; or flooding the daemon with reads whose replies it leaves unread;
//! and doing what it likes to the memory it is given: shrinking, growing, punching holes in and
//! writing it, and handing it back as its transfer area.
//! Alice meanwhile writes, commits, reads, frees and maps extents, and stores to and loads from her
//! window, which copies on write, in this process. Then:
//!
//! - **Alice is served as her model says**, her commits among it, which go on succeeding
//!   (TASK-150.7); and her store closed, which mallory names, holds what she wrote.
//! - **Mallory gets nothing of theirs**: no reply, no stage operation, names their branches or
//!   holds their bytes; it maps no arena but its tenant's and the public one, gets no other
//!   tenant's memory, and finds none of their bytes in the memory it gets, read whole.
//! - **The daemon holds no more of mallory's than the protocol's bounds say** (`server::Peer::room`
//!   each connection), however it floods it (TASK-150.3): the process's heap, counted as it is
//!   allocated ([`Bounded`]), and a VM process past its room let go.
//! - **Nothing of mallory's stays, nor goes**: its every connection ends; every store it named opens
//!   again (TASK-150.5), every one it wrote commits; no point is taken of a branch it named
//!   (TASK-150.4); nothing comes into the others' directories; nothing panics, nor crashes; bob's
//!   store reads, writes and commits.
//!
//! What it found is pinned beside it: VM processes shrinking the memory they are given crashed the
//! daemon ([`vm_processes_shrinking_their_memory_crash_nothing`]); requests held copies of
//! their whole lengths past the connection's room ([`what_requests_hold_is_within_the_room`]);
//! and a connection's faults paged in as many parts at once as it kept faults in flight
//! ([`page_ins_in_flight_are_bounded`], TASK-149.34), which mallory's floods of page-ins do here.
//! `crates/fxdax/mutants.sh` puts back a bug of each kind it exists to find, and sees it fail.

use super::super::client::RemoteExtentStore;
use super::super::server::{QUEUED_ROOM, Tenant, connections, serve, serve_tenant};
use super::super::stores::table;
use super::super::wire::*;
use super::{BLOCK, LastWords, control, host, memory, read, within, write};
use crate::pages::{ArenaId, page_size};
use crate::{Errno, ExtentId};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::os::fd::{AsFd as _, AsRawFd as _, BorrowedFd, FromRawFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use zerocopy::IntoBytes as _;

/// The bytes of mallory's usual window, and of its stores.
const WINDOW_BYTES: u64 = 1 << 20;
const STORE_BYTES: u64 = 1 << 24;
/// Alice's window, of pages, and the most of them private at once.
const ALICE_PAGES: usize = 32;
const ALICE_PRIVATE: usize = 6;
/// What every 16 bytes alice and bob write begin with, which no byte mallory gets may hold.
const ALICE_MARK: [u8; 8] = *b"ALICE\xa1\x1c\xe5";
const BOB_MARK: [u8; 8] = *b"BOB\xb0\xb0\xb0\xb0\x0b";
/// The ID of the request that ends what a connection of mallory's says, which it waits for.
const LAST: u64 = u64::MAX - 1;
/// The extents of the point mallory's VM manager made for it, a MiB each.
const GIVEN_EXTENTS: u64 = 12;
/// The extent a flood reads, its own, and how much of it each read asks for.
const FLOOD_EXTENT: u64 = 0xf100d;
const FLOOD_READ: u64 = 1 << 20;
/// What a case's heap may hold past what mallory's connections' rooms do: alice's, mallory's stores
/// as they open, and the bucket in memory taking what they upload.
const SLACK: u64 = 256 << 20;
/// How long anything may take before the case fails: what would otherwise hang for good.
const WAIT: Duration = Duration::from_secs(60);

/// Counts what the process holds of its heap (`HELD`), and, while a case's [`LIMIT`] is set, aborts
/// past it, saying the case and where it allocated: as fxfs-posix's corrupt tests bound theirs.
/// Aborted, as what is asked for past it might be asked for without end, the machine's memory
/// with it; and so as soon as it is asked for.
struct Bounded;

static HELD: AtomicUsize = AtomicUsize::new(0);
/// The most it held since [`peak_while`] last looked.
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// The most the heap may hold, or 0, which checks nothing.
static LIMIT: AtomicUsize = AtomicUsize::new(0);
/// The case running, as proptest says it.
static CASE: Mutex<String> = Mutex::new(String::new());

#[global_allocator]
static ALLOCATOR: Bounded = Bounded;

impl Bounded {
    fn check(size: usize) {
        let held = HELD.fetch_add(size, Ordering::Relaxed) + size;
        PEAK.fetch_max(held, Ordering::Relaxed);
        let limit = LIMIT.load(Ordering::Relaxed);
        if limit != 0 && held > limit {
            // Nothing below may come back here.
            LIMIT.store(0, Ordering::Relaxed);
            let backtrace = std::backtrace::Backtrace::force_capture();
            let case = CASE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            eprintln!("{held} bytes held, of at most {limit}, in {case}:\n{backtrace}");
            std::process::abort();
        }
    }
}

// SAFETY: this only counts what `System` does.
unsafe impl GlobalAlloc for Bounded {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::check(layout.size());
        // SAFETY: As the caller promises.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::check(layout.size());
        // SAFETY: As the caller promises.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: As the caller promises.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        Self::check(size);
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: As the caller promises.
        unsafe { System.realloc(pointer, layout, size) }
    }
}

/// The most the heap held past what it holds now while `work` ran.
fn peak_while(work: impl FnOnce()) -> u64 {
    let before = HELD.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    work();
    PEAK.load(Ordering::Relaxed).saturating_sub(before) as u64
}

/// What the daemon holds at most of what a VM process with a window of `window` bytes keeps in
/// flight, as the protocol bounds it: its requests' stream data, what it queued for it besides,
/// and as many mappings as a reply to `MAPPINGS` holds of its window (TASK-79.16, TASK-79.43).
fn room(window: u64) -> u64 {
    MAX_STREAM_BYTES + QUEUED_ROOM + window / page_size() * MAPPING_BYTES
}

/// A word of a message mallory says: as it is, or naming a branch, as the case has it.
#[derive(Clone, Copy, Debug)]
enum Word {
    Is(u64),
    /// Its own store's branch, as of what it was answered so far.
    Own,
    Alice,
    Bob,
    /// The branch its VM manager made a point of for it, in its directory.
    Given,
}

/// A path mallory names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// A store of its own, of the case's: one of four, which its connections share.
    Own(u8),
    /// The point its VM manager made for it.
    Point,
    /// A point there isn't, in its directory, of the case's, which only restores name.
    Nothing,
    /// Alice's store she closed, out of its directory by `..`.
    Escaped,
    /// Alice's store she closed, through a link in its directory to hers.
    Linked,
    /// Alice's stores by their own paths: the one she closed and the one she has open.
    Alices,
    AlicesOpen,
    /// A name in alice's directory that nothing has, of the case's: by its own path, and through
    /// the link.
    AlicesNew,
    LinkedNew,
    /// Bob's store.
    Bobs,
    Relative,
    /// Its directory itself.
    Directory,
}

/// A message's payload.
#[derive(Clone, Debug)]
enum Payload {
    Bytes(Vec<u8>),
    /// So many bytes of one.
    Filled(u8, usize),
    Path(Place),
    /// A restore's: its store, a NUL, and the point it forks.
    ForkFrom(Place, Place),
    /// A `FREE_MANY`'s extents, after the first.
    Extents(Vec<(Word, u64)>),
}

/// A transfer area mallory gives, of the bytes it says.
#[derive(Clone, Copy, Debug)]
enum Area {
    Sealed(u64),
    /// Not sealed against shrinking.
    Unsealed(u64),
    /// Of half the bytes it says.
    Short(u64),
    /// A pipe, no shared memory at all.
    Pipe,
    /// The memory it was given as its store opened, its tenant's (0) or the public one (1), of
    /// the bytes it says.
    Given(u8, u64),
}

/// What mallory does to the memory it was given as its store opened ([`Shared::memories`]), its
/// tenant's arena (0), which it may write, or the public one (1), which it may only read: what
/// the daemon maps of them it must go on reaching, whatever the VM process does.
#[derive(Clone, Copy, Debug)]
enum Abuse {
    /// Shrinks it to so many bytes.
    Shrink { which: u8, to: u64 },
    /// Grows it by a tebibyte.
    Grow { which: u8 },
    /// Punches a hole of `len` bytes at `at` in it.
    Punch { which: u8, at: u64, len: u64 },
    /// Writes its name at `at` in it.
    Scribble { which: u8, at: u64 },
}

/// What mallory does next: says something, or abuses its memory; or floods the daemon with
/// page-ins (TASK-149.34): the given point's extents, a MiB each, cold, a page of each mapped and
/// faulted at once, `times` over, as many faults in flight as it says.
#[derive(Clone, Debug)]
enum Act {
    Say(Said),
    Abuse(Abuse),
    PageIns { times: u8 },
}

/// How mallory acknowledges the stage operations it is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Acking {
    /// Each that asks for it, once made, as a VM process does.
    Honest,
    Never,
    /// All it will ever be sent, at the first it is.
    Early,
    /// Each as it comes, past it, and before it again.
    Forged,
}

/// A message mallory says, `repeat` times, each of a window's `PRIVATE`s, `UNMAP`s and `FAULT`s
/// 7 pages on from the one before: a storm.
#[derive(Clone, Debug)]
struct Said {
    kind: u32,
    flags: u32,
    /// Its ID, if it has one of its own: an `ACK`'s, of the stage operation it acknowledges.
    id: Option<u64>,
    args: [Word; 5],
    payload: Payload,
    /// A descriptor it carries.
    area: Option<Area>,
    repeat: u16,
}

fn said(kind: u32, flags: u32, args: [Word; 5], payload: Payload) -> Said {
    Said { kind, flags, id: None, args, payload, area: None, repeat: 1 }
}

/// The store a connection of mallory's opens first, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opens {
    Nothing,
    /// One of the case's four, which another connection may have.
    Shared(u8),
    /// Its own, of no other connection's.
    Own,
    /// One somewhere else, out of its directory, or a name there it may not open.
    Elsewhere(Place),
    /// A restore of its own, of the point its VM manager made for it, as a restored VM opens its
    /// store, naming the point's branch.
    Restores,
    /// Two stores of its own at once, two OPENs in flight, of which a connection has one
    /// (TASK-150.5).
    Twice,
}

/// A connection of mallory's, all it does.
#[derive(Clone, Debug)]
struct Hostile {
    opens: Opens,
    /// The window it asks for first, and its private pages.
    window: (u64, u64),
    area: Option<Area>,
    acking: Acking,
    acts: Vec<Act>,
    /// Whether it waits for what it said to be answered before it goes: or hangs up mid-request.
    waits: bool,
    /// Whether it then floods the daemon with reads past its room, their replies unread.
    flood: bool,
    last_words: LastWords,
}

/// What alice does, through her one connection, as the protocol allows: each result as her model
/// says. What it picks among her extents, and where in her window, is reduced to what she has.
#[derive(Clone, Copy, Debug)]
enum AliceOp {
    /// Writes a new extent of so many blocks.
    Write { blocks: u8 },
    Commit { durable: bool, root: bool },
    /// Reads an extent she wrote, freed or not.
    Read { pick: usize, block: u8, blocks: u8 },
    /// Frees an extent sealed.
    Free { pick: usize },
    /// Maps pages of an extent not freed, sealed or not, into her window.
    Map { pick: usize, page: u8, pages: u8, at: u8 },
    Zeros { at: u8, pages: u8 },
    Unmap { at: u8, pages: u8 },
    /// A page of her window mapped, as her guest reads it.
    Load { at: u8 },
    /// A byte, as her guest stores it, to a page mapped, within her private pages.
    Store { at: u8, byte: u8 },
    Private { at: u8, pages: u8 },
    Root,
}

fn alice_op() -> impl Strategy<Value = AliceOp> {
    use AliceOp::*;
    prop_oneof![
        4 => (1u8..=8).prop_map(|blocks| Write { blocks }),
        2 => (any::<bool>(), any::<bool>()).prop_map(|(durable, root)| Commit { durable, root }),
        2 => (any::<usize>(), any::<u8>(), any::<u8>())
            .prop_map(|(pick, block, blocks)| Read { pick, block, blocks }),
        1 => any::<usize>().prop_map(|pick| Free { pick }),
        4 => (any::<usize>(), any::<u8>(), any::<u8>(), any::<u8>())
            .prop_map(|(pick, page, pages, at)| Map { pick, page, pages, at }),
        1 => (any::<u8>(), any::<u8>()).prop_map(|(at, pages)| Zeros { at, pages }),
        1 => (any::<u8>(), any::<u8>()).prop_map(|(at, pages)| Unmap { at, pages }),
        3 => any::<u8>().prop_map(|at| Load { at }),
        3 => (any::<u8>(), any::<u8>()).prop_map(|(at, byte)| Store { at, byte }),
        1 => (any::<u8>(), any::<u8>()).prop_map(|(at, pages)| Private { at, pages }),
        1 => Just(Root),
    ]
}

fn word() -> BoxedStrategy<Word> {
    let ps = page_size();
    prop_oneof![
        Just(Word::Is(0)),
        Just(Word::Is(1)),
        Just(Word::Is(ps)),
        Just(Word::Is(WINDOW_BYTES)),
        Just(Word::Is(WINDOW_BYTES - ps)),
        Just(Word::Is(MAX_WINDOW)),
        Just(Word::Is(MAX_WINDOW + ps)),
        Just(Word::Is(MAX_PAYLOAD)),
        Just(Word::Is(MAX_TRANSFER_AREA)),
        Just(Word::Is(1 << 60)),
        Just(Word::Own),
        Just(Word::Alice),
        Just(Word::Bob),
        Just(Word::Given),
        (1u64..16).prop_map(Word::Is),
        (0u64..64).prop_map(move |page| Word::Is(page * ps)),
        // Negative, as a signed word has it.
        (0u64..4096).prop_map(|back| Word::Is(u64::MAX - back)),
        any::<u64>().prop_map(Word::Is),
    ]
    .boxed()
}

fn place() -> BoxedStrategy<Place> {
    prop_oneof![
        4 => (0u8..4).prop_map(Place::Own),
        1 => Just(Place::Point),
        1 => Just(Place::Escaped),
        1 => Just(Place::Linked),
        1 => Just(Place::Alices),
        1 => Just(Place::AlicesOpen),
        1 => Just(Place::AlicesNew),
        1 => Just(Place::LinkedNew),
        1 => Just(Place::Bobs),
        1 => Just(Place::Relative),
        1 => Just(Place::Directory),
    ]
    .boxed()
}

fn area() -> BoxedStrategy<Area> {
    prop_oneof![
        1 => (0u8..2, prop_oneof![Just(1u64 << 20), Just(MAX_TRANSFER_AREA)]).prop_map(|(which, len)| Area::Given(which, len)),
        4 => Just(Area::Sealed(1 << 20)),
        2 => Just(Area::Sealed(MAX_TRANSFER_AREA)),
        1 => Just(Area::Sealed(MAX_TRANSFER_AREA + page_size())),
        1 => Just(Area::Unsealed(1 << 20)),
        1 => Just(Area::Short(1 << 20)),
        1 => Just(Area::Pipe),
    ]
    .boxed()
}

/// Anything mallory says.
fn message() -> BoxedStrategy<Said> {
    use Word::*;
    let ps = page_size();
    const KINDS: [u32; 32] = [
        OPEN, READ, WRITE, FREE, MAP, UNMAP, COMMIT, ROOT, INFO, WINDOW, FAULT, ACK, FORK, RESIZED,
        MAPPINGS, ADVANCE, LET_GO, PIN, UNPIN, TRANSFER, FREE_MANY, STAGE_COUNTS, BUILD, PRIVATE,
        REPLY, STAGE_MAP, STAGE_UNMAP, STAGE_PRIVATE, CAPACITY, COMMITTED, 29, 0xffff_ffff,
    ];
    let page = move || (0u64..WINDOW_BYTES / ps).prop_map(move |page| Is(page * ps));
    let pages = move || (1u64..16).prop_map(move |pages| Is(pages * ps));
    let sequence = || (1u64..16).prop_map(Is);
    let theirs = || prop_oneof![Just(Alice), Just(Bob)];
    let transferred = || prop_oneof![Just(Is(0)), Just(Is(ps)), Just(Is(1 << 20)), Just(Is(MAX_TRANSFER_AREA))];
    // Requests of its own store and window, as far as their extents and pages go.
    let plausible = prop_oneof![
        (sequence(), page(), 1usize..8).prop_map(move |(extent, at, count)| {
            said(WRITE, 0, [Own, extent, at, Is(0), Is(0)], Payload::Filled(0x5a, count * ps as usize))
        }),
        (sequence(), page(), pages()).prop_map(|(extent, at, len)| {
            said(READ, 0, [Own, extent, at, len, Is(0)], Payload::Bytes(Vec::new()))
        }),
        // Of any length, anywhere, as a commit can't write the extent left of it (TASK-150.7).
        (sequence(), prop_oneof![Just(0), Just(1), Just(511), Just(ps - 1)], 1usize..200).prop_map(
            |(extent, at, len)| {
                said(WRITE, 0, [Own, extent, Is(at), Is(0), Is(0)], Payload::Filled(0x77, len))
            }
        ),
        (sequence(), page(), pages(), page(), 0u32..4).prop_map(|(extent, at, len, to, flags)| {
            said(MAP, flags, [Own, extent, at, len, to], Payload::Bytes(Vec::new()))
        }),
        (page(), pages()).prop_map(|(at, len)| said(UNMAP, 0, [at, len, Is(0), Is(0), Is(0)], Payload::Bytes(Vec::new()))),
        (page(), 0u32..8).prop_map(|(at, flags)| said(FAULT, flags, [at, Is(0), Is(0), Is(0), Is(0)], Payload::Bytes(Vec::new()))),
        (page(), pages()).prop_map(|(at, len)| said(PRIVATE, 0, [at, len, Is(0), Is(0), Is(0)], Payload::Bytes(Vec::new()))),
        sequence().prop_map(|extent| said(FREE, 0, [Own, extent, Is(0), Is(0), Is(0)], Payload::Bytes(Vec::new()))),
        (0u32..4, any::<bool>()).prop_map(|(flags, root)| {
            let payload = if root && flags & COMMIT_ROOT != 0 { b"mallory".to_vec() } else { Vec::new() };
            said(COMMIT, flags, [Is(0); 5], Payload::Bytes(payload))
        }),
        Just(said(MAPPINGS, 0, [Is(0); 5], Payload::Bytes(Vec::new()))),
        Just(said(INFO, 0, [Is(0); 5], Payload::Bytes(Vec::new()))),
    ];
    // Storms of the window's own requests, and of transfers as large as its area takes.
    let storms = prop_oneof![
        (prop_oneof![Just(PRIVATE), Just(UNMAP), Just(FAULT)], page(), pages(), 16u16..128).prop_map(
            |(kind, at, len, repeat)| {
                let flags = if kind == FAULT { FAULT_WRITE } else { 0 };
                Said { repeat, ..said(kind, flags, [at, len, Is(0), Is(0), Is(0)], Payload::Bytes(Vec::new())) }
            }
        ),
        (sequence(), transferred(), transferred(), 2u16..24).prop_map(|(extent, at, len, repeat)| {
            Said { repeat, ..said(READ, READ_TRANSFER, [Own, extent, Is(0), len, at], Payload::Bytes(Vec::new())) }
        }),
        (sequence(), transferred(), transferred(), 2u16..24).prop_map(|(extent, at, len, repeat)| {
            Said { repeat, ..said(WRITE, WRITE_TRANSFER, [Own, extent, Is(0), at, len], Payload::Bytes(Vec::new())) }
        }),
    ];
    // What names another tenant's: their extents, branches and stores.
    let reaching = prop_oneof![
        1 => (theirs(), sequence(), pages()).prop_map(|(whose, extent, len)| {
            said(READ, 0, [whose, extent, Is(0), len, Is(0)], Payload::Bytes(Vec::new()))
        }),
        1 => (theirs(), sequence(), page()).prop_map(move |(whose, extent, to)| {
            said(MAP, 0, [whose, extent, Is(0), Is(ps), to], Payload::Bytes(Vec::new()))
        }),
        1 => (theirs(), sequence(), prop_oneof![Just(FREE), Just(PIN), Just(UNPIN)]).prop_map(|(whose, extent, kind)| {
            said(kind, 0, [whose, extent, Is(0), Is(0), Is(0)], Payload::Bytes(Vec::new()))
        }),
        1 => (theirs(), sequence()).prop_map(move |(whose, extent)| {
            said(WRITE, 0, [whose, extent, Is(0), Is(0), Is(0)], Payload::Filled(0x66, ps as usize))
        }),
        1 => (theirs(), proptest::collection::vec((theirs(), 1u64..16), 0..8)).prop_map(|(whose, rest)| {
            said(FREE_MANY, 0, [whose, Is(1), Is(0), Is(0), Is(0)], Payload::Extents(rest))
        }),
        // Forks of its own store to another's branch, or its own.
        1 => (0u8..4, prop_oneof![Just(Alice), Just(Bob), Just(Given), Just(Own), any::<u64>().prop_map(Is)], 0u32..2)
            .prop_map(|(store, branch, flags)| {
                said(FORK, flags, [branch, Is(0), Is(0), Is(0), Is(0)], Payload::Path(Place::Own(store)))
            }),
        3 => (place(), prop_oneof![Just(0), Just(OPEN_CREATE), Just(OPEN_READ_ONLY)]).prop_map(|(at, flags)| {
            said(OPEN, flags, [Is(STORE_BYTES), Is(0), Is(0), Is(0), Is(VERSION)], Payload::Path(at))
        }),
        // Forks of its own store to anywhere it names.
        1 => (place(), 0u32..2).prop_map(|(to, flags)| {
            said(FORK, flags, [Is(0xf0f0), Is(0), Is(0), Is(0), Is(0)], Payload::Path(to))
        }),
        // Restores, of points of any branch it names, from anywhere.
        1 => ((0u8..4).prop_map(Place::Own), prop_oneof![2 => Just(Place::Nothing), 3 => place()], prop_oneof![Just(Alice), Just(Bob), Just(Given)])
            .prop_map(|(to, from, branch)| {
                said(OPEN, OPEN_FORK_FROM, [Is(STORE_BYTES), branch, Is(0), Is(0), Is(VERSION)], Payload::ForkFrom(to, from))
            }),
    ];
    // The protocol's own: acknowledgements forged, what its stage operations cost, its build, a
    // second window and transfer area, answers to asks never made.
    let abusing = prop_oneof![
        (prop_oneof![Just(0u64), Just(u64::MAX), 1u64..1024, any::<u64>()], [word(), word(), word(), word(), word()])
            .prop_map(|(id, args)| Said { id: Some(id), ..said(ACK, 0, args, Payload::Bytes(Vec::new())) }),
        [word(), word(), word(), word(), word()].prop_map(|args| said(STAGE_COUNTS, 0, args, Payload::Bytes(Vec::new()))),
        prop_oneof![Just(b"0123abcd".to_vec()), proptest::collection::vec(any::<u8>(), 0..80)]
            .prop_map(|commit| said(BUILD, 0, [Is(0); 5], Payload::Bytes(commit))),
        (area(), word()).prop_map(|(area, len)| Said { area: Some(area), ..said(TRANSFER, 0, [len, Is(0), Is(0), Is(0), Is(0)], Payload::Bytes(Vec::new())) }),
        (word(), word()).prop_map(|(size, private)| said(WINDOW, 0, [size, private, Is(0), Is(0), Is(0)], Payload::Bytes(Vec::new()))),
        (word(), 0u64..3).prop_map(|(capacity, took)| said(RESIZED, 0, [capacity, Is(took), Is(0), Is(0), Is(0)], Payload::Bytes(Vec::new()))),
    ];
    let anything = (
        proptest::sample::select(&KINDS[..]),
        prop_oneof![0u32..32, any::<u32>()],
        [word(), word(), word(), word(), word()],
        prop_oneof![
            4 => proptest::collection::vec(any::<u8>(), 0..64).prop_map(Payload::Bytes),
            1 => place().prop_map(Payload::Path),
        ],
        proptest::option::weighted(0.05, area()),
    )
        .prop_map(|(kind, flags, args, payload, area)| Said { area, ..said(kind, flags, args, payload) });
    prop_oneof![10 => plausible, 2 => storms, 4 => reaching, 2 => abusing, 3 => anything].boxed()
}

/// Anything mallory does: mostly what it says.
fn act() -> impl Strategy<Value = Act> {
    let ps = page_size();
    let which = || 0u8..2;
    let at = move || prop_oneof![Just(0u64), (0u64..1024).prop_map(move |page| page * ps), any::<u64>()];
    let abuse = prop_oneof![
        (which(), prop_oneof![Just(0u64), Just(ps), Just(physmem::CHUNK_SIZE as u64)])
            .prop_map(|(which, to)| Abuse::Shrink { which, to }),
        which().prop_map(|which| Abuse::Grow { which }),
        (which(), at(), prop_oneof![Just(ps), Just(physmem::CHUNK_SIZE as u64), Just(1u64 << 40)])
            .prop_map(|(which, at, len)| Abuse::Punch { which, at, len }),
        (which(), at()).prop_map(|(which, at)| Abuse::Scribble { which, at }),
    ];
    prop_oneof![
        30 => message().prop_map(Act::Say),
        1 => abuse.prop_map(Act::Abuse),
        1 => (1u8..8).prop_map(|times| Act::PageIns { times }),
    ]
}

fn hostile() -> impl Strategy<Value = Hostile> {
    let ps = page_size();
    let window = prop_oneof![4 => Just(WINDOW_BYTES), 1 => Just(MAX_WINDOW), 1 => Just(MAX_WINDOW + ps), 1 => Just(1 << 60)];
    let private_max = prop_oneof![1 => Just(0u64), 6 => 1u64..16, 1 => Just(u64::MAX)];
    let elsewhere = prop_oneof![
        Just(Place::Escaped),
        Just(Place::Linked),
        Just(Place::Alices),
        Just(Place::AlicesOpen),
        Just(Place::AlicesNew),
        Just(Place::LinkedNew),
        Just(Place::Bobs),
        Just(Place::Relative),
        Just(Place::Directory),
    ];
    let opens = prop_oneof![
        2 => Just(Opens::Nothing),
        2 => (0u8..4).prop_map(Opens::Shared),
        2 => Just(Opens::Own),
        1 => elsewhere.prop_map(Opens::Elsewhere),
        1 => Just(Opens::Restores),
        1 => Just(Opens::Twice),
    ];
    let acking = prop_oneof![
        8 => Just(Acking::Honest),
        1 => Just(Acking::Never),
        1 => Just(Acking::Early),
        2 => Just(Acking::Forged),
    ];
    let last_words = prop_oneof![
        Just(LastWords::Nothing),
        Just(LastWords::PartOfAHeader),
        any::<u64>().prop_map(LastWords::MissingPayload),
        (1u32..4).prop_map(LastWords::MissingDescriptors),
    ];
    (
        opens,
        (window, private_max),
        proptest::option::of(area()),
        acking,
        proptest::collection::vec(act(), 1..64),
        prop_oneof![3 => Just(true), 1 => Just(false)],
        proptest::bool::weighted(0.15),
        last_words,
    )
        .prop_map(|(opens, window, area, acking, mut acts, waits, flood, last_words)| {
            // A flood is of a connection that has a store of its own and waits for its answers,
            // whose room, of the window it asked for, is no more than a few hundred mebibytes: one
            // it doesn't change, as another window, asked for later, may.
            let flood = flood && opens == Opens::Own && waits && window.0 != MAX_WINDOW;
            if flood {
                acts.retain(|act| !matches!(act, Act::Say(said) if said.kind == WINDOW));
            }
            Hostile { opens, window, area, acking, acts, waits, flood, last_words }
        })
}

/// The bytes alice or bob write of `tag`, `len` of them: each 16 `mark` and a word of `tag` and
/// where they are.
fn marked(mark: [u8; 8], tag: u64, len: usize) -> Vec<u8> {
    (0..len.div_ceil(16) as u64)
        .flat_map(|unit| mark.into_iter().chain((tag << 24 | unit).to_le_bytes()))
        .take(len)
        .collect()
}

/// The `len` bytes at `addr`, as the kernel reads them, faulting as a vCPU's load does: `Err` with
/// the errno it failed with, as KVM_RUN does once a fault is refused.
fn load(addr: usize, len: usize) -> Result<Vec<u8>, i32> {
    let mut pipe = [0; 2];
    let mut bytes = vec![0u8; len];
    // SAFETY: Makes a pipe, writes the bytes at `addr` to it, reads them back, and closes it.
    unsafe {
        assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
        let written = libc::write(pipe[1], addr as *const libc::c_void, len);
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap();
        let loaded = match written {
            -1 => Err(errno),
            written if written as usize == len => {
                let read = libc::read(pipe[0], bytes.as_mut_ptr().cast(), len);
                assert_eq!(read, len as isize);
                Ok(bytes)
            }
            // Part of it: never, of a page alone.
            written => Err(-(written as i32)),
        };
        libc::close(pipe[0]);
        libc::close(pipe[1]);
        loaded
    }
}

/// `byte` stored at `addr`, as the kernel writes it, faulting for writing as KVM does for a vCPU:
/// `Err` with the errno it failed with.
fn store(addr: usize, byte: u8) -> Result<(), i32> {
    let mut pipe = [0; 2];
    // SAFETY: Makes a pipe, writes a byte to it, and reads it into `addr`, closing it after.
    unsafe {
        assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
        assert_eq!(libc::write(pipe[1], [byte].as_ptr().cast(), 1), 1);
        let read = libc::read(pipe[0], addr as *mut libc::c_void, 1);
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap();
        libc::close(pipe[0]);
        libc::close(pipe[1]);
        if read == 1 { Ok(()) } else { Err(errno) }
    }
}

/// An extent alice wrote and hasn't freed.
struct Kept {
    bytes: Vec<u8>,
    sealed: bool,
}

/// What a page of alice's window holds, as her model has it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Page {
    Unmapped,
    /// Page `page` of extent `sequence`, zeros past its end.
    Extent { sequence: u64, page: u64 },
    Zeros,
    Private(Vec<u8>),
}

/// Alice: a correct VM process, of a granted tenant, beside mallory's; her store, her window in
/// this process's memory, which copies on write, and her model of both.
struct Alice {
    store: RemoteExtentStore,
    window: NonNull<u8>,
    branch: u64,
    /// The sequence of her next extent.
    next: u64,
    extents: BTreeMap<u64, Kept>,
    freed: BTreeSet<u64>,
    pages: Vec<Page>,
    root: Vec<u8>,
}

/// Alice's result of `op` unless it is `expected`, saying both.
fn expect<T: std::fmt::Debug + PartialEq>(
    op: impl std::fmt::Debug,
    got: T,
    expected: T,
) -> Result<(), String> {
    if got == expected {
        Ok(())
    } else {
        Err(format!("alice's {op:?}: got {got:?}, her model {expected:?}"))
    }
}

/// A short form of bytes, for saying what differed.
fn summary(bytes: &Result<Vec<u8>, impl std::fmt::Debug>) -> String {
    match bytes {
        Ok(bytes) => format!("{} bytes, {:02x?}..", bytes.len(), &bytes[..bytes.len().min(24)]),
        Err(err) => format!("{err:?}"),
    }
}

impl Alice {
    /// Alice's store at `path`, through the daemon at `socket`, her window made, and her root
    /// committed.
    fn new(socket: &Path, path: &Path) -> Self {
        let ps = page_size() as usize;
        let store = RemoteExtentStore::connect(socket, path, 1 << 27).unwrap();
        let window = memory(ALICE_PAGES);
        // SAFETY: The memory is hers, and outlives the store, as it is never unmapped.
        unsafe {
            store.create_dax_window_in_host_memory(
                window,
                (ALICE_PAGES * ps) as u64,
                None,
                ALICE_PRIVATE as u64,
            )
        }
        .unwrap();
        crate::guest_thread_started();
        let root = marked(ALICE_MARK, u64::MAX >> 24, 64);
        store.commit(Some(&root)).unwrap();
        let branch = store.branch();
        let pages = vec![Page::Unmapped; ALICE_PAGES];
        Self { store, window, branch, next: 1, extents: BTreeMap::new(), freed: BTreeSet::new(), pages, root }
    }

    fn id(&self, sequence: u64) -> ExtentId {
        ExtentId { branch: self.branch, sequence }
    }

    fn page_addr(&self, page: usize) -> usize {
        self.window.as_ptr() as usize + page * page_size() as usize
    }

    /// What page `page` of her window holds, as her model has it: none if unmapped.
    fn page_bytes(&self, page: &Page) -> Option<Vec<u8>> {
        let ps = page_size() as usize;
        match page {
            Page::Unmapped => None,
            Page::Zeros => Some(vec![0; ps]),
            Page::Private(bytes) => Some(bytes.clone()),
            Page::Extent { sequence, page } => {
                let bytes = &self.extents[sequence].bytes;
                let at = (*page as usize * ps).min(bytes.len());
                let mut held = bytes[at..bytes.len().min(at + ps)].to_vec();
                held.resize(ps, 0);
                Some(held)
            }
        }
    }

    fn private_pages(&self) -> usize {
        self.pages.iter().filter(|page| matches!(page, Page::Private(_))).count()
    }

    /// The window's pages `at`, and so many after as `pages` says, within it.
    fn range(at: u8, pages: u8) -> std::ops::Range<usize> {
        let at = at as usize % ALICE_PAGES;
        at..at + (1 + pages as usize % 4).min(ALICE_PAGES - at)
    }

    /// Does `op`, checking what it returns against her model, which it then changes as `op` does.
    fn run(&mut self, op: AliceOp) -> Result<(), String> {
        let ps = page_size() as usize;
        match op {
            AliceOp::Write { blocks } => {
                let sequence = self.next;
                self.next += 1;
                let bytes = marked(ALICE_MARK, sequence, blocks as usize * BLOCK);
                expect(op, write(&self.store, self.id(sequence), 0, &bytes), Ok(()))?;
                self.extents.insert(sequence, Kept { bytes, sealed: false });
            }
            AliceOp::Commit { durable, root } => {
                let root = root.then(|| marked(ALICE_MARK, u64::MAX >> 24 ^ self.next, 64));
                let committed = match durable {
                    true => self.store.commit(root.as_deref()),
                    false => self.store.commit_ordered(root.as_deref()),
                };
                expect(op, committed, Ok(()))?;
                self.extents.values_mut().for_each(|kept| kept.sealed = true);
                if let Some(root) = root {
                    self.root = root;
                }
            }
            AliceOp::Read { pick, block, blocks } => {
                let written: Vec<u64> = self.extents.keys().chain(&self.freed).copied().collect();
                if written.is_empty() {
                    return Ok(());
                }
                let sequence = written[pick % written.len()];
                match self.extents.get(&sequence) {
                    Some(kept) => {
                        let count = kept.bytes.len() / BLOCK;
                        let block = block as usize % count;
                        let blocks = 1 + blocks as usize % (count - block);
                        let (at, len) = (block * BLOCK, blocks * BLOCK);
                        let got = read(&self.store, self.id(sequence), at as u64, len);
                        let expected = Ok(kept.bytes[at..at + len].to_vec());
                        if got != expected {
                            let (got, expected) = (summary(&got), summary(&expected));
                            return Err(format!("alice's {op:?} of {sequence}: got {got}, her model {expected}"));
                        }
                    }
                    None => {
                        let got = read(&self.store, self.id(sequence), 0, BLOCK).map(drop);
                        expect((op, sequence), got, Err(Errno::ENOENT))?;
                    }
                }
            }
            AliceOp::Free { pick } => {
                let sealed: Vec<u64> =
                    self.extents.iter().filter(|(_, kept)| kept.sealed).map(|(&s, _)| s).collect();
                if sealed.is_empty() {
                    return Ok(());
                }
                self.free(sealed[pick % sealed.len()])?;
            }
            AliceOp::Map { pick, page, pages, at } => {
                let live: Vec<u64> = self.extents.keys().copied().collect();
                if live.is_empty() {
                    return Ok(());
                }
                let sequence = live[pick % live.len()];
                let kept = &self.extents[&sequence];
                // A page past its end too, which maps zeros.
                let page = page as u64 % (kept.bytes.len().div_ceil(ps) as u64 + 1);
                let range = Self::range(at, pages);
                let (offset, len) = (page * ps as u64, (range.len() * ps) as u64);
                let mapped = self.store.map(self.id(sequence), offset, len, (range.start * ps) as u64);
                if !kept.sealed {
                    return expect((op, sequence), mapped, Err(Errno::EPERM));
                }
                expect((op, sequence), mapped, Ok(()))?;
                for (index, at) in range.enumerate() {
                    self.pages[at] = Page::Extent { sequence, page: page + index as u64 };
                }
            }
            AliceOp::Zeros { at, pages } => {
                let range = Self::range(at, pages);
                let (at, len) = ((range.start * ps) as u64, (range.len() * ps) as u64);
                let mapped = futures::executor::block_on(self.store.map_zeros_async(at, len));
                expect(op, mapped, Ok(()))?;
                range.for_each(|at| self.pages[at] = Page::Zeros);
            }
            AliceOp::Unmap { at, pages } => {
                let range = Self::range(at, pages);
                let unmapped =
                    self.store.unmap((range.start * ps) as u64, (range.len() * ps) as u64);
                expect(op, unmapped, Ok(()))?;
                range.for_each(|at| self.pages[at] = Page::Unmapped);
            }
            AliceOp::Load { at } => {
                // Of a page mapped: a load of one that isn't strays, which stops a VM.
                let mapped: Vec<usize> =
                    (0..ALICE_PAGES).filter(|&at| self.pages[at] != Page::Unmapped).collect();
                let Some(&at) = mapped.get(at as usize % mapped.len().max(1)) else {
                    return Ok(());
                };
                let got = load(self.page_addr(at), ps);
                let expected = self.page_bytes(&self.pages[at]).ok_or(libc::EFAULT);
                if got != expected {
                    let (got, expected) = (summary(&got), summary(&expected));
                    return Err(format!("alice's {op:?} of page {at}: got {got}, her model {expected}"));
                }
            }
            AliceOp::Store { at, byte } => {
                // Of a page mapped, within her private pages: a store past them is refused, as one
                // to a page mapped nothing at, and stops a VM.
                let room = self.private_pages() < ALICE_PRIVATE;
                let storable: Vec<usize> = (0..ALICE_PAGES)
                    .filter(|&at| match self.pages[at] {
                        Page::Unmapped => false,
                        Page::Private(_) => true,
                        Page::Extent { .. } | Page::Zeros => room,
                    })
                    .collect();
                let Some(&at) = storable.get(at as usize % storable.len().max(1)) else {
                    return Ok(());
                };
                let offset = byte as usize * 61 % ps;
                let stored = store(self.page_addr(at) + offset, byte);
                expect((op, at), stored, Ok(()))?;
                let mut bytes = self.page_bytes(&self.pages[at]).unwrap();
                bytes[offset] = byte;
                self.pages[at] = Page::Private(bytes);
            }
            AliceOp::Private { at, pages } => {
                let range = Self::range(at, pages);
                let made =
                    self.store.make_private((range.start * ps) as u64, (range.len() * ps) as u64);
                let needed =
                    range.clone().filter(|&at| !matches!(self.pages[at], Page::Private(_))).count();
                if range.clone().any(|at| self.pages[at] == Page::Unmapped) {
                    return expect(op, made, Err(Errno::EINVAL));
                }
                if self.private_pages() + needed > ALICE_PRIVATE {
                    return expect(op, made, Err(Errno::ENOMEM));
                }
                expect(op, made, Ok(()))?;
                for at in range {
                    self.pages[at] = Page::Private(self.page_bytes(&self.pages[at]).unwrap());
                }
            }
            AliceOp::Root => expect(op, self.store.root(), Ok(self.root.clone()))?,
        }
        Ok(())
    }

    /// Frees her extent `sequence`, sealed, whose mappings map zeros from then on.
    fn free(&mut self, sequence: u64) -> Result<(), String> {
        expect(("free", sequence), self.store.free(self.id(sequence)), Ok(()))?;
        self.extents.remove(&sequence);
        self.freed.insert(sequence);
        for page in &mut self.pages {
            if matches!(page, Page::Extent { sequence: mapped, .. } if *mapped == sequence) {
                *page = Page::Zeros;
            }
        }
        Ok(())
    }

    /// Begins a case of hers as she began the first, whatever came before, so that a case is
    /// replayed alone: every extent she has freed, committed, and her window unmapped.
    fn begin(&mut self) -> Result<(), String> {
        let ps = page_size();
        expect("a case's first commit", self.store.commit(None), Ok(()))?;
        self.extents.values_mut().for_each(|kept| kept.sealed = true);
        let kept: Vec<u64> = self.extents.keys().copied().collect();
        kept.into_iter().try_for_each(|sequence| self.free(sequence))?;
        self.freed.clear();
        let unmapped = self.store.unmap(0, ALICE_PAGES as u64 * ps);
        expect("a case's first unmap", unmapped, Ok(()))?;
        self.pages.fill(Page::Unmapped);
        Ok(())
    }

    /// `error`, at her op `index`, with what her window and model hold.
    fn explained(&self, index: usize, error: String) -> String {
        let model: Vec<String> = (self.pages.iter().enumerate())
            .filter(|(_, page)| **page != Page::Unmapped)
            .map(|(at, page)| match page {
                Page::Private(_) => format!("{at}: private"),
                page => format!("{at}: {page:?}"),
            })
            .collect();
        let sealed: Vec<(u64, bool)> =
            self.extents.iter().map(|(sequence, kept)| (*sequence, kept.sealed)).collect();
        format!(
            "{error}, at op {index}; her model's window {model:?}, extents {sealed:?}; the daemon's \
             mappings {:?}",
            self.store.window_mappings(),
        )
    }

    /// A case of hers, begun as her first was ([`Alice::begin`]): `ops`, and then her whole
    /// window as her guest reads it, a commit, and her latest extents as she reads them, each as
    /// her model says.
    fn case(&mut self, ops: &[AliceOp]) -> Result<(), String> {
        self.begin()?;
        for (index, op) in ops.iter().enumerate() {
            self.run(*op).map_err(|error| self.explained(index, error))?;
        }
        let ps = page_size() as usize;
        for at in 0..ALICE_PAGES {
            if let Some(expected) = self.page_bytes(&self.pages[at]) {
                let got = load(self.page_addr(at), ps);
                if got != Ok(expected.clone()) {
                    let (got, expected) = (summary(&got), summary(&Ok::<_, ()>(expected)));
                    return Err(format!("alice's window's page {at}: got {got}, her model {expected}"));
                }
            }
        }
        expect("the case's last commit", self.store.commit(None), Ok(()))?;
        self.extents.values_mut().for_each(|kept| kept.sealed = true);
        for (sequence, kept) in self.extents.iter().rev().take(8) {
            let got = read(&self.store, self.id(*sequence), 0, kept.bytes.len());
            if got.as_ref() != Ok(&kept.bytes) {
                return Err(format!("alice's extent {sequence}: got {}", summary(&got)));
            }
        }
        Ok(())
    }
}

/// What a case names, by its words and places.
struct Names {
    case: u64,
    vms: PathBuf,
    alices: PathBuf,
    alice: u64,
    bob: PathBuf,
    bobs: u64,
    given: u64,
    /// Mallory's tenant's arena, whose cached pages a flood of page-ins evicts first.
    arena: ArenaId,
}

impl Names {
    fn place(&self, place: Place) -> PathBuf {
        let alices = self.alices.file_name().unwrap();
        match place {
            Place::Own(store) => self.vms.join(format!("{}-{store}", self.case)),
            Place::Point => self.vms.join("point"),
            Place::Nothing => self.vms.join(format!("nothing-{}", self.case)),
            Place::Escaped => self.vms.join("..").join(alices).join("archive"),
            Place::Linked => self.vms.join("link").join("archive"),
            Place::Alices => self.alices.join("archive"),
            Place::AlicesOpen => self.alices.join("store"),
            Place::AlicesNew => self.alices.join(format!("new-{}", self.case)),
            Place::LinkedNew => self.vms.join("link").join(format!("new-{}", self.case)),
            Place::Bobs => self.bob.clone(),
            Place::Relative => PathBuf::from("archive"),
            Place::Directory => self.vms.clone(),
        }
    }

    fn word(&self, word: Word, own: u64) -> u64 {
        match word {
            Word::Is(word) => word,
            Word::Own => own,
            Word::Alice => self.alice,
            Word::Bob => self.bobs,
            Word::Given => self.given,
        }
    }

    fn payload(&self, payload: &Payload, own: u64) -> Vec<u8> {
        match payload {
            Payload::Bytes(bytes) => bytes.clone(),
            Payload::Filled(byte, len) => vec![*byte; *len],
            Payload::Path(place) => self.place(*place).as_os_str().as_bytes().to_vec(),
            Payload::ForkFrom(to, from) => {
                let (to, from) = (self.place(*to), self.place(*from));
                [to.as_os_str().as_bytes(), &[0], from.as_os_str().as_bytes()].concat()
            }
            Payload::Extents(extents) => (extents.iter())
                .flat_map(|(branch, sequence)| [self.word(*branch, own), *sequence])
                .flat_map(u64::to_le_bytes)
                .collect(),
        }
    }
}

/// A descriptor of `area`, `given` the memory the connection was given: none if it was given none.
fn area_fd(area: Area, given: &Mutex<Vec<OwnedFd>>) -> Option<OwnedFd> {
    let memfd = |len: u64, sealed: bool| {
        // SAFETY: Makes a descriptor, owned below.
        let fd = unsafe { libc::memfd_create(c"transfer".as_ptr(), libc::MFD_ALLOW_SEALING) };
        assert!(fd >= 0);
        // SAFETY: Just made, and owned here.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: Sizes it, and seals it if asked.
        unsafe {
            assert_eq!(libc::ftruncate(fd.as_raw_fd(), len as i64), 0);
            if sealed {
                assert_eq!(libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_SHRINK), 0);
            }
        }
        fd
    };
    Some(match area {
        Area::Sealed(len) => memfd(len, true),
        Area::Unsealed(len) => memfd(len, false),
        Area::Short(len) => memfd(len / 2, true),
        Area::Pipe => {
            let mut pipe = [0; 2];
            // SAFETY: Makes a pipe, whose ends are owned here, the write end closed.
            unsafe {
                assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
                drop(OwnedFd::from_raw_fd(pipe[1]));
                OwnedFd::from_raw_fd(pipe[0])
            }
        }
        Area::Given(which, _) => given.lock().unwrap().get(which as usize)?.try_clone().unwrap(),
    })
}

/// The bytes a `TRANSFER` of `area` says it has.
fn area_len(area: Area) -> u64 {
    match area {
        Area::Sealed(len) | Area::Unsealed(len) | Area::Short(len) | Area::Given(_, len) => len,
        Area::Pipe => 1 << 20,
    }
}

/// Does `abuse` to the memory the connection was `given`, if it was given any, whatever it
/// comes to: the kernel may refuse it.
fn abuse(abuse: Abuse, given: &Mutex<Vec<OwnedFd>>) {
    let given = given.lock().unwrap();
    let which = match abuse {
        Abuse::Shrink { which, .. }
        | Abuse::Grow { which }
        | Abuse::Punch { which, .. }
        | Abuse::Scribble { which, .. } => which,
    };
    let Some(fd) = given.get(which as usize) else { return };
    let fd = fd.as_raw_fd();
    let at = |at: u64| (at % (1 << 46)) as libc::off_t;
    // SAFETY: Plain system calls on a descriptor the connection holds, its memory's.
    unsafe {
        match abuse {
            Abuse::Shrink { to, .. } => libc::ftruncate(fd, to as libc::off_t),
            Abuse::Grow { .. } => {
                let mut stat: libc::stat = std::mem::zeroed();
                libc::fstat(fd, &mut stat);
                libc::ftruncate(fd, stat.st_size + (1 << 40))
            }
            Abuse::Punch { at: from, len, .. } => {
                let punch = libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE;
                libc::fallocate(fd, punch, at(from), len as libc::off_t)
            }
            Abuse::Scribble { at: to, .. } => {
                libc::pwrite(fd, b"mallory!".as_ptr().cast(), 8, at(to)) as libc::c_int
            }
        };
    }
}

/// The device and inode of `fd`'s file.
fn identity(fd: BorrowedFd<'_>) -> (u64, u64) {
    // SAFETY: A stat buffer the call fills, of a descriptor the caller holds.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) }, 0);
    (stat.st_dev as u64, stat.st_ino as u64)
}

/// Where in the memory `fd` has, read whole, a `mark` is: what of it holds data, as its holes hold
/// nothing.
fn find_marks(fd: BorrowedFd<'_>, marks: &[[u8; 8]]) -> Option<(u64, [u8; 8])> {
    const CHUNK: usize = 1 << 20;
    let raw = fd.as_raw_fd();
    let mut buffer = vec![0u8; CHUNK + 8];
    let mut at: i64 = 0;
    loop {
        // SAFETY: Seeks a descriptor the caller holds.
        let data = unsafe { libc::lseek(raw, at, libc::SEEK_DATA) };
        if data < 0 {
            return None;
        }
        // SAFETY: As above.
        let hole = unsafe { libc::lseek(raw, data, libc::SEEK_HOLE) };
        assert!(hole > data);
        let mut from = data;
        while from < hole {
            let len = (CHUNK as i64 + 8).min(hole - from) as usize;
            // SAFETY: Reads into the buffer, which has room.
            let read = unsafe { libc::pread(raw, buffer.as_mut_ptr().cast(), len, from) };
            assert_eq!(read, len as isize);
            for mark in marks {
                if let Some(found) = memchr::memmem::find(&buffer[..len], mark) {
                    return Some((from as u64 + found as u64, *mark));
                }
            }
            // The last bytes again, as a mark may span chunks.
            from += (len as i64 - 7).max(1);
        }
        at = hole;
    }
}

/// How a reply a connection of mallory's waits for was answered: its status, once it was.
#[derive(Default)]
struct Answers {
    statuses: Mutex<(HashMap<u64, i32>, bool)>,
    answered: Condvar,
}

impl Answers {
    fn answer(&self, id: u64, status: i32) {
        self.statuses.lock().unwrap().0.insert(id, status);
        self.answered.notify_all();
    }

    /// Nothing more is answered: the daemon let it go, or it hung up.
    fn end(&self) {
        self.statuses.lock().unwrap().1 = true;
        self.answered.notify_all();
    }

    /// The status request `id` was answered with, waiting for it: none if nothing more is.
    fn wait(&self, id: u64) -> Option<i32> {
        let statuses = self.statuses.lock().unwrap();
        let (statuses, _) = self
            .answered
            .wait_timeout_while(statuses, WAIT, |(statuses, ended)| !statuses.contains_key(&id) && !*ended)
            .unwrap();
        statuses.0.get(&id).copied()
    }
}

/// What a connection of mallory's got, and what of it breaks an invariant.
#[derive(Default)]
struct Seen {
    served: u64,
    restored: u64,
    /// What it got of another tenant's: replies and stage operations naming their branches or
    /// holding their bytes, arenas and memory not its tenant's or the public one.
    stolen: Vec<String>,
    /// Windows it was granted past the most the daemon makes room for.
    vast: Vec<u64>,
    /// The memory it got, as its stores opened.
    memories: Vec<OwnedFd>,
    /// Its flood's write's status, and whether the daemon let it go.
    flood: Option<(Option<i32>, bool)>,
    /// Its last commit's status, of the store it writes, if it opened one to write.
    committed: Option<Option<i32>>,
    /// The store it opened to write, if any.
    wrote: Option<PathBuf>,
    /// What each of its `OPEN`s named, and how it was answered: what its last commit's failure
    /// says.
    opens: Vec<(PathBuf, u32, i32)>,
    /// The extents its floods of page-ins mapped, to fault on.
    flooded: u64,
}

/// What both of a connection's threads share: what it sent, by ID, and what was answered.
#[derive(Default)]
struct Shared {
    sent: Mutex<HashMap<u64, Header>>,
    answers: Answers,
    own: AtomicU64,
    /// The memory it was given as its store opened: its tenant's arena, and the public one.
    memories: Mutex<Vec<OwnedFd>>,
    /// Whether it opened a store to write.
    writer: std::sync::atomic::AtomicBool,
    /// The paths its `OPEN`s named, by their IDs.
    paths: Mutex<HashMap<u64, PathBuf>>,
    /// The IDs of the `MAP`s of its floods of page-ins.
    flooding: Mutex<BTreeSet<u64>>,
    /// The largest window it was granted, as `LIMIT` was raised for it.
    granted: AtomicU64,
}

/// The tenants beside mallory's, what it may not reach of theirs.
struct Theirs {
    branches: [u64; 3],
    memories: [(u64, u64); 2],
    arenas: [ArenaId; 2],
}

/// Receives what the daemon sends a connection of mallory's, acknowledging its stage operations as
/// `acking` says, until it is let go or hangs up; or, if `stop_at_last`, until [`LAST`] is
/// answered, from when what the daemon sends it stays unread.
fn receive(
    mut receiver: Receiver,
    sender: Arc<Sender>,
    shared: Arc<Shared>,
    acking: Acking,
    stop_at_last: bool,
    theirs: Arc<Theirs>,
) -> Seen {
    let mut seen = Seen::default();
    let mut early = false;
    while let Ok(Some(message)) = receiver.receive(|| u64::MAX) {
        let header = message.header;
        let theirs_named = |word: &u64| theirs.branches.contains(word);
        let mut said = header.args.iter().any(theirs_named);
        // A window's mappings name their extents' branches.
        said |= message.payload.as_chunks::<8>().0.iter().map(|word| u64::from_le_bytes(*word)).any(|word| theirs_named(&word));
        if said {
            seen.stolen.push(format!("their branch named: {header:?}"));
        }
        for mark in [ALICE_MARK, BOB_MARK] {
            if memchr::memmem::find(&message.payload, &mark).is_some() {
                seen.stolen.push(format!("their bytes, of {header:?}"));
            }
        }
        // Each page it maps of its tenant's arena, or the public one, as KVM would map it.
        let arena = match header.kind {
            STAGE_MAP => Some(header.args[4]),
            STAGE_PRIVATE if header.args[2] == 1 => Some(header.args[4]),
            _ => None,
        };
        if let Some(arena) = arena
            && theirs.arenas.iter().any(|theirs| u64::from(theirs.0) == arena)
        {
            seen.stolen.push(format!("their arena mapped: {header:?}"));
        }
        for fd in &message.fds {
            if theirs.memories.contains(&identity(fd.as_fd())) {
                seen.stolen.push(format!("their memory given: {header:?}"));
            }
        }
        if header.kind == REPLY {
            let request = shared.sent.lock().unwrap().get(&header.id).copied();
            if header.status == 0 {
                seen.served += 1;
                match request {
                    Some(request) if request.kind == OPEN && request.flags != OPEN_READ_ONLY => {
                        shared.own.store(header.args[0], Ordering::Relaxed);
                        shared.writer.store(true, Ordering::Relaxed);
                        seen.wrote = shared.paths.lock().unwrap().get(&header.id).cloned();
                        seen.restored += u64::from(request.flags == OPEN_FORK_FROM);
                    }
                    Some(request) if request.kind == WINDOW => {
                        let size = request.args[0];
                        if size > MAX_WINDOW {
                            seen.vast.push(size);
                        } else {
                            // The connection's bound, and the heap's, are its room's of it.
                            let before = shared.granted.fetch_max(size, Ordering::Relaxed);
                            let more = (room(size.max(before)) - room(before)) as usize;
                            // Unless the case is over, and checks nothing.
                            let _ = LIMIT.try_update(Ordering::Relaxed, Ordering::Relaxed, |limit| {
                                (limit != 0).then_some(limit + more)
                            });
                        }
                    }
                    _ => {}
                }
                if request.is_some_and(|request| request.kind == OPEN) {
                    let fds = message.fds.iter().map(|fd| fd.try_clone().unwrap()).collect();
                    *shared.memories.lock().unwrap() = fds;
                    seen.memories.extend(message.fds);
                }
            }
            if let Some(request) = request.filter(|request| request.kind == OPEN) {
                let path = shared.paths.lock().unwrap().get(&header.id).cloned().unwrap_or_default();
                seen.opens.push((path, request.flags, header.status));
            }
            if header.status == 0 && shared.flooding.lock().unwrap().contains(&header.id) {
                seen.flooded += 1;
            }
            shared.answers.answer(header.id, header.status);
            if header.id == LAST && stop_at_last {
                return seen;
            }
        }
        if header.kind == REPLY || header.kind == COMMITTED || header.kind == CAPACITY {
            continue;
        }
        let ack = |id| {
            let _ = sender.send(Header::new(ACK, id), &[], &[]);
        };
        match acking {
            Acking::Honest if header.flags & STAGE_ACK != 0 => ack(header.id),
            Acking::Early if !early => {
                early = true;
                ack(u64::MAX);
            }
            Acking::Forged => {
                ack(header.id.wrapping_add(1000));
                ack(header.id);
                ack(header.id.saturating_sub(5));
            }
            Acking::Honest | Acking::Early | Acking::Never => {}
        }
    }
    shared.answers.end();
    seen
}

/// Runs `hostile`, one of the connections of mallory's case named `names`, at the daemon's
/// `socket`, starting what it says once every connection of the case is ready to (`start`).
fn run_hostile(
    hostile: Hostile,
    index: usize,
    names: Arc<Names>,
    socket: PathBuf,
    theirs: Arc<Theirs>,
    start: Arc<std::sync::Barrier>,
) -> Seen {
    let stream = UnixStream::connect(&socket).unwrap();
    let sender = Arc::new(Sender::new(stream.try_clone().unwrap()));
    let receiver = Receiver::new(stream.try_clone().unwrap());
    let shared = Arc::new(Shared::default());
    let receiving = {
        let (sender, shared) = (sender.clone(), shared.clone());
        std::thread::spawn(move || receive(receiver, sender, shared, hostile.acking, hostile.flood, theirs))
    };
    let mut next = 0;
    // Each request by its ID, which mallory gives each of its own: what an acknowledgement says
    // is the stage operation's.
    let send = |header: Header, payload: &[u8], fd: Option<OwnedFd>| -> Result<u64, std::io::Error> {
        if ![ACK, STAGE_COUNTS].contains(&header.kind) {
            shared.sent.lock().unwrap().insert(header.id, header);
        }
        // What an `OPEN` names, before a restore's NUL.
        if header.kind == OPEN {
            let named = payload.split(|byte| *byte == 0).next().unwrap_or_default();
            let named = PathBuf::from(std::ffi::OsStr::from_bytes(named));
            shared.paths.lock().unwrap().insert(header.id, named);
        }
        let fds: Vec<BorrowedFd<'_>> = fd.iter().map(|fd| fd.as_fd()).collect();
        sender.send(header, payload, &fds).map(|()| header.id)
    };
    let mut id = || {
        next += 1;
        next
    };
    let opened = match hostile.opens {
        Opens::Nothing => None,
        Opens::Shared(store) => Some(names.place(Place::Own(store))),
        Opens::Own => Some(names.vms.join(format!("{}-c{index}", names.case))),
        Opens::Elsewhere(place) => Some(names.place(place)),
        Opens::Restores | Opens::Twice => None,
    };
    if let Some(path) = opened {
        let open = Header::new(OPEN, id()).with_args(&[STORE_BYTES, 0, 0, 0, VERSION]).with_flags(OPEN_CREATE);
        if let Ok(id) = send(open, path.as_os_str().as_bytes(), None) {
            shared.answers.wait(id);
        }
    }
    if hostile.opens == Opens::Twice {
        let opens: Vec<_> = ["a", "b"]
            .map(|which| {
                let path = names.vms.join(format!("{}-t{index}{which}", names.case));
                let open = Header::new(OPEN, id()).with_args(&[STORE_BYTES, 0, 0, 0, VERSION]);
                send(open.with_flags(OPEN_CREATE), path.as_os_str().as_bytes(), None)
            })
            .into_iter()
            .collect();
        for open in opens.into_iter().flatten() {
            shared.answers.wait(open);
        }
    }
    if hostile.opens == Opens::Restores {
        let (to, point) = (names.vms.join(format!("{}-r{index}", names.case)), names.place(Place::Point));
        let named = [to.as_os_str().as_bytes(), &[0], point.as_os_str().as_bytes()];
        let restore = Header::new(OPEN, id())
            .with_args(&[STORE_BYTES, names.given, 0, 0, VERSION])
            .with_flags(OPEN_FORK_FROM);
        if let Ok(id) = send(restore, &named.concat(), None) {
            shared.answers.wait(id);
        }
    }
    let window = Header::new(WINDOW, id()).with_args(&[hostile.window.0, hostile.window.1]);
    let window = send(window, &[], None);
    if let Some(area) = hostile.area {
        let transfer = Header::new(TRANSFER, id()).with_args(&[area_len(area)]);
        let _ = send(transfer, &[], area_fd(area, &shared.memories));
    }
    let ps = page_size();
    // A flood of page-ins (TASK-149.34): its extents cold, mapped, if its store is a restore of the
    // point, and faulted on, `times` over, all at once, its replies left to come.
    let page_in = |id: &mut dyn FnMut() -> u64, times: u8| {
        super::super::super::cache::evict_least_used(Some(names.arena), |_| false);
        for sequence in 1..=GIVEN_EXTENTS {
            let at = (sequence - 1) * ps;
            let mapped = Header::new(MAP, id()).with_args(&[names.given, sequence, 0, ps, at]);
            shared.flooding.lock().unwrap().insert(mapped.id);
            let _ = send(mapped, &[], None);
        }
        for _ in 0..times {
            for sequence in 1..=GIVEN_EXTENTS {
                let at = (sequence - 1) * ps;
                let faulted = Header::new(FAULT, id()).with_args(&[at]).with_flags(FAULT_PAGE);
                let _ = send(faulted, &[], None);
            }
        }
    };
    // A restore of the point floods it with page-ins at once, as a restored VM's first faults do.
    if hostile.opens == Opens::Restores {
        // Once its window is, which its MAPs, served beside it, may otherwise come before.
        if let Ok(window) = window {
            shared.answers.wait(window);
        }
        page_in(&mut id, 2);
    }
    start.wait();
    for act in &hostile.acts {
        let said = match act {
            Act::Say(said) => said,
            Act::Abuse(abused) => {
                abuse(*abused, &shared.memories);
                continue;
            }
            Act::PageIns { times } => {
                page_in(&mut id, *times);
                continue;
            }
        };
        for time in 0..u64::from(said.repeat) {
            let own = shared.own.load(Ordering::Relaxed);
            let mut args = said.args.map(|word| names.word(word, own));
            if said.repeat > 1 && [PRIVATE, UNMAP, FAULT].contains(&said.kind) {
                args[0] = (args[0].wrapping_add(time * 7 * ps)) % WINDOW_BYTES;
            }
            let id = said.id.unwrap_or_else(&mut id);
            let header = Header::new(said.kind, id).with_args(&args).with_flags(said.flags);
            let payload = names.payload(&said.payload, own);
            // The daemon may have let it go by now.
            let sent = send(header, &payload, said.area.and_then(|area| area_fd(area, &shared.memories)));
            // What comes after a restore is said of the store it opened, as the daemon may take
            // longer over it than over all that comes after.
            if let Ok(id) = sent
                && said.kind == OPEN
                && said.flags == OPEN_FORK_FROM
            {
                shared.answers.wait(id);
            }
        }
    }
    let mut flood = None;
    let mut committed = None;
    if hostile.waits {
        // Whatever it said, the store it writes commits, as one used as the protocol allows does:
        // what the daemon took of it, it can commit (TASK-150.7). Once its OPENs are answered: one
        // waiting for its path, which a connection going holds, is served after a COMMIT sent
        // meanwhile, which finds no store (EBADF), as it rightly would.
        let opens: Vec<u64> = shared.paths.lock().unwrap().keys().copied().collect();
        for open in opens {
            shared.answers.wait(open);
        }
        let commit = Header::new(COMMIT, id()).with_flags(COMMIT_DURABLE);
        let status = send(commit, &[], None).ok().and_then(|id| shared.answers.wait(id));
        committed = shared.writer.load(Ordering::Relaxed).then_some(status);
        if hostile.flood {
            let own = shared.own.load(Ordering::Relaxed);
            let written = Header::new(WRITE, id()).with_args(&[own, FLOOD_EXTENT, 0]);
            let written = send(written, &vec![0x3c; FLOOD_READ as usize], None);
            flood = Some(written.ok().and_then(|id| shared.answers.wait(id)));
        }
        if let Ok(id) = send(Header::new(INFO, LAST), &[], None) {
            shared.answers.wait(id);
        }
    }
    let mut let_go = false;
    if flood.is_some() {
        // Past the room the window it was granted makes, by a few reads: none of their replies
        // read.
        let own = shared.own.load(Ordering::Relaxed);
        let reads = room(shared.granted.load(Ordering::Relaxed)) / FLOOD_READ + 64;
        for _ in 0..reads {
            let read = Header::new(READ, id()).with_args(&[own, FLOOD_EXTENT, 0, FLOOD_READ]);
            if sender.send(read, &[], &[]).is_err() {
                break;
            }
        }
        let mut hung_up =
            libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLRDHUP, revents: 0 };
        // SAFETY: Polls the stream, which this keeps open, for 30 s at most.
        let ready = unsafe { libc::poll(&mut hung_up, 1, 30_000) };
        let_go = ready == 1 && hung_up.revents & libc::POLLRDHUP != 0;
    }
    let raw = |bytes: &[u8]| {
        let _ = std::io::Write::write_all(&mut &stream, bytes);
    };
    let header = |len, fds| Header { kind: WRITE, len, fds, ..Default::default() };
    match hostile.last_words {
        LastWords::Nothing => {}
        LastWords::PartOfAHeader => raw(&header(0, 0).as_bytes()[..16]),
        LastWords::MissingPayload(len) => raw(header(len, 0).as_bytes()),
        LastWords::MissingDescriptors(fds) => raw(header(0, fds).as_bytes()),
    }
    // Shut down for writing too: the daemon may be sending it what it never reads.
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let mut seen = receiving.join().unwrap();
    seen.flood = flood.map(|written| (written, let_go));
    seen.committed = committed;
    seen
}

/// TASK-149.17: a compromised VM process of one tenant, saying anything, on connections at once,
/// harms no other tenant beside it, nor holds the daemon past the protocol's bounds, nor leaves
/// anything of its own behind: see the module's docs. Seeded by proptest, which says the case if
/// one fails: as many as `FXDAX_HOSTILE_CASES` says, 48 by default, from `PROPTEST_RNG_SEED`'s
/// seed if set.
#[test]
fn a_compromised_vm_process_harms_no_tenant_beside_it() {
    static PANICS: AtomicU64 = AtomicU64::new(0);
    let panicked = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICS.fetch_add(1, Ordering::Relaxed);
        panicked(info);
    }));

    host();
    let dir = tempfile::tempdir().unwrap();
    // Where the paths mallory names relative to the daemon's directory are, should a daemon take
    // them: not the crate's.
    std::env::set_current_dir(dir.path()).unwrap();
    let tenant = |name: &str| {
        let stores = dir.path().join(name);
        std::fs::create_dir(&stores).unwrap();
        let tenant = Tenant::new(name, &stores).unwrap();
        let socket = dir.path().join(format!("{name}.sock"));
        let listener = UnixListener::bind(&socket).unwrap();
        let served = tenant.clone();
        std::thread::spawn(move || serve_tenant(listener, served));
        (stores, socket, tenant)
    };
    let (vms, mallory_socket, mallory) = tenant("mallory");
    let (alices, alice_socket, alice_tenant) = tenant("alice");
    let bob_socket = dir.path().join("bob.sock");
    let listener = UnixListener::bind(&bob_socket).unwrap();
    std::thread::spawn(move || serve(listener));
    std::os::unix::fs::symlink(&alices, vms.join("link")).unwrap();

    // Alice's store she closed, which mallory names every way it can.
    let archive = alices.join("archive");
    let archived = {
        let store = RemoteExtentStore::connect(&alice_socket, &archive, STORE_BYTES).unwrap();
        let extent = ExtentId { branch: store.branch(), sequence: 1 };
        write(&store, extent, 0, &marked(ALICE_MARK, 0, 4 * BLOCK)).unwrap();
        store.commit(Some(b"archived")).unwrap();
        extent
    };
    // Bob's, the default tenant's, which waits.
    let bob_path = dir.path().join("bob");
    let bob = RemoteExtentStore::connect(&bob_socket, &bob_path, STORE_BYTES).unwrap();
    let bobs = ExtentId { branch: bob.branch(), sequence: 1 };
    write(&bob, bobs, 0, &marked(BOB_MARK, 1, BLOCK)).unwrap();
    bob.commit(None).unwrap();
    // A snapshot in the bucket, of a branch mallory's VM manager makes a point of in its directory.
    // Its extents a MiB each, which mallory's floods of page-ins page in (TASK-149.34).
    let given = RemoteExtentStore::connect(&bob_socket, dir.path().join("given"), STORE_BYTES).unwrap();
    let given_branch = given.branch();
    for sequence in 1..=GIVEN_EXTENTS {
        let extent = ExtentId { branch: given_branch, sequence };
        write(&given, extent, 0, &vec![0x88; 1 << 20]).unwrap();
    }
    given.commit(Some(b"given")).unwrap();
    drop(given);
    let point = vms.join("point");
    control(super::super::stores::create(point, STORE_BYTES, Some(given_branch), None)).unwrap();

    // Alice, on a thread of her own, her guest's, doing what each case asks of her.
    let (asking, asked) = std::sync::mpsc::channel::<Vec<AliceOp>>();
    let (answering, answered) = std::sync::mpsc::channel();
    let (ready, alice_ready) = std::sync::mpsc::channel();
    let (alice_path, socket) = (alices.join("store"), alice_socket.clone());
    std::thread::spawn(move || {
        let mut alice = Alice::new(&socket, &alice_path);
        ready.send((alice.branch, identity(alice.store.physical_memory()))).unwrap();
        for ops in asked {
            if answering.send(alice.case(&ops)).is_err() {
                break;
            }
        }
    });
    let (alice_branch, alice_memory) = alice_ready.recv_timeout(WAIT).unwrap();
    let theirs = Arc::new(Theirs {
        branches: [alice_branch, archived.branch, bob.branch()],
        memories: [alice_memory, identity(bob.physical_memory())],
        arenas: [alice_tenant.arena(), super::super::server::default_tenant().unwrap().arena()],
    });
    // What the others' directories hold, which nothing mallory says changes: alice's, and the
    // daemon's roots, bob's.
    let listing = |dir: &Path| -> BTreeSet<std::ffi::OsString> {
        std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().file_name()).collect()
    };
    let (alices_listed, root_listed) = (listing(&alices), listing(dir.path()));
    // Alice's and bob's, once those that made the others' stores are gone.
    let connected = 2;
    let settled = within(WAIT, move || {
        while connections() > connected {
            std::thread::park_timeout(Duration::from_millis(1));
        }
    });
    assert_eq!(settled, Ok(()));

    let cases: u32 = std::env::var("FXDAX_HOSTILE_CASES").map_or(48, |cases| cases.parse().unwrap());
    // A failing case shrinks for two minutes at most.
    let config =
        Config { cases, failure_persistence: None, max_shrink_time: 120_000, ..Config::default() };
    let numbered = AtomicU64::new(0);
    let (served, restored, floods) = (AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0));
    let commits = AtomicU64::new(0);
    let flooded = AtomicU64::new(0);
    let strategy = (
        proptest::collection::vec(hostile(), 1..=3),
        proptest::collection::vec(alice_op(), 0..32),
    );
    TestRunner::new(config)
        .run(&strategy, |(hostiles, ops)| {
            let case = numbered.fetch_add(1, Ordering::Relaxed);
            let names = Arc::new(Names {
                case,
                vms: vms.clone(),
                alices: alices.clone(),
                alice: alice_branch,
                bob: bob_path.clone(),
                bobs: bob.branch(),
                given: given_branch,
                arena: mallory.arena(),
            });
            *CASE.lock().unwrap() = format!("case {case}: {hostiles:#?}");
            // As much as the heap may hold, the rooms of mallory's connections raised as their
            // windows are granted.
            let limit = HELD.load(Ordering::Relaxed) as u64
                + SLACK
                + hostiles.len() as u64 * room(0);
            LIMIT.store(limit as usize, Ordering::Relaxed);

            // Mallory's connections at once, and alice beside them.
            let start = Arc::new(std::sync::Barrier::new(hostiles.len()));
            let running: Vec<_> = (hostiles.iter().cloned().enumerate())
                .map(|(index, hostile)| {
                    let (names, socket, theirs, start) =
                        (names.clone(), mallory_socket.clone(), theirs.clone(), start.clone());
                    let (done, finished) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = done.send(run_hostile(hostile, index, names, socket, theirs, start));
                    });
                    finished
                })
                .collect();
            asking.send(ops.clone()).unwrap();
            let alice = answered.recv_timeout(WAIT * 4);
            let seen: Vec<Seen> = (running.into_iter())
                .map(|finished| finished.recv_timeout(WAIT * 4))
                .collect::<Result<_, _>>()
                .expect("a connection of mallory's never ended");
            LIMIT.store(0, Ordering::Relaxed);
            prop_assert_eq!(alice, Ok(Ok(())));

            // Nothing of theirs reached it; nor was it given a window past the daemon's room.
            for (hostile, seen) in hostiles.iter().zip(&seen) {
                prop_assert_eq!(&seen.stolen, &Vec::<String>::new());
                prop_assert_eq!(&seen.vast, &Vec::<u64>::new());
                // It has one store at most, however many of its OPENs were at once (TASK-150.5).
                let opened = seen.opens.iter().filter(|(_, _, status)| *status == 0).count();
                prop_assert!(opened <= 1, "a connection opened {opened} stores: {:?}", seen.opens);
                // Its store, if it writes one, commits.
                if let Some(status) = seen.committed {
                    prop_assert_eq!(status, Some(0), "mallory's store didn't commit: {:?}", seen.opens);
                    commits.fetch_add(1, Ordering::Relaxed);
                }
                // A flood past its room is let go.
                if hostile.flood {
                    prop_assert_eq!(seen.flood, Some((Some(0), true)));
                    floods.fetch_add(1, Ordering::Relaxed);
                }
                served.fetch_add(seen.served, Ordering::Relaxed);
                flooded.fetch_add(seen.flooded, Ordering::Relaxed);
                restored.fetch_add(seen.restored, Ordering::Relaxed);
            }
            // Every memory it was given, read whole, holds nothing of theirs.
            let mut memories = BTreeMap::new();
            for fd in seen.iter().flat_map(|seen| &seen.memories) {
                memories.entry(identity(fd.as_fd())).or_insert(fd);
            }
            for fd in memories.values() {
                prop_assert_eq!(find_marks(fd.as_fd(), &[ALICE_MARK, BOB_MARK]), None);
            }

            // Every connection of its ends, and holds nothing.
            let ended = within(WAIT, move || {
                while connections() > connected {
                    std::thread::park_timeout(Duration::from_millis(1));
                }
            });
            let stayed = connections().saturating_sub(connected);
            prop_assert!(ended.is_ok(), "{stayed} connections of mallory's stayed");
            prop_assert_eq!(table().writers(), connected);
            let prefix = format!("{case}-");
            let mut named: Vec<_> = std::fs::read_dir(&vms)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with(&prefix))
                .collect();
            named.sort();
            for path in named {
                let opened = RemoteExtentStore::connect(&mallory_socket, &path, STORE_BYTES);
                let errno = opened.map(drop).err().and_then(|error| error.errno());
                prop_assert_ne!(errno, Some(Errno::EBUSY), "{} stayed held", path.display());
            }
            let nothing = vms.join(format!("nothing-{case}"));
            prop_assert!(!nothing.exists(), "a point of a branch named was taken");
            // Every store it opened to write opens again, and commits, whatever it said to it,
            // as far as it was served.
            let wrote: BTreeSet<&PathBuf> = seen.iter().filter_map(|seen| seen.wrote.as_ref()).collect();
            for path in wrote {
                let reopened = RemoteExtentStore::connect(&mallory_socket, path, STORE_BYTES)
                    .map_err(|error| error.to_string())
                    .map(|store| store.commit(None));
                prop_assert_eq!(reopened, Ok(Ok(())), "{} opened again", path.display());
            }
            // And the daemon serves on.
            let after = vms.join(format!("{case}-after"));
            let after = RemoteExtentStore::connect(&mallory_socket, &after, STORE_BYTES);
            prop_assert_eq!(after.map(drop).map_err(|error| error.to_string()), Ok(()));

            // Nothing came into the others' directories, nor went.
            prop_assert_eq!(listing(&alices), alices_listed.clone());
            prop_assert_eq!(listing(dir.path()), root_listed.clone());
            // Alice's store she closed holds what she wrote; bob's serves.
            let reopened = RemoteExtentStore::connect(&alice_socket, &archive, STORE_BYTES);
            let reopened = reopened.map_err(|error| error.to_string());
            let archived_bytes = reopened.as_ref().map(|store| read(store, archived, 0, 4 * BLOCK));
            prop_assert_eq!(archived_bytes, Ok(Ok(marked(ALICE_MARK, 0, 4 * BLOCK))));
            prop_assert_eq!(reopened.map(|store| store.root()), Ok(Ok(b"archived".to_vec())));
            prop_assert_eq!(read(&bob, bobs, 0, BLOCK), Ok(marked(BOB_MARK, 1, BLOCK)));
            let later = ExtentId { branch: bob.branch(), sequence: 2 + case };
            prop_assert_eq!(write(&bob, later, 0, &marked(BOB_MARK, 2 + case, BLOCK)), Ok(()));
            prop_assert_eq!(bob.commit(None), Ok(()));
            prop_assert_eq!(read(&bob, later, 0, BLOCK), Ok(marked(BOB_MARK, 2 + case, BLOCK)));
            // Freed, so that his store holds no more than a case's, however many cases run.
            prop_assert_eq!(bob.free(later), Ok(()));
            prop_assert_eq!(bob.commit(None), Ok(()));
            prop_assert_eq!(PANICS.load(Ordering::Relaxed), 0);
            Ok(())
        })
        .unwrap();
    // What it says reaches past the checks: not every request is refused, restores among them,
    // and floods are let go.
    let (served, restored, floods, commits) =
        (served.into_inner(), restored.into_inner(), floods.into_inner(), commits.into_inner());
    let flooded = flooded.into_inner();
    println!(
        "{served} of mallory's requests served, {restored} of them restores; {commits} of its \
         stores committed at the end; {floods} floods let go; {} extents cold, flooded with \
         page-ins",
        flooded,
    );
    assert!(served > 8 * u64::from(cases), "{served} requests served");
    assert!(restored > 0, "{restored} restores served");
    // And its stores open and commit: a VM process the daemon refused every opening of would pass
    // the rest, as one of another wire version did (TASK-141.3).
    assert!(commits > 0, "{commits} of its stores committed");
    // And its floods of page-ins page in cold extents.
    assert!(flooded > 0, "{flooded} extents flooded with page-ins");
    drop(mallory);
}

/// TASK-149.17: a VM process that shrinks the memory it is given as its store opens crashes
/// nothing: not its tenant's arena, which it may write, whose pages past a new end would fault the
/// daemon's own accesses (`SIGBUS`), and every tenant's VMs with it; nor the public one, which it
/// may only read. It may grow its arena, as the daemon goes on growing it. The memory it reads
/// alone is its owner's alone to open anew: a VM process, of another user, can't reopen it to
/// write through `/proc/self/fd`.
#[test]
fn vm_processes_shrinking_their_memory_crash_nothing() {
    host();
    let dir = tempfile::tempdir().unwrap();
    let vms = dir.path().join("vms");
    std::fs::create_dir(&vms).unwrap();
    let socket = dir.path().join("vms.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let tenant = Tenant::new("vms", &vms).unwrap();
    std::thread::spawn(move || serve_tenant(listener, tenant));
    let store = RemoteExtentStore::connect(&socket, vms.join("store"), 1 << 26).unwrap();
    let extent = |sequence| ExtentId { branch: store.branch(), sequence };
    write(&store, extent(1), 0, &super::pattern(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // In the extent cache, the arena's pages.
    assert_eq!(read(&store, extent(1), 0, BLOCK), Ok(super::pattern(1, BLOCK)));

    let (own, public) = (store.physical_memory().as_raw_fd(), store.public_memory().as_raw_fd());
    let truncated = |fd, len: i64| {
        // SAFETY: Sizes an object the test holds a descriptor of.
        match unsafe { libc::ftruncate(fd, len) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error().raw_os_error()),
        }
    };
    let shrunk = (truncated(own, 0), truncated(public, 0));
    // What the daemon reads of the arena's pages, which it would fault on, had the VM process
    // shrunk it.
    assert_eq!(read(&store, extent(1), 0, BLOCK), Ok(super::pattern(1, BLOCK)));
    assert_eq!(shrunk, (Err(Some(libc::EPERM)), Err(Some(libc::EINVAL))));
    // Grown, by a tebibyte, as the daemon grows it further, as its extents take more chunks.
    // SAFETY: A stat buffer the call fills, of a descriptor the test holds.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::fstat(own, &mut stat) }, 0);
    assert_eq!(truncated(own, stat.st_size + (1 << 40)), Ok(()));
    let extents = 2 * physmem::CHUNK_SIZE / (8 * BLOCK);
    for sequence in 2..2 + extents as u64 {
        write(&store, extent(sequence), 0, &super::pattern(sequence as u8, 8 * BLOCK)).unwrap();
    }
    store.commit(None).unwrap();
    for sequence in 1..2 + extents as u64 {
        let len = if sequence == 1 { BLOCK } else { 8 * BLOCK };
        assert_eq!(read(&store, extent(sequence), 0, len), Ok(super::pattern(sequence as u8, len)));
    }
    // SAFETY: As above.
    assert_eq!(unsafe { libc::fstat(public, &mut stat) }, 0);
    assert_eq!(stat.st_mode & 0o777, 0o600);
    drop(store);
}

/// TASK-149.17: what the daemon holds of a VM process's requests is within the connection's room
/// ([`room`]), whatever they ask: whole extents read and written, through the stream, as many at
/// once as the room holds, and through its transfer area, more at once than the connection serves,
/// their replies read as they come. Each running held a copy of its whole length, which the room
/// doesn't count, as a transfer's data isn't: 481 MiB held of reads, 1,024 of writes from the area.
/// And each `ROOT` copied a root as large as a payload: 768 MiB held of 16.
#[test]
fn what_requests_hold_is_within_the_room() {
    host();
    let dir = tempfile::tempdir().unwrap();
    // A bucket of files, as one in memory would hold what it is sent in the heap counted here.
    let bucket = format!("file://{}/", dir.path().canonicalize().unwrap().join("bucket").display());
    // SAFETY: nextest runs each test in a process of its own, whose other threads don't read the
    // environment yet.
    unsafe { std::env::set_var("FXDAX_EXTENTD_REMOTE", bucket) };
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    let (sender, mut receiver, _stream, branch) =
        super::raw_connection(&socket, &dir.path().join("store"), 1 << 33);
    let len = MAX_PAYLOAD;
    let area = area_fd(Area::Sealed(len), &Mutex::default()).unwrap();
    let transfer = Header::new(TRANSFER, 1).with_args(&[len]);
    sender.send(transfer, &[], &[area.as_fd()]).unwrap();
    let transferred = receiver.receive(|| 0).unwrap().unwrap().header;
    assert_eq!((transferred.id, transferred.status), (1, 0));
    let extent = |sequence| [branch, sequence];
    let written = Header::new(WRITE, 2).with_args(&extent(1));
    assert_eq!(super::call(&sender, &mut receiver, written, &vec![7; len as usize]).header.status, 0);
    let committed = Header::new(COMMIT, 3);
    assert_eq!(super::call(&sender, &mut receiver, committed, &[]).header.status, 0);
    // In the extent cache, so that what bringing it in holds isn't counted here.
    let warm = Header::new(READ, 4).with_args(&[branch, 1, 0, len]);
    assert_eq!(super::call(&sender, &mut receiver, warm, &[]).header.status, 0);
    // Its root, no larger than a root holds, read back as many at once as the connection serves:
    // one as large as a payload refused, as it would be copied into each reply, uncounted.
    let mut root = |id, len| {
        let rooted = Header::new(COMMIT, id).with_flags(COMMIT_ROOT);
        super::call(&sender, &mut receiver, rooted, &vec![5; len]).header.status
    };
    assert_eq!(root(5, len as usize), -Errno::EINVAL.raw());
    assert_eq!(root(6, MAX_ROOT), 0);
    // So many of each kind, `at_once` in flight, as many as the protocol allows: those of the
    // stream whose data the room holds, and those of the area all of them. Their replies are read
    // as they come.
    let mut burst = |kind: &str, headers: Vec<Header>, payload: &[u8], at_once: usize| {
        let count = headers.len();
        let held = peak_while(|| {
            let mut headers = headers.into_iter();
            for header in headers.by_ref().take(at_once) {
                sender.send(header, payload, &[]).unwrap();
            }
            for _ in 0..count {
                let reply = receiver.receive(|| 0).unwrap().unwrap();
                assert_eq!((reply.header.kind, reply.header.status), (REPLY, 0), "{kind}");
                if let Some(header) = headers.next() {
                    sender.send(header, payload, &[]).unwrap();
                }
            }
        });
        // The daemon's, and the reply this reads, which its receiver holds as it does.
        (kind.to_owned(), held <= room(0) + MAX_PAYLOAD, held >> 20)
    };
    let stream = (MAX_STREAM_BYTES / len) as usize;
    let many = 16;
    let reads = (0..many).map(|id| Header::new(READ, 10 + id).with_args(&[branch, 1, 0, len]));
    let into_area = (0..many)
        .map(|id| Header::new(READ, 100 + id).with_args(&[branch, 1, 0, len, 0]).with_flags(READ_TRANSFER));
    let from_area = (0..many).map(|id| {
        Header::new(WRITE, 200 + id).with_args(&[branch, 2 + id, 0, 0, len]).with_flags(WRITE_TRANSFER)
    });
    let writes = (0..many).map(|id| Header::new(WRITE, 300 + id).with_args(&[branch, 20 + id, 0]));
    let roots = (0..many).map(|id| Header::new(ROOT, 400 + id));
    let held = [
        burst("roots", roots.collect(), &[], many as usize),
        burst("reads", reads.collect(), &[], stream),
        burst("reads into the area", into_area.collect(), &[], many as usize),
        burst("writes from the area", from_area.collect(), &[], many as usize),
        burst("writes", writes.collect(), &vec![9; len as usize], stream),
    ];
    let within: Vec<_> = held.iter().map(|(kind, within, _)| (kind.as_str(), *within)).collect();
    assert_eq!(
        within,
        [("roots", true), ("reads", true), ("reads into the area", true), ("writes from the area", true), ("writes", true)],
        "MiB held past the room's {} MiB and a reply's: {held:?}",
        room(0) >> 20,
    );
}

/// TASK-149.34: the page-ins a connection's faults keep in flight, each a part of a cold extent
/// read into its tenant's memory, the part's frames held until its fetch from the bucket answers,
/// are bounded by the connection, not by how many faults it has in flight, as a fault holds no
/// worker while it waits for the pager (TASK-92.20). Here faults on a page of each of 256 extents
/// of a MiB, the snapshot's point of another host's, all in the bucket alone, whose reads are held.
#[test]
fn page_ins_in_flight_are_bounded() {
    use crate::{ExtentStore, VolatileSlice};
    const EXTENTS: u64 = 256;
    const MIB: u64 = 1 << 20;
    host();
    let gated = Arc::new(super::GatedReads::new());
    super::super::stores::use_bucket(extent_remote::store::Store::new(gated.clone(), "branches"));
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));
    // A point of the extents, made here, restored on another host, which has them in the bucket
    // alone.
    let (path, point) = (dir.path().join("store"), dir.path().join("point"));
    let branch = {
        let store = ExtentStore::create(&path, 1 << 30).unwrap();
        let branch = store.branch();
        for sequence in 1..=EXTENTS {
            let data = super::pattern(sequence as u8, MIB as usize);
            store.write(ExtentId { branch, sequence }, 0, &[VolatileSlice::from(&data[..])]).unwrap();
        }
        store.commit(None).unwrap();
        branch
    };
    let store = RemoteExtentStore::connect(&socket, &path, 1 << 30).unwrap();
    store.fork_quiescent(&point, branch + 1).unwrap();
    drop(store);
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let restored = elsewhere.join("store");
    let forked = RemoteExtentStore::connect_forked(
        &socket,
        &restored,
        elsewhere.join("point"),
        branch + 1,
        1 << 30,
    );
    drop(forked.unwrap());

    let (sender, receiver, _stream, _) = super::raw_connection(&socket, &restored, 1 << 30);
    let sender = Arc::new(sender);
    // Its replies, by ID, the stage operations before them acknowledged.
    let (replied, replies) = std::sync::mpsc::channel();
    let acking = sender.clone();
    std::thread::spawn(move || {
        let mut receiver = receiver;
        while let Ok(Some(message)) = receiver.receive(|| u64::MAX) {
            let header = message.header;
            if header.kind == REPLY {
                let _ = replied.send((header.id, header.status));
            } else if header.flags & STAGE_ACK != 0 {
                let _ = acking.send(Header::new(ACK, header.id), &[], &[]);
            }
        }
    });
    // The replies as they come, in any order, by ID.
    let answered = std::cell::RefCell::new(HashMap::new());
    let answer = |id| loop {
        if let Some(status) = answered.borrow_mut().remove(&id) {
            break status;
        }
        let (replied, status) =
            replies.recv_timeout(WAIT).unwrap_or_else(|_| panic!("no reply to request {id}"));
        answered.borrow_mut().insert(replied, status);
    };
    sender.send(Header::new(WINDOW, 2).with_args(&[EXTENTS * MIB, 1]), &[], &[]).unwrap();
    assert_eq!(answer(2), 0);
    for sequence in 1..=EXTENTS {
        let mapped = Header::new(MAP, 2 + sequence)
            .with_args(&[branch, sequence, 0, MIB, (sequence - 1) * MIB]);
        sender.send(mapped, &[], &[]).unwrap();
        assert_eq!(answer(2 + sequence), 0, "MAP {sequence}");
    }
    let arena = super::super::server::default_tenant().unwrap().arena();
    // What so many requests at once, all of them in flight, hold of the tenant's memory as their
    // page-ins wait for the bucket, and of the heap as their fetches answer: each a part, cold.
    let burst = |what: &str, headers: Vec<Header>| {
        super::super::super::cache::evict_least_used(Some(arena), |_| false);
        let (held, frames) = (HELD.load(Ordering::Relaxed), physmem::arena_bytes(arena));
        gated.hold(true);
        let ids: Vec<u64> = headers.iter().map(|header| header.id).collect();
        for header in headers {
            sender.send(header, &[], &[]).unwrap();
        }
        // Until no more page-ins begin.
        let mut paging = 0;
        for _ in 0..50 {
            std::thread::park_timeout(Duration::from_millis(200));
            let now = physmem::arena_bytes(arena).saturating_sub(frames);
            if now == paging && gated.waiting() > 0 {
                break;
            }
            paging = now;
        }
        let heap = HELD.load(Ordering::Relaxed).saturating_sub(held) as u64;
        let peak = peak_while(|| {
            gated.hold(false);
            for id in &ids {
                assert_eq!(answer(*id), 0, "{what} {id}");
            }
        });
        (what.to_owned(), paging >> 20, heap.max(peak) >> 20)
    };
    let faults = (1..=EXTENTS).map(|sequence| {
        let at = (sequence - 1) * MIB;
        Header::new(FAULT, 1000 + sequence).with_args(&[at]).with_flags(FAULT_PAGE)
    });
    // A page of each, through the stream.
    let reads = (1..=EXTENTS).map(|sequence| {
        Header::new(READ, 2000 + sequence).with_args(&[branch, sequence, 0, page_size()])
    });
    let held = [burst("faults", faults.collect()), burst("reads", reads.collect())];
    // A connection's page-ins, each a part and those it reads ahead, as many at once as it
    // resolves; and the heap within its room.
    let paged = (super::super::server::PAGE_INS as u64 * (1 + super::super::super::cache::PARTS_AHEAD)
        * extent_frame::PART) >> 20;
    let room = room(EXTENTS * MIB) >> 20;
    println!("MiB of the tenant's memory and of the heap held: {held:?}");
    let within: Vec<_> =
        held.iter().map(|(what, paging, heap)| (what.as_str(), *paging <= paged, *heap <= room)).collect();
    assert_eq!(
        within,
        [("faults", true, true), ("reads", true, true)],
        "MiB of the tenant's memory and of the heap held, past {paged} and {room}: {held:?}"
    );
}
