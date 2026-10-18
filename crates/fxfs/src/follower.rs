// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: a read-only mount follows its writer (backlog doc-8, The follower in Fxfs;
//! TASK-42.4).
//!
//! A follower is a read-only mount on a device that reads a store another mount writes, as of one
//! of its commits (`Device::advance`). [`FxFilesystem::fetch`] advances the device to the
//! writer's latest commit and reads the journal on from the end of the last whole transaction it
//! applied; [`FxFilesystem::apply`] applies each whole transaction as replay applies them, to the
//! live trees (`follow` does both, and lets go of the commit the follower was as of): the
//! root parent store's mutations, the root store's, then the other stores', opening stores the
//! writer created. A flush's last transaction (EndFlush) swaps the store's sealed layers for those
//! the flush wrote, as the writer's own commit of it does; the allocator's mutations are left, as a
//! follower allocates nothing. A fetch tells what its batch changes ([`Changes`]), for the caller
//! to invalidate what it cached, once applied (crates/follower, TASK-42.5).
//!
//! The follower's graveyard: a reader that has an object open holds it ([`FxFilesystem::hold`]).
//! When the writer deletes an object a reader holds (its graveyard entry, or its tombstone), the
//! follower keeps the object's records as they were before, and pins the extents they refer to
//! (`Device::pin`), which the writer frees: it skips the object's later mutations, and puts its
//! records back after a flush swaps in layers without them. Once the last hold goes, the next call
//! applies what it skipped, and unpins the extents.
//!
//! Callers keep readers out of the filesystem while a batch applies, or read through the
//! protocol of crates/follower: a seqcount around the apply (TASK-42.5).

use crate::filesystem::{ApplyContext, ApplyMode, FxFilesystem};
use crate::log::*;
use crate::lsm_tree::Query;
use crate::lsm_tree::types::{ItemRef, LayerIterator};
use crate::object_store::journal::{JournalCheckpoint, JournaledTransaction};
use crate::object_store::object_record::{
    AttributeId, AttributeKey, ObjectItem, ObjectKey, ObjectKeyData, ObjectValue,
};
use crate::object_store::transaction::{AssocObj, Mutation, ObjectStoreMutation};
use crate::object_store::{ExtentValue, ObjectStore};
use anyhow::{Context, Error, ensure};
use fuchsia_sync::Mutex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;
use storage_device::{ExtentId, Previous};

/// The rules a follower keeps, each of which the followers' simulation (crates/follower_sim)
/// shows to be needed by turning it off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// A flush's end swaps the store's layers for those it wrote.
    pub swap_flushed_layers: bool,
    /// An object its writer deletes is kept while a reader holds it.
    pub keep_held: bool,
    /// A batch's changes are told whole: its data ranges too.
    pub tell_data: bool,
    /// What was skipped of a kept object is applied once no reader holds it.
    pub apply_released: bool,
}

impl Rules {
    pub const ALL: Self =
        Self { swap_flushed_layers: true, keep_held: true, tell_data: true, apply_released: true };
}

/// The rules in force: all of them, but in a mutation test of the simulation.
#[cfg(not(feature = "shuttle"))]
fn rules() -> Rules {
    Rules::ALL
}

#[cfg(feature = "shuttle")]
static RULES: std::sync::Mutex<Rules> = std::sync::Mutex::new(Rules::ALL);

/// The rules in force: all of them, but in a mutation test of the simulation.
#[cfg(feature = "shuttle")]
pub fn rules() -> Rules {
    *RULES.lock().unwrap()
}

/// Sets the rules in force, for the simulation's mutation tests.
#[cfg(feature = "shuttle")]
pub fn set_rules(rules: Rules) {
    *RULES.lock().unwrap() = rules;
}

/// An object of a store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreObject {
    pub store_object_id: u64,
    pub object_id: u64,
}

impl StoreObject {
    fn new(store_object_id: u64, object_id: u64) -> Self {
        Self { store_object_id, object_id }
    }
}

/// What a batch of the writer's transactions changed (doc-8, The follower in Fxfs).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Objects created, or whose attributes (sizes, times, links, extended attributes) changed.
    pub attributes: BTreeSet<StoreObject>,
    /// Directories whose entries changed.
    pub entries: BTreeSet<StoreObject>,
    /// Ranges of objects' data that changed, in order, merged where they meet.
    pub data: BTreeMap<StoreObject, Vec<Range<u64>>>,
    /// Objects deleted: unlinked from their last directory, or tombstoned.
    pub deleted: BTreeSet<StoreObject>,
    /// Volumes deleted, with all their objects.
    pub volumes_deleted: BTreeSet<u64>,
}

impl Changes {
    fn add(&mut self, store_object_id: u64, mutation: &Mutation) {
        let item = match mutation {
            Mutation::ObjectStore(ObjectStoreMutation { item, .. }) => item,
            Mutation::DeleteVolume => {
                self.volumes_deleted.insert(store_object_id);
                return;
            }
            _ => return,
        };
        let object = StoreObject::new(store_object_id, item.key.object_id);
        match &item.key.data {
            ObjectKeyData::Object if item.is_tombstone() => {
                self.deleted.insert(object);
            }
            ObjectKeyData::Attribute(AttributeId::DATA, AttributeKey::Extent(extent)) => {
                if rules().tell_data {
                    self.data.entry(object).or_default().push(extent.0.clone())
                }
            }
            ObjectKeyData::Object
            | ObjectKeyData::Keys
            | ObjectKeyData::Attribute(..)
            | ObjectKeyData::Project { .. }
            | ObjectKeyData::ExtendedAttribute { .. } => {
                self.attributes.insert(object);
            }
            ObjectKeyData::Child { .. }
            | ObjectKeyData::EncryptedCasefoldChild(_)
            | ObjectKeyData::LegacyCasefoldChild(_)
            | ObjectKeyData::EncryptedChild(_)
            | ObjectKeyData::CasefoldChild { .. } => {
                self.entries.insert(object);
            }
            ObjectKeyData::GraveyardEntry { object_id } => {
                if matches!(item.value, ObjectValue::Some) {
                    self.deleted.insert(StoreObject::new(store_object_id, *object_id));
                }
            }
            ObjectKeyData::GraveyardAttributeEntry { object_id, .. } => {
                self.attributes.insert(StoreObject::new(store_object_id, *object_id));
            }
        }
    }

    /// Sorts and merges the data ranges.
    fn merge_ranges(&mut self) {
        for ranges in self.data.values_mut() {
            ranges.sort_by_key(|range| range.start);
            let mut merged: Vec<Range<u64>> = Vec::with_capacity(ranges.len());
            for range in ranges.drain(..) {
                match merged.last_mut() {
                    Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                    _ => merged.push(range),
                }
            }
            *ranges = merged;
        }
    }
}

/// The writer's commits a follower applies next ([`FxFilesystem::fetch`]), read whole, and what
/// keeps the commit it was as of.
pub struct Batch {
    transactions: Vec<JournaledTransaction>,
    previous: Previous,
}

/// The object a reader holds open on a follower, which the follower keeps if its writer deletes
/// it, until the last hold of it goes (see the module docs).
pub struct Hold {
    holds: Arc<Mutex<Holds>>,
    object: StoreObject,
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut holds = self.holds.lock();
        let held = holds.objects.get_mut(&self.object).unwrap();
        held.opens -= 1;
        if held.opens == 0 && held.kept.is_none() {
            holds.objects.remove(&self.object);
        }
    }
}

#[derive(Default)]
struct Holds {
    objects: HashMap<StoreObject, Held>,
    /// The extents kept objects refer to, pinned, with how many refer to each.
    pins: HashMap<ExtentId, usize>,
}

#[derive(Default)]
struct Held {
    opens: usize,
    kept: Option<Kept>,
}

/// An object its writer deleted that a reader holds.
#[derive(Default)]
struct Kept {
    /// Its records, as they were before the transaction that deleted it.
    items: Vec<ObjectItem>,
    /// The extents they refer to, pinned.
    extents: BTreeSet<ExtentId>,
    /// Its mutations since, in order, to apply once no reader holds it.
    skipped: Vec<(Mutation, JournalCheckpoint)>,
}

/// A follower's state, in its filesystem.
#[derive(Default)]
pub(crate) struct Follower {
    /// Held while a batch is applied: one at a time.
    following: futures::lock::Mutex<()>,
    holds: Arc<Mutex<Holds>>,
}

impl FxFilesystem {
    /// Holds `object_id` of the store `store_object_id` open, on a follower: if its writer deletes
    /// it, it stays readable as it was until the hold goes. A hold of an object already deleted
    /// keeps nothing.
    pub fn hold(&self, store_object_id: u64, object_id: u64) -> Hold {
        let object = StoreObject::new(store_object_id, object_id);
        let holds = self.follower().holds.clone();
        holds.lock().objects.entry(object).or_default().opens += 1;
        Hold { holds, object }
    }

    /// The writer's commits since those the follower applied, and what they change: advances the
    /// device to the writer's latest commit, and reads the journal's whole transactions since.
    /// Nothing readers read changes until the batch is applied ([`Self::apply`]), the next thing
    /// the follower does: one batch at a time. An error leaves the follower as of no commit: it
    /// remounts.
    pub async fn fetch(&self) -> Result<(Batch, Changes), Error> {
        ensure!(self.options().read_only, "Only a read-only mount follows its writer");
        let _following = self.follower().following.lock().await;
        let previous = self.device().advance().await.context("Advancing the device")?;
        let transactions = self.journal().read_on().await?;
        let mut changes = Changes::default();
        for transaction in &transactions {
            for (store_object_id, mutation) in self.followed_mutations(transaction) {
                changes.add(store_object_id, mutation);
            }
        }
        changes.merge_ranges();
        Ok((Batch { transactions, previous }, changes))
    }

    /// Applies `batch` to the trees readers read, transaction by transaction: readers are kept
    /// out meanwhile (see the module docs). Returns what keeps the commit the follower was as of
    /// (its frees, which readers may still be reading from): its holds' extents are pinned, and
    /// the rest goes when it is dropped, once readers are done with it (TASK-42.5's grace period).
    pub async fn apply(&self, batch: Batch) -> Result<Previous, Error> {
        let _following = self.follower().following.lock().await;
        for extent in self.release_holds()? {
            self.device().unpin(extent).await;
        }
        for transaction in batch.transactions {
            self.apply_followed(transaction).await?;
        }
        Ok(batch.previous)
    }

    /// Follows the writer a batch: fetches it, applies it, and lets go of the commit the follower
    /// was as of at once. Returns what changed. Readers are kept out meanwhile.
    pub async fn follow(&self) -> Result<Changes, Error> {
        let (batch, changes) = self.fetch().await?;
        self.apply(batch).await?;
        Ok(changes)
    }

    /// Whether the follower applies the mutations of `object_id`: all but the allocator's, as it
    /// allocates nothing.
    fn follows(&self, object_id: u64) -> bool {
        object_id != self.object_manager().allocator().object_id()
    }

    /// `transaction`'s mutations the follower applies, by store, in order: the root parent
    /// store's, the root store's, and the others'.
    fn followed_mutations<'a>(
        &self,
        transaction: &'a JournaledTransaction,
    ) -> Vec<(u64, &'a Mutation)> {
        let objects = self.object_manager();
        let (root_parent, root) =
            (objects.root_parent_store_object_id(), objects.root_store_object_id());
        transaction
            .root_parent_mutations
            .iter()
            .map(|mutation| (root_parent, mutation))
            .chain(transaction.root_mutations.iter().map(|mutation| (root, mutation)))
            .chain(transaction.non_root_mutations.iter().map(|(id, mutation)| (*id, mutation)))
            .filter(|(object_id, _)| self.follows(*object_id))
            .collect()
    }

    async fn apply_followed(&self, transaction: JournaledTransaction) -> Result<(), Error> {
        let JournaledTransaction {
            checkpoint,
            root_parent_mutations,
            root_mutations,
            non_root_mutations,
            end_offset,
            ..
        } = transaction;
        let objects = self.object_manager();
        let mutations: Vec<(u64, Mutation)> = root_parent_mutations
            .into_iter()
            .map(|mutation| (objects.root_parent_store_object_id(), mutation))
            .chain(
                root_mutations
                    .into_iter()
                    .map(|mutation| (objects.root_store_object_id(), mutation)),
            )
            .chain(non_root_mutations)
            .filter(|(object_id, _)| self.follows(*object_id))
            .collect();

        // Held objects it deletes are kept as they were, and their mutations skipped.
        self.keep_deleted(&mutations).await?;
        let mut flushed = Vec::new();
        let mut applied = Vec::new();
        {
            let mut holds = self.follower().holds.lock();
            for (store_object_id, mutation) in mutations {
                if let Mutation::ObjectStore(ObjectStoreMutation { item, .. }) = &mutation {
                    let object = StoreObject::new(store_object_id, item.key.object_id);
                    if let Some(Held { kept: Some(kept), .. }) = holds.objects.get_mut(&object) {
                        kept.skipped.push((mutation, checkpoint.clone()));
                        continue;
                    }
                }
                if matches!(mutation, Mutation::EndFlush) {
                    flushed.push(store_object_id);
                }
                applied.push((store_object_id, mutation));
            }
        }

        // In order, the root parent store's first: replay opens the stores the writer made, the
        // root stores being open.
        let context = ApplyContext { mode: ApplyMode::Replay, checkpoint: checkpoint.clone() };
        objects
            .replay_mutations(applied, &Default::default(), &context, end_offset)
            .await
            .context("Failed to follow mutations")?;

        for store_object_id in flushed {
            if !rules().swap_flushed_layers {
                continue;
            }
            let Some(store) = objects.store(store_object_id) else { continue };
            store.take_flushed_layers().await?;
            // The layers the writer flushed hold none of what it deleted that readers hold here.
            let holds = self.follower().holds.lock();
            let kept = holds
                .objects
                .iter()
                .filter(|(object, _)| object.store_object_id == store_object_id)
                .filter_map(|(_, held)| held.kept.as_ref());
            for item in kept.flat_map(|kept| &kept.items) {
                store.tree().replace_or_insert(item.clone());
            }
        }
        Ok(())
    }

    /// Keeps the held objects `mutations` delete, as they are, and pins the extents they refer to.
    async fn keep_deleted(&self, mutations: &[(u64, Mutation)]) -> Result<(), Error> {
        for (store_object_id, mutation) in mutations {
            let Mutation::ObjectStore(ObjectStoreMutation { item, .. }) = mutation else {
                continue;
            };
            let object_id = match &item.key.data {
                ObjectKeyData::GraveyardEntry { object_id }
                    if matches!(item.value, ObjectValue::Some) =>
                {
                    *object_id
                }
                ObjectKeyData::Object if item.is_tombstone() => item.key.object_id,
                _ => continue,
            };
            let object = StoreObject::new(*store_object_id, object_id);
            let Some(store) = self.object_manager().store(*store_object_id) else { continue };
            // Kept from now: a hold that goes meanwhile leaves it to be let go of.
            match self.follower().holds.lock().objects.get_mut(&object) {
                Some(held @ Held { opens: 1.., kept: None }) if rules().keep_held => {
                    held.kept = Some(Kept::default());
                }
                _ => continue,
            }
            let items = object_items(&store, object_id).await?;
            let extents: BTreeSet<ExtentId> = items
                .iter()
                .filter_map(|item| match item.value {
                    ObjectValue::Extent(ExtentValue::Some { extent, .. }) => Some(extent),
                    _ => None,
                })
                .collect();
            debug!(object:?, extents = extents.len(); "Keeping a held object its writer deleted");
            // Counted under the lock, pinned after it: one batch at a time pins and unpins.
            let pinned: Vec<ExtentId> = {
                let mut holds = self.follower().holds.lock();
                let kept = holds.objects.get_mut(&object).unwrap().kept.as_mut().unwrap();
                (kept.items, kept.extents) = (items, extents.clone());
                extents
                    .into_iter()
                    .filter(|extent| {
                        let pins = holds.pins.entry(*extent).or_default();
                        *pins += 1;
                        *pins == 1
                    })
                    .collect()
            };
            for extent in pinned {
                self.device().pin(extent).await.context("Pinning a held object's extent")?;
            }
        }
        Ok(())
    }

    /// Lets go of the kept objects no reader holds: applies what was skipped of them. Returns
    /// their extents to unpin.
    fn release_holds(&self) -> Result<Vec<ExtentId>, Error> {
        let mut unpinned = Vec::new();
        let mut holds = self.follower().holds.lock();
        let released: Vec<StoreObject> = holds
            .objects
            .iter()
            .filter(|(_, held)| held.opens == 0)
            .map(|(object, _)| *object)
            .collect();
        for object in released {
            let kept = holds.objects.remove(&object).unwrap().kept.unwrap();
            if rules().apply_released
                && self.object_manager().store(object.store_object_id).is_some()
            {
                for (mutation, checkpoint) in kept.skipped {
                    let context = ApplyContext { mode: ApplyMode::Replay, checkpoint };
                    self.object_manager()
                        .apply_mutation(object.store_object_id, mutation, &context, AssocObj::None)
                        .context("Failed to apply a released object's mutation")?;
                }
            }
            for extent in kept.extents {
                let pins = holds.pins.get_mut(&extent).unwrap();
                *pins -= 1;
                if *pins == 0 {
                    holds.pins.remove(&extent);
                    unpinned.push(extent);
                }
            }
        }
        Ok(unpinned)
    }
}

/// The records of `object_id` in `store`'s tree.
async fn object_items(store: &ObjectStore, object_id: u64) -> Result<Vec<ObjectItem>, Error> {
    let layer_set = store.tree().layer_set();
    let mut merger = layer_set.merger();
    let mut iter = merger.query(Query::FullRange(&ObjectKey::object(object_id))).await?;
    let mut items = Vec::new();
    while let Some(item @ ItemRef { key, .. }) = iter.get() {
        if key.object_id != object_id {
            break;
        }
        items.push(item.cloned());
        iter.advance().await?;
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::{Changes, StoreObject};
    use crate::object_store::object_record::{
        AttributeId, AttributeKey, ObjectItem, ObjectKey, ObjectKeyData, ObjectValue,
    };
    use crate::object_store::transaction::{Mutation, ObjectStoreMutation, Operation};
    use crate::object_store::{DirType, Extent, ExtentValue};

    fn mutation(key: ObjectKey, value: ObjectValue) -> Mutation {
        Mutation::ObjectStore(ObjectStoreMutation {
            item: ObjectItem::new(key, value),
            op: Operation::Insert,
        })
    }

    fn data(object_id: u64, range: std::ops::Range<u64>) -> Mutation {
        let key = ObjectKey {
            object_id,
            data: ObjectKeyData::Attribute(AttributeId::DATA, AttributeKey::Extent(Extent(range))),
        };
        mutation(key, ObjectValue::Extent(ExtentValue::None))
    }

    fn object(object_id: u64) -> StoreObject {
        StoreObject { store_object_id: 4, object_id }
    }

    /// A batch's changes: data ranges merged where they meet, deletions by tombstone and by
    /// graveyard entry, and volumes deleted.
    #[test]
    fn changes_say_what_a_batch_changed() {
        let mut changes = Changes::default();
        for mutation in [
            data(10, 8192..12288),
            data(10, 0..4096),
            data(10, 4096..6000),
            data(11, 0..4096),
            mutation(ObjectKey::object(12), ObjectValue::None),
            mutation(ObjectKey::graveyard_entry(6, 13), ObjectValue::Some),
            mutation(ObjectKey::graveyard_entry(6, 14), ObjectValue::Trim),
            mutation(ObjectKey::child(7, "a", DirType::Normal), ObjectValue::None),
        ] {
            changes.add(4, &mutation);
        }
        changes.add(5, &Mutation::DeleteVolume);
        changes.merge_ranges();
        assert_eq!(
            changes.data.into_iter().collect::<Vec<_>>(),
            vec![(object(10), vec![0..6000, 8192..12288]), (object(11), vec![0..4096])]
        );
        assert_eq!(changes.deleted.into_iter().collect::<Vec<_>>(), vec![object(12), object(13)]);
        assert_eq!(changes.entries.into_iter().collect::<Vec<_>>(), vec![object(7)]);
        assert_eq!(changes.attributes.into_iter().collect::<Vec<_>>(), vec![]);
        assert_eq!(changes.volumes_deleted.into_iter().collect::<Vec<_>>(), vec![5]);
    }
}
