// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Where branches keep what, in a bucket, and what the objects hold (doc-7, Layout).
//!
//! Numbers in keys are fixed-width hex, so keys list in numeric order. Objects are read from a
//! bucket, by ranged reads, as [`crate::object`] reads them.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use storage_device::ExtentId;

pub use extent_frame::segment::{
    Branch, Cell, Chain, Child, Contents, Continued, Footer, Location, epoch_of, tag, writer_of,
};

/// fxdax: the format of a branch's objects, which its record says first
/// ([`Record::format`](crate::record::Record::format)): a reader refuses a branch of any other,
/// newer or older, naming it, and reads none of it (doc-11, rule 1; TASK-141.2). 2: a footer holds
/// its segment's [`Contents`], with the capacity its commits set, and a checkpoint the branch's
/// capacity (TASK-165.3); nothing reads 1's, as nothing keeps any (AGENTS.md). 3: a commit may
/// span segments, a footer naming the runs in its data of the commit it goes on with
/// (`Contents::continued`, TASK-92.41); nothing reads 2's. 4: a checkpoint names the branch's
/// fences, which the collector keeps ([`State::fences`](crate::recovery::State::fences),
/// TASK-371); nothing reads 3's. 5: a branch's objects are under its tenant's cell's prefix, and
/// footers, forks and origins name branches with their cells ([`Branch`], TASK-364); nothing reads
/// 4's. 6: a checkpoint's fences count the branch's strays (TASK-383); nothing reads 5's, whose
/// fences lack them. 7: a checkpoint names the branch's strays apart from its fences, each void or
/// late ([`Stray`](crate::recovery::Stray), TASK-388); nothing reads 6's. 8: a point's record says
/// whether other tenants may fork it ([`Record::shared`](crate::record::Record::shared),
/// TASK-395.7); nothing reads 7's. 9: everything a cell's collection reads is under the cell's
/// prefix: its objects' moves ([`moves`]), its collector's lease and marks ([`gc_lease`],
/// [`marks`]), and the references of forks of its branches ([`reference`]), so that a pass of the
/// collector lists the cell alone (TASK-395.2); nothing reads 8's, whose moves and collector's lease
/// were at the bucket's root.
pub const FORMAT: u32 = 9;

/// fxdax: the prefix of every tenant's cells' objects: all the bucket's branches (TASK-364).
pub const TENANTS: &str = "tenants/";
/// fxdax: under a cell's prefix, the collector's own: its lease, its marks, and what else its
/// passes over the cell keep there; never a pass's candidate (TASK-395.2).
const GC: &str = "gc/";
const GC_LEASE: &str = "gc/lease";
const GC_MARKS: &str = "gc/marks/";
/// Under a cell's prefix, the moves of its objects, and the moves of those, `moves/moves/...`.
const MOVES: &str = "moves/";
/// fxdax: under a cell's prefix, the references of forks of its branches ([`reference`]).
const REFS: &str = "refs/";
/// fxdax: the prefix of the blocks of object IDs taken ([`objectid_block`]).
const OBJECTIDS: &str = "objectid/";
/// fxdax: there while a mark-only pass marks the bucket, which it writes and an operator removes
/// with the marks: the one thing readers read to know whether to watch any cell's marks
/// ([`crate::marks`], TASK-395.4's review). No cell's, and no candidate of any pass.
pub const MARKING: &str = "gc/marking";

pub fn segments(branch: Branch) -> String {
    format!("{}segments/{:016x}/", branch.cell.prefix(), branch.id)
}

/// Segment `seq` of `branch`: `seq` counts from 0 without gaps.
pub fn segment(branch: Branch, seq: u64) -> String {
    format!("{}{seq:016x}", segments(branch))
}

pub fn extents(branch: Branch) -> String {
    format!("{}extents/{:016x}/", branch.cell.prefix(), branch.id)
}

/// The object of an extent of `cell`'s stored on its own, uploaded by the writer of epoch `epoch`
/// before its segment `seq`, which names it.
pub fn extent(cell: Cell, epoch: u64, seq: u64, extent: ExtentId) -> String {
    extent_frame::segment::extent_key(cell, epoch, seq, extent)
}

pub fn checkpoints(branch: Branch) -> String {
    format!("{}checkpoints/{:016x}/", branch.cell.prefix(), branch.id)
}

/// The checkpoint of `branch` as of its segment `seq`.
pub fn checkpoint(branch: Branch, seq: u64) -> String {
    format!("{}{seq:016x}", checkpoints(branch))
}

/// fxdax: the prefix of the records of `cell`'s branches.
pub fn records(cell: Cell) -> String {
    format!("{}branches/", cell.prefix())
}

pub fn record(branch: Branch) -> String {
    format!("{}{:016x}", records(branch.cell), branch.id)
}

pub fn lease(branch: Branch) -> String {
    format!("{}leases/{:016x}", branch.cell.prefix(), branch.id)
}

/// fxdax: the block of object IDs `block` taken, by one allocator of one process's
/// ([`crate::objectid`], TASK-364): there once taken, and kept for good, so that no other takes
/// it, and none of its IDs is given twice.
pub fn objectid_block(block: u64) -> String {
    format!("{OBJECTIDS}{block:016x}")
}

/// fxdax: the prefix of `tenant`'s cells: listed by delimiter, its cells' prefixes
/// ([`crate::cell::of_tenant`]).
pub fn cells_of(tenant: u64) -> String {
    format!("{TENANTS}{tenant:016x}/cells/")
}

/// fxdax: the tenant a prefix of a delimiter listing of [`TENANTS`] names, `tenants/<tenant>/`.
pub fn tenant_of_prefix(prefix: &str) -> Option<u64> {
    hex(prefix.strip_prefix(TENANTS)?.strip_suffix('/')?)
}

/// fxdax: the object of `cell` itself: who holds it, and its volumes ([`crate::cell`]).
pub fn cell(cell: Cell) -> String {
    format!("{}cell", cell.prefix())
}

/// fxdax: the cell whose prefix `key` is under, and the rest of it: none for a key of no cell's.
fn in_cell(key: &str) -> Option<(Cell, &str)> {
    let (tenant, rest) = key.strip_prefix(TENANTS)?.split_once('/')?;
    let (id, rest) = rest.strip_prefix("cells/")?.split_once('/')?;
    Some((Cell { tenant: hex(tenant)?, id: hex(id)? }, rest))
}

/// fxdax: the cell whose prefix `key` is under, a key or a prefix: none for one of no cell's.
pub fn cell_of(key: &str) -> Option<Cell> {
    in_cell(key).map(|(cell, _)| cell)
}

/// fxdax: the cell whose whole prefix `prefix` is, as a listing of every object of the cell's
/// names it: none for any other prefix.
pub fn cell_of_prefix(prefix: &str) -> Option<Cell> {
    in_cell(prefix).filter(|(_, rest)| rest.is_empty()).map(|(cell, _)| cell)
}

/// What the collector made of the object at `key` when it compacted it: the extents of it that were
/// live, and where in it each is. Being one object, written whole, it is never there without them.
/// fxdax: under the cell of the object, `<C>moves/<rest>` for the object `<C><rest>`, and so
/// `<C>moves/moves/<rest>` for its moves' moves, so that the cell's pass of the collector lists
/// them with the cell (TASK-395.2). Only a cell's objects are compacted: a key of no cell's, which
/// a location read from a checkpoint or a footer may name if they are corrupt, has none
/// ([`crate::Error::Corrupt`]).
pub fn moves(key: &str) -> Result<String, crate::Error> {
    let Some((cell, rest)) = in_cell(key) else {
        return Err(crate::Error::Corrupt(format!("{key}: no cell's object, which has no moves")));
    };
    Ok(format!("{}{MOVES}{rest}", cell.prefix()))
}

/// fxdax: the key of the object the moves at `key` are of, if they are moves: the inverse of
/// [`moves`].
pub fn moved_from(key: &str) -> Option<String> {
    match kind(key) {
        Kind::Moves { cell, of } => Some(format!("{}{of}", cell.prefix())),
        _ => None,
    }
}

/// fxdax: the collector's lease over `cell`, which its passes over the cell take (doc-7, Garbage
/// collection; TASK-395.2), a [`crate::passed::GcLease`]: passes over other cells run beside them,
/// unfenced.
pub fn gc_lease(cell: Cell) -> String {
    format!("{}{GC_LEASE}", cell.prefix())
}

/// fxdax: the prefix of the collector's own objects under `cell`: its lease, which holds what its
/// passes record ([`crate::passed`]), and its marks; never a pass's candidate, nor what a pass
/// decides by but its lease.
pub fn collectors(cell: Cell) -> String {
    format!("{}{GC}", cell.prefix())
}

/// fxdax: the prefix of the marks of the mark-only passes over `cell` ([`marks`]).
pub fn marks_of(cell: Cell) -> String {
    format!("{}{GC_MARKS}", cell.prefix())
}

/// fxdax: what the mark-only pass over `cell` of the collector that took the cell's lease at
/// `epoch` found no live branch needs, and would have deleted ([`Marks`]; doc-7, Garbage collection;
/// TASK-264): keyed by the epoch, so that the newest lists last.
pub fn marks(cell: Cell, epoch: u64) -> String {
    format!("{}{epoch:016x}", marks_of(cell))
}

/// fxdax: the prefix of the references of forks of `cell`'s branches ([`reference`]): listed by
/// delimiter, the branches of the cell with forks referring to them.
pub fn references(cell: Cell) -> String {
    format!("{}{REFS}", cell.prefix())
}

/// fxdax: the prefix of the references of forks of `parent`: listed by delimiter, its distinct fork
/// points, which a pass over its cell keeps the states of ([`fork_point`]).
pub fn references_of(parent: Branch) -> String {
    format!("{}{:016x}/", references(parent.cell), parent.id)
}

/// fxdax: the prefix of the references of the forks of `parent` at the end of its segment `at`:
/// listed, their children.
pub fn fork_point(parent: Branch, at: u64) -> String {
    format!("{}{at:016x}/", references_of(parent))
}

/// fxdax: the reference of the fork of `parent` at the end of its segment `at` to `child`, under
/// the parent's cell, the cell holding what the child inherits, whatever the child's (doc-7,
/// References; TASK-395.2): while it stands, the parent's cell's passes keep the parent's whole
/// state at the fork point. Its bytes are a [`Reference`], unique to the fork, so that every writer
/// of it writes the same; the child's cell's passes retire it, computing it from the child's
/// record's origin, never listing it.
pub fn reference(parent: Branch, at: u64, child: Branch) -> String {
    let Branch { cell: Cell { tenant, id: cell }, id } = child;
    format!("{}{tenant:016x}.{cell:016x}.{id:016x}", fork_point(parent, at))
}

/// fxdax: the parent a prefix of `cell`'s references names, `<C>refs/<parent>/`, as a delimiter
/// listing of [`references`] gives it.
pub fn referenced_parent(cell: Cell, prefix: &str) -> Option<Branch> {
    let rest = prefix.strip_prefix(&references(cell))?.strip_suffix('/')?;
    Some(Branch { cell, id: hex(rest)? })
}

/// fxdax: the fork point a prefix of `parent`'s references names, `<C>refs/<parent>/<at>/`, as a
/// delimiter listing of [`references_of`] gives it.
pub fn referenced_at(parent: Branch, prefix: &str) -> Option<u64> {
    hex(prefix.strip_prefix(&references_of(parent))?.strip_suffix('/')?)
}

/// Whether the object at `later` is further along the way of an extent from the object at `key`:
/// the moves of it, or of moves of it.
pub fn further(key: &str, later: &str) -> bool {
    let (Some((cell, of)), Some((at_cell, mut at))) = (in_cell(key), in_cell(later)) else {
        return false;
    };
    if cell != at_cell {
        return false;
    }
    while let Some(from) = at.strip_prefix(MOVES) {
        if from == of {
            return true;
        }
        at = from;
    }
    false
}

/// What kind of object a key names, and what its key says. fxdax: an extent's says the segment
/// that names it, `seq`, of the writer of `epoch`, which uploaded it before: what it may yet be
/// named by, as its writer declares it (TASK-79.3, TASK-153.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind<'a> {
    Segment {
        branch: Branch,
        seq: u64,
    },
    Extent {
        branch: Branch,
        epoch: u64,
        seq: u64,
        sequence: u64,
    },
    Checkpoint {
        branch: Branch,
        seq: u64,
    },
    Record {
        branch: Branch,
    },
    Lease {
        branch: Branch,
    },
    /// fxdax: a cell's own object ([`cell`]).
    Cell {
        cell: Cell,
    },
    /// fxdax: a block of object IDs taken ([`objectid_block`]).
    ObjectIds {
        block: u64,
    },
    /// fxdax: the collector's lease over a cell ([`gc_lease`]).
    GcLease {
        cell: Cell,
    },
    /// fxdax: a mark-only pass's word that it marks the bucket ([`MARKING`]).
    Marking,
    /// fxdax: a mark-only pass's marks over a cell ([`marks`]): the collector's own, never a
    /// candidate.
    Marks {
        cell: Cell,
        epoch: u64,
    },
    /// The moves of the object `of`, the rest of its key under the cell's prefix ([`moves`],
    /// [`moved_from`]).
    Moves {
        cell: Cell,
        of: &'a str,
    },
    /// fxdax: the reference of the fork of `parent`, of the cell it is under, at the end of its
    /// segment `at`, to `child` ([`reference`]): never a candidate.
    Reference {
        parent: Branch,
        at: u64,
        child: Branch,
    },
    Unknown,
}

impl Kind<'_> {
    /// fxdax: its name, as an operator reads it (`fxdax-gc --dry-run`).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Segment { .. } => "segment",
            Self::Extent { .. } => "extent",
            Self::Checkpoint { .. } => "checkpoint",
            Self::Record { .. } => "record",
            Self::Lease { .. } => "lease",
            Self::Cell { .. } => "cell",
            Self::ObjectIds { .. } => "objectid",
            Self::GcLease { .. } => "gc-lease",
            Self::Marks { .. } => "marks",
            Self::Marking => "marking",
            Self::Moves { .. } => "moves",
            Self::Reference { .. } => "reference",
            Self::Unknown => "unknown",
        }
    }
}

/// fxdax: the cell whose object `key` is, if a cell's: every kind under its prefix, the moves of its
/// objects and of their moves, its collector's lease and marks, and the references of forks of its
/// branches, under the parent's cell (`fxdax-gc --usage`, TASK-394, TASK-395.2).
pub fn owner(key: &str) -> Option<Cell> {
    match kind(key) {
        Kind::Segment { branch, .. }
        | Kind::Extent { branch, .. }
        | Kind::Checkpoint { branch, .. }
        | Kind::Record { branch }
        | Kind::Lease { branch }
        | Kind::Reference { parent: branch, .. } => Some(branch.cell),
        Kind::Cell { cell } | Kind::GcLease { cell } | Kind::Marks { cell, .. } => Some(cell),
        Kind::Moves { cell, .. } => Some(cell),
        Kind::ObjectIds { .. } | Kind::Marking | Kind::Unknown => None,
    }
}

fn hex(text: &str) -> Option<u64> {
    (text.len() == 16).then(|| u64::from_str_radix(text, 16).ok()).flatten()
}

pub fn kind(key: &str) -> Kind<'_> {
    let parsed = || -> Option<Kind<'_>> {
        if key == MARKING {
            return Some(Kind::Marking);
        }
        if let Some(block) = key.strip_prefix(OBJECTIDS) {
            return Some(Kind::ObjectIds { block: hex(block)? });
        }
        let (cell, rest) = in_cell(key)?;
        let branch = |id| Some(Branch { cell, id: hex(id)? });
        if rest == "cell" {
            return Some(Kind::Cell { cell });
        }
        if let Some(of) = rest.strip_prefix(MOVES) {
            return (!of.is_empty()).then_some(Kind::Moves { cell, of });
        }
        if let Some(rest) = rest.strip_prefix(GC) {
            if rest == GC_LEASE.strip_prefix(GC)? {
                return Some(Kind::GcLease { cell });
            }
            let epoch = rest.strip_prefix(GC_MARKS.strip_prefix(GC)?)?;
            return Some(Kind::Marks { cell, epoch: hex(epoch)? });
        }
        if let Some(rest) = rest.strip_prefix(REFS) {
            let [parent, at, child] = rest.split('/').collect::<Vec<_>>()[..] else {
                return None;
            };
            let [tenant, child_cell, child] = child.split('.').collect::<Vec<_>>()[..] else {
                return None;
            };
            let child = Branch {
                cell: Cell { tenant: hex(tenant)?, id: hex(child_cell)? },
                id: hex(child)?,
            };
            return Some(Kind::Reference { parent: branch(parent)?, at: hex(at)?, child });
        }
        if let Some(rest) = rest.strip_prefix("branches/") {
            return Some(Kind::Record { branch: branch(rest)? });
        }
        if let Some(rest) = rest.strip_prefix("leases/") {
            return Some(Kind::Lease { branch: branch(rest)? });
        }
        if let Some(rest) = rest.strip_prefix("segments/") {
            let (id, seq) = rest.split_once('/')?;
            return Some(Kind::Segment { branch: branch(id)?, seq: hex(seq)? });
        }
        if let Some(rest) = rest.strip_prefix("checkpoints/") {
            let (id, seq) = rest.split_once('/')?;
            return Some(Kind::Checkpoint { branch: branch(id)?, seq: hex(seq)? });
        }
        if let Some(rest) = rest.strip_prefix("extents/") {
            let (id, rest) = rest.split_once('/')?;
            let [epoch, seq, sequence] = rest.split('.').collect::<Vec<_>>()[..] else {
                return None;
            };
            let (epoch, seq, sequence) = (hex(epoch)?, hex(seq)?, hex(sequence)?);
            return Some(Kind::Extent { branch: branch(id)?, epoch, seq, sequence });
        }
        None
    };
    parsed().unwrap_or(Kind::Unknown)
}

/// A fork of a branch, made at the end of the commits of its segment `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fork {
    pub child: Branch,
    pub at: u64,
    /// Whether its child is a point ([`Kind::Point`](crate::record::Kind::Point)).
    pub point: bool,
    /// fxdax: its child's owner, which every record of the child names
    /// ([`Record::owner`](crate::record::Record::owner)).
    pub owner: u64,
}

impl Fork {
    /// The fork its segment `at`'s footer names, `child`.
    pub fn at(at: u64, child: &Child) -> Self {
        Self { child: child.branch, at, point: child.point, owner: child.owner }
    }
}

/// A segment: its data, then its footer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub data: Vec<u8>,
    pub footer: Footer,
}

/// The child of a fork to `branch`, a branch, of no owner: tests', which name no two children
/// alike.
#[cfg(test)]
pub(crate) fn child(branch: Branch) -> Child {
    Child { branch, owner: 0, point: false }
}

/// fxdax: the cell of tests' branches, of tenant 1.
#[cfg(any(test, feature = "testing"))]
pub const TEST_CELL: Cell = Cell { tenant: 1, id: 1 };

/// fxdax: tests' branch `id`, of [`TEST_CELL`].
#[cfg(any(test, feature = "testing"))]
pub const fn test_branch(id: u64) -> Branch {
    Branch { cell: TEST_CELL, id }
}

/// Where a branch was forked from: ordered by its parent, then its segment, so that the forks made
/// at one segment sort together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Origin {
    pub parent: Branch,
    pub at: u64,
}

/// The holder of a lease, and its epoch: a branch's writer, or the garbage collector. A lease of
/// [`Lease::FINAL`] is a deleted branch's: no writer takes it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub epoch: u64,
    /// Who took it: unique to each taking, so that a write of it is recognised.
    pub holder: u64,
    /// The fencing token of the placement that took it: a branch's writer's placement's boot ID,
    /// which a later placement's exceeds, and which no lower one takes the lease from (TASK-39.7).
    /// The collector's is 0.
    pub token: u64,
}

impl Lease {
    pub const FINAL: u64 = u64::MAX;

    /// The lease `bytes` encode.
    pub fn decode(bytes: &[u8]) -> Result<Self, crate::Error> {
        decode(bytes)
    }
}

/// fxdax: a mark-only pass's marks object ([`marks`]): what it would have deleted, each object as
/// its listing found it, which readers warn of reaching, and the next such pass compares what it
/// finds doomed with (TASK-264).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks {
    /// The epoch of the collector's lease the pass took.
    pub epoch: u64,
    /// Its grace period, in milliseconds by the store's clock ([`crate::collector::Options`]):
    /// what it found doomed, a pass of no shorter one finds doomed too.
    pub grace: u64,
    /// The objects marked, in the order of their keys.
    pub marked: Vec<Marked>,
}

/// fxdax: a reference's bytes ([`reference`]): the format of the bucket's objects, first, as a
/// record's, and the fork's owner, its child's ([`Child::owner`], a clone's its restore's): unique
/// to the fork, so that every writer of it writes the same, a writable fork's host, its parent's
/// writers and its parent's deletion alike, and a create landed though its answer was lost is taken
/// for its own (TASK-395.3). What the collector keeps by is the key alone; nothing reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub format: u32,
    pub owner: u64,
}

impl Reference {
    /// The reference of the fork whose child's owner is `owner`.
    pub fn of(owner: u64) -> Self {
        Self { format: FORMAT, owner }
    }
}

/// An object a mark-only pass found doomed, as the listing it went by found it: an object at its
/// key of another version is another object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marked {
    pub key: String,
    pub size: u64,
    pub e_tag: Option<String>,
    pub version: Option<String>,
    /// When it was written, by the store's clock.
    pub modified: u64,
}

impl Marked {
    /// The object `meta` lists.
    pub fn of(meta: &crate::bucket::Meta) -> Self {
        Self {
            key: meta.key.clone(),
            size: meta.size,
            e_tag: meta.version.e_tag.clone(),
            version: meta.version.version.clone(),
            modified: meta.modified,
        }
    }

    /// Whether `meta` lists it: the object at its key, of its version.
    pub fn is(&self, meta: &crate::bucket::Meta) -> bool {
        self.key == meta.key
            && self.e_tag == meta.version.e_tag
            && self.version == meta.version.version
            && self.size == meta.size
    }
}

/// The footer of a compacted object's moves: where in the moves each extent the object held, live,
/// now is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moves {
    pub extents: Vec<(ExtentId, Location)>,
}

pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::serialize(value).expect("objects serialize")
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, crate::Error> {
    bincode::deserialize(bytes).map_err(|error| crate::Error::Corrupt(error.to_string()))
}

pub use extent_frame::{Frame, PART, TRAILER, crc};

/// The bytes of an object of `data` and `footer`, framed (`extent_frame`): the data's parts,
/// compressed, the footer, the table of where the parts are, then the [`TRAILER`].
pub fn with_footer<T: Serialize>(data: &[u8], footer: &T) -> Vec<u8> {
    extent_frame::framed(data, &encode(footer))
}

/// The data of a whole object [`with_footer`] made, checked against its CRCs.
pub fn data(bytes: &[u8]) -> Result<Vec<u8>, crate::Error> {
    Ok(extent_frame::data(bytes)?)
}

/// The footer of a whole object [`with_footer`] made.
pub fn footer<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, crate::Error> {
    decode(extent_frame::footer(bytes)?)
}

impl Segment {
    pub fn encode(&self) -> Vec<u8> {
        with_footer(&self.data, &self.footer)
    }

    /// The footer of a segment's object.
    pub fn footer(bytes: &[u8]) -> Result<Footer, crate::Error> {
        footer(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The moves of `key`, a cell's object's.
    fn moves(key: &str) -> String {
        super::moves(key).unwrap()
    }

    /// A branch's extents stored on their own, whichever writer uploaded them, are under its
    /// prefix, and no other branch's are, of its cell or another.
    #[test]
    fn extents_are_under_their_branchs_prefix() {
        let branch = test_branch(0x12);
        let elsewhere = Cell { tenant: 1, id: 2 };
        let expected = "tenants/0000000000000001/cells/0000000000000001/extents/0000000000000012/";
        assert_eq!(extents(branch), expected);
        for epoch in [1, 7] {
            let key = extent(TEST_CELL, epoch, 3, ExtentId::new(0x12, 5));
            assert!(key.starts_with(&extents(branch)));
        }
        assert!(!extent(TEST_CELL, 1, 3, ExtentId::new(0x13, 5)).starts_with(&extents(branch)));
        assert!(!extent(elsewhere, 1, 3, ExtentId::new(0x12, 5)).starts_with(&extents(branch)));
    }

    /// fxdax: every object of a branch, and its cell's own, is under the cell's prefix, by tenant
    /// then cell (TASK-364); and since format 9 everything the cell's collection reads: its objects'
    /// moves and their moves, its collector's lease and marks, and the references of forks of its
    /// branches, whatever the children's cells (TASK-395.2).
    #[test]
    fn a_cells_objects_are_under_its_tenants_prefix() {
        let cell = Cell { tenant: 0xa, id: 0xc };
        let branch = Branch { cell, id: 3 };
        let elsewhere = Branch { cell: Cell { tenant: 0xb, id: 0xd }, id: 4 };
        let prefix = "tenants/000000000000000a/cells/000000000000000c/";
        assert_eq!(cell.prefix(), prefix);
        for key in [
            segment(branch, 7),
            extent(cell, 2, 4, ExtentId::new(3, 9)),
            checkpoint(branch, 7),
            record(branch),
            lease(branch),
            self::cell(cell),
            moves(&segment(branch, 7)),
            moves(&moves(&segment(branch, 7))),
            gc_lease(cell),
            marks(cell, 2),
            reference(branch, 7, elsewhere),
        ] {
            assert!(key.starts_with(prefix) && key.starts_with(TENANTS), "{key}");
        }
        assert_eq!(record(branch), format!("{prefix}branches/0000000000000003"));
        assert!(record(branch).starts_with(&records(cell)));
        assert_eq!(
            moves(&segment(branch, 7)),
            format!("{prefix}moves/segments/0000000000000003/0000000000000007")
        );
        assert_eq!(
            moves(&moves(&segment(branch, 7))),
            format!("{prefix}moves/moves/segments/0000000000000003/0000000000000007")
        );
        assert_eq!(gc_lease(cell), format!("{prefix}gc/lease"));
        assert_eq!(marks(cell, 0x2a), format!("{prefix}gc/marks/000000000000002a"));
        assert_eq!(
            reference(branch, 7, elsewhere),
            format!(
                "{prefix}refs/0000000000000003/0000000000000007/\
                 000000000000000b.000000000000000d.0000000000000004"
            )
        );
    }

    /// TASK-395.2: a delimiter listing of a cell's references gives the parents with forks, and one
    /// of a parent's, its distinct fork points: each prefix read back as what it names, and no
    /// other.
    #[test]
    fn references_list_by_parent_then_fork_point() {
        let cell = Cell { tenant: 0xa, id: 0xc };
        let (parent, child) =
            (Branch { cell, id: 3 }, Branch { cell: Cell { tenant: 1, id: 2 }, id: 5 });
        let key = reference(parent, 7, child);
        assert!(key.starts_with(&fork_point(parent, 7)));
        assert!(fork_point(parent, 7).starts_with(&references_of(parent)));
        assert!(references_of(parent).starts_with(&references(cell)));
        assert_eq!(referenced_parent(cell, &references_of(parent)), Some(parent));
        assert_eq!(referenced_at(parent, &fork_point(parent, 7)), Some(7));
        assert_eq!(referenced_parent(cell, &fork_point(parent, 7)), None);
        assert_eq!(referenced_at(parent, &references_of(parent)), None);
        let other = Cell { tenant: 0xa, id: 0xd };
        assert_eq!(referenced_parent(other, &references_of(parent)), None);
    }

    /// TASK-395.2: the moves of a moves are of what it is of, along the way, in its cell alone.
    #[test]
    fn moves_are_further_along_the_way_in_their_cell() {
        let cell = Cell { tenant: 0xa, id: 0xc };
        let sealed = segment(Branch { cell, id: 3 }, 7);
        let (once, twice) = (moves(&sealed), moves(&moves(&sealed)));
        assert!(further(&sealed, &once) && further(&sealed, &twice) && further(&once, &twice));
        assert!(!further(&once, &sealed) && !further(&twice, &once) && !further(&sealed, &sealed));
        let other = segment(Branch { cell: Cell { tenant: 0xa, id: 0xd }, id: 3 }, 7);
        assert!(!further(&other, &once) && !further(&sealed, &moves(&other)));
        assert_eq!(moved_from(&once), Some(sealed.clone()));
        assert_eq!(moved_from(&twice), Some(once.clone()));
        assert_eq!(moved_from(&sealed), None);
    }

    /// The moves of an object of no cell's, which nothing writes, and a location read from a
    /// corrupt checkpoint may name: refused as corrupt, not put at the bucket's root as before
    /// format 9, nor a panic of the reader's.
    #[test]
    fn objects_of_no_cell_have_no_moves() {
        let refused = "probe/0000000000000001: no cell's object, which has no moves".to_owned();
        assert_eq!(super::moves("probe/0000000000000001"), Err(crate::Error::Corrupt(refused)));
    }

    #[test]
    fn kinds() {
        let cell = Cell { tenant: 0xa, id: 0xc };
        let branch = Branch { cell, id: 3 };
        let id = ExtentId::new(3, 9);
        assert_eq!(kind(&segment(branch, 7)), Kind::Segment { branch, seq: 7 });
        let own = Kind::Extent { branch, epoch: 2, seq: 4, sequence: 9 };
        assert_eq!(kind(&extent(cell, 2, 4, id)), own);
        // A key naming no segment declares nothing: no extent's.
        let earlier =
            format!("{}extents/0000000000000003/0000000000000002.0000000000000009", cell.prefix());
        assert_eq!(kind(&earlier), Kind::Unknown);
        assert_eq!(kind(&checkpoint(branch, 7)), Kind::Checkpoint { branch, seq: 7 });
        assert_eq!(kind(&record(branch)), Kind::Record { branch });
        assert_eq!(kind(&lease(branch)), Kind::Lease { branch });
        assert_eq!(kind(&self::cell(cell)), Kind::Cell { cell });
        assert_eq!(objectid_block(0x2a), "objectid/000000000000002a");
        assert_eq!(kind(&objectid_block(0x2a)), Kind::ObjectIds { block: 0x2a });
        assert_eq!(kind("objectid/2a"), Kind::Unknown);
        assert_eq!(kind(&gc_lease(cell)), Kind::GcLease { cell });
        assert_eq!(kind(&marks(cell, 0x2a)), Kind::Marks { cell, epoch: 0x2a });
        assert_eq!(kind(&format!("{}gc/marks/2a", cell.prefix())), Kind::Unknown);
        assert_eq!(kind(&format!("{}gc/other", cell.prefix())), Kind::Unknown);
        let from = segment(branch, 7);
        let of = "segments/0000000000000003/0000000000000007";
        assert_eq!(kind(&moves(&from)), Kind::Moves { cell, of });
        let moved = format!("moves/{of}");
        assert_eq!(kind(&moves(&moves(&from))), Kind::Moves { cell, of: &moved });
        assert_eq!(kind(&format!("{}moves/", cell.prefix())), Kind::Unknown);
        let child = Branch { cell: Cell { tenant: 1, id: 2 }, id: 5 };
        assert_eq!(
            kind(&reference(branch, 7, child)),
            Kind::Reference { parent: branch, at: 7, child }
        );
        assert_eq!(kind(&format!("{}2", fork_point(branch, 7))), Kind::Unknown);
        assert_eq!(kind(&fork_point(branch, 7)), Kind::Unknown);
        // The layout before cells, and keys of cells' prefixes malformed, are no objects of a
        // branch; nor the collector's lease, marks and moves at the bucket's root, as format 8 had
        // them (TASK-395.2).
        assert_eq!(kind("segments/0000000000000003/0000000000000007"), Kind::Unknown);
        assert_eq!(kind("branches/0000000000000003"), Kind::Unknown);
        assert_eq!(kind("tenants/a/cells/000000000000000c/cell"), Kind::Unknown);
        assert_eq!(kind("tenants/000000000000000a/cell/000000000000000c/cell"), Kind::Unknown);
        let short = format!("{}segments/0000000000000003/7", cell.prefix());
        assert_eq!(kind(&short), Kind::Unknown);
        assert_eq!(kind("gc/lease"), Kind::Unknown);
        assert_eq!(kind("gc/marks/000000000000002a"), Kind::Unknown);
        assert_eq!(kind(&format!("moves/{from}")), Kind::Unknown);
        assert_eq!(cell_of_prefix(&cell.prefix()), Some(cell));
        assert_eq!(cell_of_prefix(&records(cell)), None);
        assert_eq!(cell_of_prefix(""), None);
    }

    /// TASK-394, TASK-395.2: every kind of a cell's objects is the cell's, its moves however many
    /// times moved, its collector's lease and marks, and the references under it, of forks of its
    /// branches to other cells; object-ID blocks and keys of no kind are no cell's.
    #[test]
    fn owners() {
        let cell = Cell { tenant: 0xa, id: 0xc };
        let branch = Branch { cell, id: 3 };
        let child = Branch { cell: Cell { tenant: 1, id: 2 }, id: 5 };
        let keys = [
            segment(branch, 7),
            extent(cell, 2, 4, ExtentId::new(3, 9)),
            checkpoint(branch, 7),
            record(branch),
            lease(branch),
            self::cell(cell),
            moves(&segment(branch, 7)),
            moves(&moves(&extent(cell, 2, 4, ExtentId::new(3, 9)))),
            gc_lease(cell),
            marks(cell, 1),
            reference(branch, 7, child),
        ];
        for key in keys {
            assert_eq!(owner(&key), Some(cell), "{key}");
        }
        let keys = [
            "gc/lease".to_owned(),
            "gc/marks/0000000000000001".to_owned(),
            objectid_block(1),
            "probe/0000000000000001".to_owned(),
            "moves/probe/0000000000000001".to_owned(),
            format!("{}leases/3", cell.prefix()),
        ];
        for key in keys {
            assert_eq!(owner(&key), None, "{key}");
        }
    }

    #[test]
    fn leases_decode_as_they_were_written() {
        let lease = Lease { epoch: 4, holder: 9, token: 7 };
        assert_eq!(Lease::decode(&encode(&lease)), Ok(lease));
    }

    #[test]
    fn segments_hold_their_footers() {
        let footer = Footer {
            branch: test_branch(1),
            seq: 2,
            epoch: 3,
            after: Some((1, 3)),
            contents: Contents {
                commits: vec![4],
                extents: vec![(ExtentId::new(1, 5), 0, 3)],
                own: vec![],
                frees: vec![ExtentId::new(1, 1)],
                root: Some(b"root".to_vec()),
                next_sequence: 6,
                forks: vec![Child { branch: test_branch(7), owner: 8, point: false }],
                capacity: Some(1 << 30),
                continued: None,
            },
        };
        let segment = Segment { data: b"abc".to_vec(), footer: footer.clone() };
        let bytes = segment.encode();
        assert_eq!(&bytes[..3], b"abc");
        assert_eq!(Segment::footer(&bytes).unwrap(), footer);
        assert!(Segment::footer(&bytes[..bytes.len() - 1]).is_err());
    }

    /// Objects' footers are found by their frames.
    #[test]
    fn objects_hold_their_footers() {
        let bytes = with_footer(&[1; 5000], &"footer");
        assert_eq!(footer::<String>(&bytes).unwrap(), "footer");
        assert_eq!(data(&bytes).unwrap(), &[1; 5000][..]);
        let mut flipped = bytes.clone();
        flipped[7] ^= 1;
        assert_eq!(
            data(&flipped),
            Err(crate::Error::Corrupt("an object's part, by its CRC".into()))
        );
    }
}
