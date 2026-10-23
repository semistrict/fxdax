// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A host's copies of the parts of objects it fetched (doc-7, Reading: the NVMe cache): each
//! object's in a sparse file, its parts as stored, compressed or not, at their offsets in the
//! object, the least recently read
//! evicted by punching holes, so that they take no more than a capacity and the filesystem keeps a
//! floor of free space. They are copies of what is in the bucket, so none is data the bucket lacks;
//! a host starts with none.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A part of an object, by its key and number.
type Id = (String, u64);

/// The cache's lock is held only to change what it keeps, not across its files' I/O: a part's write,
/// or its eviction, is done outside it, the part busy meanwhile, and an object's file removed with
/// its last part so too, the object busy; neither is kept or read until done (TASK-92.21).
#[derive(Debug)]
pub struct Cache {
    dir: PathBuf,
    capacity: u64,
    floor: u64,
    kept: Mutex<Kept>,
}

#[derive(Debug, Default)]
struct Kept {
    /// Each part's offset and length, and when it was last read.
    parts: HashMap<Id, (u64, u64, u64)>,
    /// The parts by when they were last read.
    by_use: BTreeMap<u64, Id>,
    /// How many parts each object has kept, or is being written.
    objects: HashMap<String, usize>,
    /// The parts being written, or evicted.
    busy: HashSet<Id>,
    /// The objects whose files are being removed.
    removing: HashSet<String>,
    clock: u64,
    /// The bytes of the parts kept, and being written.
    bytes: u64,
}

/// What evicting a part leaves to do with its object's file, outside the cache's lock.
enum Evicted {
    /// Punch the part's hole: its offset and length.
    Punch(Id, u64, u64),
    /// Remove the file of the object, whose last part it was.
    Remove(String),
}

impl Kept {
    fn touch(&mut self, id: &Id) {
        self.clock += 1;
        let clock = self.clock;
        if let Some((.., used)) = self.parts.get_mut(id) {
            self.by_use.remove(used);
            *used = clock;
            self.by_use.insert(clock, id.clone());
        }
    }

    /// Drops `id`, no longer in `by_use`: it, or its object, is busy until [`Cache::evict`] has
    /// done with its file what this leaves to do.
    fn evict(&mut self, id: Id) -> Evicted {
        let (offset, len, _) = self.parts.remove(&id).expect("a part kept");
        self.bytes -= len;
        if self.release(&id.0) {
            self.removing.insert(id.0.clone());
            return Evicted::Remove(id.0);
        }
        self.busy.insert(id.clone());
        Evicted::Punch(id, offset, len)
    }

    /// Counts a part of the object at `key` gone. Returns whether it was its last.
    fn release(&mut self, key: &str) -> bool {
        let left = self.objects.get_mut(key).expect("its object kept");
        *left -= 1;
        if *left > 0 {
            return false;
        }
        self.objects.remove(key);
        true
    }
}

/// Where the cache keeps its files under the directory it is given, which it empties at start.
const FILES: &str = "fxdax-parts";

impl Cache {
    /// A cache in `dir` of at most `capacity` bytes of parts, keeping `floor` bytes free there.
    pub fn new(dir: &Path, capacity: u64, floor: u64) -> io::Result<Self> {
        let dir = dir.join(FILES);
        match std::fs::remove_dir_all(&dir) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir, capacity, floor, kept: Mutex::default() })
    }

    pub(crate) fn path(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    /// The stored bytes of part `part` of the object at `key`, if they are kept.
    pub fn get(&self, key: &str, part: u64) -> io::Result<Option<Vec<u8>>> {
        let id = (key.to_owned(), part);
        let (offset, len) = {
            let mut kept = self.kept.lock().unwrap();
            let Some(&(offset, len, _)) = kept.parts.get(&id) else { return Ok(None) };
            kept.touch(&id);
            (offset, len)
        };
        let mut bytes = vec![0; len as usize];
        match File::open(self.path(key)).and_then(|file| file.read_exact_at(&mut bytes, offset)) {
            Ok(()) => Ok(Some(bytes)),
            // Evicted meanwhile.
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Keeps `bytes`, the stored bytes of part `part` of the object at `key`, of `size` bytes, at
    /// `offset` in it, evicting what was read least recently to make room: unless no room can be
    /// made, or it, or its object, is busy.
    pub fn put(
        &self,
        key: &str,
        size: u64,
        part: u64,
        offset: u64,
        bytes: &[u8],
    ) -> io::Result<()> {
        let id = (key.to_owned(), part);
        let len = bytes.len() as u64;
        if len > self.capacity {
            return Ok(());
        }
        let free = self.free()?;
        let (fits, evicted) = {
            let mut kept = self.kept.lock().unwrap();
            if kept.parts.contains_key(&id)
                || kept.busy.contains(&id)
                || kept.removing.contains(key)
            {
                return Ok(());
            }
            let (mut evicted, mut freed) = (Vec::new(), 0);
            let fits = loop {
                if kept.bytes + len <= self.capacity && free + freed >= self.floor + len {
                    break true;
                }
                let Some((_, oldest)) = kept.by_use.pop_first() else { break false };
                freed += kept.parts[&oldest].1;
                evicted.push(kept.evict(oldest));
            };
            if fits {
                kept.bytes += len;
                *kept.objects.entry(key.to_owned()).or_default() += 1;
                kept.busy.insert(id.clone());
            }
            (fits, evicted)
        };
        // Its object's file, if one of those evicted was its last part, goes first.
        let written = self.evict(evicted);
        if !fits {
            return written;
        }
        let written = written.and_then(|()| self.write(key, size, offset, bytes));
        let mut kept = self.kept.lock().unwrap();
        kept.busy.remove(&id);
        if written.is_ok() {
            kept.clock += 1;
            let clock = kept.clock;
            kept.parts.insert(id.clone(), (offset, len, clock));
            kept.by_use.insert(clock, id);
            return written;
        }
        kept.bytes -= len;
        // What it wrote is no part kept: with no other of its object's, its file goes.
        if kept.release(key) {
            kept.removing.insert(key.to_owned());
            drop(kept);
            if let Err(err) = self.evict(vec![Evicted::Remove(key.to_owned())]) {
                log::warn!("fxdax: removing {key} from the cache: {err}");
            }
        }
        written
    }

    /// Writes `bytes` at `offset` in the file of the object at `key`, of `size` bytes.
    fn write(&self, key: &str, size: u64, offset: u64, bytes: &[u8]) -> io::Result<()> {
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
        file.write_all_at(bytes, offset)
    }

    /// Drops part `part` of the object at `key`: one that failed its check.
    pub fn forget(&self, key: &str, part: u64) -> io::Result<()> {
        let id = (key.to_owned(), part);
        let evicted = {
            let mut kept = self.kept.lock().unwrap();
            let Some(&(.., used)) = kept.parts.get(&id) else { return Ok(()) };
            kept.by_use.remove(&used);
            vec![kept.evict(id)]
        };
        self.evict(evicted)
    }

    /// How many bytes of parts are kept.
    pub fn bytes(&self) -> u64 {
        self.kept.lock().unwrap().bytes
    }

    /// Does with the files of the parts evicted what evicting them left to do ([`Kept::evict`]),
    /// holding no lock: punches their holes, or removes their objects' files with their last
    /// parts; then they are no longer busy.
    fn evict(&self, evicted: Vec<Evicted>) -> io::Result<()> {
        let mut done = Ok(());
        for evicted in &evicted {
            let evicting = match evicted {
                Evicted::Punch(id, offset, len) => (File::options().write(true))
                    .open(self.path(&id.0))
                    .and_then(|file| punch(&file, *offset, *len)),
                Evicted::Remove(key) => std::fs::remove_file(self.path(key)),
            };
            match evicting {
                // Its object's file removed meanwhile, with the object's last part, which another
                // put evicted: its hole with it.
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                evicting => done = done.and(evicting),
            }
        }
        let mut kept = self.kept.lock().unwrap();
        for evicted in evicted {
            match evicted {
                Evicted::Punch(id, ..) => kept.busy.remove(&id),
                Evicted::Remove(key) => kept.removing.remove(&key),
            };
        }
        done
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
    use std::os::fd::AsRawFd as _;
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
    use std::os::fd::AsRawFd as _;
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

    const KEY: &str = "segments/0000000000000001/0000000000000002";

    fn part(fill: u8) -> Vec<u8> {
        vec![fill; PART as usize]
    }

    /// Parts read least recently go first, by punching holes where they were; an object's last
    /// part takes its file with it.
    #[test]
    fn keeps_parts_evicting_the_least_recently_read() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        for number in 0..3 {
            cache.put(KEY, 6 * PART, number, number * PART, &part(number as u8 + 1)).unwrap();
        }
        assert_eq!(cache.bytes(), 3 * PART);
        assert_eq!(cache.get(KEY, 0).unwrap(), Some(part(1)));

        // Part 1 was read least recently.
        cache.put(KEY, 6 * PART, 5, 5 * PART, &part(6)).unwrap();
        assert_eq!(cache.get(KEY, 1).unwrap(), None);
        let mut hole = vec![1; PART as usize];
        File::open(cache.path(KEY)).unwrap().read_exact_at(&mut hole, PART).unwrap();
        assert_eq!(hole, vec![0; PART as usize]);
        assert_eq!(cache.get(KEY, 0).unwrap(), Some(part(1)));
        assert_eq!(cache.get(KEY, 2).unwrap(), Some(part(3)));
        assert_eq!(cache.get(KEY, 5).unwrap(), Some(part(6)));

        let other = "segments/0000000000000001/0000000000000003";
        cache.put(other, 200, 0, 0, &[7; 100]).unwrap();
        assert_eq!(cache.get(other, 0).unwrap(), Some(vec![7; 100]));
        assert_eq!(cache.bytes(), 2 * PART + 100);
        cache.forget(other, 0).unwrap();
        assert!(!cache.path(other).exists());
        assert_eq!(cache.get(other, 0).unwrap(), None);
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
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        for number in 0..3 {
            cache.put(KEY, 64 * PART, number, number * PART, &part(1)).unwrap();
        }
        assert_eq!(allocated(&cache, KEY), 3 * PART);
        cache.put(KEY, 64 * PART, 63, 63 * PART, &part(1)).unwrap();
        assert_eq!(allocated(&cache, KEY), 3 * PART);
    }

    /// Nothing is kept that would leave less than the floor free.
    #[test]
    fn keeps_the_floor_free() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, u64::MAX / 2).unwrap();
        cache.put(KEY, PART, 0, 0, &part(1)).unwrap();
        assert_eq!(cache.get(KEY, 0).unwrap(), None);
        assert_eq!(cache.bytes(), 0);
    }

    /// What goes first is what was read least recently, not put: each read counts.
    #[test]
    fn reads_order_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        for number in 0..3 {
            cache.put(KEY, 6 * PART, number, number * PART, &part(1)).unwrap();
        }
        cache.get(KEY, 0).unwrap();
        cache.get(KEY, 1).unwrap();
        cache.put(KEY, 6 * PART, 3, 3 * PART, &part(1)).unwrap();
        assert_eq!(cache.get(KEY, 2).unwrap(), None);
        assert_eq!(cache.get(KEY, 0).unwrap(), Some(part(1)));
        assert_eq!(cache.get(KEY, 1).unwrap(), Some(part(1)));
        assert_eq!(cache.get(KEY, 3).unwrap(), Some(part(1)));
    }

    /// A part whose file went, or shrank, behind the cache's back reads as not kept; one whose file
    /// can't be read fails.
    #[test]
    fn parts_gone_read_as_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        cache.put(KEY, 2 * PART, 1, PART, &part(1)).unwrap();
        File::options().write(true).open(cache.path(KEY)).unwrap().set_len(PART).unwrap();
        assert_eq!(cache.get(KEY, 1).unwrap(), None);
        std::fs::remove_file(cache.path(KEY)).unwrap();
        assert_eq!(cache.get(KEY, 1).unwrap(), None);
        std::fs::create_dir(cache.path(KEY)).unwrap();
        let err = cache.get(KEY, 1).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EISDIR), "{err}");
    }

    /// A part larger than the cache isn't kept, and evicts nothing; one as large is; one kept
    /// already is kept once.
    #[test]
    fn keeps_parts_that_fit_once() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 2 * PART, 0).unwrap();
        cache.put(KEY, 8 * PART, 0, 0, &part(1)).unwrap();
        cache.put(KEY, 8 * PART, 0, 0, &part(1)).unwrap();
        assert_eq!(cache.bytes(), PART);
        let other = "segments/0000000000000001/0000000000000003";
        cache.put(other, 8 * PART, 0, 0, &vec![2; 2 * PART as usize + 1]).unwrap();
        assert_eq!(cache.get(other, 0).unwrap(), None);
        assert_eq!(cache.get(KEY, 0).unwrap(), Some(part(1)));
        cache.put(other, 8 * PART, 1, 0, &vec![2; 2 * PART as usize]).unwrap();
        assert_eq!(cache.get(other, 1).unwrap(), Some(vec![2; 2 * PART as usize]));
        assert_eq!(cache.get(KEY, 0).unwrap(), None);
        assert_eq!(cache.bytes(), 2 * PART);
    }

    /// An object's file is its size from its first part, the rest holes.
    #[test]
    fn files_are_their_objects_size() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        cache.put(KEY, 5 * PART, 0, 0, &part(1)).unwrap();
        assert_eq!(std::fs::metadata(cache.path(KEY)).unwrap().len(), 5 * PART);
    }

    /// The free bytes are the free blocks' size, as statvfs counts them.
    #[test]
    fn free_bytes_are_the_free_blocks_size() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 0, 0).unwrap();
        let path = std::ffi::CString::new(cache.dir.as_os_str().as_encoded_bytes()).unwrap();
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is a C string and `stat` a statvfs, which the call fills.
        assert_eq!(unsafe { libc::statvfs(path.as_ptr(), &mut stat) }, 0);
        #[allow(clippy::unnecessary_cast, reason = "the fields' types differ by platform")]
        let (blocks, size) = (stat.f_bavail as u64, stat.f_frsize as u64);
        // Within a few blocks of it: other processes write meanwhile.
        let free = cache.free().unwrap();
        assert!(free.abs_diff(blocks * size) < 1024 * size, "{free} bytes, {blocks} of {size}");
    }

    /// A part whose write fails is neither kept nor counted; the cache keeps others after it.
    #[test]
    fn parts_whose_writes_fail_are_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        let other = "segments/0000000000000001/0000000000000003";
        std::fs::create_dir_all(cache.path(other)).unwrap();
        let failed = cache.put(other, PART, 0, 0, &part(1)).unwrap_err();
        assert_eq!(failed.raw_os_error(), Some(libc::EISDIR), "{failed}");
        assert_eq!((cache.get(other, 0).unwrap(), cache.bytes()), (None, 0));
        cache.put(KEY, PART, 0, 0, &part(2)).unwrap();
        assert_eq!((cache.get(KEY, 0).unwrap(), cache.bytes()), (Some(part(2)), PART));
    }

    /// TASK-92.21: a part dropped whose object's file went meanwhile, as one does that another
    /// put's eviction of the object's last part removes outside the cache's lock, is dropped, its
    /// hole gone with the file; and so is the object's last part.
    #[test]
    fn parts_dropped_whose_files_went_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        cache.put(KEY, 2 * PART, 0, 0, &part(1)).unwrap();
        cache.put(KEY, 2 * PART, 1, PART, &part(2)).unwrap();
        std::fs::remove_file(cache.path(KEY)).unwrap();
        cache.forget(KEY, 0).unwrap();
        assert_eq!(cache.bytes(), PART);
        cache.forget(KEY, 1).unwrap();
        assert_eq!(cache.bytes(), 0);
    }

    /// TASK-92.21: puts, reads and drops of a few objects' parts, in threads at once, evicting one
    /// another's, keep the cache whole: no put or drop fails, and, once they are done, what the
    /// cache counts it keeps, it keeps, each part kept read whole.
    #[test]
    fn parts_kept_read_and_dropped_at_once_keep_the_cache_whole() {
        const OBJECTS: u64 = 3;
        const PARTS: u64 = 4;
        let dir = tempfile::tempdir().unwrap();
        let cache = std::sync::Arc::new(Cache::new(dir.path(), 6 * PART, 0).unwrap());
        let key = |object: u64| format!("segments/0000000000000001/{object:016x}");
        let fill = |object: u64, part: u64| (object * PARTS + part + 1) as u8;
        let threads: Vec<_> = (0..4u64)
            .map(|thread| {
                let cache = cache.clone();
                std::thread::spawn(move || {
                    let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ thread;
                    for _ in 0..500 {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        let (object, number) = (state % OBJECTS, state / OBJECTS % PARTS);
                        let (key, at) = (key(object), number * PART);
                        match state / (OBJECTS * PARTS) % 4 {
                            0 | 1 => {
                                let bytes = part(fill(object, number));
                                cache.put(&key, PARTS * PART, number, at, &bytes).unwrap();
                            }
                            2 => drop(cache.get(&key, number).unwrap()),
                            _ => cache.forget(&key, number).unwrap(),
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let mut kept = 0;
        for object in 0..OBJECTS {
            for number in 0..PARTS {
                if let Some(bytes) = cache.get(&key(object), number).unwrap() {
                    assert_eq!(bytes, part(fill(object, number)), "part {number} of {object}");
                    kept += PART;
                }
            }
        }
        assert_eq!(cache.bytes(), kept);
        let held = cache.kept.lock().unwrap();
        assert_eq!((held.busy.len(), held.removing.len()), (0, 0));
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
        let cache = std::sync::Arc::new(Cache::new(dir.path(), 16 * PART, 0).unwrap());
        cache.put(KEY, PART, 0, 0, &part(1)).unwrap();
        let keeping = {
            let cache = cache.clone();
            std::thread::spawn(move || {
                for number in 0..PUTS {
                    let key = format!("segments/0000000000000002/{:016x}", number / 8);
                    let at = number % 8 * PART;
                    cache.put(&key, 8 * PART, number % 8, at, &part(number as u8)).unwrap();
                }
            })
        };
        let (mut most, mut all, mut reads) =
            (std::time::Duration::ZERO, std::time::Duration::ZERO, 0);
        while !keeping.is_finished() {
            let started = std::time::Instant::now();
            assert_eq!(cache.get(KEY, 0).unwrap(), Some(part(1)));
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
        Cache::new(dir.path(), 3 * PART, 0).unwrap().put(KEY, PART, 0, 0, &part(1)).unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        assert_eq!(cache.get(KEY, 0).unwrap(), None);
        assert!(!cache.path(KEY).exists());
    }
}
