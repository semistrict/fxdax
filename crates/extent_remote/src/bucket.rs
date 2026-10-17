//! What branches need of an object store: objects under keys, read whole, listed by prefix, and
//! written with conditions (S3's `If-None-Match` and `If-Match`, GCS's generation preconditions).
//!
//! The helpers below retry requests the store failed, which may or may not have taken effect, so
//! that callers see only answers: a write that landed and then failed, retried into a failed
//! condition, is recognised by reading the object back.

use std::future::Future;

/// An object's version, which a conditional update names: S3 names it by its ETag, GCS by its
/// generation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Version {
    pub e_tag: Option<String>,
    pub version: Option<String>,
}

/// How a write treats an object already at its key.
#[derive(Clone, Debug)]
pub enum PutMode {
    /// Replaces it.
    Overwrite,
    /// Fails with [`Error::AlreadyExists`] if there is one.
    Create,
    /// Fails with [`Error::Precondition`] unless there is one, of this version.
    Update(Version),
}

/// What a store says of an object.
#[derive(Clone, Debug)]
pub struct Meta {
    pub key: String,
    pub version: Version,
    pub size: u64,
    /// When it was written, by the store's clock ([`Bucket::now`]).
    pub modified: u64,
}

/// An object, read whole.
#[derive(Clone, Debug)]
pub struct Object {
    pub bytes: Vec<u8>,
    pub meta: Meta,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("no such object")]
    NotFound,
    #[error("an object exists at the key")]
    AlreadyExists,
    #[error("the object is not of the version named")]
    Precondition,
    /// The request failed. It may or may not have taken effect.
    #[error("the store is unavailable")]
    Unavailable,
}

/// Which bytes of an object a ranged read reads: an object shorter than the range reads short.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ByteRange {
    Bounded(std::ops::Range<u64>),
    /// Its last so many bytes.
    Suffix(u64),
}

/// A bucket of objects, or a prefix of one.
pub trait Bucket: Send + Sync + 'static {
    fn get(&self, key: &str) -> impl Future<Output = Result<Object, Error>> + Send;

    fn get_range(
        &self,
        key: &str,
        range: ByteRange,
    ) -> impl Future<Output = Result<Vec<u8>, Error>> + Send;

    fn head(&self, key: &str) -> impl Future<Output = Result<Meta, Error>> + Send;

    /// The objects whose keys start with `prefix`, in the order of their keys.
    fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<Meta>, Error>> + Send;

    fn put(
        &self,
        key: &str,
        bytes: Vec<u8>,
        mode: PutMode,
    ) -> impl Future<Output = Result<Version, Error>> + Send;

    /// Deletes the object at `key`. Deleting none is no error.
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), Error>> + Send;

    /// The store's clock, which [`Meta::modified`] is in.
    fn now(&self) -> u64;
}

/// The object at `key`, if any.
pub(crate) async fn get(bucket: &impl Bucket, key: &str) -> Option<Object> {
    loop {
        match bucket.get(key).await {
            Ok(object) => return Some(object),
            Err(Error::NotFound) => return None,
            Err(_) => {}
        }
    }
}

/// The bytes of `range` of the object at `key`, if there is one.
pub(crate) async fn get_range(
    bucket: &impl Bucket,
    key: &str,
    range: ByteRange,
) -> Option<Vec<u8>> {
    loop {
        match bucket.get_range(key, range.clone()).await {
            Ok(bytes) => return Some(bytes),
            Err(Error::NotFound) => return None,
            Err(_) => {}
        }
    }
}

pub(crate) async fn list(bucket: &impl Bucket, prefix: &str) -> Vec<Meta> {
    loop {
        if let Ok(list) = bucket.list(prefix).await {
            return list;
        }
    }
}

pub(crate) async fn delete(bucket: &impl Bucket, key: &str) {
    while bucket.delete(key).await.is_err() {}
}

/// What [`create`] found.
pub(crate) enum Created {
    /// The object holds what was written: this call wrote it, or one before it wrote the same.
    Created,
    /// Another object was there.
    Exists(Object),
}

/// Writes `bytes` at `key` unless an object is there.
pub(crate) async fn create(bucket: &impl Bucket, key: &str, bytes: Vec<u8>) -> Created {
    loop {
        match bucket.put(key, bytes.clone(), PutMode::Create).await {
            Ok(_) => return Created::Created,
            Err(Error::AlreadyExists) => {
                // Perhaps this one's own write, which landed though its answer was lost.
                match get(bucket, key).await {
                    Some(object) if object.bytes == bytes => return Created::Created,
                    Some(object) => return Created::Exists(object),
                    // Deleted since: try again.
                    None => {}
                }
            }
            Err(_) => {}
        }
    }
}

/// What [`update`] found.
pub(crate) enum Updated {
    Updated,
    /// The object changed since `version`.
    Changed,
}

/// Writes `bytes` at `key` if the object there is of `version`. `bytes` must differ from every
/// other writer's, so that a write that landed though its answer was lost is recognised.
pub(crate) async fn update(
    bucket: &impl Bucket,
    key: &str,
    bytes: Vec<u8>,
    version: &Version,
) -> Updated {
    loop {
        match bucket.put(key, bytes.clone(), PutMode::Update(version.clone())).await {
            Ok(_) => return Updated::Updated,
            Err(Error::Precondition | Error::NotFound) => {
                return match get(bucket, key).await {
                    Some(object) if object.bytes == bytes => Updated::Updated,
                    _ => Updated::Changed,
                };
            }
            Err(_) => {}
        }
    }
}
