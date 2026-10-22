// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

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

impl ByteRange {
    /// Whether it begins within an object of `size` bytes: a read of it reads some of the object.
    /// Stores refuse one that does not.
    pub(crate) fn begins_within(&self, size: u64) -> bool {
        match self {
            Self::Bounded(range) => range.start < size,
            Self::Suffix(len) => *len > 0 && size > 0,
        }
    }
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

    /// Waits before a request that failed `failures` times in a row goes again.
    fn pause(&self, failures: u32) -> impl Future<Output = ()> + Send {
        let _ = failures;
        std::future::ready(())
    }
}

/// The object at `key`, if any.
pub(crate) async fn get(bucket: &impl Bucket, key: &str) -> Option<Object> {
    let mut failures = 0;
    loop {
        match bucket.get(key).await {
            Ok(object) => return Some(object),
            Err(Error::NotFound) => return None,
            Err(_) => failed(bucket, &mut failures).await,
        }
    }
}

/// Counts a failure, and waits before the request goes again.
async fn failed(bucket: &impl Bucket, failures: &mut u32) {
    *failures += 1;
    bucket.pause(*failures).await;
}

/// A ranged read of an object that holds none of its range, which begins past the object's end
/// ([`get_range`]): an object not as its reader took it to be.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{key}: {range:?} past its end, at {size} bytes")]
pub(crate) struct Past {
    key: String,
    range: ByteRange,
    size: u64,
}

impl From<Past> for crate::Error {
    fn from(past: Past) -> Self {
        Self::Corrupt(past.to_string())
    }
}

/// The bytes of `range` of the object at `key`, if there is one; of a range of no bytes, none,
/// asking nothing. `object_store` refuses a range that begins past the object's end as it fails
/// when the store is out of reach (`Generic`, [`Error::Unavailable`] here): a read that fails goes
/// again only once the object's size shows the range begins within it, and is [`Past`] its end
/// otherwise.
pub(crate) async fn get_range(
    bucket: &impl Bucket,
    key: &str,
    range: ByteRange,
) -> Result<Option<Vec<u8>>, Past> {
    if let ByteRange::Bounded(bounded) = &range {
        assert!(bounded.start <= bounded.end, "{key}: {bounded:?}, reversed");
        if bounded.is_empty() {
            return Ok(Some(Vec::new()));
        }
    }
    let mut failures = 0;
    loop {
        match bucket.get_range(key, range.clone()).await {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(Error::NotFound) => return Ok(None),
            Err(_) => match bucket.head(key).await {
                Err(Error::NotFound) => return Ok(None),
                Ok(meta) if !range.begins_within(meta.size) => {
                    return Err(Past { key: key.to_owned(), range, size: meta.size });
                }
                _ => failed(bucket, &mut failures).await,
            },
        }
    }
}

pub(crate) async fn list(bucket: &impl Bucket, prefix: &str) -> Vec<Meta> {
    let mut failures = 0;
    loop {
        match bucket.list(prefix).await {
            Ok(list) => return list,
            Err(_) => failed(bucket, &mut failures).await,
        }
    }
}

pub(crate) async fn delete(bucket: &impl Bucket, key: &str) {
    let mut failures = 0;
    while bucket.delete(key).await.is_err() {
        failed(bucket, &mut failures).await;
    }
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
    let mut failures = 0;
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
            Err(_) => failed(bucket, &mut failures).await,
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
    let mut failures = 0;
    loop {
        match bucket.put(key, bytes.clone(), PutMode::Update(version.clone())).await {
            Ok(_) => return Updated::Updated,
            Err(Error::Precondition | Error::NotFound) => {
                return match get(bucket, key).await {
                    Some(object) if object.bytes == bytes => Updated::Updated,
                    _ => Updated::Changed,
                };
            }
            Err(_) => failed(bucket, &mut failures).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::sync::Mutex;

    /// A bucket whose GETs fail so many times, then find nothing, and that records its pauses.
    struct Flaky {
        failing: Mutex<u32>,
        pauses: Mutex<Vec<u32>>,
    }

    impl Bucket for Flaky {
        fn get(&self, _: &str) -> impl Future<Output = Result<Object, Error>> + Send {
            let mut failing = self.failing.lock().unwrap();
            let result = match *failing {
                0 => Err(Error::NotFound),
                _ => Err(Error::Unavailable),
            };
            *failing = failing.saturating_sub(1);
            std::future::ready(result)
        }

        fn get_range(
            &self,
            _: &str,
            _: ByteRange,
        ) -> impl Future<Output = Result<Vec<u8>, Error>> + Send {
            std::future::ready(Err(Error::Unavailable))
        }

        fn head(&self, _: &str) -> impl Future<Output = Result<Meta, Error>> + Send {
            std::future::ready(Err(Error::Unavailable))
        }

        fn list(&self, _: &str) -> impl Future<Output = Result<Vec<Meta>, Error>> + Send {
            std::future::ready(Err(Error::Unavailable))
        }

        fn put(
            &self,
            _: &str,
            _: Vec<u8>,
            _: PutMode,
        ) -> impl Future<Output = Result<Version, Error>> + Send {
            std::future::ready(Err(Error::Unavailable))
        }

        fn delete(&self, _: &str) -> impl Future<Output = Result<(), Error>> + Send {
            std::future::ready(Err(Error::Unavailable))
        }

        fn now(&self) -> u64 {
            0
        }

        fn pause(&self, failures: u32) -> impl Future<Output = ()> + Send {
            self.pauses.lock().unwrap().push(failures);
            std::future::ready(())
        }
    }

    /// A request the store failed goes again, after a pause that counts the failures in a row.
    #[test]
    fn failed_requests_pause_counting_their_failures() {
        let bucket = Flaky { failing: Mutex::new(3), pauses: Mutex::default() };
        assert!(block_on(get(&bucket, "key")).is_none());
        assert_eq!(*bucket.pauses.lock().unwrap(), [1, 2, 3]);
    }

    /// Objects in memory, of `object_store`'s, whose requests the test fails where they would go
    /// again: what `object_store` refuses but for the protocol's conditions, it says as it says the
    /// store is out of reach, which is retried.
    struct Unretried(crate::store::Store);

    impl Bucket for Unretried {
        fn get(&self, key: &str) -> impl Future<Output = Result<Object, Error>> + Send {
            self.0.get(key)
        }

        fn get_range(
            &self,
            key: &str,
            range: ByteRange,
        ) -> impl Future<Output = Result<Vec<u8>, Error>> + Send {
            self.0.get_range(key, range)
        }

        fn head(&self, key: &str) -> impl Future<Output = Result<Meta, Error>> + Send {
            self.0.head(key)
        }

        fn list(&self, prefix: &str) -> impl Future<Output = Result<Vec<Meta>, Error>> + Send {
            self.0.list(prefix)
        }

        fn put(
            &self,
            key: &str,
            bytes: Vec<u8>,
            mode: PutMode,
        ) -> impl Future<Output = Result<Version, Error>> + Send {
            self.0.put(key, bytes, mode)
        }

        fn delete(&self, key: &str) -> impl Future<Output = Result<(), Error>> + Send {
            self.0.delete(key)
        }

        fn now(&self) -> u64 {
            self.0.now()
        }

        fn pause(&self, failures: u32) -> impl Future<Output = ()> + Send {
            async move { panic!("a request goes again, failed {failures} times") }
        }
    }

    /// TASK-79.40: a ranged read starting past the end of an object, which `object_store` refuses
    /// as it says the store is out of reach, goes not again for ever: the object's size says why,
    /// and the object is corrupt. So an object's tail whose trailer says it is longer than it is.
    #[test]
    fn ranges_past_an_objects_end_go_not_again() {
        use crate::layout;
        use extent_frame::Frame;
        let memory = std::sync::Arc::new(object_store::memory::InMemory::new());
        let bucket = Unretried(crate::store::Store::new(memory, ""));
        // A tail longer than a first read of it, its object cut short before it: its data, which
        // does not compress, from a xorshift generator.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let data: Vec<u8> = (0..3 * crate::object::TAIL)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        let long = layout::with_footer(&data, &vec![2u8; 5000]);
        let tail = Frame::of(&long).unwrap().tail();
        let short = long[(tail.end - tail.start) as usize..].to_vec();
        let size = short.len() as u64;
        assert!(size >= crate::object::TAIL);
        block_on(bucket.put("key", short.clone(), PutMode::Create)).unwrap();
        let read = block_on(crate::object::parts_and_footer(&bucket, "key"));
        let past = format!("key: {:?} past its end, at {size} bytes", ByteRange::Bounded(tail));
        assert_eq!(read, Err(crate::Error::Corrupt(past)));

        // One that ends past it reads short; one of no bytes, which `object_store` refuses too,
        // reads none, asking nothing; one of an object gone, nothing.
        let read = |key: &str, range| block_on(get_range(&bucket, key, ByteRange::Bounded(range)));
        let tail = short[(size - 10) as usize..].to_vec();
        assert_eq!(read("key", size - 10..size + 10), Ok(Some(tail)));
        let past = Past { key: "key".into(), range: ByteRange::Bounded(size..size + 1), size };
        assert_eq!(read("key", size..size + 1), Err(past));
        assert_eq!(read("key", 5..5), Ok(Some(Vec::new())));
        assert_eq!(read("gone", 0..10), Ok(None));
    }
}
