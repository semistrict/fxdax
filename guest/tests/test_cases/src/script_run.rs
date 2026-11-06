// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the interpreter of the scripts `script` makes, which does their operations on files, each
//! with a mapping of it kept across all of them, and checks them against the model (TASK-149.2).
//! Nothing here is of Fxfs: the tests below run it on the host's filesystems.
//!
//! Each step comes to an outcome, which must be the model's, and changes the files' times as it
//! does, which a run notes for `differential` to compare across filesystems (TASK-44.15).
//!
//! On a filesystem that may be `full` no operation needs to succeed, as long as it fails with
//! ENOSPC, and leaves the files as the model says, which a refused write, store, truncation or
//! fsync leaves as they were. A clone or a punch, of many steps, stands as far as it got: the model
//! takes that from what the files read, within what it could have come to
//! ([`Model::stopped_short`]). A store that finds no room is a SIGBUS, which would end the guest's
//! init: there every store is a child's. Under fault injection ([`Faults`]) the same holds of
//! ENOMEM, the operations' own calls failing their allocations.

use crate::script::{self, Failure, MOST, Mix, Model, Op, Outcome, PAGE, Reach, Step, kind};
use nix::libc;
use std::cell::Cell;
use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicUsize, Ordering};

const FICLONE: libc::c_ulong = 0x4004_9409;
/// `_IOW(0x94, 13, struct file_clone_range)`.
const FICLONERANGE: libc::c_ulong = 0x4020_940d;

/// `struct file_clone_range`.
#[repr(C)]
struct CloneRange {
    src_fd: i64,
    src_offset: u64,
    src_length: u64,
    dest_offset: u64,
}

/// Marks what the filesystem refused of an operation, among those done.
const REFUSED: &str = "-- refused:";

/// The operations of `mix`'s script of `seed`, each with its index in the whole of it, less those
/// `script::DROP_VAR` drops, and the whole script's length, which is the index of its end.
pub fn from_env(seed: u64, mix: Mix) -> (Vec<(usize, Op)>, usize) {
    let drops = std::env::var(script::DROP_VAR).unwrap_or_default();
    numbered(seed, mix, &drops)
}

/// fxdax: the operations of `mix`'s script of `seed` less those `drops` drops, as `DROP_VAR`
/// says them, as [`from_env`] has them.
pub fn numbered(seed: u64, mix: Mix, drops: &str) -> (Vec<(usize, Op)>, usize) {
    let ops = script::generate(seed, mix);
    let kept = script::kept(drops, ops.len()).expect(script::DROP_VAR);
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

/// A mapping of `len` bytes of `fd` from `offset`, shared or private, or of memory that a child
/// shares if `fd` is -1.
struct Mapping {
    bytes: *mut u8,
    len: usize,
}

impl Mapping {
    fn new(fd: libc::c_int, len: usize) -> Self {
        Self::of(fd, 0, len, libc::MAP_SHARED).expect("mmap")
    }

    fn of(fd: libc::c_int, offset: u64, len: usize, flags: libc::c_int) -> io::Result<Self> {
        let anonymous = if fd < 0 { libc::MAP_ANONYMOUS } else { 0 };
        let prot = libc::PROT_READ | libc::PROT_WRITE;
        let null = std::ptr::null_mut();
        let map = unsafe { libc::mmap(null, len, prot, flags | anonymous, fd, offset as i64) };
        if map == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { bytes: map.cast(), len })
    }

    /// fxdax: none, in place of one gone.
    fn none() -> Self {
        Self { bytes: std::ptr::null_mut(), len: 0 }
    }

    fn slice(&self, at: u64, len: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bytes.add(at as usize), len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        if self.len != 0 {
            unsafe { libc::munmap(self.bytes.cast(), self.len) };
        }
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

/// A system call's return, as a count, or the errno it set.
fn counted(ret: isize) -> io::Result<u64> {
    if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(ret as u64) }
}

fn done(ret: libc::c_int) -> io::Result<u64> {
    if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(0) }
}

/// fxdax: a file's modification and change times, as fstat has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Times {
    mtime: (i64, i64),
    ctime: (i64, i64),
}

/// fxdax: of a file, which of its times a step changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Changed {
    pub mtime: bool,
    pub ctime: bool,
}

/// fxdax: what a step came to on a filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    /// The operation, as the model resolved it.
    pub step: Step,
    pub outcome: Outcome,
    /// Whether the filesystem refused it, as one that may refuse may, leaving what it got to.
    pub refused: bool,
    /// Which times of each file it changed.
    pub changed: Vec<Changed>,
}

/// What's left of a run that passed.
pub struct Ran {
    pub model: Model,
    pub done: Vec<String>,
    pub refused: usize,
    /// fxdax: how many files read in again were DAX.
    pub dax: usize,
}

/// fxdax: what a run's filesystem is, and may do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Whether it may refuse operations for lack of room.
    pub full: bool,
    /// Whether it has no reflinks, as ext4 hasn't: a clone is then a copy of what the model says
    /// the clone takes, after the VFS's checks of a clone, which are then the model's.
    pub copies_clones: bool,
    /// Whether whole-file checks walk the files' holes and data too (SEEK_HOLE, SEEK_DATA), each a
    /// file's writeback.
    pub seeks: bool,
}

/// fxdax: the kernel's fault injection (failslab and fail_page_alloc, a debug kernel's, as
/// fxfs-guest-faults-dax sets them up: TASK-44.10) for the calls a run makes on its files alone,
/// one allocation in a hundred of theirs failing, whatever else the task does unharmed.
#[derive(Clone, Copy)]
pub struct Faults(());

impl Faults {
    const DEBUGFS: &str = "/sys/kernel/debug";
    const KINDS: [&str; 2] = ["failslab", "fail_page_alloc"];

    /// Sets the injection up, failing only the tasks that ask.
    pub fn set_up() -> Self {
        let debugfs = CString::new(Self::DEBUGFS).unwrap();
        let fstype = c"debugfs";
        let mounted = unsafe {
            libc::mount(fstype.as_ptr(), debugfs.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        let err = io::Error::last_os_error();
        assert!(mounted == 0 || err.raw_os_error() == Some(libc::EBUSY), "debugfs: {err}");
        let set = |kind: &str, name: &str, value: &str| {
            let path = format!("{}/{kind}/{name}", Self::DEBUGFS);
            fs::write(&path, value).unwrap_or_else(|err| {
                panic!("{path}: {err} (the debug kernel injects faults: FXDAX_GUEST_KERNEL_BUILD=debug)")
            });
        };
        for kind in Self::KINDS {
            set(kind, "task-filter", "Y");
            set(kind, "interval", "1");
            set(kind, "times", "-1");
            set(kind, "verbose", "0");
            set(kind, "ignore-gfp-wait", "N");
            set(kind, "probability", "1");
        }
        // Not the pages of the task's own memory (__GFP_HIGHMEM), which a call faults in.
        set("fail_page_alloc", "ignore-gfp-highmem", "Y");
        set("fail_page_alloc", "min-order", "0");
        Self(())
    }

    /// Makes `call` with this task's allocations failing as the injection says.
    fn during<T>(&self, call: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        Self::fail("1");
        let made = call();
        Self::fail("0");
        made
    }

    /// Sets whether this task's allocations fail, which writing "0" itself may, failing.
    fn fail(value: &str) {
        while let Err(err) = fs::write("/proc/thread-self/make-it-fail", value) {
            assert_eq!(err.raw_os_error(), Some(libc::ENOMEM), "make-it-fail: {err}");
        }
    }
}

/// A script's files, as a run has them.
pub struct Run {
    names: Vec<String>,
    files: Vec<fs::File>,
    maps: Vec<Mapping>,
    /// How many bytes a child has stored, in memory it shares.
    stored: Mapping,
    pub(crate) model: Model,
    options: Options,
    faults: Option<Faults>,
    /// fxdax: each file's times after the step before.
    times: Vec<Times>,
    /// Each operation done, as it was resolved, and what the filesystem refused of it.
    pub done: Vec<String>,
    /// How many it refused.
    pub refused: usize,
    /// fxdax: how many files read in again were DAX.
    pub dax: usize,
    /// fxdax: each step that changed a file, by its index and as done, and what it reached.
    touches: Vec<(usize, String, Reach)>,
    /// fxdax: the byte of which file a check found differing from the model, the first.
    differed: Cell<Option<(usize, u64)>>,
}

impl Run {
    /// A run on the files `names`, which it makes, empty.
    pub fn new(names: &[String], options: Options) -> Self {
        let files: Vec<fs::File> = names.iter().map(|name| open(name, true)).collect();
        // The mappings kept: as large as the files may be, touched within their sizes alone.
        let maps = files.iter().map(|file| Mapping::new(file.as_raw_fd(), MOST as usize)).collect();
        let stored = Mapping::new(-1, PAGE as usize);
        let model = Model::new(names.len());
        let times = files.iter().map(|file| times(file).expect("fstat")).collect();
        let names = names.to_vec();
        let (done, refused, dax, faults) = (Vec::new(), 0, 0, None);
        let (touches, differed) = (Vec::new(), Cell::new(None));
        Self {
            names,
            files,
            maps,
            stored,
            model,
            options,
            faults,
            times,
            done,
            refused,
            dax,
            touches,
            differed,
        }
    }

    /// fxdax: has the run's calls on its files fail their allocations as `faults` says, ENOMEM
    /// then a refusal.
    pub fn inject(&mut self, faults: Faults) {
        self.faults = Some(faults);
    }

    /// Runs `script`, what's left of a script of `len` operations, and checks the files as it ends.
    pub fn script(&mut self, script: &[(usize, Op)], len: usize) -> Result<(), Failure> {
        for (index, op) in script {
            self.step(*index, op)?;
        }
        self.step(len, &Op::Check).map(drop)
    }

    /// Does `op`, the script's `index`th: what it came to, which is the model's, if it fails no
    /// check.
    pub fn step(&mut self, index: usize, op: &Op) -> Result<Effect, Failure> {
        let failure = |(kind, what): Failed| Failure { op: Some(index), kind, what };
        let step = self.model.resolve(op);
        let expected = self.model.outcome(&step);
        let reach = step.reach(&self.model);
        self.done.push(step.to_string());
        let refused = self.refused;
        let outcome = self.act(&step, &expected).map_err(failure)?;
        let refused = self.refused > refused;
        if let Some(reach) = reach {
            self.touches.push((index, self.done.last().cloned().unwrap_or_default(), reach));
        }
        if !refused && outcome != expected {
            return Err(failure(unexpected(&step, outcome, expected)));
        }
        let changed = self.times_changed().map_err(failure)?;
        Ok(Effect { step, outcome, refused, changed })
    }

    /// Says `failure`, of the run of the script of `seed`, on standard output, which the host
    /// reads, and panics, with what came before it.
    pub fn fail(&self, seed: u64, failure: &Failure) -> ! {
        println!("{failure}");
        panic!("seed {seed}: {failure}; {}", self.history());
    }

    /// fxdax: what came before the operation the run is at, and what was refused, for a failure.
    pub fn history(&self) -> String {
        let last = &self.done[self.done.len().saturating_sub(24)..];
        let refused: Vec<_> = (self.done.iter().enumerate())
            .filter(|(_, step)| step.contains(REFUSED))
            .map(|(index, step)| format!("{index}: {step}"))
            .collect();
        format!(
            "the last before it:\n{}\nwhat was refused:\n{}{}",
            last.join("\n"),
            refused.join("\n"),
            self.lineage()
        )
    }

    /// fxdax: the steps that changed the byte a check found differing, the latest first, back
    /// through the clones and copies it came by to where it was written: what a failure that
    /// doesn't replay, under fault injection, has to say of how it came about.
    fn lineage(&self) -> String {
        const MOST_SAID: usize = 32;
        let Some((mut file, mut at)) = self.differed.get() else { return String::new() };
        let mut said = vec![format!("\nwhat changed byte {at:#x} of file {file}:")];
        let mut before = self.touches.len();
        while let Some(found) = self.touches[..before]
            .iter()
            .rposition(|(_, _, reach)| reach.file == file && (reach.start..reach.end).contains(&at))
        {
            let (index, done, reach) = &self.touches[found];
            said.push(format!("{index}: {done}"));
            if said.len() > MOST_SAID {
                said.push("...".to_owned());
                break;
            }
            if let Some((from, src)) = reach.source {
                (file, at) = (from, src + (at - reach.start));
                said.push(format!("  its byte {at:#x} of file {file}'s, before it:"));
            }
            before = found;
        }
        said.join("\n")
    }

    /// Ends the run, the files written back.
    pub fn finish(mut self) -> Ran {
        self.maps.clear();
        let files = std::mem::take(&mut self.files);
        for file in &files {
            if let Err(err) = file.sync_all() {
                assert!(self.refusal(&err), "the files' last fsync: {err}");
                self.refuse("fsync");
            }
        }
        Ran { model: self.model, done: self.done, refused: self.refused, dax: self.dax }
    }

    /// fxdax: makes `file` hold `bytes`, as another run's model has it after a refusal there,
    /// outside the script: written whole, its times taken anew.
    pub fn adopt(&mut self, file: usize, bytes: &[u8]) -> Result<(), Failure> {
        let failed = |err: io::Error| Failure {
            op: None,
            kind: Failure::errno_kind(err.raw_os_error()),
            what: format!("making file {file} the other run's: {err}"),
        };
        let handle = &self.files[file];
        handle
            .write_all_at(bytes, 0)
            .and_then(|()| handle.set_len(bytes.len() as u64))
            .map_err(failed)?;
        self.model.files[file] = bytes.to_vec();
        self.times[file] = times(handle).map_err(failed)?;
        Ok(())
    }

    /// Whether `err` is a refusal the filesystem may make: of room on one that may be full, of
    /// memory under fault injection.
    fn refusal(&self, err: &io::Error) -> bool {
        match err.raw_os_error() {
            Some(script::errno::ENOSPC) => self.options.full,
            Some(script::errno::ENOMEM) => self.faults.is_some(),
            _ => false,
        }
    }

    /// Notes of the operation being done that the filesystem refused `what`.
    fn refuse(&mut self, what: &str) {
        self.refused += 1;
        let step = self.done.last_mut().expect("the operation being done");
        step.push_str(&format!(" {REFUSED} {what}"));
    }

    /// Makes `call`, one of the run's calls on its files, under fault injection if the run has it.
    fn call<T>(&self, call: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        match &self.faults {
            Some(faults) => faults.during(call),
            None => call(),
        }
    }

    /// What `step`'s call failing with `err` comes to: a refusal, if the filesystem may refuse
    /// it, which leaves the file it changes as far as it got ([`Model::stopped_short`]); else the
    /// errno, which the model may say.
    fn failed(&mut self, step: &Step, what: &str, err: io::Error) -> Result<Outcome, Failed> {
        let errno = err.raw_os_error().expect("an errno");
        if !self.refusal(&err) {
            return Ok(Err(errno));
        }
        self.refuse(&format!("{what}: {err}"));
        if let Some(file) = step.changes() {
            let got = self.read(file, 0, self.length(file)?)?;
            self.model.stopped_short(step, file, &got).map_err(differs)?;
            self.check_read(file)?;
        }
        Ok(Err(errno))
    }

    fn act(&mut self, step: &Step, expected: &Outcome) -> Result<Outcome, Failed> {
        let fd = |file: usize| self.files[file].as_raw_fd();
        match step {
            Step::Write { file, at, data } => {
                let (file, fd) = (*file, fd(*file));
                let call = || {
                    counted(unsafe {
                        libc::pwrite(fd, data.as_ptr().cast(), data.len(), *at as i64)
                    })
                };
                let written = match self.call(call) {
                    Ok(written) => written as usize,
                    Err(err) => return self.failed(step, "pwrite", err),
                };
                let may_refuse = self.options.full || self.faults.is_some();
                if written < data.len() && !may_refuse {
                    let what = format!("{written:#x} bytes of {:#x} at {at:#x}", data.len());
                    return Err((kind::SHORT_WRITE.to_owned(), what));
                }
                self.model.write(file, *at, &data[..written]);
                if written < data.len() {
                    self.refuse(&format!("{written:#x} bytes written"));
                    self.check_read(file)?;
                }
                Ok(Ok(written as u64))
            }
            Step::Store { file, at, data, child } => {
                let mapped = unsafe { self.maps[*file].bytes.add(*at as usize) };
                let stored = if *child || self.options.full {
                    self.store_in_child(mapped, data)?
                } else {
                    store(mapped, data);
                    data.len()
                };
                self.model.write(*file, *at, &data[..stored]);
                Ok(Ok(stored as u64))
            }
            Step::Read { file, at, len } => {
                let (handle, mut got) = (&self.files[*file], vec![0u8; *len as usize]);
                match self.call(|| handle.read_at(&mut got, *at)) {
                    Ok(n) => got.truncate(n),
                    Err(err) => return self.failed(step, "pread", err),
                }
                self.compare(*file, *at, *len, &got, "read")?;
                Ok(Ok(got.len() as u64))
            }
            Step::Truncate { file, size } => {
                let handle = &self.files[*file];
                match self.call(|| handle.set_len(*size)) {
                    Ok(()) => self.model.truncate(*file, *size),
                    Err(err) => return self.failed(step, "truncate", err),
                }
                Ok(Ok(0))
            }
            Step::Punch { file, at, len } => {
                let mode = libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE;
                self.allocate(step, *file, mode, *at, *len)
            }
            Step::Zero { file, at, len, keep_size } => {
                let kept = if *keep_size { libc::FALLOC_FL_KEEP_SIZE } else { 0 };
                self.allocate(step, *file, libc::FALLOC_FL_ZERO_RANGE | kept, *at, *len)
            }
            Step::Allocate { file, at, len, keep_size } => {
                let kept = if *keep_size { libc::FALLOC_FL_KEEP_SIZE } else { 0 };
                self.allocate(step, *file, kept, *at, *len)
            }
            Step::Fsync { file } | Step::Fdatasync { file } => {
                let (handle, data) = (&self.files[*file], matches!(step, Step::Fdatasync { .. }));
                match self.call(|| if data { handle.sync_data() } else { handle.sync_all() }) {
                    Ok(()) => Ok(Ok(0)),
                    Err(err) => self.failed(step, "fsync", err),
                }
            }
            Step::Msync { file, at, len } => {
                let mapped = unsafe { self.maps[*file].bytes.add(*at as usize) };
                let call =
                    || done(unsafe { libc::msync(mapped.cast(), *len as usize, libc::MS_SYNC) });
                match self.call(call) {
                    Ok(_) => Ok(Ok(0)),
                    Err(err) => self.failed(step, "msync", err),
                }
            }
            Step::Clone { from, to } => self.clone_range(step, *from, 0, *to, 0, 0, expected),
            Step::CloneRange { from, src, to, dst, len } => {
                self.clone_range(step, *from, *src, *to, *dst, *len, expected)
            }
            Step::Copy { from, src, to, dst, len } => self.copy(step, *from, *src, *to, *dst, *len),
            Step::MapStore { file, at, data, private } => {
                self.map_store(*file, *at, data, *private)
            }
            Step::DirectWrite { file, at, data } => {
                let direct = self.open_direct(*file)?;
                let buffer = Mapping::of(-1, 0, data.len(), libc::MAP_PRIVATE).expect("mmap");
                unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buffer.bytes, data.len()) };
                let call = || {
                    counted(unsafe {
                        libc::pwrite(
                            direct.as_raw_fd(),
                            buffer.bytes.cast(),
                            data.len(),
                            *at as i64,
                        )
                    })
                };
                let written = match self.call(call) {
                    Ok(written) => written as usize,
                    Err(err) => return self.failed(step, "O_DIRECT pwrite", err),
                };
                self.model.write(*file, *at, &data[..written]);
                Ok(Ok(written as u64))
            }
            Step::DirectRead { file, at, len } => {
                let direct = self.open_direct(*file)?;
                let buffer = Mapping::of(-1, 0, *len as usize, libc::MAP_PRIVATE).expect("mmap");
                let call = || {
                    counted(unsafe {
                        libc::pread(
                            direct.as_raw_fd(),
                            buffer.bytes.cast(),
                            *len as usize,
                            *at as i64,
                        )
                    })
                };
                let got = match self.call(call) {
                    Ok(got) => got,
                    Err(err) => return self.failed(step, "O_DIRECT pread", err),
                };
                self.compare(
                    *file,
                    *at,
                    *len,
                    buffer.slice(0, got as usize),
                    "read with O_DIRECT",
                )?;
                Ok(Ok(got))
            }
            Step::Drop { file } => {
                let ret =
                    unsafe { libc::posix_fadvise(fd(*file), 0, 0, libc::POSIX_FADV_DONTNEED) };
                match ret {
                    0 => Ok(Ok(0)),
                    code => Err(errno("POSIX_FADV_DONTNEED", &io::Error::from_raw_os_error(code))),
                }
            }
            Step::Reopen { file } => self.reopen(*file).map(|()| Ok(0)),
            Step::Check => {
                for file in 0..self.files.len() {
                    self.check_whole(file)?;
                }
                Ok(Ok(0))
            }
        }
    }

    /// fxdax: fallocate of `mode` on `len` bytes at `at` of `file`, as the model has `step`.
    fn allocate(
        &mut self,
        step: &Step,
        file: usize,
        mode: i32,
        at: u64,
        len: u64,
    ) -> Result<Outcome, Failed> {
        let fd = self.files[file].as_raw_fd();
        match self.call(|| done(unsafe { libc::fallocate(fd, mode, at as i64, len as i64) })) {
            Ok(_) => {
                self.model.apply(step);
                Ok(Ok(0))
            }
            Err(err) => self.failed(step, "fallocate", err),
        }
    }

    /// Stores `data` at `mapped`, of a file's mapping, from a child, which inherits it: all of it,
    /// or on a filesystem with no room left, as much as the child got to store before it died.
    fn store_in_child(&mut self, mapped: *mut u8, data: &[u8]) -> Result<usize, Failed> {
        let stored = unsafe { AtomicUsize::from_ptr(self.stored.bytes.cast()) };
        stored.store(0, Ordering::Relaxed);
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork: {}", io::Error::last_os_error());
        if child == 0 {
            // No core of a death a full filesystem is allowed: a host counts one as a crash.
            unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) };
            for (i, &byte) in data.iter().enumerate() {
                unsafe { mapped.add(i).write_volatile(byte) };
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
        if self.options.full && libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGBUS
        {
            self.refuse(&format!("{stored:#x} bytes stored"));
            return Ok(stored);
        }
        let what = format!("{stored:#x} bytes of {:#x} stored, the child {status:#x}", data.len());
        Err((kind::STORE_KILLED.to_owned(), what))
    }

    /// FICLONERANGE of `len` bytes, to the source's end if 0, at `src` of `from` onto `dst` of `to`,
    /// FICLONE of all of it if `step` is a whole file's clone: whose outcome, on a filesystem
    /// without reflinks, is the model's, `expected`, the bytes it says copied.
    #[allow(clippy::too_many_arguments)]
    fn clone_range(
        &mut self,
        step: &Step,
        from: usize,
        src: u64,
        to: usize,
        dst: u64,
        len: u64,
        expected: &Outcome,
    ) -> Result<Outcome, Failed> {
        if self.options.copies_clones {
            return match *expected {
                Err(errno) => Ok(Err(errno)),
                Ok(count) => self.copy(step, from, src, to, dst, count),
            };
        }
        let (source, destination) = (self.files[from].as_raw_fd(), self.files[to].as_raw_fd());
        let cloned = match step {
            Step::Clone { .. } => {
                self.call(|| done(unsafe { libc::ioctl(destination, FICLONE as _, source) }))
            }
            _ => {
                let range = CloneRange {
                    src_fd: source as i64,
                    src_offset: src,
                    src_length: len,
                    dest_offset: dst,
                };
                self.call(|| done(unsafe { libc::ioctl(destination, FICLONERANGE as _, &range) }))
            }
        };
        match cloned {
            // What it cloned, the ioctl doesn't say: what the model said, if it said it would.
            Ok(_) => {
                self.model.apply(step);
                Ok(Ok(*expected.as_ref().unwrap_or(&0)))
            }
            Err(err) => self.failed(step, "FICLONERANGE", err),
        }
    }

    /// fxdax: copy_file_range of `len` bytes at `src` of `from` to `dst` of `to`, as many calls as
    /// it takes, each going on where the one before stopped: what they copied.
    fn copy(
        &mut self,
        step: &Step,
        from: usize,
        src: u64,
        to: usize,
        dst: u64,
        len: u64,
    ) -> Result<Outcome, Failed> {
        let (source, destination) = (self.files[from].as_raw_fd(), self.files[to].as_raw_fd());
        // As far as the source reaches as the copy starts: a copy within a file that grows it
        // would go on copying what it copied.
        let reach = len.min(self.length(from)?.saturating_sub(src));
        let (mut src, mut dst, mut copied) = (src as i64, dst as i64, 0);
        while copied == 0 || copied < reach {
            let call = || {
                // All of it the first time, for the kernel to shorten.
                let left = (if copied == 0 { len } else { reach - copied }) as usize;
                counted(unsafe {
                    let (src, dst): (*mut i64, *mut i64) = (&mut src, &mut dst);
                    libc::syscall(
                        libc::SYS_copy_file_range,
                        source,
                        src,
                        destination,
                        dst,
                        left,
                        0u32,
                    )
                } as isize)
            };
            let failed = match self.call(call) {
                Ok(0) => break,
                Ok(n) => {
                    copied += n;
                    continue;
                }
                Err(err) => err,
            };
            if copied == 0 {
                return self.failed(step, "copy_file_range", failed);
            }
            // A call failing after others copied leaves what they did, a refusal at most.
            self.copied(from, src as u64, to, dst as u64, copied);
            if !self.refusal(&failed) {
                return Err(errno(&format!("copy_file_range after {copied:#x} bytes"), &failed));
            }
            self.refuse(&format!("{copied:#x} bytes copied: {failed}"));
            self.check_read(to)?;
            return Ok(Ok(copied));
        }
        self.copied(from, src as u64, to, dst as u64, copied);
        Ok(Ok(copied))
    }

    /// fxdax: has the model hold what a copy that has got to `src` of `from` and `dst` of `to`
    /// copied, the `copied` bytes before them, read before any was written: a file and itself
    /// never overlap.
    fn copied(&mut self, from: usize, src: u64, to: usize, dst: u64, copied: u64) {
        if copied == 0 {
            return;
        }
        let bytes = self.model.files[from][(src - copied) as usize..src as usize].to_vec();
        self.model.write(to, dst - copied, &bytes);
    }

    /// fxdax: stores `data` at `at` of `file` through a mapping of its pages made for them, gone
    /// after: shared, and the file holds them, or private, and only the mapping does, which reads
    /// as the file with them.
    fn map_store(
        &mut self,
        file: usize,
        at: u64,
        data: &[u8],
        private: bool,
    ) -> Result<Outcome, Failed> {
        if data.is_empty() {
            return Ok(Ok(0));
        }
        let first = at / PAGE * PAGE;
        let len = (at - first) as usize + data.len();
        let flags = if private { libc::MAP_PRIVATE } else { libc::MAP_SHARED };
        let fd = self.files[file].as_raw_fd();
        let mapping = Mapping::of(fd, first, len, flags).map_err(|err| errno("mmap", &err))?;
        let mapped = unsafe { mapping.bytes.add((at - first) as usize) };
        let stored = if self.options.full && !private {
            self.store_in_child(mapped, data)?
        } else {
            store(mapped, data);
            data.len()
        };
        if private {
            let mut want = self.model.files[file][first as usize..first as usize + len].to_vec();
            want[(at - first) as usize..].copy_from_slice(data);
            if let Some(off) = mapping.slice(0, len).iter().zip(&want).position(|(a, b)| a != b) {
                let at = first + off as u64;
                self.differs_at(file, at);
                return Err(differs(format!("file {file}'s private mapping differs at {at:#x}")));
            }
        } else {
            self.model.write(file, at, &data[..stored]);
        }
        Ok(Ok(stored as u64))
    }

    /// fxdax: `file` opened again, with O_DIRECT.
    fn open_direct(&self, file: usize) -> Result<fs::File, Failed> {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).custom_flags(libc::O_DIRECT);
        options.open(&self.names[file]).map_err(|err| errno("open with O_DIRECT", &err))
    }

    /// fxdax: closes `file`, its kept mapping gone, evicts its inode, written back and its pages
    /// dropped, and opens and maps it again; on a DAX mount, notes whether it is DAX now.
    fn reopen(&mut self, file: usize) -> Result<(), Failed> {
        drop(std::mem::replace(&mut self.maps[file], Mapping::none()));
        // Its descriptor goes before its inode can: one of /dev/null holds its place.
        let null = fs::File::open("/dev/null").map_err(|err| errno("open /dev/null", &err))?;
        let handle = std::mem::replace(&mut self.files[file], null);
        if let Err(err) = handle.sync_all() {
            if !self.refusal(&err) {
                return Err(errno("fsync before eviction", &err));
            }
            self.refuse(&format!("fsync before eviction: {err}"));
        }
        let fd = handle.as_raw_fd();
        assert_eq!(unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED) }, 0);
        drop(handle);
        fs::write("/proc/sys/vm/drop_caches", b"2").map_err(|err| errno("drop_caches", &err))?;
        self.files[file] = open(&self.names[file], false);
        self.maps[file] = Mapping::new(self.files[file].as_raw_fd(), MOST as usize);
        self.dax += usize::from(is_dax(&self.files[file]));
        Ok(())
    }

    /// fxdax: which times of each file the step just done changed.
    fn times_changed(&mut self) -> Result<Vec<Changed>, Failed> {
        let mut changed = Vec::with_capacity(self.files.len());
        for (file, before) in self.files.iter().zip(&mut self.times) {
            let now = times(file).map_err(|err| errno("fstat", &err))?;
            changed.push(Changed {
                mtime: now.mtime != before.mtime,
                ctime: now.ctime != before.ctime,
            });
            *before = now;
        }
        Ok(changed)
    }

    /// fxdax: notes that byte `at` of `file` differs from the model, if no byte did before.
    fn differs_at(&self, file: usize, at: u64) {
        if self.differed.get().is_none() {
            self.differed.set(Some((file, at)));
        }
    }

    fn length(&self, file: usize) -> Result<u64, Failed> {
        let metadata = self.files[file].metadata();
        Ok(metadata.map_err(|err| errno("fstat", &err))?.len())
    }

    /// What read(2) gives of `len` bytes at `at` of `file`: a check's read, never one fault
    /// injection fails, as a read the script does may (`Step::Read`).
    fn read(&self, file: usize, at: u64, len: u64) -> Result<Vec<u8>, Failed> {
        let mut got = vec![0u8; len as usize];
        let n = self.files[file].read_at(&mut got, at);
        got.truncate(n.map_err(|err| errno(&format!("pread at {at:#x}"), &err))?);
        Ok(got)
    }

    /// Checks `len` bytes at `at` of `file`, through read(2), against the model: how many it read.
    fn check(&self, file: usize, at: u64, len: u64) -> Result<u64, Failed> {
        let got = self.read(file, at, len)?;
        self.compare(file, at, len, &got, "read")?;
        Ok(got.len() as u64)
    }

    /// Checks `got`, what a read of `len` bytes at `at` of `file` gave, against the model.
    fn compare(&self, file: usize, at: u64, len: u64, got: &[u8], how: &str) -> Result<(), Failed> {
        let model = &self.model.files[file];
        let end = (at as usize + len as usize).min(model.len()).max(at as usize);
        let want = model.get(at as usize..end).unwrap_or(&[]);
        if got == want {
            return Ok(());
        }
        let first =
            (got.iter().zip(want).position(|(a, b)| a != b)).unwrap_or(got.len().min(want.len()));
        self.differs_at(file, at + first as u64);
        Err(differs(format!(
            "{len:#x} bytes {how} at {at:#x} of file {file}: {} bytes, differing from the model at \
             {:#x} ({} bytes there)",
            got.len(),
            at as usize + first,
            want.len()
        )))
    }

    /// Checks `file`'s size, and all of it through read(2), against the model, and again uncached
    /// if the filesystem may refuse: at once after an operation refused, which may leave no other
    /// trace, and which a check 100 operations later says nothing of.
    fn check_read(&self, file: usize) -> Result<(), Failed> {
        let model = &self.model.files[file];
        let size = self.length(file)?;
        if size != model.len() as u64 {
            let want = model.len();
            return Err(differs(format!("file {file} has size {size:#x}, the model's {want:#x}")));
        }
        self.check(file, 0, size + PAGE)?;
        if !self.options.full && self.faults.is_none() {
            return Ok(());
        }
        // As the filesystem has it: a refusal can leave a cache saying what it never stored.
        let fd = self.files[file].as_raw_fd();
        assert_eq!(unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED) }, 0);
        self.check(file, 0, size + PAGE).map(drop)
    }

    /// Checks all of `file`, its size, and through read(2) and its mapping, against the model; and
    /// fxdax: if the run seeks, that each hole SEEK_HOLE and SEEK_DATA find reads as zeros.
    fn check_whole(&self, file: usize) -> Result<(), Failed> {
        self.check_read(file)?;
        let model = &self.model.files[file];
        let through = self.maps[file].slice(0, model.len());
        if let Some(at) = through.iter().zip(model).position(|(a, b)| a != b) {
            self.differs_at(file, at as u64);
            return Err(differs(format!("file {file}'s mapping differs at {at:#x}")));
        }
        if self.options.seeks {
            self.check_holes(file)?;
        }
        Ok(())
    }

    /// fxdax: walks `file`'s holes and data, SEEK_DATA from each hole's start and SEEK_HOLE from
    /// each run of data's: what the filesystem says is a hole must be zeros in the model.
    fn check_holes(&self, file: usize) -> Result<(), Failed> {
        let (fd, model) = (self.files[file].as_raw_fd(), &self.model.files[file]);
        let size = model.len() as i64;
        let seek = |at: i64, whence| {
            let found = unsafe { libc::lseek(fd, at, whence) };
            if found >= 0 {
                return Ok(Some(found));
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::ENXIO) => Ok(None),
                _ => Err(errno(&format!("lseek {whence} from {at:#x}"), &err)),
            }
        };
        let mut at = 0;
        while at < size {
            let data = seek(at, libc::SEEK_DATA)?.unwrap_or(size).min(size);
            if let Some(off) = model[at as usize..data as usize].iter().position(|&byte| byte != 0)
            {
                let hole = format!("{at:#x}..{data:#x}");
                let off = at as usize + off;
                return Err(differs(format!(
                    "file {file} has a hole at {hole}, holding data at {off:#x}"
                )));
            }
            if data == size {
                break;
            }
            let hole = seek(data, libc::SEEK_HOLE)?.unwrap_or(size);
            if hole <= data {
                return Err(differs(format!("file {file}'s data at {data:#x} ends at {hole:#x}")));
            }
            at = hole;
        }
        Ok(())
    }
}

/// fxdax: what failing to come to `expected`, coming to `outcome`, says of `step`.
fn unexpected(step: &Step, outcome: Outcome, expected: Outcome) -> Failed {
    match (outcome, expected) {
        (Err(errno), _) => {
            let err = io::Error::from_raw_os_error(errno);
            let model = match expected {
                Ok(_) => String::new(),
                Err(errno) => format!(", where the model says errno {errno}"),
            };
            (Failure::errno_kind(Some(errno)), format!("{step}: {err}{model}"))
        }
        (Ok(_), Err(errno)) => {
            differs(format!("{step} wasn't refused, as the model says it is ({errno})"))
        }
        (Ok(did), Ok(want)) => differs(format!("{step} came to {did:#x}, the model's {want:#x}")),
    }
}

/// Opens `name`, read and written, made empty if `empty`.
fn open(name: &str, empty: bool) -> fs::File {
    let mut options = fs::OpenOptions::new();
    options.create(empty).truncate(empty).read(true).write(true);
    options.open(name).unwrap_or_else(|err| panic!("open {name}: {err}"))
}

/// Stores `data` at `mapped`.
fn store(mapped: *mut u8, data: &[u8]) {
    for (i, &byte) in data.iter().enumerate() {
        unsafe { mapped.add(i).write_volatile(byte) };
    }
}

/// fxdax: `file`'s times.
fn times(file: &fs::File) -> io::Result<Times> {
    let metadata = file.metadata()?;
    Ok(Times {
        mtime: (metadata.mtime(), metadata.mtime_nsec()),
        ctime: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

/// fxdax: whether the kernel accesses `file` through a DAX window: STATX_ATTR_DAX.
fn is_dax(file: &fs::File) -> bool {
    const STATX_ATTR_DAX: u64 = 0x200000;
    // struct statx: stx_attributes at byte 8.
    let mut buf = [0u64; 32];
    let fd = file.as_raw_fd();
    let ret = unsafe {
        libc::syscall(libc::SYS_statx, fd, c"".as_ptr(), libc::AT_EMPTY_PATH, 0, buf.as_mut_ptr())
    };
    assert_eq!(ret, 0, "statx: {}", io::Error::last_os_error());
    buf[1] & STATX_ATTR_DAX != 0
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::script::{ONE_FILE, Place, SEVERAL_FILES, TRIES, WIDE, shrink_failure};
    use std::process::Command;
    use tempdir::TempDir;

    /// The room left on a filesystem full enough: a script's files, full of holes, come to less
    /// than a MiB and a half on average, and 2 MiB left saw few refusals. 1 MiB left saw none on
    /// XFS once a check after a refusal dropped the cache, whose writeback frees what XFS
    /// reserved ahead.
    const LEFT: u64 = 1 << 18;

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
        let mut run = Run::new(&names, Options::default());
        run.script(&script, len).unwrap_or_else(|failure| run.fail(1, &failure));
        let ran = run.finish();
        assert_eq!(ran.done.len(), len + 1);
        assert_eq!(ran.refused, 0);
        assert_hold(&names, &ran.model);
    }

    #[test]
    fn several_files_script_passes_on_a_filesystem_with_reflinks() {
        let dir = TempDir::new("script").unwrap();
        let xfs = Loop::reflinks(dir.path());
        let names = names(&xfs.mountpoint, SEVERAL_FILES);
        for seed in 1..=seeds() {
            let (script, len) = numbered(SEVERAL_FILES, seed);
            let mut run = Run::new(&names, Options::default());
            run.script(&script, len).unwrap_or_else(|failure| run.fail(seed, &failure));
            let ran = run.finish();
            assert_eq!(ran.refused, 0);
            assert_hold(&names, &ran.model);
        }
    }

    /// fxdax: the wide mix's scripts on XFS, whose reflinks hold the model's rules of a range's
    /// clone (`Model::clone_range`) to the VFS's: each refusal the model says, the kernel makes, and
    /// no other. Through every operation's outcome and the bytes after it.
    #[test]
    fn wide_scripts_pass_on_a_filesystem_with_reflinks() {
        let dir = TempDir::new("script").unwrap();
        let xfs = Loop::reflinks(dir.path());
        let names = names(&xfs.mountpoint, WIDE);
        for seed in 1..=seeds() {
            let (script, len) = numbered(WIDE, seed);
            let options = Options { seeks: true, ..Options::default() };
            let mut run = Run::new(&names, options);
            run.script(&script, len).unwrap_or_else(|failure| run.fail(seed, &failure));
            let ran = run.finish();
            assert_eq!(ran.refused, 0);
            assert_hold(&names, &ran.model);
        }
    }

    /// fxdax: and on ext4, with no reflinks, its clones copies of what the model says they take.
    #[test]
    fn wide_scripts_pass_on_a_filesystem_without_reflinks() {
        let dir = TempDir::new("script").unwrap();
        let ext4 = Loop::new(dir.path(), 300 << 20, "mkfs.ext4", &["-q", "-F"]);
        let names = names(&ext4.mountpoint, WIDE);
        let (script, len) = numbered(WIDE, 1);
        let options = Options { copies_clones: true, seeks: true, ..Options::default() };
        let mut run = Run::new(&names, options);
        run.script(&script, len).unwrap_or_else(|failure| run.fail(1, &failure));
        let ran = run.finish();
        assert_eq!(ran.dax, 0);
        assert_hold(&names, &ran.model);
    }

    /// fxdax: each operation's outcome, which a step's effect says, is the model's: a range clone
    /// refused as the VFS refuses it, a copy shortened at the source's end; and a refusal and a
    /// read change no file's times. That a write changes them a host's kernel may not show: before
    /// 6.13, its filesystems' times are coarse, and a write in the tick the file was made in leaves
    /// them (the guest's ext4 and Fxfs are multigrain, FS_MGTIME).
    #[test]
    fn a_steps_effect_says_its_outcome_and_the_times_it_changed() {
        let dir = TempDir::new("script").unwrap();
        let xfs = Loop::reflinks(dir.path());
        let names = names(&xfs.mountpoint, WIDE);
        let mut run = Run::new(&names, Options::default());
        let ops = [
            Op::Write { file: 0, place: Place::At(0), len: 3 * PAGE + 7, data: 1 },
            Op::CloneRange {
                from: 0,
                from_place: Place::At(1),
                to: 1,
                to_place: Place::At(0),
                len: PAGE,
                aligned: false,
            },
            Op::CloneRange {
                from: 0,
                from_place: Place::At(0),
                to: 1,
                to_place: Place::At(PAGE),
                len: 0,
                aligned: true,
            },
            Op::Copy {
                from: 0,
                from_place: Place::At(3 * PAGE),
                to: 2,
                to_place: Place::At(5),
                len: 100,
            },
            Op::Read { file: 1, place: Place::At(0), len: 100 },
        ];
        let effects: Vec<Effect> = (ops.iter().enumerate())
            .map(|(index, op)| run.step(index, op).unwrap_or_else(|failure| run.fail(0, &failure)))
            .collect();
        let outcomes: Vec<Outcome> = effects.iter().map(|effect| effect.outcome).collect();
        assert_eq!(
            outcomes,
            [Ok(3 * PAGE + 7), Err(script::errno::EINVAL), Ok(3 * PAGE + 7), Ok(7), Ok(100)]
        );
        let unchanged = Changed::default();
        assert_eq!(effects[0].changed[1..], [unchanged; 2]);
        assert_eq!(effects[1].changed, [unchanged; 3]);
        assert_eq!(effects[4].changed, [unchanged; 3]);
        assert_eq!(run.model.files[1].len() as u64, PAGE + 3 * PAGE + 7);
        assert_hold(&names, &run.model);
    }

    /// fxdax: a private mapping's stores are its own, the file's as they were; a file reopened is
    /// as it was, and its times too.
    #[test]
    fn private_stores_and_reopens_leave_the_file() {
        let dir = TempDir::new("script").unwrap();
        let ext4 = Loop::new(dir.path(), 300 << 20, "mkfs.ext4", &["-q", "-F"]);
        let names = names(&ext4.mountpoint, WIDE);
        let mut run = Run::new(&names, Options::default());
        let ops = [
            Op::Write { file: 0, place: Place::At(0), len: 2 * PAGE, data: 1 },
            Op::MapStore { file: 0, place: Place::At(PAGE + 5), len: 10, data: 2, private: true },
            Op::Reopen { file: 0 },
            Op::MapStore { file: 0, place: Place::At(5), len: 10, data: 3, private: false },
            Op::Reopen { file: 0 },
            Op::Check,
        ];
        let mut effects = Vec::new();
        for (index, op) in ops.iter().enumerate() {
            effects.push(run.step(index, op).unwrap_or_else(|failure| run.fail(0, &failure)));
        }
        assert_eq!(effects[1].changed[0], Changed::default());
        assert_eq!(effects[2].changed[0], Changed::default());
        assert_eq!(effects[4].changed[0], Changed::default());
        let written = script::Rng::new(1).bytes(2 * PAGE as usize);
        let stored = script::Rng::new(3).bytes(10);
        let held = [&written[..5], &stored[..], &written[15..]].concat();
        assert_eq!(run.model.files[0], held);
        assert_hold(&names, &run.model);
    }

    /// fxdax: a byte found differing says what changed it, back through the clone it came by.
    #[test]
    fn a_byte_that_differs_says_what_changed_it() {
        let dir = TempDir::new("script").unwrap();
        let xfs = Loop::reflinks(dir.path());
        let names = names(&xfs.mountpoint, WIDE);
        let mut run = Run::new(&names, Options::default());
        let ops = [
            Op::Write { file: 1, place: Place::At(0), len: 2 * PAGE, data: 1 },
            Op::Write { file: 0, place: Place::At(0), len: 100, data: 2 },
            Op::Clone { from: 1, to: 0 },
            Op::Truncate { file: 2, size: 10 },
        ];
        for (index, op) in ops.iter().enumerate() {
            run.step(index, op).unwrap_or_else(|failure| run.fail(0, &failure));
        }
        fs::OpenOptions::new().write(true).open(&names[0]).unwrap().write_at(&[0], 50).unwrap();
        let failure = run.step(4, &Op::Check).unwrap_err();
        assert_eq!(failure.kind, kind::DIFFERS);
        assert_eq!(
            run.lineage(),
            "\nwhat changed byte 0x32 of file 0:\n2: clone file 1 onto file 0\n  its byte 0x32 of \
             file 1's, before it:\n0: write 0x2000 at 0x0 of file 1"
        );
    }

    /// fxdax: a hole SEEK_DATA skips must read as zeros: the model holding data there fails.
    #[test]
    fn a_hole_over_data_fails() {
        let dir = TempDir::new("script").unwrap();
        let ext4 = Loop::new(dir.path(), 300 << 20, "mkfs.ext4", &["-q", "-F"]);
        let names = names(&ext4.mountpoint, WIDE);
        let mut run = Run::new(&names, Options { seeks: true, ..Options::default() });
        run.step(0, &Op::Truncate { file: 0, size: 64 * PAGE }).unwrap();
        run.step(1, &Op::Check).unwrap();
        run.model.files[0][40 * PAGE as usize] = 1;
        let failed = run.check_holes(0).unwrap_err();
        assert_eq!(failed.0, kind::DIFFERS);
        assert_eq!(failed.1, "file 0 has a hole at 0x0..0x40000, holding data at 0x28000");
    }

    /// A clone onto a longer file: what the VFS says, and the model.
    #[test]
    fn clones_do_as_the_model_says() {
        let dir = TempDir::new("script").unwrap();
        let xfs = Loop::reflinks(dir.path());
        let mix = Mix { files: 2, ..ONE_FILE };
        let names = names(&xfs.mountpoint, mix);
        let mut run = Run::new(&names, Options::default());
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
    pub(crate) struct Loop {
        pub(crate) mountpoint: std::path::PathBuf,
    }

    fn run_ok(command: &mut Command) {
        let status = command.status().unwrap_or_else(|err| panic!("{command:?}: {err}"));
        assert!(status.success(), "{command:?}: {status}");
    }

    impl Loop {
        pub(crate) fn new(dir: &std::path::Path, len: u64, mkfs: &str, args: &[&str]) -> Self {
            let (image, mountpoint) = (dir.join("image"), dir.join("mounted"));
            fs::File::create(&image).unwrap().set_len(len).unwrap();
            fs::create_dir(&mountpoint).unwrap();
            run_ok(Command::new(mkfs).args(args).arg(&image));
            run_ok(Command::new("mount").args(["-o", "loop"]).arg(&image).arg(&mountpoint));
            Self { mountpoint }
        }

        /// XFS with reflinks, which a host's temporary directory may not have: its ext4 says
        /// EOPNOTSUPP to FICLONE.
        fn reflinks(dir: &std::path::Path) -> Self {
            Self::new(dir, 300 << 20, "mkfs.xfs", &["-q", "-f", "-m", "reflink=1"])
        }
    }

    impl Drop for Loop {
        /// Unmounts it lazily: the test has checked all it will, and whatever else holds the mount
        /// a moment longer, as a host's probe of the new loop device may, can't fail it. A plain
        /// umount said "target is busy" on GCE hosts, after the test's own files and children had
        /// gone (TASK-160.27's runs of these tests).
        fn drop(&mut self) {
            run_ok(Command::new("umount").arg("--lazy").arg(&self.mountpoint));
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
        drop(Run::new(&names, Options { full: true, ..Options::default() }));
        fill(dir.join("ballast").to_str().unwrap(), LEFT);
        let mut refused = 0;
        for seed in 1..=seeds() {
            let (script, len) = numbered(mix, seed);
            let mut run = Run::new(&names, Options { full: true, ..Options::default() });
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
    /// hole, and a full one fails that with a SIGBUS, as Fxfs doesn't. The tests on a [`Loop`] need
    /// root, to mount, and mkfs.ext4 or mkfs.xfs: nothing else checks what refusals leave off a
    /// guest.
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
        let xfs = Loop::reflinks(dir.path());
        refuses_on(&xfs.mountpoint, SEVERAL_FILES);
    }

    /// On a filesystem that has room, no store dies and is taken for a refusal.
    #[test]
    fn a_store_killed_with_room_left_fails() {
        let dir = TempDir::new("script").unwrap();
        let names = names(dir.path(), ONE_FILE);
        let mut run = Run::new(&names, Options::default());
        // A child's store past the mapping of an empty file dies of it.
        let mapped = run.maps[0].bytes;
        let failed = run.store_in_child(mapped, &[1]).unwrap_err();
        assert_eq!(failed.0, kind::STORE_KILLED);
        assert_eq!(failed.1, format!("0x0 bytes of 0x1 stored, the child {:#x}", libc::SIGBUS));
    }

    /// The interpreter sees a byte changed behind the model's back, as a bug of a filesystem
    /// would leave it.
    #[test]
    fn a_byte_that_differs_from_the_model_fails_the_script() {
        let dir = TempDir::new("script").unwrap();
        let names = names(dir.path(), ONE_FILE);
        let mut run = Run::new(&names, Options::default());
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
            let mut run = Run::new(&names, Options::default());
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
