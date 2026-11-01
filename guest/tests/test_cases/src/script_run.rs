// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the interpreter of the scripts `script` makes, which does their operations on files, each
//! with a mapping of it kept across all of them, and checks them against the model (TASK-149.2).
//! Nothing here is of Fxfs: the tests below run it on the host's filesystems.
//!
//! On a filesystem that may be `full` no operation needs to succeed, as long as it fails with
//! ENOSPC, and leaves the files as the model says, which a refused write, store, truncation or
//! fsync leaves as they were. A clone or a punch, of many steps, stands as far as it got: the model
//! takes that from what the files read, within what it could have come to
//! ([`Model::clone_stopped_short`], [`Model::punch_stopped_short`]). A store that finds no room is
//! a SIGBUS, which would end the guest's init: there every store is a child's.

use crate::script::{self, Failure, MOST, Mix, Model, Op, PAGE, Step, kind};
use nix::libc;
use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicUsize, Ordering};

const FICLONE: libc::c_ulong = 0x4004_9409;

/// The operations of `mix`'s script of `seed`, each with its index in the whole of it, less those
/// `script::DROP_VAR` drops, and the whole script's length, which is the index of its end.
pub fn from_env(seed: u64, mix: Mix) -> (Vec<(usize, Op)>, usize) {
    let drops = std::env::var(script::DROP_VAR).unwrap_or_default();
    let ops = script::generate(seed, mix);
    let kept = script::kept(&drops, ops.len()).expect(script::DROP_VAR);
    (kept.into_iter().map(|index| (index, ops[index])).collect(), ops.len())
}

/// What's free on the filesystem of `name`.
pub fn free(name: &str) -> u64 {
    let name = CString::new(name).unwrap();
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::statvfs(name.as_ptr(), &mut stat) }, 0);
    stat.f_bfree as u64 * stat.f_frsize as u64
}

/// Takes all the room but `left` on a filesystem, or as much as it gives, with the file `ballast`:
/// after making what needs room to be made.
pub fn fill(ballast: &str, left: u64) {
    let mut file = fs::File::create(ballast).unwrap();
    let block = vec![0x5a; 1 << 20];
    while free(ballast) > left {
        match file.write_all(&block).and_then(|()| file.sync_all()) {
            Ok(()) => {}
            Err(err) if err.raw_os_error() == Some(libc::ENOSPC) => break,
            Err(err) => panic!("filling the filesystem: {err}"),
        }
    }
}

/// A shared mapping of `len` bytes of `fd`, or of memory that a child shares if `fd` is -1.
struct Mapping {
    bytes: *mut u8,
    len: usize,
}

impl Mapping {
    fn new(fd: libc::c_int, len: usize) -> Self {
        let anonymous = if fd < 0 { libc::MAP_ANONYMOUS } else { 0 };
        let prot = libc::PROT_READ | libc::PROT_WRITE;
        let null = std::ptr::null_mut();
        let map = unsafe { libc::mmap(null, len, prot, libc::MAP_SHARED | anonymous, fd, 0) };
        assert_ne!(map, libc::MAP_FAILED, "mmap: {}", io::Error::last_os_error());
        Self { bytes: map.cast(), len }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.bytes.cast(), self.len) };
    }
}

/// How an operation failed: its kind, and what to say of it.
type Failed = (String, String);

fn differs(what: String) -> Failed {
    (kind::DIFFERS.to_owned(), what)
}

fn errno(what: &str, err: &io::Error) -> Failed {
    (Failure::errno_kind(err.raw_os_error()), format!("{what}: {err}"))
}

/// What's left of a run that passed.
pub struct Ran {
    pub model: Model,
    pub done: Vec<String>,
    pub refused: usize,
}

/// A script's files, as a run has them.
pub struct Run {
    files: Vec<fs::File>,
    maps: Vec<Mapping>,
    /// How many bytes a child has stored, in memory it shares.
    stored: Mapping,
    model: Model,
    /// Whether the filesystem may refuse operations for lack of room.
    full: bool,
    /// Each operation done, as it was resolved.
    pub done: Vec<String>,
    /// How many it refused.
    pub refused: usize,
}

impl Run {
    /// A run on the files `names`, which it makes, empty.
    pub fn new(names: &[String], full: bool) -> Self {
        let open = |name: &String| {
            let mut options = fs::OpenOptions::new();
            options.create(true).truncate(true).read(true).write(true);
            options.open(name).unwrap_or_else(|err| panic!("open {name}: {err}"))
        };
        let files: Vec<fs::File> = names.iter().map(open).collect();
        // The mappings kept: as large as the files may be, touched within their sizes alone.
        let maps = files.iter().map(|file| Mapping::new(file.as_raw_fd(), MOST as usize)).collect();
        let stored = Mapping::new(-1, PAGE as usize);
        let model = Model::new(names.len());
        Self { files, maps, stored, model, full, done: Vec::new(), refused: 0 }
    }

    /// Runs `script`, what's left of a script of `len` operations, and checks the files as it ends.
    pub fn script(&mut self, script: &[(usize, Op)], len: usize) -> Result<(), Failure> {
        for (index, op) in script {
            self.step(*index, op)?;
        }
        self.step(len, &Op::Check)
    }

    /// Does `op`, the script's `index`th.
    pub fn step(&mut self, index: usize, op: &Op) -> Result<(), Failure> {
        let step = self.model.resolve(op);
        self.done.push(step.to_string());
        self.act(&step).map_err(|(kind, what)| Failure { op: Some(index), kind, what })
    }

    /// Says `failure`, of the run of the script of `seed`, on standard output, which the host
    /// reads, and panics, with what came before it.
    pub fn fail(&self, seed: u64, failure: &Failure) -> ! {
        println!("{failure}");
        let last = &self.done[self.done.len().saturating_sub(24)..];
        panic!("seed {seed}: {failure}; the last before it:\n{}", last.join("\n"));
    }

    /// Ends the run, the files written back.
    pub fn finish(mut self) -> Ran {
        self.maps.clear();
        let files = std::mem::take(&mut self.files);
        for file in &files {
            if let Err(err) = file.sync_all() {
                self.refused(&err, "fsync").expect("the files' last fsync");
            }
        }
        Ran { model: self.model, done: self.done, refused: self.refused }
    }

    /// Passes `err`, of `what`, if it's a lack of room the filesystem may say.
    fn refused(&mut self, err: &io::Error, what: &str) -> Result<(), Failed> {
        if self.full && err.raw_os_error() == Some(libc::ENOSPC) {
            self.refused += 1;
            return Ok(());
        }
        Err(errno(what, err))
    }

    fn act(&mut self, step: &Step) -> Result<(), Failed> {
        match step {
            Step::Write { file, at, data } => {
                let (file, fd) = (*file, self.files[*file].as_raw_fd());
                let written =
                    unsafe { libc::pwrite(fd, data.as_ptr().cast(), data.len(), *at as i64) };
                if written < 0 {
                    return self.refused(&io::Error::last_os_error(), "pwrite");
                }
                let written = written as usize;
                if written < data.len() && !self.full {
                    let what = format!("{written:#x} bytes of {:#x} at {at:#x}", data.len());
                    return Err((kind::SHORT_WRITE.to_owned(), what));
                }
                self.model.write(file, *at, &data[..written]);
            }
            Step::Store { file, at, data, child } => {
                let stored = if *child || self.full {
                    self.store_in_child(*file, *at, data)?
                } else {
                    let mapped = self.maps[*file].bytes;
                    for (i, &byte) in data.iter().enumerate() {
                        unsafe { mapped.add(*at as usize + i).write_volatile(byte) };
                    }
                    data.len()
                };
                self.model.write(*file, *at, &data[..stored]);
            }
            Step::Read { file, at, len } => self.check(*file, *at, *len)?,
            Step::Truncate { file, size } => match self.files[*file].set_len(*size) {
                Ok(()) => self.model.truncate(*file, *size),
                Err(err) => {
                    self.refused(&err, "truncate")?;
                    // Refused writing the size back, the file has it.
                    if self.length(*file)? == *size {
                        self.model.truncate(*file, *size);
                    }
                }
            },
            Step::Punch { file, at, len } => {
                let punch = libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE;
                let fd = self.files[*file].as_raw_fd();
                if unsafe { libc::fallocate(fd, punch, *at as i64, *len as i64) } == 0 {
                    self.model.punch(*file, *at, *len);
                } else {
                    self.refused(&io::Error::last_os_error(), "punch")?;
                    let size = self.model.files[*file].len() as u64;
                    let end = (*at + *len).min(size);
                    let got = self.read(*file, *at, end.saturating_sub(*at))?;
                    self.model.punch_stopped_short(*file, *at, &got).map_err(differs)?;
                }
            }
            Step::Fsync { file } => {
                if let Err(err) = self.files[*file].sync_all() {
                    self.refused(&err, "fsync")?;
                }
            }
            Step::Clone { from, to } => self.clone_file(*from, *to)?,
            Step::Check => {
                for file in 0..self.files.len() {
                    self.check_whole(file)?;
                }
            }
        }
        Ok(())
    }

    /// Stores `data` at `at` of `file`'s mapping, from a child, which inherits it: all of it, or
    /// on a filesystem with no room left, as much as the child got to store before it died.
    fn store_in_child(&mut self, file: usize, at: u64, data: &[u8]) -> Result<usize, Failed> {
        let stored = unsafe { AtomicUsize::from_ptr(self.stored.bytes.cast()) };
        stored.store(0, Ordering::Relaxed);
        let mapped = self.maps[file].bytes;
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork: {}", io::Error::last_os_error());
        if child == 0 {
            for (i, &byte) in data.iter().enumerate() {
                unsafe { mapped.add(at as usize + i).write_volatile(byte) };
                stored.store(i + 1, Ordering::Release);
            }
            unsafe { libc::_exit(0) };
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
            return Ok(data.len());
        }
        let stored = stored.load(Ordering::Acquire);
        if self.full && libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGBUS {
            self.refused += 1;
            return Ok(stored);
        }
        let what = format!("{stored:#x} bytes of {:#x} stored, the child {status:#x}", data.len());
        Err((kind::STORE_KILLED.to_owned(), what))
    }

    /// FICLONE of `from` onto `to`.
    fn clone_file(&mut self, from: usize, to: usize) -> Result<(), Failed> {
        let (source, destination) = (self.files[from].as_raw_fd(), self.files[to].as_raw_fd());
        let invalid = self.model.clone_is_invalid(from, to);
        if unsafe { libc::ioctl(destination, FICLONE as _, source) } == 0 {
            if invalid {
                return Err(differs(format!("the clone of {from} onto {to} wasn't refused")));
            }
            self.model.clone_file(from, to);
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if invalid && err.raw_os_error() == Some(libc::EINVAL) {
            return Ok(());
        }
        self.refused(&err, "FICLONE")?;
        let got = self.read(to, 0, self.length(to)?)?;
        self.model.clone_stopped_short(from, to, &got).map_err(differs)
    }

    fn length(&self, file: usize) -> Result<u64, Failed> {
        let metadata = self.files[file].metadata();
        Ok(metadata.map_err(|err| errno("fstat", &err))?.len())
    }

    /// What read(2) gives of `len` bytes at `at` of `file`.
    fn read(&self, file: usize, at: u64, len: u64) -> Result<Vec<u8>, Failed> {
        let mut got = vec![0u8; len as usize];
        let n = self.files[file].read_at(&mut got, at);
        got.truncate(n.map_err(|err| errno(&format!("pread at {at:#x}"), &err))?);
        Ok(got)
    }

    /// Checks `len` bytes at `at` of `file`, through read(2), against the model.
    fn check(&self, file: usize, at: u64, len: u64) -> Result<(), Failed> {
        let got = self.read(file, at, len)?;
        let model = &self.model.files[file];
        let end = (at as usize + len as usize).min(model.len()).max(at as usize);
        let want = model.get(at as usize..end).unwrap_or(&[]);
        if got == want {
            return Ok(());
        }
        let first =
            (got.iter().zip(want).position(|(a, b)| a != b)).unwrap_or(got.len().min(want.len()));
        Err(differs(format!(
            "{len:#x} bytes read at {at:#x} of file {file}: {} bytes, differing from the model at \
             {:#x} ({} bytes there)",
            got.len(),
            at as usize + first,
            want.len()
        )))
    }

    /// Checks all of `file`, its size, and through read(2) and its mapping, against the model.
    fn check_whole(&self, file: usize) -> Result<(), Failed> {
        let model = &self.model.files[file];
        let size = self.length(file)?;
        if size != model.len() as u64 {
            let want = model.len();
            return Err(differs(format!("file {file} has size {size:#x}, the model's {want:#x}")));
        }
        self.check(file, 0, size + PAGE)?;
        let through = unsafe { std::slice::from_raw_parts(self.maps[file].bytes, model.len()) };
        if let Some(at) = through.iter().zip(model).position(|(a, b)| a != b) {
            return Err(differs(format!("file {file}'s mapping differs at {at:#x}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{ONE_FILE, Place, SEVERAL_FILES, TRIES, shrink_failure};
    use std::process::Command;
    use tempdir::TempDir;

    /// The room left on a filesystem full enough: a script's files, full of holes, come to less
    /// than a MiB and a half on average, and 2 MiB left saw few refusals.
    const LEFT: u64 = 1 << 20;

    /// The files of a run, in `dir`.
    fn names(dir: &std::path::Path, mix: Mix) -> Vec<String> {
        (0..mix.files).map(|n| dir.join(n.to_string()).to_str().unwrap().to_owned()).collect()
    }

    fn numbered(mix: Mix, seed: u64) -> (Vec<(usize, Op)>, usize) {
        let ops = script::generate(seed, mix);
        let len = ops.len();
        (ops.into_iter().enumerate().collect(), len)
    }

    /// Checks that `names` hold what `model` does.
    fn assert_hold(names: &[String], model: &Model) {
        for (name, held) in names.iter().zip(&model.files) {
            assert!(fs::read(name).unwrap() == *held, "{name} differs from its model");
        }
    }

    #[test]
    fn one_files_script_passes_on_this_hosts_filesystem() {
        let dir = TempDir::new("script").unwrap();
        let names = names(dir.path(), ONE_FILE);
        let (script, len) = numbered(ONE_FILE, 1);
        let mut run = Run::new(&names, false);
        run.script(&script, len).unwrap_or_else(|failure| run.fail(1, &failure));
        let ran = run.finish();
        assert_eq!(ran.done.len(), len + 1);
        assert_eq!(ran.refused, 0);
        assert_hold(&names, &ran.model);
    }

    /// With reflinks: a host whose temporary directory has none fails it, with EOPNOTSUPP.
    #[test]
    fn several_files_script_passes_on_this_hosts_filesystem() {
        let dir = TempDir::new("script").unwrap();
        let names = names(dir.path(), SEVERAL_FILES);
        for seed in 1..=seeds() {
            let (script, len) = numbered(SEVERAL_FILES, seed);
            let mut run = Run::new(&names, false);
            run.script(&script, len).unwrap_or_else(|failure| run.fail(seed, &failure));
            let ran = run.finish();
            assert_eq!(ran.refused, 0);
            assert_hold(&names, &ran.model);
        }
    }

    /// A clone onto a longer file: what the VFS says, and the model.
    #[test]
    fn clones_do_as_the_model_says() {
        let dir = TempDir::new("script").unwrap();
        let mix = Mix { files: 2, ..ONE_FILE };
        let names = names(dir.path(), mix);
        let mut run = Run::new(&names, false);
        let block = PAGE;
        let write = |file, at, len| Op::Write { file, place: Place::At(at), len, data: 1 + at };
        let script = [
            write(0, 0, 3 * block),
            write(1, 0, 8 * block),
            // Whole blocks, within the destination: its last ones stay.
            Op::Clone { from: 0, to: 1 },
            Op::Check,
            // Into a shorter file: it grows.
            Op::Clone { from: 1, to: 0 },
            Op::Check,
            Op::Truncate { file: 0, size: 2 * block + 100 },
            // A partial block within the destination: refused.
            Op::Clone { from: 0, to: 1 },
            Op::Check,
            // Writes after a clone are each file's own, through a mapping too.
            Op::Clone { from: 1, to: 0 },
            Op::Store { file: 0, place: Place::At(100), len: 100, data: 3, child: false },
            Op::Store { file: 1, place: Place::At(block), len: 100, data: 4, child: true },
            Op::Check,
        ];
        for (index, op) in script.iter().enumerate() {
            run.step(index, op).unwrap_or_else(|failure| run.fail(0, &failure));
        }
        assert_hold(&names, &run.model);
    }

    /// A filesystem of `len` bytes, by `mkfs` and `args`, on a loop device, mounted in `dir`.
    struct Loop {
        mountpoint: std::path::PathBuf,
    }

    fn run_ok(command: &mut Command) {
        let status = command.status().unwrap_or_else(|err| panic!("{command:?}: {err}"));
        assert!(status.success(), "{command:?}: {status}");
    }

    impl Loop {
        fn new(dir: &std::path::Path, len: u64, mkfs: &str, args: &[&str]) -> Self {
            let (image, mountpoint) = (dir.join("image"), dir.join("mounted"));
            fs::File::create(&image).unwrap().set_len(len).unwrap();
            fs::create_dir(&mountpoint).unwrap();
            run_ok(Command::new(mkfs).args(args).arg(&image));
            run_ok(Command::new("mount").args(["-o", "loop"]).arg(&image).arg(&mountpoint));
            Self { mountpoint }
        }
    }

    impl Drop for Loop {
        fn drop(&mut self) {
            run_ok(Command::new("umount").arg(&self.mountpoint));
        }
    }

    /// FXDAX_SCRIPT_SEEDS asks for more seeds of the tests that run scripts, to hunt.
    fn seeds() -> u64 {
        std::env::var("FXDAX_SCRIPT_SEEDS").map_or(1, |seeds| seeds.parse().unwrap())
    }

    /// Runs `mix`'s scripts on `dir`, with `LEFT` bytes of room, and checks that some operations
    /// were refused, as a run that fills nothing has looked at nothing, each leaving the files as
    /// the model says.
    fn refuses_on(dir: &std::path::Path, mix: Mix) {
        let names = names(dir, mix);
        // Made first: there may be no room for them after.
        drop(Run::new(&names, true));
        fill(dir.join("ballast").to_str().unwrap(), LEFT);
        let mut refused = 0;
        for seed in 1..=seeds() {
            let (script, len) = numbered(mix, seed);
            let mut run = Run::new(&names, true);
            run.script(&script, len).unwrap_or_else(|failure| run.fail(seed, &failure));
            let ran = run.finish();
            assert_hold(&names, &ran.model);
            refused += ran.refused;
        }
        println!("{refused} operations refused");
        assert!(refused > 0, "no operation refused");
    }

    /// A script's files, up to 12 MiB, don't fit, and each refusal, a store's SIGBUS among them,
    /// leaves them as the model says. Not on a tmpfs, which allocates a page for a load from a
    /// hole, and a full one fails that with a SIGBUS, as Fxfs doesn't. These two need root, to
    /// mount, and mkfs.ext4 and mkfs.xfs: nothing else checks what refusals leave off a guest.
    #[test]
    fn a_filesystem_with_no_room_refuses_and_leaves_the_files_as_the_model_says() {
        let dir = TempDir::new("script").unwrap();
        let ext4 = Loop::new(dir.path(), 8 << 20, "mkfs.ext4", &["-q", "-F", "-m", "0"]);
        // With no clones: ext4 has none.
        let mix = Mix { clones: false, ..SEVERAL_FILES };
        refuses_on(&ext4.mountpoint, mix);
    }

    /// Clones, on a filesystem with reflinks, where a store to a block a clone shares takes room.
    #[test]
    fn a_filesystem_with_reflinks_and_no_room_refuses_and_leaves_the_files_as_the_model_says() {
        let dir = TempDir::new("script").unwrap();
        let xfs = Loop::new(dir.path(), 300 << 20, "mkfs.xfs", &["-q", "-f", "-m", "reflink=1"]);
        refuses_on(&xfs.mountpoint, SEVERAL_FILES);
    }

    /// On a filesystem that has room, no store dies and is taken for a refusal.
    #[test]
    fn a_store_killed_with_room_left_fails() {
        let dir = TempDir::new("script").unwrap();
        let names = names(dir.path(), ONE_FILE);
        let mut run = Run::new(&names, false);
        // A child's store past the mapping of an empty file dies of it.
        let failed = run.store_in_child(0, 0, &[1]).unwrap_err();
        assert_eq!(failed.0, kind::STORE_KILLED);
        assert_eq!(failed.1, format!("0x0 bytes of 0x1 stored, the child {:#x}", libc::SIGBUS));
    }

    /// The interpreter sees a byte changed behind the model's back, as a bug of a filesystem
    /// would leave it.
    #[test]
    fn a_byte_that_differs_from_the_model_fails_the_script() {
        let dir = TempDir::new("script").unwrap();
        let names = names(dir.path(), ONE_FILE);
        let mut run = Run::new(&names, false);
        let write = Op::Write { file: 0, place: Place::At(100), len: 1000, data: 1 };
        run.step(0, &write).unwrap();
        fs::OpenOptions::new().write(true).open(&names[0]).unwrap().write_at(&[0], 150).unwrap();
        let failure = run.step(1, &Op::Check).unwrap_err();
        assert_eq!(failure.op, Some(1));
        assert_eq!(failure.kind, kind::DIFFERS);
        assert_eq!(
            failure.what,
            "0x144c bytes read at 0x0 of file 0: 1100 bytes, differing from the model at 0x96 \
             (1100 bytes there)"
        );
        assert_eq!(Failure::parse(&failure.to_string()), Some(failure));
    }

    /// A bug of a filesystem, planted: what operations `pair` of a script do together leaves a
    /// byte of a file wrong. The run's failure names an operation, a shrink of real runs on
    /// a real filesystem finds the pair, and the drop list it arrives at replays it.
    #[test]
    fn a_planted_bug_shrinks_to_the_operations_that_set_it_off() {
        let seed = 3;
        let mix = Mix { clones: false, ..SEVERAL_FILES };
        let (script, len) = numbered(mix, seed);
        let pair = [700, 2300];
        let attempt = |drops: &str| -> Option<Failure> {
            let dir = TempDir::new("script").unwrap();
            let names = names(dir.path(), mix);
            let mut run = Run::new(&names, false);
            let kept = script::kept(drops, len).unwrap();
            let mut seen = 0;
            for index in kept {
                let (index, op) = &script[index];
                if let Err(failure) = run.step(*index, op) {
                    return Some(failure);
                }
                seen += usize::from(pair.contains(index));
                if seen == pair.len() {
                    let file = fs::OpenOptions::new().write(true).open(&names[0]).unwrap();
                    file.write_at(b"!", 0).unwrap();
                    seen = 0;
                }
            }
            run.step(len, &Op::Check).err()
        };
        let first = attempt("").expect("the bug");
        assert_eq!(first.kind, kind::DIFFERS);
        let shrunk = shrink_failure(len, &first, TRIES, attempt);
        assert_eq!(shrunk.kept, Some(pair.to_vec()));
        assert!(shrunk.tries <= TRIES, "{} tries", shrunk.tries);
    }

    #[test]
    fn the_environment_drops_a_scripts_operations() {
        // No test here sets DROP_VAR but this one, which runs in a process of its own.
        unsafe { std::env::set_var(script::DROP_VAR, "1-3000,3005") };
        let (script, len) = from_env(1, ONE_FILE);
        let whole = script::generate(1, ONE_FILE);
        assert_eq!(len, whole.len());
        let indices: Vec<usize> = script.iter().map(|(index, _)| *index).collect();
        let expected: Vec<usize> =
            (0..len).filter(|i| *i == 0 || !(1..=3000).contains(i)).collect();
        let expected: Vec<usize> = expected.into_iter().filter(|i| *i != 3005).collect();
        assert_eq!(indices, expected);
        assert_eq!(script[1].1, whole[3001]);
    }
}
