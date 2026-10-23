// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's segments (see the crate's docs): those it tracks, each where it is in its life
//! ([`Phase`]), with the bytes of extents' data it holds; the open segment's closing, its footer
//! of what the commits since it opened changed, each footer following the one before; fxdax: a
//! closed segment's data framed, with its footer, off the commit that closed it; and the segments
//! a crash left open, closed again.

use crate::dir::{Dir, File, not_found};
use crate::durability::Syncs;
use crate::framed::{Aligned, Files, Framed, Source, read_frame, read_parts, read_whole};
use crate::layers::Extents;
use crate::layout::{
    Change, Data, Extent, OPEN_SEGMENTS, Piece, SEGMENTS, SegmentId, branch_dir, list_numbers,
    open_segment_path, segment_path,
};
use crate::lineage::Lineage;
use crate::store::{Options, Usage};
use crate::sync::{Condvar, Mutex, thread};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use extent_frame::segment::{Chain, Contents, Footer, epoch_of};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;
use storage_device::ExtentId;

/// Where a segment the branch tracks is in its life.
enum Phase<F> {
    /// Taking writes, in its raw file.
    Open(Arc<F>),
    /// fxdax: closed by commit `last`, its data being framed off the commit ([`Closing`]): read
    /// from its raw file, which stays open, until the commit's flush has its framed file.
    Framing { raw: Arc<F>, last: u64 },
    /// Closed: the epoch of its footer's last commit, if the branch closed it since it was opened.
    /// Its framed file, if open, is among those kept open (`Segments::files`).
    Closed { last: Option<u64> },
    /// fxdax: set aside, full, though it held no commit's data: what writes since the last commit
    /// wrote to it is in its raw file, which no footer names, nor framing frames, and which goes
    /// once the next commit has copied the extents in it out (TASK-79.2).
    Retired(Arc<F>),
    /// Another branch's, which the branch inherited: tracked for the bytes of extents' data it
    /// holds; its file is the lineage's, which its branches share.
    Inherited,
}

/// A segment the branch tracks.
struct Segment<F> {
    phase: Phase<F>,
    /// The bytes of its data, if it is the branch's own.
    len: u64,
    /// The bytes its file takes, if it is the branch's own.
    size: u64,
    /// The bytes of extents' data it holds: pieces of extents not freed.
    live: u64,
    /// The writes to it in progress, and the bytes they reserved: neither live nor dead yet.
    writers: u64,
    pending: u64,
    /// fxdax: whether another branch may hold extents in it: a fork of the branch since it closed,
    /// unless cleaning found it holds none (TASK-79.2).
    shared: bool,
}

impl<F> Segment<F> {
    fn new(phase: Phase<F>, live: u64) -> Self {
        Self { phase, len: 0, size: 0, live, writers: 0, pending: 0, shared: false }
    }

    /// Whether reaping lets it go: no extent's data is in it, nor write in progress, and it takes
    /// none, closed, set aside or inherited.
    fn is_empty(&self) -> bool {
        matches!(self.phase, Phase::Closed { .. } | Phase::Retired(_) | Phase::Inherited)
            && self.live == 0
            && self.writers == 0
    }

    /// fxdax: the bytes of its data no extent is in, which the branch may take back, cleaning:
    /// none but the branch's own segments', and those no other branch may hold extents in.
    fn dead(&self) -> u64 {
        match self.phase {
            Phase::Inherited => 0,
            _ if self.shared => 0,
            _ => self.len - self.live - self.pending,
        }
    }
}

/// The segments a branch tracks: those its extents' data is in, its own, taking writes or not,
/// and those it inherited that it holds otherwise than the state it shares does, having let
/// extents in them go. It holds the others, untracked, as that state does. Its own segments' files
/// it keeps open itself: the open segment's, and one's being framed, which their phases hold, and
/// closed ones', at most `Options::open_files` but for those reads use, the least lately read
/// closing as more open.
///
/// One segment is open, which writes go to. It closes at a commit, or between commits, its footer
/// what the commits since it opened changed ([`Window`]); and another opens. Its footer follows
/// the one closed before it, as the chain of the footers' writer stamps it.
pub(crate) struct Segments<F> {
    tracked: BTreeMap<SegmentId, Segment<F>>,
    /// fxdax: those reaping lets go of ([`Segment::is_empty`]), but for those kept: so that a commit
    /// looks over them alone, not every segment the branch tracks, thousands as it grows
    /// (TASK-92.16).
    empty: BTreeSet<SegmentId>,
    /// The segment taking writes.
    open: SegmentId,
    /// When it opened.
    opened: Instant,
    /// Whether the next commit closes it, whatever it holds.
    close_wanted: bool,
    /// What the commits since it opened changed, which its footer records.
    window: Window,
    /// fxdax: the chain of the footers' writer, the writer of the branch in object storage, if an
    /// uploader gave one ([`Segments::set_chain`]), which stamps each footer with its place: its
    /// number, the footer it follows, its writer's lease epoch and its floor of the branch's next
    /// sequence number. Without, the footers are of epoch 0, numbered on from the segment open as
    /// the branch was opened, one for each segment closed, but none for those set aside, holding no
    /// commit's data (TASK-79.2); each following the last closed since the branch was opened.
    chain: Chain,
    /// The closed segments' framed files kept open.
    files: Files<SegmentId, F>,
    /// Whether the open segment was written since the last commit, which syncs it.
    written: bool,
    /// Whether a segment closed since the last commit, which syncs the directories of segments,
    /// the framed file's, and of open ones, the next's.
    closed_since: bool,
    /// fxdax: the bytes of the branch's own segments' data no extent is in, but those another
    /// branch may hold ([`Segment::dead`]).
    dead: u64,
    /// fxdax: how often branches had let go of files others made, as the lineage counts it, when
    /// it last took its segments shared for shared no more ([`Segments::unshare`]).
    unshared: u64,
}

/// A segment cleaning takes: the bytes of its data, and of extents' data in it.
pub(crate) struct Victim {
    pub len: u64,
    pub live: u64,
}

/// A segment the branch let go of, no extent being in it any more: its footer is the branch's
/// history, until a checkpoint covers it, if the branch closed it since it was opened, after
/// commit `last`; and the bytes of its data.
pub(crate) struct Reaped {
    pub number: SegmentId,
    pub last: Option<u64>,
    pub len: u64,
}

impl<F: File> Segments<F> {
    /// The segments of `branch`, opened as of commit `epoch`, holding `tracked`, the bytes of
    /// extents' data each holds, whose files must be there; and another, open, past whatever the
    /// branch's last one holds, whose file is made durable. At most `open_files` of their files
    /// stay open between reads.
    pub fn open<D: Dir<File = F>>(
        dir: &D,
        branch: u64,
        tracked: BTreeMap<SegmentId, u64>,
        epoch: u64,
        open_files: usize,
    ) -> Result<Self, Error> {
        let mut segments = BTreeMap::new();
        for (id, live) in tracked {
            let path = segment_path(id);
            let file = dir.open(&path, true).with_context(|| format!("segment {path}"))?;
            let segment = if id.branch == branch {
                // fxdax: its trailer and table, a block, not its footer (TASK-92.16).
                let parts = read_parts(&file)?
                    .ok_or_else(|| anyhow!("{path} is not a whole framed file"))?;
                let closed = Phase::Closed { last: None };
                Segment {
                    len: parts.frame.data_len,
                    size: parts.frame.size(),
                    ..Segment::new(closed, live)
                }
            } else {
                Segment::new(Phase::Inherited, live)
            };
            segments.insert(id, segment);
        }
        // Writes go to a new segment, past whatever the branch's last one holds.
        let own = list_numbers(dir, &branch_dir(SEGMENTS, branch))?;
        let open = SegmentId { branch, number: own.last().map_or(0, |last| last + 1) };
        let file = Arc::new(dir.create(&open_segment_path(open), true)?);
        dir.sync_dir(&branch_dir(OPEN_SEGMENTS, branch))?;
        segments.insert(open, Segment::new(Phase::Open(file), 0));
        let dead = segments.values().map(Segment::dead).sum();
        Ok(Self {
            tracked: segments,
            empty: BTreeSet::new(),
            open,
            opened: Instant::now(),
            close_wanted: false,
            window: Window::starting(epoch + 1),
            chain: Chain::new(branch, 0, open.number, None, 0),
            files: Files::new(open_files),
            written: false,
            closed_since: false,
            dead,
            unshared: 0,
        })
    }

    /// The segment taking writes.
    pub fn open_segment(&self) -> SegmentId {
        self.open
    }

    /// Whether the open segment takes `len` bytes more and holds no more than `limit`
    /// (`Options::segment_limit`).
    pub fn fits(&self, len: u64, limit: u64) -> bool {
        self.tracked[&self.open].len + len <= limit
    }

    /// Changes segment `number` as `change` says, keeping count of the bytes no extent is in, and
    /// of whether it is empty.
    fn change<T>(&mut self, number: SegmentId, change: impl FnOnce(&mut Segment<F>) -> T) -> T {
        let segment = self.tracked.get_mut(&number).unwrap();
        let dead = segment.dead();
        let changed = change(segment);
        self.dead = self.dead + segment.dead() - dead;
        self.note(number);
        changed
    }

    /// Notes whether segment `number`, tracked, is empty ([`Segment::is_empty`]), as it changed.
    fn note(&mut self, number: SegmentId) {
        if self.tracked[&number].is_empty() {
            self.empty.insert(number);
        } else {
            self.empty.remove(&number);
        }
    }

    /// Reserves `len` bytes of the open segment for a write: the segment, its raw file, and where
    /// in it the write goes. The write is in progress until it is done: held
    /// ([`Segments::held`]), or dropped ([`Segments::dropped`]).
    pub fn reserve(&mut self, len: u64) -> (SegmentId, Arc<F>, u64) {
        let open = self.open;
        self.change(open, |segment| {
            let Phase::Open(file) = &segment.phase else {
                unreachable!("the open segment is open")
            };
            let at = segment.len;
            segment.len += len;
            segment.size += len;
            segment.writers += 1;
            segment.pending += len;
            (open, file.clone(), at)
        })
    }

    /// A write of `len` bytes to segment `number` is done, and the segment holds what it wrote,
    /// extents' data: returns whether none is in progress in it any more.
    pub fn held(&mut self, number: SegmentId, len: u64) -> bool {
        self.written |= number == self.open;
        self.done(number, len, len)
    }

    /// A write of `len` bytes to segment `number` is done, and holds nothing: abandoned, or to an
    /// extent freed since it began; what it wrote is dead space. Returns whether none is in
    /// progress in the segment any more.
    pub fn dropped(&mut self, number: SegmentId, len: u64) -> bool {
        self.done(number, len, 0)
    }

    /// A write of `len` bytes to segment `number` is done, `live` of them holding extents' data.
    fn done(&mut self, number: SegmentId, len: u64, live: u64) -> bool {
        self.change(number, |segment| {
            segment.writers -= 1;
            segment.pending -= len;
            segment.live += live;
            segment.writers == 0
        })
    }

    /// The data of `piece` is in its segment no more: freed, or moved.
    pub fn let_go(&mut self, piece: &Piece) {
        self.change(piece.segment, |segment| segment.live -= piece.len);
    }

    /// Appends `len` bytes of an extent's data, sealed in one run, to the open segment, which
    /// holds them: returns its raw file, and where in it they go.
    pub fn append(&mut self, len: u64) -> (Arc<F>, u64) {
        self.written = true;
        self.change(self.open, |segment| {
            let Phase::Open(file) = &segment.phase else {
                unreachable!("the open segment is open")
            };
            let at = segment.len;
            segment.len += len;
            segment.size += len;
            segment.live += len;
            (file.clone(), at)
        })
    }

    /// fxdax: the bytes of the branch's own segments' data no extent is in, which cleaning may take
    /// back: but those another branch may hold extents in (TASK-79.2).
    pub fn dead(&self) -> u64 {
        self.dead
    }

    /// fxdax: takes the branch's own segments that `shared` names for shared with another branch,
    /// which may hold extents in them: as a fork of the branch does those its state at the fork
    /// holds (TASK-79.2).
    pub fn share(&mut self, shared: impl Fn(SegmentId) -> bool) {
        self.set_shared(|segment| !segment.shared, shared);
    }

    /// fxdax: takes the branch's own segments shared for shared no more, but those `shared` says
    /// another branch may still hold extents in, if branches let go of files others made since it
    /// last did: as `unshared`, the lineage's count of those, says (TASK-79.2). So that what a
    /// fork, deleted, held of them is the branch's again, without asking of each at each commit.
    pub fn unshare(&mut self, unshared: u64, shared: impl Fn(SegmentId) -> bool) {
        if std::mem::replace(&mut self.unshared, unshared) != unshared {
            self.set_shared(|segment| segment.shared, shared);
        }
    }

    /// Sets which of the branch's own segments `which` takes are shared, as `shared` says.
    fn set_shared(
        &mut self,
        which: impl Fn(&Segment<F>) -> bool,
        shared: impl Fn(SegmentId) -> bool,
    ) {
        let own: Vec<SegmentId> = (self.tracked.iter())
            .filter(|(_, segment)| !matches!(segment.phase, Phase::Inherited) && which(segment))
            .map(|(number, _)| *number)
            .collect();
        for number in own {
            let shared = shared(number);
            self.change(number, |segment| segment.shared = shared);
        }
    }

    /// Tracks `segment`, another branch's, unless it does: as the branch lets an extent in it go,
    /// it holds otherwise than the state it shares, which holds `live()` bytes of extents' data in
    /// it.
    pub fn track(&mut self, segment: SegmentId, live: impl FnOnce() -> u64) {
        let own = self.open.branch;
        self.tracked.entry(segment).or_insert_with(|| {
            debug_assert_ne!(segment.branch, own, "the branch's own segments are tracked");
            Segment::new(Phase::Inherited, live())
        });
        self.note(segment);
    }

    /// Whether it tracks `segment`.
    pub fn is_tracked(&self, segment: SegmentId) -> bool {
        self.tracked.contains_key(&segment)
    }

    /// How many segments it tracks.
    pub fn tracked(&self) -> usize {
        self.tracked.len()
    }

    /// The writes to the open segment in progress.
    pub fn open_writers(&self) -> u64 {
        self.tracked[&self.open].writers
    }

    /// Has the next commit close the open segment, whatever it holds.
    pub fn want_close(&mut self) {
        self.close_wanted = true;
    }

    /// Has the next commit close the open segment, at the fork of `child`, which its footer
    /// names (doc-7, Forks).
    pub fn close_for_fork(&mut self, child: u64) {
        self.close_wanted = true;
        self.window.forks.push(child);
    }

    /// Whether the open segment holds commits up to `epoch`, the last, or forks, which closing it
    /// records.
    pub fn has_commits(&self, epoch: u64) -> bool {
        self.window.first <= epoch || !self.window.forks.is_empty()
    }

    /// Records what a commit changed, `changes`, and its root, if it set one, in the open
    /// segment's footer to be: `extents` the branch's extents as of it.
    pub fn record(&mut self, changes: &[Change], root: Option<&[u8]>, extents: &Extents) {
        self.window.commit(changes, root, extents);
    }

    /// Whether the open segment is to close, as `options` say, once no write is in progress in
    /// it: full, old, wanted closed, or at a fork.
    pub fn due(&self, options: &Options) -> bool {
        let open = &self.tracked[&self.open];
        let old = open.len > 0 && self.opened.elapsed() >= options.segment_age;
        open.writers == 0 && (open.len >= options.segment_size || old || self.close_wanted)
    }

    /// fxdax: stamps the footers with `chain` from now on, an uploader's writer's
    /// ([`Store::upload`](crate::Store::upload)).
    pub fn set_chain(&mut self, chain: Chain) {
        self.chain = chain;
    }

    /// Closes the open segment, after commit `epoch`, the branch's next sequence number
    /// `next_sequence`: its footer, of what the commits since it opened changed, stamped by the
    /// chain, after its data, to be framed in a file of its own, as object storage keeps segments;
    /// and opens the next, in `dir`. Nothing changes if it fails.
    pub fn close<D: Dir<File = F>>(
        &mut self,
        dir: &D,
        epoch: u64,
        next_sequence: u64,
    ) -> Result<Unframed<F>, Error> {
        let number = self.open;
        let (extents, own) = self.window.places(number)?;
        let next = SegmentId { number: number.number + 1, ..number };
        let file = Arc::new(dir.create(&open_segment_path(next), true)?);
        // Nothing fails from here on.
        let window = std::mem::replace(&mut self.window, Window::starting(epoch + 1));
        let footer = self.chain.close(Contents {
            commits: vec![self.chain.tag(epoch)],
            extents,
            own,
            frees: window.freed.into_iter().collect(),
            root: window.root,
            next_sequence,
            forks: window.forks,
        });
        let segment = self.tracked.get_mut(&number).unwrap();
        let raw = match &segment.phase {
            Phase::Open(raw) => raw.clone(),
            _ => unreachable!("the open segment is open"),
        };
        segment.phase = Phase::Framing { raw: raw.clone(), last: epoch };
        let len = segment.len;
        // What was written to it is durable once framed: the commit's flush syncs its framed
        // file, or the closing between commits does.
        self.written = false;
        self.tracked.insert(next, Segment::new(Phase::Open(file), 0));
        self.open = next;
        self.opened = Instant::now();
        self.close_wanted = false;
        self.closed_since = true;
        Ok(Unframed { number, raw, len, footer })
    }

    /// fxdax: sets the open segment aside, full, though it holds no commit's data, which it must
    /// not, and opens the next, in `dir` (TASK-79.2): writes in progress in it go on, and reads
    /// read it, as its raw file; the next commit copies the extents in it out, as they are not in
    /// the open segment; then no extent is in it, and it goes. The next holds what the commits
    /// since it opened changed, none yet, its footer, when it closes, following the last closed in
    /// the chain, as this one, which has none, would have. Nothing changes if it fails.
    pub fn retire<D: Dir<File = F>>(&mut self, dir: &D) -> Result<(), Error> {
        let number = self.open;
        let window = &self.window;
        debug_assert!(
            window.touched.is_empty() && window.forks.is_empty(),
            "{number:?}, set aside, holds a commit's data or a fork"
        );
        let next = SegmentId { number: number.number + 1, ..number };
        let file = Arc::new(dir.create(&open_segment_path(next), true)?);
        let segment = self.tracked.get_mut(&number).unwrap();
        let Phase::Open(raw) = &segment.phase else { unreachable!("the open segment is open") };
        segment.phase = Phase::Retired(raw.clone());
        self.note(number);
        self.tracked.insert(next, Segment::new(Phase::Open(file), 0));
        self.open = next;
        self.opened = Instant::now();
        // What it holds, the next commit copies to the open segment: nothing of it is to be
        // synced, but the next's entry, with those of segments closed.
        self.written = false;
        self.closed_since = true;
        Ok(())
    }

    /// fxdax: segment `number`, closed and being framed, is in `framed` from now on, its raw file
    /// no more.
    pub fn framed(&mut self, number: SegmentId, framed: Arc<Framed<F>>) {
        let segment = self.tracked.get_mut(&number).expect("a segment being framed is tracked");
        let Phase::Framing { last, .. } = segment.phase else {
            unreachable!("{number:?}, framed, was being framed")
        };
        segment.size = framed.size();
        segment.phase = Phase::Closed { last: Some(last) };
        self.note(number);
        self.files.insert(number, framed);
    }

    /// fxdax: whether segment `number` is being framed.
    pub fn is_framing(&self, number: SegmentId) -> bool {
        matches!(self.tracked.get(&number), Some(Segment { phase: Phase::Framing { .. }, .. }))
    }

    /// The file of segment `number`, the branch's own, opened in `dir` if it isn't: the raw file
    /// of the open segment, or of one being framed or set aside; a closed one's framed file.
    pub fn source<D: Dir<File = F>>(
        &mut self,
        dir: &D,
        number: SegmentId,
    ) -> Result<Source<F>, Error> {
        let segment = self.tracked.get(&number).ok_or_else(|| anyhow!("no segment {number:?}"))?;
        match &segment.phase {
            Phase::Open(raw) | Phase::Framing { raw, .. } | Phase::Retired(raw) => {
                return Ok(Source::Raw(raw.clone()));
            }
            Phase::Closed { .. } => {}
            Phase::Inherited => bail!("segment {number:?} is another branch's"),
        }
        if let Some(framed) = self.files.get(number) {
            return Ok(Source::Framed(framed));
        }
        let path = segment_path(number);
        let framed = Arc::new(Framed::open(dir.open(&path, true)?, &path)?);
        Ok(Source::Framed(self.files.insert(number, framed)))
    }

    /// Adds to `syncs` what the next commit's flush syncs of the segments': the open segment's
    /// file, if it was written since the last commit, keeping its pages in the host's page cache,
    /// as they go with it once it is framed and removed (TASK-92.23); and if one closed since, the
    /// directories of segments and of open ones.
    pub fn take_syncs(&mut self, syncs: &mut Syncs<F>) {
        if std::mem::take(&mut self.written) {
            let Phase::Open(file) = &self.tracked[&self.open].phase else {
                unreachable!("the open segment is open")
            };
            syncs.cached(file.clone());
        }
        if std::mem::take(&mut self.closed_since) {
            for kind in [SEGMENTS, OPEN_SEGMENTS] {
                syncs.dir(branch_dir(kind, self.open.branch));
            }
        }
    }

    /// Lets go of the segments no extent's data is in any more, closed, set aside or inherited,
    /// which no write is in progress in, but those `keep` says to keep, and returns them.
    pub fn reap(&mut self, keep: impl Fn(SegmentId) -> bool) -> Vec<Reaped> {
        let dead: Vec<SegmentId> =
            self.empty.iter().filter(|number| !keep(**number)).copied().collect();
        let mut reaped = Vec::with_capacity(dead.len());
        for number in dead {
            self.empty.remove(&number);
            let segment = self.tracked.remove(&number).unwrap();
            self.dead -= segment.dead();
            self.files.remove(number);
            let last = match segment.phase {
                Phase::Closed { last } => last,
                _ => None,
            };
            reaped.push(Reaped { number, last, len: segment.len });
        }
        reaped
    }

    /// The branch's own segments, but the open one, whose extents' data takes less than
    /// `live_fraction` of them, which no write is in progress in, and which no other branch holds
    /// extents in, as `shared` says: the bytes of each one's data, and of its extents', which
    /// cleaning copies out.
    pub fn victims(
        &self,
        live_fraction: f64,
        shared: impl Fn(SegmentId) -> bool,
    ) -> BTreeMap<SegmentId, Victim> {
        (self.tracked.iter())
            .filter(|(number, segment)| {
                number.branch == self.open.branch
                    && **number != self.open
                    && !shared(**number)
                    && segment.writers == 0
                    && segment.len > 0
                    && (segment.live as f64) < live_fraction * segment.len as f64
            })
            .map(|(number, segment)| (*number, Victim { len: segment.len, live: segment.live }))
            .collect()
    }

    /// What the branch's own segments take: how many there are, the bytes of their data, of
    /// extents' data, and of their files; none of what the extents stored on their own take.
    pub fn usage(&self) -> Usage {
        let own = || self.tracked.iter().filter(|(id, _)| id.branch == self.open.branch);
        Usage {
            segments: own().count() as u64,
            segment_bytes: own().map(|(_, segment)| segment.len).sum(),
            live_bytes: own().map(|(_, segment)| segment.live).sum(),
            dead_bytes: self.dead,
            own_bytes: 0,
            disk_bytes: own().map(|(_, segment)| segment.size).sum(),
            kept_bytes: 0,
            kept_for_views: 0,
        }
    }
}

/// fxdax: a segment just closed, whose data is yet to be framed, with `footer`: now
/// ([`Unframed::frame`]), or off the commit that closed it ([`Unframed::frame_behind`]).
pub(crate) struct Unframed<F> {
    number: SegmentId,
    raw: Arc<F>,
    len: u64,
    pub footer: Footer,
}

impl<F: File> Unframed<F> {
    /// The segment.
    pub fn number(&self) -> SegmentId {
        self.number
    }

    /// Frames it now, in its framed file in `dir`, and returns that.
    pub fn frame<D: Dir<File = F>>(self, dir: &D) -> Result<Arc<Framed<F>>, Error> {
        let footer = self.footer.encode();
        frame_segment(dir, &segment_path(self.number), &self.raw, self.len, &footer)
    }

    /// Frames it on a thread of its own, in its framed file in `lineage`'s directory, which the
    /// flush of the commit that closed it waits for.
    pub fn frame_behind<D: Dir<File = F>>(self, lineage: Arc<Lineage<D>>) -> Closing<F> {
        let (path, footer) = (segment_path(self.number), self.footer.encode());
        let framing = Framing::start(lineage, path, self.raw, self.len, footer);
        Closing { number: self.number, framing }
    }
}

/// fxdax: a segment a commit closed, being framed off it: what the commit's flush waits for.
pub(crate) struct Closing<F> {
    number: SegmentId,
    framing: Arc<Framing<F>>,
}

impl<F: File> Closing<F> {
    /// The segment, and its framed file, once framed; or what failed.
    pub fn wait(&self) -> Result<(SegmentId, Arc<Framed<F>>), Error> {
        Ok((self.number, self.framing.wait()?))
    }
}

/// fxdax: a closed segment's data being framed, with its footer, into its framed file, off the
/// commit that closed it, which would otherwise hold up every write and commit behind it for as
/// long as reading, compressing and writing a segment takes (TASK-41: 67 ms under pgbench, the
/// store's commits' time). The flush of that commit waits for it
/// ([`Store::flush`](crate::Store::flush)).
struct Framing<F> {
    framed: Mutex<Option<Result<Arc<Framed<F>>, String>>>,
    done: Condvar,
}

impl<F: File> Framing<F> {
    /// Frames, on a thread of its own, the `len` bytes of data of the segment whose raw file is
    /// `raw`, with `footer`, into the framed file at `path` in `dir`.
    fn start<D: Dir<File = F>>(
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
    fn wait(&self) -> Result<Arc<Framed<F>>, Error> {
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
fn frame_segment<D: Dir>(
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
struct Window {
    /// Its first commit's epoch.
    first: u64,
    /// The extents its commits sealed, adopted or moved, which the branch holds as of its last, as
    /// they were then: a segment closed between commits holds what they made, not what came after.
    touched: BTreeMap<ExtentId, Extent>,
    freed: BTreeSet<ExtentId>,
    root: Option<Vec<u8>>,
    /// The branches forked at its end.
    forks: Vec<u64>,
}

impl Window {
    fn starting(first: u64) -> Self {
        Self { first, ..Self::default() }
    }

    /// Records what a commit changed, `extents` the branch's extents as of it.
    fn commit(&mut self, changes: &[Change], root: Option<&[u8]>, extents: &Extents) {
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
            // Held as of it, though freed before it, or in it: freed while open, and written anew
            // (TASK-79.1). It is no free of the footer's, which readers apply after its extents.
            self.freed.remove(&extent);
            self.touched.insert(extent, extents.get(&extent).expect("touched and held").clone());
        }
        if let Some(root) = root {
            self.root = Some(root.to_vec());
        }
    }

    /// Where the extents its commits touched are, as the footer of `segment`, the segment it is
    /// of, names them: each sealed in one run of `segment`, or in a file of its own, which the
    /// footer's chain names at its writer's key; those held elsewhere, a checkpoint names.
    fn places(&self, segment: SegmentId) -> Result<Places, Error> {
        let (mut extents, mut own) = (Vec::new(), Vec::new());
        for (extent, entry) in &self.touched {
            match &entry.data {
                Data::Pieces(pieces) => {
                    let [piece] = pieces[..] else { bail!("{extent:?} is not sealed in one run") };
                    ensure!(piece.segment == segment, "{extent:?} is sealed in another segment");
                    extents.push((*extent, piece.offset, entry.len));
                }
                Data::Own => own.push((*extent, entry.len)),
                Data::Remote => {}
            }
        }
        Ok((extents, own))
    }
}

/// Where a footer's extents are: runs in its segment, at an offset, of a length; and files of
/// their own, of a length.
type Places = (Vec<(ExtentId, u64, u64)>, Vec<(ExtentId, u64)>);

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
/// closed whose footers name commits after it, which were not durable: framed anew, with no
/// footer, from their raw files, which go once that is durable, so that a crash meanwhile leaves
/// the next opening a raw file whole. fxdax: a closed segment's raw file goes only once its framed
/// file and the commits its footer names are durable, with a commit after them (TASK-92.16): so
/// one whose footer names a commit not durable has its raw file still, holding what the durable
/// commits wrote to it, which their flushes synced; what its framed file holds besides is of
/// commits lost (TASK-79.33). Those whose raw files are gone it reads not.
pub(crate) fn close_left_open<D: Dir>(dir: &D, branch: u64, epoch: u64) -> Result<(), Error> {
    for number in list_numbers(dir, &branch_dir(OPEN_SEGMENTS, branch))? {
        let segment = SegmentId { branch, number };
        let (path, raw_path) = (segment_path(segment), open_segment_path(segment));
        // Closed for good if its framed file is whole, its footer naming durable commits alone:
        // by a commit made durable, or a closing between commits ([`Store::close_now`]). One an
        // opening before framed again, with none, is framed again, its raw file being there still.
        let closed = match dir.open(&path, true) {
            Ok(file) => read_frame(&file)?.is_some_and(|(_, footer)| {
                matches!(segment_footer(&footer), Ok(Some(footer)) if footer_last(&footer) <= epoch)
            }),
            Err(err) if not_found(&err) => false,
            Err(err) => return Err(err.into()),
        };
        if !closed {
            let data = read_whole(&dir.open(&raw_path, true)?)?;
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
        match dir.remove(&raw_path) {
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
/// framing them took: their data read, framed and written, off the commits that closed them, and
/// off the store's locks for those closed between commits
/// ([`Store::close_now`](crate::Store::close_now)'s).
pub fn segment_closes() -> (u64, u64) {
    let load =
        |count: &std::sync::atomic::AtomicU64| count.load(std::sync::atomic::Ordering::Relaxed);
    (load(&SEGMENT_CLOSES), load(&SEGMENT_CLOSE_NS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::read_file;
    use crate::layout::create_branch_dirs;
    use crate::{MemDir, MemFile};

    const BLOCK: u64 = 4096;
    const BRANCH: u64 = 7;

    fn segment(number: u64) -> SegmentId {
        SegmentId { branch: BRANCH, number }
    }

    /// The segments of a branch with none closed, `open_files` of whose files stay open.
    fn opened(dir: &MemDir, open_files: usize) -> Segments<MemFile> {
        create_branch_dirs(dir, BRANCH).unwrap();
        Segments::open(dir, BRANCH, BTreeMap::new(), 0, open_files).unwrap()
    }

    /// Writes a block of `byte`s to the open segment, which holds it, and returns its piece.
    fn write(segments: &mut Segments<MemFile>, byte: u8) -> Piece {
        let (number, file, at) = segments.reserve(BLOCK);
        file.write_at(&[byte; BLOCK as usize], at).unwrap();
        segments.held(number, BLOCK);
        Piece { segment: number, offset: at, extent_offset: 0, len: BLOCK }
    }

    /// Segment `number`'s data, as its file holds it.
    fn data(segments: &mut Segments<MemFile>, dir: &MemDir, number: SegmentId) -> Vec<u8> {
        match segments.source(dir, number).unwrap() {
            Source::Raw(file) => read_file(&*file).unwrap(),
            Source::Framed(framed) => {
                extent_frame::data(&read_whole(&*framed.file).unwrap()).unwrap()
            }
        }
    }

    /// A closed segment is read from its raw file while it is being framed, which its phase keeps
    /// open, and from its framed file once framed, which closes once idle: as more files are open
    /// than allowed, and no read uses it. The open segment's never does.
    #[test]
    fn closing_frames_through_framing_to_closed() {
        let dir = MemDir::new();
        let mut segments = opened(&dir, 1);
        write(&mut segments, 1);
        let Source::Raw(raw) = segments.source(&dir, segment(0)).unwrap() else {
            panic!("the open segment is read framed")
        };
        let unframed = segments.close(&dir, 1, 0).unwrap();
        // Two files open, the most one: but that being framed stays open.
        assert!(segments.is_framing(segment(0)));
        let Source::Raw(read) = segments.source(&dir, segment(0)).unwrap() else {
            panic!("a segment being framed is read framed")
        };
        assert!(Arc::ptr_eq(&read, &raw));
        drop((read, raw));

        let framed = unframed.frame(&dir).unwrap();
        segments.framed(segment(0), framed.clone());
        assert!(!segments.is_framing(segment(0)));
        let Source::Framed(read) = segments.source(&dir, segment(0)).unwrap() else {
            panic!("a segment framed is read raw")
        };
        assert!(Arc::ptr_eq(&read, &framed));
        let reading = framed.file.clone();
        let first = Arc::downgrade(&framed);
        drop((read, framed));

        // Read meanwhile, it stays open as the next closes.
        write(&mut segments, 2);
        let unframed = segments.close(&dir, 2, 0).unwrap();
        assert!(first.upgrade().is_some());
        drop(reading);
        segments.framed(segment(1), unframed.frame(&dir).unwrap());

        // Idle, it closed as the next was framed, the least lately read; that being framed and the
        // open one stay open.
        write(&mut segments, 3);
        let unframed = segments.close(&dir, 3, 0).unwrap();
        assert!(first.upgrade().is_none());
        assert!(matches!(segments.source(&dir, segment(2)).unwrap(), Source::Raw(_)));
        assert!(matches!(segments.source(&dir, segment(3)).unwrap(), Source::Raw(_)));
        // Opened again, framed.
        assert!(matches!(segments.source(&dir, segment(0)).unwrap(), Source::Framed(_)));
        let first_bytes = [0, 1, 2].map(|number| data(&mut segments, &dir, segment(number))[0]);
        assert_eq!(first_bytes, [1, 2, 3]);
        drop(unframed);
    }

    /// Footers are numbered as their segments are, each following the last closed since the
    /// branch was opened; given a writer's chain, numbered on from it, each following the one
    /// before, for its writer, the next sequence number they name its floor at least.
    #[test]
    fn footers_chain_from_the_segments_numbers_then_their_writers() {
        use extent_frame::segment::tag;
        let dir = MemDir::new();
        create_branch_dirs(&dir, BRANCH).unwrap();
        for number in [0, 1] {
            dir.create(&segment_path(segment(number)), true).unwrap();
        }
        let mut segments = Segments::open(&dir, BRANCH, BTreeMap::new(), 0, 64).unwrap();
        assert_eq!(segments.open_segment(), segment(2));
        let close = |segments: &mut Segments<MemFile>, epoch| {
            let footer = segments.close(&dir, epoch, 7).unwrap().footer;
            (footer.seq, footer.epoch, footer.after, footer.commits, footer.next_sequence)
        };
        assert_eq!(close(&mut segments, 1), (2, 0, None, vec![tag(0, 1)], 7));
        assert_eq!(close(&mut segments, 2), (3, 0, Some((2, 0)), vec![tag(0, 2)], 7));
        segments.set_chain(Chain::new(BRANCH, 3, 5, Some((4, 3)), 100));
        assert_eq!(close(&mut segments, 3), (5, 3, Some((4, 3)), vec![tag(3, 3)], 100));
        assert_eq!(close(&mut segments, 4), (6, 3, Some((5, 3)), vec![tag(3, 4)], 100));
    }

    /// Reaping lets go of the closed segments, and those inherited, no extent's data is in any
    /// more, which no write is in progress in: not the open segment, nor one being framed, nor
    /// those kept.
    #[test]
    fn reaping_keeps_open_framing_and_held_segments() {
        let dir = MemDir::new();
        let mut segments = opened(&dir, 64);
        let inherited = SegmentId { branch: BRANCH + 1, number: 4 };
        segments.track(inherited, || 0);
        // Written, its write abandoned: no extent is in it.
        let (number, _, _) = segments.reserve(BLOCK);
        segments.dropped(number, BLOCK);
        let empty = segments.close(&dir, 1, 0).unwrap();
        // Holding an extent.
        let held = write(&mut segments, 1);
        let holding = segments.close(&dir, 2, 0).unwrap();
        // A write in progress, which a closing between commits did not wait for.
        let (writing, _, _) = segments.reserve(BLOCK);
        let unwritten = segments.close(&dir, 3, 0).unwrap();
        let reap = |segments: &mut Segments<MemFile>, keep: &[SegmentId]| {
            let reaped = segments.reap(|number| keep.contains(&number));
            reaped.into_iter().map(|reaped| (reaped.number, reaped.last, reaped.len)).collect()
        };
        // Being framed, they stay.
        let reaped: Vec<_> = reap(&mut segments, &[]);
        assert_eq!(reaped, [(inherited, None, 0)]);
        for (number, unframed) in [(0, empty), (1, holding), (2, unwritten)] {
            segments.framed(segment(number), unframed.frame(&dir).unwrap());
        }
        let reaped: Vec<_> = reap(&mut segments, &[segment(0)]);
        assert_eq!(reaped, []);
        let reaped: Vec<_> = reap(&mut segments, &[]);
        assert_eq!(reaped, [(segment(0), Some(1), BLOCK)]);
        segments.let_go(&held);
        segments.dropped(writing, BLOCK);
        let reaped: Vec<_> = reap(&mut segments, &[]);
        assert_eq!(reaped, [(segment(1), Some(2), BLOCK), (segment(2), Some(3), BLOCK)]);
        assert_eq!(segments.tracked(), 1);
        assert!(segments.is_tracked(segments.open_segment()));
    }

    /// fxdax: a segment set aside, full, takes no more writes, which go to the next, and reads from
    /// its raw file; once no extent is in it, it is reaped, with no footer, and its raw file closes
    /// with it (TASK-79.2).
    #[test]
    fn segments_set_aside_go_once_no_extent_is_in_them() {
        let dir = MemDir::new();
        let mut segments = opened(&dir, 64);
        let first = write(&mut segments, 1);
        segments.retire(&dir).unwrap();
        assert_eq!(segments.open_segment(), segment(1));
        let Source::Raw(raw) = segments.source(&dir, segment(0)).unwrap() else {
            panic!("a segment set aside is read framed")
        };
        assert_eq!(data(&mut segments, &dir, segment(0))[0], 1);
        assert_eq!(write(&mut segments, 2).segment, segment(1));
        // Its phase holds its raw file open, as this does.
        assert_eq!(Arc::strong_count(&raw), 2);
        assert_eq!(segments.reap(|_| false).len(), 0);
        segments.let_go(&first);
        let reaped: Vec<_> = (segments.reap(|_| false).into_iter())
            .map(|reaped| (reaped.number, reaped.last, reaped.len))
            .collect();
        assert_eq!(reaped, [(segment(0), None, BLOCK)]);
        assert_eq!(Arc::strong_count(&raw), 1);
    }

    /// A commit syncs the open segment's file if it was written since the last commit, keeping
    /// its pages cached (TASK-92.23), and the directories of segments and of open ones if one
    /// closed, whose data its framed file holds.
    #[test]
    fn only_written_open_segments_are_synced() {
        let dir = MemDir::new();
        let mut segments = opened(&dir, 64);
        let syncs = |segments: &mut Segments<MemFile>| {
            let mut syncs = Syncs::default();
            segments.take_syncs(&mut syncs);
            let (files, cached) = syncs.files();
            ((files.len(), cached.len()), syncs.dirs().iter().cloned().collect::<Vec<_>>())
        };
        assert_eq!(syncs(&mut segments), ((0, 0), vec![]));
        write(&mut segments, 1);
        assert_eq!(syncs(&mut segments), ((0, 1), vec![]));
        assert_eq!(syncs(&mut segments), ((0, 0), vec![]));
        write(&mut segments, 2);
        segments.close(&dir, 1, 0).unwrap();
        let dirs = vec![branch_dir(OPEN_SEGMENTS, BRANCH), branch_dir(SEGMENTS, BRANCH)];
        assert_eq!(syncs(&mut segments), ((0, 0), dirs));
        segments.append(BLOCK);
        assert_eq!(syncs(&mut segments), ((0, 1), vec![]));
    }
}
