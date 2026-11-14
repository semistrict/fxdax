// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A host's copies of objects of its bucket (doc-7, Reading: the NVMe cache; doc-14): of the
//! parts it fetched, and of whole objects it wrote, adopted once the bucket has them
//! ([`Cache::adopt`]). Each object's is a sparse file of its size, at its key, holding its tail,
//! which says where its parts are stored, and those of its parts kept, as stored, compressed or
//! not, at their offsets, the rest holes. Those read least recently are evicted, by punching holes,
//! an object's tail with its last part, so that they take no more than a capacity and the
//! filesystem keeps a floor of free space. They are copies of what is in the bucket, so none is
//! data the bucket lacks.
//!
//! The files are all the cache keeps: one opened finds what an earlier one kept in them
//! ([`Cache::open`]), so that a host's restart refetches nothing it had. Its index is rebuilt from
//! their tails, each checked against its CRCs, a file whose tail fails going whole; an object's
//! parts kept are those its file holds data for, not holes. And what it gives, it checks: a tail
//! against its CRCs, a part against the one its tail gives, dropping what fails. So whatever a
//! crash left of a write, torn or lost, reads as not kept, never as other bytes, and is fetched
//! again by its reader. One cache at a time has a directory.

use crate::object;
use extent_frame::{Frame, Parts, Stored, TRAILER};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io;
use std::ops::Range;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// The number an object's tail is kept by, among its parts, which count from 0: its footer, table
/// and trailer, after its parts, which a fetcher of another of the host's stores reads here rather
/// than from the bucket (TASK-92.22), and without which none of its parts can be read.
const TAIL: u64 = u64::MAX;

/// The cache's lock is held only to change what it keeps, not across its files' I/O: a part's write,
/// or its eviction, is done outside it, the part busy meanwhile, and an object's file removed, or
/// replaced by one adopted, so too, the object's key removing; neither is kept or read until done
/// (TASK-92.21).
#[derive(Debug)]
pub struct Cache {
    files: Files,
    capacity: u64,
    floor: u64,
    kept: Mutex<Kept>,
    /// The directory, locked, so that no other cache opens it while this one has it.
    _lock: File,
}

#[derive(Debug, Default)]
struct Kept {
    objects: HashMap<String, Object>,
    /// The parts kept, by when they were last read.
    by_use: BTreeMap<u64, (String, u64)>,
    /// The objects whose files are being removed, or replaced by one adopted.
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

    /// Keeps the object at `key` that `found` is, its parts read now, its tail after them.
    fn insert(&mut self, key: &str, found: Found) {
        let object = Object {
            parts: found.parts,
            kept: HashMap::new(),
            busy: HashMap::new(),
            forgotten: false,
        };
        self.objects.insert(key.to_owned(), object);
        for part in found.kept.into_iter().chain([TAIL]) {
            self.keep(key, part);
        }
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

/// Where the cache keeps its files under the directory it is given.
const FILES: &str = "fxdax-parts";

/// The blocks of the filesystems the cache's files are on, as the parts in them are aligned to:
/// what punching a hole frees whole.
const BLOCK: u64 = extent_frame::ALIGN;

impl Cache {
    /// The cache in `dir`, keeping at most `capacity` bytes of objects, and `floor` bytes free
    /// there: what an earlier one kept, read from its files, those that fail their checks removed,
    /// then evicted, those read least recently first, as far as `capacity` and `floor` need. Fails
    /// while another cache has the directory.
    pub fn open(dir: &Path, capacity: u64, floor: u64) -> io::Result<Self> {
        let dir = dir.join(FILES);
        std::fs::create_dir_all(&dir)?;
        let lock = lock(&dir)?;
        let files = Files::new(dir)?;
        let mut found = Vec::new();
        files.walk(Path::new(""), &mut found)?;
        // By when each was written last: it was read then, if not since.
        found.sort_by_key(|(_, found)| found.modified);
        let mut kept = Kept::default();
        for (key, found) in found {
            kept.bytes += found.bytes();
            kept.insert(&key, found);
        }
        let cache = Self { files, capacity, floor, kept: Mutex::new(kept), _lock: lock };
        let free = cache.files.free()?;
        let (_, evicted) = cache.kept.lock().unwrap().make_room((capacity, floor), free, 0, 0);
        cache.evict(evicted)?;
        Ok(cache)
    }

    #[cfg(test)]
    pub(crate) fn path(&self, key: &str) -> PathBuf {
        self.files.path(key)
    }

    /// The directory the cache keeps its objects in.
    pub fn dir(&self) -> &Path {
        &self.files.dir
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

    /// Whether part `part` of the object at `key` is kept, stored as `stored` says, as the cache's
    /// index has it: reading nothing of its file, nor counting it read, as [`Cache::part`] does.
    pub fn holds(&self, key: &str, part: u64, stored: &Stored) -> bool {
        let kept = self.kept.lock().unwrap();
        kept.objects.get(key).is_some_and(|object| {
            object.kept.contains_key(&part)
                && object.parts.stored.get(part as usize) == Some(stored)
        })
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
    /// or busy already. One kept torn, as a crash may leave a part, a reader finds failing its
    /// CRC first ([`Cache::part`]), which drops it, and then keeps it anew.
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

    /// Takes the object framed whole in the file at `from`, on the cache's filesystem, as the one at
    /// `key`, by renaming it, evicting what was read least recently to make room: then it is kept as
    /// any is, its parts evicted as fetched ones are. Returns whether it took it: not if no room can
    /// be made, or the object it keeps at `key` is busy, and then the file is left where it is. One
    /// it keeps at `key` already, it replaces.
    pub fn adopt(&self, key: &str, from: &Path) -> io::Result<bool> {
        let found = found(from, key, self.files.holes)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{}: no object", from.display()))
        })?;
        let len = found.bytes();
        if len > self.capacity {
            return Ok(false);
        }
        let free = self.files.free()?;
        let (fits, evicted) = {
            let mut kept = self.kept.lock().unwrap();
            match kept.objects.get(key) {
                _ if kept.removing.contains(key) => return Ok(false),
                Some(object) if !object.busy.is_empty() => return Ok(false),
                // Its file goes with the rename, which replaces it.
                Some(_) => {
                    kept.forget(key);
                }
                None => {
                    kept.removing.insert(key.to_owned());
                }
            }
            // Its file's blocks are allocated already.
            let (fits, mut evicted) = kept.make_room((self.capacity, self.floor), free, len, 0);
            if fits {
                kept.bytes += len;
            } else {
                evicted.push(Evicted::Remove(key.to_owned()));
            }
            (fits, evicted)
        };
        let evicting = self.evict(evicted);
        if !fits {
            return evicting.map(|()| false);
        }
        let renamed = evicting.and_then(|()| self.files.rename(from, key));
        let mut kept = self.kept.lock().unwrap();
        kept.removing.remove(key);
        match renamed {
            Ok(()) => {
                kept.insert(key, found);
                Ok(true)
            }
            Err(err) => {
                kept.bytes -= len;
                Err(err)
            }
        }
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

    /// The most bytes of objects it keeps.
    pub fn capacity(&self) -> u64 {
        self.capacity
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

/// Locks the directory `dir`, as a cache that has it does: fails if another has.
fn lock(dir: &Path) -> io::Result<File> {
    let file = File::open(dir)?;
    // SAFETY: an flock of an open file.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            let held = format!("{}: another cache has it", dir.display());
            return Err(io::Error::new(io::ErrorKind::WouldBlock, held));
        }
        return Err(err);
    }
    Ok(file)
}

/// An object a file of the cache holds: where its parts are stored, as its tail, checked against
/// its CRCs, says; those the file holds data for, not holes, those shorter than a block, and all
/// on a filesystem that doesn't find its holes, checked against their CRCs too; and when it was
/// written last.
#[derive(Debug)]
struct Found {
    parts: Parts,
    kept: Vec<u64>,
    modified: SystemTime,
}

impl Found {
    /// The bytes of its tail and of the parts it holds.
    fn bytes(&self) -> u64 {
        let tail = self.parts.frame.tail();
        let parts = self.kept.iter().map(|&part| self.parts.stored[part as usize].len);
        tail.end - tail.start + parts.sum::<u64>()
    }
}

/// The object at `key` the file at `path` holds, if its tail, at its end, passes its checks: its
/// parts checked against their CRCs unless `holes`, the filesystem finding holes punched.
fn found(path: &Path, key: &str, holes: bool) -> io::Result<Option<Found>> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    let size = metadata.len();
    let Some(at) = size.checked_sub(TRAILER) else { return Ok(None) };
    let mut trailer = vec![0; TRAILER as usize];
    file.read_exact_at(&mut trailer, at)?;
    let Ok(frame) = Frame::of_trailer(&trailer) else { return Ok(None) };
    if frame.size() != size {
        return Ok(None);
    }
    let tail = frame.tail();
    let mut bytes = vec![0; (tail.end - tail.start) as usize];
    file.read_exact_at(&mut bytes, tail.start)?;
    let Ok(tail) = object::Tail::of(key, bytes) else { return Ok(None) };
    let data = data(&file, size)?;
    let mut kept = Vec::new();
    for (part, stored) in (0..).zip(&tail.parts.stored) {
        if !holds(&data, stored.range()) {
            continue;
        }
        // A punch frees none of a part shorter than a block, whose hole none finds: what dropped
        // it is told by its CRC.
        if stored.len < BLOCK || !holes {
            let mut bytes = vec![0; stored.len as usize];
            file.read_exact_at(&mut bytes, stored.offset)?;
            if stored.check(&bytes).is_err() {
                continue;
            }
        }
        kept.push(part);
    }
    Ok(Some(Found { parts: tail.parts, kept, modified: metadata.modified()? }))
}

/// The ranges of `file`, of `size` bytes, that hold data, not holes, as `SEEK_DATA` and
/// `SEEK_HOLE` find them: all of it, on a filesystem that finds no holes.
fn data(file: &File, size: u64) -> io::Result<Vec<Range<u64>>> {
    let seek = |at: u64, whence| {
        // SAFETY: an lseek of an open file.
        match unsafe { libc::lseek(file.as_raw_fd(), at as libc::off_t, whence) } {
            -1 => Err(io::Error::last_os_error()),
            at => Ok(at as u64),
        }
    };
    let (mut ranges, mut at) = (Vec::new(), 0);
    while at < size {
        let start = match seek(at, libc::SEEK_DATA) {
            Ok(start) => start,
            // None after it.
            Err(err) if err.raw_os_error() == Some(libc::ENXIO) => break,
            Err(err) if err.raw_os_error() == Some(libc::EINVAL) => {
                let whole = 0..size;
                return Ok(vec![whole]);
            }
            Err(err) => return Err(err),
        };
        let end = seek(start, libc::SEEK_HOLE)?.min(size);
        if end <= start {
            break;
        }
        ranges.push(start..end);
        at = end;
    }
    Ok(ranges)
}

/// Whether the filesystem `dir` is on finds the holes of a file it punched a hole in, as
/// `SEEK_DATA` finds them: a file of four blocks, the second written then punched, holding none.
/// APFS doesn't: those a resize left read as data once a hole is punched.
fn finds_holes(dir: &Path) -> io::Result<bool> {
    let path = dir.join(".holes");
    let mut options = std::fs::OpenOptions::new();
    let file = options.create(true).truncate(true).read(true).write(true).open(&path)?;
    let found = (|| {
        file.set_len(4 * BLOCK)?;
        file.write_all_at(&[1; BLOCK as usize], BLOCK)?;
        punch(&file, BLOCK, BLOCK)?;
        Ok(data(&file, 4 * BLOCK)?.is_empty())
    })();
    std::fs::remove_file(&path)?;
    found
}

/// Whether `range` is within one of `data`, ranges in order.
fn holds(data: &[Range<u64>], range: Range<u64>) -> bool {
    let after = data.partition_point(|data| data.start <= range.start);
    after > 0 && data[after - 1].end >= range.end
}

/// What changes the cache makes to its files: each a step of [`Files`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    /// An object's file made, as long as it ever is not yet.
    Create,
    /// Made as long as its object.
    Resize,
    /// Bytes written to it.
    Write,
    Punch,
    Remove,
    /// A file adopted.
    Rename,
}

/// The cache's files, under its directory, each its object's at its key: every change the cache
/// makes to them is one of these, which the crash tests cut, as a crash would, at each.
#[derive(Debug)]
struct Files {
    dir: PathBuf,
    /// Whether its filesystem finds the holes punched in its files ([`finds_holes`]).
    holes: bool,
    #[cfg(test)]
    cut: Option<std::sync::Arc<tests::Cut>>,
}

impl Files {
    fn new(dir: PathBuf) -> io::Result<Self> {
        Ok(Self {
            holes: finds_holes(&dir)?,
            dir,
            #[cfg(test)]
            cut: None,
        })
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    /// Finds the objects in the files under `rel`, removing files of none, and directories left
    /// empty. Returns whether there is any.
    fn walk(&self, rel: &Path, found: &mut Vec<(String, Found)>) -> io::Result<bool> {
        let mut any = false;
        for entry in std::fs::read_dir(self.dir.join(rel))? {
            let entry = entry?;
            let (rel, kind) = (rel.join(entry.file_name()), entry.file_type()?);
            let path = self.dir.join(&rel);
            if kind.is_dir() {
                if self.walk(&rel, found)? {
                    any = true;
                } else if let Err(err) = std::fs::remove_dir(&path) {
                    log::warn!("fxdax: removing {} from the cache: {err}", path.display());
                }
                continue;
            }
            let object = match rel.to_str() {
                Some(key) if kind.is_file() => match self::found(&path, key, self.holes) {
                    Ok(object) => object.map(|object| (key.to_owned(), object)),
                    Err(err) => {
                        log::warn!("fxdax: reading {} in the cache: {err}", path.display());
                        None
                    }
                },
                _ => None,
            };
            match object {
                Some(object) => {
                    found.push(object);
                    any = true;
                }
                None => {
                    log::warn!("fxdax: removing {} from the cache: no object", path.display());
                    std::fs::remove_file(&path)?;
                }
            }
        }
        Ok(any)
    }

    /// Makes the file of the object at `key`, of `size` bytes, if there is none.
    fn create(&self, key: &str, size: u64) -> io::Result<()> {
        if !self.done(Change::Create) {
            return Ok(());
        }
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file =
            std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(path)?;
        // Its file is the object's size from the first, so that what is not written to it is a
        // hole: some filesystems (APFS) fill what a write past the end skips.
        if file.metadata()?.len() < size && self.done(Change::Resize) {
            file.set_len(size)?;
        }
        Ok(())
    }

    /// Writes `bytes` at `offset` in the file of the object at `key`.
    fn write(&self, key: &str, offset: u64, bytes: &[u8]) -> io::Result<()> {
        let file = File::options().write(true).open(self.path(key))?;
        for piece in self.pieces(Change::Write, offset..offset + bytes.len() as u64) {
            let from = (piece.start - offset) as usize..(piece.end - offset) as usize;
            file.write_all_at(&bytes[from], piece.start)?;
        }
        Ok(())
    }

    /// Frees the blocks wholly within `place` in the file of the object at `key`, which reads
    /// zeros there after.
    fn punch(&self, key: &str, place: Range<u64>) -> io::Result<()> {
        let file = File::options().write(true).open(self.path(key))?;
        for piece in self.pieces(Change::Punch, place) {
            punch(&file, piece.start, piece.end - piece.start)?;
        }
        Ok(())
    }

    fn remove(&self, key: &str) -> io::Result<()> {
        match self.done(Change::Remove) {
            true => std::fs::remove_file(self.path(key)),
            false => Ok(()),
        }
    }

    /// Moves the file at `from` to be that of the object at `key`, replacing any there.
    fn rename(&self, from: &Path, key: &str) -> io::Result<()> {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match self.done(Change::Rename) {
            true => std::fs::rename(from, path),
            false => Ok(()),
        }
    }

    /// The bytes at `place` in the file of the object at `key`.
    fn read(&self, key: &str, place: Range<u64>) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; (place.end - place.start) as usize];
        File::open(self.path(key))?.read_exact_at(&mut bytes, place.start)?;
        Ok(bytes)
    }

    /// Whether `change`, which isn't a write, is made.
    #[cfg(not(test))]
    fn done(&self, _: Change) -> bool {
        true
    }

    /// The pieces of `range` that `change`, a write or a punch, changes.
    #[cfg(not(test))]
    fn pieces(&self, _: Change, range: Range<u64>) -> Vec<Range<u64>> {
        vec![range]
    }

    #[cfg(test)]
    fn done(&self, change: Change) -> bool {
        self.cut.as_ref().is_none_or(|cut| cut.done(change))
    }

    #[cfg(test)]
    fn pieces(&self, change: Change, range: Range<u64>) -> Vec<Range<u64>> {
        match &self.cut {
            Some(cut) => cut.pieces(change, range),
            None => vec![range],
        }
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

        /// Keeps in `cache` what it finds there not: as a reader does, which fetches and keeps
        /// what it reads not.
        fn fetch(&self, cache: &Cache) {
            if cache.tail(&self.key).unwrap().is_none() {
                self.keep_tail(cache);
            }
            for part in 0..self.parts() {
                if self.read(cache, part).is_none() {
                    self.keep_part(cache, part);
                }
            }
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

        /// Writes it whole to a file in `dir`, as a store's WAL would: its path.
        fn write_to(&self, dir: &Path) -> PathBuf {
            let path = dir.join(self.key.replace('/', "-"));
            std::fs::write(&path, &self.bytes).unwrap();
            path
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
        let cache = Cache::open(dir.path(), 3 * PART + object.tail_len(), 0).unwrap();
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
        let cache = Cache::open(dir.path(), 3 * PART + object.tail_len(), 0).unwrap();
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
        let cache = Cache::open(dir.path(), 3 * PART, u64::MAX / 2).unwrap();
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
        let cache = Cache::open(dir.path(), 3 * PART + object.tail_len(), 0).unwrap();
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
        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
        let object = Object::noise(KEY, 2 * PART, 1);
        object.keep_part(&cache, 0);
        assert_eq!((cache.bytes(), cache.keys()), (0, Vec::<String>::new()));
        object.keep_tail(&cache);
        assert_eq!(cache.tail(KEY).unwrap(), Some(object.tail.clone()));
        // Another object's part, at its key.
        let other = Object::noise(KEY, 2 * PART, 2);
        other.keep_part(&cache, 1);
        assert_eq!((other.read(&cache, 1), object.read(&cache, 1)), (None, None));
        assert!(!cache.holds(KEY, 0, &object.stored(0)));
        object.keep_part(&cache, 0);
        assert_eq!(cache.part(KEY, 0, &object.stored(1)).unwrap(), None);
        assert_eq!(object.read(&cache, 0).as_deref(), Some(object.part(0)));
        assert_eq!(cache.bytes(), PART + object.tail_len());
        // What it holds, as stored so: not part 1, nor part 0 as another object stores it.
        assert!(cache.holds(KEY, 0, &object.stored(0)));
        assert!(!cache.holds(KEY, 0, &object.stored(1)));
        assert!(!cache.holds(KEY, 1, &object.stored(1)));
        assert!(!cache.holds("segments/0000000000000002/0000000000000000", 0, &object.stored(0)));
        cache.check();
    }

    /// Another object's tail at a key replaces what is kept there: none of it is of use to the new
    /// one's readers.
    #[test]
    fn tails_of_objects_recreated_replace_what_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
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
        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
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
        let cache = Cache::open(dir.path(), 3 * PART, 0).unwrap();
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
        let cache = Cache::open(dir.path(), capacity, 0).unwrap();
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
        let cache = Cache::open(dir.path(), 3 * PART, 0).unwrap();
        let object = Object::noise(KEY, 5 * PART, 1);
        object.keep_tail(&cache);
        assert_eq!(std::fs::metadata(cache.path(KEY)).unwrap().len(), object.bytes.len() as u64);
    }

    /// The free bytes are the free blocks' size, as statvfs counts them.
    #[test]
    fn free_bytes_are_the_free_blocks_size() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path(), 0, 0).unwrap();
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
        let cache = Cache::open(dir.path(), 3 * PART, 0).unwrap();
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
        let cache = Cache::open(dir.path(), 3 * PART, 0).unwrap();
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
    /// cache counts it keeps, each part kept read whole; and a cache opened after it finds the same.
    #[test]
    fn parts_kept_read_and_dropped_at_once_keep_the_cache_whole() {
        const OBJECTS: u64 = 3;
        const PARTS: u64 = 4;
        let dir = tempfile::tempdir().unwrap();
        let key = |object: u64| format!("segments/0000000000000001/{object:016x}");
        let objects: Arc<Vec<_>> =
            Arc::new((0..OBJECTS).map(|at| Object::noise(&key(at), PARTS * PART, at)).collect());
        let cache = Arc::new(Cache::open(dir.path(), 6 * PART, 0).unwrap());
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
        let kept = read(&cache);
        assert!(cache.kept.lock().unwrap().removing.is_empty());
        drop(cache);
        assert_eq!(read(&Cache::open(dir.path(), 6 * PART, 0).unwrap()), kept);
    }

    /// TASK-92.21: a benchmark of reads of a part kept while others are kept, each evicting one,
    /// in another thread: how long the reads took, the most and in all, which wait for the
    /// cache's lock. `cargo nextest run -p extent_remote --run-ignored only
    /// bench_reads_while_parts_are_kept --no-capture`.
    #[test]
    #[ignore = "a benchmark (TASK-92.21): --run-ignored only, alone"]
    fn bench_reads_while_parts_are_kept() {
        const PUTS: u64 = 2048;
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::open(dir.path(), 16 * PART, 0).unwrap());
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

    /// TASK-165.4: a cache keeps what an earlier one in its directory kept: its tails, and the
    /// parts it held, counted as they were, but not those it evicted, whose holes it finds.
    #[test]
    fn keeps_what_an_earlier_one_kept() {
        let dir = tempfile::tempdir().unwrap();
        let object = Object::noise(KEY, 4 * PART + 100, 1);
        let other = Object::noise(OTHER, 3000, 2);
        let capacity = 3 * PART + 100 + object.tail_len() + other.len();
        let cache = Cache::open(dir.path(), capacity, 0).unwrap();
        object.keep_tail(&cache);
        for part in [0, 1, 2, 4] {
            object.keep_part(&cache, part);
        }
        object.read(&cache, 0);
        other.keep(&cache);
        // Part 1 goes for it.
        object.keep_part(&cache, 3);
        assert_eq!(object.kept(&cache), [0, 2, 3, 4]);
        let bytes = cache.bytes();
        drop(cache);

        let cache = Cache::open(dir.path(), capacity, 0).unwrap();
        cache.check();
        assert_eq!(cache.bytes(), bytes);
        assert_eq!(object.kept(&cache), [0, 2, 3, 4]);
        assert_eq!(other.kept(&cache), [0]);
        assert_eq!(cache.tail(KEY).unwrap(), Some(object.tail.clone()));
        for part in [0, 2, 3, 4] {
            assert_eq!(object.read(&cache, part).as_deref(), Some(object.part(part)));
        }
        assert_eq!(cache.bytes(), bytes);
    }

    /// TASK-165.4: a cache opened drops the files an earlier one left that hold no object whose
    /// tail passes its checks, as a crash leaves one whose tail was being written, and anything
    /// else in its directory but objects' files; and directories left empty.
    #[test]
    fn earlier_files_failing_their_checks_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let objects: Vec<_> = (0..4)
            .map(|at| Object::noise(&format!("segments/0000000000000001/{at:016x}"), 3000, at))
            .collect();
        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
        for object in &objects {
            object.keep(&cache);
        }
        let path = |at: usize| cache.path(&objects[at].key);
        // A byte of its table flipped.
        flip(&path(0), objects[0].tail.parts.frame.table().start);
        // Cut short, longer, a stray file, a link, an empty directory.
        File::options().write(true).open(path(1)).unwrap().set_len(1000).unwrap();
        let longer = objects[2].bytes.len() as u64 + 8;
        File::options().write(true).open(path(2)).unwrap().set_len(longer).unwrap();
        let stray = cache.path("segments/stray");
        std::fs::write(&stray, b"stray").unwrap();
        std::os::unix::fs::symlink(path(3), cache.path("segments/link")).unwrap();
        std::fs::create_dir_all(cache.path("checkpoints/0000000000000001")).unwrap();
        drop(cache);

        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
        cache.check();
        assert_eq!(cache.keys(), [objects[3].key.clone()]);
        assert_eq!(files(dir.path()), [objects[3].key.clone()]);
        assert_eq!(cache.bytes(), objects[3].len());
        assert!(!cache.path("checkpoints").exists());
    }

    /// TASK-165.4: a part an earlier cache kept whose bytes changed since, as a write torn by a
    /// crash leaves them, is counted until read: then it fails its CRC, and is dropped. On APFS,
    /// which finds no holes, the cache checks it as it opens.
    #[test]
    fn earlier_parts_failing_their_crcs_are_dropped_as_read() {
        let dir = tempfile::tempdir().unwrap();
        let object = Object::noise(KEY, 2 * PART, 1);
        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
        object.keep(&cache);
        drop(cache);
        flip(&dir.path().join(FILES).join(KEY), object.stored(1).offset + 100);

        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
        let counted = object.len() - if cfg!(target_os = "linux") { 0 } else { PART };
        assert_eq!(cache.bytes(), counted);
        assert_eq!(object.read(&cache, 1), None);
        assert_eq!(cache.bytes(), object.len() - PART);
        assert_eq!(object.kept(&cache), [0]);
    }

    /// TASK-165.4: a cache opened with a smaller capacity than an earlier one evicts what that one
    /// kept, by when each object was written last, until what is left fits; with a floor past
    /// what the filesystem has free, it evicts all.
    #[test]
    fn capacity_and_floor_hold_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let (object, other) = (Object::noise(KEY, 2 * PART, 1), Object::noise(OTHER, PART, 2));
        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
        object.keep(&cache);
        other.keep(&cache);
        let (path, written) = (cache.path(OTHER), SystemTime::now());
        drop(cache);
        // The other written after it, as its file says.
        let file = File::options().write(true).open(&path).unwrap();
        file.set_modified(written + std::time::Duration::from_secs(10)).unwrap();

        let capacity = PART + object.tail_len() + other.len();
        let cache = Cache::open(dir.path(), capacity, 0).unwrap();
        cache.check();
        assert!(cache.bytes() <= capacity);
        assert_eq!((object.kept(&cache), other.kept(&cache)), (vec![1], vec![0]));
        drop(cache);

        let cache = Cache::open(dir.path(), capacity, u64::MAX / 2).unwrap();
        assert_eq!((cache.bytes(), cache.keys(), files(dir.path())), (0, vec![], vec![]));
    }

    /// TASK-165.4: the filesystems the cache runs on find the holes punched in a file, but APFS,
    /// whose files a cache then checks each part of as it opens: macOS is for development.
    #[test]
    fn filesystems_find_holes_but_apfs() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(finds_holes(dir.path()).unwrap(), cfg!(target_os = "linux"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// TASK-165.4: one cache at a time has a directory; another may once it is dropped.
    #[test]
    fn one_cache_has_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path(), PART, 0).unwrap();
        let err = Cache::open(dir.path(), PART, 0).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock, "{err}");
        drop(cache);
        Cache::open(dir.path(), PART, 0).unwrap();
    }

    /// TASK-165.4: an object adopted from a file on the cache's filesystem, as a WAL segment
    /// acknowledged is, is moved in whole, and kept as fetched parts are: read back, evicted as
    /// they are, and kept by the cache opened next.
    #[test]
    fn adopts_whole_objects() {
        let dir = tempfile::tempdir().unwrap();
        let object = Object::noise(KEY, 2 * PART + 100, 1);
        let other = Object::noise(OTHER, PART, 2);
        let cache = Cache::open(dir.path(), object.len() + other.len() - PART, 0).unwrap();
        let from = object.write_to(dir.path());
        assert!(cache.adopt(KEY, &from).unwrap());
        assert!(!from.exists());
        assert_eq!(cache.bytes(), object.len());
        assert_eq!(object.kept(&cache), [0, 1, 2]);
        object.read(&cache, 1);
        object.read(&cache, 2);
        // Part 0 was read least recently.
        other.keep(&cache);
        assert_eq!((object.kept(&cache), other.kept(&cache)), (vec![1, 2], vec![0]));
        cache.check();
        drop(cache);
        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
        assert_eq!((object.kept(&cache), other.kept(&cache)), (vec![1, 2], vec![0]));
    }

    /// TASK-165.4: an object adopted replaces what is kept at its key, and is counted once.
    #[test]
    fn adoptions_replace_what_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
        let object = Object::noise(KEY, 2 * PART, 1);
        object.keep_tail(&cache);
        object.keep_part(&cache, 1);
        assert!(cache.adopt(KEY, &object.write_to(dir.path())).unwrap());
        assert_eq!((cache.bytes(), object.kept(&cache)), (object.len(), vec![0, 1]));
        cache.check();
    }

    /// TASK-165.4: an object that can't fit, past the capacity or the floor, isn't adopted: its
    /// file is left where it was. One holding no object fails.
    #[test]
    fn adoptions_past_the_capacity_or_the_floor_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let object = Object::noise(KEY, 2 * PART, 1);
        let other = Object::noise(OTHER, 100, 2);
        let cache = Cache::open(dir.path(), object.len() - 1, 0).unwrap();
        other.keep(&cache);
        let from = object.write_to(dir.path());
        assert!(!cache.adopt(KEY, &from).unwrap());
        assert_eq!((cache.bytes(), other.kept(&cache)), (other.len(), vec![0]));
        drop(cache);
        let cache = Cache::open(dir.path(), 8 * PART, u64::MAX / 2).unwrap();
        assert!(!cache.adopt(KEY, &from).unwrap());
        assert_eq!((std::fs::read(&from).unwrap(), cache.bytes()), (object.bytes.clone(), 0));
        assert_eq!(cache.keys(), Vec::<String>::new());

        drop(cache);
        let cache = Cache::open(dir.path(), 8 * PART, 0).unwrap();
        std::fs::write(&from, b"no object").unwrap();
        let err = cache.adopt(KEY, &from).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
        assert!(from.exists());
    }

    /// How a crash test cuts the change it crashes at, if a write or a punch: what of it is made.
    #[derive(Clone, Copy, Debug)]
    enum Tear {
        Nothing,
        FirstByte,
        Half,
        AllButLastByte,
        /// Every other block, by the file's, as writes back reordered leave them.
        OddBlocks,
    }

    const TEARS: [Tear; 5] =
        [Tear::Nothing, Tear::FirstByte, Tear::Half, Tear::AllButLastByte, Tear::OddBlocks];

    impl Tear {
        fn pieces(self, range: Range<u64>) -> Vec<Range<u64>> {
            const BLOCK: u64 = 4096;
            let Range { start, end } = range;
            let prefix = |len: u64| {
                let piece = start..start + len;
                vec![piece]
            };
            match self {
                Tear::Nothing => vec![],
                Tear::FirstByte => prefix(1.min(end - start)),
                Tear::Half => prefix((end - start) / 2),
                Tear::AllButLastByte => prefix((end - start).saturating_sub(1)),
                Tear::OddBlocks => (start / BLOCK..end.div_ceil(BLOCK))
                    .filter(|block| block % 2 == 1)
                    .map(|block| (block * BLOCK).max(start)..((block + 1) * BLOCK).min(end))
                    .collect(),
            }
        }
    }

    /// Where a crash test cuts the changes a cache makes to its files ([`Files`]), numbered as
    /// made: those before `at` made, but `lost`, a write or a punch a power cut lost while later
    /// ones landed, as nothing that syncs orders them; change `at` torn as `tear` says, if a write
    /// or a punch, else not made; none after it.
    #[derive(Debug)]
    pub(super) struct Cut {
        at: usize,
        tear: Tear,
        lost: Option<usize>,
        made: Mutex<Vec<Change>>,
    }

    impl Cut {
        fn new(at: usize, tear: Tear, lost: Option<usize>) -> Arc<Self> {
            Arc::new(Self { at, tear, lost, made: Mutex::default() })
        }

        /// Counts `change` made, or not: its number.
        fn next(&self, change: Change) -> usize {
            let mut made = self.made.lock().unwrap();
            made.push(change);
            made.len() - 1
        }

        pub(super) fn done(&self, change: Change) -> bool {
            let number = self.next(change);
            number < self.at && Some(number) != self.lost
        }

        pub(super) fn pieces(&self, change: Change, range: Range<u64>) -> Vec<Range<u64>> {
            let number = self.next(change);
            match number.cmp(&self.at) {
                std::cmp::Ordering::Less if Some(number) != self.lost => vec![range],
                std::cmp::Ordering::Equal => self.tear.pieces(range),
                _ => vec![],
            }
        }
    }

    /// The crash test's objects: one of two parts, its last short; one of a part shorter than a
    /// block; one of a third of a part; one whose part compresses; and one adopted, of two parts.
    fn crash_objects() -> [Object; 5] {
        let pattern: Vec<u8> = (0..300_000u32).map(|at| (at % 251) as u8).collect();
        [
            Object::noise(KEY, PART + 3000, 1),
            Object::noise(OTHER, 3000, 2),
            Object::noise("segments/0000000000000001/0000000000000004", 300_000, 3),
            Object::new("checkpoints/0000000000000001/0000000000000004", &pattern),
            Object::noise("segments/0000000000000002/0000000000000001", PART + 100, 4),
        ]
    }

    /// The crash test's capacity: the first object, and a part's third, so that what it keeps
    /// evicts parts, tails and objects.
    const CRASH_CAPACITY: u64 = PART + 300_000;

    /// What the crash test does with a cache in `dir`, its changes cut by `cut`: keeps objects and
    /// reads them, evicting parts and objects to stay within its capacity, forgets one, and adopts
    /// another from a file in `wal`.
    fn crash_workload(dir: &Path, wal: &Path, cut: Arc<Cut>) {
        let [a, b, c, d, e] = crash_objects();
        let mut cache = Cache::open(dir, CRASH_CAPACITY, 0).unwrap();
        cache.files.cut = Some(cut);
        // A crash past a change makes none after it: what fails then is not the cache's.
        let keep = |object: &Object, parts: &[u64]| {
            let _ = cache.keep_tail(&object.key, &object.tail);
            for &part in parts {
                let stored = object.stored(part);
                let _ = cache.keep_part(&object.key, part, &stored, object.part(part));
            }
        };
        keep(&a, &[0, 1]);
        keep(&b, &[0]);
        let _ = cache.part(&a.key, 0, &a.stored(0));
        // Evicts a's part 1, b, and a's part 0.
        keep(&c, &[0]);
        let _ = cache.forget(&a.key);
        keep(&d, &[0]);
        // Evicts c's part.
        let _ = cache.adopt(&e.key, &e.write_to(wal));
        keep(&b, &[0]);
        let _ = cache.tail(&e.key);
        let _ = cache.part(&e.key, 1, &e.stored(1));
    }

    /// Checks the cache in `dir` opened after a crash: its index whole, within its capacity; no file
    /// but its objects'; and everything it keeps read back as kept, or dropped. Then a reader of
    /// all keeps all anew, as in a cache never crashed, which one opened after it finds.
    fn check_after_crash(dir: &Path, crashed: &str) {
        let objects = crash_objects();
        let cache = Cache::open(dir, CRASH_CAPACITY, 0).unwrap();
        cache.check();
        assert!(cache.bytes() <= CRASH_CAPACITY, "{crashed}: {} bytes", cache.bytes());
        assert_eq!(cache.keys(), files(dir), "{crashed}");
        let mut read = 0;
        for object in &objects {
            let Some(tail) = cache.tail(&object.key).unwrap() else { continue };
            assert_eq!(tail, object.tail, "{crashed}: {}", object.key);
            read += object.tail_len();
            for part in 0..object.parts() {
                let Some(bytes) = object.read(&cache, part) else { continue };
                assert_eq!(bytes, object.part(part), "{crashed}: part {part} of {}", object.key);
                read += object.stored(part).len;
            }
        }
        assert_eq!(cache.bytes(), read, "{crashed}");
        cache.check();
        drop(cache);

        let cache = Cache::open(dir, 8 * PART, 0).unwrap();
        for object in &objects {
            object.fetch(&cache);
        }
        drop(cache);
        let cache = Cache::open(dir, 8 * PART, 0).unwrap();
        for object in &objects {
            assert_eq!(object.kept(&cache), Vec::from_iter(0..object.parts()), "{crashed}");
        }
        assert_eq!(cache.bytes(), objects.iter().map(Object::len).sum::<u64>(), "{crashed}");
    }

    /// TASK-165.4: a crash at any change the cache makes to its files leaves nothing a cache
    /// opened after it reads back wrong: each change in turn the one cut, not made, or a write or
    /// a punch torn (its first byte, half, all but its last byte, every other block), the changes
    /// after it never made; and, as a power cut leaves them, each write or punch lost with those
    /// after it made, the next one or all. What a cache opened then keeps reads
    /// back as kept, or fails its check and is dropped; it holds no file but its objects', counts
    /// what it keeps, and keeps all anew.
    #[test]
    fn crashes_at_each_change_leave_nothing_read_back_wrong() {
        let changes = {
            let dir = tempfile::tempdir().unwrap();
            let cut = Cut::new(usize::MAX, Tear::Nothing, None);
            crash_workload(dir.path(), dir.path(), cut.clone());
            check_after_crash(dir.path(), "uncut");
            cut.made.lock().unwrap().clone()
        };
        // It makes each kind of change.
        for change in [Change::Create, Change::Resize, Change::Write, Change::Punch] {
            assert!(changes.contains(&change), "{change:?} in {changes:?}");
        }
        assert!(changes.contains(&Change::Remove) && changes.contains(&Change::Rename));
        let mut cuts = Vec::new();
        for (at, change) in changes.iter().enumerate() {
            match change {
                Change::Write | Change::Punch => cuts.extend(TEARS.map(|tear| (at, tear, None))),
                _ => cuts.push((at, Tear::Nothing, None)),
            }
        }
        for (lost, change) in changes.iter().enumerate() {
            if matches!(change, Change::Write | Change::Punch) {
                if lost + 2 < changes.len() {
                    cuts.push((lost + 2, Tear::Nothing, Some(lost)));
                }
                cuts.push((changes.len(), Tear::Nothing, Some(lost)));
            }
        }
        // Each in a directory of its own, a few at once.
        let threads = std::thread::available_parallelism().map_or(4, usize::from);
        let cuts = Mutex::new(cuts);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    loop {
                        let Some((at, tear, lost)) = cuts.lock().unwrap().pop() else { break };
                        let crashed =
                            format!("cut at {at} of {changes:?}, {tear:?}, losing {lost:?}");
                        let dir = tempfile::tempdir().unwrap();
                        crash_workload(dir.path(), dir.path(), Cut::new(at, tear, lost));
                        check_after_crash(dir.path(), &crashed);
                    }
                });
            }
        });
    }
}
