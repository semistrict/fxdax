// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a closed segment's parts, read from its raw file, looked at and packed on threads, each
//! part in a buffer of its own (TASK-336): what a framing does with its data before it writes it
//! (`segments::frame_segment`).
//!
//! A framing read the whole of its data, 32 MiB, into one buffer on its own thread, looked at all
//! of it there, its zeros and a sample of its compression, and checksummed it, before compressing
//! its parts on threads. On a host whose CPUs its VMs' vCPUs keep busy, each thread gets its share
//! of them, no more: so the reading, its buffer's every page faulted in afresh, and the looking,
//! on one thread, took most of a framing's time, up to half a second each of 0.8 s, beside 28
//! spinning threads on ubu1's 8 CPUs (store-bench framing); and so on ubu1's sandboxes run, its
//! slow framings' reading 2.5 s, compressing 2.3 s and writing 0.6 s of 5.3 s, 1.1 s of it waiting
//! to run. On threads, each part in a buffer kept for the framings after ([`Buffer`]), as are the
//! threads' compressors, no page faulted in afresh, they take what compressing them takes.

use crate::dir::File;
use crate::timings::runnable_ns;
use extent_frame::{PART, PackedPart, PartPacker};
use std::io;
use std::sync::Mutex;

/// fxdax: the most part buffers ([`Buffer`]) and compressors ([`PartPacker`]) kept for framings to
/// come: two segments' worth of parts, 64 MiB, and a compressor for each of their threads. Taken
/// afresh, each framing's 32 MiB of buffers were faulted in page by page, 8,192 faults, and each
/// thread's compressor's tables too, every framing: as much of its reading's time as the copying
/// (store-bench framing, perf stat on ubu1). More framings at once than that take theirs afresh,
/// and keep them only while there is room.
const KEPT: usize = 2 * 32;

/// fxdax: what [`KEPT`] keeps, taken by framings and given back.
struct Kept<T> {
    kept: Mutex<Vec<T>>,
}

impl<T> Kept<T> {
    const fn new() -> Self {
        Self { kept: Mutex::new(Vec::new()) }
    }

    /// One kept, or `fresh`'s.
    fn take(&self, fresh: impl FnOnce() -> T) -> T {
        let kept = self.kept.lock().unwrap().pop();
        kept.unwrap_or_else(fresh)
    }

    /// `it`, kept if there is room.
    fn give(&self, it: T) {
        let mut kept = self.kept.lock().unwrap();
        if kept.len() < KEPT {
            kept.push(it);
        }
    }
}

static BUFFERS: Kept<Vec<u8>> = Kept::new();
static PACKERS: Kept<PartPacker> = Kept::new();

/// fxdax: the bytes of a part of a segment's data, read, in a buffer of a part's bytes kept for
/// the framings after it once it goes, if there is room ([`KEPT`]).
pub(crate) struct Buffer(Vec<u8>);

impl Buffer {
    /// One of `len` bytes, a part's at most.
    fn of(len: usize) -> Self {
        let mut bytes = BUFFERS.take(|| Vec::with_capacity(PART as usize));
        bytes.resize(len, 0);
        Self(bytes)
    }
}

impl std::ops::Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl std::ops::DerefMut for Buffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        BUFFERS.give(std::mem::take(&mut self.0));
    }
}

/// fxdax: what of a segment's data compressing its parts must save for it to be framed in a copy,
/// compressed, rather than in place, every part raw (`segments::frame_segment`, TASK-310): an
/// eighth. Writing the data again to save less of it, in the bucket and in the host's cache, costs
/// the host's disk more than it saves, on a disk VMs share with their bucket above all. Framed in
/// place only if none compressed, segments a guest's filesystem fills, a large file's data with its
/// metadata in a few of their parts, were all framed in copies but for a few.
pub(crate) const IN_PLACE_SAVING: u64 = 8;

/// fxdax: the bytes of each part of a segment's data its sample compresses ([`Look`]): an eighth,
/// its first, so that every part has its share sampled, spread over all of it (TASK-343).
pub(crate) const SAMPLE: usize = (PART / 8) as usize;

/// fxdax: the blocks of a segment's data [`Look`] finds zeros in, every one of them.
const ZERO_BLOCK: usize = 4096;

/// fxdax: a look at parts of a segment's data short of compressing them: their bytes, those of
/// their blocks of zeros, and of their samples, as they are and compressed (TASK-310).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Look {
    len: u64,
    zeros: u64,
    sampled: u64,
    packed: u64,
}

impl Look {
    /// A look at `part`, a part of a segment's data, its sample compressed by `packer`.
    pub fn at(part: &[u8], packer: &mut PartPacker) -> Self {
        let zeros = (part.chunks(ZERO_BLOCK))
            .filter(|block| block.iter().all(|&byte| byte == 0))
            .map(|block| block.len() as u64)
            .sum();
        let sample = &part[..part.len().min(SAMPLE)];
        let (sampled, packed) = (sample.len() as u64, packer.packed_len(sample));
        Self { len: part.len() as u64, zeros, sampled, packed }
    }

    /// The look at these parts and `other`'s.
    pub fn plus(self, other: Self) -> Self {
        Self {
            len: self.len + other.len,
            zeros: self.zeros + other.zeros,
            sampled: self.sampled + other.sampled,
            packed: self.packed + other.packed,
        }
    }

    /// Whether the parts looked at, a segment's data, save too little compressed for a framing in
    /// a copy: their blocks of zeros under half of what a copy must save ([`IN_PLACE_SAVING`]),
    /// and their samples compressing by under that too; what a segment of a large file's data,
    /// which doesn't compress, holds. One saving more, or near the bound, is compressed whole to
    /// tell (TASK-310).
    ///
    /// The zeros counted exactly, as the store writes them of its own: an extent's holes, sealed in
    /// one run with it, which no guest's write wrote, nor a tenant's disk admitted (TASK-94).
    /// Sampled a part in eight, 1 MiB each, the first part alone of a segment under 9 MiB, the
    /// sample took a segment of a hostile VM process's 3 MiB of noise, then 2.6 MB of its extents'
    /// holes, for one that saves nothing: framed in place, raw, the holes took its tenant's disk
    /// past its limit by what a copy, compressed, takes none of (TASK-343, Hunt run 42949975683).
    pub fn saves_little(&self) -> bool {
        if self.zeros >= self.len / (2 * IN_PLACE_SAVING) {
            return false;
        }
        self.sampled - self.packed < self.sampled / (2 * IN_PLACE_SAVING)
    }
}

/// fxdax: a part of a segment's data, read ([`read`]): its bytes; if its segment may be framed in
/// place, their CRC, as a raw part's, and a look at them.
pub(crate) struct Read {
    pub data: Buffer,
    pub crc: u32,
    pub look: Look,
}

/// fxdax: the parts of the `len` bytes of a segment's data its raw file `raw` holds, each read in
/// a buffer of its own, on threads ([`on_threads`]); and, if `in_place`, the segment may be framed
/// in place, each checksummed and looked at. Returns them, and the longest any thread waited to
/// run, in nanoseconds.
pub(crate) fn read<F: File>(raw: &F, len: u64, in_place: bool) -> (io::Result<Vec<Read>>, u64) {
    on_threads(len.div_ceil(PART) as usize, |part, packer| {
        let at = part as u64 * PART;
        let mut data = Buffer::of((len - at).min(PART) as usize);
        raw.read_cached(&mut data, at)?;
        let (crc, look) = match in_place {
            true => (PackedPart::raw(&data).crc, Look::at(&data, packer)),
            false => (0, Look::default()),
        };
        Ok(Read { data, crc, look })
    })
}

/// fxdax: `parts`, a segment's data, read ([`read`]), packed, on threads ([`on_threads`]); and the
/// longest any thread waited to run, in nanoseconds.
pub(crate) fn pack(parts: &[Read]) -> (Vec<PackedPart>, u64) {
    let (packed, waited) =
        on_threads(parts.len(), |part, packer| Ok(packer.pack(&parts[part].data)));
    (packed.expect("packing fails not"), waited)
}

/// fxdax: `each` of `count` parts, on as many threads as the host has CPUs for this process, but
/// no more than there are parts, each thread taking those after the last's, with a packer of its
/// own, kept for the threads after ([`KEPT`]): their results, in order, or the first error; and the longest any of the threads waited to
/// run, runnable, in nanoseconds ([`runnable_ns`]). On the calling thread, if one is all it takes.
fn on_threads<T: Send>(
    count: usize,
    each: impl Fn(usize, &mut PartPacker) -> io::Result<T> + Sync,
) -> (io::Result<Vec<T>>, u64) {
    let threads = std::thread::available_parallelism().map_or(1, |threads| threads.get());
    let per = count.div_ceil(threads.min(count).max(1));
    let run = |parts: std::ops::Range<usize>| {
        let since = runnable_ns();
        let mut packer = PACKERS.take(PartPacker::new);
        let done: io::Result<Vec<T>> = parts.map(|part| each(part, &mut packer)).collect();
        PACKERS.give(packer);
        (done, runnable_ns().saturating_sub(since))
    };
    if per >= count {
        return run(0..count);
    }
    std::thread::scope(|scope| {
        let running: Vec<_> = (0..count)
            .step_by(per)
            .map(|first| scope.spawn(move || run(first..(first + per).min(count))))
            .collect();
        let (mut all, mut waited) = (Ok(Vec::with_capacity(count)), 0);
        for thread in running {
            let (done, thread_waited) = thread.join().expect("a framing's thread panicked");
            waited = waited.max(thread_waited);
            all = all.and_then(|mut all: Vec<T>| {
                all.extend(done?);
                Ok(all)
            });
        }
        (all, waited)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dir, MemDir};

    const MIB: usize = 1 << 20;

    /// `len` bytes that don't compress, of `seed`'s.
    fn noise(seed: u64, len: usize) -> Vec<u8> {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    /// Whether `data`, a segment's, saves too little compressed, by a look at each of its parts.
    fn saves_little(data: &[u8]) -> bool {
        let mut packer = PartPacker::new();
        let parts = data.chunks(PART as usize).map(|part| Look::at(part, &mut packer));
        parts.fold(Look::default(), Look::plus).saves_little()
    }

    /// TASK-343: a segment's look short of compressing it sees all of it: a segment of 3 MiB of
    /// noise, then holes, an extent's zeros, as Hunt's hostile VM process made one, saves much
    /// compressed, as do zeros at every offset no sample piece reaches; one of noise alone, or with
    /// a few blocks of zeros, saves little. Red with every eighth MiB part sampled: the first only
    /// of a segment under 9 MiB, noise, so it saved little, and was framed in place, raw.
    #[test]
    fn segments_holding_holes_save_much_compressed() {
        let mut holes = noise(1, 3 * MIB);
        holes.resize(6 * MIB, 0);
        assert!(!saves_little(&holes), "3 MiB of noise, then 3 MiB of zeros");
        // Zeros past each part's sampled piece, where no sample reaches.
        let mut unsampled = noise(2, 8 * MIB);
        for part in unsampled.chunks_mut(MIB) {
            part[SAMPLE..SAMPLE + MIB / 4].fill(0);
        }
        assert!(!saves_little(&unsampled), "a quarter of each part zeros, unsampled");
        assert!(saves_little(&noise(3, 6 * MIB)), "noise");
        let mut few = noise(4, 6 * MIB);
        few[..4 * ZERO_BLOCK].fill(0);
        assert!(saves_little(&few), "noise, but for four blocks of zeros");
    }

    /// TASK-336: a segment's parts, read and packed on threads, are its data's, in order, each
    /// packed as one packer packs it, a last part cut short among them; each checksummed and
    /// looked at as a raw part if it may be framed in place, and not otherwise.
    #[test]
    fn parts_read_on_threads_are_the_segments() {
        let len = 9 * MIB + 1000;
        let mut data = noise(5, len);
        data[2 * MIB..3 * MIB].fill(7);
        let raw = MemDir::new().create("raw", true).unwrap();
        raw.write_at(&data, 0).unwrap();
        let (parts, _) = read(&raw, len as u64, true);
        let parts = parts.unwrap();
        assert_eq!(parts.len(), 10);
        let mut packer = PartPacker::new();
        for (read, part) in parts.iter().zip(data.chunks(PART as usize)) {
            assert!(*read.data == *part, "a part's bytes");
            assert_eq!(read.crc, PackedPart::raw(part).crc);
            assert_eq!(read.look, Look::at(part, &mut packer));
        }
        let (packed, _) = pack(&parts);
        let each: Vec<PackedPart> =
            data.chunks(PART as usize).map(|part| packer.pack(part)).collect();
        assert_eq!(packed, each);
        assert!(packed[2].compressed.is_some() && packed[0].compressed.is_none());
        let (parts, _) = read(&raw, len as u64, false);
        let parts = parts.unwrap();
        assert!(parts.iter().all(|read| read.crc == 0 && read.look == Look::default()));
        assert!(read(&raw, 0, true).0.unwrap().is_empty(), "no data, no parts");
    }

    /// TASK-336: a part that fails to read fails them all.
    #[test]
    fn a_part_failing_to_read_fails_the_framing() {
        struct Failing;
        impl File for Failing {
            fn read_at(&self, _: &mut [u8], at: u64) -> io::Result<()> {
                match at == 3 * PART {
                    true => Err(io::Error::other("failing")),
                    false => Ok(()),
                }
            }
            fn write_at(&self, _: &[u8], _: u64) -> io::Result<()> {
                unreachable!()
            }
            fn size(&self) -> io::Result<u64> {
                unreachable!()
            }
            fn sync(&self) -> io::Result<()> {
                unreachable!()
            }
            fn sync_cached(&self) -> io::Result<()> {
                unreachable!()
            }
        }
        let error = read(&Failing, 8 * PART, true).0.err().expect("a part failed");
        assert_eq!(error.to_string(), "failing");
    }
}
