// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Branches' records and leases: creating a branch, forking one, taking one over, deleting one.

use crate::bucket::{self, Bucket, Created, Updated};
use crate::layout::{self, Lease, Origin, Record};
use crate::recovery::recover;
use crate::{Error, rules};

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
    let key = layout::lease(branch);
    let lease = layout::encode(&Lease { epoch: Lease::FINAL, holder, token: u64::MAX });
    loop {
        match bucket::get(bucket, &key).await {
            None => {
                if let Created::Created = bucket::create(bucket, &key, lease.clone()).await {
                    break;
                }
            }
            Some(object) => {
                let current = Lease::decode(&object.bytes)?;
                if current.epoch == Lease::FINAL {
                    break;
                }
                if rules().delete_tokens && current.token > token {
                    return Err(Error::Stale);
                }
                if let Updated::Updated =
                    bucket::update(bucket, &key, lease.clone(), &object.meta.version).await
                {
                    break;
                }
            }
        }
    }
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
/// An ID found free may be taken by another branch's record before the deletion takes the child's
/// lease, which no single request to object storage rules out: a fork looks for its child's record
/// before it is made (`ExtentStore::fork`), so that this only arises for an ID another branch
/// takes while the fork is being made or deleted.
pub async fn delete_fork(
    bucket: &impl Bucket,
    child: u64,
    origin: Option<Origin>,
    holder: u64,
    token: u64,
) -> Result<bool, Error> {
    match record(bucket, child).await? {
        Some(record) if record.deleted => return Ok(true),
        Some(record) if origin.is_none() || record.origin != origin => return Ok(false),
        _ => {}
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
