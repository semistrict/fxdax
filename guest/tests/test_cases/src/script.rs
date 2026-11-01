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
}

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
}

/// What a script's operations are drawn from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mix {
    /// The operations drawn: what `clones` and `forks` add, and the whole-file checks, are more.
    pub ops: usize,
    pub files: usize,
    pub clones: bool,
    pub forks: bool,
}

/// `random_ops`'s.
pub const ONE_FILE: Mix = Mix { ops: 3000, files: 1, clones: false, forks: false };
/// `fxfs-guest-script`'s.
pub const SEVERAL_FILES: Mix = Mix { ops: 3000, files: 3, clones: true, forks: true };

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
        script.push(match rng.next(17) {
            0..=4 => Op::Write { file, place, len, data },
            5..=8 => Op::Store { file, place, len, data, child: false },
            9..=11 => Op::Read { file, place, len },
            12 => Op::Truncate { file, size: rng.next(MAX) },
            13 => Op::Punch { file, place, len },
            16 => Op::Append { file, len, data },
            _ => Op::Fsync { file },
        });
        if mix.clones && rng.next(CLONE_ONE_IN) == 0 {
            let from = rng.next(mix.files as u64) as usize;
            let to = (from + 1 + rng.next(mix.files as u64 - 1) as usize) % mix.files;
            script.push(Op::Clone { from, to });
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
    Write { file: usize, at: u64, data: Vec<u8> },
    Store { file: usize, at: u64, data: Vec<u8>, child: bool },
    Read { file: usize, at: u64, len: u64 },
    Truncate { file: usize, size: u64 },
    Punch { file: usize, at: u64, len: u64 },
    Fsync { file: usize },
    Clone { from: usize, to: usize },
    Check,
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
        }
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
                let size = self.size(file);
                let at = place.resolve(size).checked_rem(size).unwrap_or(0);
                Step::Store { file, at, data: bytes(data, len.min(size - at)), child }
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
        }
    }

    pub fn write(&mut self, file: usize, at: u64, data: &[u8]) {
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

    /// Whether the VFS refuses to clone `from` onto `to`: a partial block at the source's end
    /// can't replace part of a block of the destination's (generic_remap_check_len).
    pub fn clone_is_invalid(&self, from: usize, to: usize) -> bool {
        let (source, destination) = (self.size(from), self.size(to));
        source % PAGE != 0 && source < destination
    }

    /// FICLONE of `from` onto `to`: what `to` holds past the source's end stays.
    pub fn clone_file(&mut self, from: usize, to: usize) {
        let source = std::mem::take(&mut self.files[from]);
        self.write(to, 0, &source);
        self.files[from] = source;
    }

    /// What a clone refused for lack of room left: a clone is many transactions, and what it got
    /// to stands (fxfs_vfs_remap_file_range), its blocks in order, which `got`, `to` as it reads
    /// now, says the extent of. Fails with the size if that's no size the clone could have reached.
    pub fn clone_stopped_short(
        &mut self,
        from: usize,
        to: usize,
        got: &[u8],
    ) -> Result<(), String> {
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

    /// What a punch refused for lack of room left: the partial blocks at its ends are zeroed before
    /// the rest goes (fxfs_dax_punch_begin), so each of its bytes may be as it was or a zero, which
    /// `got`, what the range `at` of `file` reads now, says. Fails with the offset of a byte that's
    /// neither.
    pub fn punch_stopped_short(&mut self, file: usize, at: u64, got: &[u8]) -> Result<(), String> {
        for (i, &byte) in got.iter().enumerate() {
            let held = &mut self.files[file][at as usize + i];
            if byte != *held && byte != 0 {
                let offset = at + i as u64;
                return Err(format!(
                    "file {file} at {offset:#x} reads {byte:#x} after a punch stopped short, not \
                     {held:#x} or a zero"
                ));
            }
            *held = byte;
        }
        Ok(())
    }
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

    /// Whether a shorter script could fail this way: not by being refused nothing, which only the
    /// whole of one is held to.
    pub fn shrinks(&self) -> bool {
        self.kind != kind::NOTHING_REFUSED
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
            };
            *counts.entry(kind).or_insert(0) += 1;
        }
        counts
    }

    /// Applies what `step` comes to, as a run that nothing refuses does.
    fn apply(model: &mut Model, step: &Step) {
        match step {
            Step::Write { file, at, data } => model.write(*file, *at, data),
            Step::Store { file, at, data, .. } => model.write(*file, *at, data),
            Step::Truncate { file, size } => model.truncate(*file, *size),
            Step::Punch { file, at, len } => model.punch(*file, *at, *len),
            Step::Clone { from, to } if !model.clone_is_invalid(*from, *to) => {
                model.clone_file(*from, *to)
            }
            _ => {}
        }
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
        let script = generate(11, SEVERAL_FILES);
        for keep_one_in in [2, 3, 7, 50] {
            let mut rng = Rng::new(keep_one_in);
            let mut model = Model::new(SEVERAL_FILES.files);
            for op in script.iter().filter(|_| rng.next(keep_one_in) == 0) {
                let step = model.resolve(op);
                if let Step::Store { file, at, data, .. } = &step {
                    assert!(*at as usize + data.len() <= model.files[*file].len(), "{step}");
                }
                apply(&mut model, &step);
                let longest = model.files.iter().map(Vec::len).max().unwrap() as u64;
                assert!(longest <= MOST, "{step} made a file of {longest:#x} bytes");
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
            apply(&mut whole, &step);
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

    #[test]
    fn a_clone_of_a_partial_block_within_the_destination_is_invalid() {
        let model = Model { files: vec![vec![1; 5000], vec![2; 9000], vec![3; 5000]] };
        assert!(model.clone_is_invalid(0, 1));
        assert!(!model.clone_is_invalid(0, 2));
        assert!(!model.clone_is_invalid(1, 0));
        let aligned = Model { files: vec![vec![1; 4096], vec![2; 9000]] };
        assert!(!aligned.clone_is_invalid(0, 1));
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
        assert_eq!(model.punch_stopped_short(0, 2, &[0, 0, 7, 7, 0]), Ok(()));
        assert_eq!(model.files[0], [7, 7, 0, 0, 7, 7, 0, 7, 7, 7]);
    }

    #[test]
    fn a_punch_stopped_short_finds_a_byte_it_cant_have_left() {
        let mut model = Model { files: vec![vec![7; 10]] };
        assert_eq!(
            model.punch_stopped_short(0, 2, &[0, 9]),
            Err("file 0 at 0x3 reads 0x9 after a punch stopped short, not 0x7 or a zero".to_owned())
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
