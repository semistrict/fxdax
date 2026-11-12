// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a byte of a framed file, a closed segment's or a sealed extent's own, that the host's
//! disk damaged fails whatever reads it, rather than giving another byte, and goes nowhere under a
//! CRC of its own (TASK-153.11): the store's reads, which the guest's and the extent cache's
//! page-ins are (`Store::read`), the uploader's (`Store::read_kept`, and the segments it takes as
//! they are, `Closed::bytes`), a follower's (`Store::read_view`), a fork's of what it inherited,
//! and the copies of a seal. Of parts stored compressed and stored raw alike: a raw
//! part was read in place, unchecked, until TASK-153.11.

mod common;

use common::noise;
use extent_frame::{Corrupt, Encoding, PART, Parts};
use extent_store::{Description, Dir, File as _, MemDir, Options, Place, Store};
use storage_device::ExtentId;

const BLOCK: usize = 4096;
const MIB: usize = 1 << 20;
const BRANCH: extent_remote::layout::Branch = extent_remote::layout::test_branch(7);

/// Segments of a few parts, and extents on their own from 8 MiB.
const OPTIONS: Options = Options {
    max_extent: 64 << 20,
    segment_size: 4 << 20,
    segment_age: std::time::Duration::from_secs(3600),
    large_extent: 8 << 20,
    open_files: 2,
    view_lag: None,
    view_kept_bytes: None,
    held_most: 256 << 10,
};

fn extent(sequence: u64) -> ExtentId {
    ExtentId::new(BRANCH.id, sequence)
}

/// Bytes each their offset modulo 251, plus `seed`: they compress.
fn pattern(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8 ^ seed).collect()
}

fn create(dir: &MemDir, options: Options) -> Store<MemDir> {
    let description = Description { block_size: BLOCK as u32, capacity: 256 << 20, branch: BRANCH };
    Store::create(dir.clone(), description, options).unwrap()
}

/// The extents [`holding`] writes: in a segment, compressed and raw, and on their own, compressed
/// and raw, each across parts.
fn contents() -> Vec<(ExtentId, Vec<u8>, Encoding)> {
    vec![
        (extent(1), pattern(1, 3 * MIB / 2), Encoding::Zstd),
        (extent(2), noise(2, MIB + 8 * BLOCK), Encoding::Raw),
        (extent(3), pattern(3, 9 * MIB), Encoding::Zstd),
        (extent(4), noise(4, 9 * MIB), Encoding::Raw),
    ]
}

/// A store holding [`contents`], sealed, its segment closed.
fn holding(dir: &MemDir) -> Store<MemDir> {
    let store = create(dir, OPTIONS);
    for (extent, data, _) in contents() {
        store.write(extent, 0, &data).unwrap();
    }
    store.close_segment().unwrap();
    store.commit(None).unwrap();
    store
}

/// A byte of a framed file, as stored: in a file at a path, at an offset.
struct Stored {
    path: String,
    at: u64,
    /// How the part it is in is stored.
    encoding: Encoding,
}

/// The stored byte of `extent`, which `store` holds sealed, at `offset` in it: the byte itself in
/// a part stored raw, the middle of the part's stored bytes in one stored compressed.
fn stored(dir: &MemDir, store: &Store<MemDir>, extent: ExtentId, offset: u64) -> Stored {
    let (path, at) = match store.place(extent).unwrap().unwrap() {
        Place::Run { branch, number, offset: run } => {
            (format!("segments/{branch:016x}/{number:016x}"), run + offset)
        }
        Place::Own => (format!("extents/{:016x}/{:016x}", extent.branch, extent.sequence), offset),
        place => panic!("{extent:?} is {place:?}"),
    };
    let bytes = extent_store::read_whole(&dir.open(&path, true).unwrap()).unwrap();
    let (place, part) = Parts::of(&bytes).unwrap().part(at / PART).unwrap();
    let at = match part.encoding {
        Encoding::Raw => part.offset + (at - place.start),
        Encoding::Zstd => part.offset + part.len / 2,
    };
    Stored { path, at, encoding: part.encoding }
}

/// Flips a bit of `byte`, or flips it back.
fn flip(dir: &MemDir, byte: &Stored) {
    let file = dir.open(&byte.path, true).unwrap();
    let mut bytes = [0];
    file.read_at(&mut bytes, byte.at).unwrap();
    bytes[0] ^= 0x10;
    file.write_at(&bytes, byte.at).unwrap();
}

/// That `result` failed for a part failing its CRC.
fn corrupt<T>(result: Result<T, anyhow::Error>) {
    let Err(error) = result else { panic!("read a flipped byte") };
    let expected = Corrupt("an object's part, by its CRC".into());
    assert_eq!(error.downcast_ref::<Corrupt>(), Some(&expected), "{error:#}");
}

/// A byte flipped in a part of a closed segment or of a sealed extent's own file, stored
/// compressed or raw, fails the store's reads of the part, its uploader's and a follower's, read
/// by a store opened anew, which has none of it unpacked: of a block, or the whole extent. Its
/// other parts read as written.
#[test]
fn flipped_bytes_fail_the_reads_of_their_parts() {
    let dir = MemDir::new();
    drop(holding(&dir));
    for (extent, data, encoding) in contents() {
        // Its last block, in another part than its first.
        let offset = (data.len() - BLOCK) as u64;
        let store = Store::open(dir.clone(), OPTIONS).unwrap();
        let byte = stored(&dir, &store, extent, offset);
        assert_eq!(byte.encoding, encoding, "{extent:?}");
        flip(&dir, &byte);
        drop(store);

        let store = Store::open(dir.clone(), OPTIONS).unwrap();
        let view = store.view();
        let mut block = vec![0; BLOCK];
        corrupt(store.read(extent, offset, &mut block));
        corrupt(store.read_view(&view, extent, offset, &mut block));
        corrupt(store.read(extent, 0, &mut vec![0; data.len()]));
        // Its first block is in another part, whole.
        store.read(extent, 0, &mut block).unwrap();
        assert!(block == data[..BLOCK], "{extent:?}'s first block");

        flip(&dir, &byte);
        drop((view, store));
    }
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    for (extent, data, _) in contents() {
        let mut read = vec![0; data.len()];
        store.read(extent, 0, &mut read).unwrap();
        assert!(read == data, "{extent:?}, flipped back");
    }
}

/// An open extent's piece in a segment closed between commits, framed, is read checked: a byte
/// flipped there fails its reads, and the seal that would copy it to a file of its own.
#[test]
fn flipped_bytes_fail_reads_and_seals_of_open_extents_in_closed_segments() {
    let dir = MemDir::new();
    let store = create(&dir, OPTIONS);
    store.write(extent(1), 0, &noise(1, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // Open, in pieces, the first in the segment closed between commits.
    store.write(extent(2), 0, &noise(2, BLOCK)).unwrap();
    assert!(store.close_now().unwrap());
    store.write(extent(2), BLOCK as u64, &noise(3, BLOCK)).unwrap();
    let segment = format!("segments/{:016x}/{:016x}", BRANCH.id, 0);
    let bytes = extent_store::read_whole(&dir.open(&segment, true).unwrap()).unwrap();
    let part = Parts::of(&bytes).unwrap().stored[0];
    assert_eq!(part.encoding, Encoding::Raw);
    // Extent 2's first block follows extent 1's.
    flip(
        &dir,
        &Stored { path: segment, at: part.offset + BLOCK as u64 + 7, encoding: part.encoding },
    );
    corrupt(store.read(extent(2), 0, &mut vec![0; 2 * BLOCK]));
    corrupt(store.commit(None));
}

/// The bytes of a closed segment the uploader takes, which it uploads as they are, are checked:
/// a byte flipped in them fails them, rather than going to object storage under the CRC it fails.
#[test]
fn closed_segments_with_flipped_bytes_are_not_taken() {
    for compressed in [false, true] {
        let dir = MemDir::new();
        let store = create(&dir, OPTIONS);
        let data = if compressed { pattern(1, 2 * MIB) } else { noise(1, 2 * MIB) };
        store.write(extent(1), 0, &data).unwrap();
        store.close_segment().unwrap();
        store.commit(None).unwrap();
        store.flush().unwrap();
        let [closed] = <[_; 1]>::try_from(store.closed(usize::MAX).unwrap()).ok().unwrap();
        let bytes = closed.bytes().unwrap();
        assert_eq!(extent_frame::data(&bytes).unwrap(), data);
        let byte = stored(&dir, &store, extent(1), MIB as u64 + 5);
        flip(&dir, &byte);
        let Err(error) = closed.bytes() else { panic!("took a flipped byte") };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "an object's part, by its CRC");
    }
}
