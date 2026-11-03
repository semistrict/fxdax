// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's `src/storage/fxfs/src/object_store/extent_record.rs`. fxdax: an extent is located by
// an extent in the extent store and an offset within it (`ExtentValueV60`), not a device offset
// (backlog doc-4); and an overwrite extent's bitmap, offset, is collected rather than split off
// (`offset_by`), as bit-vec 0.10's `split_off` may leave storage its deserializer refuses. What a
// record holds is what an image says, so what's offset or shrunk stays within it (`entries_in`).

// TODO(https://fxbug.dev/42178223): need validation after deserialization. fxdax: what a layer or
// the journal holds is validated as it's read (`ExtentValue::fits`, `ObjectValue::is_valid_for`).

use crate::checksum::{Checksums, ChecksumsV38};
use bit_vec::BitVec;
use fprint::TypeFingerprint;
use serde::{Deserialize, Serialize};
use storage_device::ExtentId;
use storage_units::BlockSize;

/// The mode the extent is operating in. This changes how writes work to this region of the file.
pub type ExtentMode = ExtentModeV38;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TypeFingerprint)]
pub enum ExtentModeV38 {
    /// This extent doesn't have defined write semantics. The writer chooses how to handle the data
    /// here. Notable uses of this are things which have their own separate checksum mechanism,
    /// like the journal file and blobs.
    Raw,
    /// This extent uses copy-on-write semantics. We store the post-encryption checksums for data
    /// validation. New writes to this logical range are written to new extents.
    Cow(ChecksumsV38),
    /// This extent uses overwrite semantics. The bitmap keeps track of blocks which have been
    /// written to at least once. Blocks which haven't been written to at least once are logically
    /// zero, so the bitmap needs to be accounted for while reading. While this extent exists, new
    /// writes to this logical range will go to the same on-disk location.
    OverwritePartial(BitVec),
    /// This extent uses overwrite semantics. Every block in this extent has been written to at
    /// least once, so we don't store the block bitmap anymore. While this extent exists, new
    /// writes to this logical range will go to the same on-disk location.
    Overwrite,
}

#[cfg(fuzz)]
impl<'a> arbitrary::Arbitrary<'a> for ExtentMode {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(match u.int_in_range(0..=3)? {
            0 => ExtentMode::Raw,
            1 => ExtentMode::Cow(u.arbitrary()?),
            2 => ExtentMode::OverwritePartial(BitVec::from_bytes(u.arbitrary()?)),
            3 => ExtentMode::Overwrite,
            _ => unreachable!(),
        })
    }
}

/// fxdax: how many of the `entries` that cover an extent of `extent_len` bytes, a checksum's or a
/// bitmap's, cover its first `amount` bytes. A record merges as it was read, without the checks
/// that reading it for data makes, so `entries` may be none, or more than `extent_len`: this is
/// then all of them, however little they fit.
fn entries_in(amount: u64, extent_len: u64, entries: usize) -> usize {
    match extent_len.checked_div(entries as u64) {
        Some(0) | None => entries,
        Some(len) => (amount / len).try_into().unwrap_or(usize::MAX).min(entries),
    }
}

/// ExtentValue is the payload for an extent in the object store, which describes where the extent
/// is physically located.
pub type ExtentValue = ExtentValueV60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TypeFingerprint)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub enum ExtentValueV60 {
    /// Indicates a deleted extent; that is, the logical range described by the extent key is
    /// considered to be deleted.
    None,
    /// The location of the extent, `extent_offset` bytes into `extent` in the extent store, and
    /// other related information.  `key_id` identifies which of the object's keys should be used.
    /// Unencrypted files should use 0 (which can also be used for encrypted files).  `mode`
    /// describes the write pattern for this extent.
    Some { extent: ExtentId, extent_offset: u64, mode: ExtentModeV38, key_id: u64 },
}

impl ExtentValue {
    pub fn new(extent: ExtentId, extent_offset: u64, mode: ExtentMode, key_id: u64) -> ExtentValue {
        ExtentValue::Some { extent, extent_offset, mode, key_id }
    }

    pub fn new_raw(extent: ExtentId, extent_offset: u64, key_id: u64) -> ExtentValue {
        Self::new(extent, extent_offset, ExtentMode::Raw, key_id)
    }

    /// Creates an ExtentValue with a checksum
    pub fn with_checksum(
        extent: ExtentId,
        extent_offset: u64,
        checksums: Checksums,
        key_id: u64,
    ) -> ExtentValue {
        Self::new(extent, extent_offset, ExtentMode::Cow(checksums), key_id)
    }

    /// Creates an ExtentValue for an overwrite range with no blocks written to yet.
    pub fn blank_overwrite_extent(
        extent: ExtentId,
        extent_offset: u64,
        num_blocks: usize,
        key_id: u64,
    ) -> ExtentValue {
        Self::new(
            extent,
            extent_offset,
            ExtentMode::OverwritePartial(BitVec::from_elem(num_blocks, false)),
            key_id,
        )
    }

    /// Creates an ExtentValue for an overwrite range with all the blocks initialized.
    pub fn initialized_overwrite_extent(
        extent: ExtentId,
        extent_offset: u64,
        key_id: u64,
    ) -> ExtentValue {
        Self::new(extent, extent_offset, ExtentMode::Overwrite, key_id)
    }

    /// Creates an ObjectValue for a deletion of an object extent.
    pub fn deleted_extent() -> ExtentValue {
        ExtentValue::None
    }

    pub fn is_deleted(&self) -> bool {
        if let ExtentValue::None = self { true } else { false }
    }

    /// fxdax: whether what this says fits an extent `len` bytes long, of `block_size` blocks:
    /// checksums each of a whole number of blocks, a bit a block, and an offset on blocks that
    /// doesn't overflow, as an object store's records must (`ObjectValue::is_valid_for`).
    pub fn fits(&self, len: u64, block_size: BlockSize) -> bool {
        let ExtentValue::Some { extent_offset, mode, .. } = self else {
            return true;
        };
        let entries_fit = match mode {
            ExtentMode::Cow(checksums) if checksums.len() > 0 => {
                let sums = checksums.len() as u64;
                len % sums == 0 && block_size.is_aligned(len / sums)
            }
            ExtentMode::OverwritePartial(bitmap) => bitmap.len() as u64 == len / block_size,
            _ => true,
        };
        entries_fit
            && block_size.is_aligned(*extent_offset)
            && extent_offset.checked_add(len).is_some()
    }

    /// Returns a new ExtentValue offset by `amount`.  Both `amount` and `extent_len` must be
    /// multiples of the underlying block size.
    pub fn offset_by(&self, amount: u64, extent_len: u64) -> Self {
        match self {
            ExtentValue::None => Self::deleted_extent(),
            ExtentValue::Some { extent, extent_offset, mode, key_id } => {
                let mode = match mode {
                    ExtentMode::Raw => ExtentMode::Raw,
                    ExtentMode::Cow(checksums) => {
                        if checksums.len() > 0 {
                            let index = entries_in(amount, extent_len, checksums.len());
                            ExtentMode::Cow(checksums.offset_by(index))
                        } else {
                            ExtentMode::Cow(Checksums::fletcher(Vec::new()))
                        }
                    }
                    ExtentMode::Overwrite => ExtentMode::Overwrite,
                    ExtentMode::OverwritePartial(bitmap) => {
                        let index = entries_in(amount, extent_len, bitmap.len());
                        // fxdax: the bits after `index`, collected: bit-vec 0.10's `split_off` can
                        // leave a word of storage more than the bits need, which its deserializer
                        // refuses, so that the record couldn't be read back.
                        ExtentMode::OverwritePartial(bitmap.iter().skip(index).collect())
                    }
                };
                ExtentValue::Some {
                    extent: *extent,
                    extent_offset: extent_offset.saturating_add(amount),
                    mode,
                    key_id: *key_id,
                }
            }
        }
    }

    /// Returns a new ExtentValue after shrinking the extent from |original_len| to |new_len|.
    pub fn shrunk(&self, original_len: u64, new_len: u64) -> Self {
        match self {
            ExtentValue::None => Self::deleted_extent(),
            ExtentValue::Some { extent, extent_offset, mode, key_id } => {
                let mode = match mode {
                    ExtentMode::Raw => ExtentMode::Raw,
                    ExtentMode::Cow(checksums) => {
                        if checksums.len() > 0 {
                            let len = entries_in(new_len, original_len, checksums.len());
                            ExtentMode::Cow(checksums.shrunk(len))
                        } else {
                            ExtentMode::Cow(Checksums::fletcher(Vec::new()))
                        }
                    }
                    ExtentMode::Overwrite => ExtentMode::Overwrite,
                    ExtentMode::OverwritePartial(bitmap) => {
                        let len = entries_in(new_len, original_len, bitmap.len());
                        let mut new_bitmap = bitmap.clone();
                        new_bitmap.truncate(len);
                        ExtentMode::OverwritePartial(new_bitmap)
                    }
                };
                ExtentValue::Some {
                    extent: *extent,
                    extent_offset: *extent_offset,
                    mode,
                    key_id: *key_id,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExtentMode, ExtentValue};
    use crate::checksum::Checksums;
    use crate::object_store::VOLUME_DATA_KEY_ID;
    use bit_vec::BitVec;
    use storage_device::ExtentId;

    /// The extent the tests' extents are in.
    const E: ExtentId = ExtentId::new(0, 0);

    /// fxdax: a record's checksums and bitmap may be as many as an image says, and merging splits
    /// them: too many for the extent, or none, stay within what there is.
    #[test]
    fn extent_entries_that_do_not_fit() {
        let many = ExtentValue::with_checksum(E, 0, Checksums::fletcher(vec![1; 8]), 0);
        assert_eq!(
            many.offset_by(2, 4),
            ExtentValue::with_checksum(E, 2, Checksums::fletcher(Vec::new()), 0)
        );
        assert_eq!(
            many.shrunk(4, 2),
            ExtentValue::with_checksum(E, 0, Checksums::fletcher(vec![1; 8]), 0)
        );
        let none = ExtentMode::OverwritePartial(BitVec::new());
        let none = ExtentValue::Some { extent: E, extent_offset: 0, mode: none, key_id: 0 };
        assert_eq!(none.offset_by(4096, 8192).shrunk(4096, 2048), {
            let mode = ExtentMode::OverwritePartial(BitVec::new());
            ExtentValue::Some { extent: E, extent_offset: 4096, mode, key_id: 0 }
        });
        let few = ExtentValue::with_checksum(E, u64::MAX, Checksums::fletcher(vec![1, 2]), 0);
        assert_eq!(
            few.offset_by(u64::MAX, 2),
            ExtentValue::with_checksum(E, u64::MAX, Checksums::fletcher(Vec::new()), 0)
        );
    }

    #[test]
    fn extent_value_offset_by() {
        assert_eq!(ExtentValue::None.offset_by(1024, 2048), ExtentValue::None);
        assert_eq!(
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID).offset_by(0, 2048),
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID)
        );
        assert_eq!(
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID).offset_by(1024, 2048),
            ExtentValue::new_raw(E, 2048, VOLUME_DATA_KEY_ID)
        );
        assert_eq!(
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID).offset_by(2048, 2048),
            ExtentValue::new_raw(E, 3072, VOLUME_DATA_KEY_ID)
        );

        let make_checksums = |range: std::ops::Range<u64>| Checksums::fletcher(range.collect());

        // In these tests we are making block size 256.
        assert_eq!(
            ExtentValue::with_checksum(E, 1024, make_checksums(0..8), VOLUME_DATA_KEY_ID)
                .offset_by(0, 2048),
            ExtentValue::with_checksum(E, 1024, make_checksums(0..8), VOLUME_DATA_KEY_ID)
        );
        assert_eq!(
            ExtentValue::with_checksum(E, 1024, make_checksums(0..8), VOLUME_DATA_KEY_ID)
                .offset_by(1024, 2048),
            ExtentValue::with_checksum(E, 2048, make_checksums(4..8), VOLUME_DATA_KEY_ID)
        );
        assert_eq!(
            ExtentValue::with_checksum(E, 1024, make_checksums(0..8), VOLUME_DATA_KEY_ID)
                .offset_by(2048, 2048),
            ExtentValue::with_checksum(
                E,
                3072,
                Checksums::fletcher(Vec::new()),
                VOLUME_DATA_KEY_ID
            )
        );

        // Takes a place to switch from zeros to ones. The goal is to make sure there is exactly
        // one zero and then only ones where we expect offset to slice.
        let make_bitmap = |cut, length| {
            let mut begin_bitmap = BitVec::from_elem(cut, false);
            let mut end_bitmap = BitVec::from_elem(length - cut, true);
            begin_bitmap.append(&mut end_bitmap);
            ExtentMode::OverwritePartial(begin_bitmap)
        };
        let make_extent =
            |extent_offset, mode| ExtentValue::Some { extent: E, extent_offset, mode, key_id: 0 };

        assert_eq!(
            make_extent(1024, make_bitmap(1, 8)).offset_by(0, 2048),
            make_extent(1024, make_bitmap(1, 8))
        );
        assert_eq!(
            make_extent(1024, make_bitmap(5, 8)).offset_by(1024, 2048),
            make_extent(2048, make_bitmap(1, 4))
        );
        assert_eq!(
            make_extent(1024, make_bitmap(0, 8)).offset_by(2048, 2048),
            make_extent(3072, ExtentMode::OverwritePartial(BitVec::new()))
        );

        assert_eq!(
            make_extent(1024, ExtentMode::Overwrite).offset_by(0, 2048),
            make_extent(1024, ExtentMode::Overwrite)
        );
        assert_eq!(
            make_extent(1024, ExtentMode::Overwrite).offset_by(1024, 2048),
            make_extent(2048, ExtentMode::Overwrite)
        );
        assert_eq!(
            make_extent(1024, ExtentMode::Overwrite).offset_by(2048, 2048),
            make_extent(3072, ExtentMode::Overwrite)
        );
    }

    /// fxdax: an overwrite extent offset by a number of blocks that isn't a multiple of 32 keeps a
    /// bitmap that serializes and deserializes back.
    #[test]
    fn extent_value_offset_by_bitmap_round_trips() {
        use crate::serialized_types::{LATEST_VERSION, Versioned};
        let bitmap: BitVec = (0..40).map(|i| i % 3 == 0).collect();
        let value = ExtentValue::Some {
            extent: E,
            extent_offset: 0,
            mode: ExtentMode::OverwritePartial(bitmap.clone()),
            key_id: 0,
        }
        .offset_by(10 * 256, 40 * 256);
        let expected: BitVec = bitmap.iter().skip(10).collect();
        let ExtentValue::Some { mode: ExtentMode::OverwritePartial(offset), .. } = &value else {
            panic!("{value:?}");
        };
        assert_eq!(offset, &expected);
        // As a layer holds it.
        let value = crate::object_store::ObjectValue::Extent(value);
        let mut serialized = Vec::new();
        value.serialize_into(&mut serialized).expect("serialize failed");
        let read = crate::object_store::ObjectValue::deserialize_from(
            &mut std::io::Cursor::new(serialized),
            LATEST_VERSION,
        )
        .expect("deserialize failed");
        assert_eq!(read, value);
    }

    #[test]
    fn extent_value_shrunk() {
        assert_eq!(ExtentValue::None.shrunk(2048, 1024), ExtentValue::None);
        assert_eq!(
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID).shrunk(2048, 2048),
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID)
        );
        assert_eq!(
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID).shrunk(2048, 1024),
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID)
        );
        assert_eq!(
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID).shrunk(2048, 0),
            ExtentValue::new_raw(E, 1024, VOLUME_DATA_KEY_ID)
        );

        let make_checksums = |range: std::ops::Range<u64>| Checksums::fletcher(range.collect());

        // In these tests we are making block size 256.
        assert_eq!(
            ExtentValue::with_checksum(E, 1024, make_checksums(0..8), VOLUME_DATA_KEY_ID)
                .shrunk(2048, 2048),
            ExtentValue::with_checksum(E, 1024, make_checksums(0..8), VOLUME_DATA_KEY_ID)
        );
        assert_eq!(
            ExtentValue::with_checksum(E, 1024, make_checksums(0..8), VOLUME_DATA_KEY_ID)
                .shrunk(2048, 1024),
            ExtentValue::with_checksum(E, 1024, make_checksums(0..4), VOLUME_DATA_KEY_ID)
        );
        assert_eq!(
            ExtentValue::with_checksum(E, 1024, make_checksums(0..8), VOLUME_DATA_KEY_ID)
                .shrunk(2048, 0),
            ExtentValue::with_checksum(
                E,
                1024,
                Checksums::fletcher(Vec::new()),
                VOLUME_DATA_KEY_ID
            )
        );

        // Takes a place to switch from zeros to ones. The goal is to make sure there is exactly
        // one zero and then only ones where we expect offset to slice.
        let make_bitmap = |cut, length| {
            let mut begin_bitmap = BitVec::from_elem(cut, false);
            let mut end_bitmap = BitVec::from_elem(length - cut, true);
            begin_bitmap.append(&mut end_bitmap);
            ExtentMode::OverwritePartial(begin_bitmap)
        };
        let make_extent =
            |extent_offset, mode| ExtentValue::Some { extent: E, extent_offset, mode, key_id: 0 };

        assert_eq!(
            make_extent(1024, make_bitmap(1, 8)).shrunk(2048, 2048),
            make_extent(1024, make_bitmap(1, 8))
        );
        assert_eq!(
            make_extent(1024, make_bitmap(3, 8)).shrunk(2048, 1024),
            make_extent(1024, make_bitmap(3, 4))
        );
        assert_eq!(
            make_extent(1024, make_bitmap(0, 8)).shrunk(2048, 0),
            make_extent(1024, ExtentMode::OverwritePartial(BitVec::new()))
        );

        assert_eq!(
            make_extent(1024, ExtentMode::Overwrite).shrunk(2048, 2048),
            make_extent(1024, ExtentMode::Overwrite)
        );
        assert_eq!(
            make_extent(1024, ExtentMode::Overwrite).shrunk(2048, 1024),
            make_extent(1024, ExtentMode::Overwrite)
        );
        assert_eq!(
            make_extent(1024, ExtentMode::Overwrite).shrunk(2048, 0),
            make_extent(1024, ExtentMode::Overwrite)
        );
    }
}
