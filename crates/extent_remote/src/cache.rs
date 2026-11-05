// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A host's copies of the parts of objects of its bucket it fetched (doc-7, Reading: the NVMe
//! cache). Each object's is a sparse file of its size, at its key, holding its tail,
//! which says where its parts are stored, and those of its parts kept, as stored, compressed or
//! not, at their offsets, the rest holes. Those read least recently are evicted, by punching holes,
//! an object's tail with its last part, so that they take no more than a capacity and the
//! filesystem keeps a floor of free space. They are copies of what is in the bucket, so none is
//! data the bucket lacks; a host starts with none. What it gives, it checks: a tail against its
//! CRCs, a part against the one its tail gives, dropping what fails.

use crate::object;
use extent_frame::{Parts, Stored};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io;
use std::ops::Range;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The number an object's tail is kept by, among its parts, which count from 0: its footer, table
/// and trailer, after its parts, which a fetcher of another of the host's stores reads here rather
/// than from the bucket (TASK-92.22), and without which none of its parts can be read.
const TAIL: u64 = u64::MAX;

/// The cache's lock is held only to change what it keeps, not across its files' I/O: a part's write,
/// or its eviction, is done outside it, the part busy meanwhile, and an object's file removed so
/// too, the object's key removing; neither is kept or read until done (TASK-92.21).
#[derive(Debug)]
pub struct Cache {
    files: Files,
    capacity: u64,
    floor: u64,
    kept: Mutex<Kept>,
}

#[derive(Debug, Default)]
struct Kept {
    objects: HashMap<String, Object>,
    /// The parts kept, by when they were last read.
    by_use: BTreeMap<u64, (String, u64)>,
    /// The objects whose files are being removed.
    removing: HashSet<String>,
    clock: u64,
    /// The bytes of the parts kept, and being written.
    bytes: u64,
}

/// An object the cache keeps some of, or is writing: in the cache's index while it keeps or
/// writes any of it. Its parts are kept only with its tail, and it is read only by it. It is
/// readable, its tail being written, or forgotten.
#[derive(Debug)]
struct Object {
    /// Where its parts are stored, as its tail says.
    parts: Parts,
    /// The parts kept, its tail among them once written ([`TAIL`]), and when each was last read,
    /// its tail after its parts: their places in `by_use`.
    kept: HashMap<u64, u64>,
    /// The parts being written, or punched.
    busy: HashMap<u64, Busy>,
    /// Dropped while parts of it were busy: nothing more of it is kept, and its file goes, with
    /// what they wrote, once they are done.
    forgotten: bool,
}

/// What is being done with a part busy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Busy {
    Writing,
    Punching,
}

impl Object {
    /// An object of parts stored as `parts` says, whose part `part` is being written.
    fn writing(parts: Parts, part: u64) -> Self {
        let busy = HashMap::from([(part, Busy::Writing)]);
        Self { parts, kept: HashMap::new(), busy, forgotten: false }
    }

    /// Where part `part`, or its tail, is in its file.
    fn place(&self, part: u64) -> Range<u64> {
        match part {
            TAIL => self.parts.frame.tail(),
            part => self.parts.stored[part as usize].range(),
        }
    }

    fn len(&self, part: u64) -> u64 {
        let place = self.place(part);
        place.end - place.start
    }

    /// Whether its tail is kept: whether it can be read.
    fn readable(&self) -> bool {
        self.kept.contains_key(&TAIL)
    }
}

/// What evicting a part leaves to do with its object's file, outside the cache's lock.
#[derive(Debug)]
enum Evicted {
    /// Punch the part's hole: where it is in the file.
    Punch { key: String, part: u64, place: Range<u64> },
    /// Remove the file of the object, whose last part it was.
    Remove(String),
}

impl Kept {
    /// Counts part `part` of the object at `key` read now, and its tail after it.
    fn touch(&mut self, key: &str, part: u64) {
        let parts: &[u64] = if part == TAIL { &[TAIL] } else { &[part, TAIL] };
        for &part in parts {
            let Some(used) =
                self.objects.get_mut(key).and_then(|object| object.kept.get_mut(&part))
            else {
                continue;
            };
            self.clock += 1;
            self.by_use.remove(used);
            *used = self.clock;
            self.by_use.insert(self.clock, (key.to_owned(), part));
        }
    }

    /// Keeps part `part` of the object at `key`, read now.
    fn keep(&mut self, key: &str, part: u64) {
        self.clock += 1;
        let object = self.objects.get_mut(key).expect("an object kept");
        object.kept.insert(part, self.clock);
        self.by_use.insert(self.clock, (key.to_owned(), part));
    }

    /// Evicts what was read least recently, but tails whose objects keep or write other parts, until
    /// `len` bytes more fit in `capacity` and `free` bytes, and those evictions free, leave `floor`
    /// and `needed` more free. Returns whether they do, and what the evictions leave to do.
    fn make_room(
        &mut self,
        (capacity, floor): (u64, u64),
        free: u64,
        len: u64,
        needed: u64,
    ) -> (bool, Vec<Evicted>) {
        let (mut evicted, mut freed, mut from) = (Vec::new(), 0, 0);
        let fits = loop {
            if self.bytes + len <= capacity && free + freed >= floor + needed {
                break true;
            }
            let Some((&used, (key, part))) = self.by_use.range(from..).next() else { break false };
            from = used + 1;
            let (key, part) = (key.clone(), *part);
            let object = &self.objects[&key];
            // A tail goes with its object's last part, and not while one is being written.
            let writing = object.busy.values().any(|busy| *busy == Busy::Writing);
            if part == TAIL && (object.kept.len() > 1 || writing) {
                continue;
            }
            freed += object.len(part);
            evicted.extend(self.evict(&key, part));
        };
        (fits, evicted)
    }

    /// Drops part `part` of the object at `key`, kept: its tail drops the object. Returns what that
    /// leaves to do with its file, the part, or its object, busy until done.
    fn evict(&mut self, key: &str, part: u64) -> Option<Evicted> {
        if part == TAIL {
            return self.forget(key);
        }
        let object = self.objects.get_mut(key).expect("an object kept");
        let used = object.kept.remove(&part).expect("a part kept");
        self.by_use.remove(&used);
        let place = object.place(part);
        self.bytes -= place.end - place.start;
        object.busy.insert(part, Busy::Punching);
        Some(Evicted::Punch { key: key.to_owned(), part, place })
    }

    /// Drops what is kept of the object at `key`. Returns its file to remove, if none of it is
    /// busy: else the last part of it that is removes it once done.
    fn forget(&mut self, key: &str) -> Option<Evicted> {
        let object = self.objects.get_mut(key)?;
        object.forgotten = true;
        for (part, used) in std::mem::take(&mut object.kept) {
            self.by_use.remove(&used);
            self.bytes -= object.len(part);
        }
        self.release(key)
    }

    /// Part `part` of the object at `key`, which was being written, is written, if `written`: kept
    /// then, unless its object was forgotten meanwhile. Returns the object's file to remove, if
    /// that leaves it nothing.
    fn written(&mut self, key: &str, part: u64, written: bool) -> Option<Evicted> {
        let object = self.objects.get_mut(key).expect("an object its busy parts keep");
        if written && !object.forgotten {
            object.busy.remove(&part);
            self.keep(key, part);
            self.touch(key, TAIL);
            return None;
        }
        self.bytes -= object.len(part);
        self.freed(key, part)
    }

    /// Part `part` of the object at `key`, not kept, is busy no more. Returns the object's file to
    /// remove, if that leaves it nothing.
    fn freed(&mut self, key: &str, part: u64) -> Option<Evicted> {
        (self.objects.get_mut(key).expect("an object its busy parts keep").busy).remove(&part);
        self.release(key)
    }

    /// The file of the object at `key` to remove, if it keeps nothing and nothing of it is busy.
    fn release(&mut self, key: &str) -> Option<Evicted> {
        let object = &self.objects[key];
        if !object.kept.is_empty() || !object.busy.is_empty() {
            return None;
        }
        self.objects.remove(key);
        self.removing.insert(key.to_owned());
        Some(Evicted::Remove(key.to_owned()))
    }
}

/// Where the cache keeps its files under the directory it is given, which it empties at start.
const FILES: &str = "fxdax-parts";

impl Cache {
    /// A cache in `dir` of at most `capacity` bytes of objects, keeping `floor` bytes free there.
    pub fn new(dir: &Path, capacity: u64, floor: u64) -> io::Result<Self> {
        let dir = dir.join(FILES);
        match std::fs::remove_dir_all(&dir) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        std::fs::create_dir_all(&dir)?;
        Ok(Self { files: Files { dir }, capacity, floor, kept: Mutex::default() })
    }

    #[cfg(test)]
    pub(crate) fn path(&self, key: &str) -> PathBuf {
        self.files.path(key)
    }

    /// The tail of the object at `key`, if it is kept, checked against its CRCs: one that fails
    /// is dropped, its object with it.
    pub fn tail(&self, key: &str) -> io::Result<Option<object::Tail>> {
        let (place, frame) = {
            let mut kept = self.kept.lock().unwrap();
            let Some(object) = kept.objects.get(key).filter(|object| object.readable()) else {
                return Ok(None);
            };
            let found = (object.place(TAIL), object.parts.frame);
            kept.touch(key, TAIL);
            found
        };
        let Some(bytes) = self.read(key, place)? else { return Ok(None) };
        match object::Tail::of(key, bytes) {
            Ok(tail) if tail.parts.frame == frame => Ok(Some(tail)),
            checked => {
                let failed =
                    checked.err().map_or("its frame changed".into(), |err| err.to_string());
                log::warn!("fxdax: the tail of {key} in the cache: {failed}");
                self.forget(key)?;
                Ok(None)
            }
        }
    }

    /// The stored bytes of part `part` of the object at `key`, if it is kept, stored as `stored`
    /// says (as the reader's tail does), and they pass its CRC: one that fails is dropped.
    pub fn part(&self, key: &str, part: u64, stored: &Stored) -> io::Result<Option<Vec<u8>>> {
        {
            let mut kept = self.kept.lock().unwrap();
            let Some(object) = kept.objects.get(key) else { return Ok(None) };
            if !object.kept.contains_key(&part)
                || object.parts.stored.get(part as usize) != Some(stored)
            {
                return Ok(None);
            }
            kept.touch(key, part);
        }
        let Some(bytes) = self.read(key, stored.range())? else { return Ok(None) };
        if stored.check(&bytes).is_ok() {
            return Ok(Some(bytes));
        }
        log::warn!("fxdax: part {part} of {key} in the cache failed its CRC");
        self.forget_part(key, part)?;
        Ok(None)
    }

    /// The bytes at `place` in the file of the object at `key`: none if the file went, or shrank,
    /// behind the cache's back, which then forgets the object.
    fn read(&self, key: &str, place: Range<u64>) -> io::Result<Option<Vec<u8>>> {
        match self.files.read(key, place) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err)
                if matches!(err.kind(), io::ErrorKind::NotFound | io::ErrorKind::UnexpectedEof) =>
            {
                self.forget(key)?;
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// Keeps `tail`, the tail of the object at `key`, as read, evicting what was read least recently
    /// to make room: unless no room can be made, or its object is being removed, forgotten, or kept
    /// or written already. Another object kept at its key, as one recreated there is, goes first,
    /// unless parts of it are busy.
    pub fn keep_tail(&self, key: &str, tail: &object::Tail) -> io::Result<()> {
        let len = tail.bytes.len() as u64;
        if len > self.capacity {
            return Ok(());
        }
        let free = self.files.free()?;
        let mut kept = self.kept.lock().unwrap();
        if let Some(object) = kept.objects.get(key)
            && object.parts != tail.parts
            && object.busy.is_empty()
        {
            let gone = Vec::from_iter(kept.forget(key));
            drop(kept);
            self.evict(gone)?;
            kept = self.kept.lock().unwrap();
        }
        match kept.objects.get(key) {
            Some(object) if object.readable() && object.parts == tail.parts => {
                kept.touch(key, TAIL);
                return Ok(());
            }
            // Being written, or forgotten; or another object, parts of it busy.
            Some(_) => return Ok(()),
            None if kept.removing.contains(key) => return Ok(()),
            None => {}
        }
        kept.objects.insert(key.to_owned(), Object::writing(tail.parts.clone(), TAIL));
        let (fits, mut evicted) = kept.make_room((self.capacity, self.floor), free, len, len);
        if fits {
            kept.bytes += len;
        } else {
            evicted.extend(kept.freed(key, TAIL));
        }
        drop(kept);
        let evicting = self.evict(evicted);
        if !fits {
            return evicting;
        }
        let frame = tail.parts.frame;
        let written = evicting
            .and_then(|()| self.files.create(key, frame.size()))
            .and_then(|()| self.files.write(key, frame.tail().start, &tail.bytes));
        self.settle(key, TAIL, written)
    }

    /// Keeps `bytes`, the stored bytes of part `part` of the object at `key`, which pass the CRC of
    /// `stored`, where its tail says it is stored, evicting what was read least recently to make
    /// room: unless no room can be made, or its object's tail isn't kept, as stored, or it is kept
    /// or busy already.
    pub fn keep_part(&self, key: &str, part: u64, stored: &Stored, bytes: &[u8]) -> io::Result<()> {
        debug_assert!(stored.check(bytes).is_ok(), "part {part} of {key}, kept unchecked");
        let len = stored.len;
        if len > self.capacity {
            return Ok(());
        }
        let free = self.files.free()?;
        let (fits, evicted) = {
            let mut kept = self.kept.lock().unwrap();
            let Some(object) = kept.objects.get_mut(key) else { return Ok(()) };
            if !object.readable()
                || object.parts.stored.get(part as usize) != Some(stored)
                || object.kept.contains_key(&part)
                || object.busy.contains_key(&part)
            {
                return Ok(());
            }
            object.busy.insert(part, Busy::Writing);
            let (fits, mut evicted) = kept.make_room((self.capacity, self.floor), free, len, len);
            if fits {
                kept.bytes += len;
            } else {
                evicted.extend(kept.freed(key, part));
            }
            (fits, evicted)
        };
        let evicting = self.evict(evicted);
        if !fits {
            return evicting;
        }
        let written =
            evicting.and_then(|()| self.files.write(key, stored.offset, &bytes[..len as usize]));
        self.settle(key, part, written)
    }

    /// Part `part` of the object at `key` is written, or failed to be: kept, or not, and its
    /// object's file removed if that leaves it nothing.
    fn settle(&self, key: &str, part: u64, written: io::Result<()>) -> io::Result<()> {
        let gone = self.kept.lock().unwrap().written(key, part, written.is_ok());
        if let Err(err) = self.evict(Vec::from_iter(gone)) {
            log::warn!("fxdax: removing {key} from the cache: {err}");
        }
        written
    }

    /// Drops the object at `key`: one whose part, read from the bucket, failed the CRC its tail
    /// gives, as one recreated at its key's does.
    pub fn forget(&self, key: &str) -> io::Result<()> {
        let gone = self.kept.lock().unwrap().forget(key);
        self.evict(Vec::from_iter(gone))
    }

    /// Drops part `part` of the object at `key`, if it is kept: its tail, the object.
    pub(crate) fn forget_part(&self, key: &str, part: u64) -> io::Result<()> {
        let evicted = {
            let mut kept = self.kept.lock().unwrap();
            match kept.objects.get(key) {
                Some(object) if object.kept.contains_key(&part) => kept.evict(key, part),
                _ => return Ok(()),
            }
        };
        self.evict(Vec::from_iter(evicted))
    }

    /// How many bytes of objects are kept.
    pub fn bytes(&self) -> u64 {
        self.kept.lock().unwrap().bytes
    }

    /// Does with the files what evicting left to do ([`Kept::evict`]), holding no lock: punches
    /// parts' holes, or removes objects' files; then they are no longer busy, which may leave an
    /// object forgotten meanwhile nothing, its file removed then too.
    fn evict(&self, mut evicted: Vec<Evicted>) -> io::Result<()> {
        // Its object's file removed meanwhile, with the object's last part, which another put
        // evicted: its hole with it.
        let present = |done: io::Result<()>| match done {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            done => done,
        };
        let mut done = Ok(());
        let mut at = 0;
        while let Some(next) = evicted.get(at) {
            at += 1;
            match next {
                Evicted::Punch { key, part, place } => {
                    done = done.and(present(self.files.punch(key, place.clone())));
                    let gone = self.kept.lock().unwrap().freed(key, *part);
                    evicted.extend(gone);
                }
                Evicted::Remove(key) => {
                    done = done.and(present(self.files.remove(key)));
                    self.kept.lock().unwrap().removing.remove(key);
                }
            }
        }
        done
    }
}

/// The cache's files, under its directory, each its object's at its key.
#[derive(Debug)]
struct Files {
    dir: PathBuf,
}

impl Files {
    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    /// Makes the file of the object at `key`, of `size` bytes, if there is none.
    fn create(&self, key: &str, size: u64) -> io::Result<()> {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file =
            std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(path)?;
        // Its file is the object's size from the first, so that what is not written to it is a
        // hole: some filesystems (APFS) fill what a write past the end skips.
        if file.metadata()?.len() < size {
            file.set_len(size)?;
        }
        Ok(())
    }

    /// Writes `bytes` at `offset` in the file of the object at `key`.
    fn write(&self, key: &str, offset: u64, bytes: &[u8]) -> io::Result<()> {
        File::options().write(true).open(self.path(key))?.write_all_at(bytes, offset)
    }

    /// Frees the blocks wholly within `place` in the file of the object at `key`, which reads
    /// zeros there after.
    fn punch(&self, key: &str, place: Range<u64>) -> io::Result<()> {
        let file = File::options().write(true).open(self.path(key))?;
        punch(&file, place.start, place.end - place.start)
    }

    fn remove(&self, key: &str) -> io::Result<()> {
        std::fs::remove_file(self.path(key))
    }

    /// The bytes at `place` in the file of the object at `key`.
    fn read(&self, key: &str, place: Range<u64>) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; (place.end - place.start) as usize];
        File::open(self.path(key))?.read_exact_at(&mut bytes, place.start)?;
        Ok(bytes)
    }

    /// The bytes free to unprivileged users on the cache's filesystem.
    fn free(&self) -> io::Result<u64> {
        let path = std::ffi::CString::new(self.dir.as_os_str().as_encoded_bytes())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is a C string and `stat` a statvfs, which the call fills.
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        #[allow(clippy::unnecessary_cast, reason = "the fields' types differ by platform")]
        Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
    }
}

/// Frees the blocks wholly within `len` bytes at `offset` of `file`, which reads zeros there after.
#[cfg(target_os = "linux")]
fn punch(file: &File, offset: u64, len: u64) -> io::Result<()> {
    let mode = libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE;
    // SAFETY: an fallocate of an open file.
    let punched = unsafe { libc::fallocate(file.as_raw_fd(), mode, offset as i64, len as i64) };
    if punched != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Frees the blocks wholly within `len` bytes at `offset` of `file`, which reads zeros there after.
/// macOS punches whole blocks only, so a part's last partial block stays.
#[cfg(target_os = "macos")]
fn punch(file: &File, offset: u64, len: u64) -> io::Result<()> {
    const BLOCK: u64 = 4096;
    let len = len / BLOCK * BLOCK;
    if len == 0 {
        return Ok(());
    }
    let hole = libc::fpunchhole_t {
        fp_flags: 0,
        reserved: 0,
        fp_offset: offset as libc::off_t,
        fp_length: len as libc::off_t,
    };
    // SAFETY: an F_PUNCHHOLE of an open file, with its argument.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PUNCHHOLE, &hole) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::PART;
    use extent_frame::Frame;
    use std::sync::Arc;

    const KEY: &str = "segments/0000000000000001/0000000000000002";
    const OTHER: &str = "segments/0000000000000001/0000000000000003";

    /// `len` bytes that don't compress, from a xorshift generator seeded by `seed`: their objects'
    /// parts are stored raw.
    fn noise(len: u64, seed: u64) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ seed;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    /// An object at its key, framed: its bytes and its tail.
    #[derive(Clone)]
    struct Object {
        key: String,
        bytes: Vec<u8>,
        tail: object::Tail,
    }

    impl Object {
        /// The object of `data` at `key`.
        fn new(key: &str, data: &[u8]) -> Self {
            let bytes = extent_frame::framed(data, b"footer");
            let start = Frame::of(&bytes).unwrap().tail().start as usize;
            let tail = object::Tail::of(key, bytes[start..].to_vec()).unwrap();
            Self { key: key.to_owned(), bytes, tail }
        }

        /// The object at `key` of `len` bytes of noise seeded by `seed`.
        fn noise(key: &str, len: u64, seed: u64) -> Self {
            Self::new(key, &noise(len, seed))
        }

        fn parts(&self) -> u64 {
            self.tail.parts.frame.parts()
        }

        fn stored(&self, part: u64) -> Stored {
            self.tail.parts.stored[part as usize]
        }

        /// Part `part`'s stored bytes.
        fn part(&self, part: u64) -> &[u8] {
            let range = self.stored(part).range();
            &self.bytes[range.start as usize..range.end as usize]
        }

        fn tail_len(&self) -> u64 {
            self.tail.bytes.len() as u64
        }

        /// The bytes of its tail and all its parts.
        fn len(&self) -> u64 {
            self.tail_len() + (0..self.parts()).map(|part| self.stored(part).len).sum::<u64>()
        }

        fn keep_tail(&self, cache: &Cache) {
            cache.keep_tail(&self.key, &self.tail).unwrap();
        }

        fn keep_part(&self, cache: &Cache, part: u64) {
            cache.keep_part(&self.key, part, &self.stored(part), self.part(part)).unwrap();
        }

        /// Keeps its tail and all its parts in `cache`.
        fn keep(&self, cache: &Cache) {
            self.keep_tail(cache);
            for part in 0..self.parts() {
                self.keep_part(cache, part);
            }
        }

        /// Part `part`'s stored bytes, as `cache` keeps them.
        fn read(&self, cache: &Cache, part: u64) -> Option<Vec<u8>> {
            cache.part(&self.key, part, &self.stored(part)).unwrap()
        }

        /// Which of its parts `cache` keeps, read back as stored: none without its tail.
        fn kept(&self, cache: &Cache) -> Vec<u64> {
            if cache.tail(&self.key).unwrap().is_none() {
                return Vec::new();
            }
            let parts = (0..self.parts()).filter(|&part| self.read(cache, part).is_some());
            parts.collect()
        }
    }

    impl Kept {
        /// Checks what the index says of itself: each object it holds keeps or writes something,
        /// keeps parts only with its tail, read after them; what it counts by use it keeps; and,
        /// with none busy, the bytes it counts are those it keeps.
        fn check(&self) {
            let (mut bytes, mut kept) = (0, 0);
            for (key, object) in &self.objects {
                assert!(!object.kept.is_empty() || !object.busy.is_empty(), "{key} holds nothing");
                assert!(object.kept.is_empty() || object.readable(), "{key} kept without its tail");
                assert!(!self.removing.contains(key), "{key} kept while removing");
                let writing = object.busy.get(&TAIL) == Some(&Busy::Writing);
                assert!(object.readable() || writing || object.forgotten, "{key} stuck");
                assert!(!object.forgotten || object.kept.is_empty(), "{key} kept forgotten");
                for (&part, &used) in &object.kept {
                    assert_eq!(self.by_use.get(&used), Some(&(key.clone(), part)));
                    assert!(part == TAIL || used < object.kept[&TAIL], "{key}'s tail read first");
                    bytes += object.len(part);
                }
                kept += object.kept.len();
            }
            assert_eq!(self.by_use.len(), kept);
            if self.objects.values().all(|object| object.busy.is_empty()) {
                assert_eq!(self.bytes, bytes);
            }
        }
    }

    impl Cache {
        /// Its index, checked.
        fn check(&self) {
            self.kept.lock().unwrap().check();
        }

        /// The keys of the objects it holds.
        fn keys(&self) -> Vec<String> {
            let mut keys = Vec::from_iter(self.kept.lock().unwrap().objects.keys().cloned());
            keys.sort();
            keys
        }
    }

    /// Flips a bit of the byte at `at` in the file at `path`, as a torn write might.
    fn flip(path: &Path, at: u64) {
        let file = File::options().read(true).write(true).open(path).unwrap();
        let mut byte = [0];
        file.read_exact_at(&mut byte, at).unwrap();
        file.write_all_at(&[byte[0] ^ 1], at).unwrap();
    }

    /// The keys of the files under `dir`, a cache's.
    fn files(dir: &Path) -> Vec<String> {
        fn walk(dir: &Path, rel: &Path, keys: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir.join(rel)).unwrap() {
                let rel = rel.join(entry.unwrap().file_name());
                match dir.join(&rel).is_dir() {
                    true => walk(dir, &rel, keys),
                    false => keys.push(rel.to_str().unwrap().to_owned()),
                }
            }
        }
        let mut keys = Vec::new();
        walk(&dir.join(FILES), Path::new(""), &mut keys);
        keys.sort();
        keys
    }

    /// Parts read least recently go first, by punching holes where they were, their object's tail
    /// after its last; an object's last part takes its file with it.
    #[test]
    fn keeps_parts_evicting_the_least_recently_read() {
        let dir = tempfile::tempdir().unwrap();
        let object = Object::noise(KEY, 6 * PART, 1);
        let cache = Cache::new(dir.path(), 3 * PART + object.tail_len(), 0).unwrap();
        object.keep_tail(&cache);
        for part in 0..3 {
            object.keep_part(&cache, part);
        }
        assert_eq!(cache.bytes(), 3 * PART + object.tail_len());
        assert_eq!(object.read(&cache, 0).as_deref(), Some(object.part(0)));

        // Part 1 was read least recently.
        object.keep_part(&cache, 5);
        assert_eq!(object.read(&cache, 1), None);
        let mut hole = vec![1; PART as usize];
        File::open(cache.path(KEY)).unwrap().read_exact_at(&mut hole, PART).unwrap();
        assert_eq!(hole, vec![0; PART as usize]);
        assert_eq!(object.kept(&cache), [0, 2, 5]);

        // The tail goes last, with its object's file.
        let other = Object::noise(OTHER, 3 * PART, 2);
        other.keep(&cache);
        assert_eq!(object.kept(&cache), Vec::<u64>::new());
        assert!(!cache.path(KEY).exists());
        assert_eq!(other.kept(&cache), [0, 1, 2]);
        cache.check();
        cache.forget(OTHER).unwrap();
        assert!(!cache.path(OTHER).exists());
        assert_eq!((cache.bytes(), other.kept(&cache)), (0, vec![]));
        cache.check();
    }

    /// Evicted parts' blocks are freed, and parts never kept take none. (APFS, which development
    /// on macOS uses, allocates a sparse file's holes as it is written, and frees none it punches.)
    #[cfg(target_os = "linux")]
    #[test]
    fn evicted_parts_free_their_blocks() {
        use std::os::unix::fs::MetadataExt as _;
        // The bytes the object's file takes on its filesystem.
        let allocated =
            |cache: &Cache, key: &str| std::fs::metadata(cache.path(key)).unwrap().blocks() * 512;
        let dir = tempfile::tempdir().unwrap();
        let object = Object::noise(KEY, 64 * PART, 1);
        // Its tail takes a block, of the parts' last.
        let tail = object.tail_len().next_multiple_of(4096);
        let cache = Cache::new(dir.path(), 3 * PART + object.tail_len(), 0).unwrap();
        object.keep_tail(&cache);
        for part in 0..3 {
            object.keep_part(&cache, part);
        }
        assert_eq!(allocated(&cache, KEY), 3 * PART + tail);
        object.keep_part(&cache, 63);
        assert_eq!(allocated(&cache, KEY), 3 * PART + tail);
    }

    /// Nothing is kept that would leave less than the floor free.
    #[test]
    fn keeps_the_floor_free() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, u64::MAX / 2).unwrap();
        let object = Object::noise(KEY, PART, 1);
        object.keep(&cache);
        assert_eq!((cache.tail(KEY).unwrap(), object.read(&cache, 0)), (None, None));
        assert_eq!(cache.bytes(), 0);
    }

    /// What goes first is what was read least recently, not kept: each read counts.
    #[test]
    fn reads_order_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let object = Object::noise(KEY, 6 * PART, 1);
        let cache = Cache::new(dir.path(), 3 * PART + object.tail_len(), 0).unwrap();
        object.keep_tail(&cache);
        for part in 0..3 {
            object.keep_part(&cache, part);
        }
        object.read(&cache, 0);
        object.read(&cache, 1);
        object.keep_part(&cache, 3);
        assert_eq!(object.kept(&cache), [0, 1, 3]);
    }

    /// A part is kept only with its object's tail, and as the tail kept says it is stored; one kept
    /// is read only as stored so.
    #[test]
    fn parts_are_kept_with_their_tails() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 8 * PART, 0).unwrap();
        let object = Object::noise(KEY, 2 * PART, 1);
        object.keep_part(&cache, 0);
        assert_eq!((cache.bytes(), cache.keys()), (0, Vec::<String>::new()));
        object.keep_tail(&cache);
        assert_eq!(cache.tail(KEY).unwrap(), Some(object.tail.clone()));
        // Another object's part, at its key.
        let other = Object::noise(KEY, 2 * PART, 2);
        other.keep_part(&cache, 1);
        assert_eq!((other.read(&cache, 1), object.read(&cache, 1)), (None, None));
        object.keep_part(&cache, 0);
        assert_eq!(cache.part(KEY, 0, &object.stored(1)).unwrap(), None);
        assert_eq!(object.read(&cache, 0).as_deref(), Some(object.part(0)));
        assert_eq!(cache.bytes(), PART + object.tail_len());
        cache.check();
    }

    /// Another object's tail at a key replaces what is kept there: none of it is of use to the new
    /// one's readers.
    #[test]
    fn tails_of_objects_recreated_replace_what_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 8 * PART, 0).unwrap();
        let (old, new) = (Object::noise(KEY, 2 * PART, 1), Object::noise(KEY, PART, 2));
        old.keep(&cache);
        new.keep_tail(&cache);
        assert_eq!(cache.tail(KEY).unwrap(), Some(new.tail.clone()));
        assert_eq!((old.read(&cache, 0), new.read(&cache, 0)), (None, None));
        assert_eq!(cache.bytes(), new.tail_len());
        assert_eq!(std::fs::metadata(cache.path(KEY)).unwrap().len(), new.bytes.len() as u64);
        new.keep_part(&cache, 0);
        assert_eq!(new.kept(&cache), [0]);
        cache.check();
    }

    /// A part whose bytes changed behind the cache's back, as a torn write leaves them, fails its
    /// CRC, reads as not kept, and is dropped; a tail so takes its object with it.
    #[test]
    fn parts_failing_their_crcs_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 8 * PART, 0).unwrap();
        let object = Object::noise(KEY, 2 * PART, 1);
        object.keep(&cache);
        flip(&cache.path(KEY), object.stored(1).offset + 7);
        assert_eq!(object.read(&cache, 1), None);
        assert_eq!(cache.bytes(), PART + object.tail_len());
        assert_eq!(object.kept(&cache), [0]);
        flip(&cache.path(KEY), object.tail.parts.frame.tail().start + 1);
        assert_eq!(cache.tail(KEY).unwrap(), None);
        assert_eq!((cache.bytes(), object.read(&cache, 0)), (0, None));
        assert!(!cache.path(KEY).exists());
        cache.check();
    }

    /// An object whose file went, or shrank, behind the cache's back reads as not kept, and is
    /// dropped; one whose file can't be read fails.
    #[test]
    fn parts_gone_read_as_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        let object = Object::noise(KEY, PART + 100, 1);
        object.keep(&cache);
        File::options().write(true).open(cache.path(KEY)).unwrap().set_len(PART).unwrap();
        assert_eq!((object.read(&cache, 1), cache.bytes()), (None, 0));
        object.keep(&cache);
        std::fs::remove_file(cache.path(KEY)).unwrap();
        assert_eq!((object.read(&cache, 1), cache.bytes()), (None, 0));
        object.keep(&cache);
        std::fs::remove_file(cache.path(KEY)).unwrap();
        std::fs::create_dir(cache.path(KEY)).unwrap();
        let err = cache.part(KEY, 1, &object.stored(1)).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EISDIR), "{err}");
    }

    /// A part larger than the cache isn't kept, and evicts nothing; one that fits beside its tail
    /// is, evicting what was read before; one kept already is kept once.
    #[test]
    fn keeps_parts_that_fit_once() {
        let dir = tempfile::tempdir().unwrap();
        let (object, other) = (Object::noise(KEY, 1000, 1), Object::noise(OTHER, PART + 100, 2));
        let capacity = 1000 + object.tail_len() + other.tail_len();
        let cache = Cache::new(dir.path(), capacity, 0).unwrap();
        other.keep_tail(&cache);
        other.keep_part(&cache, 1);
        other.keep_part(&cache, 1);
        assert_eq!(cache.bytes(), 100 + other.tail_len());
        other.keep_part(&cache, 0);
        assert_eq!(other.kept(&cache), [1]);
        object.keep(&cache);
        assert_eq!(object.kept(&cache), [0]);
        assert_eq!(cache.tail(OTHER).unwrap(), Some(other.tail.clone()));
        assert_eq!(other.read(&cache, 1), None);
        assert_eq!(cache.bytes(), capacity);
        cache.check();
    }

    /// An object's file is its size from its tail, the rest holes.
    #[test]
    fn files_are_their_objects_size() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        let object = Object::noise(KEY, 5 * PART, 1);
        object.keep_tail(&cache);
        assert_eq!(std::fs::metadata(cache.path(KEY)).unwrap().len(), object.bytes.len() as u64);
    }

    /// The free bytes are the free blocks' size, as statvfs counts them.
    #[test]
    fn free_bytes_are_the_free_blocks_size() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 0, 0).unwrap();
        let path = std::ffi::CString::new(cache.files.dir.as_os_str().as_encoded_bytes()).unwrap();
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is a C string and `stat` a statvfs, which the call fills.
        assert_eq!(unsafe { libc::statvfs(path.as_ptr(), &mut stat) }, 0);
        #[allow(clippy::unnecessary_cast, reason = "the fields' types differ by platform")]
        let (blocks, size) = (stat.f_bavail as u64, stat.f_frsize as u64);
        // Within a few blocks of it: other processes write meanwhile.
        let free = cache.files.free().unwrap();
        assert!(free.abs_diff(blocks * size) < 1024 * size, "{free} bytes, {blocks} of {size}");
    }

    /// A tail whose write fails is neither kept nor counted; the cache keeps others after it.
    #[test]
    fn tails_whose_writes_fail_are_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        let (object, other) = (Object::noise(KEY, PART, 1), Object::noise(OTHER, PART, 2));
        std::fs::create_dir_all(cache.path(OTHER)).unwrap();
        let failed = cache.keep_tail(OTHER, &other.tail).unwrap_err();
        assert_eq!(failed.raw_os_error(), Some(libc::EISDIR), "{failed}");
        assert_eq!((cache.tail(OTHER).unwrap(), cache.bytes()), (None, 0));
        object.keep(&cache);
        assert_eq!((object.kept(&cache), cache.bytes()), (vec![0], object.len()));
        cache.check();
    }

    /// TASK-92.21: a part dropped whose object's file went meanwhile, as one does that another
    /// put's eviction of the object's last part removes outside the cache's lock, is dropped, its
    /// hole gone with the file; and so is the object's tail.
    #[test]
    fn parts_dropped_whose_files_went_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        let object = Object::noise(KEY, PART + 100, 1);
        object.keep(&cache);
        std::fs::remove_file(cache.path(KEY)).unwrap();
        cache.forget_part(KEY, 0).unwrap();
        assert_eq!(cache.bytes(), 100 + object.tail_len());
        cache.forget_part(KEY, TAIL).unwrap();
        assert_eq!((cache.bytes(), cache.keys()), (0, Vec::<String>::new()));
        cache.check();
    }

    /// TASK-92.21: keeps, reads and drops of a few objects' tails and parts, in threads at once,
    /// evicting one another's, keep the cache whole: none fails, and, once they are done, what the
    /// cache counts it keeps, each part kept read whole.
    #[test]
    fn parts_kept_read_and_dropped_at_once_keep_the_cache_whole() {
        const OBJECTS: u64 = 3;
        const PARTS: u64 = 4;
        let dir = tempfile::tempdir().unwrap();
        let key = |object: u64| format!("segments/0000000000000001/{object:016x}");
        let objects: Arc<Vec<_>> =
            Arc::new((0..OBJECTS).map(|at| Object::noise(&key(at), PARTS * PART, at)).collect());
        let cache = Arc::new(Cache::new(dir.path(), 6 * PART, 0).unwrap());
        let threads: Vec<_> = (0..4u64)
            .map(|thread| {
                let (cache, objects) = (cache.clone(), objects.clone());
                std::thread::spawn(move || {
                    let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ thread;
                    for _ in 0..500 {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        let object = &objects[(state % OBJECTS) as usize];
                        let part = state / OBJECTS % PARTS;
                        match state / (OBJECTS * PARTS) % 16 {
                            0..=5 => object.keep_part(&cache, part),
                            6..=7 => object.keep_tail(&cache),
                            8..=11 => drop(object.read(&cache, part)),
                            12 => drop(cache.tail(&object.key).unwrap()),
                            13..=14 => cache.forget_part(&object.key, part).unwrap(),
                            _ => cache.forget(&object.key).unwrap(),
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let read = |cache: &Cache| {
            cache.check();
            let mut kept = 0;
            for object in objects.iter() {
                let Some(tail) = cache.tail(&object.key).unwrap() else { continue };
                assert_eq!(tail, object.tail);
                kept += object.tail_len();
                for part in 0..PARTS {
                    if let Some(bytes) = object.read(cache, part) {
                        assert_eq!(bytes, object.part(part), "part {part} of {}", object.key);
                        kept += PART;
                    }
                }
            }
            assert_eq!(cache.bytes(), kept);
            assert_eq!(cache.keys(), files(dir.path()));
            kept
        };
        read(&cache);
        assert!(cache.kept.lock().unwrap().removing.is_empty());
    }

    /// TASK-92.21: a benchmark of reads of a part kept while others are kept, each evicting one,
    /// in another thread: how long the reads took, the most and in all, which wait for the
    /// cache's lock. `cargo nextest run -p extent_remote --run-ignored only
    /// bench_reads_while_parts_are_kept --no-capture`.
    #[test]
    #[ignore]
    fn bench_reads_while_parts_are_kept() {
        const PUTS: u64 = 2048;
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path(), 16 * PART, 0).unwrap());
        let read = Object::noise(KEY, PART, 1);
        read.keep(&cache);
        let objects: Vec<_> = (0..PUTS / 8)
            .map(|at| Object::noise(&format!("segments/0000000000000002/{at:016x}"), 8 * PART, at))
            .collect();
        let keeping = {
            let cache = cache.clone();
            std::thread::spawn(move || {
                for number in 0..PUTS {
                    let object = &objects[(number / 8) as usize];
                    if number % 8 == 0 {
                        object.keep_tail(&cache);
                    }
                    object.keep_part(&cache, number % 8);
                }
            })
        };
        let (mut most, mut all, mut reads) =
            (std::time::Duration::ZERO, std::time::Duration::ZERO, 0);
        while !keeping.is_finished() {
            let started = std::time::Instant::now();
            assert_eq!(read.read(&cache, 0).as_deref(), Some(read.part(0)));
            let took = started.elapsed();
            (most, all, reads) = (most.max(took), all + took, reads + 1);
        }
        keeping.join().unwrap();
        println!("{reads} reads while {PUTS} parts were kept: at most {most:?}, {all:?} in all");
    }

    /// A cache starts empty, whatever an earlier one left.
    #[test]
    fn starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let object = Object::noise(KEY, PART, 1);
        object.keep(&Cache::new(dir.path(), 3 * PART, 0).unwrap());
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        assert_eq!((cache.tail(KEY).unwrap(), object.read(&cache, 0)), (None, None));
        assert!(!cache.path(KEY).exists());
    }
}
