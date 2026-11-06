// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the random checks, of a script's operations on a file (random_ops) and of threads' on
//! several (torture), against models of their bytes.

use super::*;

/// fxdax: the seed of the run's random checks (FXDAX_RANDOM_OPS_SEED, which the host picks
/// anew each run, or 1).
pub(crate) fn seed() -> u64 {
    std::env::var(script::SEED_VAR).ok().and_then(|s| s.parse().ok()).unwrap_or(1)
}

/// fxdax: a file's random operations, as fsx makes them (TASK-44.15): write(2)s, appends,
/// stores through a mapping, reads, truncations, holes punched and fsyncs, at random offsets
/// and lengths, a script made from a seed (FXDAX_RANDOM_OPS_SEED, which the host picks anew
/// each run, or 1) and run by `script_run` (TASK-149.2), each read, and every 256th operation
/// and the last the whole file, through read(2) and the mapping, checked against what a model
/// of its bytes says it holds. A mismatch names the seed, the operation and those before it,
/// and the host shrinks the script. The file stays, and the model beside it, which the host
/// compares it with, as its Fxfs reads them from the store (TASK-119.2), and the operations
/// made, its history (TASK-119.15). fxdax: the file reaches across two 2 MiB chunks, the
/// offsets drawn near a chunk's end, a page's, and the file's as often as anywhere, and one
/// mapping of it is kept across every operation, seals and truncations among them, which its
/// stores and reads go through (TASK-119.15).
pub(super) fn test_random_ops() {
    let seed = seed();
    let name = path(RANDOM_OPS);
    let (script, len) = script_run::from_env(seed, script::ONE_FILE);
    let history = |done: &[String]| {
        fs::write(path(RANDOM_OPS_HISTORY), format!("seed {seed}\n{}\n", done.join("\n"))).unwrap();
        fs::File::open(path(RANDOM_OPS_HISTORY)).unwrap().sync_all().unwrap();
    };
    let mut run = Run::new(std::slice::from_ref(&name), script_run::Options::default());
    if let Err(failure) = run.script(&script, len) {
        history(&run.done);
        run.fail(seed, &failure);
    }
    let ran = run.finish();
    history(&ran.done);
    let model = &ran.model.files[0];
    fs::write(path(RANDOM_OPS_MODEL), model).unwrap();
    fs::File::open(path(RANDOM_OPS_MODEL)).unwrap().sync_all().unwrap();
    evict(&name);
    assert!(fs::read(&name).unwrap() == *model, "seed {seed}: the file evicted differs");
}

/// fxdax: several threads' random operations on several small files at once (TASK-119.5):
/// write(2)s, stores through mappings kept across them, reads, fdatasyncs and fsyncs, msyncs,
/// while another thread fsyncs another file and syncs the filesystem, writing back and sealing
/// again and again under one another's stores.
/// The threads share every page, each byte owned by one, which checks it as
/// its own model says after each of its writes, and now and then, through read(2) and its
/// mapping: whatever the others do to the page, a byte holds what its owner left there. Between
/// phases, the threads gone, a file is truncated and extended again, has a hole punched, or is
/// evicted, and every file checked whole. A mismatch names the seed, the phase, the thread, the
/// operation and those before it. The files stay, with their models, which the host compares
/// them with, as its Fxfs reads them from the store.
pub(super) fn test_torture() {
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    const PAGE: usize = 4096;
    const PAGES: usize = 64;
    const SIZE: usize = PAGES * PAGE;
    /// The pages before it are shared by the threads 64 bytes at a time, those after it a page
    /// at a time.
    const SHARED: usize = PAGES / 2 * PAGE;
    const SLOT: usize = 64;
    const THREADS: usize = 3;
    const PHASES: usize = 24;
    const OPS: usize = 300;
    let seed = seed();
    let names: Vec<String> = (0..TORTURE_FILES).map(|n| path(&torture(n))).collect();
    let models: Vec<Vec<AtomicU8>> =
        (0..TORTURE_FILES).map(|_| (0..SIZE).map(|_| AtomicU8::new(0)).collect()).collect();
    for name in &names {
        let file = fs::File::create(name).unwrap();
        file.set_len(SIZE as u64).unwrap();
        file.sync_all().unwrap();
    }
    // Which thread owns the byte at `at`.
    let owner = |at: usize| if at < SHARED { at / SLOT } else { (at - SHARED) / PAGE } % THREADS;
    // A run of bytes `thread` owns, at random: in a slot of the shared pages, or a page.
    let owned = |rng: &mut Rng, thread: usize| -> (usize, usize) {
        if rng.next(2) == 0 {
            let slots = SHARED / SLOT;
            let slot =
                thread + THREADS * rng.next(((slots - thread).div_ceil(THREADS)) as u64) as usize;
            let start = rng.next(SLOT as u64) as usize;
            (slot * SLOT + start, 1 + rng.next((SLOT - start) as u64) as usize)
        } else {
            let pages = PAGES / 2;
            let page =
                thread + THREADS * rng.next(((pages - thread).div_ceil(THREADS)) as u64) as usize;
            (SHARED + page * PAGE, PAGE)
        }
    };
    let mut rngs: Vec<Rng> =
        (0..THREADS).map(|thread| Rng::new(seed ^ (thread as u64 + 1) << 56)).collect();
    let mut logs: Vec<Vec<String>> = vec![Vec::new(); THREADS];
    let mut coordinator = Rng::new(seed);
    let sealing = AtomicBool::new(true);
    // Every file, read whole, as its model says.
    let check_all = |phase: usize, what: &str| {
        for (n, name) in names.iter().enumerate() {
            let read = fs::read(name).unwrap();
            assert_eq!(read.len(), SIZE, "seed {seed}, phase {phase}, {what}: {name}'s size");
            if let Some(at) =
                (0..SIZE).find(|&at| read[at] != models[n][at].load(Ordering::Relaxed))
            {
                trace_dump(name, at as u64 / 4096);
                panic!(
                    "seed {seed}, phase {phase}, {what}: {name} at {at:#x} (thread {}'s) reads {:#x}, its model {:#x}",
                    owner(at),
                    read[at],
                    models[n][at].load(Ordering::Relaxed)
                );
            }
        }
    };
    std::thread::scope(|outer| {
        // Seals, again and again: another file's fsyncs, and the filesystem's syncs.
        outer.spawn(|| {
            let other = path("torture-other");
            let mut n = 0usize;
            while sealing.load(Ordering::Relaxed) {
                fs::write(&other, [n as u8; 100]).unwrap();
                if n % 8 == 7 {
                    assert_eq!(
                        unsafe { libc::syncfs(fs::File::open(&other).unwrap().as_raw_fd()) },
                        0
                    );
                } else {
                    fs::File::open(&other).unwrap().sync_all().unwrap();
                }
                n += 1;
            }
            fs::remove_file(&other).unwrap();
        });
        let phases = catch_unwind(AssertUnwindSafe(|| {
            for phase in 0..PHASES {
                std::thread::scope(|scope| {
                    let threads: Vec<_> = (rngs.iter_mut().zip(logs.iter_mut()).enumerate())
                        .map(|(thread, (rng, log))| {
                            let (names, models) = (&names, &models);
                            scope.spawn(move || {
                                torture_phase(
                                    seed, phase, thread, rng, log, names, models, &owned, OPS,
                                )
                            })
                        })
                        .collect();
                    for thread in threads {
                        if let Err(panic) = thread.join() {
                            std::panic::resume_unwind(panic);
                        }
                    }
                });
                check_all(phase, "after the threads");
                // Between phases, to a file: truncated and extended again, a hole punched,
                // evicted, or nothing.
                let n = coordinator.next(TORTURE_FILES as u64) as usize;
                let (name, model) = (&names[n], &models[n]);
                let what = match coordinator.next(4) {
                    0 => {
                        let size = coordinator.next(SIZE as u64) as usize;
                        let file = fs::OpenOptions::new().write(true).open(name).unwrap();
                        file.set_len(size as u64).unwrap();
                        file.set_len(SIZE as u64).unwrap();
                        model[size..].iter().for_each(|byte| byte.store(0, Ordering::Relaxed));
                        format!("{name} truncated to {size:#x} and extended")
                    }
                    1 => {
                        let at = coordinator.next(SIZE as u64) as usize;
                        let len = 1 + coordinator.next((SIZE - at) as u64) as usize;
                        let file = fs::OpenOptions::new().write(true).open(name).unwrap();
                        let punch = libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE;
                        let punched = unsafe {
                            libc::fallocate(file.as_raw_fd(), punch, at as i64, len as i64)
                        };
                        assert_eq!(punched, 0, "punch: {}", std::io::Error::last_os_error());
                        model[at..at + len]
                            .iter()
                            .for_each(|byte| byte.store(0, Ordering::Relaxed));
                        format!("{name} punched {len:#x} at {at:#x}")
                    }
                    2 => {
                        evict(name);
                        format!("{name} evicted")
                    }
                    _ => "nothing".to_owned(),
                };
                for log in &mut logs {
                    log.push(format!("between phases: {what}"));
                }
                check_all(phase, &what);
            }
        }));
        sealing.store(false, Ordering::Relaxed);
        if let Err(panic) = phases {
            std::panic::resume_unwind(panic);
        }
    });
    for (n, name) in names.iter().enumerate() {
        let model: Vec<u8> = models[n].iter().map(|byte| byte.load(Ordering::Relaxed)).collect();
        fs::write(format!("{name}.model"), &model).unwrap();
        fs::File::open(format!("{name}.model")).unwrap().sync_all().unwrap();
        fs::File::open(name).unwrap().sync_all().unwrap();
    }
}

/// fxdax: a phase of test_torture's thread `thread`: `ops` random operations on the bytes it
/// owns (`owned`) of the files `names`, as `models` has them, through write(2), and stores
/// through mappings of the files it keeps for the phase, checked through both; and fdatasyncs,
/// fsyncs and msyncs. `log` keeps what it did, for a mismatch to say.
#[allow(clippy::too_many_arguments)]
fn torture_phase(
    seed: u64,
    phase: usize,
    thread: usize,
    rng: &mut Rng,
    log: &mut Vec<String>,
    names: &[String],
    models: &[Vec<std::sync::atomic::AtomicU8>],
    owned: &dyn Fn(&mut Rng, usize) -> (usize, usize),
    ops: usize,
) {
    use std::os::unix::fs::FileExt;
    use std::sync::atomic::Ordering;
    let size = models[0].len();
    let files: Vec<fs::File> = (names.iter())
        .map(|name| fs::OpenOptions::new().read(true).write(true).open(name).unwrap())
        .collect();
    let maps: Vec<usize> = (files.iter())
        .map(|file| {
            let map = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    file.as_raw_fd(),
                    0,
                )
            };
            assert_ne!(map, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
            map as usize
        })
        .collect();
    let failed = |log: &[String], what: String| -> ! {
        let last = &log[log.len().saturating_sub(32)..];
        panic!(
            "seed {seed}, phase {phase}, thread {thread}, operation {}: {what}; the last before it:\n{}",
            log.len(),
            last.join("\n")
        );
    };
    // The thread's bytes `len` from `at` of file `n`, through read(2) and the mapping, as the
    // model has them.
    let check = |log: &[String], n: usize, at: usize, len: usize| {
        let mut read = vec![0; len];
        files[n].read_exact_at(&mut read, at as u64).unwrap();
        let mapped = unsafe { std::slice::from_raw_parts((maps[n] + at) as *const u8, len) };
        for i in 0..len {
            let want = models[n][at + i].load(Ordering::Relaxed);
            let mapped = unsafe { std::ptr::read_volatile(&mapped[i]) };
            if read[i] != want || mapped != want {
                trace_dump(&names[n], (at + i) as u64 / 4096);
                failed(
                    log,
                    format!(
                        "file {n} at {:#x}: read {:#x}, mapped {mapped:#x}, the model {want:#x}",
                        at + i,
                        read[i]
                    ),
                );
            }
        }
    };
    for _ in 0..ops {
        let n = rng.next(files.len() as u64) as usize;
        let (at, len) = owned(rng, thread);
        match rng.next(16) {
            // write(2)
            0..=4 => {
                let data = rng.bytes(len);
                log.push(format!("write {len:#x} at {at:#x} of file {n}"));
                files[n].write_all_at(&data, at as u64).unwrap();
                (0..len).for_each(|i| models[n][at + i].store(data[i], Ordering::Relaxed));
                check(log, n, at, len);
            }
            // Stores through the mapping.
            5..=8 => {
                let data = rng.bytes(len);
                log.push(format!("store {len:#x} at {at:#x} of file {n}"));
                for (i, &byte) in data.iter().enumerate() {
                    unsafe { ((maps[n] + at + i) as *mut u8).write_volatile(byte) };
                    models[n][at + i].store(byte, Ordering::Relaxed);
                }
                check(log, n, at, len);
            }
            // Reads.
            9..=11 => {
                log.push(format!("read {len:#x} at {at:#x} of file {n}"));
                check(log, n, at, len);
            }
            12 | 13 => {
                log.push(format!("fdatasync file {n}"));
                files[n].sync_data().unwrap();
            }
            14 => {
                log.push(format!("fsync file {n}"));
                files[n].sync_all().unwrap();
            }
            _ => {
                let page = at / 4096 * 4096;
                log.push(format!("msync the page at {page:#x} of file {n}"));
                let synced = unsafe {
                    libc::msync((maps[n] + page) as *mut libc::c_void, 4096, libc::MS_SYNC)
                };
                assert_eq!(synced, 0, "msync: {}", std::io::Error::last_os_error());
            }
        }
    }
    for map in maps {
        assert_eq!(unsafe { libc::munmap(map as *mut libc::c_void, size) }, 0);
    }
}
