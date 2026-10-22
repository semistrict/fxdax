// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A branch's extents, in layers (TASK-31). What a fork inherits is its parent's state at the
//! fork: a [`Layer`], which every fork of that point shares in memory, one copy, and the parent
//! too, which goes on over it. Each holds over it what it changed since ([`Extents`]). A layer
//! never changes: a branch forking again freezes what it changed since into a layer over the one
//! it had, so layers make chains, which are merged into one once they grow deep. Each layer keeps,
//! as differences from the one below, what its extents add up to: the bytes they hold, those of
//! each segment, and how many extents are in each file; so a branch opening over a layer, or the
//! lineage holding its files, need not go through what the layer holds.

use crate::layout::{Data, Extent, FileKey, SegmentId, files_of};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use storage_device::ExtentId;

/// How many layers a chain may have before freezing another merges those above the branch's
/// origin into one.
const MAX_DEPTH: usize = 16;

/// A branch's state as of a commit, which forks of it share: what it holds over the layer below,
/// if any, and what of that it no longer holds. See the module's docs.
pub(crate) struct Layer {
    below: Option<Arc<Layer>>,
    /// The layers in its chain, itself included.
    depth: usize,
    /// The extents it adds, or holds otherwise than the layer below.
    extents: HashMap<ExtentId, Extent>,
    /// The extents of the layer below it holds no longer.
    gone: HashSet<ExtentId>,
    /// The root as of its commit.
    root: Vec<u8>,
    /// How many extents it holds, and their bytes.
    len: usize,
    stored: u64,
    /// The bytes of extents' data in each segment, and the extents in each file, as differences
    /// from the layer below.
    live: HashMap<SegmentId, i64>,
    files: HashMap<FileKey, i64>,
}

impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer").field("depth", &self.depth).field("len", &self.len).finish()
    }
}

/// What extents add up to, as a layer keeps it.
#[derive(Default)]
struct Sums {
    len: i64,
    stored: i64,
    live: HashMap<SegmentId, i64>,
    files: HashMap<FileKey, i64>,
}

impl Sums {
    /// Adds `entry`, of `extent`, if `sign` is 1, or takes it away, if -1.
    fn add(&mut self, extent: ExtentId, entry: &Extent, sign: i64) {
        self.len += sign;
        self.stored += sign * entry.len as i64;
        if let Data::Pieces(pieces) = &entry.data {
            for piece in pieces {
                *self.live.entry(piece.segment).or_default() += sign * piece.len as i64;
            }
        }
        for file in files_of([(&extent, entry)]) {
            *self.files.entry(file).or_default() += sign;
        }
    }
}

impl Layer {
    /// A layer over `below` holding `extents`, and no longer what `gone` names of `below`'s, as of
    /// `root`.
    fn new(
        below: Option<Arc<Layer>>,
        extents: HashMap<ExtentId, Extent>,
        mut gone: HashSet<ExtentId>,
        root: Vec<u8>,
    ) -> Arc<Self> {
        let below_get = |extent: &ExtentId| below.as_ref().and_then(|below| below.get(extent));
        gone.retain(|extent| !extents.contains_key(extent) && below_get(extent).is_some());
        let mut sums = Sums::default();
        for (extent, entry) in &extents {
            if let Some(old) = below_get(extent) {
                sums.add(*extent, old, -1);
            }
            sums.add(*extent, entry, 1);
        }
        for extent in &gone {
            sums.add(*extent, below_get(extent).unwrap(), -1);
        }
        sums.live.retain(|_, bytes| *bytes != 0);
        sums.files.retain(|_, count| *count != 0);
        let (depth, len, stored) =
            below.as_ref().map_or((0, 0, 0), |below| (below.depth, below.len, below.stored));
        Arc::new(Self {
            below,
            depth: depth + 1,
            extents,
            gone,
            root,
            len: (len as i64 + sums.len) as usize,
            stored: (stored as i64 + sums.stored) as u64,
            live: sums.live,
            files: sums.files,
        })
    }

    /// The layers of its chain, from it down.
    fn chain(&self) -> impl Iterator<Item = &Layer> {
        std::iter::successors(Some(self), |layer| layer.below.as_deref())
    }

    /// `extent`'s entry, if the layer holds it.
    pub(crate) fn get(&self, extent: &ExtentId) -> Option<&Extent> {
        for layer in self.chain() {
            if let Some(entry) = layer.extents.get(extent) {
                return Some(entry);
            }
            if layer.gone.contains(extent) {
                return None;
            }
        }
        None
    }

    /// The root as of its commit.
    pub(crate) fn root(&self) -> &[u8] {
        &self.root
    }

    /// The bytes its extents hold.
    pub(crate) fn stored(&self) -> u64 {
        self.stored
    }

    /// The bytes of its extents' data in `segment`.
    pub(crate) fn live(&self, segment: SegmentId) -> u64 {
        self.chain().map(|layer| layer.live.get(&segment).copied().unwrap_or(0)).sum::<i64>() as u64
    }

    /// Whether it holds an extent in `file`.
    pub(crate) fn holds(&self, file: FileKey) -> bool {
        self.count(file) > 0
    }

    /// How many of its extents are in `file`.
    pub(crate) fn count(&self, file: FileKey) -> u64 {
        self.chain().map(|layer| layer.files.get(&file).copied().unwrap_or(0)).sum::<i64>() as u64
    }

    /// The files its extents are in.
    pub(crate) fn files(&self) -> BTreeSet<FileKey> {
        let mut entries = Vec::new();
        self.visible(HashSet::new(), |extent, entry| entries.push((extent, entry)));
        files_of(entries)
    }

    /// Calls `f` with each extent it holds, and its entry, but for those in `hidden`.
    fn visible<'a>(
        &'a self,
        mut hidden: HashSet<ExtentId>,
        mut f: impl FnMut(&'a ExtentId, &'a Extent),
    ) {
        for layer in self.chain() {
            for (extent, entry) in &layer.extents {
                if !hidden.contains(extent) {
                    f(extent, entry);
                }
            }
            if layer.below.is_some() {
                hidden.extend(layer.extents.keys().copied());
                hidden.extend(layer.gone.iter().copied());
            }
        }
    }

    /// What the layers of its chain above `stop` hold over it: the extents they add or hold
    /// otherwise, and those of `stop`'s they no longer hold. `stop` must be in its chain, or none,
    /// for all it holds.
    fn over(&self, stop: Option<&Arc<Layer>>) -> (HashMap<ExtentId, Extent>, HashSet<ExtentId>) {
        let (mut extents, mut gone) = (HashMap::new(), HashSet::new());
        for layer in self.chain() {
            if stop.is_some_and(|stop| std::ptr::eq(layer, &**stop)) {
                break;
            }
            for (extent, entry) in &layer.extents {
                if !extents.contains_key(extent) && !gone.contains(extent) {
                    extents.insert(*extent, entry.clone());
                }
            }
            for extent in &layer.gone {
                if !extents.contains_key(extent) {
                    gone.insert(*extent);
                }
            }
        }
        gone.retain(|extent| stop.is_some_and(|stop| stop.get(extent).is_some()));
        (extents, gone)
    }
}

/// What a branch holds of the files of extents' data, as the lineage records it
/// ([`Extents::holdings`]).
pub(crate) struct Holdings {
    /// The files of the extents it holds of its own, over its base: those it changed.
    pub own: BTreeSet<FileKey>,
    /// The files of its base it holds no extent in: it let them go, though its base holds them.
    pub dropped: BTreeSet<FileKey>,
}

/// A branch's extents: a layer it is over, if any, which it shares, and what it changed over it.
#[derive(Default)]
pub(crate) struct Extents {
    base: Option<Arc<Layer>>,
    /// The extents it holds that its base doesn't, or holds otherwise.
    changed: HashMap<ExtentId, Extent>,
    /// The extents of its base it holds no longer.
    gone: HashSet<ExtentId>,
}

impl std::fmt::Debug for Extents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (f.debug_struct("Extents"))
            .field("base", &self.base)
            .field("changed", &self.changed.len())
            .field("gone", &self.gone.len())
            .finish()
    }
}

impl Extents {
    /// What `base` holds, or nothing.
    pub(crate) fn over(base: Option<Arc<Layer>>) -> Self {
        Self { base, ..Self::default() }
    }

    /// The layer it is over, if any.
    pub(crate) fn base(&self) -> Option<&Arc<Layer>> {
        self.base.as_ref()
    }

    /// The extents it holds that its base doesn't, or holds otherwise: those it changed.
    pub(crate) fn changed(&self) -> impl Iterator<Item = (&ExtentId, &Extent)> {
        self.changed.iter()
    }

    /// How many entries it holds of its own, over its base: its footprint.
    pub(crate) fn own_entries(&self) -> usize {
        self.changed.len() + self.gone.len()
    }

    /// The entries of its base it hides: those it no longer holds, or holds otherwise.
    pub(crate) fn shadowed(&self) -> Vec<(&ExtentId, &Extent)> {
        let Some(base) = &self.base else { return Vec::new() };
        (self.changed.keys().chain(&self.gone))
            .filter_map(|extent| Some((extent, base.get(extent)?)))
            .collect()
    }

    /// What it holds of the files of extents' data: those of its own, and those of its base it
    /// let go of.
    pub(crate) fn holdings(&self) -> Holdings {
        Holdings { own: files_of(self.changed()), dropped: self.dropped() }
    }

    /// The files of its base it holds no extent in: those all the base's extents in which it no
    /// longer holds, or holds otherwise.
    fn dropped(&self) -> BTreeSet<FileKey> {
        let Some(base) = self.base() else { return BTreeSet::new() };
        let mut lost: HashMap<FileKey, i64> = HashMap::new();
        for (extent, entry) in self.shadowed() {
            for file in files_of([(extent, entry)]) {
                *lost.entry(file).or_default() += 1;
            }
        }
        for (extent, entry) in self.changed() {
            for file in files_of([(extent, entry)]) {
                if let Some(lost) = lost.get_mut(&file) {
                    *lost -= 1;
                }
            }
        }
        (lost.into_iter())
            .filter(|(file, lost)| *lost > 0 && base.count(*file) == *lost as u64)
            .map(|(file, _)| file)
            .collect()
    }

    /// `extent`'s entry in its base, unless it no longer holds it.
    fn inherited(&self, extent: &ExtentId) -> Option<&Extent> {
        if self.gone.contains(extent) {
            return None;
        }
        self.base.as_ref()?.get(extent)
    }

    pub(crate) fn get(&self, extent: &ExtentId) -> Option<&Extent> {
        self.changed.get(extent).or_else(|| self.inherited(extent))
    }

    pub(crate) fn contains_key(&self, extent: &ExtentId) -> bool {
        self.get(extent).is_some()
    }

    /// `extent`'s entry, to change: its base's, copied, if it is inherited.
    pub(crate) fn get_mut(&mut self, extent: &ExtentId) -> Option<&mut Extent> {
        if !self.changed.contains_key(extent) {
            let entry = self.inherited(extent)?.clone();
            self.changed.insert(*extent, entry);
        }
        self.changed.get_mut(extent)
    }

    /// Holds `extent` as `entry`, in place of any entry it had.
    pub(crate) fn insert(&mut self, extent: ExtentId, entry: Extent) {
        self.gone.remove(&extent);
        self.changed.insert(extent, entry);
    }

    /// `extent`'s entry, inserted as `entry()` makes it if it holds none.
    pub(crate) fn get_or_insert_with(
        &mut self,
        extent: ExtentId,
        entry: impl FnOnce() -> Extent,
    ) -> &mut Extent {
        if !self.contains_key(&extent) {
            self.insert(extent, entry());
        }
        self.get_mut(&extent).unwrap()
    }

    /// Lets `extent` go, returning its entry, if it held it.
    pub(crate) fn remove(&mut self, extent: &ExtentId) -> Option<Extent> {
        let changed = self.changed.remove(extent);
        let inherited = self.inherited(extent).cloned();
        if inherited.is_some() {
            self.gone.insert(*extent);
        }
        changed.or(inherited)
    }

    /// Each extent it holds, with its entry, in no order.
    pub(crate) fn iter(&self) -> Vec<(&ExtentId, &Extent)> {
        let mut all: Vec<(&ExtentId, &Extent)> = self.changed.iter().collect();
        if let Some(base) = &self.base {
            let hidden = self.changed.keys().chain(&self.gone).copied().collect();
            base.visible(hidden, |extent, entry| all.push((extent, entry)));
        }
        all
    }

    /// What it holds over `origin`, a layer in its chain, as the index of a branch over `origin`
    /// records it: the extents it holds that `origin` doesn't, or holds otherwise, and those of
    /// `origin`'s it holds no longer; or, with no origin, all it holds.
    pub(crate) fn overlay(
        &self,
        origin: Option<&Arc<Layer>>,
    ) -> (Vec<(ExtentId, Extent)>, Vec<ExtentId>) {
        let Some(origin) = origin else {
            let all = self.iter().into_iter().map(|(extent, entry)| (*extent, entry.clone()));
            return (all.collect(), Vec::new());
        };
        let (mut extents, mut gone) = (self.changed.clone(), self.gone.clone());
        if let Some(base) = &self.base {
            let (below, below_gone) = base.over(Some(origin));
            for (extent, entry) in below {
                if !extents.contains_key(&extent) && !gone.contains(&extent) {
                    extents.insert(extent, entry);
                }
            }
            for extent in below_gone {
                if !extents.contains_key(&extent) {
                    gone.insert(extent);
                }
            }
        }
        gone.retain(|extent| origin.get(extent).is_some());
        (extents.into_iter().collect(), gone.into_iter().collect())
    }

    /// Freezes what it holds into a layer, as of `root`, which it goes on over: what a fork
    /// inherits. The layer it is over, if it changed nothing over it. A chain grown too deep has
    /// its layers above `origin`, the layer the branch's index is over, if any, merged into one.
    pub(crate) fn freeze(&mut self, origin: Option<&Arc<Layer>>, root: &[u8]) -> Arc<Layer> {
        if self.changed.is_empty()
            && self.gone.is_empty()
            && let Some(base) = &self.base
            && base.root == root
        {
            return base.clone();
        }
        let changed = std::mem::take(&mut self.changed);
        let gone = std::mem::take(&mut self.gone);
        let mut layer = Layer::new(self.base.take(), changed, gone, root.to_vec());
        if layer.depth > MAX_DEPTH {
            let (extents, gone) = layer.over(origin);
            layer = Layer::new(origin.cloned(), extents, gone, root.to_vec());
        }
        self.base = Some(layer.clone());
        layer
    }

    /// What it holds, over `layer` instead of its base: sharing what `layer` holds as it does.
    /// It must be over no layer: as a branch whose index holds all it holds is.
    pub(crate) fn rebase(&mut self, layer: Arc<Layer>) {
        debug_assert!(self.base.is_none());
        let mut gone = HashSet::new();
        layer.visible(HashSet::new(), |extent, _| {
            if !self.changed.contains_key(extent) {
                gone.insert(*extent);
            }
        });
        self.changed.retain(|extent, entry| layer.get(extent) != Some(&*entry));
        self.gone = gone;
        self.base = Some(layer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Piece;

    fn piece(branch: u64, number: u64, len: u64) -> Piece {
        Piece { segment: SegmentId { branch, number }, offset: 0, extent_offset: 0, len }
    }

    fn run(branch: u64, number: u64, len: u64) -> Extent {
        Extent { len, data: Data::Pieces(vec![piece(branch, number, len)]) }
    }

    fn held(extents: &Extents) -> Vec<(ExtentId, u64)> {
        let mut held: Vec<_> =
            extents.iter().into_iter().map(|(extent, entry)| (*extent, entry.len)).collect();
        held.sort();
        held
    }

    /// A fork's extents over its parent's at the fork, and the parent's own over it, change each on
    /// their own, and what the layers add up to follows.
    #[test]
    fn layers_are_shared_and_changed_over() {
        let (a, b, c) = (ExtentId::new(1, 0), ExtentId::new(1, 1), ExtentId::new(1, 2));
        let mut parent = Extents::default();
        parent.insert(a, run(1, 0, 4096));
        parent.insert(b, run(1, 0, 8192));
        let layer = parent.freeze(None, b"one");
        assert_eq!(
            (layer.len, layer.stored(), layer.live(SegmentId { branch: 1, number: 0 })),
            (2, 12288, 12288)
        );
        assert!(layer.holds(FileKey::Segment(SegmentId { branch: 1, number: 0 })));
        // Nothing changed: the same layer.
        assert!(Arc::ptr_eq(&parent.freeze(None, b"one"), &layer));

        let mut child = Extents::over(Some(layer.clone()));
        child.remove(&a);
        child.insert(c, run(2, 0, 4096));
        assert_eq!(held(&child), vec![(b, 8192), (c, 4096)]);
        assert_eq!(child.own_entries(), 2);
        assert_eq!(child.shadowed(), vec![(&a, layer.get(&a).unwrap())]);
        parent.remove(&b);
        assert_eq!(held(&parent), vec![(a, 4096)]);
        assert_eq!(held(&child), vec![(b, 8192), (c, 4096)]);

        let (extents, gone) = child.overlay(Some(&layer));
        assert_eq!((extents, gone), (vec![(c, run(2, 0, 4096))], vec![a]));
        // The child forks: a layer over its parent's, which it goes on over.
        let above = child.freeze(Some(&layer), b"two");
        assert_eq!(above.len, 2);
        assert_eq!(above.live(SegmentId { branch: 1, number: 0 }), 8192);
        assert!(!above.holds(FileKey::Segment(SegmentId { branch: 3, number: 0 })));
        assert_eq!(child.overlay(Some(&layer)), (vec![(c, run(2, 0, 4096))], vec![a]));
        child.remove(&c);
        assert_eq!(child.overlay(Some(&layer)), (vec![], vec![a]));
        assert_eq!(held(&child), vec![(b, 8192)]);
    }

    /// Deep chains merge into one layer over the branch's origin.
    #[test]
    fn deep_chains_merge() {
        let mut parent = Extents::default();
        parent.insert(ExtentId::new(1, 0), run(1, 0, 4096));
        let origin = parent.freeze(None, b"");
        let mut extents = Extents::over(Some(origin.clone()));
        for sequence in 1..=2 * MAX_DEPTH as u64 {
            extents.insert(ExtentId::new(2, sequence), run(2, sequence, 4096));
            let layer = extents.freeze(Some(&origin), b"");
            assert!(layer.depth <= MAX_DEPTH + 1);
            assert_eq!(layer.len, sequence as usize + 1);
        }
        let (changed, gone) = extents.overlay(Some(&origin));
        assert_eq!((changed.len(), gone.len()), (2 * MAX_DEPTH, 0));
    }

    /// A fork drops the files of its base all of whose extents it hides, and holds none of its own
    /// in: not those it hides only some of the extents in. The files of the extents it changed are
    /// its own.
    #[test]
    fn holdings_name_own_and_dropped_files() {
        let id = |sequence| ExtentId::new(1, sequence);
        let segment = |branch, number| FileKey::Segment(SegmentId { branch, number });
        let mut parent = Extents::default();
        // Two extents in each of two segments, and one in a file of its own.
        for sequence in 0..4 {
            parent.insert(id(sequence), run(1, sequence / 2, 4096));
        }
        parent.insert(id(4), Extent { len: 1 << 20, data: Data::Own });
        let mut fork = Extents::over(Some(parent.freeze(None, b"")));
        // Both of segment 0's hidden: one freed, the other copied to the fork's own segment.
        fork.remove(&id(0));
        fork.get_mut(&id(1)).unwrap().data = run(2, 0, 4096).data;
        // One of segment 1's hidden, the other held.
        fork.remove(&id(2));
        fork.remove(&id(4));
        let holdings = fork.holdings();
        assert_eq!(holdings.own, BTreeSet::from([segment(2, 0)]));
        assert_eq!(holdings.dropped, BTreeSet::from([segment(1, 0), FileKey::Own(id(4))]));

        // The other of segment 1's changed, but some of its data still there: segment 1 is the
        // fork's own, and not dropped.
        fork.get_mut(&id(3)).unwrap().data =
            Data::Pieces(vec![piece(1, 1, 4096), piece(2, 0, 4096)]);
        let holdings = fork.holdings();
        assert_eq!(holdings.own, BTreeSet::from([segment(1, 1), segment(2, 0)]));
        assert_eq!(holdings.dropped, BTreeSet::from([segment(1, 0), FileKey::Own(id(4))]));
    }

    /// A branch holding all it holds shares a layer holding most of it.
    #[test]
    fn rebasing_keeps_what_is_held() {
        let (a, b, c) = (ExtentId::new(1, 0), ExtentId::new(1, 1), ExtentId::new(1, 2));
        let mut old = Extents::default();
        old.insert(a, run(1, 0, 4096));
        old.insert(b, run(1, 0, 4096));
        let layer = old.freeze(None, b"");
        let mut now = Extents::default();
        now.insert(a, run(1, 0, 4096));
        now.insert(c, run(1, 1, 4096));
        now.rebase(layer);
        assert_eq!(held(&now), vec![(a, 4096), (c, 4096)]);
        assert_eq!(now.own_entries(), 2);
    }
}
