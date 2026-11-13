// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what a pass of the collector over a cell records for the passes after it, and what the
//! cell takes in the bucket (doc-7, Garbage collection, Passes that skip; TASK-395.6, TASK-395.8).
//!
//! It is kept in the cell's collector lease (`<C>gc/lease`, [`layout::gc_lease`]), which every
//! pass over the cell takes, reading it, before anything else: so a pass reads what the pass before
//! it recorded with no request more, and any collector process, of any host, reads the same. A pass
//! that ran to its end records, with a condition on the lease being still its taking, the digest
//! of its listing of the cell, but the collector's own objects (`<C>gc/`); the options it decided
//! by; what it found no live branch needs but for the time since an object was written, each with
//! when it may go ([`Deferred`]); and the cell's objects and bytes, by kind, as of its listing
//! ([`Usage`]). Each taking of the lease carries what is recorded to the next.
//!
//! A pass whose listing has the digest recorded, of options the same, decides what the recorded
//! one did: everything it decides by is under the cell's prefix, and an object there written,
//! rewritten or deleted since changes the listing, or it was recorded with no digest
//! ([`Passed::listing`]). It deletes what was deferred once due, and nothing else (doc-7 has why
//! that is as safe as a pass in full).

use crate::bucket::{self, Bucket, Meta};
use crate::collector::Options;
use crate::layout::{self, Branch, Cell, FORMAT};
use crate::{Error, branch};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

/// A cell's collector lease ([`layout::gc_lease`]): the format of the bucket's objects, first, as a
/// cell's object says it; the epoch of its last taking, and who took it, as a branch's lease
/// ([`layout::Lease`]); and what the last pass over the cell that ran to its end recorded, carried
/// from each taking to the next ([`Passed`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcLease {
    pub format: u32,
    pub epoch: u64,
    /// Who took it: unique to each taking, so that a write of it is recognised.
    pub holder: u64,
    pub passed: Option<Passed>,
}

impl GcLease {
    /// The lease of `cell` `bytes` encode: [`Error::Format`] if it is of another format.
    pub fn decode(cell: Cell, bytes: &[u8]) -> Result<Self, Error> {
        let format: u32 = layout::decode(bytes.get(..4).unwrap_or(bytes))?;
        if format != FORMAT {
            return Err(Error::Format { branch: Branch { cell, id: 0 }, format });
        }
        layout::decode(bytes)
    }

    /// Itself, its usage counting it as it is encoded: one object of the bytes it takes. Its
    /// encoding's length is the same whatever numbers it holds (`bincode`'s are of fixed width), so
    /// counting itself changes it not.
    fn counting_itself(mut self) -> Self {
        loop {
            let len = layout::encode(&self).len() as u64;
            let Some(passed) = &mut self.passed else { return self };
            let itself = Sum { objects: 1, bytes: len };
            let counted = passed.usage.kinds.entry(GC_LEASE.to_owned()).or_default();
            if *counted == itself {
                return self;
            }
            *counted = itself;
        }
    }
}

/// The name of the kind of a cell's collector lease ([`layout::Kind::name`]).
const GC_LEASE: &str = "gc-lease";

/// What a pass over a cell that ran to its end recorded ([`GcLease::passed`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passed {
    /// The digest of the listing of the cell it decided by, but the collector's own objects under
    /// it ([`Listed::digest`]): what a pass listing the same decides as it did. None if it decided
    /// by more than the cell holds, which may change with no object of the cell's changing: a point
    /// of another cell's record, read for a clone of it being made; a fork point found lost; a
    /// branch's recovery that found what it needs lost, and then nothing lost. Or if it deleted
    /// only what a pass before deferred, without deciding anew what goes once that is gone.
    pub listing: Option<[u8; 32]>,
    /// The options it decided by.
    pub decided: Decided,
    /// What it found no live branch needs, but that it was written too recently, each with when it
    /// may go, by the store's clock: in the order of their keys.
    pub deferred: Vec<Deferred>,
    /// The references it retired, under other cells, its cell being dead: a pass that skips the
    /// cell retires them again, as a parent's writer that read the child's record live may write a
    /// reference again after its retirement ([`crate::branch::record_fork`]), which no change of
    /// this cell's shows (found by the simulation, seed 53 of `restores`).
    pub retired: Vec<String>,
    /// The cell's objects and bytes, as of its listing, less what it deleted, and with what it
    /// compacted to.
    pub usage: Usage,
}

impl Passed {
    /// Whether a pass of `options` that listed the cell as `listed` decides as this one did.
    pub(crate) fn holds_for(&self, listed: &Listed, options: &Options) -> bool {
        self.listing == Some(listed.digest) && self.decided == Decided::by(options)
    }
}

/// The options a pass decided by ([`Options`]), as recorded: the fraction below which it compacts,
/// by its bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decided {
    pub compact_below: u64,
    pub grace: u64,
}

impl Decided {
    pub(crate) fn by(options: &Options) -> Self {
        Self { compact_below: options.compact_below.to_bits(), grace: options.grace }
    }
}

/// An object a pass found no live branch needs once a grace period went by since an object was
/// written: one compacted, its moves too recent; moves of an object gone, or compacted in turn
/// ([`crate::collector`]). Its key, and when it may go, by the store's clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deferred {
    pub key: String,
    pub due: u64,
}

/// What a cell takes in the bucket, as a pass's listing found it ([`Passed::usage`],
/// TASK-395.8).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// When the listing it is of began, by the store's clock: milliseconds since the Unix epoch on
    /// a real store.
    pub measured: u64,
    /// Objects, and their bytes as stored, by kind ([`layout::Kind::name`]): the cell's branches'
    /// objects, what the collector compacted of them, its lease and marks, and the references of
    /// forks of its branches; the garbage among them the collector has yet to reclaim too.
    pub kinds: BTreeMap<String, Sum>,
}

impl Usage {
    /// Counts the object at `key`, of `size` bytes.
    pub(crate) fn add(&mut self, key: &str, size: u64) {
        let sum = self.kinds.entry(layout::kind(key).name().to_owned()).or_default();
        sum.objects += 1;
        sum.bytes += size;
    }

    /// Counts the object at `key`, of `size` bytes, no longer.
    pub(crate) fn remove(&mut self, key: &str, size: u64) {
        let name = layout::kind(key).name();
        let Some(sum) = self.kinds.get_mut(name) else { return };
        sum.objects = sum.objects.saturating_sub(1);
        sum.bytes = sum.bytes.saturating_sub(size);
        if sum.objects == 0 {
            self.kinds.remove(name);
        }
    }

    /// Its objects and bytes, of every kind.
    pub fn total(&self) -> Sum {
        let mut total = Sum::default();
        self.kinds.values().for_each(|sum| total += *sum);
        total
    }
}

/// Objects, and their bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sum {
    pub objects: u64,
    pub bytes: u64,
}

impl std::ops::AddAssign for Sum {
    fn add_assign(&mut self, other: Self) {
        self.objects += other.objects;
        self.bytes += other.bytes;
    }
}

/// What a pass's listing of a cell found.
pub(crate) struct Listed {
    /// Every object of the cell, by its key, but the references of forks of its branches, which are
    /// never candidates, nor named by any state.
    pub objects: BTreeMap<String, Meta>,
    /// SHA-256 over every object listed but the collector's own (`<C>gc/`), in the order of their
    /// keys: each one's key, size, time written, and version (its ETag and its version, as the
    /// store names them). Its lease, which every pass writes, and its marks, which no pass decides
    /// by, change it not.
    pub digest: [u8; 32],
    /// Every object listed but the cell's collector lease, which recording counts as it writes it
    /// ([`record`]).
    pub usage: Usage,
}

/// Lists `cell`, once.
pub(crate) async fn list(bucket: &impl Bucket, cell: Cell) -> Listed {
    let measured = bucket.now();
    let mut listed = bucket::list(bucket, &cell.prefix()).await;
    listed.sort_by(|a, b| a.key.cmp(&b.key));
    let (own, lease) = (layout::collectors(cell), layout::gc_lease(cell));
    let mut digest = Sha256::new();
    let mut usage = Usage { measured, ..Usage::default() };
    let mut objects = BTreeMap::new();
    for meta in listed {
        if meta.key != lease {
            usage.add(&meta.key, meta.size);
        }
        if !meta.key.starts_with(&own) {
            digest_of(&mut digest, &meta);
        }
        if !matches!(layout::kind(&meta.key), layout::Kind::Reference { .. }) {
            objects.insert(meta.key.clone(), meta);
        }
    }
    Listed { objects, digest: digest.finalize().into(), usage }
}

/// Feeds `meta` to `digest`, every part of it after its length, so that no two listings feed the
/// same bytes.
fn digest_of(digest: &mut Sha256, meta: &Meta) {
    let text = |digest: &mut Sha256, text: Option<&str>| match text {
        None => digest.update([0]),
        Some(text) => {
            digest.update([1]);
            digest.update((text.len() as u64).to_le_bytes());
            digest.update(text.as_bytes());
        }
    };
    text(digest, Some(&meta.key));
    digest.update(meta.size.to_le_bytes());
    digest.update(meta.modified.to_le_bytes());
    text(digest, meta.version.e_tag.as_deref());
    text(digest, meta.version.version.as_deref());
}

/// What a pass's taking of its cell's collector lease found ([`take`]).
pub(crate) struct Taken {
    /// The epoch it took.
    pub epoch: u64,
    /// What the last pass over the cell that ran to its end recorded.
    pub passed: Option<Passed>,
}

/// The collector lease of `cell` as a read finds it, if there is one, for a taking of it after
/// ([`take`]).
pub(crate) async fn found(bucket: &impl Bucket, cell: Cell) -> Option<bucket::Object> {
    bucket::get(bucket, &layout::gc_lease(cell)).await
}

/// Takes the collector lease of `cell` as `holder`, at the epoch after its last taking's, from
/// `found`, what a read of it found ([`found`]): carrying what it holds to the next taking.
pub(crate) async fn take(
    bucket: &impl Bucket,
    cell: Cell,
    found: Option<bucket::Object>,
    holder: u64,
) -> Result<Taken, Error> {
    let start = branch::Start::Found(found);
    let decode = |bytes: &[u8]| GcLease::decode(cell, bytes);
    branch::transition(bucket, &layout::gc_lease(cell), start, decode, |found| {
        let (epoch, passed) = match found {
            None => (1, None),
            Some(lease) => (lease.epoch + 1, lease.passed),
        };
        let lease = GcLease { format: FORMAT, epoch, holder, passed: passed.clone() };
        Ok(branch::Step::Write(lease, Taken { epoch, passed }))
    })
    .await
}

/// Records `passed` in the collector lease of `cell`, if it is still the taking of `epoch` by
/// `holder`: whether it was. Its usage counts the lease as it is written.
pub(crate) async fn record(
    bucket: &impl Bucket,
    cell: Cell,
    epoch: u64,
    holder: u64,
    passed: Passed,
) -> Result<bool, Error> {
    let decode = |bytes: &[u8]| GcLease::decode(cell, bytes);
    let key = layout::gc_lease(cell);
    branch::transition(bucket, &key, branch::Start::Read, decode, |found| match found {
        Some(lease) if lease.epoch == epoch && lease.holder == holder => {
            let lease = GcLease { passed: Some(passed.clone()), ..lease }.counting_itself();
            Ok(branch::Step::Write(lease, true))
        }
        _ => Ok(branch::Step::Stop(false)),
    })
    .await
}

/// The collector lease of `cell`, if there is one: [`Error::Format`] if of another format.
pub async fn read(bucket: &impl Bucket, cell: Cell) -> Result<Option<GcLease>, Error> {
    found(bucket, cell).await.map(|object| GcLease::decode(cell, &object.bytes)).transpose()
}

/// The epoch of the last taking of the collector lease of `cell`: 0 if none took it.
pub(crate) async fn epoch(bucket: &impl Bucket, cell: Cell) -> Result<u64, Error> {
    Ok(read(bucket, cell).await?.map_or(0, |lease| lease.epoch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::Version;
    use crate::layout::TEST_CELL;

    fn meta(key: &str, size: u64, modified: u64, e_tag: &str) -> Meta {
        let version = Version { e_tag: Some(e_tag.to_owned()), version: None };
        Meta { key: key.to_owned(), version, size, modified }
    }

    fn digest(metas: &[Meta]) -> [u8; 32] {
        let mut digest = Sha256::new();
        metas.iter().for_each(|meta| digest_of(&mut digest, meta));
        digest.finalize().into()
    }

    /// A listing's digest changes with any part of any object listed: its key, its size, when it
    /// was written, its version; and not with where one key ends and the next begins.
    #[test]
    fn digests_change_with_every_part_of_a_listing() {
        let record = layout::record(Branch { cell: TEST_CELL, id: 1 });
        let base = [meta(&record, 10, 5, "1")];
        let changed = [
            vec![meta(&record, 11, 5, "1")],
            vec![meta(&record, 10, 6, "1")],
            vec![meta(&record, 10, 5, "2")],
            vec![Meta { version: Version::default(), ..meta(&record, 10, 5, "1") }],
            vec![meta(&format!("{record}0"), 10, 5, "1")],
            vec![meta(&record, 10, 5, "1"), meta(&format!("{record}0"), 10, 5, "1")],
            Vec::new(),
        ];
        for changed in changed {
            assert_ne!(digest(&changed), digest(&base), "{changed:?}");
        }
        assert_eq!(digest(&base), digest(&[meta(&record, 10, 5, "1")]));
        let joined = [meta("ab", 1, 1, "c"), meta("d", 1, 1, "e")];
        let split = [meta("a", 1, 1, "bc"), meta("d", 1, 1, "e")];
        assert_ne!(digest(&joined), digest(&split));
    }

    /// A lease's usage counts the lease as it is encoded, one object of its bytes.
    #[test]
    fn leases_count_themselves() {
        let mut usage = Usage { measured: 3, ..Usage::default() };
        usage.add(&layout::record(Branch { cell: TEST_CELL, id: 1 }), 40);
        let deferred = vec![Deferred { key: "k".into(), due: 9 }];
        let passed = Passed {
            listing: Some([7; 32]),
            decided: Decided { compact_below: 0, grace: 0 },
            deferred,
            retired: vec!["r".into()],
            usage,
        };
        let lease = GcLease { format: FORMAT, epoch: 2, holder: 3, passed: Some(passed) };
        let lease = lease.counting_itself();
        let len = layout::encode(&lease).len() as u64;
        let usage = &lease.passed.as_ref().unwrap().usage;
        assert_eq!(usage.kinds[GC_LEASE], Sum { objects: 1, bytes: len });
        assert_eq!(usage.total(), Sum { objects: 2, bytes: 40 + len });
        assert_eq!(GcLease::decode(TEST_CELL, &layout::encode(&lease)), Ok(lease));
    }

    /// A lease of another format is refused by name, before the rest of it is read.
    #[test]
    fn leases_of_other_formats_are_refused() {
        for format in [FORMAT + 1, 8] {
            let lease = GcLease { format, epoch: 1, holder: 2, passed: None };
            let refused = Error::Format { branch: Branch { cell: TEST_CELL, id: 0 }, format };
            assert_eq!(GcLease::decode(TEST_CELL, &layout::encode(&lease)), Err(refused));
        }
    }
}
