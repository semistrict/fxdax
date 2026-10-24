// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Files of extents' data as reads find them: raw, as written, or framed (`extent_frame`), their
//! parts compressed; and the parts of framed files read lately, unpacked.

use crate::dir::File;
use crate::sync::Mutex;
use anyhow::{Error, bail};
use extent_frame::{ALIGN, Corrupt, Encoding, Frame, PART, Parts, Stored, TRAILER};
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::io;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

    /// Its `len` bytes from `skip`, in the buffer still.
    fn within(mut self, skip: usize, len: usize) -> Self {
        assert!(skip + len <= self.len, "{len} bytes from {skip} of {}", self.len);
        self.start += skip;
        self.len = len;
        self
    }
}

/// `len` bytes at `at` in `file`, read in whole aligned blocks, as reads of extents' files that
/// bypass the host's page cache must be (`O_DIRECT` on Linux), and left in them: a part's stored
/// bytes are read on each page-in that unpacks it (TASK-92.19).
pub(crate) fn read_aligned<F: File>(file: &F, at: u64, len: usize) -> io::Result<Aligned> {
    let align = ALIGNMENT as u64;
    let start = at / align * align;
    let end = (at + len as u64).div_ceil(align) * align;
    let mut buf = Aligned::new((end - start) as usize);
    file.read_at(buf.as_mut_slice(), start)?;
    Ok(buf.within((at - start) as usize, len))
}

/// The whole of `file`, read in aligned blocks.
pub fn read_whole<F: File>(file: &F) -> io::Result<Vec<u8>> {
    Ok(read_aligned(file, 0, file.size()? as usize)?.as_slice().to_vec())
}

/// The parts of the framed file `file`, and its footer, if it is whole: its trailer, table and
/// footer pass their checks. Its parts are checked as they are read.
pub(crate) fn read_frame<F: File>(file: &F) -> Result<Option<(Parts, Vec<u8>)>, Error> {
    let size = file.size()?;
    let Some(at) = size.checked_sub(TRAILER) else { return Ok(None) };
    let trailer = read_aligned(file, at, TRAILER as usize)?;
    let Ok(frame) = Frame::of_trailer(trailer.as_slice()) else { return Ok(None) };
    if frame.size() != size {
        return Ok(None);
    }
    let tail = read_aligned(file, frame.stored_len, (size - frame.stored_len) as usize)?;
    let tail = tail.as_slice();
    let Ok(stored) = frame.stored_in(tail) else { return Ok(None) };
    let Ok(footer) = frame.check_footer(&tail[..frame.footer_len as usize]) else {
        return Ok(None);
    };
    Ok(Some((Parts { frame, stored }, footer.to_vec())))
}

/// fxdax: the parts of the framed file `file`, if its trailer and table pass their checks: read
/// from the blocks they are in alone, the last, but for a table that begins before them, and not
/// its footer, which reading its data needs not (TASK-92.2). Its parts are checked as they are
/// read.
pub(crate) fn read_parts<F: File>(file: &F) -> Result<Option<Parts>, Error> {
    let size = file.size()?;
    let Some(trailer) = size.checked_sub(TRAILER) else { return Ok(None) };
    let last = trailer / ALIGN * ALIGN;
    let suffix = read_aligned(file, last, (size - last) as usize)?;
    let Ok(frame) = Frame::of_trailer(suffix.as_slice()) else { return Ok(None) };
    if frame.size() != size {
        return Ok(None);
    }
    let table = frame.table().start;
    let suffix =
        if table >= last { suffix } else { read_aligned(file, table, (size - table) as usize)? };
    let Ok(stored) = frame.stored_in(suffix.as_slice()) else { return Ok(None) };
    Ok(Some(Parts { frame, stored }))
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

    /// The framed file `file`, which must be whole, as far as its trailer and table say; `path`
    /// names it.
    pub(crate) fn open(file: F, path: &str) -> Result<Self, Error> {
        match read_parts(&file)? {
            Some(parts) => Ok(Self::new(Arc::new(file), parts)),
            None => bail!("{path} is not a whole framed file"),
        }
    }

    /// The bytes the file takes.
    pub(crate) fn size(&self) -> u64 {
        self.parts.frame.size()
    }

    /// Whether no read uses it: only what keeps it open holds it.
    fn is_idle(self: &Arc<Self>) -> bool {
        Arc::strong_count(self) == 1 && Arc::strong_count(&self.file) == 1
    }
}

/// fxdax: framed files kept open, by their keys: at most `limit` of them, but for those reads use,
/// the least lately read of the others closing as more open; not all of them, which reads would
/// open again, each opening anew a file whose parts unpacked no read finds then (TASK-92.2).
pub(crate) struct Files<K, F> {
    /// Each file, and when it was last read.
    files: HashMap<K, (Arc<Framed<F>>, u64)>,
    by_use: BTreeMap<u64, K>,
    clock: u64,
    limit: usize,
}

impl<K: Copy + Eq + Hash, F: File> Files<K, F> {
    pub(crate) fn new(limit: usize) -> Self {
        Self { files: HashMap::new(), by_use: BTreeMap::new(), clock: 0, limit }
    }

    /// The file of `key`, if open, read now.
    pub(crate) fn get(&mut self, key: K) -> Option<Arc<Framed<F>>> {
        let (file, used) = self.files.get_mut(&key)?;
        self.clock += 1;
        self.by_use.remove(used);
        *used = self.clock;
        self.by_use.insert(self.clock, key);
        Some(file.clone())
    }

    /// Keeps `file` open, the file of `key`, read now, unless one is open already, as one opened
    /// meanwhile: returns the one kept. The least lately read of those no read uses close, while
    /// more than `limit` are open.
    pub(crate) fn insert(&mut self, key: K, file: Arc<Framed<F>>) -> Arc<Framed<F>> {
        if let Some(open) = self.get(key) {
            return open;
        }
        self.clock += 1;
        self.files.insert(key, (file.clone(), self.clock));
        self.by_use.insert(self.clock, key);
        let excess = self.files.len().saturating_sub(self.limit);
        let idle: Vec<(u64, K)> = (self.by_use.iter())
            .filter(|(_, key)| self.files[*key].0.is_idle())
            .take(excess)
            .map(|(used, key)| (*used, *key))
            .collect();
        for (used, key) in idle {
            self.by_use.remove(&used);
            self.files.remove(&key);
        }
        file
    }

    /// Closes the file of `key`, if open.
    pub(crate) fn remove(&mut self, key: K) {
        if let Some((_, used)) = self.files.remove(&key) {
            self.by_use.remove(&used);
        }
    }

    /// Closes them all.
    pub(crate) fn clear(&mut self) {
        self.files.clear();
        self.by_use.clear();
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
    /// `unpacked` keeps them, or not. fxdax: fails with [`Corrupt`] if they are past its data, as
    /// an index would place them that names another file made at its path (TASK-79.38).
    pub(crate) fn locate(
        &self,
        at: u64,
        len: u64,
        unpacked: &Unpacked,
        parts: &mut Vec<Part<F>>,
    ) -> Result<(), Corrupt> {
        let framed = match self {
            Self::Raw(file) => {
                push_data(parts, file, at, len, true);
                return Ok(());
            }
            Self::Framed(framed) => framed,
        };
        let frame = &framed.parts.frame;
        if at.checked_add(len).is_none_or(|end| end > frame.data_len) {
            let data_len = frame.data_len;
            return Err(Corrupt(format!("{len} bytes at {at}, past a framed file's {data_len}")));
        }
        for part in frame.parts_of(at..at + len) {
            let place = frame.part(part);
            let stored = framed.parts.stored[part as usize];
            let (from, to) = (at.max(place.start), (at + len).min(place.end));
            let (skip, len) = (from - place.start, to - from);
            match stored.encoding {
                Encoding::Raw => push_data(parts, &framed.file, stored.offset + skip, len, false),
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
        Ok(())
    }
}

/// Adds `len` bytes at `offset` in `file`, raw if `raw`, to `parts`, extending the last if they
/// follow it.
fn push_data<F>(parts: &mut Vec<Part<F>>, file: &Arc<F>, offset: u64, len: u64, raw: bool) {
    if let Some(Part::Data { file: last, offset: last_offset, len: last_len, .. }) =
        parts.last_mut()
        && Arc::ptr_eq(last, file)
        && *last_offset + *last_len == offset
    {
        *last_len += len;
        return;
    }
    parts.push(Part::Data { file: file.clone(), offset, len, raw });
}

/// A part of a range of an extent: bytes of a file, raw; bytes of a part of a file stored
/// compressed, which [`crate::Store::unpack`] unpacks from its stored bytes; bytes of a part
/// unpacked lately; or zeros, where nothing was written.
pub enum Part<F> {
    Data {
        file: Arc<F>,
        offset: u64,
        len: u64,
        /// fxdax: whether its file is raw, written lately through the host's page cache, which
        /// reads of it go through ([`File::read_cached`](crate::File::read_cached)).
        raw: bool,
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
    /// CRC, and kept in `unpacked`: unpacked into what keeps it, rather than copied there
    /// (TASK-92.19).
    pub(crate) fn unpack(&self, bytes: &[u8], unpacked: &Unpacked) -> Result<Arc<[u8]>, Error> {
        debug_assert!(self.len <= PART);
        let mut part: Arc<[u8]> = std::iter::repeat_n(0, self.len as usize).collect();
        let out = Arc::get_mut(&mut part).expect("a part just made is its own");
        self.stored.unpack_into(bytes, out)?;
        UNPACKS.fetch_add(1, Ordering::Relaxed);
        unpacked.put(self.key, part.clone());
        Ok(part)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dir as _, MemDir};

    /// fxdax: bytes past a framed file's data are located nowhere in it: an index naming them, as
    /// one of a file gone and made anew would, is corrupt, which reads fail on, and the store's
    /// state is not poisoned by a panic (TASK-79.38).
    #[test]
    fn bytes_past_the_data_are_located_nowhere() {
        let dir = MemDir::new();
        let file = dir.create("framed", true).unwrap();
        let bytes = extent_frame::framed(&[7; 4096], &[]);
        file.write_at(&bytes, 0).unwrap();
        let framed = Framed::new(Arc::new(file), Parts::of(&bytes).unwrap());
        let source = Source::Framed(Arc::new(framed));
        let mut parts = Vec::new();
        let corrupt = source.locate(2 * PART, 4096, &Unpacked::default(), &mut parts).unwrap_err();
        assert_eq!(
            corrupt.to_string(),
            format!("4096 bytes at {}, past a framed file's 4096", 2 * PART)
        );
        assert_eq!(parts.len(), 0);
        source.locate(0, 4096, &Unpacked::default(), &mut parts).unwrap();
        assert_eq!(parts.len(), 1);
    }

    /// fxdax: of the files kept open, the least lately read closes as one more opens past the
    /// limit, but for those reads use; and a file opened again meanwhile is not kept, but the one
    /// kept already (TASK-92.2).
    #[test]
    fn files_close_the_least_lately_read_first() {
        let (dir, made) = (MemDir::new(), std::cell::Cell::new(0));
        // A file of its own each time, opened anew.
        let framed = |key: u64| {
            made.set(made.get() + 1);
            let file = dir.create(&made.get().to_string(), true).unwrap();
            let bytes = extent_frame::framed(&[key as u8; 4096], &[]);
            file.write_at(&bytes, 0).unwrap();
            Arc::new(Framed::new(Arc::new(file), Parts::of(&bytes).unwrap()))
        };
        let open = |files: &Files<u64, _>| {
            let mut open: Vec<u64> = files.files.keys().copied().collect();
            open.sort();
            open
        };
        let mut files = Files::new(2);
        files.insert(1, framed(1));
        files.insert(2, framed(2));
        files.get(1);
        files.insert(3, framed(3));
        assert_eq!(open(&files), [1, 3]);
        // 1, read last but for 3, is in use.
        let reading = files.get(1).unwrap().file.clone();
        files.get(3);
        files.insert(4, framed(4));
        assert_eq!(open(&files), [1, 4]);
        drop(reading);
        let again = framed(4);
        assert!(!Arc::ptr_eq(&files.insert(4, again.clone()), &again));
        files.insert(5, framed(5));
        assert_eq!(open(&files), [4, 5]);
    }
}
