// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Files of extents' data as reads find them: raw, as written, or framed (`extent_frame`), their
//! parts compressed; and the parts of framed files read lately, unpacked.

use crate::dir::File;
use anyhow::{Error, bail};
use extent_frame::{ALIGN, Encoding, Frame, PART, Parts, Stored, TRAILER};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The alignment of I/O that bypasses the host's page cache: the largest block size a store has.
pub(crate) const ALIGNMENT: usize = 4096;

/// A buffer aligned for I/O that bypasses the host's page cache.
pub struct Aligned {
    bytes: Vec<u8>,
    start: usize,
    len: usize,
}

impl Aligned {
    pub fn new(len: usize) -> Self {
        let bytes = vec![0; len + ALIGNMENT];
        let start = bytes.as_ptr().align_offset(ALIGNMENT);
        Self { bytes, start, len }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[self.start..self.start + self.len]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.bytes[self.start..self.start + self.len]
    }
}

/// `len` bytes at `at` in `file`, read in whole aligned blocks, as reads of extents' files that
/// bypass the host's page cache must be (`O_DIRECT` on Linux).
pub(crate) fn read_aligned<F: File>(file: &F, at: u64, len: usize) -> io::Result<Vec<u8>> {
    let align = ALIGNMENT as u64;
    let start = at / align * align;
    let end = (at + len as u64).div_ceil(align) * align;
    let mut buf = Aligned::new((end - start) as usize);
    file.read_at(buf.as_mut_slice(), start)?;
    Ok(buf.as_slice()[(at - start) as usize..][..len].to_vec())
}

/// The whole of `file`, read in aligned blocks.
pub fn read_whole<F: File>(file: &F) -> io::Result<Vec<u8>> {
    read_aligned(file, 0, file.size()? as usize)
}

/// The parts of the framed file `file`, and its footer, if it is whole: its trailer, table and
/// footer pass their checks. Its parts are checked as they are read.
pub(crate) fn read_frame<F: File>(file: &F) -> Result<Option<(Parts, Vec<u8>)>, Error> {
    let size = file.size()?;
    let Some(at) = size.checked_sub(TRAILER) else { return Ok(None) };
    let trailer = read_aligned(file, at, TRAILER as usize)?;
    let Ok(frame) = Frame::of_trailer(&trailer) else { return Ok(None) };
    if frame.size() != size {
        return Ok(None);
    }
    let tail = read_aligned(file, frame.stored_len, (size - frame.stored_len) as usize)?;
    let Ok(stored) = frame.stored_in(&tail) else { return Ok(None) };
    let Ok(footer) = frame.check_footer(&tail[..frame.footer_len as usize]) else {
        return Ok(None);
    };
    Ok(Some((Parts { frame, stored }, footer.to_vec())))
}

/// Numbers the framed files opened, for the parts unpacked to name them by.
static OPENED: AtomicU64 = AtomicU64::new(0);

/// A framed file, open, and where its parts are.
pub(crate) struct Framed<F> {
    pub file: Arc<F>,
    pub parts: Parts,
    /// Its opening's number.
    id: u64,
}

impl<F: File> Framed<F> {
    pub(crate) fn new(file: Arc<F>, parts: Parts) -> Self {
        Self { file, parts, id: OPENED.fetch_add(1, Ordering::Relaxed) }
    }

    /// The framed file `file`, which must be whole; `path` names it.
    pub(crate) fn open(file: F, path: &str) -> Result<Self, Error> {
        match read_frame(&file)? {
            Some((parts, _)) => Ok(Self::new(Arc::new(file), parts)),
            None => bail!("{path} is not a whole framed file"),
        }
    }

    /// The bytes the file takes.
    pub(crate) fn size(&self) -> u64 {
        self.parts.frame.size()
    }
}

/// A file of extents' data, open.
pub(crate) enum Source<F> {
    /// Raw, as written: an open segment's, or an open extent's own.
    Raw(Arc<F>),
    /// Framed: a closed segment's, or a sealed extent's own.
    Framed(Arc<Framed<F>>),
}

impl<F> Clone for Source<F> {
    fn clone(&self) -> Self {
        match self {
            Self::Raw(file) => Self::Raw(file.clone()),
            Self::Framed(framed) => Self::Framed(framed.clone()),
        }
    }
}

impl<F: File> Source<F> {
    pub(crate) fn file(&self) -> &Arc<F> {
        match self {
            Self::Raw(file) => file,
            Self::Framed(framed) => &framed.file,
        }
    }

    /// The parts of where `len` bytes at `at` in its data are, to `parts`: unpacked lately, as
    /// `unpacked` keeps them, or not.
    pub(crate) fn locate(&self, at: u64, len: u64, unpacked: &Unpacked, parts: &mut Vec<Part<F>>) {
        let framed = match self {
            Self::Raw(file) => return push_data(parts, file, at, len),
            Self::Framed(framed) => framed,
        };
        let frame = &framed.parts.frame;
        for part in frame.parts_of(at..at + len) {
            let place = frame.part(part);
            let stored = framed.parts.stored[part as usize];
            let (from, to) = (at.max(place.start), (at + len).min(place.end));
            let (skip, len) = (from - place.start, to - from);
            match stored.encoding {
                Encoding::Raw => push_data(parts, &framed.file, stored.offset + skip, len),
                Encoding::Zstd => {
                    let key = (framed.id, part);
                    parts.push(match unpacked.get(key) {
                        Some(bytes) => Part::Unpacked { bytes, skip, len },
                        None => Part::Packed {
                            part: Packed {
                                file: framed.file.clone(),
                                stored,
                                len: place.end - place.start,
                                key,
                            },
                            skip,
                            len,
                        },
                    });
                }
            }
        }
    }
}

/// Adds `len` bytes at `offset` in `file` to `parts`, extending the last if they follow it.
fn push_data<F>(parts: &mut Vec<Part<F>>, file: &Arc<F>, offset: u64, len: u64) {
    if let Some(Part::Data { file: last, offset: last_offset, len: last_len }) = parts.last_mut()
        && Arc::ptr_eq(last, file)
        && *last_offset + *last_len == offset
    {
        *last_len += len;
        return;
    }
    parts.push(Part::Data { file: file.clone(), offset, len });
}

/// A part of a range of an extent: bytes of a file, raw; bytes of a part of a file stored
/// compressed, which [`crate::Store::unpack`] unpacks from its stored bytes; bytes of a part
/// unpacked lately; or zeros, where nothing was written.
pub enum Part<F> {
    Data {
        file: Arc<F>,
        offset: u64,
        len: u64,
    },
    /// `len` bytes from `skip` in the part.
    Packed {
        part: Packed<F>,
        skip: u64,
        len: u64,
    },
    Unpacked {
        bytes: Arc<[u8]>,
        skip: u64,
        len: u64,
    },
    Zeros(u64),
}

impl<F> Part<F> {
    /// The bytes of the extent it is.
    pub fn len(&self) -> u64 {
        match self {
            Self::Data { len, .. } | Self::Packed { len, .. } | Self::Unpacked { len, .. } => *len,
            Self::Zeros(len) => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A part of a file, stored compressed.
pub struct Packed<F> {
    pub file: Arc<F>,
    pub stored: Stored,
    /// Its length, unpacked.
    len: u64,
    /// Its name among the parts unpacked.
    key: Key,
}

impl<F> Packed<F> {
    /// The range of the file a read of its stored bytes reads, in whole aligned blocks: as far as
    /// the file goes, which may end before the range does.
    pub fn read_range(&self) -> Range<u64> {
        let start = self.stored.offset;
        start..(start + self.stored.len).next_multiple_of(ALIGN)
    }

    /// The part, unpacked from `bytes`, its stored bytes and any after them, checked against its
    /// CRC, and kept in `unpacked`.
    pub(crate) fn unpack(&self, bytes: &[u8], unpacked: &Unpacked) -> Result<Arc<[u8]>, Error> {
        debug_assert!(self.len <= PART);
        let bytes: Arc<[u8]> = self.stored.unpack(bytes, self.len)?.into();
        UNPACKS.fetch_add(1, Ordering::Relaxed);
        unpacked.put(self.key, bytes.clone());
        Ok(bytes)
    }
}

/// How many parts stored compressed this process unpacked.
static UNPACKS: AtomicU64 = AtomicU64::new(0);

/// How many parts stored compressed this process unpacked (decompressed) since it started: a
/// part read again once it is no longer kept unpacked is unpacked again.
pub fn unpacks() -> u64 {
    UNPACKS.load(Ordering::Relaxed)
}

/// The parts of framed files unpacked lately, by their files' openings and their numbers: most
/// reads of extents are of neighbouring blocks, which a part holds.
#[derive(Default)]
pub(crate) struct Unpacked {
    kept: Mutex<Kept>,
}

/// A part unpacked: its file's opening, and its number.
type Key = (u64, u64);

#[derive(Default)]
struct Kept {
    /// Each part, and when it was last read.
    parts: HashMap<Key, (Arc<[u8]>, u64)>,
    by_use: BTreeMap<u64, Key>,
    clock: u64,
}

/// How many parts are kept unpacked.
const UNPACKED: usize = 32;

impl Unpacked {
    fn get(&self, key: Key) -> Option<Arc<[u8]>> {
        let mut kept = self.kept.lock().unwrap();
        let kept = &mut *kept;
        let (bytes, used) = kept.parts.get_mut(&key)?;
        kept.clock += 1;
        kept.by_use.remove(used);
        *used = kept.clock;
        kept.by_use.insert(kept.clock, key);
        Some(bytes.clone())
    }

    fn put(&self, key: Key, bytes: Arc<[u8]>) {
        let mut kept = self.kept.lock().unwrap();
        let kept = &mut *kept;
        kept.clock += 1;
        if let Some((_, used)) = kept.parts.insert(key, (bytes, kept.clock)) {
            kept.by_use.remove(&used);
        }
        kept.by_use.insert(kept.clock, key);
        while kept.parts.len() > UNPACKED {
            let (_, oldest) = kept.by_use.pop_first().unwrap();
            kept.parts.remove(&oldest);
        }
    }
}
