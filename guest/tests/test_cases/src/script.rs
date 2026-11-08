// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: scripts of operations on a few files, as fsx makes them, which a guest runs against a
//! model of their bytes (TASK-149.2): what both the host and the guest build. A script is a pure
//! function of a seed, so that the guest makes it from the seed alone, and a replay is told that
//! and the operations to drop (`DROP_VAR`, as the kernel's command line carries the guest's
//! environment), which is all a shrink has to say.
//!
//! Each operation stands alone, so that those around it can go: it carries the seed of its own
//! data, and says where it goes in terms the interpreter resolves against the model as it finds it
//! ([`Place`], [`Model::resolve`]), never what the generator took the file's size to be.
//!
//! This holds no system calls: [`Model`] says what an operation should come to, the guest's
//! `script_run` does it, and [`shrink`] and [`shrink_failure`] take the running of a script as an
//! argument, so that all of it is tested on any host.
//!
//! fxdax: the wide mix ([`WIDE`], TASK-44.15) adds what a VFS asks of a file beyond those: ranges
//! cloned and copied, zeroed and allocated, msyncs and fdatasyncs, stores through mappings made for
//! them, private ones too, O_DIRECT, a file's page cache dropped, and a file closed, evicted and
//! opened again. The model says each operation's outcome, its errno too, as the VFS's own checks
//! decide it ([`Model::clone_range`], [`Model::copy_range`]), and a reference filesystem in the
//! same guest holds the model to the kernel (`differential`).

use std::fmt;

/// The guest's seed of a script.
pub const SEED_VAR: &str = "FXDAX_RANDOM_OPS_SEED";
/// The operations to drop from a script, by their index in the whole of it: `12-40,100-110`.
pub const DROP_VAR: &str = "FXDAX_RANDOM_OPS_DROP";
/// How many runs of a script a shrink may take: FXDAX_SHRINK_TRIES's, or else this many, enough for
/// a pair of operations among thousands.
pub const TRIES: usize = 64;
/// The longest value `DROP_VAR` may have: it goes on the kernel's command line, of 2048 bytes,
/// beside the kernel's own arguments and the test's.
pub const LONGEST_DROPS: usize = 1200;

pub const PAGE: u64 = 4096;
const CHUNK: u64 = 2 << 20;
/// Where offsets and sizes are drawn from.
const MAX: u64 = 4 << 20;
/// The most a write writes.
const LONGEST: u64 = 16 << 10;
/// How far around the ends of things offsets are drawn.
const NEAR: u64 = 16 << 10;
/// The most a file holds: a write at MAX's last byte, of the most a write writes.
pub const MOST: u64 = MAX + LONGEST;
/// A whole-file check follows every so many operations, and the script's end.
const CHECK_EVERY: usize = 256;
/// Of the operations, one in this many has a clone after it, and a store in a child.
const CLONE_ONE_IN: u64 = 32;
const FORK_ONE_IN: u64 = 16;
/// fxdax: the most pages an O_DIRECT read or write takes.
const DIRECT_PAGES: u64 = 4;

/// fxdax: errnos the model says, as Linux numbers them (asm-generic/errno-base.h), which this
/// crate's builds on any host agree on.
pub mod errno {
    pub const EINVAL: i32 = 22;
    pub const ENOSPC: i32 = 28;
    pub const ENOMEM: i32 = 12;
    pub const EFAULT: i32 = 14;
    pub const EFBIG: i32 = 27;
}

/// fxdax: what an operation came to, or should: how many bytes it did (written, stored, read,
/// cloned or copied; 0 for an operation that says none), or the errno it failed with.
pub type Outcome = Result<u64, i32>;

/// The kind of failure a script's run saw, which a shrink keeps: the same bug may show at another
/// operation of a shorter script, but it should show the same way.
pub mod kind {
    /// A read, a file's size or a mapping isn't what the model says.
    pub const DIFFERS: &str = "differs";
    /// A write wrote less than it was given, with room to write it all.
    pub const SHORT_WRITE: &str = "short write";
    /// A store through a mapping, of a file's bytes, killed its process.
    pub const STORE_KILLED: &str = "store killed";
    /// The run on a nearly full filesystem was never refused anything.
    pub const NOTHING_REFUSED: &str = "nothing refused";
    /// The host's Fxfs doesn't read what the guest's model says.
    pub const STORED_DIFFERS: &str = "stored differs";
    /// The host's check of the image failed.
    pub const FSCK: &str = "fsck";
    /// The guest's kernel complained on its console.
    pub const COMPLAINT: &str = "complaint";
    /// The guest said neither that it passed nor what failed: its kernel went down, likely.
    pub const DIED: &str = "died";
    /// The VM ran out of time.
    pub const HANG: &str = "hang";
    /// fxdax: the reference filesystem doesn't do as the model says: the model's mistake, or the
    /// kernel's, not the filesystem's under test (`differential`).
    pub const REFERENCE_DIFFERS: &str = "reference differs";
    /// fxdax: the filesystems' timestamps changed otherwise, in ways not allowed (`differential`).
    pub const TIMES_DIFFER: &str = "times differ";
    /// fxdax: on a DAX mount, no file read in again was DAX: the run looked at the page cache.
    pub const NEVER_DAX: &str = "never DAX";
    /// fxdax: no time compared changed on either filesystem: the comparison looked at nothing.
    pub const NO_TIMES: &str = "no times changed";
    /// fxdax: after the VM was killed, a file holds what none of its states since its last fsync
    /// held ([`Survivals`], TASK-149.2).
    pub const KILLED_DIFFERS: &str = "killed differs";
}

/// fxdax: what a line of a guest's saying how the filesystems' times differed starts with
/// (`differential`), and what one allowed says before why.
pub const TIMES: &str = "times: ";
pub const ALLOWED: &str = " (allowed: ";

/// xorshift64*, as the random checks need no more.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// A number below `bound`.
    pub fn next(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % bound.max(1)
    }

    /// `len` random bytes.
    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next(256) as u8).collect()
    }
}

/// Where an operation goes, whatever else a script does: an offset of the file, or an offset from
/// its end, which the interpreter resolves against the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    At(u64),
    FromEnd(i64),
}

impl Place {
    fn resolve(self, size: u64) -> u64 {
        let at = match self {
            Place::At(at) => at,
            Place::FromEnd(by) => size.saturating_add_signed(by),
        };
        at.min(MAX - 1)
    }
}

/// An operation of a script. `data` seeds what it writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// write(2).
    Write { file: usize, place: Place, len: u64, data: u64 },
    /// An append.
    Append { file: usize, len: u64, data: u64 },
    /// Stores through the mapping kept of the file, within its size, by the process that keeps it
    /// or, `child`, by a child that inherited it.
    Store { file: usize, place: Place, len: u64, data: u64, child: bool },
    /// read(2), checked.
    Read { file: usize, place: Place, len: u64 },
    /// A truncation, shorter or longer.
    Truncate { file: usize, size: u64 },
    /// A hole punched, the size kept.
    Punch { file: usize, place: Place, len: u64 },
    /// fsync, a seal and a commit.
    Fsync { file: usize },
    /// FICLONE of a file onto another, whose bytes past the source's end stay.
    Clone { from: usize, to: usize },
    /// Every file read whole, through read(2) and its mapping, and checked.
    Check,
    /// fxdax: FICLONERANGE of `len` bytes, or to the source's end if 0, at `from_place` of `from`,
    /// onto `to_place` of `to`, which may be the same file: the offsets and length rounded down to
    /// blocks if `aligned`, as most are, and the rest refused, as the VFS refuses them.
    CloneRange {
        from: usize,
        from_place: Place,
        to: usize,
        to_place: Place,
        len: u64,
        aligned: bool,
    },
    /// fxdax: copy_file_range of `len` bytes, as many calls as it takes.
    Copy { from: usize, from_place: Place, to: usize, to_place: Place, len: u64 },
    /// fxdax: fallocate's FALLOC_FL_ZERO_RANGE, the size kept or grown to the range's end.
    Zero { file: usize, place: Place, len: u64, keep_size: bool },
    /// fxdax: fallocate's allocation, which leaves the bytes as they were, the size kept or grown.
    Allocate { file: usize, place: Place, len: u64, keep_size: bool },
    /// fxdax: msync(MS_SYNC) of the kept mapping's pages around `len` bytes at `place`.
    Msync { file: usize, place: Place, len: u64 },
    /// fxdax: fdatasync.
    Fdatasync { file: usize },
    /// fxdax: stores within the file's size through a mapping made for them and gone after:
    /// shared, or `private`, whose stores the file never sees.
    MapStore { file: usize, place: Place, len: u64, data: u64, private: bool },
    /// fxdax: an O_DIRECT write, or read, of `pages` pages at the page `place` falls in.
    Direct { file: usize, place: Place, pages: u64, data: u64, write: bool },
    /// fxdax: the file's page cache dropped (POSIX_FADV_DONTNEED), what the kept mapping maps too.
    Drop { file: usize },
    /// fxdax: the file closed, its kept mapping gone, its inode evicted, and the file opened and
    /// mapped again: on a DAX mount, a file read in again once its writes are sealed is DAX, its
    /// stores copying on write, where one created and kept open stays in the page cache.
    Reopen { file: usize },
    /// fxdax: a call with arguments no well-behaved caller passes (TASK-149.16), which the VFS
    /// refuses as it is, whatever the filesystem's largest file: an offset and a length past i64's
    /// end together, a negative offset, a length of nothing. `k` places it near the end.
    Limit { file: usize, call: Limit, k: u64, len: u64 },
    /// fxdax: a write, or a truncation, `k` bytes either side of `base`, far past what the model
    /// holds, on a scratch file of the run's own (`Run`'s far file): past a small DAX window's end,
    /// at ext4's largest file's end, a DAX mount's, i64's. What it comes to is the filesystem's
    /// largest file's, which a run is told ([`far`], `Options::largest`).
    Far { base: u64, k: i64, len: u64, data: u64, truncate: bool },
    /// fxdax: mmap of `len` bytes of the file at `place`, a page's start if `aligned`: refused
    /// (`EINVAL`) at no page's start, or of nothing; else read through, past the file's end too, to
    /// its last page's.
    MapOdd { file: usize, place: Place, len: u64, aligned: bool },
    /// fxdax: a new shared mapping of the file's pages at `place`, MAP_FIXED over the same pages of
    /// the kept mapping of `over`, which may be another file's, its stores through it within the
    /// file; `over`'s pages then mapped back.
    MapFixed { file: usize, over: usize, place: Place, len: u64, data: u64 },
    /// fxdax: a new mapping of the file's pages at `place`, remapped (mremap) twice as large, moved
    /// if it must, its stores through it within the file.
    Mremap { file: usize, place: Place, len: u64, data: u64 },
    /// fxdax: madvise of the kept mapping's pages at `place`: DONTNEED, which leaves the file, or
    /// REMOVE, which punches them; at no page's start if `odd`, `EINVAL`.
    Madvise { file: usize, place: Place, len: u64, remove: bool, odd: bool },
    /// fxdax: a truncation down to `size` and back, while a thread stores through the kept mapping
    /// below it.
    TruncateRacing { file: usize, size: u64, place: Place, len: u64, data: u64 },
    /// fxdax: a write(2) from a buffer whose bytes past its `good`th fault (TASK-149.16): it
    /// stops there, or short of it, at an offset of the file no page's start, as the copy got.
    FaultingWrite { file: usize, place: Place, len: u64, good: u64, data: u64 },
    /// fxdax: a write(2) from the kept mapping of its own file, `by` bytes on from where it writes:
    /// the same bytes, if 0, or bytes apart, in pages they may share. Not overlapping bytes, which
    /// a write copies as memcpy does, its result undefined: the guest's x86 copy wrote bytes of a
    /// line before it read them, which the source at 17 bytes on from them held (GCE r868).
    WriteFromMapping { file: usize, place: Place, len: u64, by: u64 },
    /// fxdax: the kept mapping read through, whole, so that each page the file has is mapped, then
    /// made read-only (mprotect) and writable again, all of it, whose PTEs the kernel changes in
    /// batches, then stored to within the file (TASK-149.42): to a page only read, as it comes,
    /// whose store must fault, which makes it private.
    ProtectKept { file: usize, place: Place, len: u64, data: u64 },
    /// fxdax: as [`Op::ProtectKept`], the kept mapping moved (mremap), its PTEs with it.
    MoveKept { file: usize, place: Place, len: u64, data: u64 },
}

/// fxdax: the calls of [`Op::Limit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// pwrite whose offset and length pass i64's end: `EINVAL` (rw_verify_area).
    WritePastI64,
    /// pwrite at a negative offset: `EINVAL`.
    WriteNegative,
    /// pread `k` bytes short of i64's end: `EINVAL` past it, nothing within it, past the largest
    /// file.
    ReadPastI64,
    /// ftruncate to a negative size: `EINVAL`.
    TruncateNegative,
    /// fallocate of nothing (`EINVAL`), at a negative offset (`EINVAL`), or past i64's end
    /// (`EFBIG`, vfs_fallocate).
    AllocateNothing,
    AllocateNegative,
    AllocatePastI64,
    /// copy_file_range from `k` bytes short of i64's end: nothing to copy.
    CopyFromPastI64,
    /// FICLONERANGE of a length negative as a loff_t: `EINVAL` (remap_verify_area).
    CloneNegative,
}

/// What a script's operations are drawn from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mix {
    /// The operations drawn: what `clones` and `forks` add, and the whole-file checks, are more.
    pub ops: usize,
    pub files: usize,
    pub clones: bool,
    pub forks: bool,
    /// fxdax: whether the operations are drawn from the wide mix's, [`wide_op`]'s, too.
    pub wide: bool,
    /// fxdax: whether hostile operations are drawn too, [`hostile_op`]'s (TASK-149.16).
    pub hostile: bool,
}

/// `random_ops`'s.
pub const ONE_FILE: Mix =
    Mix { ops: 3000, files: 1, clones: false, forks: false, wide: false, hostile: false };
/// `fxfs-guest-script`'s.
pub const SEVERAL_FILES: Mix =
    Mix { ops: 3000, files: 3, clones: true, forks: true, wide: false, hostile: false };
/// fxdax: `fxfs-guest-fsx`'s (TASK-44.15).
pub const WIDE: Mix = Mix { wide: true, ..SEVERAL_FILES };
/// fxdax: `fxfs-guest-fsx`'s since TASK-149.16: the wide mix, and hostile operations.
pub const HOSTILE: Mix = Mix { hostile: true, ..WIDE };

/// Where an operation goes: anywhere, or near a chunk's end, a page's, or the file's.
fn place(rng: &mut Rng) -> Place {
    let near = |rng: &mut Rng, end: u64| (end + rng.next(NEAR)).saturating_sub(NEAR / 2);
    match rng.next(4) {
        0 => Place::At(rng.next(MAX)),
        1 => Place::At(near(rng, CHUNK)),
        2 => {
            let page = rng.next(MAX / PAGE) * PAGE;
            Place::At(near(rng, page))
        }
        _ => Place::FromEnd(rng.next(NEAR) as i64 - (NEAR / 2) as i64),
    }
}

/// The script of `seed`.
pub fn generate(seed: u64, mix: Mix) -> Vec<Op> {
    let mut rng = Rng::new(seed);
    let mut script = Vec::with_capacity(mix.ops * 9 / 8);
    for op in 0..mix.ops {
        let file = rng.next(mix.files as u64) as usize;
        let place = place(&mut rng);
        let len = 1 + rng.next(LONGEST);
        let data = rng.next(u64::MAX);
        script.push(match rng.next(if mix.wide { WIDE_KINDS } else { 17 }) {
            0..=4 => Op::Write { file, place, len, data },
            5..=8 => Op::Store { file, place, len, data, child: false },
            9..=11 => Op::Read { file, place, len },
            12 => Op::Truncate { file, size: rng.next(MAX) },
            13 => Op::Punch { file, place, len },
            14 | 15 => Op::Fsync { file },
            16 => Op::Append { file, len, data },
            kind => wide_op(&mut rng, mix, kind, file, place, len, data),
        });
        if mix.clones && rng.next(CLONE_ONE_IN) == 0 {
            let from = rng.next(mix.files as u64) as usize;
            let to = (from + 1 + rng.next(mix.files as u64 - 1) as usize) % mix.files;
            script.push(Op::Clone { from, to });
        }
        if mix.hostile && rng.next(HOSTILE_ONE_IN) == 0 {
            script.push(hostile_op(&mut rng, mix));
        }
        if mix.forks && rng.next(FORK_ONE_IN) == 0 {
            let file = rng.next(mix.files as u64) as usize;
            let place = self::place(&mut rng);
            let (len, data) = (1 + rng.next(LONGEST), rng.next(u64::MAX));
            script.push(Op::Store { file, place, len, data, child: true });
        }
        if op % CHECK_EVERY == CHECK_EVERY - 1 {
            script.push(Op::Check);
        }
    }
    script
}

/// fxdax: the kinds of operation the wide mix draws from, one each: [`generate`]'s 17 first, in
/// their proportions, then [`wide_op`]'s.
const WIDE_KINDS: u64 = 32;

/// fxdax: the wide mix's operation of `kind`, one of [`generate`]'s draws past its own 17, on
/// `file` at `place`, of `len` bytes of `data`'s, as [`generate`] drew them: ranges cloned and copied
/// twice as often as the rest, between any two files, a file and itself too.
fn wide_op(
    rng: &mut Rng,
    mix: Mix,
    kind: u64,
    file: usize,
    place: Place,
    len: u64,
    data: u64,
) -> Op {
    let other = |rng: &mut Rng| rng.next(mix.files as u64) as usize;
    match kind {
        17 | 18 => {
            let (to, to_place) = (other(rng), self::place(rng));
            // A clone to the source's end, one in four; three in four of whole blocks.
            let len = if rng.next(4) == 0 { 0 } else { len };
            let aligned = rng.next(4) != 0;
            Op::CloneRange { from: file, from_place: place, to, to_place, len, aligned }
        }
        19 | 20 => Op::Copy {
            from: file,
            from_place: place,
            to: other(rng),
            to_place: self::place(rng),
            len,
        },
        21 => Op::Zero { file, place, len, keep_size: rng.next(2) == 0 },
        22 => Op::Allocate { file, place, len, keep_size: rng.next(2) == 0 },
        23 => Op::Msync { file, place, len },
        24 => Op::Fdatasync { file },
        25 | 26 => Op::MapStore { file, place, len, data, private: kind == 26 },
        27 | 28 => {
            let pages = 1 + rng.next(DIRECT_PAGES);
            Op::Direct { file, place, pages, data, write: kind == 27 }
        }
        29 => Op::Drop { file },
        30 => Op::Reopen { file },
        _ => Op::Truncate { file, size: rng.next(MAX) },
    }
}

/// fxdax: of the operations, one in this many has a hostile one after it, in the hostile mix.
const HOSTILE_ONE_IN: u64 = 4;

/// fxdax: where [`Op::Far`]'s offsets are drawn around: a file's offsets past a small DAX window's
/// end (512 MiB, 1 GiB, as a file larger than its window maps them in turn), ext4's largest file
/// (16 TiB less a block, 4 KiB blocks), a DAX mount's (i64's end less a chunk), i64's, and between
/// those (1 PiB).
const FAR: [u64; 6] = [
    512 << 20,
    1 << 30,
    (1 << 32) * PAGE - PAGE,
    (i64::MAX as u64) / CHUNK * CHUNK,
    i64::MAX as u64,
    1 << 50,
];

/// fxdax: a hostile operation (TASK-149.16), of [`Op`]'s last eleven, its arguments drawn here, or
/// a range of a file cloned or copied onto itself, overlapping.
fn hostile_op(rng: &mut Rng, mix: Mix) -> Op {
    let file = rng.next(mix.files as u64) as usize;
    let place = place(rng);
    let (len, data) = (1 + rng.next(LONGEST), rng.next(u64::MAX));
    match rng.next(14) {
        0 | 1 => {
            const CALLS: [Limit; 9] = [
                Limit::WritePastI64,
                Limit::WriteNegative,
                Limit::ReadPastI64,
                Limit::TruncateNegative,
                Limit::AllocateNothing,
                Limit::AllocateNegative,
                Limit::AllocatePastI64,
                Limit::CopyFromPastI64,
                Limit::CloneNegative,
            ];
            let call = CALLS[rng.next(CALLS.len() as u64) as usize];
            Op::Limit { file, call, k: rng.next(2 * len), len }
        }
        2 => {
            let base = FAR[rng.next(FAR.len() as u64) as usize];
            let k = rng.next(4 * LONGEST) as i64 - 2 * LONGEST as i64;
            Op::Far { base, k, len, data, truncate: rng.next(4) == 0 }
        }
        3 => {
            let len = if rng.next(8) == 0 { 0 } else { len };
            Op::MapOdd { file, place, len, aligned: rng.next(2) == 0 }
        }
        4 => Op::MapFixed { file, over: rng.next(mix.files as u64) as usize, place, len, data },
        5 => Op::Mremap { file, place, len, data },
        6 => Op::Madvise { file, place, len, remove: rng.next(2) == 0, odd: rng.next(8) == 0 },
        7 => Op::TruncateRacing { file, size: rng.next(MAX), place, len, data },
        // Appends as often as anything, a write past the end stopped short (TASK-133).
        8 | 9 => {
            let place = if rng.next(2) == 0 { Place::FromEnd(0) } else { place };
            Op::FaultingWrite { file, place, len, good: rng.next(len + 1), data }
        }
        // The same bytes one in four, else bytes apart, in a page they share or not, as it comes.
        10 => {
            let by = if rng.next(4) == 0 { 0 } else { rng.next(2 * len) };
            Op::WriteFromMapping { file, place, len, by }
        }
        12 => Op::ProtectKept { file, place, len, data },
        13 => Op::MoveKept { file, place, len, data },
        // A range cloned or copied onto itself, overlapping, or just not, unaligned as it comes.
        _ => {
            let shift = rng.next(2 * len + 1) as i64 - len as i64;
            let to_place = match place {
                Place::At(at) => Place::At(at.saturating_add_signed(shift)),
                Place::FromEnd(by) => Place::FromEnd(by + shift),
            };
            match rng.next(2) {
                0 => Op::Copy { from: file, from_place: place, to: file, to_place, len },
                _ => {
                    let aligned = rng.next(2) == 0;
                    Op::CloneRange {
                        from: file,
                        from_place: place,
                        to: file,
                        to_place,
                        len,
                        aligned,
                    }
                }
            }
        }
    }
}

/// `DROP_VAR`'s value: what isn't in `kept`, indices of a script of `len` operations, in order.
pub fn drops(kept: &[usize], len: usize) -> String {
    let mut ranges = Vec::new();
    let mut next = 0;
    for &index in kept.iter().chain([&len]) {
        match index - next {
            0 => {}
            1 => ranges.push(next.to_string()),
            _ => ranges.push(format!("{next}-{}", index - 1)),
        }
        next = index + 1;
    }
    ranges.join(",")
}

/// The indices of a script of `len` operations that `DROP_VAR`'s value `drops` leaves.
pub fn kept(drops: &str, len: usize) -> Result<Vec<usize>, String> {
    let mut dropped = vec![false; len];
    for range in drops.split(',').filter(|range| !range.is_empty()) {
        let (first, last) = range.split_once('-').unwrap_or((range, range));
        let index = |index: &str| index.parse().map_err(|err| format!("{range:?}: {err}"));
        let (first, last): (usize, usize) = (index(first)?, index(last)?);
        if first > last || last >= len {
            return Err(format!("{range:?} isn't among {len} operations"));
        }
        dropped[first..=last].fill(true);
    }
    Ok((0..len).filter(|&index| !dropped[index]).collect())
}

/// What an operation comes to on the files as the model has them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Write {
        file: usize,
        at: u64,
        data: Vec<u8>,
    },
    Store {
        file: usize,
        at: u64,
        data: Vec<u8>,
        child: bool,
    },
    Read {
        file: usize,
        at: u64,
        len: u64,
    },
    Truncate {
        file: usize,
        size: u64,
    },
    Punch {
        file: usize,
        at: u64,
        len: u64,
    },
    Fsync {
        file: usize,
    },
    Clone {
        from: usize,
        to: usize,
    },
    Check,
    /// fxdax: the wide mix's.
    CloneRange {
        from: usize,
        src: u64,
        to: usize,
        dst: u64,
        len: u64,
    },
    Copy {
        from: usize,
        src: u64,
        to: usize,
        dst: u64,
        len: u64,
    },
    Zero {
        file: usize,
        at: u64,
        len: u64,
        keep_size: bool,
    },
    Allocate {
        file: usize,
        at: u64,
        len: u64,
        keep_size: bool,
    },
    /// Of the kept mapping's pages from `at`'s, `at` a page's start, `len` bytes on.
    Msync {
        file: usize,
        at: u64,
        len: u64,
    },
    Fdatasync {
        file: usize,
    },
    MapStore {
        file: usize,
        at: u64,
        data: Vec<u8>,
        private: bool,
    },
    /// At a page's start, of whole pages.
    DirectWrite {
        file: usize,
        at: u64,
        data: Vec<u8>,
    },
    DirectRead {
        file: usize,
        at: u64,
        len: u64,
    },
    Drop {
        file: usize,
    },
    Reopen {
        file: usize,
    },
    /// fxdax: the hostile mix's (TASK-149.16). `at` as the call has it, negative too.
    Limit {
        file: usize,
        call: Limit,
        at: i64,
        len: u64,
    },
    /// Of the run's far file, empty before it and after.
    Far {
        at: u64,
        data: Vec<u8>,
        truncate: bool,
    },
    MapOdd {
        file: usize,
        at: u64,
        len: u64,
    },
    /// Through a mapping of `file`'s pages, MAP_FIXED over the same of `over`'s kept mapping.
    MapFixed {
        file: usize,
        over: usize,
        at: u64,
        data: Vec<u8>,
    },
    Mremap {
        file: usize,
        at: u64,
        data: Vec<u8>,
    },
    /// Of the kept mapping's pages from `at`'s, `at` a page's start, from a byte on if `odd`.
    Madvise {
        file: usize,
        at: u64,
        len: u64,
        remove: bool,
        odd: bool,
    },
    /// Down to `size` and back, while a child stores `data` at `at`, below both.
    TruncateRacing {
        file: usize,
        size: u64,
        at: u64,
        data: Vec<u8>,
    },
    /// Of `data`, its first `good` bytes readable, the rest past a fault.
    FaultingWrite {
        file: usize,
        at: u64,
        data: Vec<u8>,
        good: u64,
    },
    /// Of the kept mapping's `len` bytes at `src`, onto `dst`, at it or before it, within the file
    /// and apart from it: no byte both read and written but at the same offset.
    WriteFromMapping {
        file: usize,
        src: u64,
        dst: u64,
        len: u64,
    },
    /// Of the store after, at `at`, within the file.
    ProtectKept {
        file: usize,
        at: u64,
        data: Vec<u8>,
    },
    MoveKept {
        file: usize,
        at: u64,
        data: Vec<u8>,
    },
}

impl Step {
    /// fxdax: what kind of operation it is, as differences between filesystems are counted.
    pub fn kind(&self) -> &'static str {
        match self {
            Step::Write { .. } => "write",
            Step::Store { child: false, .. } => "store",
            Step::Store { child: true, .. } => "store by a child",
            Step::Read { .. } => "read",
            Step::Truncate { .. } => "truncate",
            Step::Punch { .. } => "punch",
            Step::Fsync { .. } => "fsync",
            Step::Clone { .. } => "clone",
            Step::Check => "check",
            Step::CloneRange { .. } => "clone range",
            Step::Copy { .. } => "copy",
            Step::Zero { keep_size: false, .. } => "zero range",
            Step::Zero { keep_size: true, .. } => "zero range, size kept",
            Step::Allocate { keep_size: false, .. } => "allocate",
            Step::Allocate { keep_size: true, .. } => "allocate, size kept",
            Step::Msync { .. } => "msync",
            Step::Fdatasync { .. } => "fdatasync",
            Step::MapStore { private: false, .. } => "store through a new mapping",
            Step::MapStore { private: true, .. } => "store through a private mapping",
            Step::DirectWrite { .. } => "direct write",
            Step::DirectRead { .. } => "direct read",
            Step::Drop { .. } => "drop the page cache",
            Step::Reopen { .. } => "reopen",
            Step::Limit { call, .. } => match call {
                Limit::WritePastI64 => "write past i64's end",
                Limit::WriteNegative => "write at a negative offset",
                Limit::ReadPastI64 => "read near i64's end",
                Limit::TruncateNegative => "truncate to a negative size",
                Limit::AllocateNothing => "allocate nothing",
                Limit::AllocateNegative => "allocate at a negative offset",
                Limit::AllocatePastI64 => "allocate past i64's end",
                Limit::CopyFromPastI64 => "copy from near i64's end",
                Limit::CloneNegative => "clone a negative length",
            },
            Step::Far { truncate: false, .. } => "write far",
            Step::Far { truncate: true, .. } => "truncate far",
            Step::MapOdd { .. } => "map oddly",
            Step::MapFixed { .. } => "store through a fixed mapping",
            Step::Mremap { .. } => "store through a remapped mapping",
            Step::Madvise { remove: false, .. } => "madvise DONTNEED",
            Step::Madvise { remove: true, .. } => "madvise REMOVE",
            Step::TruncateRacing { .. } => "truncate racing a store",
            Step::FaultingWrite { .. } => "write from a faulting buffer",
            Step::WriteFromMapping { .. } => "write from its own mapping",
            Step::ProtectKept { .. } => "mprotect the kept mapping",
            Step::MoveKept { .. } => "move the kept mapping",
        }
    }

    /// fxdax: what `file` is to it, as differences between filesystems are counted.
    pub fn role(&self, file: usize) -> Role {
        match *self {
            Step::Clone { from, to }
            | Step::CloneRange { from, to, .. }
            | Step::Copy { from, to, .. } => match (file == from, file == to) {
                (true, true) => Role::Its,
                (false, true) => Role::Destination,
                (true, false) => Role::Source,
                (false, false) => Role::Other,
            },
            Step::Write { file: its, .. }
            | Step::Store { file: its, .. }
            | Step::Read { file: its, .. }
            | Step::Truncate { file: its, .. }
            | Step::Punch { file: its, .. }
            | Step::Fsync { file: its }
            | Step::Zero { file: its, .. }
            | Step::Allocate { file: its, .. }
            | Step::Msync { file: its, .. }
            | Step::Fdatasync { file: its }
            | Step::MapStore { file: its, .. }
            | Step::DirectWrite { file: its, .. }
            | Step::DirectRead { file: its, .. }
            | Step::Drop { file: its }
            | Step::Reopen { file: its }
            | Step::Limit { file: its, .. }
            | Step::MapOdd { file: its, .. }
            | Step::MapFixed { file: its, .. }
            | Step::Mremap { file: its, .. }
            | Step::Madvise { file: its, .. }
            | Step::TruncateRacing { file: its, .. }
            | Step::FaultingWrite { file: its, .. }
            | Step::WriteFromMapping { file: its, .. }
            | Step::ProtectKept { file: its, .. }
            | Step::MoveKept { file: its, .. }
                if its == file =>
            {
                Role::Its
            }
            _ => Role::Other,
        }
    }

    /// fxdax: the file whose bytes or size it changes, if it changes one: a clone's or a copy's
    /// destination.
    pub fn changes(&self) -> Option<usize> {
        match *self {
            Step::Write { file, .. }
            | Step::Store { file, .. }
            | Step::Truncate { file, .. }
            | Step::Punch { file, .. }
            | Step::Zero { file, .. }
            | Step::Allocate { file, .. }
            | Step::MapStore { file, private: false, .. }
            | Step::DirectWrite { file, .. }
            | Step::MapFixed { file, .. }
            | Step::Mremap { file, .. }
            | Step::ProtectKept { file, .. }
            | Step::MoveKept { file, .. }
            | Step::Madvise { file, remove: true, .. }
            | Step::TruncateRacing { file, .. }
            | Step::FaultingWrite { file, .. }
            | Step::WriteFromMapping { file, .. } => Some(file),
            Step::Clone { to, .. } | Step::CloneRange { to, .. } | Step::Copy { to, .. } => {
                Some(to)
            }
            _ => None,
        }
    }

    /// fxdax: the bytes it changes, as the model has the files before it: the file, the range,
    /// and for a clone's or a copy's, where in which file the first came from.
    pub fn reach(&self, model: &Model) -> Option<Reach> {
        let size = |file: usize| model.files[file].len() as u64;
        let (file, start, end, source) = match *self {
            Step::Write { file, at, ref data }
            | Step::Store { file, at, ref data, .. }
            | Step::MapStore { file, at, ref data, private: false }
            | Step::DirectWrite { file, at, ref data }
            | Step::MapFixed { file, at, ref data, .. }
            | Step::Mremap { file, at, ref data }
            | Step::ProtectKept { file, at, ref data }
            | Step::MoveKept { file, at, ref data }
            | Step::FaultingWrite { file, at, ref data, .. } => {
                (file, at, at + data.len() as u64, None)
            }
            Step::TruncateRacing { file, size: to, at, ref data } => {
                let end = to.max(size(file)).max(at + data.len() as u64);
                (file, at.min(to), end, None)
            }
            Step::Madvise { file, at, len, remove: true, odd: false } => {
                (file, at, at + len.next_multiple_of(PAGE), None)
            }
            Step::WriteFromMapping { file, src, dst, len } => {
                (file, dst, dst + len, Some((file, src)))
            }
            Step::Truncate { file, size: to } => {
                (file, to.min(size(file)), to.max(size(file)), None)
            }
            Step::Punch { file, at, len }
            | Step::Zero { file, at, len, .. }
            | Step::Allocate { file, at, len, .. } => (file, at, at + len, None),
            Step::Clone { from, to } => (to, 0, size(from).next_multiple_of(PAGE), Some((from, 0))),
            Step::CloneRange { from, src, to, dst, len } => {
                let len = if len == 0 { size(from).saturating_sub(src) } else { len };
                (to, dst, dst + len.next_multiple_of(PAGE), Some((from, src)))
            }
            Step::Copy { from, src, to, dst, len } => (to, dst, dst + len, Some((from, src))),
            _ => return None,
        };
        Some(Reach { file, start, end, source })
    }

    /// fxdax: whether its timestamps are its own to say: not a store's through a shared mapping,
    /// whose times change at a write fault, which comes as the filesystem's cache has the page,
    /// written back or not, which filesystems differ in.
    pub fn says_its_times(&self) -> bool {
        !matches!(
            self,
            Step::Store { .. }
                | Step::MapStore { private: false, .. }
                | Step::MapFixed { .. }
                | Step::Mremap { .. }
                | Step::ProtectKept { .. }
                | Step::MoveKept { .. }
                | Step::TruncateRacing { .. }
        )
    }

    /// fxdax: whether its outcome is its filesystem's own, by its largest file ([`far`]), which
    /// filesystems differ in, as the model says each's.
    pub fn outcome_is_the_filesystems(&self) -> bool {
        matches!(self, Step::Far { .. })
    }
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Write { file, at, data } => {
                write!(f, "write {:#x} at {at:#x} of file {file}", data.len())
            }
            Step::Store { file, at, data, child } => {
                let by = if *child { " by a child" } else { "" };
                write!(f, "store {:#x} at {at:#x} of file {file}{by}", data.len())
            }
            Step::Read { file, at, len } => write!(f, "read {len:#x} at {at:#x} of file {file}"),
            Step::Truncate { file, size } => write!(f, "truncate file {file} to {size:#x}"),
            Step::Punch { file, at, len } => write!(f, "punch {len:#x} at {at:#x} of file {file}"),
            Step::Fsync { file } => write!(f, "fsync file {file}"),
            Step::Clone { from, to } => write!(f, "clone file {from} onto file {to}"),
            Step::Check => write!(f, "read the whole files"),
            Step::CloneRange { from, src, to, dst, len } => {
                write!(f, "clone {len:#x} at {src:#x} of file {from} onto {dst:#x} of file {to}")
            }
            Step::Copy { from, src, to, dst, len } => {
                write!(f, "copy {len:#x} at {src:#x} of file {from} to {dst:#x} of file {to}")
            }
            Step::Zero { file, at, len, keep_size } => {
                let kept = if *keep_size { ", the size kept" } else { "" };
                write!(f, "zero {len:#x} at {at:#x} of file {file}{kept}")
            }
            Step::Allocate { file, at, len, keep_size } => {
                let kept = if *keep_size { ", the size kept" } else { "" };
                write!(f, "allocate {len:#x} at {at:#x} of file {file}{kept}")
            }
            Step::Msync { file, at, len } => write!(f, "msync {len:#x} at {at:#x} of file {file}"),
            Step::Fdatasync { file } => write!(f, "fdatasync file {file}"),
            Step::MapStore { file, at, data, private } => {
                let how = if *private { "a private" } else { "a new" };
                write!(f, "store {:#x} at {at:#x} of file {file} through {how} mapping", data.len())
            }
            Step::DirectWrite { file, at, data } => {
                write!(f, "direct write {:#x} at {at:#x} of file {file}", data.len())
            }
            Step::DirectRead { file, at, len } => {
                write!(f, "direct read {len:#x} at {at:#x} of file {file}")
            }
            Step::Drop { file } => write!(f, "drop file {file}'s page cache"),
            Step::Reopen { file } => write!(f, "reopen file {file}"),
            Step::Limit { file, call, at, len } => {
                write!(f, "{call:?}: {len:#x} at {at:#x} of file {file}")
            }
            Step::Far { at, data, truncate: false } => {
                write!(f, "write {:#x} at {at:#x} of the far file", data.len())
            }
            Step::Far { at, truncate: true, .. } => write!(f, "truncate the far file to {at:#x}"),
            Step::MapOdd { file, at, len } => write!(f, "map {len:#x} at {at:#x} of file {file}"),
            Step::MapFixed { file, over, at, data } => write!(
                f,
                "store {:#x} at {at:#x} of file {file} through a mapping fixed over file {over}'s",
                data.len()
            ),
            Step::Mremap { file, at, data } => write!(
                f,
                "store {:#x} at {at:#x} of file {file} through a mapping remapped larger",
                data.len()
            ),
            Step::Madvise { file, at, len, remove, odd } => {
                let advice = if *remove { "REMOVE" } else { "DONTNEED" };
                let at = at + u64::from(*odd);
                write!(f, "madvise {advice} {len:#x} at {at:#x} of file {file}")
            }
            Step::TruncateRacing { file, size, at, data } => write!(
                f,
                "truncate file {file} to {size:#x} and back, while a child stores {:#x} at {at:#x}",
                data.len()
            ),
            Step::FaultingWrite { file, at, data, good } => write!(
                f,
                "write {:#x} at {at:#x} of file {file} from a buffer faulting after {good:#x}",
                data.len()
            ),
            Step::WriteFromMapping { file, src, dst, len } => {
                write!(f, "write {len:#x} at {dst:#x} of file {file} from its mapping at {src:#x}")
            }
            Step::ProtectKept { file, at, data } => write!(
                f,
                "read file {file}'s kept mapping, make it read-only and writable again, and store \
                 {:#x} at {at:#x}",
                data.len()
            ),
            Step::MoveKept { file, at, data } => write!(
                f,
                "read file {file}'s kept mapping, move it, and store {:#x} at {at:#x}",
                data.len()
            ),
        }
    }
}

/// fxdax: the bytes a step changes ([`Step::reach`]): `start` to `end` of `file`, and where the
/// first came from, a clone's or a copy's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reach {
    pub file: usize,
    pub start: u64,
    pub end: u64,
    pub source: Option<(usize, u64)>,
}

/// fxdax: what a file is to a step: the one it's on, a clone's or a copy's source or destination,
/// or another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Its,
    Source,
    Destination,
    Other,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Role::Its => "its file",
            Role::Source => "its source",
            Role::Destination => "its destination",
            Role::Other => "another file",
        })
    }
}

/// What a script's files should hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    pub files: Vec<Vec<u8>>,
}

impl Model {
    pub fn new(files: usize) -> Self {
        Self { files: vec![Vec::new(); files] }
    }

    fn size(&self, file: usize) -> u64 {
        self.files[file].len() as u64
    }

    /// What `op` comes to now.
    pub fn resolve(&self, op: &Op) -> Step {
        let bytes = |data: u64, len: u64| Rng::new(data).bytes(len as usize);
        match *op {
            Op::Write { file, place, len, data } => {
                let at = place.resolve(self.size(file));
                Step::Write { file, at, data: bytes(data, len) }
            }
            Op::Append { file, len, data } => {
                let at = self.size(file);
                Step::Write { file, at, data: bytes(data, len.min(MOST - at)) }
            }
            Op::Store { file, place, len, data, child } => {
                let (at, data) = self.within(file, place, len, data);
                Step::Store { file, at, data, child }
            }
            Op::Read { file, place, len } => {
                Step::Read { file, at: place.resolve(self.size(file)), len }
            }
            Op::Truncate { file, size } => Step::Truncate { file, size },
            Op::Punch { file, place, len } => {
                Step::Punch { file, at: place.resolve(self.size(file)), len }
            }
            Op::Fsync { file } => Step::Fsync { file },
            Op::Clone { from, to } => Step::Clone { from, to },
            Op::Check => Step::Check,
            Op::CloneRange { from, from_place, to, to_place, len, aligned } => {
                let block = |at: u64| if aligned { at / PAGE * PAGE } else { at };
                let (src, len) = (block(from_place.resolve(self.size(from))), block(len));
                let mut dst = block(to_place.resolve(self.size(to)));
                // A clone to the source's end, which may be all of it, lands where the file it
                // makes is no longer than a file may be.
                if len == 0 {
                    let whole = self.size(from).saturating_sub(src);
                    dst = block(dst.min(MOST.saturating_sub(whole)));
                }
                Step::CloneRange { from, src, to, dst, len }
            }
            Op::Copy { from, from_place, to, to_place, len } => {
                let src = from_place.resolve(self.size(from));
                Step::Copy { from, src, to, dst: to_place.resolve(self.size(to)), len }
            }
            Op::Zero { file, place, len, keep_size } => {
                Step::Zero { file, at: place.resolve(self.size(file)), len, keep_size }
            }
            Op::Allocate { file, place, len, keep_size } => {
                Step::Allocate { file, at: place.resolve(self.size(file)), len, keep_size }
            }
            Op::Msync { file, place, len } => {
                let at = place.resolve(self.size(file));
                Step::Msync { file, at: at / PAGE * PAGE, len: len + at % PAGE }
            }
            Op::Fdatasync { file } => Step::Fdatasync { file },
            Op::MapStore { file, place, len, data, private } => {
                let Step::Store { file, at, data, .. } =
                    self.resolve(&Op::Store { file, place, len, data, child: false })
                else {
                    unreachable!("a store resolves to a store")
                };
                Step::MapStore { file, at, data, private }
            }
            Op::Direct { file, place, pages, data, write } => {
                let at = place.resolve(self.size(file)) / PAGE * PAGE;
                let len = pages * PAGE;
                match write {
                    true => Step::DirectWrite { file, at, data: bytes(data, len) },
                    false => Step::DirectRead { file, at, len },
                }
            }
            Op::Drop { file } => Step::Drop { file },
            Op::Reopen { file } => Step::Reopen { file },
            Op::Limit { file, call, k, len } => {
                const END: i64 = i64::MAX;
                let at = match call {
                    // Past i64's end, by less than the length.
                    Limit::WritePastI64 | Limit::AllocatePastI64 => END - (k % len) as i64,
                    Limit::WriteNegative | Limit::TruncateNegative | Limit::AllocateNegative => {
                        -1 - k as i64
                    }
                    Limit::ReadPastI64 | Limit::CopyFromPastI64 => END - k as i64,
                    Limit::AllocateNothing | Limit::CloneNegative => k as i64,
                };
                let len = match call {
                    Limit::AllocateNothing => 0,
                    Limit::CloneNegative => len | 1 << 63,
                    _ => len,
                };
                Step::Limit { file, call, at, len }
            }
            Op::Far { base, k, len, data, truncate } => {
                Step::Far { at: base.saturating_add_signed(k), data: bytes(data, len), truncate }
            }
            Op::MapOdd { file, place, len, aligned } => {
                let at = place.resolve(self.size(file));
                Step::MapOdd { file, at: if aligned { at / PAGE * PAGE } else { at }, len }
            }
            Op::MapFixed { file, over, place, len, data } => {
                let (at, data) = self.within(file, place, len, data);
                Step::MapFixed { file, over, at, data }
            }
            Op::Mremap { file, place, len, data } => {
                let (at, data) = self.within(file, place, len, data);
                Step::Mremap { file, at, data }
            }
            Op::Madvise { file, place, len, remove, odd } => {
                let at = place.resolve(self.size(file)) / PAGE * PAGE;
                Step::Madvise { file, at, len, remove, odd }
            }
            Op::TruncateRacing { file, size, place, len, data } => {
                let below = size.min(self.size(file));
                let at = place.resolve(below).checked_rem(below).unwrap_or(0);
                Step::TruncateRacing { file, size, at, data: bytes(data, len.min(below - at)) }
            }
            Op::FaultingWrite { file, place, len, good, data } => {
                let at = place.resolve(self.size(file));
                Step::FaultingWrite { file, at, data: bytes(data, len), good }
            }
            Op::WriteFromMapping { file, place, len, by } => {
                let size = self.size(file);
                let src = place.resolve(size).checked_rem(size).unwrap_or(0);
                let dst = src - by.min(src);
                // As far as the source, if it's elsewhere, that no byte is both.
                let apart = if dst == src { u64::MAX } else { src - dst };
                Step::WriteFromMapping { file, src, dst, len: len.min(size - src).min(apart) }
            }
            Op::ProtectKept { file, place, len, data } => {
                let (at, data) = self.within(file, place, len, data);
                Step::ProtectKept { file, at, data }
            }
            Op::MoveKept { file, place, len, data } => {
                let (at, data) = self.within(file, place, len, data);
                Step::MoveKept { file, at, data }
            }
        }
    }

    /// Where, and what, a store of `len` bytes of `data`'s at `place` of `file` stores: within the
    /// file, wrapping round to its start, as much as it holds.
    fn within(&self, file: usize, place: Place, len: u64, data: u64) -> (u64, Vec<u8>) {
        let size = self.size(file);
        let at = place.resolve(size).checked_rem(size).unwrap_or(0);
        (at, Rng::new(data).bytes(len.min(size - at) as usize))
    }

    /// fxdax: what `step` should come to, done whole: what it does, or the errno it fails with.
    pub fn outcome(&self, step: &Step) -> Outcome {
        let read = |file: usize, at: u64, len: u64| len.min(self.size(file).saturating_sub(at));
        match *step {
            Step::Write { ref data, .. } | Step::DirectWrite { ref data, .. } => {
                Ok(data.len() as u64)
            }
            Step::Store { ref data, .. } | Step::MapStore { ref data, .. } => Ok(data.len() as u64),
            Step::Read { file, at, len } | Step::DirectRead { file, at, len } => {
                Ok(read(file, at, len))
            }
            Step::Clone { from, to } => self.clone_range(from, 0, to, 0, 0),
            Step::CloneRange { from, src, to, dst, len } => {
                self.clone_range(from, src, to, dst, len)
            }
            Step::Copy { from, src, to, dst, len } => self.copy_range(from, src, to, dst, len),
            Step::Limit { call, at, len, .. } => match call {
                Limit::ReadPastI64 if at.checked_add(len as i64).is_some() => Ok(0),
                Limit::CopyFromPastI64 => Ok(0),
                Limit::AllocatePastI64 => Err(errno::EFBIG),
                _ => Err(errno::EINVAL),
            },
            Step::Far { .. } => {
                unreachable!("a far step's outcome is its filesystem's: script::far")
            }
            Step::MapOdd { at, len, .. } if !at.is_multiple_of(PAGE) || len == 0 => {
                Err(errno::EINVAL)
            }
            Step::MapFixed { ref data, .. }
            | Step::Mremap { ref data, .. }
            | Step::ProtectKept { ref data, .. }
            | Step::MoveKept { ref data, .. }
            | Step::TruncateRacing { ref data, .. } => Ok(data.len() as u64),
            Step::Madvise { odd: true, .. } => Err(errno::EINVAL),
            Step::FaultingWrite { good: 0, .. } => Err(errno::EFAULT),
            Step::FaultingWrite { good, .. } => Ok(good),
            Step::WriteFromMapping { len, .. } => Ok(len),
            _ => Ok(0),
        }
    }

    /// fxdax: does what `step` does, done whole, as [`Model::outcome`] says it comes to.
    pub fn apply(&mut self, step: &Step) {
        match *step {
            Step::Write { file, at, ref data }
            | Step::Store { file, at, ref data, .. }
            | Step::MapStore { file, at, ref data, private: false }
            | Step::DirectWrite { file, at, ref data } => self.write(file, at, data),
            Step::Truncate { file, size } => self.truncate(file, size),
            Step::Punch { file, at, len } => self.punch(file, at, len),
            Step::Clone { from, to } => self.apply_clone(from, 0, to, 0, step),
            Step::CloneRange { from, src, to, dst, .. } | Step::Copy { from, src, to, dst, .. } => {
                self.apply_clone(from, src, to, dst, step)
            }
            Step::Zero { file, at, len, keep_size } => {
                self.punch(file, at, len);
                self.grow(file, at + len, keep_size);
            }
            Step::Allocate { file, at, len, keep_size } => self.grow(file, at + len, keep_size),
            Step::MapFixed { file, at, ref data, .. }
            | Step::Mremap { file, at, ref data }
            | Step::ProtectKept { file, at, ref data }
            | Step::MoveKept { file, at, ref data } => self.write(file, at, data),
            Step::Madvise { file, at, len, remove: true, odd: false } => {
                self.punch(file, at, len.next_multiple_of(PAGE))
            }
            Step::TruncateRacing { file, size, at, ref data } => {
                let was = self.size(file);
                if size < was {
                    self.truncate(file, size);
                    self.truncate(file, was);
                }
                self.write(file, at, data);
            }
            Step::FaultingWrite { file, at, ref data, good } => {
                self.write(file, at, &data[..good as usize])
            }
            Step::WriteFromMapping { file, src, dst, len } => {
                let bytes = self.files[file][src as usize..(src + len) as usize].to_vec();
                self.write(file, dst, &bytes);
            }
            _ => {}
        }
    }

    /// Of a clone or a copy, `step`: the bytes its outcome says, from `src` of `from` to `dst` of
    /// `to`, read before any is written, as a file and itself never overlap.
    fn apply_clone(&mut self, from: usize, src: u64, to: usize, dst: u64, step: &Step) {
        if let Ok(count @ 1..) = self.outcome(step) {
            let bytes = self.files[from][src as usize..(src + count) as usize].to_vec();
            self.write(to, dst, &bytes);
        }
    }

    /// Makes `file` `end` bytes long if it is shorter, unless the size is kept.
    fn grow(&mut self, file: usize, end: u64, keep_size: bool) {
        if !keep_size && end > self.size(file) {
            self.truncate(file, end);
        }
    }

    /// fxdax: what FICLONERANGE of `len` bytes, or to the source's end if 0, at `src` of `from` onto
    /// `dst` of `to` comes to, as the VFS checks it before a filesystem sees it, with no flags
    /// (fs/remap_range.c: generic_remap_file_range_prep, generic_remap_checks,
    /// generic_remap_check_len; and ioctl_file_clone): the bytes it clones, or EINVAL. FICLONE is
    /// one of the whole file, to 0. Blocks are pages, as Fxfs's, ext4's and XFS's are here.
    pub fn clone_range(&self, from: usize, src: u64, to: usize, dst: u64, len: u64) -> Outcome {
        let (size_in, size_out) = (self.size(from), self.size(to));
        // Zero is to the source's end, which may be nothing to clone.
        let len = match len {
            0 if src == size_in => return Ok(0),
            0 if src > size_in => return Err(errno::EINVAL),
            0 => size_in - src,
            len => len,
        };
        if !src.is_multiple_of(PAGE) || !dst.is_multiple_of(PAGE) || src >= size_in {
            return Err(errno::EINVAL);
        }
        let count = len.min(size_in - src);
        // To the source's end, its last block is taken whole; otherwise only whole blocks.
        let (count, blocks) = if src + count == size_in {
            (count, size_in.next_multiple_of(PAGE) - src)
        } else {
            let count = count / PAGE * PAGE;
            (count, count)
        };
        if from == to && dst + blocks > src && dst < src + blocks {
            return Err(errno::EINVAL);
        }
        // Shortened, and nothing asked that it may be.
        if count != len {
            return Err(errno::EINVAL);
        }
        // A partial block can't replace part of one of the destination's.
        if !count.is_multiple_of(PAGE) && dst + count < size_out {
            return Err(errno::EINVAL);
        }
        Ok(count)
    }

    /// fxdax: what copy_file_range of `len` bytes at `src` of `from` to `dst` of `to` comes to, as
    /// the VFS checks it (fs/read_write.c: generic_copy_file_checks), however many calls it takes:
    /// what lies within the source, or EINVAL for ranges of one file that overlap.
    pub fn copy_range(&self, from: usize, src: u64, to: usize, dst: u64, len: u64) -> Outcome {
        let count = len.min(self.size(from).saturating_sub(src));
        if from == to && dst + count > src && dst < src + count {
            return Err(errno::EINVAL);
        }
        Ok(count)
    }

    /// Writes `data` at `at` of `file`: nothing, if it's empty, which grows no file.
    pub fn write(&mut self, file: usize, at: u64, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let (at, end) = (at as usize, at as usize + data.len());
        let bytes = &mut self.files[file];
        if bytes.len() < end {
            bytes.resize(end, 0);
        }
        bytes[at..end].copy_from_slice(data);
    }

    pub fn truncate(&mut self, file: usize, size: u64) {
        self.files[file].resize(size as usize, 0);
    }

    pub fn punch(&mut self, file: usize, at: u64, len: u64) {
        let bytes = &mut self.files[file];
        let end = ((at + len) as usize).min(bytes.len());
        if (at as usize) < end {
            bytes[at as usize..end].fill(0);
        }
    }

    /// FICLONE of `from` onto `to`: what `to` holds past the source's end stays.
    pub fn clone_file(&mut self, from: usize, to: usize) {
        self.apply(&Step::Clone { from, to });
    }

    /// What `step`, refused for lack of room or memory, left of `file`: `got`, what it reads now.
    /// An operation of many transactions stands as far as it got (fxfs_vfs_remap_file_range,
    /// fxfs_dax_punch_begin). Takes `got` for `file`'s if `step` could have reached it: a write
    /// nothing, as one that writes any says how much; a whole file's clone its blocks in order,
    /// a size between; any other, each byte as it was or as the step done whole leaves it, and a
    /// size between those. An O_DIRECT write is the last kind: it may fail having written some of
    /// its blocks, iomap's saying the error, not what it wrote (iomap_dio_complete; ext4 on a full
    /// disk, refusals_are_adopted_by_the_reference). Fails saying how it couldn't.
    pub fn stopped_short(&mut self, step: &Step, file: usize, got: &[u8]) -> Result<(), String> {
        let before = &self.files[file];
        let reached = |what: String| format!("file {file} {what} after `{step}` stopped short");
        match *step {
            Step::Write { .. } | Step::FaultingWrite { .. } | Step::WriteFromMapping { .. }
                if got != &before[..] =>
            {
                let first = got.iter().zip(before).position(|(a, b)| a != b);
                let first =
                    first.map_or("past the end of either".to_owned(), |at| format!("{at:#x}"));
                let (got, was) = (got.len(), before.len());
                Err(reached(format!("is {got:#x} bytes long, was {was:#x}, differing at {first}")))
            }
            Step::Write { .. } | Step::FaultingWrite { .. } | Step::WriteFromMapping { .. } => {
                Ok(())
            }
            Step::Clone { from, to } => {
                assert_eq!(to, file, "a clone changes its destination alone");
                self.clone_stopped_short(from, to, got)
            }
            _ => {
                let mut whole = self.clone();
                whole.apply(step);
                let after = &whole.files[file];
                let (shortest, longest) =
                    (before.len().min(after.len()), before.len().max(after.len()));
                if !(shortest..=longest).contains(&got.len()) {
                    return Err(reached(format!("is {:#x} bytes long", got.len())));
                }
                let byte = |bytes: &[u8], at: usize| bytes.get(at).copied().unwrap_or(0);
                let wrong = (0..got.len())
                    .find(|&at| got[at] != byte(before, at) && got[at] != byte(after, at));
                if let Some(at) = wrong {
                    let (was, would) = (byte(before, at), byte(after, at));
                    return Err(reached(format!(
                        "reads {:#x} at {at:#x}, neither {was:#x}, as it was, nor {would:#x}",
                        got[at]
                    )));
                }
                self.files[file] = got.to_vec();
                Ok(())
            }
        }
    }

    /// What a clone refused for lack of room left: a clone is many transactions, and what it got
    /// to stands (fxfs_vfs_remap_file_range), its blocks in order, which `got`, `to` as it reads
    /// now, says the extent of. Fails with the size if that's no size the clone could have reached.
    fn clone_stopped_short(&mut self, from: usize, to: usize, got: &[u8]) -> Result<(), String> {
        let (before, source) = (self.files[to].len(), self.files[from].clone());
        let reached = got.len();
        if !(before..=before.max(source.len())).contains(&reached) {
            return Err(format!(
                "file {to} is {reached:#x} bytes long after a clone stopped short"
            ));
        }
        let blocks = source.len().min(reached);
        let mut end = 0;
        while end < blocks {
            let next = (end + PAGE as usize).min(blocks);
            if got[end..next] != source[end..next] {
                break;
            }
            end = next;
        }
        self.files[to].resize(reached, 0);
        self.files[to][..end].copy_from_slice(&source[..end]);
        Ok(())
    }
}

/// fxdax: the largest file a filesystem holds, its superblock's `s_maxbytes`: the VFS's own
/// (MAX_LFS_FILESIZE), as XFS's and Fxfs's without a DAX window are; Fxfs's with one, a chunk short
/// of the VFS's (fxfs_vfs_fill_super); and ext4's of blocks of `block` bytes, its extents' 32-bit
/// block numbers' (ext4_max_size).
pub const LARGEST: u64 = i64::MAX as u64;
pub const LARGEST_DAX: u64 = LARGEST / CHUNK * CHUNK;
pub const fn largest_ext4(block: u64) -> u64 {
    ((1 << 32) - 1) * block
}

/// fxdax: what [`Step::Far`] comes to, a write of `len` bytes at `at` of an empty file, or a
/// truncation to `at`, on a filesystem whose largest file is `largest`, as the VFS checks it
/// (pwrite64, rw_verify_area, generic_write_check_limits; do_ftruncate, inode_newsize_ok): `EINVAL`
/// at a negative offset, or past i64's end, `EFBIG` at the largest file's end or past it, and a
/// write shortened to it.
pub fn far(at: u64, len: u64, truncate: bool, largest: u64) -> Outcome {
    let end = i64::MAX as u64;
    if at > end || (!truncate && at + len > end) {
        Err(errno::EINVAL)
    } else if truncate {
        if at > largest { Err(errno::EFBIG) } else { Ok(0) }
    } else if at >= largest {
        Err(errno::EFBIG)
    } else {
        Ok(len.min(largest - at))
    }
}

/// fxdax: the index in the whole of `seed`'s script of `len` operations before which a killed run
/// stops (TASK-149.2): it does those before it of what's kept, says so, and waits to be killed.
/// Never 0, so that something is done, nor past the end.
pub fn kill_index(seed: u64, len: usize) -> usize {
    1 + Rng::new(seed ^ 0x6b69_6c6c_6564).next(len.max(2) as u64 - 1) as usize
}

/// fxdax: what a file may hold after its VM is killed, as of the operations done (TASK-149.2): the
/// sizes it had since its last fsync, and of each of its pages, the contents it had since then,
/// past its end reading zeros. Writeback writes pages as they are when it takes them, and a kill
/// keeps what it wrote, so that each page, and the size, may be of a state of its own. Kept page
/// by page, not state by state: a file that isn't synced for thousands of operations had thousands
/// of states of megabytes.
#[derive(Clone, Debug, Default)]
pub struct Survivals {
    sizes: std::collections::BTreeSet<u64>,
    pages: Vec<std::collections::HashSet<Vec<u8>>>,
}

/// fxdax: `data`'s page `index`, past its end reading zeros.
fn page_of(data: &[u8], index: usize) -> Vec<u8> {
    let mut page = vec![0; PAGE as usize];
    let start = (index * PAGE as usize).min(data.len());
    let end = ((index + 1) * PAGE as usize).min(data.len());
    page[..end - start].copy_from_slice(&data[start..end]);
    page
}

impl Survivals {
    /// What a file synced as `synced` may hold.
    pub fn synced(synced: &[u8]) -> Self {
        let mut survivals = Self::default();
        survivals.sizes.insert(synced.len() as u64);
        for index in 0..synced.len().div_ceil(PAGE as usize) {
            survivals.page(index).insert(page_of(synced, index));
        }
        survivals
    }

    fn page(&mut self, index: usize) -> &mut std::collections::HashSet<Vec<u8>> {
        if self.pages.len() <= index {
            // A page past every state's end read zeros in each of them.
            self.pages.resize_with(index + 1, || [vec![0; PAGE as usize]].into());
        }
        &mut self.pages[index]
    }

    /// The file went from `was` to `now`: its new size, and the pages that changed, may be kept.
    pub fn changed(&mut self, was: &[u8], now: &[u8]) {
        self.sizes.insert(now.len() as u64);
        for index in 0..was.len().max(now.len()).div_ceil(PAGE as usize) {
            let page = page_of(now, index);
            if page != page_of(was, index) {
                self.page(index).insert(page);
            }
        }
    }

    /// Whether `got`, what the file holds after the kill, or nothing if it isn't there, is what it
    /// may hold: a size it had, and each page as one of its states had it, as far as it reaches.
    /// A file not there is one whose creation the kill lost, as empty.
    pub fn hold(&self, got: Option<&[u8]>) -> Result<(), String> {
        let got = got.unwrap_or_default();
        if !self.sizes.contains(&(got.len() as u64)) {
            return Err(format!("{} bytes, where it was {:?}", got.len(), self.sizes));
        }
        for index in 0..got.len().div_ceil(PAGE as usize) {
            let end = ((index + 1) * PAGE as usize).min(got.len());
            let held = &got[index * PAGE as usize..end];
            let zero = [vec![0; PAGE as usize]].into();
            let pages = self.pages.get(index).unwrap_or(&zero);
            if !pages.iter().any(|page| page[..held.len()] == *held) {
                return Err(format!("page {index:#x} holds what none of its states held"));
            }
        }
        Ok(())
    }
}

/// fxdax: what each of `files` may hold after a kill before the script's operation `kill`, of
/// `script` done in order, its outcomes the model's, as on a volume with room: each file's
/// [`Survivals`] since its last fsync.
pub fn survivals(script: &[(usize, Op)], kill: usize, files: usize) -> Vec<Survivals> {
    let mut model = Model::new(files);
    let mut survivals = vec![Survivals::synced(&[]); files];
    for (_, op) in script.iter().take_while(|(index, _)| *index < kill) {
        let step = model.resolve(op);
        // Of the files the operation may change, what they were: copies of megabytes otherwise.
        let changes = match *op {
            Op::Read { .. } | Op::Check | Op::Fsync { .. } => Vec::new(),
            Op::Write { file, .. }
            | Op::Append { file, .. }
            | Op::Store { file, .. }
            | Op::Truncate { file, .. }
            | Op::Punch { file, .. } => vec![file],
            Op::Clone { to, .. } => vec![to],
            _ => (0..files).collect(),
        };
        let was: Vec<_> = changes.iter().map(|&file| model.files[file].clone()).collect();
        model.apply(&step);
        for (&file, was) in changes.iter().zip(&was) {
            if *was != model.files[file] {
                survivals[file].changed(was, &model.files[file]);
            }
        }
        if let Op::Fsync { file } = *op {
            survivals[file] = Survivals::synced(&model.files[file]);
        }
    }
    survivals
}

/// A script's failure, as its run saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The index in the whole script of the operation that saw it, the script's length if its end
    /// did, if a script's run could say.
    pub op: Option<usize>,
    /// One of [`kind`]'s, or [`Failure::errno_kind`]'s.
    pub kind: String,
    pub what: String,
}

const FAILED: &str = "script failed: ";

impl Failure {
    /// Of an operation that failed with `errno`.
    pub fn errno_kind(errno: Option<i32>) -> String {
        format!("errno {}", errno.unwrap_or_default())
    }

    /// The failure a guest said on its standard output `output`.
    pub fn parse(output: &str) -> Option<Self> {
        let line = output.lines().find_map(|line| line.strip_prefix(FAILED))?;
        let mut parts = line.splitn(3, ": ");
        let op = parts.next()?.strip_prefix("operation ")?;
        let (kind, what) = (parts.next()?.to_owned(), parts.next()?.to_owned());
        Some(Self { op: op.parse().ok(), kind, what })
    }

    pub fn same_way(&self, other: &Self) -> bool {
        self.kind == other.kind
    }

    /// Whether a shorter script could fail this way: not by being refused nothing, nor by no file
    /// being DAX, nor by no time changing, which only the whole of one is held to.
    pub fn shrinks(&self) -> bool {
        ![kind::NOTHING_REFUSED, kind::NEVER_DAX, kind::NO_TIMES].contains(&self.kind.as_str())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { op, kind, what } = self;
        match op {
            Some(op) => write!(f, "{FAILED}operation {op}: {kind}: {what}"),
            None => write!(f, "{FAILED}operation unknown: {kind}: {what}"),
        }
    }
}

/// `script` with what can go gone, of what `fails` says still fails, deleting ever smaller chunks,
/// the later ones first, as delta debugging does, until no single operation can go or `fails` has
/// been asked `tries` times. It returns the smallest it saw fail, `script` if it saw none.
pub fn shrink<T: Clone>(script: &[T], tries: usize, mut fails: impl FnMut(&[T]) -> bool) -> Vec<T> {
    let mut script = script.to_vec();
    let mut left = tries;
    let mut chunk = script.len().div_ceil(2).max(1);
    loop {
        let mut shorter = false;
        let mut end = script.len();
        while end > 0 {
            if left == 0 {
                return script;
            }
            left -= 1;
            let start = end.saturating_sub(chunk);
            let candidate = [&script[..start], &script[end..]].concat();
            if fails(&candidate) {
                script = candidate;
                shorter = true;
            }
            end = start;
        }
        if chunk == 1 && !shorter {
            return script;
        }
        chunk = chunk.div_ceil(2);
    }
}

/// What a shrink of a failure came to.
#[derive(Debug, PartialEq, Eq)]
pub struct Shrunk {
    /// The indices of the operations of the smallest script that failed as the first run did:
    /// `None` if none did, not even the whole script's.
    pub kept: Option<Vec<usize>>,
    /// How many runs that took.
    pub tries: usize,
}

/// Shrinks a script of `len` operations that failed with `failure`, at most `tries` runs of it:
/// `attempt` runs it with `DROP_VAR` as it's told, and says how it failed, if it did. A run ends at
/// its first failure, so what comes after the operation that saw it goes first, for a single run.
/// A drop list too long for the guest's command line isn't run.
pub fn shrink_failure(
    len: usize,
    failure: &Failure,
    tries: usize,
    mut attempt: impl FnMut(&str) -> Option<Failure>,
) -> Shrunk {
    let mut ran = 0;
    let mut fails = |kept: &[usize]| {
        let drops = drops(kept, len);
        if ran == tries || drops.len() > LONGEST_DROPS {
            return false;
        }
        ran += 1;
        attempt(&drops).is_some_and(|again| again.same_way(failure))
    };
    let all: Vec<usize> = (0..len).collect();
    let upto = failure.op.map_or(len, |op| (op + 1).min(len));
    let start = if upto < len && fails(&all[..upto]) {
        Some(&all[..upto])
    } else if fails(&all) {
        Some(&all[..])
    } else {
        None
    };
    let kept = start.map(|start| shrink(start, tries, &mut fails));
    Shrunk { kept, tries: ran }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fxdax: after a kill, a file holds a size it had since its last fsync, and each page as one of
    /// its states since then had it, which may be of different states; not what it held before
    /// that fsync, nor bytes no state held, nor a size none had. A file the kill lost the creation
    /// of is empty (TASK-149.2).
    #[test]
    fn a_killed_file_holds_pages_of_its_states_since_its_last_fsync() {
        let page = PAGE as usize;
        let place = Place::At(0);
        let script: Vec<(usize, Op)> = [
            Op::Write { file: 0, place, len: 2 * PAGE, data: 1 },
            Op::Fsync { file: 0 },
            Op::Write { file: 0, place: Place::At(PAGE), len: PAGE, data: 2 },
            Op::Write { file: 0, place, len: PAGE, data: 3 },
            Op::Write { file: 1, place, len: 10, data: 4 },
            Op::Truncate { file: 0, size: 3 * PAGE + 5 },
        ]
        .into_iter()
        .enumerate()
        .collect();
        let mut model = Model::new(2);
        let mut states = vec![model.clone()];
        for (_, op) in &script {
            model.apply(&model.resolve(op));
            states.push(model.clone());
        }
        let survivals = survivals(&script, script.len(), 2);
        let state = |n: usize| states[n].files[0].clone();
        assert_eq!(survivals[0].hold(Some(&state(2))), Ok(()), "as synced");
        assert_eq!(survivals[0].hold(Some(&state(6))), Ok(()), "as last done");
        // The first page as the second write left it, the second as the third did.
        let mut mixed = state(4);
        mixed[..page].copy_from_slice(&state(3)[..page]);
        assert_eq!(survivals[0].hold(Some(&mixed)), Ok(()));
        assert!(survivals[0].hold(Some(&state(1))).is_ok(), "the synced state, by its pages");
        let mut garbage = state(6);
        garbage[page + 7] ^= 0xff;
        assert_eq!(
            survivals[0].hold(Some(&garbage)),
            Err("page 0x1 holds what none of its states held".to_owned())
        );
        assert_eq!(
            survivals[0].hold(None),
            Err(format!(
                "0 bytes, where it was {:?}",
                std::collections::BTreeSet::from([2 * PAGE, 3 * PAGE + 5])
            ))
        );
        // Of file 1, never synced: empty, or written.
        assert_eq!(survivals[1].hold(None), Ok(()));
        assert_eq!(survivals[1].hold(Some(&states[5].files[1])), Ok(()));
        // Before the kill, as the guest stops before the operation it names.
        let before = super::survivals(&script, 2, 2);
        assert_eq!(before[0].hold(Some(&state(2))), Ok(()));
        assert!(before[0].hold(Some(&state(3))).is_err(), "a write after the kill kept");
    }

    /// fxdax: a killed run stops at an operation of the script, never before its first.
    #[test]
    fn kill_indices_fall_within_the_script() {
        for seed in 0..1000 {
            assert!((1..3000).contains(&kill_index(seed, 3000)), "seed {seed}");
        }
        assert_eq!(kill_index(7, 1), 1);
        assert_ne!(kill_index(1, 3000), kill_index(2, 3000));
    }
    use std::collections::BTreeMap;

    /// Each kind of operation, and how many of it, of `script`.
    fn tally(script: &[Op]) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for op in script {
            let kind = match op {
                Op::Write { .. } => "write",
                Op::Append { .. } => "append",
                Op::Store { child: false, .. } => "store",
                Op::Store { child: true, .. } => "fork",
                Op::Read { .. } => "read",
                Op::Truncate { .. } => "truncate",
                Op::Punch { .. } => "punch",
                Op::Fsync { .. } => "fsync",
                Op::Clone { .. } => "clone",
                Op::Check => "check",
                Op::CloneRange { len: 0, .. } => "clone to the end",
                Op::CloneRange { aligned: true, .. } => "clone range",
                Op::CloneRange { aligned: false, .. } => "clone range, unaligned",
                Op::Copy { .. } => "copy",
                Op::Zero { .. } => "zero",
                Op::Allocate { .. } => "allocate",
                Op::Msync { .. } => "msync",
                Op::Fdatasync { .. } => "fdatasync",
                Op::MapStore { private: false, .. } => "map store",
                Op::MapStore { private: true, .. } => "private store",
                Op::Direct { write: true, .. } => "direct write",
                Op::Direct { write: false, .. } => "direct read",
                Op::Drop { .. } => "drop",
                Op::Reopen { .. } => "reopen",
                Op::Limit { .. } => "limit",
                Op::Far { .. } => "far",
                Op::MapOdd { .. } => "map oddly",
                Op::MapFixed { .. } => "fixed map store",
                Op::Mremap { .. } => "remapped store",
                Op::Madvise { remove: false, .. } => "dontneed",
                Op::Madvise { remove: true, .. } => "remove",
                Op::TruncateRacing { .. } => "truncate racing",
                Op::FaultingWrite { .. } => "faulting write",
                Op::WriteFromMapping { .. } => "write from mapping",
                Op::ProtectKept { .. } => "protect kept",
                Op::MoveKept { .. } => "move kept",
            };
            *counts.entry(kind).or_insert(0) += 1;
        }
        counts
    }

    #[test]
    fn a_script_is_a_function_of_its_seed() {
        assert_eq!(generate(7, SEVERAL_FILES), generate(7, SEVERAL_FILES));
        assert_ne!(generate(7, SEVERAL_FILES), generate(8, SEVERAL_FILES));
    }

    #[test]
    fn a_seeds_script_stays_what_it_was() {
        assert_eq!(
            generate(1, ONE_FILE)[..3],
            [
                Op::Write {
                    file: 0,
                    place: Place::FromEnd(-4488),
                    len: 1914,
                    data: 10437328418169017477
                },
                Op::Read { file: 0, place: Place::At(131244), len: 14750 },
                Op::Write {
                    file: 0,
                    place: Place::At(2097057),
                    len: 12942,
                    data: 2668146810060474434
                },
            ]
        );
    }

    /// random_ops's mix, as test_random_ops drew it: of 3000, 5 in 17 writes (882), 4 stores (706),
    /// 3 reads (529), 2 fsyncs (353) and one each (176) of truncations, punches and appends, and a
    /// check every 256 operations.
    #[test]
    fn one_files_mix_is_random_opss() {
        let script = generate(1, ONE_FILE);
        let expected = BTreeMap::from([
            ("append", 170),
            ("check", 11),
            ("fsync", 368),
            ("punch", 180),
            ("read", 525),
            ("store", 696),
            ("truncate", 174),
            ("write", 887),
        ]);
        assert_eq!(tally(&script), expected);
        assert_eq!(script.len(), 3000 + 11);
    }

    #[test]
    fn several_files_mix_adds_to_one_files() {
        let script = generate(1, SEVERAL_FILES);
        let expected = BTreeMap::from([
            ("append", 185),
            ("check", 11),
            ("clone", 94),
            ("fork", 195),
            ("fsync", 343),
            ("punch", 184),
            ("read", 545),
            ("store", 670),
            ("truncate", 153),
            ("write", 920),
        ]);
        assert_eq!(tally(&script), expected);
        let files = script.iter().filter_map(|op| match op {
            Op::Write { file, .. } => Some(*file),
            _ => None,
        });
        let files: std::collections::BTreeSet<usize> = files.collect();
        assert_eq!(files, [0, 1, 2].into());
        let onto_itself = |op: &Op| matches!(op, Op::Clone { from, to } if from == to);
        assert!(!script.iter().any(onto_itself));
    }

    /// Drawn offsets lean on the ends of things as test_random_ops's did: of 4000 places, one in
    /// four from the file's end, within 8 KiB of it, and one in four around the second chunk.
    #[test]
    fn places_lean_on_the_ends_of_things() {
        let mut rng = Rng::new(3);
        let places: Vec<Place> = (0..4000).map(|_| place(&mut rng)).collect();
        let from_end = places.iter().filter(|place| matches!(place, Place::FromEnd(_))).count();
        assert_eq!(from_end, 989);
        let near_chunk =
            |place: &&Place| matches!(place, Place::At(at) if at.abs_diff(CHUNK) <= NEAR / 2);
        assert_eq!(places.iter().filter(near_chunk).count(), 1057);
        let within = |place: &Place| match *place {
            Place::At(at) => at < MAX + NEAR,
            Place::FromEnd(by) => by.unsigned_abs() <= NEAR / 2,
        };
        assert!(places.iter().all(within));
    }

    #[test]
    fn places_resolve_against_the_size_they_find() {
        assert_eq!(Place::At(4096).resolve(0), 4096);
        assert_eq!(Place::FromEnd(-100).resolve(4096), 3996);
        assert_eq!(Place::FromEnd(100).resolve(4096), 4196);
        assert_eq!(Place::FromEnd(-100).resolve(50), 0);
        assert_eq!(Place::At(MAX + 5).resolve(0), MAX - 1);
        assert_eq!(Place::FromEnd(100).resolve(MOST), MAX - 1);
    }

    /// Any subset of a script resolves: the model never reaches past what a file may hold, and no
    /// store past a file's end.
    #[test]
    fn any_subset_of_a_script_resolves() {
        for (mix, seed) in [(SEVERAL_FILES, 11), (WIDE, 11), (WIDE, 12), (HOSTILE, 11)] {
            let script = generate(seed, mix);
            for keep_one_in in [1, 2, 3, 7, 50] {
                let mut rng = Rng::new(keep_one_in);
                let mut model = Model::new(mix.files);
                for op in script.iter().filter(|_| rng.next(keep_one_in) == 0) {
                    let step = model.resolve(op);
                    if let Step::Store { file, at, data, .. }
                    | Step::MapStore { file, at, data, .. }
                    | Step::MapFixed { file, at, data, .. }
                    | Step::Mremap { file, at, data }
                    | Step::ProtectKept { file, at, data }
                    | Step::MoveKept { file, at, data }
                    | Step::TruncateRacing { file, at, data, .. } = &step
                    {
                        assert!(*at as usize + data.len() <= model.files[*file].len(), "{step}");
                    }
                    if let Step::TruncateRacing { size, at, data, .. } = &step {
                        assert!(at + data.len() as u64 <= *size, "{step}");
                    }
                    if let Step::WriteFromMapping { file, src, dst, len } = &step {
                        assert!(src + len <= model.files[*file].len() as u64, "{step}");
                        assert!(dst == src || dst + len <= *src, "{step}");
                    }
                    if let Step::DirectWrite { at, .. } | Step::DirectRead { at, .. } = &step {
                        assert_eq!(at % PAGE, 0, "{step}");
                    }
                    model.apply(&step);
                    let longest = model.files.iter().map(Vec::len).max().unwrap() as u64;
                    assert!(longest <= MOST, "{step} made a file of {longest:#x} bytes");
                }
            }
        }
    }

    /// What an operation writes is its own: the same bytes whatever came before it, only as many
    /// as there's room for.
    #[test]
    fn an_operation_writes_the_same_whatever_is_dropped_before_it() {
        let script = generate(5, SEVERAL_FILES);
        let mut whole = Model::new(SEVERAL_FILES.files);
        let alone = Model::new(SEVERAL_FILES.files);
        let mut written = 0;
        for op in &script {
            let step = whole.resolve(op);
            if let (Step::Write { data, .. }, Step::Write { data: its_own, .. }) =
                (&step, alone.resolve(op))
            {
                assert_eq!(data[..], its_own[..data.len()], "{op:?}");
                written += 1;
            }
            whole.apply(&step);
        }
        assert_eq!(written, 1077);
    }

    #[test]
    fn a_store_on_an_empty_file_stores_nothing() {
        let op = Op::Store { file: 0, place: Place::At(100), len: 10, data: 1, child: true };
        let step = Model::new(1).resolve(&op);
        assert_eq!(step, Step::Store { file: 0, at: 0, data: Vec::new(), child: true });
    }

    #[test]
    fn a_store_wraps_within_the_file() {
        let mut model = Model::new(1);
        model.truncate(0, 1000);
        let op = Op::Store { file: 0, place: Place::At(1900), len: 500, data: 1, child: false };
        let Step::Store { at, data, .. } = model.resolve(&op) else { panic!("a store") };
        assert_eq!((at, data.len()), (900, 100));
    }

    #[test]
    fn an_append_stops_at_the_most_a_file_holds() {
        let mut model = Model::new(1);
        model.truncate(0, MOST - 10);
        let Step::Write { at, data, .. } =
            model.resolve(&Op::Append { file: 0, len: 100, data: 1 })
        else {
            panic!("a write")
        };
        assert_eq!((at, data.len()), (MOST - 10, 10));
    }

    #[test]
    fn a_clone_keeps_what_lies_past_the_sources_end() {
        let mut model = Model { files: vec![vec![1; PAGE as usize], vec![2; 3 * PAGE as usize]] };
        model.clone_file(0, 1);
        let expected = [vec![1; PAGE as usize], vec![2; 2 * PAGE as usize]].concat();
        assert_eq!(model.files[1], expected);
        assert_eq!(model.files[0], vec![1; PAGE as usize]);
    }

    /// FICLONE is FICLONERANGE of the whole file, onto 0.
    #[test]
    fn a_clone_of_a_partial_block_within_the_destination_is_invalid() {
        let model = Model { files: vec![vec![1; 5000], vec![2; 9000], vec![3; 5000], vec![]] };
        let clone = |from, to| model.outcome(&Step::Clone { from, to });
        assert_eq!(clone(0, 1), Err(errno::EINVAL));
        assert_eq!(clone(0, 2), Ok(5000));
        assert_eq!(clone(1, 0), Ok(9000));
        assert_eq!(clone(3, 0), Ok(0));
        let aligned = Model { files: vec![vec![1; 4096], vec![2; 9000]] };
        assert_eq!(aligned.outcome(&Step::Clone { from: 0, to: 1 }), Ok(4096));
    }

    /// The VFS's checks of a range's clone (generic_remap_checks and the rest), each refusal.
    #[test]
    fn a_range_clone_is_refused_as_the_vfs_refuses_it() {
        let page = PAGE;
        let model = Model { files: vec![vec![1; 3 * 4096 + 100], vec![2; 2 * 4096]] };
        let clone = |src, dst, len| model.clone_range(0, src, 1, dst, len);
        // Whole blocks within both.
        assert_eq!(clone(page, 0, page), Ok(page));
        // Starts off a block.
        assert_eq!(clone(100, 0, page), Err(errno::EINVAL));
        assert_eq!(clone(0, 100, page), Err(errno::EINVAL));
        // A length of part of a block, short of the source's end.
        assert_eq!(clone(0, 0, 100), Err(errno::EINVAL));
        // Past the source's end, though it starts within it.
        assert_eq!(clone(3 * page, 0, page), Err(errno::EINVAL));
        // From its end or past it.
        assert_eq!(clone(4 * page, 0, page), Err(errno::EINVAL));
        // To the source's end: its partial block, into the middle of the destination, refused;
        // onto its end, or past it, taken.
        assert_eq!(clone(3 * page, 0, 100), Err(errno::EINVAL));
        assert_eq!(clone(3 * page, page, 100), Err(errno::EINVAL));
        assert_eq!(clone(3 * page, 2 * page, 100), Ok(100));
        assert_eq!(clone(3 * page, 5 * page, 0), Ok(100));
        assert_eq!(clone(2 * page, 2 * page, 0), Ok(page + 100));
        // Zero is to the end: from the end, nothing, past it, refused.
        assert_eq!(clone(3 * page + 100, 0, 0), Ok(0));
        assert_eq!(clone(4 * page, 0, 0), Err(errno::EINVAL));
    }

    /// Ranges of one file overlapping, by the blocks a clone takes: a partial last one whole.
    #[test]
    fn a_range_cloned_onto_itself_overlapping_is_refused() {
        let model = Model { files: vec![vec![1; 5000]] };
        let clone = |src, dst, len| model.clone_range(0, src, 0, dst, len);
        assert_eq!(clone(0, PAGE, 0), Err(errno::EINVAL));
        assert_eq!(clone(0, 2 * PAGE, 0), Ok(5000));
        assert_eq!(clone(PAGE, 0, 0), Err(errno::EINVAL));
        assert_eq!(clone(0, 0, PAGE), Err(errno::EINVAL));
    }

    /// A clone writes what it takes where it lands, growing the destination, and leaves the rest.
    #[test]
    fn a_range_clone_lands_and_grows_its_destination() {
        let source: Vec<u8> = (0..2 * PAGE + 10).map(|at| at as u8).collect();
        let mut model = Model { files: vec![source.clone(), vec![7; PAGE as usize]] };
        let step = Step::CloneRange { from: 0, src: PAGE, to: 1, dst: 2 * PAGE, len: 0 };
        assert_eq!(model.outcome(&step), Ok(PAGE + 10));
        model.apply(&step);
        let held =
            [vec![7; PAGE as usize], vec![0; PAGE as usize], source[PAGE as usize..].to_vec()];
        assert_eq!(model.files[1], held.concat());
        assert_eq!(model.files[0], source);
        // Refused, it leaves both.
        let refused = Step::CloneRange { from: 0, src: 1, to: 1, dst: 0, len: PAGE };
        assert_eq!(model.outcome(&refused), Err(errno::EINVAL));
        model.apply(&refused);
        assert_eq!(model.files[1], held.concat());
    }

    /// The VFS's checks of a copy (generic_copy_file_checks): shortened to the source's end, and
    /// refused overlapping within a file.
    #[test]
    fn a_copy_is_shortened_and_refused_as_the_vfs_has_it() {
        let mut model = Model { files: vec![(0..100).collect(), vec![9; 10]] };
        assert_eq!(model.copy_range(0, 10, 1, 50, 20), Ok(20));
        assert_eq!(model.copy_range(0, 90, 1, 0, 20), Ok(10));
        assert_eq!(model.copy_range(0, 100, 1, 0, 20), Ok(0));
        assert_eq!(model.copy_range(0, 200, 1, 0, 20), Ok(0));
        assert_eq!(model.copy_range(0, 10, 0, 20, 20), Err(errno::EINVAL));
        assert_eq!(model.copy_range(0, 30, 0, 20, 20), Err(errno::EINVAL));
        assert_eq!(model.copy_range(0, 10, 0, 30, 20), Ok(20));
        // Past the source's end, what overlaps is nothing.
        assert_eq!(model.copy_range(0, 100, 0, 50, 20), Ok(0));
        model.apply(&Step::Copy { from: 0, src: 95, to: 1, dst: 12, len: 20 });
        let held = [vec![9; 10], vec![0; 2], (95..100).collect()].concat();
        assert_eq!(model.files[1], held);
    }

    /// A zeroed range and an allocated one grow the file to their ends unless the size is kept;
    /// zeroing past the end zeroes nothing.
    #[test]
    fn ranges_zeroed_and_allocated_grow_unless_the_size_is_kept() {
        let mut model = Model { files: vec![vec![5; 100]] };
        model.apply(&Step::Zero { file: 0, at: 90, len: 20, keep_size: true });
        assert_eq!(model.files[0], [vec![5; 90], vec![0; 10]].concat());
        model.apply(&Step::Zero { file: 0, at: 10, len: 5, keep_size: false });
        assert_eq!(model.files[0].len(), 100);
        assert_eq!(model.files[0][10..15], [0; 5]);
        model.apply(&Step::Zero { file: 0, at: 200, len: 5, keep_size: false });
        assert_eq!(model.files[0].len(), 205);
        model.apply(&Step::Allocate { file: 0, at: 300, len: 5, keep_size: true });
        assert_eq!(model.files[0].len(), 205);
        model.apply(&Step::Allocate { file: 0, at: 300, len: 5, keep_size: false });
        assert_eq!(model.files[0].len(), 305);
        assert_eq!(model.files[0][205..], [0; 100]);
    }

    /// Reads, direct ones too, read what the file has of their range.
    #[test]
    fn a_reads_outcome_is_what_lies_within_the_file() {
        let model = Model { files: vec![vec![1; 5000]] };
        assert_eq!(model.outcome(&Step::Read { file: 0, at: 4000, len: 4096 }), Ok(1000));
        assert_eq!(model.outcome(&Step::DirectRead { file: 0, at: 8192, len: 4096 }), Ok(0));
        assert_eq!(model.outcome(&Step::Truncate { file: 0, size: 1 }), Ok(0));
    }

    /// The wide mix's places resolve as its operations need: direct I/O and msyncs at pages' starts,
    /// aligned clones in whole blocks, a clone to the end landing where the file it makes fits.
    #[test]
    fn wide_operations_resolve_to_what_they_need() {
        let mut model = Model { files: vec![vec![1; 3 * PAGE as usize + 7], vec![]] };
        let direct =
            Op::Direct { file: 0, place: Place::At(5000), pages: 2, data: 1, write: false };
        assert_eq!(model.resolve(&direct), Step::DirectRead { file: 0, at: PAGE, len: 2 * PAGE });
        let msync = Op::Msync { file: 0, place: Place::At(5000), len: 10 };
        assert_eq!(model.resolve(&msync), Step::Msync { file: 0, at: PAGE, len: 10 + 904 });
        let clone = |len, aligned| Op::CloneRange {
            from: 0,
            from_place: Place::At(PAGE + 3),
            to: 1,
            to_place: Place::At(MAX - 10),
            len,
            aligned,
        };
        assert_eq!(
            model.resolve(&clone(PAGE + 5, true)),
            Step::CloneRange { from: 0, src: PAGE, to: 1, dst: MAX - PAGE, len: PAGE }
        );
        assert_eq!(
            model.resolve(&clone(PAGE + 5, false)),
            Step::CloneRange { from: 0, src: PAGE + 3, to: 1, dst: MAX - 10, len: PAGE + 5 }
        );
        model.files[0] = vec![1; MOST as usize];
        let Step::CloneRange { dst, len: 0, .. } = model.resolve(&clone(0, true)) else {
            panic!("a clone to the end")
        };
        assert_eq!(dst, PAGE);
        let store =
            Op::MapStore { file: 0, place: Place::At(MOST + 5), len: 10, data: 1, private: true };
        let Step::MapStore { at, data, private: true, .. } = model.resolve(&store) else {
            panic!("a private store")
        };
        assert_eq!((at, data.len()), (MAX - 1, 10));
    }

    /// The wide mix: of 3000, half again as many kinds as random_ops', in their proportions, the
    /// clones and stores by a child as fxfs-guest-script's, and none of its own lost to another.
    #[test]
    fn the_wide_mix_adds_to_several_files() {
        let script = generate(1, WIDE);
        let expected = BTreeMap::from([
            ("allocate", 91),
            ("append", 94),
            ("check", 11),
            ("clone", 85),
            ("clone range", 103),
            ("clone range, unaligned", 41),
            ("clone to the end", 41),
            ("copy", 180),
            ("direct read", 103),
            ("direct write", 94),
            ("drop", 92),
            ("fdatasync", 90),
            ("fork", 197),
            ("fsync", 179),
            ("map store", 103),
            ("msync", 95),
            ("private store", 91),
            ("punch", 78),
            ("read", 279),
            ("reopen", 103),
            ("store", 359),
            ("truncate", 198),
            ("write", 501),
            ("zero", 85),
        ]);
        assert_eq!(tally(&script), expected);
        let onto_itself = |op: &Op| matches!(op, Op::Copy { from, to, .. } if from == to);
        assert_eq!(script.iter().filter(|op| onto_itself(op)).count(), 57);
    }

    /// The mixes before the wide one are as they were: the wide mix draws its own only when asked.
    #[test]
    fn the_wide_mix_leaves_the_others_scripts_as_they_were() {
        assert_eq!(generate(1, Mix { wide: false, ..WIDE }), generate(1, SEVERAL_FILES));
        assert_ne!(generate(1, WIDE), generate(1, SEVERAL_FILES));
    }

    /// The hostile mix: the wide one's, and after one operation in four, one of its own, a
    /// range onto itself among them.
    #[test]
    fn the_hostile_mix_adds_to_the_wide_one() {
        let script = generate(1, HOSTILE);
        let expected = BTreeMap::from([
            ("allocate", 111),
            ("append", 80),
            ("check", 11),
            ("clone", 93),
            ("clone range", 120),
            ("clone range, unaligned", 65),
            ("clone to the end", 42),
            ("copy", 205),
            ("direct read", 95),
            ("direct write", 121),
            ("dontneed", 28),
            ("drop", 94),
            ("far", 46),
            ("faulting write", 108),
            ("fdatasync", 94),
            ("fixed map store", 55),
            ("fork", 179),
            ("fsync", 190),
            ("limit", 110),
            ("map oddly", 62),
            ("map store", 97),
            ("move kept", 56),
            ("msync", 99),
            ("private store", 80),
            ("protect kept", 54),
            ("punch", 95),
            ("read", 277),
            ("remapped store", 48),
            ("remove", 24),
            ("reopen", 90),
            ("store", 357),
            ("truncate", 184),
            ("truncate racing", 52),
            ("write", 475),
            ("write from mapping", 55),
            ("zero", 94),
        ]);
        assert_eq!(tally(&script), expected);
        assert_eq!(generate(1, Mix { hostile: false, ..HOSTILE }), generate(1, WIDE));
    }

    /// Arguments no well-behaved caller passes, refused by the VFS before any filesystem sees them
    /// (rw_verify_area, ksys_pwrite64, do_ftruncate, vfs_fallocate, remap_verify_area), or
    /// nothing done: a read near i64's end past the largest file, a copy from past the source's end.
    #[test]
    fn calls_past_i64_are_refused_as_the_vfs_refuses_them() {
        let model = Model { files: vec![vec![1; 100]] };
        let limit = |call, k, len| {
            let step = model.resolve(&Op::Limit { file: 0, call, k, len });
            let Step::Limit { at, len, .. } = step else { panic!("a limit") };
            (at, len, model.outcome(&step))
        };
        let end = i64::MAX;
        let (einval, efbig) = (Err(errno::EINVAL), Err(errno::EFBIG));
        assert_eq!(limit(Limit::WritePastI64, 12, 10), (end - 2, 10, einval));
        assert_eq!(limit(Limit::WriteNegative, 0, 10), (-1, 10, einval));
        assert_eq!(limit(Limit::ReadPastI64, 10, 10), (end - 10, 10, Ok(0)));
        assert_eq!(limit(Limit::ReadPastI64, 9, 10), (end - 9, 10, einval));
        assert_eq!(limit(Limit::TruncateNegative, 4, 10), (-5, 10, einval));
        assert_eq!(limit(Limit::AllocateNothing, 4, 10), (4, 0, einval));
        assert_eq!(limit(Limit::AllocateNegative, 4, 10), (-5, 10, einval));
        assert_eq!(limit(Limit::AllocatePastI64, 3, 10), (end - 3, 10, efbig));
        assert_eq!(limit(Limit::CopyFromPastI64, 30, 10), (end - 30, 10, Ok(0)));
        assert_eq!(limit(Limit::CloneNegative, 0, 10), (0, 10 | 1 << 63, einval));
    }

    /// Far writes and truncations come to what each filesystem's largest file lets them.
    #[test]
    fn far_steps_come_to_what_the_largest_file_lets_them() {
        let ext4 = largest_ext4(PAGE);
        assert_eq!(ext4, 0xfff_ffff_f000);
        assert_eq!(LARGEST_DAX, 0x7fff_ffff_ffe0_0000);
        assert_eq!(far(512 << 20, 100, false, ext4), Ok(100));
        assert_eq!(far(ext4 - 10, 100, false, ext4), Ok(10));
        assert_eq!(far(ext4, 100, false, ext4), Err(errno::EFBIG));
        assert_eq!(far(ext4, 100, false, LARGEST_DAX), Ok(100));
        assert_eq!(far(ext4, 0, true, ext4), Ok(0));
        assert_eq!(far(ext4 + 1, 0, true, ext4), Err(errno::EFBIG));
        assert_eq!(far(LARGEST_DAX - 1, 100, false, LARGEST_DAX), Ok(1));
        assert_eq!(far(LARGEST - 100, 100, false, LARGEST), Ok(100));
        assert_eq!(far(LARGEST - 99, 100, false, LARGEST), Err(errno::EINVAL));
        assert_eq!(far(LARGEST, 0, true, LARGEST), Ok(0));
        assert_eq!(far(LARGEST + 1, 0, true, LARGEST), Err(errno::EINVAL));
        assert_eq!(far(LARGEST + 1, 100, false, LARGEST), Err(errno::EINVAL));
        let step = Model::new(1).resolve(&Op::Far {
            base: LARGEST,
            k: -5,
            len: 3,
            data: 1,
            truncate: false,
        });
        assert_eq!(
            step,
            Step::Far { at: LARGEST - 5, data: Rng::new(1).bytes(3), truncate: false }
        );
    }

    /// Mappings at no page's start, or of nothing, are refused; madvise too, and REMOVE punches
    /// the pages it advises of, whole.
    #[test]
    fn odd_mappings_and_advice_are_refused_and_removal_punches_pages() {
        let mut model = Model { files: vec![vec![1; 3 * PAGE as usize]] };
        let map = |at, len| model.outcome(&Step::MapOdd { file: 0, at, len });
        assert_eq!(map(PAGE + 1, 10), Err(errno::EINVAL));
        assert_eq!(map(PAGE, 0), Err(errno::EINVAL));
        assert_eq!(map(PAGE, 10), Ok(0));
        let madvise = |odd, remove| Step::Madvise { file: 0, at: PAGE, len: 10, remove, odd };
        assert_eq!(model.outcome(&madvise(true, true)), Err(errno::EINVAL));
        model.apply(&madvise(false, false));
        assert_eq!(model.files[0], vec![1; 3 * PAGE as usize]);
        model.apply(&madvise(false, true));
        let punched = [vec![1; PAGE as usize], vec![0; PAGE as usize], vec![1; PAGE as usize]];
        assert_eq!(model.files[0], punched.concat());
    }

    /// A write from a buffer that faults writes what lies before the fault, or fails, nothing
    /// before it; a write from the file's own mapping moves bytes apart from where it writes them,
    /// or writes them where they are.
    #[test]
    fn writes_from_faulting_buffers_and_their_files_mappings() {
        let mut model = Model { files: vec![(0..100).collect()] };
        let faulting = |good| Step::FaultingWrite { file: 0, at: 90, data: vec![7; 20], good };
        assert_eq!(model.outcome(&faulting(0)), Err(errno::EFAULT));
        assert_eq!(model.outcome(&faulting(5)), Ok(5));
        model.apply(&faulting(5));
        assert_eq!(model.files[0], [(0..90).collect(), vec![7; 5], (95..100).collect()].concat());
        let from = |by| Op::WriteFromMapping { file: 0, place: Place::At(30), len: 50, by };
        // Overlapping, it writes as far as its source.
        let step = model.resolve(&from(10));
        assert_eq!(step, Step::WriteFromMapping { file: 0, src: 30, dst: 20, len: 10 });
        assert_eq!(model.outcome(&step), Ok(10));
        let mut moved = model.files[0].clone();
        moved.copy_within(30..40, 20);
        model.apply(&step);
        assert_eq!(model.files[0], moved);
        // The same bytes, all, or bytes apart, all.
        assert_eq!(
            model.resolve(&from(0)),
            Step::WriteFromMapping { file: 0, src: 30, dst: 30, len: 50 }
        );
        let apart = Op::WriteFromMapping { file: 0, place: Place::At(60), len: 20, by: 40 };
        assert_eq!(
            model.resolve(&apart),
            Step::WriteFromMapping { file: 0, src: 60, dst: 20, len: 20 }
        );
    }

    /// A truncation racing a store leaves the store, below the size it went down to, and zeros
    /// between that and the size it came back to.
    #[test]
    fn a_truncation_racing_a_store_leaves_the_store_and_zeros_past_its_size() {
        let mut model = Model { files: vec![vec![1; 100]] };
        let op = Op::TruncateRacing { file: 0, size: 50, place: Place::At(140), len: 30, data: 2 };
        let step = model.resolve(&op);
        let stored = Rng::new(2).bytes(10);
        assert_eq!(step, Step::TruncateRacing { file: 0, size: 50, at: 40, data: stored.clone() });
        model.apply(&step);
        assert_eq!(model.files[0], [vec![1; 40], stored, vec![0; 50]].concat());
    }

    #[test]
    fn a_clone_stopped_short_holds_the_blocks_it_got_to() {
        let source: Vec<u8> = vec![1; 3 * PAGE as usize];
        let mut model = Model { files: vec![source.clone(), vec![2; 3 * PAGE as usize]] };
        let got = [vec![1; 2 * PAGE as usize], vec![2; PAGE as usize]].concat();
        assert_eq!(model.clone_stopped_short(0, 1, &got), Ok(()));
        assert_eq!(model.files[1], got);
        assert_eq!(model.files[0], source);
    }

    #[test]
    fn a_clone_stopped_short_of_a_shorter_file_holds_the_size_it_reached() {
        let mut model = Model { files: vec![vec![1; 3 * PAGE as usize], vec![2; 100]] };
        let got = vec![1; 2 * PAGE as usize];
        assert_eq!(model.clone_stopped_short(0, 1, &got), Ok(()));
        assert_eq!(model.files[1], got);
    }

    #[test]
    fn a_clone_stopped_short_that_shrank_its_destination_fails() {
        let mut model = Model { files: vec![vec![1; 100], vec![2; 3 * PAGE as usize]] };
        assert_eq!(
            model.clone_stopped_short(0, 1, &vec![2; PAGE as usize]),
            Err("file 1 is 0x1000 bytes long after a clone stopped short".to_owned())
        );
    }

    #[test]
    fn a_punch_stopped_short_holds_its_zeros() {
        let mut model = Model { files: vec![vec![7; 10]] };
        let punch = Step::Punch { file: 0, at: 2, len: 5 };
        assert_eq!(model.stopped_short(&punch, 0, &[7, 7, 0, 0, 7, 7, 0, 7, 7, 7]), Ok(()));
        assert_eq!(model.files[0], [7, 7, 0, 0, 7, 7, 0, 7, 7, 7]);
    }

    #[test]
    fn a_punch_stopped_short_finds_a_byte_it_cant_have_left() {
        let mut model = Model { files: vec![vec![7; 10]] };
        let punch = Step::Punch { file: 0, at: 2, len: 5 };
        assert_eq!(
            model.stopped_short(&punch, 0, &[7, 7, 0, 9, 7, 7, 7, 7, 7, 7]),
            Err(
                "file 0 reads 0x9 at 0x3, neither 0x7, as it was, nor 0x0 after `punch 0x5 at 0x2 \
                 of file 0` stopped short"
                    .to_owned()
            )
        );
        // Bytes past what it could reach, or a size it couldn't: the file outside the range is
        // the model's.
        assert_eq!(
            model.stopped_short(&punch, 0, &[0; 10]),
            Err(
                "file 0 reads 0x0 at 0x0, neither 0x7, as it was, nor 0x7 after `punch 0x5 at 0x2 \
                 of file 0` stopped short"
                    .to_owned()
            )
        );
        assert_eq!(
            model.stopped_short(&punch, 0, &[7; 9]),
            Err("file 0 is 0x9 bytes long after `punch 0x5 at 0x2 of file 0` stopped short"
                .to_owned())
        );
    }

    /// A growth stopped short is any size between, zeros past what the file was.
    #[test]
    fn a_growth_stopped_short_is_of_a_size_between() {
        let mut model = Model { files: vec![vec![7; 10]] };
        let zero = Step::Zero { file: 0, at: 5, len: 20, keep_size: false };
        let got = [vec![7; 5], vec![0; 10]].concat();
        assert_eq!(model.stopped_short(&zero, 0, &got), Ok(()));
        assert_eq!(model.files[0], got);
    }

    /// A write that fails wrote nothing: one that writes any says how much.
    #[test]
    fn a_refused_write_leaves_the_file_as_it_was() {
        let mut model = Model { files: vec![vec![7; 10]] };
        let write = Step::Write { file: 0, at: 5, data: vec![1; 10] };
        assert_eq!(model.stopped_short(&write, 0, &[7; 10]), Ok(()));
        assert_eq!(
            model.stopped_short(&write, 0, &[[7; 5], [1; 5]].concat()),
            Err("file 0 is 0xa bytes long, was 0xa, differing at 0x5 after `write 0xa at 0x5 of file \
                 0` stopped short"
                .to_owned())
        );
    }

    #[test]
    fn drops_say_the_ranges_left_out() {
        assert_eq!(drops(&[0, 1, 2, 3], 4), "");
        assert_eq!(drops(&[], 4), "0-3");
        assert_eq!(drops(&[1, 3], 5), "0,2,4");
        assert_eq!(drops(&[10, 11, 41, 99, 111], 120), "0-9,12-40,42-98,100-110,112-119");
    }

    #[test]
    fn drops_are_read_back() {
        assert_eq!(kept("", 3), Ok(vec![0, 1, 2]));
        assert_eq!(kept("0,2-3", 5), Ok(vec![1, 4]));
        assert_eq!(kept("0-1,1-2", 4), Ok(vec![3]));
        let script: Vec<usize> = (0..1000).filter(|index| index % 7 < 2).collect();
        assert_eq!(kept(&drops(&script, 1000), 1000).as_ref(), Ok(&script));
    }

    #[test]
    fn drops_that_make_no_sense_are_refused() {
        assert_eq!(kept("3-1", 5), Err("\"3-1\" isn't among 5 operations".to_owned()));
        assert_eq!(kept("9", 5), Err("\"9\" isn't among 5 operations".to_owned()));
        assert_eq!(kept("x", 5), Err("\"x\": invalid digit found in string".to_owned()));
    }

    #[test]
    fn a_failure_is_read_back_from_the_guests_output() {
        let failure = Failure {
            op: Some(1234),
            kind: kind::DIFFERS.into(),
            what: "0x10 bytes read at 0x20: 0x10 bytes, differing".into(),
        };
        let output = format!("{failure}\nFAILED: random_ops\n");
        assert_eq!(Failure::parse(&output), Some(failure));
        assert_eq!(Failure::parse("OK\n"), None);
    }

    #[test]
    fn a_failure_of_no_operation_is_read_back() {
        let failure = Failure { op: None, kind: kind::HANG.into(), what: "out of time".into() };
        assert_eq!(Failure::parse(&failure.to_string()), Some(failure));
    }

    #[test]
    fn a_failure_with_an_errno_differs_from_one_with_another() {
        let failure = |errno| Failure {
            op: Some(1),
            kind: Failure::errno_kind(Some(errno)),
            what: "pwrite".into(),
        };
        assert!(failure(5).same_way(&failure(5)));
        assert!(!failure(5).same_way(&failure(28)));
    }

    #[test]
    fn a_run_refused_nothing_does_not_shrink() {
        let failure = |kind: &str| Failure { op: Some(1), kind: kind.into(), what: "what".into() };
        assert!(!failure(kind::NOTHING_REFUSED).shrinks());
        assert!(failure(kind::DIFFERS).shrinks());
    }

    /// `script` fails if it has both of `pair`.
    fn has_both(script: &[usize], pair: [usize; 2]) -> bool {
        pair.iter().all(|index| script.contains(index))
    }

    #[test]
    fn a_planted_pair_is_found() {
        let script: Vec<usize> = (0..3000).collect();
        let mut asked = 0;
        let shrunk = shrink(&script, TRIES, |script| {
            asked += 1;
            has_both(script, [1234, 2999])
        });
        assert_eq!(shrunk, [1234, 2999]);
        assert!(asked <= TRIES, "{asked} tries");
    }

    #[test]
    fn a_pair_anywhere_is_found_within_the_tries() {
        let script: Vec<usize> = (0..3000).collect();
        let mut rng = Rng::new(9);
        for _ in 0..200 {
            let first = rng.next(3000) as usize;
            let second = (first + 1 + rng.next(2999) as usize) % 3000;
            let pair = [first.min(second), first.max(second)];
            let shrunk = shrink(&script, TRIES, |script| has_both(script, pair));
            assert_eq!(shrunk, pair);
        }
    }

    #[test]
    fn a_single_operation_is_found() {
        let script: Vec<usize> = (0..3000).collect();
        assert_eq!(shrink(&script, TRIES, |script| script.contains(&17)), [17]);
    }

    #[test]
    fn a_script_needing_all_of_itself_stays_whole() {
        let script: Vec<usize> = (0..16).collect();
        assert_eq!(shrink(&script, TRIES, |candidate| candidate.len() == 16), script);
    }

    #[test]
    fn a_script_that_fails_of_nothing_goes_altogether() {
        let script: Vec<usize> = (0..16).collect();
        assert_eq!(shrink(&script, TRIES, |_| true), Vec::<usize>::new());
    }

    #[test]
    fn the_tries_bound_a_shrink_and_it_keeps_what_it_got_to() {
        let script: Vec<usize> = (0..3000).collect();
        for tries in [0, 1, 2, 5, 12] {
            let mut asked = 0;
            let shrunk = shrink(&script, tries, |script| {
                asked += 1;
                has_both(script, [100, 2000])
            });
            assert_eq!(asked, tries);
            assert!(has_both(&shrunk, [100, 2000]), "{} operations", shrunk.len());
        }
    }

    /// A failure that comes and goes costs a shrink only what it might have dropped: what it
    /// returns failed when it was run.
    #[test]
    fn a_failure_that_comes_and_goes_shrinks_to_what_was_seen_failing() {
        let script: Vec<usize> = (0..300).collect();
        let mut rng = Rng::new(2);
        let mut failed = vec![script.clone()];
        let shrunk = shrink(&script, TRIES, |candidate| {
            let fails = has_both(candidate, [10, 20]) && rng.next(3) != 0;
            if fails {
                failed.push(candidate.to_vec());
            }
            fails
        });
        assert!(failed.contains(&shrunk), "{shrunk:?}");
        assert!(has_both(&shrunk, [10, 20]));
    }

    fn failure(op: Option<usize>, kind: &str) -> Failure {
        Failure { op, kind: kind.into(), what: "what".into() }
    }

    #[test]
    fn a_shrink_of_a_failure_drops_what_came_after_it_first() {
        let mut asked = Vec::new();
        let first = failure(Some(99), kind::DIFFERS);
        let shrunk = shrink_failure(3000, &first, 1, |drops| {
            asked.push(drops.to_owned());
            Some(first.clone())
        });
        assert_eq!(asked, ["100-2999"]);
        assert_eq!(shrunk.tries, 1);
        assert_eq!(shrunk.kept, Some((0..100).collect()));
    }

    #[test]
    fn a_shrink_of_a_failure_finds_a_planted_pair_in_a_script_of_operations() {
        let first = failure(Some(2500), kind::DIFFERS);
        let shrunk = shrink_failure(3011, &first, TRIES, |drops| {
            let kept = kept(drops, 3011).unwrap();
            has_both(&kept, [40, 2500]).then(|| failure(kept.last().copied(), kind::DIFFERS))
        });
        assert_eq!(shrunk.kept, Some(vec![40, 2500]));
        assert!(shrunk.tries <= TRIES, "{} tries", shrunk.tries);
    }

    /// The first failure's end of the script: nothing follows it to drop.
    #[test]
    fn a_failure_at_the_scripts_end_shrinks_from_the_whole_script() {
        let first = failure(Some(100), kind::DIFFERS);
        let shrunk = shrink_failure(100, &first, TRIES, |drops| {
            let kept = kept(drops, 100).unwrap();
            kept.contains(&7).then(|| first.clone())
        });
        assert_eq!(shrunk.kept, Some(vec![7]));
    }

    #[test]
    fn a_shrink_keeps_only_what_fails_the_same_way() {
        let first = failure(None, kind::DIFFERS);
        let shrunk = shrink_failure(64, &first, TRIES, |drops| {
            let kept = kept(drops, 64).unwrap();
            match (kept.contains(&5), kept.contains(&50)) {
                (true, true) => Some(failure(None, kind::DIFFERS)),
                // Without the second, a failure, but another: it's not the same bug.
                (true, false) => Some(failure(None, kind::HANG)),
                _ => None,
            }
        });
        assert_eq!(shrunk.kept, Some(vec![5, 50]));
    }

    #[test]
    fn a_failure_that_never_comes_again_says_so() {
        let first = failure(Some(10), kind::DIFFERS);
        let shrunk = shrink_failure(100, &first, TRIES, |_| None);
        assert_eq!(shrunk, Shrunk { kept: None, tries: 2 });
    }

    #[test]
    fn a_shrink_of_no_tries_runs_nothing() {
        let first = failure(Some(10), kind::DIFFERS);
        let shrunk = shrink_failure(100, &first, 0, |_| panic!("a run"));
        assert_eq!(shrunk, Shrunk { kept: None, tries: 0 });
    }

    /// A list of drops too long for the kernel's command line is passed over, not run.
    #[test]
    fn drops_too_long_for_the_command_line_are_not_run() {
        let len = 4000;
        let first = failure(None, kind::DIFFERS);
        // What must stay is every other operation: the list is one range for each.
        let needed = |index: &usize| index.is_multiple_of(2);
        let shrunk = shrink_failure(len, &first, 400, |drops| {
            assert!(drops.len() <= LONGEST_DROPS, "{} bytes", drops.len());
            let kept = kept(drops, len).unwrap();
            (0..len).filter(needed).all(|index| kept.contains(&index)).then(|| first.clone())
        });
        let kept = shrunk.kept.unwrap();
        assert!(kept.len() > len / 2, "{} operations kept", kept.len());
    }

    #[test]
    fn a_full_scripts_drops_fit_on_the_command_line_at_each_shrink() {
        // Every other chunk of 64 gone, the worst of a shrink at that size.
        let kept: Vec<usize> = (0..3011).filter(|index| index / 64 % 2 == 0).collect();
        assert_eq!(drops(&kept, 3011).len(), 223);
    }
}
