// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's segments (see the crate's docs): those it tracks, each where it is in its life
//! ([`Phase`]), with the bytes of extents' data it holds; the open segment's closing, its footer
//! of what the commits since it opened changed, each footer following the one before; fxdax: a
//! closed segment's data framed, with its footer, off the commit that closed it, its log of the
//! commits in it going with its raw data (TASK-165.9).

use crate::dir::{Dir, File};
use crate::durability::Syncs;
use crate::framed::{Aligned, Files, Framed, Source, read_parts};
use crate::layout::{
    Change, Data, EXTENTS, Extents, LOGS, OPEN_SEGMENTS, Piece, SEGMENTS, SegmentId, branch_dir,
    extent_path, held_frees, log_path, open_segment_path, segment_path,
};
use crate::store::{Options, Usage};
use crate::timings::{self, Phases, framing};
use anyhow::{Context as _, Error, anyhow, bail, ensure};
use extent_frame::segment::{Chain, Child, Contents, Footer, epoch_of};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Instant;
use storage_device::ExtentId;

/// Where a segment the branch tracks is in its life.
enum Phase<F> {
    /// Taking writes, in its raw file; fxdax: its commits recorded in its log, which the flushes
    /// append to (TASK-165.9).
    Open { raw: Arc<F>, log: Arc<F> },
    /// fxdax: closed by commit `last`, its data being framed off the commit, on a thread of its
    /// own, which ends the framing itself ([`Segments::framed`], TASK-153.8): read from its raw
    /// file, which stays open, until then; its commits' records still appended to its log by the
    /// flushes that make them durable, which wait for its framing only if it closed with data
    /// written since the commit before (TASK-165.9).
    Framing { raw: Arc<F>, log: Arc<F>, last: u64 },
    /// fxdax: its framing failed, which failed the store: read from its raw file, which stays.
    Failed { raw: Arc<F>, log: Arc<F>, error: String },
    /// Closed, a segment of the write-ahead log until acknowledged in the bucket. Its framed file,
    /// if open, is among those kept open (`Segments::files`).
    Closed,
    /// fxdax: set aside, full, though it held no commit's data: what writes since the last commit
    /// wrote to it is in its raw file, which no footer names, nor framing frames, and which goes
    /// once the next commit has copied the extents in it out (TASK-79.2).
    Retired(Arc<F>),
}

/// A segment the branch tracks.
struct Segment<F> {
    phase: Phase<F>,
    /// The bytes of its data.
    len: u64,
    /// The bytes its file takes.
    size: u64,
    /// The bytes of extents' data it holds: pieces of extents not freed.
    live: u64,
    /// The writes to it in progress, and the bytes they reserved: neither live nor dead yet.
    writers: u64,
    pending: u64,
    /// fxdax: the sealed extents the branch holds whose data is in it: what is read in the bucket
    /// once it is acknowledged there, found without going through every extent the branch holds
    /// (TASK-165.8).
    sealed: HashSet<ExtentId>,
    /// fxdax: its key in the bucket, once its uploader's writer acknowledged it there: it leaves
    /// the store for the host's cache of objects once no extent's data is in it here (TASK-165.8),
    /// if the object there is its framed file, not stamped anew after a takeover (TASK-165.9).
    acknowledged: Option<Option<String>>,
}

impl<F> Segment<F> {
    fn new(phase: Phase<F>, live: u64) -> Self {
        let (len, size, writers, pending) = (0, 0, 0, 0);
        Self {
            phase,
            len,
            size,
            live,
            writers,
            pending,
            sealed: HashSet::new(),
            acknowledged: None,
        }
    }

    /// A write of `len` bytes to it is done, `live` of them holding extents' data: returns
    /// whether none is in progress in it any more.
    fn done(&mut self, len: u64, live: u64) -> bool {
        self.writers -= 1;
        self.pending -= len;
        self.live += live;
        self.writers == 0
    }

    /// Whether reaping lets it go: no extent's data is in it, nor write in progress, and it takes
    /// none, set aside, or closed and acknowledged in the bucket: the write-ahead log holds the
    /// rest (TASK-165.9).
    fn is_empty(&self) -> bool {
        let done = match self.phase {
            Phase::Closed => self.acknowledged.is_some(),
            Phase::Retired(_) => true,
            _ => false,
        };
        done && self.live == 0 && self.writers == 0
    }

    /// fxdax: panics unless what it counts of itself, segment `number`, is what it holds
    /// (TASK-160.16): `pieces` of extents' data, and the sealed extents `sealed`.
    #[cfg(any(test, feature = "audit"))]
    fn audit(&self, number: SegmentId, pieces: &[Piece], sealed: &BTreeSet<ExtentId>) {
        for piece in pieces {
            assert!(piece.offset + piece.len <= self.len, "{piece:?} past {number:?}'s data");
        }
        let live: u64 = pieces.iter().map(|piece| piece.len).sum();
        assert_eq!(self.live, live, "the bytes of extents' data {number:?} holds");
        assert!(self.writers > 0 || self.pending == 0, "{number:?}, pending, unwritten");
        let seated: BTreeSet<ExtentId> = self.sealed.iter().copied().collect();
        assert_eq!(&seated, sealed, "the extents sealed in {number:?}");
        if !matches!(self.phase, Phase::Closed) {
            assert_eq!(self.size, self.len, "{number:?}'s raw file, its data");
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
    /// The forks the next commit makes at its end, which close the open segment.
    forking: Vec<Child>,
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
    /// fxdax: the segments closed since the last commit, written since the one before, whose
    /// framings the next commit's flush waits for, their framed files what makes that durable
    /// (TASK-165.9).
    closed_unsynced: Vec<SegmentId>,
    /// Whether a segment closed since the last commit, which syncs the directories of segments,
    /// the framed file's, and of open ones, the next's.
    closed_since: bool,
    /// fxdax: the segments whose framings are running, each of which ends itself (TASK-153.8).
    framing: BTreeSet<SegmentId>,
    /// fxdax: the segments framed whose raw files and logs are still there, each with the last
    /// commit its footer names: they go once that commit is durable, so that an opening finding it
    /// not durable closes the segment again from them (`wal::recover`, TASK-92.16, TASK-165.9).
    raw: Vec<(u64, SegmentId)>,
}

/// A segment the branch let go of, no extent being in it any more: set aside, its raw file going;
/// or fxdax: acknowledged in the bucket, its key there if its file is the object there, which goes
/// into the host's cache (TASK-165.8).
pub(crate) enum Reaped {
    Retired(SegmentId),
    Acknowledged(SegmentId, Option<String>),
}

impl<F: File> Segments<F> {
    /// The segments of `branch`, opened as of commit `epoch`: those of its write-ahead log,
    /// `tracked`, each with the bytes of extents' data it holds, whose files must be there; and
    /// another, open, `next`, past them, whose raw file and log are made, their entries durable.
    /// Its footer, and those after it, `chain` stamps. At most `open_files` of their files stay
    /// open between reads.
    pub fn open<D: Dir<File = F>>(
        dir: &D,
        (branch, tracked): (u64, BTreeMap<SegmentId, u64>),
        (epoch, next, chain): (u64, u64, Chain),
        open_files: usize,
    ) -> Result<Self, Error> {
        let mut segments = BTreeMap::new();
        for (id, live) in tracked {
            ensure!(id.branch == branch, "{id:?} is another branch's");
            let path = segment_path(id);
            let file = dir.open(&path, true).with_context(|| format!("segment {path}"))?;
            // fxdax: its trailer and table, a block, not its footer (TASK-92.16).
            let parts =
                read_parts(&file)?.ok_or_else(|| anyhow!("{path} is not a whole framed file"))?;
            let segment = Segment {
                len: parts.frame.data_len,
                size: parts.frame.size(),
                ..Segment::new(Phase::Closed, live)
            };
            segments.insert(id, segment);
        }
        let open = SegmentId { branch, number: next };
        let phase = create_open(dir, open)?;
        for kind in [OPEN_SEGMENTS, LOGS] {
            dir.sync_dir(&branch_dir(kind, branch))?;
        }
        segments.insert(open, Segment::new(phase, 0));
        Ok(Self {
            tracked: segments,
            empty: BTreeSet::new(),
            open,
            opened: Instant::now(),
            close_wanted: false,
            forking: Vec::new(),
            window: Window::starting(branch, epoch + 1),
            chain,
            files: Files::new(open_files),
            written: false,
            closed_unsynced: Vec::new(),
            closed_since: false,
            framing: BTreeSet::new(),
            raw: Vec::new(),
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

    /// Changes segment `number` as `change` says, keeping count of whether it is empty.
    fn change<T>(&mut self, number: SegmentId, change: impl FnOnce(&mut Segment<F>) -> T) -> T {
        let changed = change(self.tracked.get_mut(&number).unwrap());
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
            let Phase::Open { raw: file, .. } = &segment.phase else {
                unreachable!("the open segment is open")
            };
            let (file, at) = (file.clone(), segment.len);
            segment.len += len;
            segment.size += len;
            segment.writers += 1;
            segment.pending += len;
            (open, file, at)
        })
    }

    /// A write of `len` bytes to segment `number` is done, and the segment holds what it wrote,
    /// extents' data: returns whether none is in progress in it any more.
    pub fn held(&mut self, number: SegmentId, len: u64) -> bool {
        self.written |= number == self.open;
        self.change(number, |segment| segment.done(len, len))
    }

    /// A write of `len` bytes to segment `number` is done, and holds nothing: abandoned, or to an
    /// extent freed since it began; what it wrote is dead space. Returns whether none is in
    /// progress in the segment any more.
    pub fn dropped(&mut self, number: SegmentId, len: u64) -> bool {
        self.change(number, |segment| segment.done(len, 0))
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
            let Phase::Open { raw: file, .. } = &segment.phase else {
                unreachable!("the open segment is open")
            };
            let (file, at) = (file.clone(), segment.len);
            segment.len += len;
            segment.size += len;
            segment.live += len;
            (file, at)
        })
    }

    /// fxdax: segment `number`, closed, is acknowledged in the bucket, its object there at `key`
    /// if its framed file is that object (TASK-165.8): the extents sealed in it, which it returns,
    /// are read there from now on, and it leaves the store once no extent's data is in it here,
    /// the next reaping letting it go as such ([`Reaped::Acknowledged`]).
    pub fn acknowledge(&mut self, number: SegmentId, key: Option<&str>) -> Vec<ExtentId> {
        if !self.tracked.contains_key(&number) {
            return Vec::new();
        }
        self.change(number, |segment| {
            segment.acknowledged = Some(key.map(str::to_owned));
            segment.sealed.iter().copied().collect()
        })
    }

    /// fxdax: `extent`, sealed, holds `data` from now on: in the segments, if in any, which say so
    /// once acknowledged in the bucket (TASK-165.8).
    pub fn place(&mut self, extent: ExtentId, data: &Data) {
        self.seat(extent, data, true);
    }

    /// fxdax: `extent`, sealed, holds `data` no more: it was freed, or moved.
    pub fn displace(&mut self, extent: ExtentId, data: &Data) {
        self.seat(extent, data, false);
    }

    /// Notes that `extent`, sealed, is in the segments `data` names, if `seated`, or not.
    fn seat(&mut self, extent: ExtentId, data: &Data, seated: bool) {
        let Data::Pieces(pieces) = data else { return };
        for piece in pieces {
            let sealed = &mut self.tracked.get_mut(&piece.segment).unwrap().sealed;
            if seated {
                sealed.insert(extent);
            } else {
                sealed.remove(&extent);
            }
        }
    }

    /// The writes to the open segment in progress.
    pub fn open_writers(&self) -> u64 {
        self.tracked[&self.open].writers
    }

    /// Has the next commit close the open segment, whatever it holds.
    pub fn want_close(&mut self) {
        self.close_wanted = true;
    }

    /// Has the next commit fork `child` at its end, which its contents name, and close the open
    /// segment, whose footer names the fork (doc-7, Forks): with its owner, and whether it is a
    /// point (doc-7, Points).
    pub fn close_for_fork(&mut self, child: Child) {
        self.close_wanted = true;
        self.forking.push(child);
    }

    /// Whether the open segment holds commits up to `epoch`, the last, which closing it records.
    pub fn has_commits(&self, epoch: u64) -> bool {
        self.window.first <= epoch
    }

    /// fxdax: the log of the open segment's commits (TASK-165.9).
    pub fn open_log(&self) -> Arc<F> {
        let Phase::Open { log, .. } = &self.tracked[&self.open].phase else {
            unreachable!("the open segment is open")
        };
        log.clone()
    }

    /// fxdax: has the footers of the segments closed from now on stamped by `chain`, the chain of
    /// a writer that took the branch again, after the write-ahead log's last segment stamped anew
    /// (`Store::rebase`, TASK-204).
    pub fn set_chain(&mut self, chain: Chain) {
        self.chain = chain;
    }

    /// fxdax: the log of segment `number`'s commits, open or closed since, which a flush appends
    /// their records to: there until its raw file goes, once it is framed and they are durable
    /// ([`Segments::raw`]).
    pub fn log_file<D: Dir<File = F>>(&self, dir: &D, number: SegmentId) -> Result<Arc<F>, Error> {
        match self.tracked.get(&number).map(|segment| &segment.phase) {
            Some(
                Phase::Open { log, .. } | Phase::Framing { log, .. } | Phase::Failed { log, .. },
            ) => Ok(log.clone()),
            _ => Ok(Arc::new(dir.open(&log_path(number), false)?)),
        }
    }

    /// fxdax: the bytes of segment `number`'s data.
    pub fn len(&self, number: SegmentId) -> u64 {
        self.tracked.get(&number).map_or(0, |segment| segment.len)
    }

    /// fxdax: the bytes of the extents the open segment's commits sealed, in its data and in
    /// files of their own: what of the write-ahead log it holds, which closing it uploads, and not
    /// the writes no commit sealed yet, which no upload takes (TASK-165.9).
    pub fn open_bytes(&self) -> u64 {
        self.window.sealed.values().map(|(_, len)| len).sum()
    }

    /// Records commit `epoch`, which changed `changes`, and set its root and the branch's
    /// capacity, if it set them, `extents` the branch's extents as of it, its extents sealed in
    /// the open segment or in files of their own, `next_sequence` the branch's next sequence
    /// number: its contents, as the chain tags it, which it returns, fold into the open segment's
    /// footer to be ([`Window`]), with the forks it makes ([`Segments::close_for_fork`]).
    pub fn record(
        &mut self,
        epoch: u64,
        changes: &[Change],
        (root, capacity): (Option<&[u8]>, Option<u64>),
        extents: &Extents,
        next_sequence: u64,
    ) -> Result<Contents, Error> {
        let mut contents = commit_contents(self.open, changes, extents)?;
        contents.commits = vec![self.chain.tag(epoch)];
        contents.root = root.map(<[u8]>::to_vec);
        contents.capacity = capacity;
        contents.next_sequence = next_sequence;
        contents.forks = std::mem::take(&mut self.forking);
        self.window.fold(&contents);
        Ok(contents)
    }

    /// Whether the open segment is to close, as `options` say, once no write is in progress in
    /// it: full, old, wanted closed, or at a fork.
    pub fn due(&self, options: &Options) -> bool {
        let open = &self.tracked[&self.open];
        let old = open.len > 0 && self.opened.elapsed() >= options.segment_age;
        open.writers == 0 && (open.len >= options.segment_size || old || self.close_wanted)
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
        let next = SegmentId { number: number.number + 1, ..number };
        let phase = create_open(dir, next)?;
        // Nothing fails from here on.
        let window =
            std::mem::replace(&mut self.window, Window::starting(number.branch, epoch + 1));
        let footer = self.chain.close(window.contents(vec![self.chain.tag(epoch)], next_sequence));
        let segment = self.tracked.get_mut(&number).unwrap();
        let (raw, log) = match &segment.phase {
            Phase::Open { raw, log } => (raw.clone(), log.clone()),
            _ => unreachable!("the open segment is open"),
        };
        segment.phase = Phase::Framing { raw: raw.clone(), log, last: epoch };
        let len = segment.len;
        // What was written to it since the last commit is durable once it is framed, which the
        // next flush waits for, rather than write it back from its raw file too, as the framing
        // writes it: both at once, each flush's syncs waited for as long, up to 0.8 s under pgbench
        // (GCE r737, TASK-165.9).
        if std::mem::take(&mut self.written) {
            self.closed_unsynced.push(number);
        }
        self.tracked.insert(next, Segment::new(phase, 0));
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
        debug_assert!(
            self.window.is_empty(),
            "{number:?}, set aside, holds a commit's data or a fork"
        );
        let next = SegmentId { number: number.number + 1, ..number };
        let phase = create_open(dir, next)?;
        let segment = self.tracked.get_mut(&number).unwrap();
        let Phase::Open { raw, .. } = &segment.phase else {
            unreachable!("the open segment is open")
        };
        segment.phase = Phase::Retired(raw.clone());
        self.note(number);
        self.tracked.insert(next, Segment::new(phase, 0));
        self.open = next;
        self.opened = Instant::now();
        // What it holds, the next commit copies to the open segment: nothing of it is to be
        // synced, but the next's entry, with those of segments closed.
        self.written = false;
        self.closed_since = true;
        Ok(())
    }

    /// fxdax: segment `number`, closed and being framed, is in `framed` from now on, durable, its
    /// raw file no more, which goes once the commits its footer names are ([`Segments::raw`]).
    pub fn framed(&mut self, number: SegmentId, framed: Arc<Framed<F>>) {
        let mut closed = 0;
        self.change(number, |segment| {
            let Phase::Framing { last, .. } = segment.phase else {
                unreachable!("{number:?}, framed, was being framed")
            };
            segment.size = framed.size();
            segment.phase = Phase::Closed;
            closed = last;
        });
        self.files.insert(number, framed);
        self.raw.push((closed, number));
        self.framing.remove(&number);
    }

    /// fxdax: segment `number`'s framing failed, for `error`, which failed the store: it is read
    /// from its raw file, which stays.
    pub fn framing_failed(&mut self, number: SegmentId, error: String) {
        let segment = self.tracked.get_mut(&number).expect("a segment being framed is tracked");
        let Phase::Framing { raw, log, .. } = &segment.phase else {
            unreachable!("{number:?}, whose framing failed, was being framed")
        };
        segment.phase = Phase::Failed { raw: raw.clone(), log: log.clone(), error };
        self.framing.remove(&number);
    }

    /// fxdax: segment `number`, closed, is being framed from now on, by a framing that ends itself
    /// ([`Segments::framed`], [`Segments::framing_failed`]).
    pub fn framing_begun(&mut self, number: SegmentId) {
        debug_assert!(self.is_framing(number), "{number:?}, being framed, was closed");
        self.framing.insert(number);
    }

    /// fxdax: whether segment `number` is being framed.
    pub fn is_framing(&self, number: SegmentId) -> bool {
        matches!(self.tracked.get(&number), Some(Segment { phase: Phase::Framing { .. }, .. }))
    }

    /// fxdax: why segment `number`'s framing failed, if it did.
    pub fn framing_failure(&self, number: SegmentId) -> Option<&str> {
        match self.tracked.get(&number) {
            Some(Segment { phase: Phase::Failed { error, .. }, .. }) => Some(error),
            _ => None,
        }
    }

    /// fxdax: the segments whose framings are running.
    pub fn framings(&self) -> &BTreeSet<SegmentId> {
        &self.framing
    }

    /// fxdax: the paths of the raw files, and logs, of the segments framed whose footers name no
    /// commit past `durable`, the last durable, which may go: an opening reads their framed files
    /// (TASK-153.8, TASK-165.9).
    pub fn raw(&mut self, durable: u64) -> Vec<String> {
        let (going, staying) = std::mem::take(&mut self.raw)
            .into_iter()
            .partition::<Vec<_>, _>(|(last, _)| *last <= durable);
        self.raw = staying;
        let paths = |(_, number)| [open_segment_path(number), log_path(number)];
        going.into_iter().flat_map(paths).collect()
    }

    /// The file of segment `number`, opened in `dir` if it isn't: the raw file
    /// of the open segment, or of one being framed or set aside; a closed one's framed file.
    pub fn source<D: Dir<File = F>>(
        &mut self,
        dir: &D,
        number: SegmentId,
    ) -> Result<Source<F>, Error> {
        let segment = self.tracked.get(&number).ok_or_else(|| anyhow!("no segment {number:?}"))?;
        match &segment.phase {
            Phase::Open { raw, .. }
            | Phase::Framing { raw, .. }
            | Phase::Failed { raw, .. }
            | Phase::Retired(raw) => {
                return Ok(Source::Raw(raw.clone()));
            }
            Phase::Closed => {}
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
    /// directories of segments, of open ones and of their logs.
    pub fn take_syncs(&mut self, syncs: &mut Syncs<F>) {
        if std::mem::take(&mut self.written) {
            let Phase::Open { raw, .. } = &self.tracked[&self.open].phase else {
                unreachable!("the open segment is open")
            };
            syncs.cached(raw.clone());
        }
        for number in self.closed_unsynced.drain(..) {
            syncs.framing(number);
        }
        if std::mem::take(&mut self.closed_since) {
            for kind in [SEGMENTS, OPEN_SEGMENTS, LOGS] {
                syncs.dir(branch_dir(kind, self.open.branch));
            }
        }
    }

    /// Lets go of the segments no extent's data is in any more, set aside, or closed and
    /// acknowledged in the bucket, which no write is in progress in, and returns them.
    pub fn reap(&mut self) -> Vec<Reaped> {
        let dead: Vec<SegmentId> = self.empty.iter().copied().collect();
        let mut reaped = Vec::with_capacity(dead.len());
        for number in dead {
            self.empty.remove(&number);
            let segment = self.tracked.remove(&number).unwrap();
            self.files.remove(number);
            reaped.push(match segment.acknowledged {
                Some(key) => Reaped::Acknowledged(number, key),
                None => Reaped::Retired(number),
            });
        }
        reaped
    }

    /// What the segments take: how many there are, the bytes of their data, of extents' data, and
    /// of their files; none of what the extents stored on their own take.
    pub fn usage(&self) -> Usage {
        let all = || self.tracked.values();
        Usage {
            segments: all().count() as u64,
            segment_bytes: all().map(|segment| segment.len).sum(),
            live_bytes: all().map(|segment| segment.live).sum(),
            own_bytes: 0,
            disk_bytes: all().map(|segment| segment.size).sum(),
            kept_bytes: 0,
            kept_for_views: 0,
        }
    }

    /// fxdax: panics unless what it counts of the segments it tracks is what `recount` says they
    /// hold, as the branch holds them; and it tracks every segment the branch holds extents in
    /// (TASK-160.16).
    #[cfg(any(test, feature = "audit"))]
    pub fn audit(&self, recount: &Recount) {
        let none = (Vec::new(), BTreeSet::new());
        for (number, segment) in &self.tracked {
            let (pieces, sealed) = recount.held.get(number).unwrap_or(&none);
            assert_eq!(self.empty.contains(number), segment.is_empty(), "{number:?}, empty");
            segment.audit(*number, pieces, sealed);
        }
        assert!(self.empty.iter().all(|number| self.tracked.contains_key(number)));
        for number in recount.held.keys() {
            assert!(self.tracked.contains_key(number), "{number:?}, held, untracked");
        }
    }
}

/// fxdax: what each segment holds, recounted from what the branch holds, for [`Segments::audit`]
/// (TASK-160.16): the pieces of extents' data in it, of the extents the branch holds, open or
/// sealed, and of those it keeps, freed; and the extents sealed in it.
#[cfg(any(test, feature = "audit"))]
#[derive(Default)]
pub(crate) struct Recount {
    held: BTreeMap<SegmentId, (Vec<Piece>, BTreeSet<ExtentId>)>,
}

#[cfg(any(test, feature = "audit"))]
use crate::layout::Extent;

#[cfg(any(test, feature = "audit"))]
impl Recount {
    /// The branch holds `extent` as `entry` says, sealed if `sealed`, or open.
    pub fn holds(&mut self, extent: ExtentId, entry: &Extent, sealed: bool) {
        let Data::Pieces(pieces) = &entry.data else { return };
        for piece in pieces {
            let (held, seated) = self.held.entry(piece.segment).or_default();
            held.push(*piece);
            if sealed {
                seated.insert(extent);
            }
        }
    }

    /// The branch keeps `entry`'s data, freed.
    pub fn keeps(&mut self, entry: &Extent) {
        let Data::Pieces(pieces) = &entry.data else { return };
        for piece in pieces {
            self.held.entry(piece.segment).or_default().0.push(*piece);
        }
    }
}

/// fxdax: a segment just closed, whose data is yet to be framed, with `footer`
/// ([`Unframed::frame`]): off the commit that closed it, on a thread of its own, whose framing
/// ends itself (TASK-153.8).
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

    /// Frames it in its framed file in `dir`, made durable with its entry and the directory of
    /// raw files', and returns that: the commit that closed it, whose data is in it, is durable
    /// once it is, and its raw file may go once the commits its footer names are too. fxdax: the
    /// files of the extents stored on their own its footer names are durable first, so that a
    /// framed file a crash leaves whole names none torn (TASK-165.9).
    pub fn frame<D: Dir<File = F>>(self, dir: &D) -> Result<Arc<Framed<F>>, Error> {
        // fxdax: timed, phase by phase (TASK-165.9).
        let mut phases = Phases::default();
        let started = Instant::now();
        // The extents stored on their own it names, durable before it is, as a flush syncs them,
        // several at once: one at a time, each a journal commit of the host's file system's, they
        // held up for as long the syncs of the flushes of extentd's turns, which wait on the same
        // files, up to 0.9 s under pgbench (GCE r730, TASK-165.9).
        let mut own = Syncs::default();
        for (extent, _) in self.footer.own() {
            own.file(Arc::new(dir.open(&extent_path(extent), true)?));
            own.dir(branch_dir(EXTENTS, self.number.branch));
        }
        own.sync(dir)?;
        let synced = phases.took(framing::OWN, started);
        let footer = self.footer.encode();
        let framed = frame_segment(dir, &segment_path(self.number), &self.raw, self.len, &footer)?;
        let written = phases.took(framing::WRITE, synced);
        framed.file.sync()?;
        let synced = phases.took(framing::SYNC, written);
        for kind in [SEGMENTS, OPEN_SEGMENTS] {
            dir.sync_dir(&branch_dir(kind, self.number.branch))?;
        }
        dir.barrier()?;
        phases.took(framing::DIRS, synced);
        timings::framed(self.number, phases);
        Ok(framed)
    }
}

/// fxdax: the bytes of a framed file a framing writes back at a time ([`File::write_back`]): what
/// a sync behind it, a daemon's turn's, waits for at most, as the host's disk writes them.
pub(crate) const WRITTEN_BACK: usize = 1 << 20;

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
    raw.read_cached(data.as_mut_slice(), 0)?;
    let bytes = extent_frame::framed(data.as_slice(), footer);
    let file = dir.create(path, true)?;
    // Where the framed file is in its directory, not yet written: whole only once its framing
    // ends, which its store says ([`Store::framing`](crate::Store::framing), TASK-189).
    fail::fail_point!("segments::framing");
    // fxdax: a piece at a time, each written back before the next (TASK-165.9).
    for (piece, written) in bytes.chunks(WRITTEN_BACK).enumerate() {
        let offset = (piece * WRITTEN_BACK) as u64;
        file.write_at(written, offset)?;
        file.write_back(offset, written.len() as u64)?;
    }
    let framed = Framed::new(Arc::new(file), extent_frame::Parts::of(&bytes)?);
    SEGMENT_CLOSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = started.elapsed().as_nanos() as u64;
    SEGMENT_CLOSE_NS.fetch_add(nanos, std::sync::atomic::Ordering::Relaxed);
    Ok(Arc::new(framed))
}

/// What commit `changes` made, as the footer of `segment`, the open segment, names it: the extents
/// it sealed or moved, which the branch holds as of it, `extents`, each in one run of `segment`
/// or in a file of its own, the branch's, by its sequence number; and of its frees, those of
/// extents a commit held, which the store spent, if its own: so that the footer's readers spend
/// all it frees, as the store did (TASK-160.15). Those of extents written in it, open, it frees
/// not: no commit held them, nor names them, and the store writes them again. Those held
/// elsewhere, in the bucket, it names not. The rest of its contents are its caller's.
fn commit_contents(
    segment: SegmentId,
    changes: &[Change],
    extents: &Extents,
) -> Result<Contents, Error> {
    let held = held_frees(changes);
    let (mut touched, mut frees) = (BTreeSet::new(), BTreeSet::new());
    for change in changes {
        match change {
            Change::Write { extent, .. }
            | Change::WriteOwn { extent, .. }
            | Change::Move { extent, .. }
            | Change::MoveOwn { extent } => {
                touched.insert(*extent);
            }
            Change::Free(extent) => {
                touched.remove(extent);
                if held.contains(extent) {
                    frees.insert(*extent);
                }
            }
        }
    }
    let mut contents = Contents { frees: frees.into_iter().collect(), ..Contents::default() };
    for extent in touched {
        let entry = extents.get(&extent).expect("touched and held");
        match &entry.data {
            Data::Pieces(pieces) => {
                let [piece] = pieces[..] else { bail!("{extent:?} is not sealed in one run") };
                ensure!(piece.segment == segment, "{extent:?} is sealed in another segment");
                contents.extents.push((extent, piece.offset, entry.len));
            }
            Data::Own => {
                ensure!(extent.branch == segment.branch, "{extent:?} is another branch's");
                contents.own.push((extent.sequence, entry.len));
            }
            Data::Remote(_) => {}
        }
    }
    Ok(contents)
}

/// What the commits since a segment opened made, as its footer says: their contents
/// ([`commit_contents`]) folded, each over those before, so that applying it to the branch's state
/// as of any of those commits gives its state as of the last. fxdax: one fold, of the commits as
/// they are made and of the records of a segment's log of them alike (TASK-165.9).
#[derive(Default)]
struct Window {
    /// Its first commit's epoch.
    first: u64,
    /// The branch whose extents are stored on their own.
    branch: u64,
    /// The extents its commits sealed, which the branch holds as of its last, where they sealed
    /// them: in a run of its segment's data, at an offset, of a length, or on their own, of a
    /// length.
    sealed: BTreeMap<ExtentId, (Option<u64>, u64)>,
    /// The extents they freed.
    frees: BTreeSet<ExtentId>,
    /// The root and capacity they set last, if any did, and the forks at their ends.
    root: Option<Vec<u8>>,
    capacity: Option<u64>,
    forks: Vec<Child>,
}

impl Window {
    fn starting(branch: u64, first: u64) -> Self {
        Self { first, branch, ..Self::default() }
    }

    /// Whether its commits made nothing.
    fn is_empty(&self) -> bool {
        self.sealed.is_empty()
            && self.frees.is_empty()
            && self.root.is_none()
            && self.capacity.is_none()
            && self.forks.is_empty()
    }

    /// Folds `commit`'s contents over those of the commits before it: its frees take out what
    /// they sealed, and are the footer's; what it seals is no free of the footer's, though freed
    /// before, as readers apply its frees after its extents (TASK-79.1).
    fn fold(&mut self, commit: &Contents) {
        for extent in &commit.frees {
            self.sealed.remove(extent);
            self.frees.insert(*extent);
        }
        let own = commit
            .own
            .iter()
            .map(|&(sequence, len)| (ExtentId::new(self.branch, sequence), (None, len)));
        let runs =
            commit.extents.iter().map(|&(extent, offset, len)| (extent, (Some(offset), len)));
        for (extent, sealed) in runs.chain(own).collect::<Vec<_>>() {
            self.frees.remove(&extent);
            self.sealed.insert(extent, sealed);
        }
        if commit.root.is_some() {
            self.root.clone_from(&commit.root);
        }
        if commit.capacity.is_some() {
            self.capacity = commit.capacity;
        }
        self.forks.extend_from_slice(&commit.forks);
    }

    /// The footer's contents, of `commits`, naming `next_sequence`.
    fn contents(self, commits: Vec<u64>, next_sequence: u64) -> Contents {
        let (mut extents, mut own) = (Vec::new(), Vec::new());
        for (extent, (offset, len)) in self.sealed {
            match offset {
                Some(offset) => extents.push((extent, offset, len)),
                None => own.push((extent.sequence, len)),
            }
        }
        Contents {
            commits,
            extents,
            own,
            frees: self.frees.into_iter().collect(),
            root: self.root,
            next_sequence,
            forks: self.forks,
            capacity: self.capacity,
        }
    }
}

/// fxdax: the contents of a segment of `branch`'s whose commits' contents are `commits`, in order,
/// as its footer names them: their last commit, and the largest next sequence number they name
/// (TASK-165.9). The one fold of a segment's commits ([`Window`]), of a log of them as of the
/// commits as they are made.
pub(crate) fn fold(branch: u64, commits: &[Contents]) -> Contents {
    let mut window = Window::starting(branch, 0);
    for commit in commits {
        window.fold(commit);
    }
    let last = commits.last().map_or(Vec::new(), |last| last.commits.clone());
    let next_sequence = commits.iter().map(|commit| commit.next_sequence).max().unwrap_or(0);
    window.contents(last, next_sequence)
}

/// fxdax: the phase of a new open segment, `segment`, whose raw file and log it makes in `dir`.
fn create_open<D: Dir>(dir: &D, segment: SegmentId) -> Result<Phase<D::File>, Error> {
    let raw = Arc::new(dir.create(&open_segment_path(segment), true)?);
    let log = Arc::new(dir.create(&log_path(segment), false)?);
    Ok(Phase::Open { raw, log })
}

/// The store's epoch of the last commit of the segment whose footer is `footer`.
pub(crate) fn footer_last(footer: &Footer) -> u64 {
    footer.contents.commits.last().map_or(0, |tag| epoch_of(*tag))
}

/// The footer of a closed segment, `bytes`: none if the segment closed with none, as one whose
/// footer named commits not durable, which opening its branch closes again.
pub(crate) fn segment_footer(bytes: &[u8]) -> Result<Option<Footer>, Error> {
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(Footer::decode(bytes).context("a segment's footer")?))
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
    use crate::framed::read_whole;
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
        let chain = Chain::new(BRANCH, 0, 0, None, 0);
        Segments::open(dir, (BRANCH, BTreeMap::new()), (0, 0, chain), open_files).unwrap()
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

    /// Footers are stamped by the writer's chain the segments opened with, numbered on from it,
    /// each following the one before, of its writer's epoch, the next sequence number they name
    /// its floor at least; the segments are numbered on from the number they opened with, whatever
    /// the chain's (TASK-165.9).
    #[test]
    fn footers_chain_from_their_writers_chain() {
        use extent_frame::segment::tag;
        let dir = MemDir::new();
        create_branch_dirs(&dir, BRANCH).unwrap();
        let chain = Chain::new(BRANCH, 3, 5, Some((4, 3)), 100);
        let segments = Segments::open(&dir, (BRANCH, BTreeMap::new()), (0, 2, chain), 64);
        let mut segments = segments.unwrap();
        assert_eq!(segments.open_segment(), segment(2));
        let close = |segments: &mut Segments<MemFile>, epoch| {
            let footer = segments.close(&dir, epoch, 7).unwrap().footer;
            (
                footer.seq,
                footer.epoch,
                footer.after,
                footer.contents.commits,
                footer.contents.next_sequence,
            )
        };
        assert_eq!(close(&mut segments, 3), (5, 3, Some((4, 3)), vec![tag(3, 3)], 100));
        assert_eq!(close(&mut segments, 4), (6, 3, Some((5, 3)), vec![tag(3, 4)], 100));
        assert_eq!(segments.open_segment(), segment(4));
    }

    /// Reaping lets go of the closed segments acknowledged in the bucket no extent's data is in any
    /// more, which no write is in progress in: not the open segment, nor one being framed, nor one
    /// the write-ahead log holds, not yet acknowledged (TASK-165.9).
    #[test]
    fn reaping_keeps_open_framing_and_unacknowledged_segments() {
        let dir = MemDir::new();
        let mut segments = opened(&dir, 64);
        // Written, its write abandoned: no extent is in it.
        let (number, ..) = segments.reserve(BLOCK);
        segments.dropped(number, BLOCK);
        let empty = segments.close(&dir, 1, 0).unwrap();
        // Holding an extent.
        let held = write(&mut segments, 1);
        let holding = segments.close(&dir, 2, 0).unwrap();
        // A write in progress, which a closing between commits did not wait for.
        let (writing, ..) = segments.reserve(BLOCK);
        let unwritten = segments.close(&dir, 3, 0).unwrap();
        let reap = |segments: &mut Segments<MemFile>| {
            let reaped = segments.reap().into_iter();
            reaped
                .map(|reaped| match reaped {
                    Reaped::Acknowledged(number, key) => (number, key),
                    Reaped::Retired(number) => panic!("{number:?} set aside"),
                })
                .collect::<Vec<_>>()
        };
        for (number, unframed) in [(0, empty), (1, holding), (2, unwritten)] {
            segments.framed(segment(number), unframed.frame(&dir).unwrap());
        }
        // Not acknowledged, they stay.
        assert_eq!(reap(&mut segments), []);
        for number in 0..3 {
            let key = (number != 1).then_some("key");
            segments.acknowledge(segment(number), key);
        }
        assert_eq!(reap(&mut segments), [(segment(0), Some("key".to_owned()))]);
        segments.let_go(&held);
        segments.dropped(writing, BLOCK);
        let reaped = reap(&mut segments);
        assert_eq!(reaped, [(segment(1), None), (segment(2), Some("key".to_owned()))]);
        for number in 0..3 {
            assert!(!segments.tracked.contains_key(&segment(number)), "{number} tracked still");
        }
        assert!(segments.tracked.contains_key(&segments.open_segment()));
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
        assert_eq!(segments.reap().len(), 0);
        segments.let_go(&first);
        let reaped: Vec<_> = (segments.reap().into_iter())
            .map(|reaped| match reaped {
                Reaped::Retired(number) => number,
                Reaped::Acknowledged(number, _) => panic!("{number:?} acknowledged"),
            })
            .collect();
        assert_eq!(reaped, [segment(0)]);
        assert_eq!(Arc::strong_count(&raw), 1);
    }

    /// A commit syncs the open segment's file if it was written since the last commit, keeping
    /// its pages cached (TASK-92.23), and the directories of segments and of open ones if one
    /// closed; fxdax: and, of one closed, if written since, its framing, which makes that durable
    /// (TASK-165.9).
    #[test]
    fn only_written_open_segments_are_synced() {
        let dir = MemDir::new();
        let mut segments = opened(&dir, 64);
        let syncs = |segments: &mut Segments<MemFile>| {
            let mut syncs = Syncs::default();
            segments.take_syncs(&mut syncs);
            let (files, cached) = syncs.files();
            let dirs = syncs.dirs().iter().cloned().collect::<Vec<_>>();
            ((files.len(), cached.len()), dirs, syncs.framings().to_vec())
        };
        assert_eq!(syncs(&mut segments), ((0, 0), vec![], vec![]));
        write(&mut segments, 1);
        assert_eq!(syncs(&mut segments), ((0, 1), vec![], vec![]));
        assert_eq!(syncs(&mut segments), ((0, 0), vec![], vec![]));
        write(&mut segments, 2);
        let closed = segments.open_segment();
        segments.close(&dir, 1, 0).unwrap();
        let dirs = vec![
            branch_dir(LOGS, BRANCH),
            branch_dir(OPEN_SEGMENTS, BRANCH),
            branch_dir(SEGMENTS, BRANCH),
        ];
        assert_eq!(syncs(&mut segments), ((0, 0), dirs, vec![closed]));
        segments.append(BLOCK);
        assert_eq!(syncs(&mut segments), ((0, 1), vec![], vec![]));
    }
}
