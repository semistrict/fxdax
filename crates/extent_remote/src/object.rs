// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Objects framed as `extent_frame` frames them, read from a bucket by ranged reads (doc-7,
//! Segments): their tails, which say where their parts are stored and hold their footers, and
//! ranges of their data, by the parts that hold them.

use crate::Error;
use crate::bucket::{self, Bucket, ByteRange};
use crate::layout;
use extent_frame::{Frame, Parts, TRAILER};
use serde::de::DeserializeOwned;
use std::ops::Range;

/// How many bytes of an object's end a read of its tail reads at first: its trailer, its table
/// of 238 parts, and its footer, if they fit.
pub const TAIL: u64 = 4096;

/// Where the parts of the object at `key` are stored, and its footer, checked against its CRC, if
/// there is one: a ranged read of its last [`TAIL`] bytes, and a second, of its footer and table,
/// if they are longer.
pub async fn parts_and_footer(
    bucket: &impl Bucket,
    key: &str,
) -> Result<Option<(Parts, Vec<u8>)>, Error> {
    let Some(mut suffix) = bucket::get_range(bucket, key, ByteRange::Suffix(TAIL)).await? else {
        return Ok(None);
    };
    let frame = Frame::of_trailer(&suffix)?;
    // An object shorter than the read is in it whole.
    if (suffix.len() as u64) < TAIL && frame.size() != suffix.len() as u64 {
        return Err(Error::Corrupt(format!("{key}, by its trailer")));
    }
    if frame.size() - frame.footer().start > suffix.len() as u64 {
        let tail = ByteRange::Bounded(frame.tail());
        let Some(longer) = bucket::get_range(bucket, key, tail).await? else { return Ok(None) };
        suffix = longer;
    }
    let stored = frame.stored_in(&suffix)?;
    let at = suffix.len() - (frame.size() - frame.footer().start) as usize;
    let footer = frame.check_footer(&suffix[at..at + frame.footer_len as usize])?.to_vec();
    Ok(Some((Parts { frame, stored }, footer)))
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

/// The bytes `range` of the data of the object at `key`, whose parts are `parts`: a ranged read of
/// each part the range touches, checked and decompressed; none if the object is gone.
pub async fn range(
    bucket: &impl Bucket,
    key: &str,
    parts: &Parts,
    range: Range<u64>,
) -> Result<Option<Vec<u8>>, Error> {
    if range.start > range.end || range.end > parts.frame.data_len {
        return Err(Error::Corrupt(format!("{range:?} past the end of {key}")));
    }
    let numbers = parts.frame.parts_of(range.clone());
    let reads = numbers.map(|part| async move {
        let (place, stored) = parts.part(part)?;
        let bytes = bucket::get_range(bucket, key, ByteRange::Bounded(stored.range())).await?;
        Ok::<_, Error>((place, stored, bytes))
    });
    let mut data = Vec::with_capacity((range.end - range.start) as usize);
    for (place, stored, bytes) in futures::future::try_join_all(reads).await? {
        let Some(bytes) = bytes else { return Ok(None) };
        let unpacked = stored.unpack(&bytes, place.end - place.start)?;
        let from = range.start.max(place.start) - place.start;
        let to = range.end.min(place.end) - place.start;
        data.extend_from_slice(&unpacked[from as usize..to as usize]);
    }
    Ok(Some(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::PutMode;
    use crate::sim::bucket::{Direct, SimBucket};
    use extent_frame::PART;
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
    #[test]
    fn tails_are_read_once_when_they_fit() {
        let read = |bytes: &[u8]| {
            let bucket = holding(bytes.to_vec());
            bucket.served(parts_and_footer(&*bucket, KEY))
        };
        let tail_len = |bytes: &[u8]| {
            let tail = Frame::of(bytes).unwrap().tail();
            tail.end - tail.start
        };
        let first = format!("get {KEY} last {TAIL}");

        let short = layout::with_footer(&[1; 5000], &"footer");
        assert!((short.len() as u64) < TAIL);
        let (tail, asked) = read(&short);
        assert_eq!(tail, Ok(Some((Parts::of(&short).unwrap(), layout::encode(&"footer")))));
        assert_eq!(asked, std::slice::from_ref(&first));

        let fitting = (0..TAIL as usize)
            .map(|len| layout::with_footer(&[1; 5000], &vec![2u8; len]))
            .find(|bytes| tail_len(bytes) == TAIL)
            .unwrap();
        assert!(fitting.len() as u64 > TAIL);
        let (tail, asked) = read(&fitting);
        assert_eq!(tail.unwrap().unwrap().0, Parts::of(&fitting).unwrap());
        assert_eq!(asked, std::slice::from_ref(&first));

        let long = layout::with_footer(&[1; 5000], &vec![2u8; 5000]);
        let range = Frame::of(&long).unwrap().tail();
        let (tail, asked) = read(&long);
        assert_eq!(tail.unwrap().unwrap().1, layout::encode(&vec![2u8; 5000]));
        assert_eq!(asked, [first, format!("get {KEY} {range:?}")]);
    }

    /// An object shorter than a tail's read whose trailer says another length is corrupt.
    #[test]
    fn short_objects_whose_trailers_lie_are_corrupt() {
        let mut bytes = vec![0; 10];
        bytes.extend_from_slice(&layout::with_footer(&[1; 100], &"footer"));
        let bucket = holding(bytes);
        let corrupt = Error::Corrupt(format!("{KEY}, by its trailer"));
        assert_eq!(bucket.served(parts_and_footer(&*bucket, KEY)).0, Err(corrupt));
    }

    /// A range of an object's data is read from the parts it touches; an empty one reads none; one
    /// reversed, or past the data's end, is corrupt.
    #[test]
    fn data_is_read_by_the_parts_a_range_touches() {
        let data: Vec<u8> = (0..2 * PART + 100).map(|i| (i % 251) as u8).collect();
        let bytes = layout::with_footer(&data, &());
        let parts = Parts::of(&bytes).unwrap();
        let bucket = holding(bytes);
        let read = |range: Range<u64>| bucket.served(super::range(&*bucket, KEY, &parts, range));

        let (read_back, asked) = read(PART - 5..PART + 5);
        assert_eq!(read_back, Ok(Some(data[(PART - 5) as usize..(PART + 5) as usize].to_vec())));
        assert_eq!(asked.len(), 2);
        assert_eq!(read(7..7), (Ok(Some(Vec::new())), Vec::new()));
        let end = data.len() as u64;
        for range in [Range { start: 8, end: 7 }, end - 1..end + 1] {
            let corrupt = format!("{range:?} past the end of {KEY}");
            assert_eq!(read(range).0, Err(Error::Corrupt(corrupt)));
        }
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
