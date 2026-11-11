// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: a host's write-ahead log of a branch (doc-14): its closed segments not yet
//! acknowledged, each stamped by its writer's chain, the very object it is to be in the bucket. A
//! segment is in the log before its upload starts, so that every segment that may land in the
//! writer's name is the log's; one leaves it once acknowledged.
//!
//! A writer taking the branch over, here after a crash or elsewhere, puts the log after the
//! branch's end ([`Wal::rebase`]), by commits: the log keeps its base, the branch's last commit
//! before its first segment, and the branch's last commit, after the writer's fence, says how far
//! the log landed. If it is one of the log's, the log's segments to it are the branch's; if it is
//! that one, or the base, the rest are stamped again on the new writer's chain, after its fence;
//! fxdax: if it ends in a segment of the log's going on with the log's next commit, that segment's
//! continuation is stamped again too, alone, before the rest (TASK-345); otherwise another
//! writer's commits moved the branch on, and the log diverged
//! ([`Error::Diverged`]): its commits, never acknowledged, are lost once `fxdaxctl take` drops it.
//! By commits, not by the footers recovery read: a checkpoint covering the log's segments, landed,
//! or a fence alone coming after them, takes nothing from the guest (TASK-165.2).

use crate::layout::{Chain, Contents, Footer};
use crate::{Error, rules};
use std::collections::VecDeque;

/// Where a writer taking a branch over found the branch's end ([`Writer::fenced`]): its last commit
/// as recovery found it, and after the writer's fence, which applied what landed at the end
/// meanwhile; and the writer's chain then, where its first segment goes. What a host's log is put
/// after ([`Wal::rebase`]).
///
/// [`Writer::fenced`]: crate::Writer::fenced
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fenced {
    pub recovered: Option<u64>,
    pub last_commit: Option<u64>,
    pub chain: Chain,
}

/// What putting a host's log after the end of a branch took out of it ([`Wal::rebase`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Rebased<T> {
    /// The segments the branch holds, landed, by their footers, with what the host keeps of each.
    pub landed: Vec<(Footer, T)>,
    /// fxdax: the footer, as it landed, of the log's first segment left, if the branch's last
    /// commit is the last that segment ends and it goes on with the next: the branch holds the
    /// commits it ends, and the log keeps it, stamped again with its continuation alone
    /// (TASK-345).
    pub ended: Option<Footer>,
}

/// A host's write-ahead log of a branch: its base, the branch's last commit before its first
/// segment, which the guest's history goes on from; then its segments, in order, by their footers,
/// with what the host keeps of each, `T`: its data, or the files holding it.
#[derive(Clone, Debug)]
pub struct Wal<T> {
    base: Option<u64>,
    segments: VecDeque<(Footer, T)>,
}

impl<T> Wal<T> {
    /// An empty log, whose next segment goes on from `base`.
    pub fn new(base: Option<u64>) -> Self {
        Self { base, segments: VecDeque::new() }
    }

    /// fxdax: the log a host's disk held, after `base`, of `segments`, in order, stamped as they
    /// were, which needn't follow each other: what follows the branch is the log's commits, and
    /// [`Wal::rebase`] stamps it anew (TASK-165.9).
    pub fn of(base: Option<u64>, segments: impl IntoIterator<Item = (Footer, T)>) -> Self {
        Self { base, segments: segments.into_iter().collect() }
    }

    /// The branch's last commit before its first segment.
    pub fn base(&self) -> Option<u64> {
        self.base
    }

    /// Its segments, in order.
    pub fn segments(&self) -> impl ExactSizeIterator<Item = &(Footer, T)> {
        self.segments.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The guest's last commit: the last its segments make, or its base.
    pub fn head(&self) -> Option<u64> {
        self.heads().last().copied().flatten()
    }

    /// Adds a segment its writer closed, which follows the last.
    pub fn push(&mut self, footer: Footer, data: T) {
        if let Some((last, _)) = self.segments.back() {
            debug_assert_eq!(footer.after, Some((last.seq, last.epoch)), "a log's segments follow");
        }
        self.segments.push_back((footer, data));
    }

    /// Takes out its segments to number `through`, acknowledged, and returns them: its base is
    /// then the last commit they make.
    pub fn acknowledge(&mut self, through: u64) -> Vec<(Footer, T)> {
        let count = self.segments.iter().take_while(|(footer, _)| footer.seq <= through).count();
        self.take(count)
    }

    /// Takes out its first `count` segments, and returns them: its base is then the last commit
    /// they make.
    fn take(&mut self, count: usize) -> Vec<(Footer, T)> {
        self.base = self.heads()[count];
        self.segments.drain(..count).collect()
    }

    /// The guest's last commit as of each of its first `n` segments, `n` from none to all: its
    /// base, then the last commit each makes, or the one before's, for a segment that makes none
    /// (its fork's alone).
    fn heads(&self) -> Vec<Option<u64>> {
        let mut head = self.base;
        let mut heads = vec![head];
        for (footer, _) in &self.segments {
            head = footer.contents.commits.last().copied().or(head);
            heads.push(head);
        }
        heads
    }

    /// Puts the log after the end of the branch a writer took over, as `fenced` says it found it,
    /// by commits: takes out and returns the segments the branch holds, those to the branch's last
    /// commit, if it is one of the log's (`wal_follows`), and stamps the rest again on the writer's
    /// chain, after its fence, their data as it was and their extents of their own at the writer's
    /// keys; the writer then takes them as it takes any segment it closed. The branch's last commit
    /// is the one after the fence (`wal_after_fence`): the log's own segments may have landed since
    /// recovery found the end, which the fence applied. The log's base is then the branch's last
    /// commit after the fence, which what it stamped again follows.
    ///
    /// A segment making no commit, a fork's alone, the branch's last commit doesn't place: the last
    /// that commit may be the branch's through is taken as landed. If it did not, its fork, never
    /// acknowledged, is lost, as a fork whose host lost its disk is; the other way, the branch would
    /// have it twice.
    ///
    /// fxdax: a segment ending commits and going on with the next (a segment filled after a commit,
    /// before the next, `Contents::continued`, TASK-92.41) is no branch's end either, the fence
    /// having left the next unended: if it landed, ending the branch's last commit, those after it
    /// not, the branch holds the commits it ends and none of its continuation. It stays the log's,
    /// stamped again with its continuation alone, its data as it was, the commits it ends, their
    /// extents, frees, root, capacity and forks dropped, the branch having them; the rest after it
    /// ([`Rebased::ended`], TASK-345). Before, the log diverged, though no other writer moved the
    /// branch on, and `fxdaxctl take` dropped it, the next commit with it, durable on the host.
    /// Taken as landed, the next commit would lose its runs in it; stamped again whole, the branch
    /// would apply the commits it ends twice.
    ///
    /// Fails with [`Error::Diverged`], the log unchanged, if another writer's commits moved the
    /// branch on: its last commit is neither one of the log's nor its base. An empty log never
    /// diverges: taking a branch with one is how a host takes it, wherever it was.
    pub fn rebase(&mut self, fenced: &Fenced) -> Result<Rebased<T>, Error> {
        let rules = rules();
        let last = if rules.wal_after_fence { fenced.last_commit } else { fenced.recovered };
        // fxdax: none of a commit spanning segments, but those to the one it ends in: the segments
        // going on with it are no branch's end, the fence having left their commit unended, never
        // acknowledged (TASK-92.41).
        let between =
            |landed: usize| landed == 0 || self.segments[landed - 1].0.contents.continued.is_none();
        let heads = self.heads();
        let landed = heads.iter().enumerate().rposition(|(at, &head)| head == last && between(at));
        // fxdax: the segment ending the branch's last commit, going on with the next.
        let ending = || {
            let ends = |contents: &Contents| {
                last.is_some()
                    && contents.commits.last() == last.as_ref()
                    && contents.continued.is_some()
            };
            self.segments.iter().position(|(footer, _)| ends(&footer.contents))
        };
        let (landed, ended) = match landed {
            Some(landed) => (landed, false),
            None if self.segments.is_empty() || !rules.wal_follows => (0, false),
            None => match ending() {
                Some(ending) if rules.wal_continues => (ending, true),
                _ => return Err(Error::Diverged),
            },
        };
        let landed = self.take(landed);
        let ended = ended.then(|| {
            let (footer, _) = self.segments.front_mut().expect("the segment ending the branch");
            let Contents { continued, next_sequence, .. } = footer.contents.clone();
            let continuation = Contents { continued, next_sequence, ..Contents::default() };
            std::mem::replace(footer, Footer { contents: continuation, ..footer.clone() })
        });
        let mut chain = fenced.chain;
        for (footer, _) in &mut self.segments {
            *footer = chain.close(footer.contents.clone());
        }
        self.base = fenced.last_commit;
        Ok(Rebased { landed, ended })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{Batch, Commit, Data, Unframed};
    use crate::layout::{self, Contents, Continued, child};
    use crate::recovery::State;
    use crate::store::Store;
    use crate::{Writer, branch, bucket};
    use object_store::memory::InMemory;
    use std::sync::Arc;
    use storage_device::ExtentId;

    fn memory() -> Arc<Store> {
        Arc::new(Store::new(Arc::new(InMemory::new()), ""))
    }

    /// A bucket holding branch 1, a root.
    async fn bucket() -> Arc<Store> {
        let bucket = memory();
        branch::create(&*bucket, 1, 0).await.unwrap();
        bucket
    }

    /// A writer of branch 1, a host's, taking it over as `holder`.
    async fn open(bucket: &Arc<Store>, holder: u64) -> Writer<Store> {
        Writer::open(bucket.clone(), 1, holder, 0).await.unwrap()
    }

    /// An empty log of the branch `writer` just took.
    fn log(writer: &Writer<Store>) -> Wal<Data> {
        Wal::new(writer.fenced().last_commit)
    }

    fn commit(tag: u64) -> Commit {
        Commit { tag, ..Commit::default() }
    }

    /// Closes `batch` into the next segment of `writer`, which takes it, and of `wal`: returns its
    /// number.
    fn close(writer: &Writer<Store>, wal: &mut Wal<Data>, batch: impl Into<Batch>) -> u64 {
        let Unframed { footer, data } = batch.into().close(&mut writer.chain());
        let seq = writer.add_segment(data.frame(&footer)).unwrap();
        wal.push(footer, data);
        seq
    }

    /// Uploads `writer`'s segment `seq` and acknowledges it, with those before it, as the host does.
    async fn land(writer: &Writer<Store>, seq: u64) {
        writer.upload(seq).await.unwrap();
        writer.acknowledge().await.unwrap();
    }

    /// Puts `wal` after the end of the branch `writer` just took, as opening a store does, and
    /// lands what it stamped again: the commits of the segments it found the branch held.
    async fn rebase(writer: &Writer<Store>, wal: &mut Wal<Data>) -> Result<Vec<u64>, Error> {
        let Rebased { landed, .. } = wal.rebase(&writer.fenced())?;
        for (footer, data) in wal.segments() {
            let seq = writer.add_segment(data.frame(footer)).unwrap();
            land(writer, seq).await;
        }
        wal.acknowledge(writer.state().through.unwrap());
        Ok(landed.iter().flat_map(|(footer, _)| footer.contents.commits.clone()).collect())
    }

    /// The commits branch 1's segments make, in the order its readers apply them, skipping those
    /// that are void: each once, if its history is whole.
    async fn history(bucket: &Arc<Store>) -> Vec<u64> {
        let mut state = State::new(1);
        let mut commits = Vec::new();
        for seq in 0.. {
            let Some(object) = bucket::get(&**bucket, &layout::segment(1, seq)).await else {
                return commits;
            };
            let footer = layout::Segment::footer(&object.bytes).unwrap();
            if state.apply(&footer) {
                commits.extend(footer.contents.commits);
            }
        }
        unreachable!()
    }

    /// A log whose segments all landed, though never acknowledged, its host crashing first, is the
    /// branch's: taken out, nothing stamped again, its base the branch's last commit.
    #[test]
    fn logs_wholly_landed_are_the_branchs() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            let seqs = [close(&first, &mut wal, commit(1)), close(&first, &mut wal, commit(2))];
            for seq in seqs {
                first.upload(seq).await.unwrap();
            }
            let second = open(&bucket, 11).await;
            assert_eq!(rebase(&second, &mut wal).await, Ok(vec![1, 2]));
            assert!(wal.is_empty());
            assert_eq!(wal.base(), Some(2));
            assert_eq!(history(&bucket).await, [1, 2]);
        });
    }

    /// TASK-165.2's case, which a rebase by the footers recovery read declared diverged: a log
    /// whose segments landed, and which another host's writer then checkpointed past, is the
    /// branch's, though recovery, starting from the checkpoint, reads none of its footers.
    #[test]
    fn logs_landed_under_a_later_checkpoint_are_the_branchs() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            let seq = close(&first, &mut wal, commit(1));
            first.upload(seq).await.unwrap();
            let elsewhere = open(&bucket, 11).await;
            let checkpoint = elsewhere.checkpoint().await.unwrap();
            assert_eq!(checkpoint, Some(seq + 1), "the checkpoint is as of the fence after it");
            let third = open(&bucket, 12).await;
            assert_eq!(rebase(&third, &mut wal).await, Ok(vec![1]));
            assert!(wal.is_empty());
            assert_eq!(third.last_commit(), Some(1));
        });
    }

    /// A log whose first segment landed, the second not: the first is the branch's, the second
    /// is stamped again after the next writer's fence, and lands, each commit once.
    #[test]
    fn logs_partly_landed_go_on_after_the_fence() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            let seq = close(&first, &mut wal, commit(1));
            close(&first, &mut wal, commit(2));
            first.upload(seq).await.unwrap();
            let second = open(&bucket, 11).await;
            let chain = second.fenced().chain;
            assert_eq!(wal.rebase(&second.fenced()).map(|rebased| rebased.landed.len()), Ok(1));
            let [(footer, _)] = &wal.segments().collect::<Vec<_>>()[..] else {
                panic!("{} segments stamped again", wal.segments().len())
            };
            let place = (footer.seq, footer.epoch, footer.after, footer.contents.commits.clone());
            assert_eq!(place, (chain.next_seq(), second.epoch(), chain.after(), vec![2]));
            assert_eq!(wal.base(), Some(1));
            assert_eq!(rebase(&second, &mut wal).await, Ok(vec![]));
            assert_eq!(history(&bucket).await, [1, 2]);
            assert_eq!(second.last_commit(), Some(2));
        });
    }

    /// fxdax: a log whose commit spans two segments, the first landed, the second not: no segment
    /// of it landed, the commit unended at the fence, so both are stamped again after it, and land,
    /// the commit once, its runs in both sealed (TASK-92.41). Taken as landed, the first's runs, the
    /// fence having left them unsealed, would be lost.
    #[test]
    fn logs_of_commits_spanning_segments_land_whole() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            let extent = |sequence| ExtentId::new(1, sequence);
            let mut close_contents = |contents: Contents| {
                let footer = first.chain().close(contents);
                let data = Data::default();
                let seq = first.add_segment(data.frame(&footer)).unwrap();
                wal.push(footer, data);
                seq
            };
            let continued = Continued { commit: 1, extents: vec![(extent(1), 0, 4096)] };
            let seq =
                close_contents(Contents { continued: Some(continued), ..Contents::default() });
            let ending = Contents {
                commits: vec![1],
                extents: vec![(extent(2), 0, 4096)],
                next_sequence: 3,
                ..Contents::default()
            };
            close_contents(ending);
            first.upload(seq).await.unwrap();
            let second = open(&bucket, 11).await;
            assert_eq!(second.fenced().chain.next_seq(), seq + 2, "the fence after the first");
            assert_eq!(rebase(&second, &mut wal).await, Ok(vec![]));
            assert_eq!(history(&bucket).await, [1]);
            let sealed: Vec<ExtentId> = second.state().extents.keys().copied().collect();
            assert_eq!(sealed, [extent(1), extent(2)]);
        });
    }

    /// fxdax: a log whose segment ending commit 1 goes on with commit 2, landed alone, the
    /// segment ending commit 2 not: the branch holds commit 1, and the log follows it, keeping the
    /// segment, stamped again after the fence with its run of commit 2 alone, then the one ending
    /// commit 2; both land, each commit once, both runs sealed (TASK-345, found by specs/bucket and
    /// by extent_store's crash rounds, round 45). It diverged, though no other writer moved the
    /// branch on.
    #[test]
    fn logs_whose_segment_ending_a_commit_and_going_on_landed_alone_go_on() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            let extent = |sequence| ExtentId::new(1, sequence);
            let mut close_contents = |contents: Contents| {
                let footer = first.chain().close(contents);
                let data = Data::default();
                let seq = first.add_segment(data.frame(&footer)).unwrap();
                wal.push(footer, data);
                seq
            };
            let continued = Continued { commit: 2, extents: vec![(extent(2), 4096, 4096)] };
            let seq = close_contents(Contents {
                commits: vec![1],
                extents: vec![(extent(1), 0, 4096)],
                next_sequence: 2,
                continued: Some(continued.clone()),
                ..Contents::default()
            });
            let ending = Contents {
                commits: vec![2],
                extents: vec![(extent(3), 0, 4096)],
                next_sequence: 4,
                ..Contents::default()
            };
            close_contents(ending);
            first.upload(seq).await.unwrap();
            let second = open(&bucket, 11).await;
            assert_eq!(second.fenced().last_commit, Some(1));
            let mut rebased = wal.clone();
            let Rebased { landed, ended } = rebased.rebase(&second.fenced()).unwrap();
            assert_eq!((landed.len(), ended.map(|footer| footer.seq)), (0, Some(seq)));
            let contents: Vec<_> = rebased.segments().map(|(f, _)| f.contents.clone()).collect();
            // Its run of commit 2 alone, minting on from the writer's floor.
            let next_sequence = contents[0].next_sequence;
            let continuation =
                Contents { continued: Some(continued), next_sequence, ..Contents::default() };
            assert_eq!(contents[0], continuation);
            assert_eq!(contents[1].commits, [2]);
            assert_eq!(rebased.base(), Some(1));
            assert_eq!(rebase(&second, &mut wal).await, Ok(vec![]));
            assert_eq!(history(&bucket).await, [1, 2]);
            let sealed: Vec<ExtentId> = second.state().extents.keys().copied().collect();
            assert_eq!(sealed, [extent(1), extent(2), extent(3)]);
            assert_eq!(second.last_commit(), Some(2));
        });
    }

    /// A log whose second segment landed past a gap, its first not: the second is void, and both
    /// are stamped again after the fence, which filled the gap.
    #[test]
    fn logs_landed_past_a_gap_are_stamped_again() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            close(&first, &mut wal, commit(1));
            let seq = close(&first, &mut wal, commit(2));
            first.upload(seq).await.unwrap();
            let second = open(&bucket, 11).await;
            assert_eq!(second.fenced().chain.next_seq(), seq + 1, "the fence filled the gap");
            assert_eq!(rebase(&second, &mut wal).await, Ok(vec![]));
            assert_eq!(history(&bucket).await, [1, 2]);
        });
    }

    /// A log none of whose segments landed is stamped again whole: its extents in its data as they
    /// were, those of their own at the new writer's keys, each read back as written.
    #[test]
    fn logs_none_landed_are_stamped_again_whole() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            let sequence = first.chain().next_sequence();
            let (inside, own) = (ExtentId::new(1, sequence), ExtentId::new(1, sequence + 1));
            let extents = vec![(inside, b"inside".to_vec())];
            let owns = vec![(own.sequence, b"own".to_vec())];
            close(&first, &mut wal, Commit { tag: 1, extents, own: owns, ..Commit::default() });
            close(&first, &mut wal, commit(2));
            let second = open(&bucket, 11).await;
            assert_eq!(wal.rebase(&second.fenced()).map(|rebased| rebased.landed.len()), Ok(0));
            let (footer, _) = wal.segments().next().unwrap();
            let key = layout::extent(second.epoch(), footer.seq, own);
            assert_eq!(footer.own().map(|(_, location)| location.key).collect::<Vec<_>>(), [key]);
            assert_eq!(footer.contents.next_sequence, second.chain().next_sequence());
            assert_eq!(rebase(&second, &mut wal).await, Ok(vec![]));
            assert_eq!(history(&bucket).await, [1, 2]);
            let state = second.state();
            for (extent, bytes) in [(inside, &b"inside"[..]), (own, b"own")] {
                assert_eq!(
                    crate::read(&bucket, extent, &state.extents[&extent]).await,
                    Ok(bytes.into())
                );
            }
        });
    }

    /// A log after whose base another writer's commit came, acknowledged, diverged: unchanged,
    /// its commits put after no other's.
    #[test]
    fn logs_another_writer_moved_on_from_diverge() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            close(&first, &mut wal, commit(1));
            let elsewhere = open(&bucket, 11).await;
            let seq = close(&elsewhere, &mut log(&elsewhere), commit(3));
            land(&elsewhere, seq).await;
            let third = open(&bucket, 12).await;
            assert_eq!(rebase(&third, &mut wal).await, Err(Error::Diverged));
            let commits: Vec<_> = wal.segments().map(|(f, _)| f.contents.commits.clone()).collect();
            assert_eq!((wal.base(), commits), (None, vec![vec![1]]));
            assert_eq!(history(&bucket).await, [3]);
        });
    }

    /// TASK-165.2's other case: a log after which only another writer's fence came, a takeover
    /// that went no further, goes on after the next fence.
    #[test]
    fn logs_only_a_fence_came_after_go_on() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            close(&first, &mut wal, commit(1));
            drop(open(&bucket, 11).await);
            let third = open(&bucket, 12).await;
            assert_eq!(rebase(&third, &mut wal).await, Ok(vec![]));
            assert_eq!(history(&bucket).await, [1]);
        });
    }

    /// An empty log never diverges, wherever the branch went: its base is the branch's last
    /// commit.
    #[test]
    fn empty_logs_go_on_from_the_branchs_end() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            let elsewhere = open(&bucket, 11).await;
            let seq = close(&elsewhere, &mut log(&elsewhere), commit(3));
            land(&elsewhere, seq).await;
            let third = open(&bucket, 12).await;
            assert_eq!(rebase(&third, &mut wal).await, Ok(vec![]));
            assert_eq!((wal.base(), wal.head()), (Some(3), Some(3)));
        });
    }

    /// A segment making no commit, a fork's alone, after the branch's last commit, is taken as
    /// landed: never put after the end twice, though it may not have landed.
    #[test]
    fn segments_of_forks_alone_after_the_last_commit_are_taken_as_landed() {
        futures::executor::block_on(async {
            let bucket = bucket().await;
            let first = open(&bucket, 10).await;
            let mut wal = log(&first);
            let seq = close(&first, &mut wal, commit(1));
            let Unframed { footer, data } = Batch::default().fork(child(2), &mut first.chain());
            first.add_segment(data.frame(&footer)).unwrap();
            wal.push(footer, data);
            close(&first, &mut wal, commit(3));
            first.upload(seq).await.unwrap();
            let second = open(&bucket, 11).await;
            let landed = wal.rebase(&second.fenced()).unwrap().landed;
            let forks: Vec<_> = landed.iter().map(|(f, _)| f.contents.forks.clone()).collect();
            assert_eq!(forks, [vec![], vec![child(2)]]);
            let commits: Vec<_> = wal.segments().map(|(f, _)| f.contents.commits.clone()).collect();
            assert_eq!(commits, [vec![3]]);
        });
    }

    /// Acknowledging takes out the segments to the number acknowledged; the base is the last
    /// commit they make, or, those making none, the base before.
    #[test]
    fn acknowledged_segments_leave_the_log() {
        let mut chain = Chain::new(1, 2, 5, Some((4, 2)), 0);
        let mut wal = Wal::new(Some(7));
        let mut push = |commits: Vec<u64>| {
            wal.push(chain.close(Contents { commits, ..Contents::default() }), ());
        };
        push(vec![]);
        push(vec![8, 9]);
        push(vec![]);
        push(vec![10]);
        assert_eq!((wal.base(), wal.head()), (Some(7), Some(10)));
        let numbers = |taken: Vec<(Footer, ())>| -> Vec<u64> {
            taken.iter().map(|(footer, _)| footer.seq).collect()
        };
        assert_eq!(numbers(wal.acknowledge(5)), [5]);
        assert_eq!(wal.base(), Some(7));
        assert_eq!(numbers(wal.acknowledge(7)), [6, 7]);
        assert_eq!((wal.base(), wal.head()), (Some(9), Some(10)));
        assert_eq!(numbers(wal.acknowledge(7)), Vec::<u64>::new());
        assert_eq!(numbers(wal.acknowledge(20)), [8]);
        assert_eq!((wal.base(), wal.head(), wal.is_empty()), (Some(10), Some(10), true));
    }
}
