// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: how an object holding extents' data is framed (backlog doc-7, Segments). Its data
//! is in parts of [`PART`] bytes, each stored on its own, compressed with zstd, or raw if that is no
//! smaller, at an offset that is a multiple of [`ALIGN`]; then its footer; then a table of where
//! each part is stored, its stored length, the CRC32C of its stored bytes, and its encoding; then a
//! [`TRAILER`] of the lengths of the data, the parts stored and the footer, and the CRCs of the
//! footer and the table. A store's closed segments and sealed extents are framed so, as are the
//! objects a branch keeps in object storage: a reader reads and decompresses only the parts it
//! needs, each checked against its CRC before it is decompressed.
//!
//! A branch's checkpoints, locally and in object storage, are such objects: tables of where its
//! extents are ([`table`]).

pub mod table;

use std::ops::Range;

/// Each this many bytes of an object's data, the last fewer, is a part, stored on its own, with
/// its own CRC32C: what a reader reads, checks and decompresses at once.
pub const PART: u64 = 1 << 20;

/// Parts are stored at multiples of this, so that a host reads them bypassing its page cache.
pub const ALIGN: u64 = 4096;

/// The bytes of the table for each part: its offset, stored length, CRC32C and encoding.
pub const ENTRY: u64 = 8 + 4 + 4 + 1;

/// The bytes after an object's table: the lengths of its data, its parts stored and its footer,
/// the CRC32Cs of its footer and its table, and [`MAGIC`].
pub const TRAILER: u64 = 8 + 8 + 8 + 4 + 4 + 8;

/// An object's last bytes.
const MAGIC: [u8; 8] = *b"fxdaxob1";

/// The zstd level parts are compressed at: the fastest, as sealing and closing segments wait for
/// it.
const LEVEL: i32 = 1;

/// The CRC32C of `bytes`.
pub fn crc(bytes: &[u8]) -> u32 {
    crc_fast::checksum(crc_fast::CrcAlgorithm::Crc32Iscsi, bytes) as u32
}

/// An object that is not as its frame says.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Corrupt(pub String);

fn corrupt(what: impl std::fmt::Display) -> Corrupt {
    Corrupt(format!("an object's {what}"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// How a part is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Raw,
    Zstd,
}

/// Where and how a part is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stored {
    pub offset: u64,
    pub len: u64,
    pub crc: u32,
    pub encoding: Encoding,
}

impl Stored {
    /// Where its bytes are in the object.
    pub fn range(&self) -> Range<u64> {
        self.offset..self.offset + self.len
    }

    /// Checks `bytes`, its stored bytes and any after them, against its CRC.
    pub fn check<'a>(&self, bytes: &'a [u8]) -> Result<&'a [u8], Corrupt> {
        let bytes = bytes.get(..self.len as usize).ok_or_else(|| corrupt("part, cut short"))?;
        if crc(bytes) != self.crc {
            return Err(corrupt("part, by its CRC"));
        }
        Ok(bytes)
    }

    /// Unpacks the part from `bytes`, its stored bytes and any after them, checked against its CRC,
    /// into `out`, which is as long as the part.
    pub fn unpack_into(&self, bytes: &[u8], out: &mut [u8]) -> Result<(), Corrupt> {
        let bytes = self.check(bytes)?;
        match self.encoding {
            Encoding::Raw if bytes.len() == out.len() => out.copy_from_slice(bytes),
            Encoding::Raw => return Err(corrupt("raw part, of the wrong length")),
            Encoding::Zstd => {
                let unpacked = zstd::bulk::decompress_to_buffer(bytes, out)
                    .map_err(|err| corrupt(format_args!("part, by zstd: {err}")))?;
                if unpacked != out.len() {
                    return Err(corrupt("compressed part, of the wrong length"));
                }
            }
        }
        Ok(())
    }

    /// The part, `len` bytes, unpacked from `bytes` as [`Stored::unpack_into`] does.
    pub fn unpack(&self, bytes: &[u8], len: u64) -> Result<Vec<u8>, Corrupt> {
        let mut out = vec![0; len as usize];
        self.unpack_into(bytes, &mut out)?;
        Ok(out)
    }
}

/// A part, packed: compressed, or raw if compressing does not shrink it.
fn pack_part(data: &[u8], compressor: &mut zstd::bulk::Compressor<'_>) -> Option<Vec<u8>> {
    // Room for no more than the raw part: compressing what will not fit fails.
    let mut packed = Vec::with_capacity(data.len().saturating_sub(1));
    match compressor.compress_to_buffer(data, &mut packed) {
        Ok(_) if !packed.is_empty() && packed.len() < data.len() => Some(packed),
        _ => None,
    }
}

/// `data`'s parts, packed: on threads of their own if there are several.
fn pack_parts(data: &[u8]) -> Vec<Option<Vec<u8>>> {
    let parts: Vec<&[u8]> = data.chunks(PART as usize).collect();
    let threads =
        std::thread::available_parallelism().map_or(1, |threads| threads.get()).min(parts.len());
    let compressor = || zstd::bulk::Compressor::new(LEVEL).expect("zstd makes a compressor");
    if threads <= 1 {
        let mut compressor = compressor();
        return parts.iter().map(|part| pack_part(part, &mut compressor)).collect();
    }
    let each = parts.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let packing: Vec<_> = parts
            .chunks(each)
            .map(|parts| {
                scope.spawn(move || {
                    let mut compressor = compressor();
                    parts.iter().map(|part| pack_part(part, &mut compressor)).collect::<Vec<_>>()
                })
            })
            .collect();
        packing.into_iter().flat_map(|thread| thread.join().unwrap()).collect()
    })
}

/// Frames an object written front to back: its parts, given in order, then its footer.
#[derive(Debug, Default)]
pub struct Packer {
    stored: Vec<Stored>,
    data_len: u64,
    /// The bytes of the object so far.
    end: u64,
}

impl Packer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes of the object so far.
    pub fn end(&self) -> u64 {
        self.end
    }

    /// Packs `data`, the object's next parts: whole parts, but for the object's last. Returns the
    /// object's next bytes, which follow those returned before.
    pub fn push(&mut self, data: &[u8]) -> Vec<u8> {
        assert!(self.data_len.is_multiple_of(PART), "parts follow a part cut short");
        let mut bytes = Vec::new();
        for (part, packed) in data.chunks(PART as usize).zip(pack_parts(data)) {
            let offset = self.end.next_multiple_of(ALIGN);
            bytes.resize(bytes.len() + (offset - self.end) as usize, 0);
            let (stored, encoding) = match &packed {
                Some(packed) => (&packed[..], Encoding::Zstd),
                None => (part, Encoding::Raw),
            };
            bytes.extend_from_slice(stored);
            let len = stored.len() as u64;
            self.stored.push(Stored { offset, len, crc: crc(stored), encoding });
            self.data_len += part.len() as u64;
            self.end = offset + len;
        }
        bytes
    }

    /// The object's last bytes, which follow those [`Packer::push`] returned: `footer`, the table,
    /// and the trailer.
    pub fn finish(self, footer: &[u8]) -> Vec<u8> {
        let mut bytes = footer.to_vec();
        let table = bytes.len();
        for stored in &self.stored {
            bytes.extend_from_slice(&stored.offset.to_le_bytes());
            bytes.extend_from_slice(&(stored.len as u32).to_le_bytes());
            bytes.extend_from_slice(&stored.crc.to_le_bytes());
            bytes.push(match stored.encoding {
                Encoding::Raw => 0,
                Encoding::Zstd => 1,
            });
        }
        bytes.extend_from_slice(&self.data_len.to_le_bytes());
        bytes.extend_from_slice(&self.end.to_le_bytes());
        bytes.extend_from_slice(&(footer.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&crc(footer).to_le_bytes());
        // The table's CRC covers the lengths and the footer's CRC before it.
        bytes.extend_from_slice(&crc(&bytes[table..]).to_le_bytes());
        bytes.extend_from_slice(&MAGIC);
        bytes
    }
}

/// The bytes of an object of `data` and `footer`.
pub fn framed(data: &[u8], footer: &[u8]) -> Vec<u8> {
    let mut packer = Packer::new();
    let mut bytes = packer.push(data);
    bytes.extend_from_slice(&packer.finish(footer));
    bytes
}

/// Where the pieces of an object [`framed`] are, as its trailer says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub data_len: u64,
    /// The bytes of the parts stored, which the footer follows.
    pub stored_len: u64,
    pub footer_len: u64,
    footer_crc: u32,
    table_crc: u32,
}

impl Frame {
    /// The frame an object's last bytes, its [`TRAILER`] at their end, give.
    pub fn of_trailer(bytes: &[u8]) -> Result<Self, Corrupt> {
        let at = bytes.len().checked_sub(TRAILER as usize).ok_or_else(|| corrupt("trailer"))?;
        let trailer = &bytes[at..];
        if trailer[TRAILER as usize - MAGIC.len()..] != MAGIC {
            return Err(corrupt("trailer, by its magic"));
        }
        let frame = Self {
            data_len: u64_at(trailer, 0),
            stored_len: u64_at(trailer, 8),
            footer_len: u64_at(trailer, 16),
            footer_crc: u32_at(trailer, 24),
            table_crc: u32_at(trailer, 28),
        };
        // Lengths so large that the object's would overflow are corrupt.
        (frame.data_len.checked_add(PART))
            .and_then(|_| frame.stored_len.checked_add(frame.footer_len))
            .and_then(|end| end.checked_add(frame.parts().checked_mul(ENTRY)?))
            .and_then(|end| end.checked_add(TRAILER))
            .ok_or_else(|| corrupt("trailer, by its lengths"))?;
        Ok(frame)
    }

    /// The frame of a whole object.
    pub fn of(bytes: &[u8]) -> Result<Self, Corrupt> {
        let frame = Self::of_trailer(bytes)?;
        if frame.size() != bytes.len() as u64 {
            return Err(corrupt("length, by its trailer"));
        }
        Ok(frame)
    }

    pub fn parts(&self) -> u64 {
        self.data_len.div_ceil(PART)
    }

    /// Where part `part` is in the data.
    pub fn part(&self, part: u64) -> Range<u64> {
        (part * PART).min(self.data_len)..((part + 1) * PART).min(self.data_len)
    }

    /// The parts `range` of the data touches.
    pub fn parts_of(&self, range: Range<u64>) -> Range<u64> {
        if range.is_empty() {
            return 0..0;
        }
        range.start / PART..range.end.div_ceil(PART)
    }

    pub fn footer(&self) -> Range<u64> {
        self.stored_len..self.stored_len + self.footer_len
    }

    pub fn table(&self) -> Range<u64> {
        let at = self.footer().end;
        at..at + ENTRY * self.parts()
    }

    /// What follows the parts: the footer, the table and the trailer.
    pub fn tail(&self) -> Range<u64> {
        self.stored_len..self.size()
    }

    /// The object's length.
    pub fn size(&self) -> u64 {
        self.table().end + TRAILER
    }

    /// The footer in `bytes`, checked against its CRC.
    pub fn check_footer<'a>(&self, bytes: &'a [u8]) -> Result<&'a [u8], Corrupt> {
        if bytes.len() as u64 != self.footer_len || crc(bytes) != self.footer_crc {
            return Err(corrupt("footer, by its CRC"));
        }
        Ok(bytes)
    }

    /// Where its parts are stored, as the table in `suffix`, the object's last bytes, which must
    /// hold it, says.
    pub fn stored_in(&self, suffix: &[u8]) -> Result<Vec<Stored>, Corrupt> {
        let at = (suffix.len() as u64 + self.table().start)
            .checked_sub(self.size())
            .ok_or_else(|| corrupt("table, cut short"))? as usize;
        let checked = &suffix[at..suffix.len() - 4 - MAGIC.len()];
        if crc(checked) != self.table_crc {
            return Err(corrupt("table, by its CRC"));
        }
        let table = &suffix[at..suffix.len() - TRAILER as usize];
        let mut stored = Vec::with_capacity(self.parts() as usize);
        let mut end = 0;
        for (part, entry) in table.chunks(ENTRY as usize).enumerate() {
            let encoding = match entry[16] {
                0 => Encoding::Raw,
                1 => Encoding::Zstd,
                other => return Err(corrupt(format_args!("part {part}'s encoding {other}"))),
            };
            let entry = Stored {
                offset: u64_at(entry, 0),
                len: u32_at(entry, 8) as u64,
                crc: u32_at(entry, 12),
                encoding,
            };
            let len = self.part(part as u64).end - self.part(part as u64).start;
            let fits = match encoding {
                Encoding::Raw => entry.len == len,
                Encoding::Zstd => entry.len > 0 && entry.len < len,
            };
            if !fits
                || !entry.offset.is_multiple_of(ALIGN)
                || entry.offset < end
                || entry.range().end > self.stored_len
            {
                return Err(corrupt(format_args!("part {part}'s place")));
            }
            end = entry.range().end;
            stored.push(entry);
        }
        Ok(stored)
    }
}

/// An object's frame, and where its parts are stored: what a reader of its data needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parts {
    pub frame: Frame,
    pub stored: Vec<Stored>,
}

impl Parts {
    /// The parts of the object whose last bytes are `suffix`, which must hold its table.
    pub fn of_suffix(suffix: &[u8]) -> Result<Self, Corrupt> {
        let frame = Frame::of_trailer(suffix)?;
        Ok(Self { frame, stored: frame.stored_in(suffix)? })
    }

    /// The parts of a whole object.
    pub fn of(bytes: &[u8]) -> Result<Self, Corrupt> {
        let frame = Frame::of(bytes)?;
        Ok(Self { frame, stored: frame.stored_in(bytes)? })
    }

    /// Part `part`'s place in the data, and where it is stored.
    pub fn part(&self, part: u64) -> Result<(Range<u64>, Stored), Corrupt> {
        let stored = self.stored.get(part as usize).ok_or_else(|| corrupt("part, past its end"))?;
        Ok((self.frame.part(part), *stored))
    }

    /// The bytes `range` of the data, reading only the stored bytes of the parts it touches, by
    /// `read`, which gives the bytes of a range of the object.
    pub fn read<E: From<Corrupt>>(
        &self,
        range: Range<u64>,
        mut read: impl FnMut(Range<u64>) -> Result<Vec<u8>, E>,
    ) -> Result<Vec<u8>, E> {
        if range.start > range.end || range.end > self.frame.data_len {
            return Err(corrupt("data, read past its end").into());
        }
        let mut bytes = Vec::with_capacity((range.end - range.start) as usize);
        for part in self.frame.parts_of(range.clone()) {
            let (place, stored) = self.part(part)?;
            let unpacked = stored.unpack(&read(stored.range())?, place.end - place.start)?;
            let from = range.start.max(place.start) - place.start;
            let to = range.end.min(place.end) - place.start;
            bytes.extend_from_slice(&unpacked[from as usize..to as usize]);
        }
        Ok(bytes)
    }
}

/// The data of a whole object [`framed`], each part checked against its CRC.
pub fn data(bytes: &[u8]) -> Result<Vec<u8>, Corrupt> {
    let parts = Parts::of(bytes)?;
    parts.read(0..parts.frame.data_len, |range| {
        Ok::<_, Corrupt>(bytes[range.start as usize..range.end as usize].to_vec())
    })
}

/// The footer of a whole object [`framed`], checked against its CRC.
pub fn footer(bytes: &[u8]) -> Result<&[u8], Corrupt> {
    let frame = Frame::of(bytes)?;
    let footer = frame.footer();
    frame.check_footer(&bytes[footer.start as usize..footer.end as usize])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Data that compresses: a pattern, which differs between parts.
    fn pattern(len: u64) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8 ^ (i / PART) as u8).collect()
    }

    /// Data that does not: from a xorshift generator.
    fn random(len: u64) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    /// Objects hold their data in parts, compressed, each where the table says, and their footers.
    #[test]
    fn objects_hold_their_parts() {
        let data = pattern(3 * PART + 7);
        let bytes = framed(&data, b"footer");
        let parts = Parts::of(&bytes).unwrap();
        let frame = parts.frame;
        assert_eq!((frame.data_len, frame.footer_len, frame.parts()), (3 * PART + 7, 6, 4));
        assert_eq!(frame.part(3), 3 * PART..3 * PART + 7);
        assert_eq!(frame.size(), bytes.len() as u64);
        assert_eq!(
            parts.stored.iter().map(|stored| stored.encoding).collect::<Vec<_>>(),
            // The last part, of 7 bytes, does not shrink.
            [Encoding::Zstd, Encoding::Zstd, Encoding::Zstd, Encoding::Raw]
        );
        assert!(frame.stored_len < PART / 10, "{} bytes stored", frame.stored_len);
        for (part, stored) in parts.stored.iter().enumerate() {
            assert_eq!(stored.offset % ALIGN, 0);
            let range = frame.part(part as u64);
            let unpacked = stored.unpack(&bytes[stored.offset as usize..], range.end - range.start);
            assert_eq!(unpacked.unwrap(), &data[range.start as usize..range.end as usize]);
        }
        assert_eq!(super::data(&bytes).unwrap(), data);
        assert_eq!(footer(&bytes).unwrap(), b"footer");
        // The table is found from a suffix holding it.
        let suffix = &bytes[frame.table().start as usize - 3..];
        assert_eq!(Parts::of_suffix(suffix).unwrap(), parts);
        assert_eq!(Frame::of_trailer(&bytes[bytes.len() - TRAILER as usize..]).unwrap(), frame);
        assert!(frame.stored_in(&bytes[frame.table().start as usize + 1..]).is_err());

        let empty = framed(&[], &[]);
        assert_eq!(Frame::of(&empty).unwrap().data_len, 0);
        assert_eq!(empty.len() as u64, TRAILER);
        assert_eq!(super::data(&empty).unwrap(), Vec::<u8>::new());

        // Written in pieces, front to back, it is the same object.
        let mut packer = Packer::new();
        let mut pieces = packer.push(&data[..2 * PART as usize]);
        assert_eq!(packer.end(), pieces.len() as u64);
        pieces.extend(packer.push(&data[2 * PART as usize..]));
        pieces.extend(packer.finish(b"footer"));
        assert_eq!(pieces, bytes);
    }

    /// A read of a range of the data reads and decompresses only the parts the range touches.
    #[test]
    fn ranges_read_only_their_parts() {
        let data = pattern(4 * PART);
        let mut bytes = framed(&data, b"");
        let parts = Parts::of(&bytes).unwrap();
        // Parts the reads below don't touch are corrupt.
        for part in [0, 3] {
            bytes[parts.stored[part].offset as usize] ^= 1;
        }
        let reads = RefCell::new(Vec::new());
        let read = |range: Range<u64>| {
            reads.borrow_mut().push(range.clone());
            Ok::<_, Corrupt>(bytes[range.start as usize..range.end as usize].to_vec())
        };
        let range = PART + 5..2 * PART + 9;
        let got = parts.read(range.clone(), read).unwrap();
        assert_eq!(got, &data[range.start as usize..range.end as usize]);
        assert_eq!(*reads.borrow(), [parts.stored[1].range(), parts.stored[2].range()]);
        reads.borrow_mut().clear();
        assert_eq!(parts.read(2 * PART + 1..2 * PART + 1, read).unwrap(), Vec::<u8>::new());
        assert!(reads.borrow().is_empty());
        assert_eq!(
            parts.read(3 * PART..3 * PART + 1, read),
            Err(Corrupt("an object's part, by its CRC".into()))
        );
        assert_eq!(
            parts.read(4 * PART..4 * PART + 1, read),
            Err(Corrupt("an object's data, read past its end".into()))
        );
    }

    /// Parts that zstd does not shrink are stored raw: incompressible data costs the table, and
    /// nothing more.
    #[test]
    fn incompressible_data_costs_its_table() {
        let data = random(2 * PART + 4096);
        let bytes = framed(&data, b"footer");
        let parts = Parts::of(&bytes).unwrap();
        assert!(parts.stored.iter().all(|stored| stored.encoding == Encoding::Raw));
        assert_eq!(parts.frame.stored_len, data.len() as u64);
        assert_eq!(bytes.len() as u64, data.len() as u64 + 6 + 3 * ENTRY + TRAILER);
        assert_eq!(&bytes[..data.len()], &data[..]);
        assert_eq!(super::data(&bytes).unwrap(), data);
    }

    /// Objects cut short, or with any byte of them changed, fail to read, without panicking.
    #[test]
    fn corrupt_objects_fail() {
        let data = [pattern(PART + 100), random(PART / 2)].concat();
        let bytes = framed(&data, b"footer");
        let read = |bytes: &[u8]| -> Result<(Vec<u8>, Vec<u8>), Corrupt> {
            let parts = Parts::of(bytes)?;
            let data = parts.read(0..parts.frame.data_len, |range| {
                bytes
                    .get(range.start as usize..range.end as usize)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| Corrupt("past the end".into()))
            })?;
            Ok((data, footer(bytes)?.to_vec()))
        };
        assert_eq!(read(&bytes).unwrap(), (data.clone(), b"footer".to_vec()));
        for len in 0..bytes.len() {
            assert!(read(&bytes[..len]).is_err(), "cut to {len}");
        }
        for start in 1..bytes.len() {
            assert!(read(&bytes[start..]).is_err(), "from {start}");
        }
        // Each byte of the tail, and of every few of the parts' stored bytes, changed, is found.
        let parts = Parts::of(&bytes).unwrap();
        let stored = parts.stored.iter().flat_map(|stored| stored.range().step_by(509));
        for at in stored.chain(parts.frame.tail()) {
            let mut flipped = bytes.clone();
            flipped[at as usize] ^= 0x10;
            assert!(read(&flipped).is_err(), "byte {at} flipped");
        }
        // A part's stored bytes, cut short, or of another part.
        let (place, stored) = parts.part(0).unwrap();
        let len = place.end - place.start;
        let first = &bytes[stored.range().start as usize..stored.range().end as usize];
        assert!(stored.unpack(&first[..first.len() - 1], len).is_err());
        let (_, second) = parts.part(1).unwrap();
        assert!(second.unpack(first, len).is_err());
        // Data that is no zstd frame, though its CRC matches.
        let garbage = vec![7; 100];
        let lying = Stored { offset: 0, len: 100, crc: crc(&garbage), encoding: Encoding::Zstd };
        assert!(lying.unpack(&garbage, PART).is_err());
        assert!(parts.part(2).is_err());
    }
}
