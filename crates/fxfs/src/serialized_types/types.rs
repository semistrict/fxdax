// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's `src/storage/fxfs/src/serialized_types/types.rs`. fxdax deviates from upstream here:
// it never migrates (AGENTS.md), so every format change is breaking, and the only version a mount
// reads is the latest (`EARLIEST_SUPPORTED_VERSION` is `LATEST_VERSION`); the table lists no type a
// version before the latest used. Version 60 locates data in extents of an extent store rather
// than at device offsets (backlog doc-4): the types that hold locations are V60. AllocatorInfoV60
// adds snapshots' owners, the sequence number the allocator mints extents from, and the extents
// dying when the allocator's tree was compacted, which a mount frees again. Version 61 adds
// StoreInfoV61: layers of a base, and the branches of its extents (backlog doc-10).

use crate::blob_metadata::{BlobMetadata, BlobMetadataV53};
use crate::lsm_tree::{
    PersistentLayerHeader, PersistentLayerHeaderV39, PersistentLayerInfo, PersistentLayerInfoV39,
};
use crate::object_store::allocator::{
    AllocatorInfo, AllocatorInfoV60, AllocatorKey, AllocatorKeyV60, AllocatorValue,
    AllocatorValueV32,
};
use crate::object_store::journal::super_block::{
    SuperBlockHeader, SuperBlockHeaderV32, SuperBlockRecord, SuperBlockRecordV60,
};
use crate::object_store::journal::{JournalRecord, JournalRecordV60};
use crate::object_store::object_record::{
    FsverityMetadata, FsverityMetadataV50, ObjectKey, ObjectKeyV54, ObjectValue, ObjectValueV60,
};
use crate::object_store::transaction::{Mutation, MutationV60};
use crate::object_store::{
    EncryptedMutations, EncryptedMutationsV49, EncryptedTransaction, EncryptedTransactionV60,
    StoreInfo, StoreInfoV61,
};
use crate::serialized_types::{Version, Versioned, VersionedLatest, versioned_type};
use std::collections::BTreeMap;

/// The latest version of on-disk filesystem format.
///
/// If all layer files are compacted the the journal flushed, and super-block
/// both rewritten, all versions should match this value.
///
/// If making a breaking change, please see EARLIEST_SUPPORTED_VERSION (below).
///
/// IMPORTANT: When changing this (major or minor), update the list of possible versions at
/// https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/cobalt_config/fuchsia/local_storage/versions.txt.
pub const LATEST_VERSION: Version = Version { major: 61, minor: 0 };

/// The last version of the filesystem where keys in layer files used the old serialization format
/// (prior to SerializeKey and delta encoding).
pub const OLD_KEY_SERIALIZATION_VERSION: Version = Version { major: 57, minor: 0 };

/// The earliest supported version of the on-disk filesystem format.
///
/// When a breaking change is made:
/// 1) LATEST_VERSION should have it's major component increased (see above).
/// 2) EARLIEST_SUPPORTED_VERSION should be set to the new LATEST_VERSION.
/// 3) The SuperBlockHeader version (below) should also be set to the new LATEST_VERSION.
///
/// Also check the constant version numbers above for any code cleanup that can happen.
///
/// fxdax: the latest, whatever it is: every change is breaking, as no format is migrated.
pub const EARLIEST_SUPPORTED_VERSION: Version = LATEST_VERSION;

// From this version forward, the journal encryption uses AES-256-XTS instead of Chacha20, as well
// as chunking the mutations together for a transaction.
pub const AES_JOURNAL_ENCRYPTION_VERSION: Version = Version { major: 57, minor: 0 };

/// This trait prevents types from showing up in `versioned_types` multiple times. `versioned_types`
/// implements this trait for every type passed to it. If a type is listed multiple times then this
/// trait will be implemented for the type multiple times which will fail to compile.
#[allow(dead_code)]
trait UniqueVersionForType {}

trait HasVersion {
    const VERSION: u32;
}

macro_rules! versioned_types {
    ( $( $name:ident { $latest:literal.. => $latest_type:ty $(, $major:literal.. => $type:ty )* $(,)? } )+ ) => {
        $(
            static_assertions::assert_type_eq_all!($name, $latest_type);

            versioned_type! {
                $latest.. => $latest_type,
                $( $major.. => $type ),*
            }

            impl UniqueVersionForType for $latest_type {}
            $( impl UniqueVersionForType for $type {} )*

            impl HasVersion for $latest_type {
                const VERSION: u32 = $latest;
            }

        )+

        pub fn get_type_fingerprints(version: Version) -> BTreeMap<String, String> {
            let mut map = BTreeMap::new();
            $(
                let fingerprint = {
                    let mut fp = None;
                    const FINGERPRINTS: &[(u32, fn() -> String)] = &[
                        ($latest, <$latest_type as fprint::TypeFingerprint>::fingerprint),
                        $( ($major, <$type as fprint::TypeFingerprint>::fingerprint) ),*
                    ];
                    for (major, type_fp) in FINGERPRINTS {
                        if version.major >= *major {
                            fp = Some(type_fp());
                            break;
                        }
                    }
                    fp
                };
                if let Some(fp) = fingerprint {
                    map.insert(stringify!($name).to_string(), fp.to_string());
                }
            )+
            map
        }
    };
}

versioned_types! {
    AllocatorInfo {
        60.. => AllocatorInfoV60,
    }
    AllocatorKey {
        60.. => AllocatorKeyV60,
    }
    AllocatorValue {
        32.. => AllocatorValueV32,
    }
    EncryptedMutations {
        49.. => EncryptedMutationsV49,
    }
    EncryptedTransaction {
        60.. => EncryptedTransactionV60,
    }
    FsverityMetadata {
        50.. => FsverityMetadataV50,
    }
    JournalRecord {
        60.. => JournalRecordV60,
    }
    Mutation {
        60.. => MutationV60,
    }
    ObjectKey {
        54.. => ObjectKeyV54,
    }
    ObjectValue {
        60.. => ObjectValueV60,
    }
    PersistentLayerHeader {
        39.. => PersistentLayerHeaderV39,
    }
    PersistentLayerInfo {
        39.. => PersistentLayerInfoV39,
    }
    StoreInfo {
        61.. => StoreInfoV61,
    }
    SuperBlockHeader {
        32.. => SuperBlockHeaderV32,
    }
    SuperBlockRecord {
        60.. => SuperBlockRecordV60,
    }
    BlobMetadata {
        53.. => BlobMetadataV53,
    }
}

// EncryptedTransaction is actually inside the Mutation type, but obscured by a layer of
// encryption. This ensures that when the EncryptedTransaction type is incremented, so is the
// Mutation and thus any types above that.
static_assertions::const_assert!(
    <EncryptedTransaction as HasVersion>::VERSION <= <Mutation as HasVersion>::VERSION
);
