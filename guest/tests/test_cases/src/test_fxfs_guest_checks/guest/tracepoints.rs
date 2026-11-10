// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the guest Fxfs's tracepoints (fs/fxfs/fxfs_trace.h, TASK-44.16), as tracefs shows them:
//! a write, an fsync, and on a DAX mount a load and a store through a mapping, each step's events
//! in order, with the fields they say.

use super::*;

const TRACING: &str = "/sys/kernel/tracing";
/// What [`Trace::mark`] writes before its label, in trace_marker.
const MARK: &str = "fxfs-check:";
/// The kernel's VM_FAULT_ERROR: OOM, SIGBUS, HWPOISON, HWPOISON_LARGE, SIGSEGV and FALLBACK.
const VM_FAULT_ERROR: i64 = 0x873;
/// A PMD's order, 2 MiB of 4 KiB pages.
const PMD_ORDER: usize = 9;

/// An event of the trace: its name and its fields, `key=value`, or a mark's label.
#[derive(Debug)]
struct Event {
    name: String,
    fields: std::collections::BTreeMap<String, String>,
    mark: Option<String>,
}

impl Event {
    fn field(&self, key: &str) -> &str {
        self.fields.get(key).unwrap_or_else(|| panic!("{key} in {self:?}"))
    }

    /// A field as a number, decimal or 0x hex.
    fn number(&self, key: &str) -> i64 {
        let value = self.field(key);
        let parsed = match value.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16).map(|n| n as i64),
            None => value.parse::<i64>().or_else(|_| value.parse::<u64>().map(|n| n as i64)),
        };
        parsed.unwrap_or_else(|_| panic!("{key} isn't a number in {self:?}"))
    }
}

/// fs/fxfs's events, on from `new` until `events` reads them, with marks between steps.
struct Trace;

impl Trace {
    /// fs/fxfs's events, every one, on, the trace emptied: tracefs mounted first, if it isn't.
    fn new() -> Self {
        if !std::path::Path::new(&format!("{TRACING}/events")).exists() {
            let (source, target) = (CString::new("tracefs").unwrap(), CString::new(TRACING).unwrap());
            let ret = unsafe {
                libc::mount(source.as_ptr(), target.as_ptr(), source.as_ptr(), 0, std::ptr::null())
            };
            assert_eq!(ret, 0, "mounting tracefs: {}", std::io::Error::last_os_error());
        }
        assert!(
            std::path::Path::new(&format!("{TRACING}/events/fxfs/fxfs_seal_begin")).exists(),
            "no fxfs events in {TRACING}/events"
        );
        fs::write(format!("{TRACING}/buffer_size_kb"), b"4096").unwrap();
        fs::write(format!("{TRACING}/trace"), b"").unwrap();
        fs::write(format!("{TRACING}/events/fxfs/enable"), b"1").unwrap();
        fs::write(format!("{TRACING}/tracing_on"), b"1").unwrap();
        Trace
    }

    /// A mark in the trace, between what came before and what comes after.
    fn mark(&self, label: &str) {
        fs::write(format!("{TRACING}/trace_marker"), format!("{MARK} {label}")).unwrap();
    }

    /// The events, in their order, the events off.
    fn events(self) -> Vec<Event> {
        fs::write(format!("{TRACING}/events/fxfs/enable"), b"0").unwrap();
        let text = fs::read_to_string(format!("{TRACING}/trace")).unwrap();
        assert!(!text.contains("LOST"), "the trace lost events:\n{text}");
        text.lines().filter(|line| !line.starts_with('#')).filter_map(Self::parse).collect()
    }

    /// A line of the trace: `task-pid [cpu] flags timestamp: name: fields`.
    fn parse(line: &str) -> Option<Event> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words.iter().position(|word| {
            word.strip_suffix(':').is_some_and(|stamp| stamp.parse::<f64>().is_ok())
        })?;
        let name = words.get(at + 1)?.strip_suffix(':')?.to_owned();
        let rest = &words[at + 2..];
        if name == "tracing_mark_write" {
            let label = rest.join(" ");
            let label = label.strip_prefix(MARK)?.trim().to_owned();
            return Some(Event { name, fields: Default::default(), mark: Some(label) });
        }
        let fields = (rest.iter())
            .filter_map(|field| field.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        Some(Event { name, fields, mark: None })
    }
}

/// The events between the marks `from` and `to`.
fn between<'a>(events: &'a [Event], from: &str, to: &str) -> &'a [Event] {
    let at = |label: &str| {
        (events.iter().position(|event| event.mark.as_deref() == Some(label)))
            .unwrap_or_else(|| panic!("no mark {label:?} in the trace"))
    };
    let (start, end) = (at(from), at(to));
    assert!(start < end, "{from:?} after {to:?}");
    &events[start + 1..end]
}

/// The first event at or after `*at` that `matches`, `*at` moved past it: steps in order.
fn next<'a>(
    events: &'a [Event],
    at: &mut usize,
    what: &str,
    matches: impl Fn(&Event) -> bool,
) -> &'a Event {
    let found = (events[*at..].iter().position(&matches))
        .unwrap_or_else(|| panic!("no {what} after event {at} of {events:#?}"));
    *at += found + 1;
    &events[*at - 1]
}

/// That an fsync of `ino`, its events `events`, wrote and sealed `len` bytes at least: its start,
/// a device WRITE, completed as submitted, a seal begun and ended, and its end, 0, in that order.
/// On a DAX file (`dax`), its writeback's start and end, of a page at least, before the seal.
fn check_fsync(events: &[Event], ino: u64, len: i64, dax: bool) {
    let mut at = 0;
    let ino_is = |event: &Event| event.number("ino") == ino as i64;
    next(events, &mut at, "fsync start", |e| e.name == "fxfs_fsync_start" && ino_is(e));
    if dax {
        next(events, &mut at, "writeback start", |e| {
            e.name == "fxfs_dax_writeback_start" && ino_is(e)
        });
    }
    let write = next(events, &mut at, "a WRITE", |e| {
        e.name == "fxfs_device_submit" && e.field("type") == "write"
    });
    assert_ne!(write.number("id"), 0, "{write:?}");
    assert!(write.number("len") > 0, "{write:?}");
    let id = write.number("id");
    let done = next(events, &mut at, "the WRITE's completion", |e| {
        e.name == "fxfs_device_done" && e.number("id") == id
    });
    assert_eq!(done.field("type"), "write", "{done:?}");
    assert_eq!(done.number("status"), 0, "{done:?}");
    if dax {
        let end = next(events, &mut at, "writeback end", |e| {
            e.name == "fxfs_dax_writeback_end" && ino_is(e)
        });
        assert!(end.number("ret") >= 1, "a writeback of no page: {end:?}");
    }
    let begin = next(events, &mut at, "a seal", |e| e.name == "fxfs_seal_begin");
    let seals = begin.number("seals");
    assert_eq!(seals % 2, 1, "a seal begun with an even count: {begin:?}");
    let end = next(events, &mut at, "the seal's end", |e| e.name == "fxfs_seal_end");
    assert_eq!(end.number("seals"), seals + 1, "{end:?}");
    let fsync_end = next(events, &mut at, "fsync end", |e| e.name == "fxfs_fsync_end" && ino_is(e));
    assert_eq!(fsync_end.number("ret"), 0, "{fsync_end:?}");
    // What the fsync wrote, the seals in it sealed.
    let sealed: i64 = (events.iter())
        .filter(|e| e.name == "fxfs_seal_end")
        .map(|e| e.number("bytes"))
        .sum();
    assert!(sealed >= len, "the fsync's seals sealed {sealed} bytes, of {len} written");
}

/// Of all `events`: each request's start and end (submit, done; a window call's), one of each,
/// the end after the start, with the same id; and each seal's end right after its beginning.
fn check_pairs(events: &[Event]) {
    use std::collections::BTreeMap;
    let mut open: BTreeMap<(String, i64), usize> = BTreeMap::new();
    let mut ended = 0;
    let mut sealing: Option<i64> = None;
    for (i, event) in events.iter().enumerate() {
        let start = match event.name.as_str() {
            "fxfs_device_submit" => Some("device"),
            "fxfs_window_map_start" => Some("map"),
            "fxfs_window_unmap_start" => Some("unmap"),
            "fxfs_window_private_start" => Some("private"),
            _ => None,
        };
        let end = match event.name.as_str() {
            "fxfs_device_done" => Some("device"),
            "fxfs_window_map_end" => Some("map"),
            "fxfs_window_unmap_end" => Some("unmap"),
            "fxfs_window_private_end" => Some("private"),
            _ => None,
        };
        if let Some(kind) = start {
            let id = event.number("id");
            assert_ne!(id, 0, "{event:?}");
            let was = open.insert((kind.to_owned(), id), i);
            assert_eq!(was, None, "id {id} started twice: {event:?}");
        }
        if let Some(kind) = end {
            let id = event.number("id");
            // Begun with its event off, before the trace: no id.
            if id == 0 {
                continue;
            }
            let began = open.remove(&(kind.to_owned(), id));
            assert!(began.is_some(), "{event:?} ended no request begun in the trace");
            if kind == "device" {
                let submitted = &events[began.unwrap()];
                assert_eq!(event.field("type"), submitted.field("type"), "{event:?}");
            }
            ended += 1;
        }
        if event.name == "fxfs_seal_begin" {
            assert_eq!(sealing, None, "a seal begun in another: {event:?}");
            sealing = Some(event.number("seals"));
        }
        if event.name == "fxfs_seal_end" {
            // The trace may begin within a seal.
            if let Some(seals) = sealing.take() {
                assert_eq!(event.number("seals"), seals + 1, "{event:?}");
            }
        }
    }
    assert!(ended > 0, "no request ended in the trace");
}

/// fxdax: the tracepoints (TASK-44.16): a file written with write(2) and fsynced, each step's
/// events in order (check_fsync); on a DAX mount, the file read in sealed, DAX, mapped shared, a
/// page loaded, which faults and MAPs the window, and stored to, which faults writable and makes
/// the window's page private (PRIVATE) within what the MAP mapped, then fsynced, its writeback
/// writing the page. Every request's events paired by id (check_pairs).
///
/// The load's fault is a page's (order 0), alone: the mapping shares its PMD-aligned 2 MiB with a
/// page of memory stored to first, whose page table the PMD's entry holds. With that entry empty,
/// the kernel offers a fault of a DAX mapping as a PMD's first (huge_fault, order 9), whatever the
/// mapping's size, leaving the handler to say whether it fits (__thp_vma_allowable_orders skips
/// the check in a fault; fs/dax.c's dax_fault_check_fallback makes it): this mapping of 16 pages
/// falls back, mapping nothing, and the page's fault follows. As many PMD faults as the kernel
/// tries the fault: under the VMA's lock, then again under the mmap lock, as do_read_fault retries
/// a mapping with no map_pages (vmf_can_call_fault). Placed where the kernel chose, the mapping
/// shared its 2 MiB with what had faulted there already, or didn't: the load took a page's fault
/// alone, or PMD faults first, from run to run (TASK-299).
pub(super) fn test_tracepoints(dax: bool) {
    const PAGE: usize = 4096;
    const LEN: usize = 16 * PAGE;
    const PMD: usize = PAGE << PMD_ORDER;
    /// A hole, which makes the file two pieces: no slice of one extent's region, which an extent
    /// another file shares may have mapped already, but a range of its own, which MAPs.
    const HOLE: std::ops::Range<usize> = 8 * PAGE..12 * PAGE;
    let name = path("tracepoints");
    let mut data: Vec<u8> = (0..LEN).map(|i| (i % 253) as u8).collect();
    data[HOLE].fill(0);
    let trace = Trace::new();

    trace.mark("write");
    let mut file = fs::File::create(&name).unwrap();
    file.write_all(&data[..HOLE.start]).unwrap();
    file.seek(SeekFrom::Start(HOLE.end as u64)).unwrap();
    file.write_all(&data[HOLE.end..]).unwrap();
    trace.mark("fsync");
    file.sync_all().unwrap();
    trace.mark("fsynced");
    let ino = file.metadata().unwrap().ino();
    drop(file);

    if dax {
        read_in_sealed(&name);
        let file = fs::OpenOptions::new().read(true).write(true).open(&name).unwrap();
        // Two PMDs' worth, inaccessible: a PMD-aligned 2 MiB within it, the mapping's, and a page
        // of memory after it, stored to, its page table the PMD's.
        let reserved = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                2 * PMD,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert_ne!(reserved, libc::MAP_FAILED, "mmap: {}", std::io::Error::last_os_error());
        let aligned = (reserved as usize).next_multiple_of(PMD);
        let map = unsafe {
            libc::mmap(
                aligned as *mut libc::c_void,
                LEN,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_eq!(map as usize, aligned, "mmap: {}", std::io::Error::last_os_error());
        let memory = unsafe {
            libc::mmap(
                (aligned + LEN) as *mut libc::c_void,
                PAGE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED,
                -1,
                0,
            )
        };
        assert_eq!(memory as usize, aligned + LEN, "mmap: {}", std::io::Error::last_os_error());
        unsafe { memory.cast::<u8>().write_volatile(1) };
        let page = unsafe { map.cast::<u8>().add(2 * PAGE) };
        trace.mark("load");
        assert_eq!(unsafe { page.read_volatile() }, data[2 * PAGE]);
        trace.mark("store");
        unsafe { page.write_volatile(!data[2 * PAGE]) };
        trace.mark("fsync-dax");
        file.sync_all().unwrap();
        trace.mark("end");
        assert_eq!(unsafe { libc::munmap(reserved, 2 * PMD) }, 0);
    } else {
        trace.mark("end");
    }
    let events = trace.events();

    check_pairs(&events);
    let written = (LEN - HOLE.len()) as i64;
    check_fsync(between(&events, "fsync", "fsynced"), ino, written, false);
    if !dax {
        fs::remove_file(&name).unwrap();
        return;
    }

    let ino_is = |event: &Event| event.number("ino") == ino as i64;
    // The load: a read fault of page 2, a page's, the one fault of the load, which MAPs the
    // window, read-only, why it does said first.
    let load = between(&events, "load", "store");
    let mut at = 0;
    let fault_start = |e: &Event| e.name == "fxfs_dax_fault_start";
    let faults: Vec<&Event> = load.iter().filter(|e| fault_start(e) && ino_is(e)).collect();
    assert_eq!(faults.len(), 1, "the load's faults: {faults:#?}");
    let fault = next(load, &mut at, "a read fault", fault_start);
    assert!(ino_is(fault) && fault.number("pgoff") == 2, "{fault:?}");
    assert_eq!((fault.number("order"), fault.number("write")), (0, 0), "{fault:?}");
    // A range's own MAP of all its pages: the data before the hole, zeros for it, the data after
    // it, and zeros for any of the range past the file's end.
    let why = next(load, &mut at, "why it maps", |e| e.name == "fxfs_dax_map");
    assert!(ino_is(why) && why.field("why") == "range", "{why:?}");
    assert!(why.number("len") >= LEN as i64 && why.number("count") >= 3, "{why:?}");
    let mapped = why.number("window")..why.number("window") + why.number("len");
    let map = next(load, &mut at, "its MAP", |e| e.name == "fxfs_window_map_start");
    assert_eq!(map.number("window"), why.number("window"), "{map:?} for {why:?}");
    assert_eq!(map.field("zeros"), "0", "{map:?}");
    assert_eq!(map.number("offset") % PAGE as i64, 0, "{map:?}");
    let id = map.number("id");
    let map_end = next(load, &mut at, "the MAP's end", |e| e.name == "fxfs_window_map_end");
    assert_eq!((map_end.number("id"), map_end.number("err")), (id, 0), "{map_end:?}");
    let end = next(load, &mut at, "the fault's end", |e| e.name == "fxfs_dax_fault_end");
    assert!(ino_is(end) && end.number("pgoff") == 2, "{end:?}");
    assert_eq!(end.number("ret") & VM_FAULT_ERROR, 0, "the load's fault failed: {end:?}");

    // The store: a write fault of page 2, which makes its window page private, copied on write.
    let store = between(&events, "store", "fsync-dax");
    let mut at = 0;
    let fault = next(store, &mut at, "a write fault", |e| {
        e.name == "fxfs_dax_fault_start" && e.number("write") == 1
    });
    assert!(ino_is(fault) && fault.number("pgoff") == 2, "{fault:?}");
    let private = next(store, &mut at, "a PRIVATE", |e| e.name == "fxfs_window_private_start");
    let window = private.number("window");
    assert!(private.number("len") >= PAGE as i64, "{private:?}");
    assert!(
        mapped.contains(&window) && window + private.number("len") <= mapped.end,
        "{private:?} isn't in what the load mapped, {mapped:x?}"
    );
    let id = private.number("id");
    let private_end =
        next(store, &mut at, "the PRIVATE's end", |e| e.name == "fxfs_window_private_end");
    assert_eq!((private_end.number("id"), private_end.number("err")), (id, 0), "{private_end:?}");
    let end = next(store, &mut at, "the fault's end", |e| e.name == "fxfs_dax_fault_end");
    assert!(ino_is(end) && end.number("pgoff") == 2, "{end:?}");
    assert_eq!(end.number("ret") & VM_FAULT_ERROR, 0, "the store's fault failed: {end:?}");

    // The fsync of the stored page: written back, then sealed.
    check_fsync(between(&events, "fsync-dax", "end"), ino, PAGE as i64, true);
    fs::remove_file(&name).unwrap();
}
