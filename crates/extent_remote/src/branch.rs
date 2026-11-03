// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Branches' records and leases: creating a branch, forking one, taking one over, deleting one.
//!
//! A fork of a branch with a writer is the writer's commit ([`record_fork`] gives its child its
//! record once the commit is acknowledged). A fork of a point is its child's record alone, which
//! writes nothing of the point's, so that any number of hosts fork one at once ([`fork_point`]).

use crate::bucket::{self, Bucket, Created, Updated};
use crate::layout::{self, Kind, Lease, Origin, Record};
use crate::recovery::{self, recover};
use crate::{Error, rules};
use std::collections::BTreeMap;

/// Creates `branch`, of no parent.
pub async fn create(bucket: &impl Bucket, branch: u64) -> Result<(), Error> {
    let record = Record::default();
    match bucket::create(bucket, &layout::record(branch), layout::encode(&record)).await {
        Created::Created => Ok(()),
        Created::Exists(_) => Err(Error::Corrupt(format!("branch {branch:x} exists"))),
    }
}

/// Creates `child`, a fork's child taking a branch of its own, of no parent, its fork not made in
/// object storage. Its record says it is a fork's, and differs from what [`create`] writes, so that
/// it takes no record of another branch's that took its ID for its own, which any create of the
/// same bytes would ([`Error::ChildTaken`], TASK-79.82).
pub async fn create_forked(bucket: &impl Bucket, child: u64) -> Result<(), Error> {
    let record = Record { forked: true, ..Record::default() };
    match bucket::create(bucket, &layout::record(child), layout::encode(&record)).await {
        Created::Created => Ok(()),
        Created::Exists(_) => Err(Error::ChildTaken(child)),
    }
}

pub async fn record(bucket: &impl Bucket, branch: u64) -> Result<Option<Record>, Error> {
    match bucket::get(bucket, &layout::record(branch)).await {
        Some(object) => Ok(Some(layout::decode(&object.bytes)?)),
        None => Ok(None),
    }
}

/// The branches in `bucket` whose records say they are live, with their records: an operator's
/// look, to compare with what hosts hold. A fork whose child has no record yet isn't among them.
pub async fn live(bucket: &impl Bucket) -> Result<BTreeMap<u64, Record>, Error> {
    let mut live = BTreeMap::new();
    for meta in bucket::list(bucket, layout::BRANCHES).await {
        if let Kind::Record { branch } = layout::kind(&meta.key)
            && let Some(record) = record(bucket, branch).await?
            && !record.deleted
        {
            live.insert(branch, record);
        }
    }
    Ok(live)
}

/// Gives `child` its record, forked from `origin`, a point if `point`, unless it has one. A fork's
/// child gets it once the fork is acknowledged, from the parent's writer, or the next one, or the
/// parent's deletion.
pub async fn record_fork(
    bucket: &impl Bucket,
    child: u64,
    origin: Origin,
    point: bool,
) -> Result<(), Error> {
    failpoint!("branch::record_fork");
    let record = Record { origin: Some(origin), forked: true, point, ..Record::default() };
    match bucket::create(bucket, &layout::record(child), layout::encode(&record)).await {
        Created::Created => {
            failpoint!("branch::recorded_fork");
            Ok(())
        }
        Created::Exists(object) => {
            let existing: Record = layout::decode(&object.bytes)?;
            // Deleted before it got its record, perhaps.
            if existing.origin == Some(origin) || existing.deleted {
                Ok(())
            } else {
                Err(Error::ChildTaken(child))
            }
        }
    }
}

/// Forks `point`, whose record names `origin`, to `child`: `child`'s record, naming `origin` as
/// its own, and writes nothing of the point's or of the branch it names, so that any number of
/// hosts fork a point at once, and none's fork is any other's business (doc-7, Points). To recovery
/// and the collector, `child` is then a fork of that branch at that segment.
///
/// What keeps what `child` inherited until it has a record is the point's. So the record comes
/// first, saying the fork is being made ([`Record::forking`]), then a read of the point's record
/// ([`finish_point_fork`]): a pass of the collector that went on the point's being deleted read
/// that after the deletion, and so after this read if it found the point live, and so listed
/// `child`'s record after it was made, and keeps what it names. Otherwise the fork fails
/// ([`Error::Deleted`]), and its host deletes `child` ([`delete_fork`]). Forked again, as a store
/// left by a crash finishes it, it goes on from where it stopped. A child with another's record is
/// another branch's ([`Error::ChildTaken`]).
pub async fn fork_point(
    bucket: &impl Bucket,
    child: u64,
    point: u64,
    origin: Origin,
) -> Result<(), Error> {
    failpoint!("point::record");
    let forking = rules().forks_pending.then_some(point);
    let record = Record { origin: Some(origin), forked: true, forking, ..Record::default() };
    match bucket::create(bucket, &layout::record(child), layout::encode(&record)).await {
        Created::Created => {}
        Created::Exists(object) => {
            let existing: Record = layout::decode(&object.bytes)?;
            if existing.deleted {
                return Err(Error::Deleted);
            }
            if existing.origin != Some(origin) || !existing.forked {
                return Err(Error::ChildTaken(child));
            }
            if existing.forking.is_none() {
                return Ok(());
            }
        }
    }
    failpoint!("point::recorded");
    finish_point_fork(bucket, child, point).await
}

/// Makes the fork being made of `point` to `child` ([`fork_point`]), whose record is created: fails
/// with [`Error::Deleted`] if the point was deleted, or the fork was given up on.
pub async fn finish_point_fork(bucket: &impl Bucket, child: u64, point: u64) -> Result<(), Error> {
    if rules().forks_check_points
        && !record(bucket, point).await?.is_some_and(|record| !record.deleted)
    {
        return Err(Error::Deleted);
    }
    failpoint!("point::checked");
    let key = layout::record(child);
    loop {
        let Some(object) = bucket::get(bucket, &key).await else { return Err(Error::NoBranch) };
        let record: Record = layout::decode(&object.bytes)?;
        if record.deleted {
            return Err(Error::Deleted);
        }
        if record.forking.is_none() {
            return Ok(());
        }
        let made = layout::encode(&Record { forking: None, ..record });
        // Conditional, against its being deleted, or given up on ([`abandon`]). A record's bytes
        // repeat only for the same change, which is then made.
        if let Updated::Updated = bucket::update(bucket, &key, made, &object.meta.version).await {
            failpoint!("point::made");
            return Ok(());
        }
    }
}

/// Whether no writer has `branch`: it has no lease, or its deletion's.
pub(crate) async fn unwritten(bucket: &impl Bucket, branch: u64) -> Result<bool, Error> {
    let epoch = lease_epoch(bucket, &layout::lease(branch)).await?;
    Ok(epoch == 0 || epoch == Lease::FINAL)
}

/// Deletes `branch`, whose record says a failed fork's deletion claimed it ([`Record::claimed`]),
/// and whose state is lost, as what it named went before that landed: unless it was given another
/// record since. Nothing ever read it: no writer recovers what a record not there names.
pub(crate) async fn abandon_claim(bucket: &impl Bucket, branch: u64) -> Result<(), Error> {
    mark_deleted(bucket, branch).await
}

/// Gives up on `child`, a fork being made of a point that was deleted ([`Record::forking`]), so
/// that nothing keeps what it names, unless the fork was made first: returns its record as it then
/// stands. Its fork, which fails then, wrote nothing: a branch that holds nothing and that nothing
/// relies on, what it names perhaps gone, which recovering it would stop the collector on.
pub(crate) async fn abandon(bucket: &impl Bucket, child: u64) -> Result<Record, Error> {
    let key = layout::record(child);
    loop {
        let Some(object) = bucket::get(bucket, &key).await else { return Err(Error::NoBranch) };
        let record: Record = layout::decode(&object.bytes)?;
        if record.deleted || record.forking.is_none() {
            return Ok(record);
        }
        let abandoned = Record { deleted: true, ..record };
        let bytes = layout::encode(&abandoned);
        if let Updated::Updated = bucket::update(bucket, &key, bytes, &object.meta.version).await {
            return Ok(abandoned);
        }
    }
}

/// Takes the lease at `key`, of the epoch after its last, as `holder`, which must differ from every
/// other taking's, of the placement whose fencing token is `token`. Fails for a deleted branch's,
/// and for one a later placement took, whose token is higher ([`Error::Stale`]): a placement cut
/// off takes no branch back from the one after it (TASK-39.7).
pub async fn take_lease(
    bucket: &impl Bucket,
    key: &str,
    holder: u64,
    token: u64,
) -> Result<u64, Error> {
    loop {
        match bucket::get(bucket, key).await {
            None => {
                let lease = Lease { epoch: 1, holder, token };
                if let Created::Created = bucket::create(bucket, key, layout::encode(&lease)).await
                {
                    return Ok(lease.epoch);
                }
            }
            Some(object) => {
                let lease = Lease::decode(&object.bytes)?;
                if lease.epoch == Lease::FINAL {
                    return Err(Error::Deleted);
                }
                if rules().tokens && lease.token > token {
                    return Err(Error::Stale);
                }
                let lease = Lease { epoch: lease.epoch + 1, holder, token };
                let bytes = layout::encode(&lease);
                if let Updated::Updated =
                    bucket::update(bucket, key, bytes, &object.meta.version).await
                {
                    return Ok(lease.epoch);
                }
            }
        }
    }
}

/// The epoch of the lease at `key`: 0 if there is none.
pub async fn lease_epoch(bucket: &impl Bucket, key: &str) -> Result<u64, Error> {
    match bucket::get(bucket, key).await {
        Some(object) => Ok(Lease::decode(&object.bytes)?.epoch),
        None => Ok(0),
    }
}

/// Deletes `branch`, as `holder`, of the placement whose fencing token is `token`: no writer takes
/// it again, the one it has stops at its next acknowledgement, and the collector reclaims what no
/// other branch inherited from it. Deleting it again does nothing, though the collector reclaimed
/// that since. Fails for a branch a later placement took, whose token is higher
/// ([`Error::Stale`]), leaving it as it was: a placement cut off deletes no branch from under the
/// one after it (TASK-45.1).
pub async fn delete(
    bucket: &impl Bucket,
    branch: u64,
    holder: u64,
    token: u64,
) -> Result<(), Error> {
    end_lease(bucket, branch, holder, token).await?;
    failpoint!("branch::lease_ended");
    let record = loop {
        if let Some(record) = record(bucket, branch).await? {
            break record;
        }
        // A fork's child before its record, or no branch: either way none is, from now, unless
        // its record came meanwhile.
        let deleted = Record { forked: true, deleted: true, ..Record::default() };
        if let Created::Created =
            bucket::create(bucket, &layout::record(branch), layout::encode(&deleted)).await
        {
            return Ok(());
        }
    };
    // Deleted already: its children got their records before, and what recovering it read, no
    // live branch needs, the collector may have deleted since (TASK-79.78).
    if record.deleted {
        return Ok(());
    }
    // Its children get their records, so that the collector finds them without its state. A fork
    // of a point still being made has none, and may name what is gone; so may a claim, with
    // neither a writer nor a child, if it does.
    if record.forking.is_none() {
        let forks = match recover(bucket, branch, record.origin).await {
            Ok(recovered) => recovered.state.forks,
            Err(Error::Lost(_)) if record.claimed && unwritten(bucket, branch).await? => Vec::new(),
            Err(error) => return Err(error),
        };
        for fork in &forks {
            let origin = Origin { parent: branch, at: fork.at };
            match record_fork(bucket, fork.child, origin, fork.point).await {
                Ok(()) | Err(Error::ChildTaken(_)) => {}
                Err(error) => return Err(error),
            }
        }
    }
    failpoint!("branch::children_recorded");
    mark_deleted(bucket, branch).await
}

/// Ends `branch`'s lease, as [`delete`] does first: its final epoch, which no writer takes, as
/// `holder` of the placement whose fencing token is `token`, unless a later placement took it
/// ([`Error::Stale`]).
async fn end_lease(
    bucket: &impl Bucket,
    branch: u64,
    holder: u64,
    token: u64,
) -> Result<(), Error> {
    let key = layout::lease(branch);
    let lease = layout::encode(&Lease { epoch: Lease::FINAL, holder, token: u64::MAX });
    loop {
        match bucket::get(bucket, &key).await {
            None => {
                if let Created::Created = bucket::create(bucket, &key, lease.clone()).await {
                    return Ok(());
                }
            }
            Some(object) => {
                let current = Lease::decode(&object.bytes)?;
                if current.epoch == Lease::FINAL {
                    return Ok(());
                }
                if rules().delete_tokens && current.token > token {
                    return Err(Error::Stale);
                }
                if let Updated::Updated =
                    bucket::update(bucket, &key, lease.clone(), &object.meta.version).await
                {
                    return Ok(());
                }
            }
        }
    }
}

/// Marks `branch`'s record deleted, as [`delete`] does last.
async fn mark_deleted(bucket: &impl Bucket, branch: u64) -> Result<(), Error> {
    let key = layout::record(branch);
    loop {
        let Some(object) = bucket::get(bucket, &key).await else {
            return Err(Error::NoBranch);
        };
        let record: Record = layout::decode(&object.bytes)?;
        if record.deleted {
            return Ok(());
        }
        let deleted = layout::encode(&Record { deleted: true, ..record });
        // A record's bytes repeat only for the same change, which is then made.
        if let Updated::Updated = bucket::update(bucket, &key, deleted, &object.meta.version).await
        {
            return Ok(());
        }
    }
}

/// Where a fork's host knows it was made, which deleting its child goes by ([`delete_fork`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkedAt {
    /// Nowhere in object storage: its segment never went there.
    Nowhere,
    /// At the end of a segment of its parent's, which may have landed, and whose branch's writers
    /// then give the child a record.
    Segment(Origin),
    /// So, its child a point: whatever record it gets says so, whoever gives it.
    ToPoint(Origin),
    /// Of a point, which names this origin. Nothing but the fork itself gives its child a record.
    OfPoint(Origin),
}

impl ForkedAt {
    /// The origin its child's record names, if any.
    fn origin(self) -> Option<Origin> {
        match self {
            Self::Nowhere => None,
            Self::Segment(origin) | Self::ToPoint(origin) | Self::OfPoint(origin) => Some(origin),
        }
    }
}

impl From<Option<Origin>> for ForkedAt {
    fn from(origin: Option<Origin>) -> Self {
        origin.map_or(Self::Nowhere, Self::Segment)
    }
}

/// Deletes `child`, the child of a fork, as [`delete`] does, but only if its branch is the one the
/// fork made: its record names `origin`, the fork's place in its parent's branch, or it has no
/// record yet, as a fork whose segment has not been acknowledged has not. A fork whose segment never
/// went to object storage names no `origin`: its child has no record but the one its deletion
/// gives it. A branch of another's, whose ID the fork's child was given, as a guest may give any,
/// is left as it is (TASK-79.82); so is a branch deleted already, whoever's. A child that took a
/// branch of its own, its fork not made ([`create_forked`]), is the fork's. Returns whether the
/// fork's branch is gone, deleted now or before: `false` for another's.
///
/// A child with no record yet gets one first, before the deletion ends its lease: created only if
/// no record is there, which another branch taking the ID meanwhile has, whose lease the deletion
/// took otherwise (TASK-79.82). Its record taken, the ID is no other's, and a deletion that stops
/// goes on as the fork's; it had no record before, nor children with none, as [`delete`] deletes
/// such a branch. The record is the one its fork would give it ([`record_fork`]), live, so that a
/// deletion a later placement refuses ([`Error::Stale`]) changes nothing, only if its parent's
/// branch holds the fork's segment, as a live branch's record must say: a child naming a segment
/// that is not there recovers from nothing, which fails its deletion, should one stopping go on,
/// and every pass of the collector. It says it is a claim ([`Record::claimed`]): that the parent's
/// branch holds a segment of that number is no proof it is the fork's, and nothing keeps what it
/// names until the record lands, a dead host's perhaps, long after. Otherwise it is deleted from
/// the first, for good: no other
/// placement took a branch nothing of which landed; and a segment landing after finds it so. So
/// is a fork of a point with no record, which holds nothing, a create of its record, a dead host's
/// landing late, finding it so too; and a point with none: no placement refuses a deletion of what
/// has no lease, and a stale copy finds no live record naming a segment that may not be its fork's.
pub async fn delete_fork(
    bucket: &impl Bucket,
    child: u64,
    forked: impl Into<ForkedAt>,
    holder: u64,
    token: u64,
) -> Result<bool, Error> {
    let forked = forked.into();
    let origin = forked.origin();
    loop {
        match record(bucket, child).await? {
            Some(record) if record.deleted => return Ok(true),
            Some(record) if !record.of_fork(origin) => return Ok(false),
            Some(_) => return delete(bucket, child, holder, token).await.map(|()| true),
            None => {
                if origin.is_none() && !rules().fork_claims_ids {
                    return delete(bucket, child, holder, token).await.map(|()| true);
                }
                let live = match forked {
                    ForkedAt::Segment(_) if !rules().fork_claims_landed => true,
                    ForkedAt::Segment(origin) => {
                        recovery::holds(bucket, origin.parent, origin.at).await?
                    }
                    ForkedAt::Nowhere | ForkedAt::ToPoint(_) | ForkedAt::OfPoint(_) => false,
                };
                let point = matches!(forked, ForkedAt::ToPoint(_));
                let record = Record {
                    origin,
                    forked: true,
                    deleted: !live,
                    point,
                    claimed: live && rules().fork_claims_landed,
                    ..Record::default()
                };
                let key = layout::record(child);
                if let Created::Created =
                    bucket::create(bucket, &key, layout::encode(&record)).await
                {
                    failpoint!("branch::claimed");
                    end_lease(bucket, child, holder, token).await?;
                    failpoint!("branch::claim_lease_ended");
                    if live {
                        mark_deleted(bucket, child).await?;
                    }
                    return Ok(true);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::bucket::{Direct, SimBucket};
    use futures::executor::block_on;

    /// TASK-79.82: a fork's child is deleted only if its branch is the fork's: one with no record,
    /// or whose record names the fork's origin. One whose record is another's, of no parent, or of
    /// another point of the fork's parent, is left as it is, as is one whose record names the
    /// parent while the fork's segment never went, the fork naming no origin. One deleted is gone
    /// already.
    #[test]
    fn forks_delete_only_their_own_branches() {
        use crate::batch::{Batch, Commit};
        use crate::writer::Writer;
        let memory = std::sync::Arc::new(object_store::memory::InMemory::new());
        let bucket = std::sync::Arc::new(crate::store::Store::new(memory, ""));
        let live = |child| {
            let record = block_on(record(&*bucket, child)).unwrap();
            record.map(|record| !record.deleted)
        };
        // A fork of branch 1's to 3, made, its child given its record.
        let origin = block_on(async {
            create(&*bucket, 1).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let forking = Batch::from(Commit { tag: 1, ..Commit::default() });
            let at = parent.add_segment(forking.fork(3, &mut parent.chain()).frame()).unwrap();
            parent.upload(at).await.unwrap();
            parent.acknowledge().await.unwrap();
            Origin { parent: 1, at }
        });
        assert_eq!(live(3), Some(true));
        assert_eq!(block_on(delete_fork(&*bucket, 3, Some(origin), 9, 0)), Ok(true));
        assert_eq!(live(3), Some(false));
        assert_eq!(block_on(delete_fork(&*bucket, 3, None, 9, 0)), Ok(true));
        // Forks whose children have no record yet, whose segments went or not.
        assert_eq!(block_on(delete_fork(&*bucket, 2, Some(origin), 9, 0)), Ok(true));
        assert_eq!(block_on(delete_fork(&*bucket, 4, None, 9, 0)), Ok(true));
        assert_eq!((live(2), live(4)), (Some(false), Some(false)));

        block_on(create(&*bucket, 5)).unwrap();
        block_on(record_fork(&*bucket, 6, Origin { at: origin.at + 1, ..origin }, false)).unwrap();
        block_on(record_fork(&*bucket, 7, origin, false)).unwrap();
        for (child, origin) in [(5, Some(origin)), (6, Some(origin)), (7, None)] {
            assert_eq!(block_on(delete_fork(&*bucket, child, origin, 9, 0)), Ok(false), "{child}");
            assert_eq!(live(child), Some(true), "{child}");
            let lease = block_on(take_lease(&*bucket, &layout::lease(child), 10 + child, 0));
            assert_eq!(lease, Ok(1), "{child}");
        }
    }

    /// A fork's child gets one record: given again from the same origin, it stands; from another,
    /// the child is another branch's.
    #[test]
    fn a_child_gets_one_record() {
        let bucket = Direct(SimBucket::new());
        let origin = Origin { parent: 1, at: 7 };
        block_on(record_fork(&bucket, 2, origin, false)).unwrap();
        block_on(record_fork(&bucket, 2, origin, false)).unwrap();
        let other = Origin { parent: 1, at: 8 };
        assert_eq!(block_on(record_fork(&bucket, 2, other, false)), Err(Error::ChildTaken(2)));
        let record = block_on(record(&bucket, 2)).unwrap();
        let forked = Record { origin: Some(origin), forked: true, ..Record::default() };
        assert_eq!(record, Some(forked));
    }

    /// TASK-79.82: a branch that takes a failed fork's child's ID after the fork's deletion found
    /// it free, before the deletion takes the child's lease, is left be: its record taken and its
    /// lease kept, not the deletion's.
    #[test]
    fn forks_deleted_leave_a_branch_taking_their_childs_id_meanwhile() {
        use crate::bucket::{ByteRange, Error as BucketError, Meta, Object, PutMode, Version};
        use std::future::Future;
        use std::sync::atomic::{AtomicBool, Ordering};

        /// A bucket in which branch `child` is created, its lease taken, as its record is first
        /// found missing.
        struct Racing {
            bucket: Direct,
            child: u64,
            raced: AtomicBool,
        }

        impl Bucket for Racing {
            fn get(&self, key: &str) -> impl Future<Output = Result<Object, BucketError>> + Send {
                let key = key.to_owned();
                async move {
                    let got = self.bucket.get(&key).await;
                    if key == layout::record(self.child)
                        && matches!(got, Err(BucketError::NotFound))
                        && !self.raced.swap(true, Ordering::SeqCst)
                    {
                        create(&self.bucket, self.child).await.unwrap();
                        let lease = layout::lease(self.child);
                        take_lease(&self.bucket, &lease, 7, 0).await.unwrap();
                    }
                    got
                }
            }

            fn get_range(
                &self,
                key: &str,
                range: ByteRange,
            ) -> impl Future<Output = Result<Vec<u8>, BucketError>> + Send {
                self.bucket.get_range(key, range)
            }

            fn head(&self, key: &str) -> impl Future<Output = Result<Meta, BucketError>> + Send {
                self.bucket.head(key)
            }

            fn list(
                &self,
                prefix: &str,
            ) -> impl Future<Output = Result<Vec<Meta>, BucketError>> + Send {
                self.bucket.list(prefix)
            }

            fn put(
                &self,
                key: &str,
                bytes: Vec<u8>,
                mode: PutMode,
            ) -> impl Future<Output = Result<Version, BucketError>> + Send {
                self.bucket.put(key, bytes, mode)
            }

            fn delete(&self, key: &str) -> impl Future<Output = Result<(), BucketError>> + Send {
                self.bucket.delete(key)
            }

            fn now(&self) -> u64 {
                self.bucket.now()
            }
        }

        let racing =
            Racing { bucket: Direct(SimBucket::new()), child: 3, raced: AtomicBool::new(false) };
        let origin = Origin { parent: 1, at: 7 };
        assert_eq!(block_on(delete_fork(&racing, 3, Some(origin), 9, 0)), Ok(false));
        let record = block_on(record(&racing.bucket, 3)).unwrap();
        assert_eq!(record, Some(Record::default()));
        let lease = block_on(bucket::get(&racing.bucket, &layout::lease(3))).unwrap();
        let lease = Lease::decode(&lease.bytes).unwrap();
        assert_eq!((lease.epoch, lease.holder), (1, 7), "the other branch's lease");
    }

    /// TASK-79.82: a parent whose state names a fork to an ID another branch took is deleted all
    /// the same, the other branch left as it is.
    #[test]
    fn parents_are_deleted_though_another_branch_took_their_forks_id() {
        use crate::batch::{Batch, Commit};
        use crate::writer::Writer;
        let memory = std::sync::Arc::new(object_store::memory::InMemory::new());
        let bucket = std::sync::Arc::new(crate::store::Store::new(memory, ""));
        block_on(async {
            create(&*bucket, 1).await.unwrap();
            create(&*bucket, 2).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let forking = Batch::from(Commit { tag: 1, ..Commit::default() });
            let at = parent.add_segment(forking.fork(2, &mut parent.chain()).frame()).unwrap();
            parent.upload(at).await.unwrap();
            assert_eq!(parent.acknowledge().await, Ok(Some(at)));
            assert!(parent.refused(2));
            assert_eq!(delete(&*bucket, 1, 20, 0).await, Ok(()));
            assert_eq!(record(&*bucket, 2).await, Ok(Some(Record::default())));
        });
    }

    /// A failed fork's claim landing, as a dead host's write may, once a pass of the collector
    /// took what it names, nothing having kept that: at 2 of the parent's, in fact the fence of its
    /// next writer, its fork's segment void. It holds up no pass, and its deletion finishes. Found
    /// by the simulation, seed 3184 of its `restores`.
    #[test]
    fn claims_landing_after_what_they_name_went_hold_up_nothing() {
        use crate::batch::{Batch, Commit};
        use crate::collector::{Options, collect};
        use crate::writer::Writer;
        let memory = std::sync::Arc::new(object_store::memory::InMemory::new());
        let bucket = std::sync::Arc::new(crate::store::Store::new(memory, ""));
        const NOW: Options = Options { compact_below: 0.0, grace: 0 };
        block_on(async {
            create(&*bucket, 1).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            for tag in 1..=3 {
                let commit = Batch::from(Commit { tag, ..Commit::default() });
                let seq = parent.add_segment(commit.close(&mut parent.chain()).frame()).unwrap();
                parent.upload(seq).await.unwrap();
                parent.acknowledge().await.unwrap();
            }
            parent.checkpoint().await.unwrap();
            collect(&*bucket, 1, &NOW).await.unwrap();
            let origin = Some(Origin { parent: 1, at: 1 });
            let lost = "branch 3: what its recovery starts from keeps going".to_owned();
            assert_eq!(recover(&*bucket, 3, origin).await.map(drop), Err(Error::Lost(lost)));

            let claim = Record { origin, forked: true, claimed: true, ..Record::default() };
            let (key, bytes) = (layout::record(3), layout::encode(&claim));
            assert!(matches!(bucket::create(&*bucket, &key, bytes).await, Created::Created));
            collect(&*bucket, 2, &NOW).await.unwrap();
            let deleted = Record { deleted: true, ..claim };
            assert_eq!(record(&*bucket, 3).await, Ok(Some(deleted)));

            // Its deletion, which stopped, finds it so; or, the pass not there first, finishes it.
            let claim = Record { origin, forked: true, claimed: true, ..Record::default() };
            let (key, bytes) = (layout::record(4), layout::encode(&claim));
            assert!(matches!(bucket::create(&*bucket, &key, bytes).await, Created::Created));
            assert_eq!(delete_fork(&*bucket, 4, origin, 9, 0).await, Ok(true));
            assert_eq!(record(&*bucket, 4).await, Ok(Some(Record { deleted: true, ..claim })));

            // But one a writer has lost something: it stops the pass, as any branch's.
            let claim = Record { origin, forked: true, claimed: true, ..Record::default() };
            let (key, bytes) = (layout::record(5), layout::encode(&claim));
            assert!(matches!(bucket::create(&*bucket, &key, bytes).await, Created::Created));
            take_lease(&*bucket, &layout::lease(5), 11, 0).await.unwrap();
            let lost = "branch 5: what its recovery starts from keeps going".to_owned();
            assert_eq!(collect(&*bucket, 3, &NOW).await, Err(Error::Lost(lost)));
            assert_eq!(record(&*bucket, 5).await, Ok(Some(claim)));
        });
    }

    /// A branch deleted, before it had a record, has a deleted one, and takes no lease again.
    #[test]
    fn deleted_branches_take_no_lease() {
        let bucket = Direct(SimBucket::new());
        let lease = layout::lease(3);
        assert_eq!(block_on(take_lease(&bucket, &lease, 1, 0)).unwrap(), 1);
        block_on(delete(&bucket, 3, 2, 0)).unwrap();
        let record = block_on(record(&bucket, 3)).unwrap();
        let deleted = Record { forked: true, deleted: true, ..Record::default() };
        assert_eq!(record, Some(deleted));
        let taken = block_on(take_lease(&bucket, &lease, 3, 0));
        assert!(matches!(taken, Err(Error::Deleted)), "{taken:?}");
    }

    /// TASK-79.78: a fork deleted is deleted again, which does nothing, though a pass of the
    /// collector deleted what recovering it read, its parent's state where it was forked, which no
    /// live branch needs: so a deletion that stopped once the branch was deleted here, its host
    /// crashing before what else keeps the branch went, finishes, deleted again. Here the parent
    /// frees what the fork inherited, and checkpoints past it.
    #[test]
    fn forks_deleted_are_deleted_again_once_what_they_inherited_went() {
        use crate::batch::{Batch, Commit};
        use crate::writer::Writer;
        use storage_device::ExtentId;
        let memory = std::sync::Arc::new(object_store::memory::InMemory::new());
        let bucket = std::sync::Arc::new(crate::store::Store::new(memory, ""));
        block_on(async {
            create(&*bucket, 1).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let extent = ExtentId::new(1, parent.chain().next_sequence());
            let extents = vec![(extent, b"inherited".to_vec())];
            let forking = Batch::from(Commit { tag: 1, extents, ..Commit::default() });
            let at = parent.add_segment(forking.fork(2, &mut parent.chain()).frame()).unwrap();
            parent.upload(at).await.unwrap();
            parent.acknowledge().await.unwrap();
            let freeing = Batch::from(Commit { tag: 2, frees: vec![extent], ..Commit::default() });
            let seq = parent.add_segment(freeing.close(&mut parent.chain()).frame()).unwrap();
            parent.upload(seq).await.unwrap();
            parent.acknowledge().await.unwrap();
            parent.checkpoint().await.unwrap();
            delete(&*bucket, 2, 20, 0).await.unwrap();
            let now = crate::collector::Options { compact_below: 0.0, grace: 0 };
            crate::collector::collect(&*bucket, 1, &now).await.unwrap();
            // What recovering the fork reads went with the pass.
            let origin = Some(Origin { parent: 1, at });
            let lost = "branch 2: what its recovery starts from keeps going".to_owned();
            assert_eq!(recover(&*bucket, 2, origin).await.map(drop), Err(Error::Lost(lost)));

            assert_eq!(delete(&*bucket, 2, 21, 0).await, Ok(()));
            let deleted = Record { origin, forked: true, deleted: true, ..Record::default() };
            assert_eq!(record(&*bucket, 2).await, Ok(Some(deleted)));
        });
    }
}
