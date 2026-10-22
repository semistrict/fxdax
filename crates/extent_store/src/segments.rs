// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's segments (see the crate's docs): what the commits since a segment opened changed,
//! which its footer records; fxdax: a closed segment's data framed, with its footer, off the
//! commit that closed it; and the segments a crash left open, closed again.

use crate::dir::{Dir, File, not_found};
use crate::framed::{Aligned, Framed, read_frame, read_whole};
use crate::layers::Extents;
use crate::layout::{Change, Extent, OPEN_SEGMENTS, SEGMENTS, SegmentId, branch_dir};
use crate::layout::{open_segment_path, segment_path};
use crate::lineage::{Lineage, list_numbers};
use crate::sync::{Condvar, Mutex, thread};
use anyhow::{Context as _, Error, bail};
use extent_frame::segment::{Footer, epoch_of};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;
use storage_device::ExtentId;

/// fxdax: a closed segment's data being framed, with its footer, into its framed file, off the
/// commit that closed it, which would otherwise hold up every write and commit behind it for as
/// long as reading, compressing and writing a segment takes (TASK-41: 67 ms under pgbench, the
/// store's commits' time). The flush of that commit waits for it
/// ([`Store::flush`](crate::Store::flush)).
pub(crate) struct Framing<F> {
    framed: Mutex<Option<Result<Arc<Framed<F>>, String>>>,
    done: Condvar,
}

impl<F: File> Framing<F> {
    /// Frames, on a thread of its own, the `len` bytes of data of the segment whose raw file is
    /// `raw`, with `footer`, into the framed file at `path` in `dir`.
    pub fn start<D: Dir<File = F>>(
        lineage: Arc<Lineage<D>>,
        path: String,
        raw: Arc<F>,
        len: u64,
        footer: Vec<u8>,
    ) -> Arc<Self> {
        let framing = Arc::new(Self { framed: Mutex::new(None), done: Condvar::new() });
        let framing_here = framing.clone();
        thread::spawn(move || {
            let framed = frame_segment(&lineage.dir, &path, &raw, len, &footer);
            *framing_here.framed.lock().unwrap() =
                Some(framed.map_err(|error| format!("{error:#}")));
            framing_here.done.notify_all();
        });
        framing
    }

    /// Its framed file, once framed, or what failed.
    pub fn wait(&self) -> Result<Arc<Framed<F>>, Error> {
        let mut framed = self.framed.lock().unwrap();
        loop {
            match &*framed {
                Some(Ok(file)) => return Ok(file.clone()),
                Some(Err(error)) => bail!("framing a segment: {error}"),
                None => framed = self.done.wait(framed).unwrap(),
            }
        }
    }
}

/// fxdax: frames the `len` bytes of a closed segment's data, which its raw file `raw` holds, with
/// `footer`, into the framed file at `path` in `dir`: counted in [`segment_closes`].
pub(crate) fn frame_segment<D: Dir>(
    dir: &D,
    path: &str,
    raw: &D::File,
    len: u64,
    footer: &[u8],
) -> Result<Arc<Framed<D::File>>, Error> {
    let started = Instant::now();
    let mut data = Aligned::new(len as usize);
    raw.read_at(data.as_mut_slice(), 0)?;
    let bytes = extent_frame::framed(data.as_slice(), footer);
    let file = dir.create(path, true)?;
    file.write_at(&bytes, 0)?;
    let framed = Framed::new(Arc::new(file), extent_frame::Parts::of(&bytes)?);
    SEGMENT_CLOSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = started.elapsed().as_nanos() as u64;
    SEGMENT_CLOSE_NS.fetch_add(nanos, std::sync::atomic::Ordering::Relaxed);
    Ok(Arc::new(framed))
}

/// What the commits since a segment opened changed: the extents they sealed, adopted or moved,
/// which the branch holds still, those they freed, and the root. Applying it to the branch's state
/// as of any of those commits gives its state as of the last.
#[derive(Default)]
pub(crate) struct Window {
    /// Its first commit's epoch.
    pub first: u64,
    /// The extents its commits sealed, adopted or moved, which the branch holds as of its last, as
    /// they were then: a segment closed between commits holds what they made, not what came after.
    pub touched: BTreeMap<ExtentId, Extent>,
    pub freed: BTreeSet<ExtentId>,
    pub root: Option<Vec<u8>>,
    /// The branches forked at its end.
    pub forks: Vec<u64>,
}

impl Window {
    pub fn starting(first: u64) -> Self {
        Self { first, ..Self::default() }
    }

    /// Records what a commit changed, `extents` the branch's extents as of it.
    pub fn commit(&mut self, changes: &[Change], root: Option<&[u8]>, extents: &Extents) {
        let mut touched = BTreeSet::new();
        for change in changes {
            match change {
                Change::Write { extent, .. }
                | Change::WriteOwn { extent, .. }
                | Change::Adopt { extent, .. }
                | Change::Move { extent, .. }
                | Change::MoveOwn { extent } => {
                    touched.insert(*extent);
                }
                Change::Free(extent) => {
                    touched.remove(extent);
                    self.touched.remove(extent);
                    self.freed.insert(*extent);
                }
            }
        }
        for extent in touched {
            self.touched.insert(extent, extents.get(&extent).expect("touched and held").clone());
        }
        if let Some(root) = root {
            self.root = Some(root.to_vec());
        }
    }
}

/// The store's epoch of the last commit of the segment whose footer is `footer`.
pub(crate) fn footer_last(footer: &Footer) -> u64 {
    footer.commits.last().map_or(0, |tag| epoch_of(*tag))
}

/// The footer of a closed segment, `bytes`: none if the segment closed with none, as one whose
/// footer named commits not durable, which opening its branch closes again.
pub(crate) fn segment_footer(bytes: &[u8]) -> Result<Option<Footer>, Error> {
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(Footer::decode(bytes).context("a segment's footer")?))
}

/// Closes `branch`'s segments left open, whose data its state as of `epoch` may need, and those
/// closed whose footers name commits after it, which were not durable: framed, with no footer.
pub(crate) fn close_left_open<D: Dir>(dir: &D, branch: u64, epoch: u64) -> Result<(), Error> {
    let open = list_numbers(dir, &branch_dir(OPEN_SEGMENTS, branch))?;
    let closed = list_numbers(dir, &branch_dir(SEGMENTS, branch))?;
    let numbers: BTreeSet<u64> = open.iter().chain(&closed).copied().collect();
    for number in numbers {
        let segment = SegmentId { branch, number };
        let path = segment_path(segment);
        let raw = || -> Result<Vec<u8>, Error> {
            Ok(read_whole(&dir.open(&open_segment_path(segment), true)?)?)
        };
        // Its data, if it is to be framed again: the framed file's, if it is whole, which a commit
        // made durable, or a closing between commits ([`Store::close_now`]), which removed the
        // raw file; or else the raw file's.
        let data = match dir.open(&path, true) {
            Ok(file) => match read_frame(&file)? {
                Some((_, footer)) => match segment_footer(&footer) {
                    Ok(footer) if footer.as_ref().is_none_or(|f| footer_last(f) <= epoch) => None,
                    _ => match extent_frame::data(&read_whole(&file)?) {
                        Ok(data) => Some(data),
                        Err(_) => Some(raw()?),
                    },
                },
                None => Some(raw()?),
            },
            Err(err) if not_found(&err) => Some(raw()?),
            Err(err) => return Err(err.into()),
        };
        if let Some(data) = data {
            match dir.remove(&path) {
                Err(err) if !not_found(&err) => return Err(err.into()),
                _ => {}
            }
            let file = dir.create(&path, true)?;
            file.write_at(&extent_frame::framed(&data, &[]), 0)?;
            file.sync()?;
            dir.sync_dir(&branch_dir(SEGMENTS, branch))?;
            dir.barrier()?;
        }
        match dir.remove(&open_segment_path(segment)) {
            Err(err) if !not_found(&err) => return Err(err.into()),
            _ => {}
        }
    }
    Ok(())
}

/// fxdax: the segments this process's stores closed, and the nanoseconds closing them took.
static SEGMENT_CLOSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SEGMENT_CLOSE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// fxdax: how many segments this process's stores closed since it started, and the nanoseconds
/// framing them took: their data read, framed and written, off the commits that closed them but
/// for [`Store::close_now`](crate::Store::close_now)'s.
pub fn segment_closes() -> (u64, u64) {
    let load =
        |count: &std::sync::atomic::AtomicU64| count.load(std::sync::atomic::Ordering::Relaxed);
    (load(&SEGMENT_CLOSES), load(&SEGMENT_CLOSE_NS))
}
