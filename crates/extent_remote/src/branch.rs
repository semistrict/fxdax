// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Branches' records and leases: creating a branch, forking one, taking one over, deleting one.

use crate::bucket::{self, Bucket, Created, Updated};
use crate::layout::{self, Kind, Lease, Origin, Record};
use crate::recovery::recover;
use crate::{Error, rules};
use std::collections::BTreeMap;

/// Creates `branch`, of no parent.
pub async fn create(bucket: &impl Bucket, branch: u64) -> Result<(), Error> {
    let record = Record { origin: None, forked: false, deleted: false };
    match bucket::create(bucket, &layout::record(branch), layout::encode(&record)).await {
        Created::Created => Ok(()),
        Created::Exists(_) => Err(Error::Corrupt(format!("branch {branch:x} exists"))),
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

/// Gives `child` its record, forked from `origin`, unless it has one. A fork's child gets it once
/// the fork is acknowledged, from the parent's writer, or the next one, or the parent's deletion.
pub async fn record_fork(bucket: &impl Bucket, child: u64, origin: Origin) -> Result<(), Error> {
    let record = Record { origin: Some(origin), forked: true, deleted: false };
    match bucket::create(bucket, &layout::record(child), layout::encode(&record)).await {
        Created::Created => Ok(()),
        Created::Exists(object) => {
            let existing: Record = layout::decode(&object.bytes)?;
            // Deleted before it got its record, perhaps.
            if existing.origin == Some(origin) || existing.deleted {
                Ok(())
            } else {
                Err(Error::Corrupt(format!("branch {child:x} has another origin")))
            }
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
    let record = loop {
        if let Some(record) = record(bucket, branch).await? {
            break record;
        }
        // A fork's child before its record, or no branch: either way none is, from now, unless
        // its record came meanwhile.
        let deleted = Record { origin: None, forked: true, deleted: true };
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
    // Its children get their records, so that the collector finds them without its state.
    let recovered = recover(bucket, branch, record.origin).await?;
    for fork in &recovered.state.forks {
        record_fork(bucket, fork.child, Origin { parent: branch, at: fork.at }).await?;
    }
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

/// Deletes `child`, the child of a fork, as [`delete`] does, but only if its branch is the one the
/// fork made: its record names `origin`, the fork's place in its parent's branch, or it has no
/// record yet, as a fork whose segment has not been acknowledged has not. A fork whose segment never
/// went to object storage names no `origin`: its child has no record but the one its deletion
/// gives it. A branch of another's, whose ID the fork's child was given, as a guest may give any,
/// is left as it is (TASK-79.82); so is a branch deleted already, whoever's. Returns whether the
/// fork's branch is gone, deleted now or before: `false` for another's.
///
/// A child with no record yet gets the one its fork would give it first ([`record_fork`]), naming
/// `origin`, before the deletion ends its lease: created only if no record is there, which another
/// branch taking the ID meanwhile has, whose lease the deletion took otherwise (TASK-79.82). Its
/// record taken, the ID is no other's, and a deletion that stops goes on as the fork's; deleted, it
/// had no record before, nor children with none, as [`delete`] deletes such a branch. A fork whose
/// segment never went, naming no `origin`, has none to take it with: an ID another branch takes
/// while it is being deleted is that one's still.
pub async fn delete_fork(
    bucket: &impl Bucket,
    child: u64,
    origin: Option<Origin>,
    holder: u64,
    token: u64,
) -> Result<bool, Error> {
    loop {
        match record(bucket, child).await? {
            Some(record) if record.deleted => return Ok(true),
            Some(record) if origin.is_none() || record.origin != origin => return Ok(false),
            Some(_) => break,
            None => {
                let Some(origin) = origin else { break };
                let record =
                    layout::encode(&Record { origin: Some(origin), forked: true, deleted: false });
                if let Created::Created =
                    bucket::create(bucket, &layout::record(child), record).await
                {
                    end_lease(bucket, child, holder, token).await?;
                    mark_deleted(bucket, child).await?;
                    return Ok(true);
                }
            }
        }
    }
    delete(bucket, child, holder, token).await?;
    Ok(true)
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
        block_on(record_fork(&*bucket, 6, Origin { at: origin.at + 1, ..origin })).unwrap();
        block_on(record_fork(&*bucket, 7, origin)).unwrap();
        for (child, origin) in [(5, Some(origin)), (6, Some(origin)), (7, None)] {
            assert_eq!(block_on(delete_fork(&*bucket, child, origin, 9, 0)), Ok(false), "{child}");
            assert_eq!(live(child), Some(true), "{child}");
            let lease = block_on(take_lease(&*bucket, &layout::lease(child), 10 + child, 0));
            assert_eq!(lease, Ok(1), "{child}");
        }
    }

    /// A fork's child gets one record: given again from the same origin, it stands; from another,
    /// the child is another branch's, which is corrupt.
    #[test]
    fn a_child_gets_one_record() {
        let bucket = Direct(SimBucket::new());
        let origin = Origin { parent: 1, at: 7 };
        block_on(record_fork(&bucket, 2, origin)).unwrap();
        block_on(record_fork(&bucket, 2, origin)).unwrap();
        let other = Origin { parent: 1, at: 8 };
        let refused = block_on(record_fork(&bucket, 2, other));
        assert!(matches!(refused, Err(Error::Corrupt(_))), "{refused:?}");
        let record = block_on(record(&bucket, 2)).unwrap();
        assert_eq!(record, Some(Record { origin: Some(origin), forked: true, deleted: false }));
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
        assert_eq!(record, Some(Record { origin: None, forked: false, deleted: false }));
        let lease = block_on(bucket::get(&racing.bucket, &layout::lease(3))).unwrap();
        let lease = Lease::decode(&lease.bytes).unwrap();
        assert_eq!((lease.epoch, lease.holder), (1, 7), "the other branch's lease");
    }

    /// A branch deleted, before it had a record, has a deleted one, and takes no lease again.
    #[test]
    fn deleted_branches_take_no_lease() {
        let bucket = Direct(SimBucket::new());
        let lease = layout::lease(3);
        assert_eq!(block_on(take_lease(&bucket, &lease, 1, 0)).unwrap(), 1);
        block_on(delete(&bucket, 3, 2, 0)).unwrap();
        let record = block_on(record(&bucket, 3)).unwrap();
        assert_eq!(record, Some(Record { origin: None, forked: true, deleted: true }));
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
            let deleted = Record { origin, forked: true, deleted: true };
            assert_eq!(record(&*bucket, 2).await, Ok(Some(deleted)));
        });
    }
}
