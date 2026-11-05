// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A batch: commits a writer makes into one segment of its own (doc-7, Layout), rather than take a
//! segment a store closed. Closed on the writer's chain, it is the writer's next segment, framed as
//! the store frames its own ([`Unframed::frame`]), which the writer takes as it takes a store's
//! ([`Writer::add_segment`](crate::Writer::add_segment)).
//!
//! What is too much to hold at once, what a store holds that its branch doesn't, goes in segments
//! planned by the lengths of the extents they seal ([`Plan`]), whose objects read their data only
//! as they upload ([`objects`]).

use crate::layout::{self, Chain, Child, Contents, Footer, Segment};
use crate::writer::{Body, Objects};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use storage_device::ExtentId;

/// A commit: the extents it seals, which it minted, stored in its segment or, `own`, by their
/// sequence numbers, on their own; the extents it frees; and the root and the branch's capacity,
/// if it sets them. `tag` names it in footers.
#[derive(Clone, Debug, Default)]
pub struct Commit {
    pub tag: u64,
    pub extents: Vec<(ExtentId, Vec<u8>)>,
    pub own: Vec<(u64, Vec<u8>)>,
    pub frees: Vec<ExtentId>,
    pub root: Option<Vec<u8>>,
    pub capacity: Option<u64>,
}

impl Commit {
    /// Whether it changes nothing: it seals and frees no extent, and sets no root nor capacity.
    pub fn is_empty(&self) -> bool {
        self.extents.is_empty()
            && self.own.is_empty()
            && self.frees.is_empty()
            && self.root.is_none()
            && self.capacity.is_none()
    }
}

/// What one segment's commits make, by the lengths of the extents they seal: the contents of its
/// footer, once closed on its writer's chain ([`Plan::close`]); the extents in its data one after
/// another, from where its data begins. fxdax: as a store's segment does, its footer seals what its
/// commits sealed that the branch holds as of its last, and frees what they freed that a commit
/// held: one sealed by a commit and freed by a later is a free alone, which spends it; one a commit
/// seals and frees itself no commit held, and is neither (TASK-160.15, TASK-165.3).
#[derive(Default)]
pub struct Plan {
    /// Its commits' tags, root, capacity, and the next sequence number as of them: its footer's
    /// contents but what [`Plan::contents`] makes of the rest.
    contents: Contents,
    /// The extents its commits seal, in its data, at an offset, of a length, and on their own, by
    /// their sequence numbers, of a length; and those they free: each with the commit, by its
    /// index, that sealed or freed it.
    extents: Vec<((ExtentId, u64, u64), usize)>,
    own: Vec<((u64, u64), usize)>,
    frees: Vec<(ExtentId, usize)>,
    /// How long its data is: where the next extent sealed in it goes.
    data: u64,
    /// The bytes of the extents it seals, in its data and on their own.
    bytes: u64,
}

impl Plan {
    /// Seals `extent`, of `len` bytes, in its data, after those before it. The branch's next
    /// sequence number is past it.
    pub fn seal(&mut self, extent: ExtentId, len: u64) {
        self.extents.push(((extent, self.data, len), self.committing()));
        self.data += len;
        self.sealed(extent.sequence, len);
    }

    /// Seals the extent of the branch whose sequence number is `sequence`, of `len` bytes, on its
    /// own. The branch's next sequence number is past it.
    pub fn seal_own(&mut self, sequence: u64, len: u64) {
        self.own.push(((sequence, len), self.committing()));
        self.sealed(sequence, len);
    }

    /// The index of the commit being made.
    fn committing(&self) -> usize {
        self.contents.commits.len()
    }

    /// An extent of `sequence`, of `len` bytes, is sealed.
    fn sealed(&mut self, sequence: u64, len: u64) {
        self.bytes += len;
        self.contents.next_sequence = self.contents.next_sequence.max(sequence + 1);
    }

    /// The bytes of the extents it seals, in its data and on their own.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Commits what it seals, as commit `tag`, which frees `frees`, and sets `root` and the
    /// branch's `capacity`, if given: after the commits before it.
    pub fn commit(
        &mut self,
        tag: u64,
        frees: Vec<ExtentId>,
        root: Option<Vec<u8>>,
        capacity: Option<u64>,
    ) {
        let commit = self.committing();
        self.frees.extend(frees.into_iter().map(|extent| (extent, commit)));
        if root.is_some() {
            self.contents.root = root;
        }
        if capacity.is_some() {
            self.contents.capacity = capacity;
        }
        self.contents.commits.push(tag);
    }

    /// Whether it holds no commit.
    fn is_empty(&self) -> bool {
        self.contents.commits.is_empty()
    }

    /// Its footer's contents, as `branch`'s: what its commits sealed that none of them freed, and
    /// what they freed that a commit before held.
    fn contents(self, branch: u64) -> Contents {
        let Self { mut contents, extents, own, frees, .. } = self;
        let in_data = extents.iter().map(|&((extent, ..), by)| (extent, by));
        let on_own = own.iter().map(|&((sequence, _), by)| (ExtentId::new(branch, sequence), by));
        let sealed: HashMap<ExtentId, usize> = in_data.chain(on_own).collect();
        let mut freed = HashSet::new();
        for (extent, by) in frees {
            match sealed.get(&extent) {
                Some(&at) => {
                    freed.insert(extent);
                    if at < by {
                        contents.frees.push(extent);
                    }
                }
                None => contents.frees.push(extent),
            }
        }
        let held = |extent: &ExtentId| !freed.contains(extent);
        contents.extents = extents
            .into_iter()
            .map(|(sealed, _)| sealed)
            .filter(|(extent, ..)| held(extent))
            .collect();
        contents.own = (own.into_iter())
            .map(|(sealed, _)| sealed)
            .filter(|&(sequence, _)| held(&ExtentId::new(branch, sequence)))
            .collect();
        contents
    }

    /// Closes it on `chain`, as the writer's next segment, which the chain goes on after: its
    /// footer, stamped, whose objects read their data as they upload ([`objects`]).
    pub fn close(self, chain: &mut Chain) -> Footer {
        self.close_forking(chain, Vec::new())
    }

    /// Closes it on `chain`, as [`Plan::close`] does, with forks of the branch at its end to
    /// `forks`, their children.
    fn close_forking(self, chain: &mut Chain, forks: Vec<Child>) -> Footer {
        chain.close(Contents { forks, ..self.contents(chain.branch()) })
    }
}

/// How the data of an extent a planned segment seals is read ([`objects`]): into a buffer of its
/// length.
pub type Read = Arc<dyn Fn(ExtentId, &mut [u8]) -> std::io::Result<()> + Send + Sync>;

/// The objects of the segment `footer` stamps, a plan's ([`Plan::close`]), and of the extents it
/// says are stored on their own, by their keys: each read, its extents' data by `read`, and
/// framed, only as it uploads.
pub fn objects(footer: Footer, read: Read) -> Objects {
    let own = (footer.own())
        .map(|(extent, location)| {
            let (len, read) = (location.len, read.clone());
            let body = Body::new(len, move || {
                let mut data = vec![0; len as usize];
                read(extent, &mut data)?;
                Ok(own_object(&data))
            });
            (location.key.clone(), body)
        })
        .collect();
    let len =
        footer.contents.extents.iter().map(|&(_, offset, len)| offset + len).max().unwrap_or(0);
    let framed = footer.clone();
    let segment = Body::new(len, move || {
        let mut data = vec![0; len as usize];
        for &(extent, offset, len) in &framed.contents.extents {
            read(extent, &mut data[offset as usize..(offset + len) as usize])?;
        }
        Ok(Segment { data, footer: framed.clone() }.encode())
    });
    Objects { footer, segment, own }
}

/// The object of an extent stored on its own, whose data is `data`: framed, with no footer.
fn own_object(data: &[u8]) -> Vec<u8> {
    layout::with_footer(data, &())
}

/// Commits for one segment, in order: what they make ([`Plan`]), its data, and that of the
/// extents they seal on their own.
#[derive(Default)]
pub struct Batch {
    plan: Plan,
    data: Vec<u8>,
    /// Those sealed on their own, by their sequence numbers.
    own: BTreeMap<u64, Vec<u8>>,
}

impl From<Commit> for Batch {
    /// A batch of `commit` alone.
    fn from(commit: Commit) -> Self {
        let mut batch = Self::default();
        batch.commit(commit);
        batch
    }
}

impl Batch {
    /// Adds `commit`, after those before it.
    pub fn commit(&mut self, commit: Commit) {
        for (extent, bytes) in commit.extents {
            self.plan.seal(extent, bytes.len() as u64);
            self.data.extend_from_slice(&bytes);
        }
        for (sequence, bytes) in commit.own {
            self.plan.seal_own(sequence, bytes.len() as u64);
            self.own.insert(sequence, bytes);
        }
        self.plan.commit(commit.tag, commit.frees, commit.root, commit.capacity);
    }

    /// Whether it holds no commit.
    pub fn is_empty(&self) -> bool {
        self.plan.is_empty()
    }

    /// Closes it on `chain`, its writer's, as the writer's next segment, which the chain goes on
    /// after: its footer, stamped, its data yet to be framed.
    pub fn close(self, chain: &mut Chain) -> Unframed {
        self.close_forking(chain, Vec::new())
    }

    /// Closes it on `chain`, as [`Batch::close`] does, with a fork of the branch to `child` at its
    /// end, of its owner, a point if it says so: a fork ends its segment, which its child inherits
    /// the whole of (doc-7, Forks, Points).
    pub fn fork(self, child: Child, chain: &mut Chain) -> Unframed {
        self.close_forking(chain, vec![child])
    }

    fn close_forking(self, chain: &mut Chain, forks: Vec<Child>) -> Unframed {
        let footer = self.plan.close_forking(chain, forks);
        Unframed { segment: Segment { data: self.data, footer }, own: self.own }
    }
}

/// A batch closed, its segment's footer stamped: its data, and that of its extents of their own,
/// by their sequence numbers, yet to be framed.
pub struct Unframed {
    segment: Segment,
    own: BTreeMap<u64, Vec<u8>>,
}

impl Unframed {
    /// Its objects, framed: the segment's, and its extents of their own's, at their keys.
    pub fn frame(mut self) -> Objects {
        let own = (self.segment.footer.own())
            .map(|(extent, location)| {
                let data = self.own.remove(&extent.sequence).expect("sealed on its own");
                (location.key.clone(), own_object(&data).into())
            })
            .collect();
        let footer = self.segment.footer.clone();
        Objects { footer, segment: self.segment.encode().into(), own }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Footer, child};

    /// The next sequence number the footer of `commit`'s segment, closed on `chain`, names.
    fn next_sequence(commit: Commit, chain: &mut Chain) -> u64 {
        let objects = Batch::from(commit).close(chain).frame();
        layout::footer::<Footer>(&objects.segment.read().unwrap()).unwrap().contents.next_sequence
    }

    /// The sequence numbers a writer mints from are past every extent its commits seal, in its
    /// segments and on their own: the footers its chain stamps say so, and so does the chain.
    #[test]
    fn writers_mint_past_what_their_commits_seal() {
        let first = 5 << crate::EPOCH_SHIFT;
        let mut chain = Chain::new(1, 5, 1, Some((0, 5)), first);
        let extents = vec![(ExtentId::new(1, first + 3), b"in".to_vec())];
        assert_eq!(
            next_sequence(Commit { tag: 1, extents, ..Commit::default() }, &mut chain),
            first + 4
        );
        assert_eq!(chain.next_sequence(), first + 4);
        let own = vec![(first + 7, b"own".to_vec())];
        assert_eq!(
            next_sequence(Commit { tag: 2, own, ..Commit::default() }, &mut chain),
            first + 8
        );
        // One minted below adds nothing.
        let extents = vec![(ExtentId::new(1, first + 1), b"below".to_vec())];
        assert_eq!(
            next_sequence(Commit { tag: 3, extents, ..Commit::default() }, &mut chain),
            first + 8
        );
        assert_eq!(chain.next_sequence(), first + 8);
    }

    /// A batch's segment holds its commits' extents in its data, in order, those of their own in
    /// objects of their own at the writer's keys, its frees, its last root and its last capacity;
    /// a fork is at its end.
    #[test]
    fn batches_frame_their_commits() {
        let mut chain = Chain::new(1, 2, 0, None, 0);
        let (small, large, freed) = (ExtentId::new(1, 3), ExtentId::new(1, 4), ExtentId::new(1, 1));
        let mut batch = Batch::default();
        assert!(batch.is_empty());
        let extents = vec![(small, b"small".to_vec())];
        let (root, capacity) = (Some(b"one".to_vec()), Some(1 << 20));
        batch.commit(Commit { tag: 7, extents, root, capacity, ..Commit::default() });
        let own = vec![(large.sequence, b"large".to_vec())];
        let frees = vec![freed];
        let (root, capacity) = (Some(b"two".to_vec()), Some(2 << 20));
        batch.commit(Commit { tag: 8, own, frees, root, capacity, ..Commit::default() });
        batch.commit(Commit { tag: 9, ..Commit::default() });
        assert!(!batch.is_empty());
        let objects = batch.fork(child(9), &mut chain).frame();
        let segment = objects.segment.read().unwrap();
        let footer: Footer = layout::footer(&segment).unwrap();
        assert_eq!(footer, objects.footer);
        assert_eq!((footer.seq, footer.epoch, footer.after), (0, 2, None));
        assert_eq!(footer.contents.commits, [7, 8, 9]);
        assert_eq!(footer.contents.extents, [(small, 0, 5)]);
        let key = layout::extent(2, footer.seq, large);
        assert_eq!(
            footer.own().collect::<Vec<_>>(),
            [(large, layout::Location { key: key.clone(), offset: 0, len: 5 })]
        );
        let contents = footer.contents;
        assert_eq!(
            (contents.frees, contents.root, contents.forks, contents.capacity),
            (vec![freed], Some(b"two".to_vec()), vec![child(9)], Some(2 << 20))
        );
        assert_eq!(layout::data(&segment).unwrap(), b"small");
        let [(own_key, own)] = &objects.own[..] else {
            panic!("{} objects of their own", objects.own.len())
        };
        let own = own.read().unwrap();
        assert_eq!((own_key, layout::data(&own).unwrap()), (&key, b"large".to_vec()));
        assert_eq!((chain.next_seq(), chain.after()), (1, Some((0, 2))));
    }

    /// TASK-165.3: as a store's segment's footer does, a batch's seals what its commits sealed that
    /// none of them freed, and frees what a commit before the freeing one held: an extent sealed and
    /// later freed in it, in its data or on its own, is a free alone; one a commit seals and frees
    /// itself is neither; an inherited extent of the same sequence number as one sealed on its own
    /// is freed as it is.
    #[test]
    fn batches_free_what_a_commit_before_held() {
        let mut chain = Chain::new(1, 2, 0, None, 0);
        let (small, large, other) = (ExtentId::new(1, 3), ExtentId::new(1, 4), ExtentId::new(1, 5));
        let (written, inherited, kept) =
            (ExtentId::new(1, 6), ExtentId::new(9, 4), ExtentId::new(1, 7));
        let mut batch = Batch::default();
        let extents = vec![(small, b"small".to_vec()), (kept, b"kept".to_vec())];
        let own = vec![(large.sequence, b"large".to_vec()), (other.sequence, b"other".to_vec())];
        batch.commit(Commit { tag: 7, extents, own, ..Commit::default() });
        let extents = vec![(written, b"written".to_vec())];
        let frees = vec![small, large, written, inherited];
        batch.commit(Commit { tag: 8, extents, frees, ..Commit::default() });
        let objects = batch.close(&mut chain).frame();
        let footer = objects.footer;
        assert_eq!(footer.contents.extents, [(kept, 5, 4)]);
        let key = layout::extent(2, 0, other);
        assert_eq!(
            footer.own().collect::<Vec<_>>(),
            [(other, layout::Location { key: key.clone(), offset: 0, len: 5 })]
        );
        assert_eq!(footer.contents.frees, [small, large, inherited]);
        let own: Vec<_> = objects.own.iter().map(|(key, _)| key.clone()).collect();
        assert_eq!(own, [key]);
    }

    /// A segment planned by the lengths of the extents it seals frames what a batch of the same
    /// commit frames, its footer, data and extents of their own: reading their data only as each
    /// object is read.
    #[test]
    fn plans_frame_as_batches_do() {
        let (small, other, large) = (ExtentId::new(1, 3), ExtentId::new(1, 4), ExtentId::new(1, 5));
        let data = std::collections::BTreeMap::from([
            (small, b"small".to_vec()),
            (other, b"other!".to_vec()),
            (large, b"large".to_vec()),
        ]);
        let (freed, root) = (ExtentId::new(1, 1), Some(b"root".to_vec()));
        let commit = Commit {
            tag: 7,
            extents: vec![(small, data[&small].clone()), (other, data[&other].clone())],
            own: vec![(large.sequence, data[&large].clone())],
            frees: vec![freed],
            root: root.clone(),
            capacity: Some(1 << 20),
        };
        let chain = Chain::new(1, 2, 0, None, 0);
        let batch = Batch::from(commit).close(&mut chain.clone()).frame();
        let mut plan = Plan::default();
        for (extent, own) in [(small, false), (other, false), (large, true)] {
            let len = data[&extent].len() as u64;
            match own {
                true => plan.seal_own(extent.sequence, len),
                false => plan.seal(extent, len),
            }
        }
        assert_eq!(plan.bytes(), 16);
        plan.commit(7, vec![freed], root, Some(1 << 20));
        let reads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let read: Read = {
            let reads = reads.clone();
            Arc::new(move |extent, into: &mut [u8]| {
                reads.lock().unwrap().push(extent);
                into.copy_from_slice(&data[&extent]);
                Ok(())
            })
        };
        let planned = objects(plan.close(&mut chain.clone()), read);
        assert_eq!(planned.footer, batch.footer);
        let read = || reads.lock().unwrap().clone();
        assert_eq!(read(), []);
        assert_eq!(planned.segment.read().unwrap(), batch.segment.read().unwrap());
        assert_eq!(read(), [small, other]);
        let [(key, own)] = &planned.own[..] else { panic!("{} of their own", planned.own.len()) };
        assert_eq!(key, &batch.own[0].0);
        assert_eq!(own.read().unwrap(), batch.own[0].1.read().unwrap());
        assert_eq!(read(), [small, other, large]);
    }
}
