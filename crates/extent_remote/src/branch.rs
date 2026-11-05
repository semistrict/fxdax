// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Branches' records and leases: creating a branch, forking one, taking one over, deleting one.
//!
//! A fork of a branch with a writer is the writer's commit ([`record_fork`] gives its child its
//! record once the commit is acknowledged). A fork of a point is its child's record alone, which
//! writes nothing of the point's, so that any number of hosts fork one at once ([`fork_point`]).
//!
//! fxdax: every change of a record or a lease is a [`transition`], one compare-and-swap loop, of
//! what it found to what it writes (TASK-160.14).

use crate::bucket::{self, Bucket, Created, Updated};
use crate::layout::{self, Child, Kind as Key, Lease, Origin};
use crate::record::{Kind, Record};
use crate::recovery::{self, recover};
use crate::{Error, rules};
use serde::Serialize;
use std::collections::BTreeMap;

/// What a [`transition`] makes of the object it found.
enum Step<T, R> {
    /// Writes `T` in place of what it found, if that is still there, and gives `R`.
    Write(T, R),
    /// Writes nothing, and gives `R`.
    Stop(R),
}

/// What a [`transition`] starts from.
enum Start {
    /// What a read finds.
    Read,
    /// Nothing, as a create expects: one that finds something there reads it from its answer.
    Create,
}

/// Changes the object at `key`, which `decode` reads, as `step` says of what is there, if anything,
/// starting from `start`: by a create where nothing is, and a conditional update otherwise, given
/// to `step` again, from what is there then, whenever another's write came first, until it writes
/// nothing or what it writes lands. Every change of a branch's record or lease is one, its rules
/// in its `step`. A write of it that landed though its answer was lost is taken for its own
/// ([`bucket::create`], [`bucket::update`]), so that what `step` writes must differ from every
/// other writer's: a record names its owner, a lease its holder.
async fn transition<T: Serialize, R>(
    bucket: &impl Bucket,
    key: &str,
    start: Start,
    decode: impl Fn(&[u8]) -> Result<T, Error>,
    mut step: impl FnMut(Option<T>) -> Result<Step<T, R>, Error>,
) -> Result<R, Error> {
    let mut found = match start {
        Start::Read => bucket::get(bucket, key).await,
        Start::Create => None,
    };
    loop {
        let current = found.as_ref().map(|object| decode(&object.bytes)).transpose()?;
        let (next, result) = match step(current)? {
            Step::Stop(result) => return Ok(result),
            Step::Write(next, result) => (next, result),
        };
        let bytes = layout::encode(&next);
        found = match found {
            None => match bucket::create(bucket, key, bytes).await {
                Created::Created => return Ok(result),
                Created::Exists(object) => Some(object),
            },
            Some(object) => match bucket::update(bucket, key, bytes, &object.meta.version).await {
                Updated::Updated => return Ok(result),
                Updated::Changed => bucket::get(bucket, key).await,
            },
        };
    }
}

/// Changes `branch`'s record as `step` says ([`transition`]).
async fn change<R>(
    bucket: &impl Bucket,
    branch: u64,
    start: Start,
    step: impl FnMut(Option<Record>) -> Result<Step<Record, R>, Error>,
) -> Result<R, Error> {
    let decode = |bytes: &[u8]| Record::decode(branch, bytes);
    transition(bucket, &layout::record(branch), start, decode, step).await
}

/// The owner every record a writer makes names, its maker having chosen `owner`
/// ([`Record::owner`]): unless the rule that they do is off, in a test that shows it needed.
fn owner(owner: u64) -> u64 {
    if rules().records_owned { owner } else { 0 }
}

/// Creates `branch`, of no parent, of `owner`, which its maker chose at random: a create of
/// another's finds its record there.
pub async fn create(bucket: &impl Bucket, branch: u64, owner: u64) -> Result<(), Error> {
    let record = Kind::Root.of(self::owner(owner));
    change(bucket, branch, Start::Create, |found| match found {
        None => Ok(Step::Write(record, ())),
        Some(_) => Err(Error::Corrupt(format!("branch {branch:x} exists"))),
    })
    .await
}

/// Creates `child`, a fork's child taking a branch of its own, of no parent, its fork not made in
/// object storage, of the fork's `owner`. Its record says it is a fork's, of that owner, so that
/// it takes no record of another branch's that took its ID for its own, another fork's to that ID
/// among them, which a create of the same bytes would ([`Error::ChildTaken`], TASK-79.82).
pub async fn create_forked(bucket: &impl Bucket, child: u64, owner: u64) -> Result<(), Error> {
    let record = Kind::Own.of(self::owner(owner));
    change(bucket, child, Start::Create, |found| match found {
        None => Ok(Step::Write(record, ())),
        Some(_) => Err(Error::ChildTaken(child)),
    })
    .await
}

/// `branch`'s record, if it has one: [`Error::Format`] if it is of another format than this one,
/// whose objects this reads none of.
pub async fn record(bucket: &impl Bucket, branch: u64) -> Result<Option<Record>, Error> {
    match bucket::get(bucket, &layout::record(branch)).await {
        Some(object) => Ok(Some(Record::decode(branch, &object.bytes)?)),
        None => Ok(None),
    }
}

/// The branches in `bucket` whose records say they are live, with their records: an operator's
/// look, to compare with what hosts hold. A fork whose child has no record yet isn't among them.
pub async fn live(bucket: &impl Bucket) -> Result<BTreeMap<u64, Record>, Error> {
    let mut live = BTreeMap::new();
    for meta in bucket::list(bucket, layout::BRANCHES).await {
        if let Key::Record { branch } = layout::kind(&meta.key)
            && let Some(record) = record(bucket, branch).await?
            && !record.deleted
        {
            live.insert(branch, record);
        }
    }
    Ok(live)
}

/// Gives `child`, a fork's, its record, forked from `origin`, of its owner, a point if it is one,
/// unless it has one: the same, whoever gives it, as the fork's footer names it. A fork's child
/// gets it once the fork is acknowledged, from the parent's writer, or the next one, or the
/// parent's deletion. One of another owner is another branch's ([`Error::ChildTaken`]).
pub async fn record_fork(bucket: &impl Bucket, child: Child, origin: Origin) -> Result<(), Error> {
    failpoint!("branch::record_fork");
    let kind = if child.point { Kind::Point(origin) } else { Kind::Fork(origin) };
    let record = kind.of(owner(child.owner));
    let recorded = change(bucket, child.branch, Start::Create, |found| match found {
        None => Ok(Step::Write(record, true)),
        // Deleted before it got its record, perhaps.
        Some(found) if found.is_child_of(record.owner) || found.deleted => Ok(Step::Stop(false)),
        Some(_) => Err(Error::ChildTaken(child.branch)),
    })
    .await?;
    if recorded {
        failpoint!("branch::recorded_fork");
    }
    Ok(())
}

/// Forks `point`, whose record names `origin`, to `child`: `child`'s record, naming `origin` as
/// its own, and writes nothing of the point's or of the branch it names, so that any number of
/// hosts fork a point at once, and none's fork is any other's business (doc-7, Points). To recovery
/// and the collector, `child` is then a fork of that branch at that segment.
///
/// What keeps what `child` inherited until it has a record is the point's. So the record comes
/// first, saying the fork is being made ([`Kind::Forking`]), then a read of the point's record
/// ([`finish_point_fork`]): a pass of the collector that went on the point's being deleted read
/// that after the deletion, and so after this read if it found the point live, and so listed
/// `child`'s record after it was made, and keeps what it names. Otherwise the fork fails
/// ([`Error::Deleted`]), and its host deletes `child` ([`delete_fork`]). Forked again, as a store
/// left by a crash finishes it, it goes on from where it stopped. A child with a record of another
/// owner than the fork's, `owner`, is another branch's ([`Error::ChildTaken`]).
pub async fn fork_point(
    bucket: &impl Bucket,
    child: u64,
    point: u64,
    origin: Origin,
    owner: u64,
) -> Result<(), Error> {
    failpoint!("point::record");
    let kind = match rules().forks_pending {
        true => Kind::Forking { origin, point },
        false => Kind::Fork(origin),
    };
    let record = kind.of(self::owner(owner));
    // Whether the fork is still to be made.
    let making = change(bucket, child, Start::Create, |found| match found {
        None => Ok(Step::Write(record, true)),
        Some(found) if found.deleted => Err(Error::Deleted),
        Some(found) if !found.is_child_of(record.owner) => Err(Error::ChildTaken(child)),
        Some(found) => Ok(Step::Stop(found.forking().is_some())),
    })
    .await?;
    if !making {
        return Ok(());
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
    // Conditional, against its being deleted, or given up on ([`abandon`]). A record's bytes
    // repeat only for the same change, which is then made.
    let made = change(bucket, child, Start::Read, |found| match found {
        None => Err(Error::NoBranch),
        Some(found) if found.deleted => Err(Error::Deleted),
        Some(found @ Record { kind: Kind::Forking { origin, .. }, .. }) => {
            Ok(Step::Write(Record { kind: Kind::Fork(origin), ..found }, true))
        }
        Some(_) => Ok(Step::Stop(false)),
    })
    .await?;
    if made {
        failpoint!("point::made");
    }
    Ok(())
}

/// Whether no writer has `branch`: it has no lease, or its deletion's.
async fn unwritten(bucket: &impl Bucket, branch: u64) -> Result<bool, Error> {
    let epoch = lease_epoch(bucket, &layout::lease(branch)).await?;
    Ok(epoch == 0 || epoch == Lease::FINAL)
}

/// Whether recovering `branch`, whose record is `record`, failing, what it names gone
/// ([`Error::Lost`]), says nothing was lost: it is a failed fork's claim ([`Kind::Claim`]), which
/// no writer has. What it names may have gone before it landed; nothing ever read it.
pub(crate) async fn lost_is_ok(
    bucket: &impl Bucket,
    branch: u64,
    record: &Record,
) -> Result<bool, Error> {
    Ok(matches!(record.kind, Kind::Claim(_)) && unwritten(bucket, branch).await?)
}

/// Deletes `branch`, whose recovery failed though nothing was lost ([`lost_is_ok`]): unless it
/// was given another record since. Nothing ever read it: no writer recovers what a record not
/// there names.
pub(crate) async fn abandon_claim(bucket: &impl Bucket, branch: u64) -> Result<(), Error> {
    mark_deleted(bucket, branch).await
}

/// Gives up on `child`, a fork being made of a point that was deleted ([`Kind::Forking`]), so
/// that nothing keeps what it names, unless the fork was made first: returns its record as it then
/// stands. Its fork, which fails then, wrote nothing: a branch that holds nothing and that nothing
/// relies on, what it names perhaps gone, which recovering it would stop the collector on.
pub(crate) async fn abandon(bucket: &impl Bucket, child: u64) -> Result<Record, Error> {
    change(bucket, child, Start::Read, |found| match found {
        None => Err(Error::NoBranch),
        Some(found) if found.deleted || found.forking().is_none() => Ok(Step::Stop(found)),
        Some(found) => {
            let abandoned = Record { deleted: true, ..found };
            Ok(Step::Write(abandoned, abandoned))
        }
    })
    .await
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
    transition(bucket, key, Start::Read, Lease::decode, |found| {
        let epoch = match found {
            None => 1,
            Some(lease) if lease.epoch == Lease::FINAL => return Err(Error::Deleted),
            Some(lease) if rules().tokens && lease.token > token => return Err(Error::Stale),
            Some(lease) => lease.epoch + 1,
        };
        Ok(Step::Write(Lease { epoch, holder, token }, epoch))
    })
    .await
}

/// The epoch of the lease at `key`: 0 if there is none.
pub async fn lease_epoch(bucket: &impl Bucket, key: &str) -> Result<u64, Error> {
    Ok(lease(bucket, key).await?.map_or(0, |lease| lease.epoch))
}

/// The lease at `key`, if there is one.
pub async fn lease(bucket: &impl Bucket, key: &str) -> Result<Option<Lease>, Error> {
    match bucket::get(bucket, key).await {
        Some(object) => Ok(Some(Lease::decode(&object.bytes)?)),
        None => Ok(None),
    }
}

/// Whether the lease at `key` is still the taking of epoch `epoch` by `holder`: its epoch alone
/// may be another taking's, as a deleted branch's lease the collector removed and a taking created
/// again, at epoch 1, before finding the branch deleted (TASK-179).
pub async fn holds_lease(
    bucket: &impl Bucket,
    key: &str,
    epoch: u64,
    holder: u64,
) -> Result<bool, Error> {
    let lease = lease(bucket, key).await?;
    Ok(lease.is_some_and(|lease| {
        lease.epoch == epoch && (lease.holder == holder || !rules().leases_by_takings)
    }))
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
    // A fork's child before its record, or no branch: either way none is, from now, unless its
    // record came meanwhile. Its owner the deletion's, which no other's is.
    let deleted = Record { deleted: true, ..Kind::Own.of(owner(holder)) };
    let found = change(bucket, branch, Start::Read, |found| match found {
        None => Ok(Step::Write(deleted, None)),
        Some(found) => Ok(Step::Stop(Some(found))),
    })
    .await?;
    // Deleted already: its children got their records before, and what recovering it read, no
    // live branch needs, the collector may have deleted since (TASK-79.78).
    let Some(record) = found.filter(|record| !record.deleted) else { return Ok(()) };
    // Its children get their records, so that the collector finds them without its state. A fork
    // of a point still being made has none, and may name what is gone; so may a claim, with
    // neither a writer nor a child, if it does.
    if record.forking().is_none() {
        let forks = match recover(bucket, branch, record.origin()).await {
            Ok(recovered) => recovered.state.forks,
            // Deleted meanwhile, by another deletion, which gave its children their records: what
            // recovering it needs, the collector may have reclaimed since (TASK-178).
            Err(Error::Lost(_))
                if self::record(bucket, branch).await?.is_none_or(|r| r.deleted) =>
            {
                return Ok(());
            }
            Err(Error::Lost(_)) if lost_is_ok(bucket, branch, &record).await? => Vec::new(),
            Err(error) => return Err(error),
        };
        for fork in &forks {
            let origin = Origin { parent: branch, at: fork.at };
            let child = Child { branch: fork.child, owner: fork.owner, point: fork.point };
            match record_fork(bucket, child, origin).await {
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
    let lease = Lease { epoch: Lease::FINAL, holder, token: u64::MAX };
    transition(bucket, &layout::lease(branch), Start::Read, Lease::decode, |found| match found {
        Some(found) if found.epoch == Lease::FINAL => Ok(Step::Stop(())),
        Some(found) if rules().delete_tokens && found.token > token => Err(Error::Stale),
        _ => Ok(Step::Write(lease, ())),
    })
    .await
}

/// Marks `branch`'s record deleted, as [`delete`] does last. A record's bytes repeat only for the
/// same change, which is then made.
async fn mark_deleted(bucket: &impl Bucket, branch: u64) -> Result<(), Error> {
    change(bucket, branch, Start::Read, |found| match found {
        None => Err(Error::NoBranch),
        Some(found) if found.deleted => Ok(Step::Stop(())),
        Some(found) => Ok(Step::Write(Record { deleted: true, ..found }, ())),
    })
    .await
}

/// Deletes `child`, the child of a fork, as [`delete`] does, but only if its branch is the one the
/// fork made: its record names the fork's `owner`, whoever wrote it, the fork's host or its
/// parent's writers, or it has no record yet, as a fork whose segment has not been acknowledged
/// has not. `forked` is the kind of record the fork gives its child, as its host knows: a branch of
/// its own ([`Kind::Own`]), its fork's segment never having gone to object storage; a fork at a
/// segment, which may have landed, and whose branch's writers then give the child a record; a
/// point, so, whatever record it gets saying so, whoever gives it; or a fork of a point, which
/// nothing but the fork itself gives its child a record. A branch of another's, whose ID the
/// fork's child was given, as a guest may give any, another fork's to the same ID among them, is
/// left as it is (TASK-79.82); so is a branch deleted already, whoever's. Returns whether the
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
/// and every pass of the collector. It is a claim ([`Kind::Claim`]): that the parent's branch
/// holds a segment of that number is no proof it is the fork's, and nothing keeps what it names
/// until the record lands, a dead host's perhaps, long after. Otherwise it is deleted from the
/// first, for good: no other placement took a branch nothing of which landed; and a segment
/// landing after finds it so. So is a fork of a point with no record, which holds nothing, a
/// create of its record, a dead host's landing late, finding it so too; and a point with none: no
/// placement refuses a deletion of what has no lease, and a stale copy finds no live record naming
/// a segment that may not be its fork's.
pub async fn delete_fork(
    bucket: &impl Bucket,
    child: u64,
    forked: Kind,
    owner: u64,
    holder: u64,
    token: u64,
) -> Result<bool, Error> {
    let owner = self::owner(owner);
    // What deleting the child, whose record is `record`, gives.
    let delete_found = async |record: Record| match record {
        Record { deleted: true, .. } => Ok(true),
        record if !record.is_child_of(owner) => Ok(false),
        _ => delete(bucket, child, holder, token).await.map(|()| true),
    };
    if let Some(record) = record(bucket, child).await? {
        return delete_found(record).await;
    }
    if forked.origin().is_none() && !rules().fork_claims_ids {
        return delete(bucket, child, holder, token).await.map(|()| true);
    }
    // The record it gets, and whether live, a claim, as its parent's branch holds its segment.
    let (kind, live) = match forked {
        Kind::Fork(origin) | Kind::Claim(origin) if !rules().fork_claims_landed => {
            (Kind::Fork(origin), true)
        }
        Kind::Fork(origin) | Kind::Claim(origin) => {
            match recovery::holds(bucket, origin.parent, origin.at).await? {
                true => (Kind::Claim(origin), true),
                false => (Kind::Fork(origin), false),
            }
        }
        Kind::Forking { origin, .. } => (Kind::Fork(origin), false),
        Kind::Point(origin) => (Kind::Point(origin), false),
        Kind::Root | Kind::Own => (Kind::Own, false),
    };
    let record = Record { deleted: !live, ..kind.of(owner) };
    let taken = change(bucket, child, Start::Create, |taken| match taken {
        None => Ok(Step::Write(record, None)),
        Some(taken) => Ok(Step::Stop(Some(taken))),
    })
    .await?;
    if let Some(taken) = taken {
        return delete_found(taken).await;
    }
    failpoint!("branch::claimed");
    end_lease(bucket, child, holder, token).await?;
    failpoint!("branch::claim_lease_ended");
    if live {
        mark_deleted(bucket, child).await?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::child;
    use crate::sim::bucket::{Direct, SimBucket};
    use futures::executor::block_on;

    /// TASK-79.82: a fork's child is deleted only if its branch is the fork's: one with no record,
    /// or whose record names the fork's owner. One whose record names another owner, a branch of no
    /// parent's, or another fork's, at another point of the fork's parent or whose segment never
    /// went, is left as it is. One deleted is gone already.
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
        let owned = |branch, owner| Child { branch, owner, point: false };
        // A fork of branch 1's to 3, made, its child given its record.
        let origin = block_on(async {
            create(&*bucket, 1, 0x11).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let forking = Batch::from(Commit { tag: 1, ..Commit::default() });
            let objects = forking.fork(owned(3, 0x33), &mut parent.chain()).frame();
            let at = parent.add_segment(objects).unwrap();
            parent.upload(at).await.unwrap();
            parent.acknowledge().await.unwrap();
            Origin { parent: 1, at }
        });
        assert_eq!(live(3), Some(true));
        assert_eq!(block_on(delete_fork(&*bucket, 3, Kind::Fork(origin), 0x33, 9, 0)), Ok(true));
        assert_eq!(live(3), Some(false));
        assert_eq!(block_on(delete_fork(&*bucket, 3, Kind::Own, 0x33, 9, 0)), Ok(true));
        // Forks whose children have no record yet, whose segments went or not.
        assert_eq!(block_on(delete_fork(&*bucket, 2, Kind::Fork(origin), 0x22, 9, 0)), Ok(true));
        assert_eq!(block_on(delete_fork(&*bucket, 4, Kind::Own, 0x44, 9, 0)), Ok(true));
        assert_eq!((live(2), live(4)), (Some(false), Some(false)));

        block_on(create(&*bucket, 5, 0x55)).unwrap();
        let other = Origin { at: origin.at + 1, ..origin };
        block_on(record_fork(&*bucket, owned(6, 0x66), other)).unwrap();
        block_on(record_fork(&*bucket, owned(7, 0x77), origin)).unwrap();
        for (child, origin, owner) in
            [(5, Kind::Fork(origin), 0x50), (6, Kind::Fork(origin), 0x60), (7, Kind::Own, 0x70)]
        {
            let deleted = block_on(delete_fork(&*bucket, child, origin, owner, 9, 0));
            assert_eq!(deleted, Ok(false), "{child}");
            assert_eq!(live(child), Some(true), "{child}");
            let lease = block_on(take_lease(&*bucket, &layout::lease(child), 10 + child, 0));
            assert_eq!(lease, Ok(1), "{child}");
        }
    }

    /// A fork's child gets one record: given again by whoever gives it, of the same owner, it
    /// stands; of another, the child is another branch's.
    #[test]
    fn a_child_gets_one_record() {
        let bucket = Direct(SimBucket::new());
        let origin = Origin { parent: 1, at: 7 };
        let forked = Child { branch: 2, owner: 0x22, point: false };
        block_on(record_fork(&bucket, forked, origin)).unwrap();
        block_on(record_fork(&bucket, forked, origin)).unwrap();
        let other = Child { owner: 0x23, ..forked };
        assert_eq!(block_on(record_fork(&bucket, other, origin)), Err(Error::ChildTaken(2)));
        let record = block_on(record(&bucket, 2)).unwrap();
        assert_eq!(record, Some(Kind::Fork(origin).of(0x22)));
    }

    /// TASK-79.82's last case: two forks name one child within a round trip, neither's segment
    /// landing, and each takes a record of its own for it. One takes it; the other finds another's,
    /// though the bytes of what each wrote differ only by their owners, and deleting its fork
    /// leaves the first's be.
    #[test]
    fn forks_to_one_child_take_one_record() {
        let bucket = Direct(SimBucket::new());
        assert_eq!(block_on(create_forked(&bucket, 2, 0x21)), Ok(()));
        assert_eq!(block_on(create_forked(&bucket, 2, 0x21)), Ok(()), "its own, again");
        assert_eq!(block_on(create_forked(&bucket, 2, 0x22)), Err(Error::ChildTaken(2)));
        assert_eq!(block_on(delete_fork(&bucket, 2, Kind::Own, 0x22, 9, 0)), Ok(false));
        let taken = Kind::Own.of(0x21);
        assert_eq!(block_on(record(&bucket, 2)), Ok(Some(taken)));
        assert_eq!(block_on(delete_fork(&bucket, 2, Kind::Own, 0x21, 9, 0)), Ok(true));
        assert_eq!(block_on(record(&bucket, 2)), Ok(Some(Record { deleted: true, ..taken })));
    }

    /// TASK-160.14: a fork of a branch at a segment and a fork of a point made at that segment,
    /// which names it as its own origin, given one child ID, as guests may give any: whichever
    /// record lands first, the other fork finds it another's, though the two name one origin and
    /// one ID, and deleting that fork leaves it be. So they were, until records named their owners
    /// (TASK-153.9): with the rule that they do off, each takes the other's for its own.
    #[test]
    fn forks_and_forks_of_points_at_one_segment_take_no_records_of_each_other() {
        use crate::{Rules, set_rules};
        let origin = Origin { parent: 1, at: 7 };
        let (point, direct, of_point) = (3, Child { branch: 2, owner: 0x21, point: false }, 0x22);
        // What each fork gave, the direct first or not, and whose the child's record is.
        let race = |direct_first: bool| {
            let bucket = Direct(SimBucket::new());
            let made = Child { branch: point, owner: 0x33, point: true };
            block_on(record_fork(&bucket, made, origin)).unwrap();
            let fork = || block_on(record_fork(&bucket, direct, origin));
            let restore = || block_on(fork_point(&bucket, 2, point, origin, of_point));
            let (forked, restored, lost, at) = if direct_first {
                let forked = fork();
                (forked, restore(), of_point, Kind::Forking { origin, point })
            } else {
                let restored = restore();
                (fork(), restored, direct.owner, Kind::Fork(origin))
            };
            // The fork that lost is deleted, as its host's cleanup deletes it.
            let deleted = block_on(delete_fork(&bucket, 2, at, lost, 9, 0));
            let owner = block_on(record(&bucket, 2)).unwrap().map(|record| record.owner);
            (forked, restored, deleted, owner)
        };
        let taken = Err(Error::ChildTaken(2));
        assert_eq!(race(true), (Ok(()), taken.clone(), Ok(false), Some(direct.owner)));
        assert_eq!(race(false), (taken, Ok(()), Ok(false), Some(of_point)));

        set_rules(Rules { records_owned: false, ..Rules::ALL });
        // The loser's deletion then deletes the winner's branch, of a parent not there.
        let unowned = |direct_first| {
            let (forked, restored, _, owner) = race(direct_first);
            (forked, restored, owner)
        };
        let unowned = (unowned(true), unowned(false));
        set_rules(Rules::ALL);
        let both = (Ok(()), Ok(()), Some(0));
        assert_eq!(unowned, (both.clone(), both), "each takes the other's record");
    }

    /// TASK-160.14: whatever made a branch's record, whatever else makes one for its ID, of
    /// another owner, as guests may give any ID, finds it another's, and changes nothing of it:
    /// a create, a fork's child taking a branch of its own, a fork's writers recording its child,
    /// a point or not, a fork of a point, and deleting a fork, wherever it was made.
    #[test]
    fn makers_of_one_id_take_nothing_of_each_others() {
        let (origin, point, child) = (Origin { parent: 1, at: 7 }, 3, 2);
        let said = |result: &dyn std::fmt::Debug| format!("{result:?}");
        let fork = |owner, point| Child { branch: child, owner, point };
        let unfork =
            |bucket: &Direct, at, owner| block_on(delete_fork(bucket, child, at, owner, 9, 0));
        // Each maker, of the owner it is given, and what it gives, in words, finding another's
        // record, live. The first five make live records; deleting a fork leaves none.
        type Maker<'a> = (&'a str, &'a dyn Fn(&Direct, u64) -> String, &'a str);
        let makers: [Maker; 9] = [
            (
                "create",
                &|bucket, owner| said(&block_on(create(bucket, child, owner))),
                "Err(Corrupt(\"branch 2 exists\"))",
            ),
            (
                "create_forked",
                &|bucket, owner| said(&block_on(create_forked(bucket, child, owner))),
                "Err(ChildTaken(2))",
            ),
            (
                "record_fork",
                &|bucket, owner| said(&block_on(record_fork(bucket, fork(owner, false), origin))),
                "Err(ChildTaken(2))",
            ),
            (
                "record_fork of a point",
                &|bucket, owner| said(&block_on(record_fork(bucket, fork(owner, true), origin))),
                "Err(ChildTaken(2))",
            ),
            (
                "fork_point",
                &|bucket, owner| said(&block_on(fork_point(bucket, child, point, origin, owner))),
                "Err(ChildTaken(2))",
            ),
            (
                "delete_fork, nowhere",
                &|bucket, owner| said(&unfork(bucket, Kind::Own, owner)),
                "Ok(false)",
            ),
            (
                "delete_fork, at a segment",
                &|bucket, owner| said(&unfork(bucket, Kind::Fork(origin), owner)),
                "Ok(false)",
            ),
            (
                "delete_fork, to a point",
                &|bucket, owner| said(&unfork(bucket, Kind::Point(origin), owner)),
                "Ok(false)",
            ),
            (
                "delete_fork, of a point",
                &|bucket, owner| said(&unfork(bucket, Kind::Forking { origin, point }, owner)),
                "Ok(false)",
            ),
        ];
        for (first, make, _) in &makers[..5] {
            for (second, again, another) in &makers {
                let bucket = Direct(SimBucket::new());
                block_on(record_fork(
                    &bucket,
                    Child { branch: point, owner: 0x33, point: true },
                    origin,
                ))
                .unwrap();
                assert_eq!(make(&bucket, 0x11), "Ok(())", "{first}");
                let made = |bucket: &Direct| {
                    let record = block_on(record(bucket, child)).unwrap();
                    (record, block_on(lease_epoch(bucket, &layout::lease(child))))
                };
                let before = made(&bucket);
                assert_eq!(again(&bucket, 0x12), *another, "{second} after {first}");
                assert_eq!(made(&bucket), before, "{second} after {first}");
            }
        }
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
                        create(&self.bucket, self.child, 0).await.unwrap();
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
        assert_eq!(block_on(delete_fork(&racing, 3, Kind::Fork(origin), 0, 9, 0)), Ok(false));
        let record = block_on(record(&racing.bucket, 3)).unwrap();
        assert_eq!(record, Some(Kind::Root.of(0)));
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
            create(&*bucket, 1, 0).await.unwrap();
            create(&*bucket, 2, 0).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let forking = Batch::from(Commit { tag: 1, ..Commit::default() });
            let at =
                parent.add_segment(forking.fork(child(2), &mut parent.chain()).frame()).unwrap();
            parent.upload(at).await.unwrap();
            assert_eq!(parent.acknowledge().await, Ok(Some(at)));
            assert!(parent.refused(2));
            assert_eq!(delete(&*bucket, 1, 20, 0).await, Ok(()));
            assert_eq!(record(&*bucket, 2).await, Ok(Some(Kind::Root.of(0))));
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
            create(&*bucket, 1, 0).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            for tag in 1..=3 {
                let commit = Batch::from(Commit { tag, ..Commit::default() });
                let seq = parent.add_segment(commit.close(&mut parent.chain()).frame()).unwrap();
                parent.upload(seq).await.unwrap();
                parent.acknowledge().await.unwrap();
            }
            parent.checkpoint().await.unwrap();
            collect(&*bucket, 1, &NOW).await.unwrap();
            let origin = Origin { parent: 1, at: 1 };
            let lost = "branch 3: what its recovery starts from keeps going".to_owned();
            assert_eq!(recover(&*bucket, 3, Some(origin)).await.map(drop), Err(Error::Lost(lost)));

            let claim = Kind::Claim(origin).of(0);
            let (key, bytes) = (layout::record(3), layout::encode(&claim));
            assert!(matches!(bucket::create(&*bucket, &key, bytes).await, Created::Created));
            collect(&*bucket, 2, &NOW).await.unwrap();
            let deleted = Record { deleted: true, ..claim };
            assert_eq!(record(&*bucket, 3).await, Ok(Some(deleted)));

            // Its deletion, which stopped, finds it so; or, the pass not there first, finishes it.
            let claim = Kind::Claim(origin).of(0);
            let (key, bytes) = (layout::record(4), layout::encode(&claim));
            assert!(matches!(bucket::create(&*bucket, &key, bytes).await, Created::Created));
            assert_eq!(delete_fork(&*bucket, 4, Kind::Fork(origin), 0, 9, 0).await, Ok(true));
            assert_eq!(record(&*bucket, 4).await, Ok(Some(Record { deleted: true, ..claim })));

            // But one a writer has lost something: it stops the pass, as any branch's.
            let claim = Kind::Claim(origin).of(0);
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
        // Of the deletion's owner, its holder's, which no other's is.
        let deleted = Record { deleted: true, ..Kind::Own.of(2) };
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
            create(&*bucket, 1, 0).await.unwrap();
            let parent = Writer::open(bucket.clone(), 1, 10, 0).await.unwrap();
            let extent = ExtentId::new(1, parent.chain().next_sequence());
            let extents = vec![(extent, b"inherited".to_vec())];
            let forking = Batch::from(Commit { tag: 1, extents, ..Commit::default() });
            let at =
                parent.add_segment(forking.fork(child(2), &mut parent.chain()).frame()).unwrap();
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
            let origin = Origin { parent: 1, at };
            let lost = "branch 2: what its recovery starts from keeps going".to_owned();
            assert_eq!(recover(&*bucket, 2, Some(origin)).await.map(drop), Err(Error::Lost(lost)));

            assert_eq!(delete(&*bucket, 2, 21, 0).await, Ok(()));
            let deleted = Record { deleted: true, ..Kind::Fork(origin).of(0) };
            assert_eq!(record(&*bucket, 2).await, Ok(Some(deleted)));
        });
    }
}
