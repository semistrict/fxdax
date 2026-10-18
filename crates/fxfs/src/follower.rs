// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: a read-only mount follows its writer (backlog doc-8, The follower in Fxfs;
//! TASK-42.4).
//!
//! A follower is a read-only mount on a device that reads a store another mount writes, as of one
//! of its commits (`Device::advance`). [`FxFilesystem::follow`] advances the device to the
//! writer's latest commit, reads the journal on from the end of the last whole transaction it
//! applied, and applies each whole transaction as replay applies them, to the live trees: the
//! root parent store's mutations, the root store's, then the other stores', opening stores the
//! writer created. A flush's last transaction (EndFlush) swaps the store's sealed layers for those
//! the flush wrote, as the writer's own commit of it does; the allocator's mutations are left, as a
//! follower allocates nothing. Each call returns what its batch changed ([`Changes`]), for the
//! caller to invalidate what it cached.
//!
//! The follower's graveyard: a reader that has an object open holds it ([`FxFilesystem::hold`]).
//! When the writer deletes an object a reader holds (its graveyard entry, or its tombstone), the
//! follower keeps the object's records as they were before, and pins the extents they refer to
//! (`Device::pin`), which the writer frees: it skips the object's later mutations, and puts its
//! records back after a flush swaps in layers without them. Once the last hold goes, the next call
//! applies what it skipped, and unpins the extents.
//!
//! Callers keep readers out of the filesystem while `follow` applies (TASK-42.5 lifts this).

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
use storage_device::ExtentId;

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

    /// Follows the writer: advances the device to its latest commit, and applies the journal's
    /// whole transactions since those applied, returning what they changed. Readers are kept out
    /// meanwhile (see the module docs). An error leaves the follower as of no commit: it remounts.
    pub async fn follow(&self) -> Result<Changes, Error> {
        ensure!(self.options().read_only, "Only a read-only mount follows its writer");
        let _following = self.follower().following.lock().await;
        // What the previous commit held is kept until its frees that holds keep are pinned.
        let previous = self.device().advance().await.context("Advancing the device")?;
        let transactions = self.journal().read_on().await?;
        self.release_holds()?;
        let mut changes = Changes::default();
        for transaction in transactions {
            self.apply_followed(transaction, &mut changes).await?;
        }
        changes.merge_ranges();
        std::mem::drop(previous);
        Ok(changes)
    }

    /// The store `store_object_id`, the root stores' included.
    fn followed_store(&self, store_object_id: u64) -> Option<Arc<ObjectStore>> {
        let objects = self.object_manager();
        if store_object_id == objects.root_parent_store_object_id() {
            Some(objects.root_parent_store())
        } else if store_object_id == objects.root_store_object_id() {
            Some(objects.root_store())
        } else {
            objects.store(store_object_id)
        }
    }

    async fn apply_followed(
        &self,
        transaction: JournaledTransaction,
        changes: &mut Changes,
    ) -> Result<(), Error> {
        let JournaledTransaction {
            checkpoint,
            root_parent_mutations,
            root_mutations,
            non_root_mutations,
            end_offset,
            ..
        } = transaction;
        let objects = self.object_manager();
        let root_parent_store_object_id = objects.root_parent_store_object_id();
        let root_store_object_id = objects.root_store_object_id();
        let allocator_object_id = objects.allocator().object_id();
        let mutations: Vec<(u64, Mutation)> = root_parent_mutations
            .into_iter()
            .map(|mutation| (root_parent_store_object_id, mutation))
            .chain(root_mutations.into_iter().map(|mutation| (root_store_object_id, mutation)))
            .chain(non_root_mutations)
            .filter(|(object_id, _)| *object_id != allocator_object_id)
            .collect();

        // Held objects it deletes are kept as they were, and their mutations skipped.
        self.keep_deleted(&mutations).await?;
        let mut flushed = Vec::new();
        let mut system = Vec::new();
        let mut others = Vec::new();
        {
            let mut holds = self.follower().holds.lock();
            for (store_object_id, mutation) in mutations {
                changes.add(store_object_id, &mutation);
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
                if store_object_id == root_parent_store_object_id
                    || store_object_id == root_store_object_id
                {
                    system.push((store_object_id, mutation));
                } else {
                    others.push((store_object_id, mutation));
                }
            }
        }

        let context = ApplyContext { mode: ApplyMode::Replay, checkpoint: checkpoint.clone() };
        for (store_object_id, mutation) in system {
            objects
                .apply_mutation(store_object_id, mutation, &context, AssocObj::None)
                .context("Failed to follow a root store's mutation")?;
        }
        objects
            .replay_mutations(others, &Default::default(), &context, end_offset)
            .await
            .context("Failed to follow mutations")?;

        for store_object_id in flushed {
            if !rules().swap_flushed_layers {
                continue;
            }
            let Some(store) = self.followed_store(store_object_id) else { continue };
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
            if !rules().keep_held || !self.holds_to_keep(&object) {
                continue;
            }
            let Some(store) = self.followed_store(*store_object_id) else { continue };
            let items = object_items(&store, object_id).await?;
            let extents: BTreeSet<ExtentId> = items
                .iter()
                .filter_map(|item| match item.value {
                    ObjectValue::Extent(ExtentValue::Some { extent, .. }) => Some(extent),
                    _ => None,
                })
                .collect();
            // Its last hold may have gone meanwhile.
            let mut holds = self.follower().holds.lock();
            let Some(held @ Held { opens: 1.., kept: None }) = holds.objects.get_mut(&object)
            else {
                continue;
            };
            debug!(object:?, extents = extents.len(); "Keeping a held object its writer deleted");
            held.kept = Some(Kept { items, extents: extents.clone(), skipped: Vec::new() });
            for extent in extents {
                let pins = holds.pins.entry(extent).or_default();
                if *pins == 0 {
                    self.device().pin(extent).context("Pinning a held object's extent")?;
                }
                *pins += 1;
            }
        }
        Ok(())
    }

    /// Whether `object` is held, and not kept yet.
    fn holds_to_keep(&self, object: &StoreObject) -> bool {
        matches!(
            self.follower().holds.lock().objects.get(object),
            Some(Held { opens: 1.., kept: None })
        )
    }

    /// Lets go of the kept objects no reader holds: applies what was skipped of them, and unpins
    /// their extents.
    fn release_holds(&self) -> Result<(), Error> {
        let mut holds = self.follower().holds.lock();
        let released: Vec<StoreObject> = holds
            .objects
            .iter()
            .filter(|(_, held)| held.opens == 0)
            .map(|(object, _)| *object)
            .collect();
        for object in released {
            let kept = holds.objects.remove(&object).unwrap().kept.unwrap();
            if rules().apply_released && self.followed_store(object.store_object_id).is_some() {
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
                    self.device().unpin(extent);
                }
            }
        }
        Ok(())
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
