// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Objects framed as `extent_frame` frames them, read from a bucket by ranged reads (doc-7,
//! Segments): their tails, which say where their parts are stored and hold their footers. Their
//! data is read by fetchers, by the parts that hold it ([`crate::fetch`]), which keep their tails
//! as read ([`Tail`]).

use crate::Error;
use crate::bucket::{self, Bucket, ByteRange};
use crate::layout;
use extent_frame::{Frame, Parts, TRAILER};
use serde::de::DeserializeOwned;

/// How many bytes of an object's end a read of its tail reads at first: its trailer, its table
/// of 238 parts, and its footer, if they fit.
pub const TAIL: u64 = 4096;

/// An object's tail: its footer, the table of where its parts are stored, and its trailer, the
/// object's bytes from where its footer begins to its end; and where its parts are stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tail {
    pub bytes: Vec<u8>,
    pub parts: Parts,
}

impl Tail {
    /// The tail of the object at `key` that `bytes` are, its bytes from where its footer begins
    /// to its end: its footer and table checked against their CRCs.
    pub fn of(key: &str, bytes: Vec<u8>) -> Result<Self, Error> {
        let frame = Frame::of_trailer(&bytes)?;
        let tail = frame.tail();
        if tail.end - tail.start != bytes.len() as u64 {
            return Err(Error::Corrupt(format!("{key}, by its trailer")));
        }
        let stored = frame.stored_in(&bytes)?;
        frame.check_footer(&bytes[..frame.footer_len as usize])?;
        Ok(Self { bytes, parts: Parts { frame, stored } })
    }

    /// Its footer.
    pub fn footer(&self) -> &[u8] {
        &self.bytes[..self.parts.frame.footer_len as usize]
    }
}

/// The tail of the object at `key`, if there is one: a ranged read of its last [`TAIL`] bytes,
/// and a second, of its footer and table, if they are longer.
pub async fn tail(bucket: &impl Bucket, key: &str) -> Result<Option<Tail>, Error> {
    let Some(mut suffix) = bucket::get_range(bucket, key, ByteRange::Suffix(TAIL)).await? else {
        return Ok(None);
    };
    let frame = Frame::of_trailer(&suffix)?;
    // An object shorter than the read is in it whole.
    if (suffix.len() as u64) < TAIL && frame.size() != suffix.len() as u64 {
        return Err(Error::Corrupt(format!("{key}, by its trailer")));
    }
    let tail = frame.tail();
    let len = (tail.end - tail.start) as usize;
    if len > suffix.len() {
        let Some(longer) = bucket::get_range(bucket, key, ByteRange::Bounded(tail)).await? else {
            return Ok(None);
        };
        suffix = longer;
    }
    let bytes = suffix.split_off(suffix.len().saturating_sub(len));
    Ok(Some(Tail::of(key, bytes)?))
}

/// The footer of the object at `key`, if there is one: a ranged read of its trailer, then one of
/// its footer, checked against its CRC.
pub async fn footer<T: DeserializeOwned>(
    bucket: &impl Bucket,
    key: &str,
) -> Result<Option<T>, Error> {
    let Some(trailer) = bucket::get_range(bucket, key, ByteRange::Suffix(TRAILER)).await? else {
        return Ok(None);
    };
    let frame = Frame::of_trailer(&trailer)?;
    let Some(footer) = bucket::get_range(bucket, key, ByteRange::Bounded(frame.footer())).await?
    else {
        return Ok(None);
    };
    Ok(Some(layout::decode(frame.check_footer(&footer)?)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::PutMode;
    use crate::sim::bucket::{Direct, SimBucket};
    use futures::executor::block_on;
    use std::sync::Arc;

    const KEY: &str = "checkpoints/0000000000000001/0000000000000002";

    /// A bucket holding `bytes` at [`KEY`].
    fn holding(bytes: Vec<u8>) -> Arc<SimBucket> {
        let bucket = SimBucket::new();
        block_on(Direct(bucket.clone()).put(KEY, bytes, PutMode::Create)).unwrap();
        bucket
    }

    /// An object's tail is read with one ranged read of its last [`TAIL`] bytes, or all of it if
    /// it is shorter, when its footer and table fit in them; and with a second, of them, when not.
    /// It is the object's bytes from where its footer begins.
    #[test]
    fn tails_are_read_once_when_they_fit() {
        let read = |bytes: &[u8]| {
            let bucket = holding(bytes.to_vec());
            bucket.served(tail(&*bucket, KEY))
        };
        let read_tail = |bytes: &[u8]| {
            let start = Frame::of(bytes).unwrap().tail().start as usize;
            Tail { bytes: bytes[start..].to_vec(), parts: Parts::of(bytes).unwrap() }
        };
        let first = format!("get {KEY} last {TAIL}");

        let short = layout::with_footer(&[1; 5000], &"footer");
        assert!((short.len() as u64) < TAIL);
        let (read_back, asked) = read(&short);
        let read_back = read_back.unwrap().unwrap();
        assert_eq!(read_back, read_tail(&short));
        assert_eq!(read_back.footer(), layout::encode(&"footer"));
        assert_eq!(asked, std::slice::from_ref(&first));

        let fitting = (0..TAIL as usize)
            .map(|len| layout::with_footer(&[1; 5000], &vec![2u8; len]))
            .find(|bytes| read_tail(bytes).bytes.len() as u64 == TAIL)
            .unwrap();
        assert!(fitting.len() as u64 > TAIL);
        let (read_back, asked) = read(&fitting);
        assert_eq!(read_back, Ok(Some(read_tail(&fitting))));
        assert_eq!(asked, std::slice::from_ref(&first));

        let long = layout::with_footer(&[1; 5000], &vec![2u8; 5000]);
        let range = Frame::of(&long).unwrap().tail();
        let (read_back, asked) = read(&long);
        let read_back = read_back.unwrap().unwrap();
        assert_eq!(read_back.footer(), layout::encode(&vec![2u8; 5000]));
        assert_eq!(read_back, read_tail(&long));
        assert_eq!(asked, [first, format!("get {KEY} {range:?}")]);
    }

    /// An object shorter than a tail's read whose trailer says another length is corrupt; so are
    /// bytes taken for a tail that are not the tail their trailer says.
    #[test]
    fn short_objects_whose_trailers_lie_are_corrupt() {
        let mut bytes = vec![0; 10];
        bytes.extend_from_slice(&layout::with_footer(&[1; 100], &"footer"));
        let bucket = holding(bytes.clone());
        let corrupt = Error::Corrupt(format!("{KEY}, by its trailer"));
        assert_eq!(bucket.served(tail(&*bucket, KEY)).0, Err(corrupt.clone()));
        assert_eq!(Tail::of(KEY, bytes), Err(corrupt));
    }

    /// An object's footer is read with two ranged reads, of its trailer and of the footer; none
    /// for an object gone.
    #[test]
    fn footers_are_read_by_two_ranges() {
        let bytes = layout::with_footer(&[1; 5000], &"footer");
        let bucket = holding(bytes.clone());
        let (read, asked) = bucket.served(footer::<String>(&*bucket, KEY));
        assert_eq!(read, Ok(Some("footer".to_owned())));
        let range = Frame::of(&bytes).unwrap().footer();
        assert_eq!(asked, [format!("get {KEY} last {TRAILER}"), format!("get {KEY} {range:?}")]);
        let (gone, asked) = bucket.served(footer::<String>(&*bucket, "moves/gone"));
        assert_eq!((gone, asked), (Ok(None), vec![format!("get moves/gone last {TRAILER}")]));
    }
}
