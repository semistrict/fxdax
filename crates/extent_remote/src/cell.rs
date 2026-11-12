// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: cells (TASK-364). A tenant's cell is its unit of placement: a host at a time holds it,
//! and with it every one of its volumes, each a branch of its own, kept under the cell's prefix
//! ([`Cell::prefix`]). The cell's object ([`layout::cell`]) says who holds it, at what epoch, by
//! which placement's fencing token, and what volumes it has.
//!
//! Taking a cell is one compare-and-swap of its object, to the next epoch, refused to a placement
//! of a lower token than the one holding it ([`Error::Stale`]), as a branch's lease is. The taker
//! then takes each volume's branch's lease with the cell's epoch as its token: a later taking of
//! the cell, of a higher epoch, takes them from it, and its volumes' writers learn of it as any
//! writer taken over does, at their next upload; it can take none back, its epoch lower. So a cell
//! moves whole, fenced by the branches' own leases, which nothing about cells changes.
//!
//! The volumes a cell has change only by its holder, each change conditional on the object being
//! as its holder last took or changed it: another's taking since fails it ([`Error::Fenced`]).

use crate::branch::{Start, Step, transition};
use crate::bucket::Bucket;
use crate::layout::{self, Branch, Cell, FORMAT};
use crate::{Error, bucket, rules};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a volume of a cell is: what its branch holds, which says what opens it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// An Fxfs filesystem.
    Fxfs,
    /// A database of `sqlite_extents`.
    Sqlite,
}

impl Kind {
    /// Its name, as a store's description says it: `fxfs` or `sqlite`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Fxfs => "fxfs",
            Self::Sqlite => "sqlite",
        }
    }

    /// The kind `name` names ([`Kind::name`]).
    pub fn named(name: &str) -> Option<Self> {
        [Self::Fxfs, Self::Sqlite].into_iter().find(|kind| kind.name() == name)
    }
}

/// The longest name a volume has.
pub const MAX_NAME: usize = 64;

/// Whether `name` may name a volume: 1 to [`MAX_NAME`] bytes of ASCII letters, digits, `.`, `_`
/// and `-`, not starting with `.`: a name a host's directories and mounts show as it is.
pub fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && !name.starts_with('.')
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// A volume of a cell: its branch, of the cell's, and what it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    pub branch: u64,
    pub kind: Kind,
}

/// A cell's object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Object {
    /// The format of the cell's objects, its branches' among them ([`FORMAT`]), first, so that a
    /// reader of any format reads it before the rest.
    pub format: u32,
    /// The epoch of its holder's taking, the token its volumes' leases are taken with.
    pub epoch: u64,
    /// Who took it: unique to each taking, so that a write of it is recognised.
    pub holder: u64,
    /// The fencing token of the placement that took it, which no lower one takes it from.
    pub token: u64,
    /// Its volumes, by their names.
    pub volumes: BTreeMap<String, Volume>,
    /// Whether it was deleted: then no one takes it again.
    pub deleted: bool,
}

impl Object {
    fn decode(cell: Cell, bytes: &[u8]) -> Result<Self, Error> {
        let format: u32 = layout::decode(bytes.get(..4).unwrap_or(bytes))?;
        if format != FORMAT {
            let branch = Branch { cell, id: 0 };
            return Err(Error::Format { branch, format });
        }
        layout::decode(bytes)
    }

    /// The branch of its volume `name`, if it has one.
    pub fn branch(&self, cell: Cell, name: &str) -> Option<Branch> {
        self.volumes.get(name).map(|volume| Branch { cell, id: volume.branch })
    }
}

/// A taking of a cell: what changing it, and taking its volumes' leases, goes by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held {
    pub cell: Cell,
    /// The epoch of the taking: the token its volumes' leases are taken with.
    pub epoch: u64,
    pub holder: u64,
}

/// Changes `cell`'s object as `step` says ([`transition`]).
async fn change<R>(
    bucket: &impl Bucket,
    cell: Cell,
    start: Start,
    step: impl FnMut(Option<Object>) -> Result<Step<Object, R>, Error>,
) -> Result<R, Error> {
    let decode = |bytes: &[u8]| Object::decode(cell, bytes);
    transition(bucket, &layout::cell(cell), start, decode, step).await
}

/// Creates `cell`, of no volumes, held by `holder` of the placement of `token` at epoch 1: a
/// create of another's finds its object there.
pub async fn create(
    bucket: &impl Bucket,
    cell: Cell,
    holder: u64,
    token: u64,
) -> Result<Held, Error> {
    let object = Object {
        format: FORMAT,
        epoch: 1,
        holder,
        token,
        volumes: BTreeMap::new(),
        deleted: false,
    };
    change(bucket, cell, Start::Create, |found| match found {
        None => Ok(Step::Write(object.clone(), ())),
        Some(found) if found.holder == holder && found.epoch == 1 => Ok(Step::Stop(())),
        Some(_) => Err(Error::Corrupt(format!("cell {:x}/{:x} exists", cell.tenant, cell.id))),
    })
    .await?;
    Ok(Held { cell, epoch: 1, holder })
}

/// Takes `cell` for `holder`, of the placement of `token`, at the epoch after its last taking's:
/// refused if it has no object ([`Error::NoCell`]), if it was deleted ([`Error::Deleted`]), or if
/// a placement of a higher token holds it ([`Error::Stale`]).
pub async fn take(
    bucket: &impl Bucket,
    cell: Cell,
    holder: u64,
    token: u64,
) -> Result<Held, Error> {
    let epoch = change(bucket, cell, Start::Read, |found| match found {
        None => Err(Error::NoCell),
        Some(found) if found.deleted => Err(Error::Deleted),
        Some(found) if rules().tokens && found.token > token => Err(Error::Stale),
        Some(found) if found.holder == holder => Ok(Step::Stop(found.epoch)),
        Some(found) => {
            let epoch = found.epoch + 1;
            Ok(Step::Write(Object { epoch, holder, token, ..found }, epoch))
        }
    })
    .await?;
    Ok(Held { cell, epoch, holder })
}

/// `cell`'s object, if it has one.
pub async fn read(bucket: &impl Bucket, cell: Cell) -> Result<Option<Object>, Error> {
    match bucket::get(bucket, &layout::cell(cell)).await {
        Some(object) => Ok(Some(Object::decode(cell, &object.bytes)?)),
        None => Ok(None),
    }
}

/// Changes the object of the cell `held` holds as `step` says, if `held` holds it still:
/// [`Error::Fenced`] if another took it since, [`Error::Deleted`] if it was deleted.
async fn change_held<R>(
    bucket: &impl Bucket,
    held: Held,
    mut step: impl FnMut(Object) -> Result<Step<Object, R>, Error>,
) -> Result<R, Error> {
    change(bucket, held.cell, Start::Read, |found| match found {
        None => Err(Error::NoCell),
        Some(found) if found.deleted => Err(Error::Deleted),
        Some(found) if (found.epoch, found.holder) != (held.epoch, held.holder) => {
            Err(Error::Fenced)
        }
        Some(found) => step(found),
    })
    .await
}

/// Holds `cell` for `holder`, of the placement of `token`, in one compare-and-swap of its object:
/// takes it as [`take`] does; or, if it has no object and `make` says so, makes it, held at epoch 1;
/// and, given `volume`, has it hold that volume, under that name, unless it names the volume's
/// branch already, under whatever name it gave it: refused, as [`add_volume`] refuses, if another
/// branch has the name, or the name names no volume. A volume's name is its branch's from its
/// first: a store of the branch taken into a directory of another name names it no other way.
/// Returns the taking, and the object as it left it.
pub async fn hold(
    bucket: &impl Bucket,
    cell: Cell,
    (holder, token): (u64, u64),
    make: bool,
    volume: Option<(&str, Volume)>,
) -> Result<(Held, Object), Error> {
    if let Some((name, _)) = volume
        && !valid_name(name)
    {
        return Err(Error::VolumeName(name.into()));
    }
    change(bucket, cell, Start::Read, |found| {
        let (mut object, mut changed) = match found {
            None if !make => return Err(Error::NoCell),
            None => {
                let volumes = BTreeMap::new();
                let made =
                    Object { format: FORMAT, epoch: 1, holder, token, volumes, deleted: false };
                (made, true)
            }
            Some(found) if found.deleted => return Err(Error::Deleted),
            Some(found) if rules().tokens && found.token > token => return Err(Error::Stale),
            Some(found) if found.holder == holder => (found, false),
            Some(found) => (Object { epoch: found.epoch + 1, holder, token, ..found }, true),
        };
        if let Some((name, volume)) = volume
            && !object.volumes.values().any(|had| had.branch == volume.branch)
        {
            if object.volumes.contains_key(name) {
                return Err(Error::VolumeExists(name.into()));
            }
            object.volumes.insert(name.into(), volume);
            changed = true;
        }
        let held = Held { cell, epoch: object.epoch, holder };
        Ok(match changed {
            true => Step::Write(object.clone(), (held, object)),
            false => Step::Stop((held, object)),
        })
    })
    .await
}

/// Adds to the cell `held` holds the volume `name`, `volume`: [`Error::VolumeExists`] if it has
/// one of that name, of another branch; [`Error::VolumeName`] if `name` names no volume
/// ([`valid_name`]).
pub async fn add_volume(
    bucket: &impl Bucket,
    held: Held,
    name: &str,
    volume: Volume,
) -> Result<(), Error> {
    if !valid_name(name) {
        return Err(Error::VolumeName(name.into()));
    }
    change_held(bucket, held, |mut found| match found.volumes.get(name) {
        Some(&had) if had == volume => Ok(Step::Stop(())),
        Some(_) => Err(Error::VolumeExists(name.into())),
        None => {
            found.volumes.insert(name.into(), volume);
            Ok(Step::Write(found, ()))
        }
    })
    .await
}

/// Removes from the cell `held` holds its volume `name`, returning it: [`Error::NoVolume`] if it
/// has none of that name. Its branch stays, to be deleted.
pub async fn remove_volume(bucket: &impl Bucket, held: Held, name: &str) -> Result<Volume, Error> {
    change_held(bucket, held, |mut found| match found.volumes.remove(name) {
        Some(volume) => Ok(Step::Write(found, volume)),
        None => Err(Error::NoVolume(name.into())),
    })
    .await
}

/// Takes out of `cell` the volume naming `branch`, deleted, if it names it: what any placement
/// deleting the branch does, whoever holds the cell, as a deleted branch's name holds nothing, and
/// changing the cell's volumes so changes nothing of who holds it, nor its epoch.
pub async fn forget(bucket: &impl Bucket, cell: Cell, branch: u64) -> Result<(), Error> {
    change(bucket, cell, Start::Read, |found| match found {
        Some(mut found) if found.volumes.values().any(|volume| volume.branch == branch) => {
            found.volumes.retain(|_, volume| volume.branch != branch);
            Ok(Step::Write(found, ()))
        }
        _ => Ok(Step::Stop(())),
    })
    .await
}

/// Marks the cell `held` holds deleted, so that no one takes it again, returning its volumes, whose
/// branches stay, to be deleted.
pub async fn delete(bucket: &impl Bucket, held: Held) -> Result<BTreeMap<String, Volume>, Error> {
    change_held(bucket, held, |found| {
        let volumes = found.volumes.clone();
        Ok(Step::Write(Object { deleted: true, ..found }, volumes))
    })
    .await
}

/// Whether `held` holds its cell still.
pub async fn holds(bucket: &impl Bucket, held: Held) -> Result<bool, Error> {
    Ok(read(bucket, held.cell).await?.is_some_and(|found| {
        !found.deleted && (found.epoch, found.holder) == (held.epoch, held.holder)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TEST_CELL;
    use crate::testing::{Direct, SimBucket};
    use futures::executor::block_on;

    const FXFS: Volume = Volume { branch: 7, kind: Kind::Fxfs };
    const SQLITE: Volume = Volume { branch: 8, kind: Kind::Sqlite };

    /// A cell made is held by its maker at epoch 1, of no volumes; made again by its maker, whose
    /// answer was lost, it stands; by another, it exists. One that was never made has no object,
    /// and no one takes it.
    #[test]
    fn cells_are_made_once() {
        let bucket = Direct(SimBucket::new());
        let held = block_on(create(&bucket, TEST_CELL, 0x11, 5)).unwrap();
        assert_eq!(held, Held { cell: TEST_CELL, epoch: 1, holder: 0x11 });
        assert_eq!(block_on(create(&bucket, TEST_CELL, 0x11, 5)), Ok(held));
        let exists = Error::Corrupt("cell 1/1 exists".into());
        assert_eq!(block_on(create(&bucket, TEST_CELL, 0x12, 5)), Err(exists));
        let object = block_on(read(&bucket, TEST_CELL)).unwrap().unwrap();
        let expected = Object {
            format: FORMAT,
            epoch: 1,
            holder: 0x11,
            token: 5,
            volumes: BTreeMap::new(),
            deleted: false,
        };
        assert_eq!(object, expected);
        let never = Cell { tenant: 1, id: 2 };
        assert_eq!(block_on(read(&bucket, never)), Ok(None));
        assert_eq!(block_on(take(&bucket, never, 0x13, 5)), Err(Error::NoCell));
    }

    /// A cell taken goes to the next epoch, its taker's; a placement of a lower token than the one
    /// holding it takes it not. Its holder before holds it no more, and changes none of its volumes.
    #[test]
    fn takings_fence_the_holder_before() {
        let bucket = Direct(SimBucket::new());
        let first = block_on(create(&bucket, TEST_CELL, 0x11, 5)).unwrap();
        block_on(add_volume(&bucket, first, "root", FXFS)).unwrap();
        let second = block_on(take(&bucket, TEST_CELL, 0x12, 5)).unwrap();
        assert_eq!(second, Held { cell: TEST_CELL, epoch: 2, holder: 0x12 });
        assert_eq!(block_on(take(&bucket, TEST_CELL, 0x13, 4)), Err(Error::Stale));
        assert_eq!(block_on(holds(&bucket, first)), Ok(false));
        assert_eq!(block_on(holds(&bucket, second)), Ok(true));
        assert_eq!(block_on(add_volume(&bucket, first, "db", SQLITE)), Err(Error::Fenced));
        assert_eq!(block_on(remove_volume(&bucket, first, "root")), Err(Error::Fenced));
        assert_eq!(block_on(delete(&bucket, first)), Err(Error::Fenced));
        // Taken again by its taker, whose answer was lost, it stays at its epoch.
        assert_eq!(block_on(take(&bucket, TEST_CELL, 0x12, 5)), Ok(second));
        let third = block_on(take(&bucket, TEST_CELL, 0x14, 6)).unwrap();
        assert_eq!(third.epoch, 3);
        assert_eq!(block_on(take(&bucket, TEST_CELL, 0x15, 5)), Err(Error::Stale));
        let object = block_on(read(&bucket, TEST_CELL)).unwrap().unwrap();
        assert_eq!((object.epoch, object.holder, object.token), (3, 0x14, 6));
        assert_eq!(object.volumes, BTreeMap::from([("root".to_owned(), FXFS)]));
    }

    /// Its holder adds volumes by name, each once: a name taken by another branch is refused; and
    /// removes them, a name it has none of refused. A cell deleted keeps its volumes' names, which
    /// its deletion returns, and no one takes it or changes it again.
    #[test]
    fn holders_add_and_remove_volumes() {
        let bucket = Direct(SimBucket::new());
        let held = block_on(create(&bucket, TEST_CELL, 0x11, 5)).unwrap();
        block_on(add_volume(&bucket, held, "root", FXFS)).unwrap();
        block_on(add_volume(&bucket, held, "db", SQLITE)).unwrap();
        assert_eq!(block_on(add_volume(&bucket, held, "db", SQLITE)), Ok(()));
        let taken = block_on(add_volume(&bucket, held, "db", FXFS));
        assert_eq!(taken, Err(Error::VolumeExists("db".into())));
        let object = block_on(read(&bucket, TEST_CELL)).unwrap().unwrap();
        assert_eq!(object.branch(TEST_CELL, "db"), Some(Branch { cell: TEST_CELL, id: 8 }));
        assert_eq!(object.branch(TEST_CELL, "logs"), None);
        assert_eq!(block_on(remove_volume(&bucket, held, "db")), Ok(SQLITE));
        let none = block_on(remove_volume(&bucket, held, "db"));
        assert_eq!(none, Err(Error::NoVolume("db".into())));
        let volumes = block_on(delete(&bucket, held)).unwrap();
        assert_eq!(volumes, BTreeMap::from([("root".to_owned(), FXFS)]));
        assert_eq!(block_on(holds(&bucket, held)), Ok(false));
        assert_eq!(block_on(take(&bucket, TEST_CELL, 0x12, 9)), Err(Error::Deleted));
        let added = block_on(add_volume(&bucket, held, "db", SQLITE));
        assert_eq!(added, Err(Error::Deleted));
    }

    /// Holding a cell takes it, or makes it if it has no object and its holder may: a holder
    /// holding it already goes on at its epoch, another placement takes it at the next; one that
    /// may not make it finds none.
    #[test]
    fn holding_takes_or_makes() {
        let bucket = Direct(SimBucket::new());
        assert_eq!(
            block_on(hold(&bucket, TEST_CELL, (5, 5), false, None)).map(|(held, _)| held),
            Err(Error::NoCell)
        );
        let (made, object) = block_on(hold(&bucket, TEST_CELL, (5, 5), true, None)).unwrap();
        assert_eq!((made.epoch, object.epoch, object.holder), (1, 1, 5));
        let (again, _) = block_on(hold(&bucket, TEST_CELL, (5, 5), true, None)).unwrap();
        assert_eq!(again, made);
        block_on(add_volume(&bucket, made, "root", FXFS)).unwrap();
        let (taken, object) = block_on(hold(&bucket, TEST_CELL, (6, 6), true, None)).unwrap();
        assert_eq!((taken.epoch, object.holder), (2, 6));
        assert_eq!(object.volumes, BTreeMap::from([("root".to_owned(), FXFS)]));
    }

    /// Holding a cell with a volume makes it hold the volume, in the one write of its taking, or
    /// none; a volume whose branch the cell names already, under whatever name, is left as named:
    /// its name is its branch's. A name another branch has is refused, writing nothing.
    #[test]
    fn holding_with_a_volume_names_its_branch_once() {
        let bucket = Direct(SimBucket::new());
        let made = block_on(hold(&bucket, TEST_CELL, (5, 5), true, Some(("root", FXFS))));
        let (made, object) = made.unwrap();
        assert_eq!((made.epoch, object.volumes.len()), (1, 1));
        let again = block_on(hold(&bucket, TEST_CELL, (5, 5), true, Some(("elsewhere", FXFS))));
        let (_, object) = again.unwrap();
        assert_eq!(object.volumes, BTreeMap::from([("root".to_owned(), FXFS)]));
        let taken = block_on(hold(&bucket, TEST_CELL, (6, 6), true, Some(("db", SQLITE))));
        let (taken, object) = taken.unwrap();
        let both = BTreeMap::from([("db".to_owned(), SQLITE), ("root".to_owned(), FXFS)]);
        assert_eq!((taken.epoch, &object.volumes), (2, &both));
        let other = Volume { branch: 9, kind: Kind::Fxfs };
        let refused = block_on(hold(&bucket, TEST_CELL, (7, 7), true, Some(("db", other))));
        assert_eq!(refused.map(|(held, _)| held), Err(Error::VolumeExists("db".into())));
        let object = block_on(read(&bucket, TEST_CELL)).unwrap().unwrap();
        assert_eq!((object.epoch, object.holder, object.volumes), (2, 6, both));
        let bad = block_on(hold(&bucket, TEST_CELL, (7, 7), true, Some(("a/b", other))));
        assert_eq!(bad.map(|(held, _)| held), Err(Error::VolumeName("a/b".into())));
    }

    /// Volumes are named as a host's directories show them: letters, digits, `.`, `_` and `-`, up
    /// to 64 of them, not starting with `.`; any other name is refused. Kinds go by their names.
    #[test]
    fn volumes_are_named_plainly() {
        for name in ["root", "db.sqlite", "a-b_c", "0", &"x".repeat(MAX_NAME)] {
            assert!(valid_name(name), "{name}");
        }
        for name in ["", ".hidden", "a/b", "a b", "é", &"x".repeat(MAX_NAME + 1)] {
            assert!(!valid_name(name), "{name}");
        }
        let bucket = Direct(SimBucket::new());
        let held = block_on(create(&bucket, TEST_CELL, 0x11, 5)).unwrap();
        let refused = block_on(add_volume(&bucket, held, "a/b", FXFS));
        assert_eq!(refused, Err(Error::VolumeName("a/b".into())));
        for kind in [Kind::Fxfs, Kind::Sqlite] {
            assert_eq!(Kind::named(kind.name()), Some(kind));
        }
        assert_eq!(Kind::named("ext4"), None);
    }

    /// A cell's object of another format is refused, naming it, before the rest of it is read.
    #[test]
    fn cells_of_other_formats_are_refused() {
        let bucket = Direct(SimBucket::new());
        let object = Object {
            format: FORMAT + 1,
            epoch: 1,
            holder: 0x11,
            token: 5,
            volumes: BTreeMap::new(),
            deleted: false,
        };
        block_on(bucket::create(&bucket, &layout::cell(TEST_CELL), layout::encode(&object)));
        let refused =
            Error::Format { branch: Branch { cell: TEST_CELL, id: 0 }, format: FORMAT + 1 };
        assert_eq!(block_on(read(&bucket, TEST_CELL)), Err(refused.clone()));
        assert_eq!(block_on(take(&bucket, TEST_CELL, 0x12, 5)), Err(refused));
    }
}
