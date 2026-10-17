//! Branches' records and leases: creating a branch, forking one, taking one over, deleting one.

use crate::Error;
use crate::bucket::{self, Bucket, Created, Updated};
use crate::layout::{self, Lease, Origin, Record};
use crate::reader::recover;

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

/// Takes the lease at `key`, of the epoch after its last: `holder` must differ from every other
/// taking's. Fails for a deleted branch's.
pub async fn take_lease(bucket: &impl Bucket, key: &str, holder: u64) -> Result<u64, Error> {
    loop {
        match bucket::get(bucket, key).await {
            None => {
                let lease = Lease { epoch: 1, holder };
                if let Created::Created = bucket::create(bucket, key, layout::encode(&lease)).await
                {
                    return Ok(lease.epoch);
                }
            }
            Some(object) => {
                let lease: Lease = layout::decode(&object.bytes)?;
                if lease.epoch == Lease::FINAL {
                    return Err(Error::Deleted);
                }
                let lease = Lease { epoch: lease.epoch + 1, holder };
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
        Some(object) => Ok(layout::decode::<Lease>(&object.bytes)?.epoch),
        None => Ok(0),
    }
}

/// Deletes `branch`: no writer takes it again, the one it has stops at its next acknowledgement,
/// and the collector reclaims what no other branch inherited from it.
pub async fn delete(bucket: &impl Bucket, branch: u64, holder: u64) -> Result<(), Error> {
    let key = layout::lease(branch);
    let lease = layout::encode(&Lease { epoch: Lease::FINAL, holder });
    loop {
        match bucket::get(bucket, &key).await {
            None => {
                if let Created::Created = bucket::create(bucket, &key, lease.clone()).await {
                    break;
                }
            }
            Some(object) => {
                if layout::decode::<Lease>(&object.bytes)?.epoch == Lease::FINAL {
                    break;
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
