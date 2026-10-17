//! A host's copies of the parts of objects it fetched (doc-7, Reading: the NVMe cache): each
//! object's in a sparse file, its parts at their offsets in the object, the least recently read
//! evicted by punching holes, so that they take no more than a capacity and the filesystem keeps a
//! floor of free space. They are copies of what is in the bucket, so none is data the bucket lacks;
//! a host starts with none.

use crate::layout::PART;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A part of an object, by its key and number.
type Id = (String, u64);

#[derive(Debug)]
pub struct Cache {
    dir: PathBuf,
    capacity: u64,
    floor: u64,
    kept: Mutex<Kept>,
}

#[derive(Debug, Default)]
struct Kept {
    /// Each part's length, and when it was last read.
    parts: HashMap<Id, (u64, u64)>,
    /// The parts by when they were last read.
    by_use: BTreeMap<u64, Id>,
    /// How many parts each object has kept.
    objects: HashMap<String, usize>,
    clock: u64,
    bytes: u64,
}

impl Kept {
    fn touch(&mut self, id: &Id) {
        self.clock += 1;
        let clock = self.clock;
        if let Some((_, used)) = self.parts.get_mut(id) {
            self.by_use.remove(used);
            *used = clock;
            self.by_use.insert(clock, id.clone());
        }
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

    /// Part `part` of the object at `key`, if it is kept.
    pub fn get(&self, key: &str, part: u64) -> io::Result<Option<Vec<u8>>> {
        let id = (key.to_owned(), part);
        let len = {
            let mut kept = self.kept.lock().unwrap();
            let Some(&(len, _)) = kept.parts.get(&id) else { return Ok(None) };
            kept.touch(&id);
            len
        };
        let mut bytes = vec![0; len as usize];
        match File::open(self.path(key))
            .and_then(|file| file.read_exact_at(&mut bytes, part * PART))
        {
            Ok(()) => Ok(Some(bytes)),
            // Evicted meanwhile.
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Keeps `bytes`, part `part` of the object at `key`, of `size` bytes, evicting what was read
    /// least recently to make room: unless no room can be made.
    pub fn put(&self, key: &str, size: u64, part: u64, bytes: &[u8]) -> io::Result<()> {
        let id = (key.to_owned(), part);
        let len = bytes.len() as u64;
        let mut kept = self.kept.lock().unwrap();
        if kept.parts.contains_key(&id) || len > self.capacity {
            return Ok(());
        }
        while kept.bytes + len > self.capacity || self.free()? < self.floor + len {
            let Some((_, oldest)) = kept.by_use.pop_first() else { return Ok(()) };
            self.evict(&mut kept, &oldest)?;
        }
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file =
            std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(path)?;
        // Its file is the object's size from the first, so that what is not written to it is a
        // hole: some filesystems (APFS) fill what a write past the end skips.
        if !kept.objects.contains_key(key) {
            file.set_len(size)?;
        }
        file.write_all_at(bytes, part * PART)?;
        kept.clock += 1;
        let clock = kept.clock;
        kept.parts.insert(id.clone(), (len, clock));
        kept.by_use.insert(clock, id);
        *kept.objects.entry(key.to_owned()).or_default() += 1;
        kept.bytes += len;
        Ok(())
    }

    /// Drops part `part` of the object at `key`: one that failed its check.
    pub fn forget(&self, key: &str, part: u64) -> io::Result<()> {
        let id = (key.to_owned(), part);
        let mut kept = self.kept.lock().unwrap();
        if let Some(&(_, used)) = kept.parts.get(&id) {
            kept.by_use.remove(&used);
            self.evict(&mut kept, &id)?;
        }
        Ok(())
    }

    /// How many bytes of parts are kept.
    pub fn bytes(&self) -> u64 {
        self.kept.lock().unwrap().bytes
    }

    /// Removes `id`, no longer in `by_use`: punches its hole, or removes its object's file with its
    /// last part.
    fn evict(&self, kept: &mut Kept, id: &Id) -> io::Result<()> {
        let (len, _) = kept.parts.remove(id).expect("a part kept");
        kept.bytes -= len;
        let path = self.path(&id.0);
        let left = kept.objects.get_mut(&id.0).expect("its object kept");
        *left -= 1;
        if *left == 0 {
            kept.objects.remove(&id.0);
            return std::fs::remove_file(path);
        }
        punch(&File::options().write(true).open(path)?, id.1 * PART, len)
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
            cache.put(KEY, 6 * PART, number, &part(number as u8 + 1)).unwrap();
        }
        assert_eq!(cache.bytes(), 3 * PART);
        assert_eq!(cache.get(KEY, 0).unwrap(), Some(part(1)));

        // Part 1 was read least recently.
        cache.put(KEY, 6 * PART, 5, &part(6)).unwrap();
        assert_eq!(cache.get(KEY, 1).unwrap(), None);
        let mut hole = vec![1; PART as usize];
        File::open(cache.path(KEY)).unwrap().read_exact_at(&mut hole, PART).unwrap();
        assert_eq!(hole, vec![0; PART as usize]);
        assert_eq!(cache.get(KEY, 0).unwrap(), Some(part(1)));
        assert_eq!(cache.get(KEY, 2).unwrap(), Some(part(3)));
        assert_eq!(cache.get(KEY, 5).unwrap(), Some(part(6)));

        let other = "segments/0000000000000001/0000000000000003";
        cache.put(other, 200, 0, &[7; 100]).unwrap();
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
            cache.put(KEY, 64 * PART, number, &part(1)).unwrap();
        }
        assert_eq!(allocated(&cache, KEY), 3 * PART);
        cache.put(KEY, 64 * PART, 63, &part(1)).unwrap();
        assert_eq!(allocated(&cache, KEY), 3 * PART);
    }

    /// Nothing is kept that would leave less than the floor free.
    #[test]
    fn keeps_the_floor_free() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, u64::MAX / 2).unwrap();
        cache.put(KEY, PART, 0, &part(1)).unwrap();
        assert_eq!(cache.get(KEY, 0).unwrap(), None);
        assert_eq!(cache.bytes(), 0);
    }

    /// A cache starts empty, whatever an earlier one left.
    #[test]
    fn starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        Cache::new(dir.path(), 3 * PART, 0).unwrap().put(KEY, PART, 0, &part(1)).unwrap();
        let cache = Cache::new(dir.path(), 3 * PART, 0).unwrap();
        assert_eq!(cache.get(KEY, 0).unwrap(), None);
        assert!(!cache.path(KEY).exists());
    }
}
