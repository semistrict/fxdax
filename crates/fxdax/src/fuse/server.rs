// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The FUSE server: a request's bytes in, its reply's out, whatever carries them.
//!
//! A server serves one filesystem, whose root is the mount's ([`Server::new`]); or many stores'
//! (TASK-302), each attached at a path in directories of the server's own, and detached again
//! ([`Server::stores`], [`Server::attach`], [`Server::detach`]): one mount, however many stores.
//! fxdax: a store's path is names, its last its own and those before it directories of the
//! server's, made as a store is attached under them and gone once none is: a host's mount of
//! cells' volumes attaches each at `TENANT/CELL/VOLUME` (TASK-364).
//!
//! Node IDs and inode numbers say whose node is whose:
//!
//! - One filesystem's node IDs are its object IDs, but the root's, which is FUSE's root ID (1), and
//!   that ID's, which is the root's: the two swap places. Inode numbers are object IDs.
//! - Many stores' are a store's slot, from 1, in the top 16 bits, and the object ID in the low 48,
//!   so that no two stores' nodes share one: `EOVERFLOW` for an object ID of more bits, which
//!   fxdax's Fxfs, numbering a volume's objects in turn, doesn't reach. Inode numbers are node IDs;
//!   the root directory of stores is FUSE's root ID, and the server's other directories are of
//!   slot 0, each its own, never one another had before. A slot is used again once the kernel has
//!   forgotten every node of the store detached from it.
//!
//! The directories of stores are the server's: nothing is made, removed or renamed in them, but by
//! attaching and detaching (`EPERM`), and nothing moves between them and a store, or between
//! stores (`EXDEV`). The kernel looks their names up again each time (they and their absence are
//! cached for no time).
//!
//! The kernel drops a dentry only when it is told to by name ([`Notifier`]), or when it is short
//! of memory: a store detached waits for it to forget every node of the store, so the server keeps
//! the names it gave the kernel each node under, and tells it to drop them all. A dentry in use, a
//! file open or a directory someone's current one, goes once it is let go of; those the kernel is
//! given while the store is detached are given for no time.

use super::abi::*;
use crate::error::{Errno, Error, Result};
use crate::filesystem::{Filesystem, Node, valid_name};
use crate::node::XattrMode;
use crate::node::{Attr, DirEntry, FileType, Inode, Owner, RenameFlags, S_ISGID, SetAttr, SetTime};
use event_listener::Event;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::mem::size_of;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zerocopy::{FromBytes, IntoBytes};

/// The most a write carries, and a read asks for: the `max_write` INIT gives the kernel, which
/// sizes its reads by it too (`max_pages`).
pub(crate) const MAX_WRITE: u32 = 1 << 20;

/// How long the kernel may cache names, their absence, and attributes: as long as it likes, as
/// the mount is each filesystem's only writer, whose store its process alone has open.
const TIMEOUT_SECS: u64 = 24 * 60 * 60;

/// Where a node ID of many stores' says its store's slot: the bits above the object ID's.
const SLOT_SHIFT: u32 = 48;

/// The size of the blocks the root directory of stores says it has, which holds none.
const STORES_BLOCK_SIZE: u32 = 4096;

/// What sends the kernel a notification, as the server has written it: a write to `/dev/fuse`.
/// Fails as the write does: `ENOENT` if the kernel has nothing the notification is of.
pub type Notifier = Box<dyn Fn(&[u8]) -> std::io::Result<()> + Send + Sync>;

/// A FUSE server of a [`Filesystem`], or of many stores' ([`Server::stores`]):
/// [`handle`](Self::handle) takes a request as the kernel sent it, and gives its reply, for
/// whatever carries them, as `Mount` does `/dev/fuse`'s. See the module docs.
pub struct Server {
    shape: Shape,
    /// The filesystems served, by slot.
    stores: Mutex<Stores>,
    /// The nodes the kernel holds.
    nodes: Mutex<Nodes>,
    /// The directories open, by handle.
    dirs: Mutex<OpenDirs>,
    /// Notified as the kernel forgets a node: for detaches waiting for it to forget a store's.
    forgotten: Event,
}

/// What the mount's root is.
enum Shape {
    /// The root directory of the one filesystem, in slot 0: its object ID and FUSE's root ID swap
    /// places as node IDs.
    One { root: Node },
    /// A directory of the server's own, of the stores attached in it, whose attributes it has
    /// ([`Server::stores_attr`]); `notify` tells the kernel of names detached.
    Many { owner: Owner, made: SystemTime, notify: Notifier },
}

/// The filesystems served: one's in slot 0, or many stores', by slot, from 1, and by path.
#[derive(Default)]
struct Stores {
    slots: BTreeMap<u16, Slot>,
    names: BTreeMap<String, u16>,
    /// Whether the server serves no more: the kernel destroyed the mount, or it was closed. No
    /// store is attached then.
    closed: bool,
    /// Many stores': the directories of the server's own, by node ID, the root's among them.
    dirs: BTreeMap<u64, Dir>,
    /// The node ID the next directory made takes: never one a directory had before, which the
    /// kernel may hold yet.
    next_dir: u64,
}

struct Slot {
    /// Its path, as it was attached at.
    name: String,
    /// The directory of the server's its root is in, and its name there.
    dir: u64,
    leaf: String,
    state: State,
}

/// fxdax: a directory of the server's own, of many stores': the root of stores, or one a path a
/// store is attached at goes through, as `TENANT/CELL/VOLUME` goes through `TENANT` and
/// `TENANT/CELL` (TASK-364).
struct Dir {
    /// Its parent's node ID: the root's is its own.
    parent: u64,
    /// Its name in its parent.
    name: String,
    entries: BTreeMap<String, Entry>,
}

/// What a name in a directory of the server's names.
#[derive(Clone, Copy)]
enum Entry {
    Dir(u64),
    Store(u16),
}

impl Stores {
    /// Many stores', of none yet: the root directory alone.
    fn many() -> Self {
        let root = Dir { parent: ROOT_ID, name: String::new(), entries: BTreeMap::new() };
        Self { dirs: BTreeMap::from([(ROOT_ID, root)]), next_dir: ROOT_ID + 1, ..Self::default() }
    }

    /// Puts `slot` at `path`, the directories it goes through made where missing: returns the
    /// directory it is in, and its name there. `EEXIST` if a store has a name the path goes
    /// through, or a directory its last.
    fn put(&mut self, path: &[&str], slot: u16) -> Result<(u64, String)> {
        let (leaf, through) = path.split_last().ok_or(Errno::EINVAL)?;
        let mut dir = ROOT_ID;
        for name in through {
            dir = match self.dirs[&dir].entries.get(*name) {
                Some(Entry::Dir(sub)) => *sub,
                Some(Entry::Store(_)) => return Err(Errno::EEXIST),
                None => {
                    let made = self.next_dir;
                    self.next_dir += 1;
                    let sub = Dir { parent: dir, name: (*name).into(), entries: BTreeMap::new() };
                    self.dirs.insert(made, sub);
                    let parent = self.dirs.get_mut(&dir).expect("a directory made before");
                    parent.entries.insert((*name).into(), Entry::Dir(made));
                    made
                }
            };
        }
        let entries = &mut self.dirs.get_mut(&dir).expect("a directory").entries;
        if entries.contains_key(*leaf) {
            return Err(Errno::EEXIST);
        }
        entries.insert((*leaf).into(), Entry::Store(slot));
        Ok((dir, (*leaf).into()))
    }

    /// Whether the directory `dir` of the server's shows: whether a store attached is under it, not
    /// one being attached or detached alone, which are gone from the mount already.
    fn shows(&self, dir: u64) -> bool {
        self.dirs.get(&dir).is_some_and(|of| {
            of.entries.values().any(|entry| match *entry {
                Entry::Dir(sub) => self.shows(sub),
                Entry::Store(slot) => matches!(self.slots[&slot].state, State::Attached(_)),
            })
        })
    }

    /// Takes the store in `slot` away from its path, and its name: the directories of the
    /// server's that no store is under any more go.
    fn remove(&mut self, slot: u16) {
        let Some(gone) = self.slots.remove(&slot) else { return };
        self.names.remove(&gone.name);
        let (mut dir, mut name) = (gone.dir, gone.leaf);
        while let Some(of) = self.dirs.get_mut(&dir) {
            of.entries.remove(&name);
            if dir == ROOT_ID || !of.entries.is_empty() {
                break;
            }
            let empty = self.dirs.remove(&dir).expect("a directory");
            (dir, name) = (empty.parent, empty.name);
        }
    }
}

enum State {
    /// Its store is being opened: its name is taken, not yet looked up.
    Opening,
    /// Its filesystem is served.
    Attached(Arc<Filesystem>),
    /// Its filesystem is being detached: its name is no longer looked up, and nodes under it the
    /// kernel still holds are served until it is unmounted.
    Detaching(Arc<Filesystem>),
}

impl State {
    fn filesystem(&self) -> Option<&Arc<Filesystem>> {
        match self {
            Self::Opening => None,
            Self::Attached(fs) | Self::Detaching(fs) => Some(fs),
        }
    }
}

/// A store attached, as [`Server::attached`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    /// Its name in the root directory of stores.
    pub name: String,
    /// Whether it is being detached.
    pub detaching: bool,
}

/// The nodes the kernel holds, and, of many stores', the names it has them under.
#[derive(Default)]
struct Nodes {
    /// The nodes the kernel has looked up, by node ID, and how many lookups it holds of each,
    /// which it forgets as it evicts their inodes: a node it holds stays, as its handle holds it,
    /// unlinked or not, until it is forgotten. The mount's root is not among them: it stays while
    /// the server does.
    looked: HashMap<u64, Looked>,
    /// Many stores': the names the kernel was given nodes under, each its directory's node ID and
    /// the name in it, and the node it names, as they are renamed and removed, for a detach to have
    /// the kernel drop them ([`Server::detach`]).
    names: HashMap<Name, u64>,
    /// Many stores': the slots of those being detached, whose nodes are given for no time.
    detaching: HashSet<u16>,
}

/// A name in a directory, whose node ID is first.
type Name = (u64, Box<[u8]>);

/// A node the kernel has looked up, and how many lookups of it it holds.
struct Looked {
    node: Node,
    lookups: u64,
    /// Many stores': the names in [`Nodes::names`] that are this node's.
    names: Vec<Name>,
}

impl Nodes {
    /// The kernel was given the node `nodeid` at `name`, which no longer names another.
    fn named(&mut self, nodeid: u64, name: Name) {
        match self.names.insert(name.clone(), nodeid) {
            Some(named) if named == nodeid => return,
            Some(other) => self.forget_name(other, &name),
            None => {}
        }
        if let Some(looked) = self.looked.get_mut(&nodeid) {
            looked.names.push(name);
        }
    }

    /// `name` is gone: it names no node.
    fn unnamed(&mut self, name: &Name) {
        if let Some(nodeid) = self.names.remove(name) {
            self.forget_name(nodeid, name);
        }
    }

    /// `from` is renamed `to`, which no longer names another; or, if `exchange`, the two swap.
    fn renamed(&mut self, from: Name, to: Name, exchange: bool) {
        let (moved, replaced) = (self.names.remove(&from), self.names.remove(&to));
        for (nodeid, was) in [(moved, &from), (replaced, &to)] {
            if let Some(nodeid) = nodeid {
                self.forget_name(nodeid, was);
            }
        }
        if let Some(nodeid) = moved {
            self.named(nodeid, to.clone());
        }
        if let Some(nodeid) = replaced.filter(|_| exchange) {
            self.named(nodeid, from);
        }
    }

    /// The node `nodeid` is no longer at `name`.
    fn forget_name(&mut self, nodeid: u64, name: &Name) {
        if let Some(looked) = self.looked.get_mut(&nodeid) {
            looked.names.retain(|had| had != name);
        }
    }
}

/// What a node ID names: a directory of the server's own, of many stores', or a node of the
/// filesystem in a slot.
#[derive(Clone)]
enum Target {
    Stores(u64),
    Node(u16, Node),
}

/// What a lookup found: a node of the filesystem in a slot, or a directory of the server's own.
enum Found {
    Node(u16, (Node, Attr)),
    Stores(u64),
}

#[derive(Default)]
struct OpenDirs {
    /// The handle the next directory opened takes.
    next: u64,
    open: HashMap<u64, Arc<OpenDir>>,
}

/// An open directory: its entries as it held them as it was opened, which its reads list in
/// turn, an offset an entry's index past `.` and `..`, so that they list each entry once, however
/// the directory changes meanwhile. Inode numbers are the kernel's.
struct OpenDir {
    dir: Target,
    /// The directory's node ID.
    nodeid: u64,
    /// `.`'s inode.
    inode: u64,
    /// `..`'s inode.
    parent: u64,
    entries: Vec<DirEntry>,
}

impl Server {
    /// A server of `fs`, whose root is the mount's.
    pub fn new(fs: Filesystem) -> Self {
        let root = fs.root().clone();
        let state = State::Attached(Arc::new(fs));
        let slot = Slot { name: String::new(), dir: ROOT_ID, leaf: String::new(), state };
        let stores = Stores { slots: BTreeMap::from([(0, slot)]), ..Stores::default() };
        Self::of(Shape::One { root }, stores)
    }

    /// A server of many stores, attached by name in its root directory, none yet: `owner`'s,
    /// readable and searchable by all (0755). `notify` sends the kernel the notifications the
    /// server writes, as a detach does.
    pub fn stores(owner: Owner, notify: Notifier) -> Self {
        let shape = Shape::Many { owner, made: SystemTime::now(), notify };
        Self::of(shape, Stores::many())
    }

    fn of(shape: Shape, stores: Stores) -> Self {
        Self {
            shape,
            stores: Mutex::new(stores),
            nodes: Mutex::default(),
            dirs: Mutex::default(),
            forgotten: Event::new(),
        }
    }

    /// The filesystem served by a server of one ([`Server::new`]), such as to close it once the
    /// kernel is done: the nodes the kernel held go, and the directories it had open.
    ///
    /// # Panics
    ///
    /// If the server is of many stores.
    pub fn into_filesystem(self) -> Filesystem {
        assert!(matches!(self.shape, Shape::One { .. }), "a server of many stores");
        let slots = std::mem::take(&mut self.stores.into_inner().unwrap().slots);
        drop(self.nodes);
        drop(self.dirs);
        match slots.into_values().next().map(|slot| slot.state) {
            Some(State::Attached(fs)) => {
                Arc::into_inner(fs).expect("the filesystem is the server's alone once it goes")
            }
            _ => unreachable!("a server of one filesystem keeps it"),
        }
    }

    /// Attaches the filesystem `open` gives at `name` in a server of many stores, once it is
    /// open: a path of names separated by `/`, the last the store's root's and those before it
    /// directories of the server's own, made as needed. `name` is taken from the start, and a
    /// store attached as `open` is awaited fails. Fails with `EINVAL` for a name no directory
    /// holds, `EEXIST` for a path another store has, or one going through another store's,
    /// `EBUSY` for one another store is being attached or detached at, `ENOSPC` with every slot
    /// taken, `ENOTCONN` once the server serves no more, `ENOTSUP` for a server of one filesystem;
    /// and as `open` does.
    pub async fn attach(
        &self,
        name: &str,
        open: impl Future<Output = Result<Filesystem, Error>>,
    ) -> Result<(), Error> {
        if !matches!(self.shape, Shape::Many { .. }) {
            return Err(Errno::ENOTSUP.into());
        }
        for component in name.split('/') {
            valid_name(component.as_bytes())?;
        }
        let reserved = self.reserve(name)?;
        let fs = Arc::new(open.await?);
        match reserved.attach(fs.clone()) {
            Ok(()) => Ok(()),
            Err(errno) => {
                // The server closed meanwhile.
                fs.unmount().await?;
                Err(errno.into())
            }
        }
    }

    /// Takes `name`, and the lowest slot free whose nodes the kernel has forgotten, for a store
    /// being opened, until what is returned attaches it or goes.
    fn reserve(&self, name: &str) -> Result<Reserved<'_>> {
        let mut stores = self.stores.lock().unwrap();
        if stores.closed {
            return Err(Errno::ENOTCONN);
        }
        if let Some(slot) = stores.names.get(name) {
            return Err(match stores.slots[slot].state {
                State::Attached(_) => Errno::EEXIST,
                State::Opening | State::Detaching(_) => Errno::EBUSY,
            });
        }
        let nodes = self.nodes.lock().unwrap();
        let held: HashSet<u16> = nodes.looked.keys().map(|&nodeid| self.slot_of(nodeid)).collect();
        drop(nodes);
        let slot = (1..=u16::MAX)
            .find(|slot| !stores.slots.contains_key(slot) && !held.contains(slot))
            .ok_or(Errno::ENOSPC)?;
        let path: Vec<&str> = name.split('/').collect();
        let (dir, leaf) = stores.put(&path, slot)?;
        let reserved = Slot { name: name.to_owned(), dir, leaf, state: State::Opening };
        stores.slots.insert(slot, reserved);
        stores.names.insert(name.to_owned(), slot);
        Ok(Reserved { server: self, slot: Some(slot) })
    }

    /// Detaches the store attached at `name` from a server of many stores: its name goes at once,
    /// and the kernel is told to drop every name it has a node of the store under, which it does
    /// at once for those not in use, then forgetting their nodes, and for the others once they are
    /// let go of. Unless `force`, the store is unmounted once the kernel has forgotten every node of
    /// it, as once whatever has a file open in it, or a directory its current one, lets it go; with
    /// `force`, at once, the nodes the kernel still holds of it failing with `ENOTCONN` from then
    /// on. Unmounting it writes back what is dirty, and closes its store.
    ///
    /// Fails with `ENOENT` if no store is attached at `name`, `EBUSY` if one is being attached or
    /// detached there, `ENOTSUP` for a server of one filesystem; and as unmounting does. A detach
    /// given up on before it completes leaves the store detaching for good: callers that may give
    /// up on it run it in a task of its own.
    pub async fn detach(&self, name: &str, force: bool) -> Result<(), Error> {
        let Shape::Many { notify, .. } = &self.shape else {
            return Err(Errno::ENOTSUP.into());
        };
        let (slot, fs) = {
            let mut stores = self.stores.lock().unwrap();
            let slot = *stores.names.get(name).ok_or(Errno::ENOENT)?;
            let detaching = stores.slots.get_mut(&slot).expect("a name's slot");
            let State::Attached(fs) = &detaching.state else {
                return Err(Errno::EBUSY.into());
            };
            let fs = fs.clone();
            detaching.state = State::Detaching(fs.clone());
            (slot, fs)
        };
        // Names given from here on are given for no time; those given before are dropped. Not
        // under the table's lock: the kernel drops a name in a directory once the requests in it
        // under way, which take the lock, are answered, the names they give with them.
        let given: Vec<Name> = {
            let mut nodes = self.nodes.lock().unwrap();
            nodes.detaching.insert(slot);
            let of_slot = nodes.looked.iter().filter(|(id, _)| self.slot_of(**id) == slot);
            of_slot.flat_map(|(_, looked)| looked.names.iter().cloned()).collect()
        };
        for (dir, given) in given {
            // `ENOENT`: the kernel dropped it already, as it dropped a directory it was in.
            if let Err(err) = notify(&inval_entry(dir, &given))
                && err.raw_os_error() != Some(libc::ENOENT)
            {
                log::error!("fuse: telling the kernel to drop a name of {name}'s: {err}");
            }
        }
        if !force {
            self.forgotten_all(slot).await;
        }
        let unmounted = fs.unmount().await;
        drop(fs);
        self.stores.lock().unwrap().remove(slot);
        self.nodes.lock().unwrap().detaching.remove(&slot);
        unmounted
    }

    /// Waits until the kernel holds no node of the store in `slot`.
    async fn forgotten_all(&self, slot: u16) {
        loop {
            let forgotten = self.forgotten.listen();
            let held = {
                let nodes = self.nodes.lock().unwrap();
                nodes.looked.keys().any(|&nodeid| self.slot_of(nodeid) == slot)
            };
            if !held {
                return;
            }
            forgotten.await;
        }
    }

    /// The stores attached to a server of many, by name, and those being detached.
    pub fn attached(&self) -> Vec<Attached> {
        let stores = self.stores.lock().unwrap();
        (stores.slots.values())
            .filter_map(|slot| match slot.state {
                State::Opening => None,
                State::Attached(_) => Some(Attached { name: slot.name.clone(), detaching: false }),
                State::Detaching(_) => Some(Attached { name: slot.name.clone(), detaching: true }),
            })
            .collect()
    }

    /// Unmounts every filesystem served, writing back what is dirty, and attaches none from then
    /// on: as the kernel destroys the mount. Fails with the first unmount's error, once all are
    /// done.
    pub async fn unmount(&self) -> Result<(), Error> {
        let served: Vec<Arc<Filesystem>> = {
            let mut stores = self.stores.lock().unwrap();
            stores.closed = true;
            stores.slots.values().filter_map(|slot| slot.state.filesystem().cloned()).collect()
        };
        let mut unmounted = Ok(());
        for fs in served {
            let done = fs.unmount().await;
            unmounted = unmounted.and(done);
        }
        unmounted
    }

    /// Handles the request whose bytes are `request`, as the kernel sent it, and returns its
    /// reply, unless it has none (`FORGET`, `BATCH_FORGET`, `INTERRUPT` and `DESTROY`).
    ///
    /// Requests may be handled concurrently, any number at once, as the kernel sends them. Fails if
    /// `request` is too short for a request's header, as then there is nothing to reply to, or if
    /// `DESTROY`'s unmount fails.
    pub async fn handle(&self, request: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let (header, rest) = InHeader::read_from_prefix(request)
            .map_err(|_| Error::msg("the request is shorter than its header"))?;
        let reply = Reply { unique: header.unique };
        let body_len = (header.len as usize).checked_sub(size_of::<InHeader>());
        let Some(body) = body_len.and_then(|len| rest.get(..len)) else {
            return Ok(Some(reply.error(Errno::EINVAL)));
        };
        let mut body = Body(body);
        let payload = match header.opcode {
            opcode::INIT => self.init(&mut body),
            opcode::DESTROY => {
                self.unmount().await?;
                return Ok(None);
            }
            opcode::FORGET => {
                if let Ok(ForgetIn { nlookup }) = body.read() {
                    self.forget(header.nodeid, nlookup);
                }
                return Ok(None);
            }
            opcode::BATCH_FORGET => {
                self.batch_forget(&mut body);
                return Ok(None);
            }
            // Requests are not interruptible: each completes, as Fxfs completes it.
            opcode::INTERRUPT => return Ok(None),
            opcode::LOOKUP => self.lookup(&header, &mut body).await,
            opcode::GETATTR => self.getattr(&header).await,
            opcode::SETATTR => self.setattr(&header, &mut body).await,
            opcode::READLINK => self.readlink(&header).await,
            opcode::SYMLINK => self.symlink(&header, &mut body).await,
            opcode::MKNOD => self.mknod(&header, &mut body).await,
            opcode::MKDIR => self.mkdir(&header, &mut body).await,
            opcode::UNLINK => self.remove(&header, &mut body, false).await,
            opcode::RMDIR => self.remove(&header, &mut body, true).await,
            opcode::RENAME => self.rename(&header, &mut body, false).await,
            opcode::RENAME2 => self.rename(&header, &mut body, true).await,
            opcode::LINK => self.link(&header, &mut body).await,
            opcode::CREATE => self.create(&header, &mut body).await,
            opcode::READ => self.read(&header, &mut body).await,
            opcode::WRITE => self.write(&header, &mut body).await,
            opcode::STATFS => self.statfs(&header).await,
            // Files have no handles: there is nothing to open, flush or release. Refused, OPEN and
            // FLUSH are not sent again, and nor is RELEASE once OPEN is not.
            opcode::OPEN | opcode::FLUSH => Err(Errno::ENOSYS),
            opcode::RELEASE => Ok(Vec::new()),
            opcode::FSYNC | opcode::FSYNCDIR => self.fsync(&header).await,
            opcode::SETXATTR => self.setxattr(&header, &mut body).await,
            opcode::GETXATTR => self.getxattr(&header, &mut body).await,
            opcode::LISTXATTR => self.listxattr(&header, &mut body).await,
            opcode::REMOVEXATTR => self.removexattr(&header, &mut body).await,
            opcode::OPENDIR => self.opendir(&header).await,
            opcode::READDIR => self.readdir(&mut body, false).await,
            opcode::READDIRPLUS => self.readdir(&mut body, true).await,
            opcode::RELEASEDIR => self.releasedir(&mut body),
            opcode::FALLOCATE => self.fallocate(&header, &mut body).await,
            _ => Err(Errno::ENOSYS),
        };
        Ok(Some(match payload {
            Ok(payload) => reply.ok(&payload),
            Err(errno) => reply.error(errno),
        }))
    }

    /// The object ID of the root of a server of one filesystem, which swaps places with FUSE's
    /// root ID as node IDs.
    #[cfg(test)]
    fn root(&self) -> u64 {
        match &self.shape {
            Shape::One { root } => root.inode().0,
            Shape::Many { .. } => panic!("a server of many stores"),
        }
    }

    /// The node ID that is the inode number `inode` (and the inode number that is the node ID
    /// `inode`): its own, but for a server of one filesystem, whose root's and [`ROOT_ID`] swap
    /// places.
    fn swap(&self, inode: u64) -> u64 {
        let Shape::One { root } = &self.shape else { return inode };
        let root = root.inode().0;
        match inode {
            ROOT_ID => root,
            id if id == root => ROOT_ID,
            id => id,
        }
    }

    /// The slot of the store whose node the node ID `nodeid` is.
    fn slot_of(&self, nodeid: u64) -> u16 {
        match self.shape {
            Shape::One { .. } => 0,
            Shape::Many { .. } => (nodeid >> SLOT_SHIFT) as u16,
        }
    }

    /// The inode number the kernel is given of the object `object` of the store in `slot`.
    fn inode(&self, slot: u16, object: u64) -> Result<u64> {
        match self.shape {
            Shape::One { .. } => Ok(object),
            Shape::Many { .. } if object >> SLOT_SHIFT != 0 => Err(Errno::EOVERFLOW),
            Shape::Many { .. } => Ok((slot as u64) << SLOT_SHIFT | object),
        }
    }

    /// What the node ID `nodeid` names: the mount's root, a directory of the server's own, or a
    /// node the kernel holds a lookup of. Any other is one the kernel forgot, or never had, or a
    /// directory of the server's gone: `ESTALE`.
    fn target(&self, nodeid: u64) -> Result<Target> {
        match &self.shape {
            Shape::One { root } if nodeid == ROOT_ID => return Ok(Target::Node(0, root.clone())),
            Shape::Many { .. } if self.slot_of(nodeid) == 0 => {
                let stores = self.stores.lock().unwrap();
                return match stores.dirs.contains_key(&nodeid) {
                    true => Ok(Target::Stores(nodeid)),
                    false => Err(Errno::ESTALE),
                };
            }
            _ => {}
        }
        let nodes = self.nodes.lock().unwrap();
        let looked = nodes.looked.get(&nodeid);
        let node = looked.map(|looked| looked.node.clone()).ok_or(Errno::ESTALE)?;
        Ok(Target::Node(self.slot_of(nodeid), node))
    }

    /// The node whose node ID is `nodeid`, as [`Self::target`] finds it, and its store's slot:
    /// `stores` if it is the root directory of stores, which the request can't be of.
    fn node(&self, nodeid: u64, stores: Errno) -> Result<(u16, Node)> {
        match self.target(nodeid)? {
            Target::Stores(_) => Err(stores),
            Target::Node(slot, node) => Ok((slot, node)),
        }
    }

    /// The filesystem in `slot`: `ENOTCONN` once it is detached.
    fn filesystem(&self, slot: u16) -> Result<Arc<Filesystem>> {
        let stores = self.stores.lock().unwrap();
        let slot = stores.slots.get(&slot).ok_or(Errno::ENOTCONN)?;
        slot.state.filesystem().cloned().ok_or(Errno::ENOTCONN)
    }

    /// The attributes of the directory of the server's own whose node ID is `dir`.
    fn stores_attr(&self, dir: u64) -> Attr {
        let Shape::Many { owner, made, .. } = &self.shape else {
            unreachable!("only a server of many stores has directories of its own")
        };
        Attr {
            inode: Inode(dir),
            size: 0,
            blocks: 0,
            atime: *made,
            mtime: *made,
            ctime: *made,
            file_type: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid: owner.uid,
            gid: owner.gid,
            rdev: 0,
            blksize: STORES_BLOCK_SIZE,
        }
    }

    /// Looks `name` up in the directory `dir`: a node of its store; or, in a directory of the
    /// server's own, another of those, or the root directory of a store attached there.
    async fn lookup_in(&self, dir: &Target, name: &[u8]) -> Result<Found> {
        match dir {
            Target::Node(slot, dir) => Ok(Found::Node(*slot, dir.lookup(name).await?)),
            Target::Stores(dir) => {
                let name = std::str::from_utf8(name).map_err(|_| Errno::ENOENT)?;
                let (slot, root) = {
                    let stores = self.stores.lock().unwrap();
                    let entries = &stores.dirs.get(dir).ok_or(Errno::ENOENT)?.entries;
                    let slot = match entries.get(name).ok_or(Errno::ENOENT)? {
                        Entry::Dir(sub) if stores.shows(*sub) => return Ok(Found::Stores(*sub)),
                        Entry::Dir(_) => return Err(Errno::ENOENT),
                        Entry::Store(slot) => *slot,
                    };
                    let State::Attached(fs) = &stores.slots[&slot].state else {
                        return Err(Errno::ENOENT);
                    };
                    (slot, fs.root().clone())
                };
                let attr = root.getattr().await?;
                Ok(Found::Node(slot, (root, attr)))
            }
        }
    }

    /// The entry of a node of the store in `slot` found or made at `name` in the directory
    /// `dir`, which the kernel holds a lookup of from the reply on.
    fn entry(&self, slot: u16, (dir, name): (u64, &[u8]), found: (Node, Attr)) -> Result<EntryOut> {
        let (node, attr) = found;
        let inode = self.inode(slot, attr.inode.0)?;
        let nodeid = self.swap(inode);
        let mut valid = TIMEOUT_SECS;
        if nodeid != ROOT_ID {
            let mut nodes = self.nodes.lock().unwrap();
            let looked = nodes.looked.entry(nodeid);
            looked.or_insert(Looked { node, lookups: 0, names: Vec::new() }).lookups += 1;
            if let Shape::Many { .. } = self.shape {
                match nodes.detaching.contains(&slot) {
                    true => valid = 0,
                    false => nodes.named(nodeid, (dir, name.into())),
                }
            }
        }
        Ok(entry_out(nodeid, inode, valid, &attr))
    }

    /// The kernel forgets `nlookup` of its lookups of the node `nodeid`: the last lets it go.
    fn forget(&self, nodeid: u64, nlookup: u64) {
        let mut nodes = self.nodes.lock().unwrap();
        let Some(looked) = nodes.looked.get_mut(&nodeid) else { return };
        looked.lookups = looked.lookups.saturating_sub(nlookup);
        if looked.lookups == 0 {
            let gone = nodes.looked.remove(&nodeid);
            for name in gone.iter().flat_map(|gone| &gone.names) {
                nodes.names.remove(name);
            }
            drop(nodes);
            // Letting a node go may free it: not under the table's lock.
            drop(gone);
            self.forgotten.notify(usize::MAX);
        }
    }

    fn batch_forget(&self, body: &mut Body<'_>) {
        let Ok(BatchForgetIn { count, .. }) = body.read() else { return };
        for _ in 0..count {
            let Ok(ForgetOne { nodeid, nlookup }) = body.read() else { return };
            self.forget(nodeid, nlookup);
        }
    }

    fn init(&self, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let init: InitIn = body.read()?;
        let flags2 = match init.flags as u64 & INIT_EXT {
            0 => 0,
            _ => body.read::<InitInExt>()?.flags2,
        };
        if init.major < KERNEL_VERSION {
            log::error!("fuse: unsupported protocol version {}.{}", init.major, init.minor);
            return Err(Errno::EPROTO);
        }
        if init.major > KERNEL_VERSION {
            // The kernel asks again with a version of the server's.
            let out = InitOut {
                major: KERNEL_VERSION,
                minor: KERNEL_MINOR_VERSION,
                ..Default::default()
            };
            return Ok(out.as_bytes().to_vec());
        }
        if init.minor < OLDEST_MINOR_VERSION {
            log::error!("fuse: unsupported protocol version {}.{}", init.major, init.minor);
            return Err(Errno::EPROTO);
        }
        // The mount is each filesystem's only writer, so the kernel may cache all it reads: not
        // AUTO_INVAL_DATA, which drops a file's cached pages when its modification time changes,
        // nor WRITEBACK_CACHE, which cost more than it saved (dead-ends/fuse-writeback-cache.md).
        // Opens are not sent (`OPEN` is refused), so not ATOMIC_O_TRUNC either: the kernel
        // truncates with `SETATTR`. Nor HANDLE_KILLPRIV_V2: the kernel kills setuid and setgid
        // itself, as for any filesystem, with a `SETATTR` of the mode, as it alone knows whether
        // its caller may keep them (CAP_FSETID), and does so for every change that kills them,
        // where with it a server is told of writes, truncations and chowns, not fallocate's
        // (TASK-149.37), and must remove security.capability too.
        let supported = ASYNC_READ
            | PARALLEL_DIROPS
            | BIG_WRITES
            | ASYNC_DIO
            | DO_READDIRPLUS
            | READDIRPLUS_AUTO
            | MAX_PAGES
            | CACHE_SYMLINKS
            | INIT_EXT;
        let enabled = (((flags2 as u64) << 32) | init.flags as u64) & supported;
        let max_pages = (MAX_WRITE as u64).div_ceil(crate::pages::page_size());
        let out = InitOut {
            major: KERNEL_VERSION,
            minor: KERNEL_MINOR_VERSION,
            max_readahead: init.max_readahead,
            flags: enabled as u32,
            max_background: u16::MAX,
            congestion_threshold: (u16::MAX / 4) * 3,
            max_write: MAX_WRITE,
            // Nanoseconds.
            time_gran: 1,
            max_pages: max_pages as u16,
            map_alignment: 0,
            flags2: (enabled >> 32) as u32,
            unused: [0; 7],
        };
        Ok(out.as_bytes().to_vec())
    }

    async fn lookup(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let [name] = strings(body.rest())?;
        let dir = self.target(header.nodeid)?;
        match self.lookup_in(&dir, name).await {
            Ok(Found::Node(slot, found)) => {
                Ok(self.entry(slot, (header.nodeid, name), found)?.as_bytes().to_vec())
            }
            // A directory of the server's own, which comes and goes as stores do: given for no
            // time, looked up again each time.
            Ok(Found::Stores(sub)) => {
                Ok(entry_out(sub, sub, 0, &self.stores_attr(sub)).as_bytes().to_vec())
            }
            // A missing name is an entry without a node, which the kernel caches, but in the
            // directories of stores, where a store may be attached at it any time.
            Err(Errno::ENOENT) => {
                let valid = match dir {
                    Target::Stores(_) => 0,
                    Target::Node(..) => TIMEOUT_SECS,
                };
                Ok(EntryOut { entry_valid: valid, ..Default::default() }.as_bytes().to_vec())
            }
            Err(errno) => Err(errno),
        }
    }

    async fn getattr(&self, header: &InHeader) -> Result<Vec<u8>> {
        let (attr, inode) = match self.target(header.nodeid)? {
            Target::Stores(dir) => (self.stores_attr(dir), dir),
            Target::Node(slot, node) => {
                let attr = node.getattr().await?;
                let inode = self.inode(slot, attr.inode.0)?;
                (attr, inode)
            }
        };
        Ok(attr_out(&attr, inode).as_bytes().to_vec())
    }

    async fn setattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let set: SetattrIn = body.read()?;
        let valid = |flag| set.valid & flag != 0;
        let time = |flag, now_flag, secs, nsecs| {
            Ok(if valid(now_flag) {
                Some(SetTime::Now)
            } else if valid(flag) {
                Some(SetTime::At(time(secs, nsecs)?))
            } else {
                None
            })
        };
        let set = SetAttr {
            perm: valid(FATTR_MODE).then_some(perm(set.mode)),
            uid: valid(FATTR_UID).then_some(set.uid),
            gid: valid(FATTR_GID).then_some(set.gid),
            size: valid(FATTR_SIZE).then_some(set.size),
            atime: time(FATTR_ATIME, FATTR_ATIME_NOW, set.atime, set.atimensec)?,
            mtime: time(FATTR_MTIME, FATTR_MTIME_NOW, set.mtime, set.mtimensec)?,
            // The kernel's to kill, with the mode it sends.
            kill_suid_sgid: false,
        };
        let (slot, node) = self.node(header.nodeid, Errno::EPERM)?;
        let attr = node.setattr(&set).await?;
        Ok(attr_out(&attr, self.inode(slot, attr.inode.0)?).as_bytes().to_vec())
    }

    async fn readlink(&self, header: &InHeader) -> Result<Vec<u8>> {
        self.node(header.nodeid, Errno::EINVAL)?.1.readlink().await
    }

    async fn fsync(&self, header: &InHeader) -> Result<Vec<u8>> {
        if let Target::Node(_, node) = self.target(header.nodeid)? {
            node.fsync().await?;
        }
        Ok(Vec::new())
    }

    /// The owner of a node the caller makes in the directory `dir`, and whether `dir`'s setgid is
    /// set, which a directory made in it takes too. Linux's VFS gives new inodes their owners
    /// (`inode_init_owner`), which FUSE leaves to the server: the caller's, but the group of a
    /// directory whose setgid is set. (Linux clears a new file's own setgid, if its caller isn't
    /// of its group, as it can't tell here, and leaves it.)
    async fn owner(&self, dir: &Node, header: &InHeader) -> Result<(Owner, bool)> {
        let attr = dir.getattr().await?;
        let setgid = attr.perm as u32 & S_ISGID != 0;
        let gid = if setgid { attr.gid } else { header.gid };
        Ok((Owner { uid: header.uid, gid }, setgid))
    }

    async fn symlink(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        // The target is a path, not a name.
        let [name, target] = strings(body.rest())?;
        let (slot, dir) = self.node(header.nodeid, Errno::EPERM)?;
        let (owner, _) = self.owner(&dir, header).await?;
        let made = dir.symlink(name, target, owner).await?;
        Ok(self.entry(slot, (header.nodeid, name), made)?.as_bytes().to_vec())
    }

    async fn mknod(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let MknodIn { mode, rdev, umask, .. } = body.read()?;
        let [name] = strings(body.rest())?;
        // A mode without a type is a regular file's.
        let file_type = FileType::from_mode(mode).unwrap_or(FileType::File);
        let (slot, dir) = self.node(header.nodeid, Errno::EPERM)?;
        let (owner, _) = self.owner(&dir, header).await?;
        let made = dir.mknod(name, file_type, perm(mode & !umask), rdev, owner).await?;
        Ok(self.entry(slot, (header.nodeid, name), made)?.as_bytes().to_vec())
    }

    async fn mkdir(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let MkdirIn { mode, umask } = body.read()?;
        let [name] = strings(body.rest())?;
        let (slot, dir) = self.node(header.nodeid, Errno::EPERM)?;
        let (owner, setgid) = self.owner(&dir, header).await?;
        let perm = perm(mode & !umask) | if setgid { S_ISGID as u16 } else { 0 };
        let made = dir.mkdir(name, perm, owner).await?;
        Ok(self.entry(slot, (header.nodeid, name), made)?.as_bytes().to_vec())
    }

    async fn create(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let CreateIn { mode, umask, .. } = body.read()?;
        let [name] = strings(body.rest())?;
        let (slot, dir) = self.node(header.nodeid, Errno::EPERM)?;
        let (owner, _) = self.owner(&dir, header).await?;
        let made = dir.create(name, perm(mode & !umask), owner).await?;
        let mut reply = self.entry(slot, (header.nodeid, name), made)?.as_bytes().to_vec();
        reply.extend_from_slice(open_out().as_bytes());
        Ok(reply)
    }

    /// Removes a name, as `rmdir` does if `directory`, and as `unlink` does otherwise.
    async fn remove(
        &self,
        header: &InHeader,
        body: &mut Body<'_>,
        directory: bool,
    ) -> Result<Vec<u8>> {
        let [name] = strings(body.rest())?;
        let (_, dir) = self.node(header.nodeid, Errno::EPERM)?;
        match directory {
            true => dir.rmdir(name).await?,
            false => dir.unlink(name).await?,
        }
        if let Shape::Many { .. } = self.shape {
            self.nodes.lock().unwrap().unnamed(&(header.nodeid, name.into()));
        }
        Ok(Vec::new())
    }

    async fn rename(
        &self,
        header: &InHeader,
        body: &mut Body<'_>,
        with_flags: bool,
    ) -> Result<Vec<u8>> {
        let (new_dir, flags) = match with_flags {
            true => {
                let Rename2In { newdir, flags, .. } = body.read()?;
                (newdir, flags)
            }
            false => (body.read::<RenameIn>()?.newdir, 0),
        };
        let [name, new_name] = strings(body.rest())?;
        let flags = RenameFlags::from_bits_retain(flags);
        // Nodes of two stores are refused by their filesystem (`EXDEV`).
        match (self.target(header.nodeid)?, self.target(new_dir)?) {
            (Target::Node(_, dir), Target::Node(_, to)) => {
                dir.rename(name, &to, new_name, flags).await?;
                if let Shape::Many { .. } = self.shape {
                    let (from, to) = ((header.nodeid, name.into()), (new_dir, new_name.into()));
                    let exchange = flags.contains(RenameFlags::EXCHANGE);
                    self.nodes.lock().unwrap().renamed(from, to, exchange);
                }
            }
            (Target::Stores(_), Target::Stores(_)) => return Err(Errno::EPERM),
            _ => return Err(Errno::EXDEV),
        }
        Ok(Vec::new())
    }

    async fn link(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let LinkIn { oldnodeid } = body.read()?;
        let [new_name] = strings(body.rest())?;
        let (_, node) = self.node(oldnodeid, Errno::EPERM)?;
        // A node of another store is refused by its filesystem (`EXDEV`).
        let (slot, dir) = self.node(header.nodeid, Errno::EXDEV)?;
        let attr = node.link(&dir, new_name).await?;
        Ok(self.entry(slot, (header.nodeid, new_name), (node, attr))?.as_bytes().to_vec())
    }

    async fn read(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let ReadIn { offset, size, .. } = body.read()?;
        if size > MAX_WRITE {
            return Err(Errno::EINVAL);
        }
        let mut data = vec![0; size as usize];
        let read = self.node(header.nodeid, Errno::EISDIR)?.1.read(offset, &mut data).await?;
        data.truncate(read);
        Ok(data)
    }

    async fn write(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let WriteIn { offset, size, .. } = body.read()?;
        if size > MAX_WRITE {
            return Err(Errno::EINVAL);
        }
        let data = body.take(size as usize)?;
        let (_, file) = self.node(header.nodeid, Errno::EISDIR)?;
        let written = file.write(offset, data).await?;
        Ok(WriteOut { size: written as u32, padding: 0 }.as_bytes().to_vec())
    }

    /// The usage of the filesystem the node is of; none of the root directory of stores', which
    /// holds no blocks of its own.
    async fn statfs(&self, header: &InHeader) -> Result<Vec<u8>> {
        let out = match self.target(header.nodeid)? {
            Target::Stores(_) => Kstatfs {
                bsize: STORES_BLOCK_SIZE,
                frsize: STORES_BLOCK_SIZE,
                namelen: crate::filesystem::NAME_MAX as u32,
                ..Default::default()
            },
            Target::Node(slot, _) => {
                let statfs = self.filesystem(slot)?.statfs().await?;
                Kstatfs {
                    blocks: statfs.blocks,
                    bfree: statfs.blocks_free,
                    bavail: statfs.blocks_free,
                    bsize: statfs.block_size,
                    namelen: statfs.name_max,
                    frsize: statfs.block_size,
                    ..Default::default()
                }
            }
        };
        Ok(out.as_bytes().to_vec())
    }

    async fn setxattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let SetxattrIn { size, flags } = body.read()?;
        let mode = match flags {
            0 => XattrMode::Set,
            XATTR_CREATE => XattrMode::Create,
            XATTR_REPLACE => XattrMode::Replace,
            _ => return Err(Errno::EINVAL),
        };
        // The name, then the value, which ends the body.
        let rest = body.rest();
        let end = rest.iter().position(|&byte| byte == 0).ok_or(Errno::EINVAL)?;
        let (name, value) = (&rest[..end], &rest[end + 1..]);
        if value.len() != size as usize {
            return Err(Errno::EINVAL);
        }
        let name = xattr_name(name)?;
        self.node(header.nodeid, Errno::EPERM)?.1.setxattr(name, value, mode).await?;
        Ok(Vec::new())
    }

    async fn getxattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let GetxattrIn { size, .. } = body.read()?;
        let [name] = strings(body.rest())?;
        let name = xattr_name(name)?;
        let value = self.node(header.nodeid, Errno::ENODATA)?.1.getxattr(name).await?;
        sized(value, size)
    }

    async fn listxattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let GetxattrIn { size, .. } = body.read()?;
        let names = match self.target(header.nodeid)? {
            Target::Stores(_) => Vec::new(),
            Target::Node(_, node) => node.listxattr().await?,
        };
        // Each name NUL-terminated, as listxattr(2) lists them.
        let list = names.into_iter().flat_map(|name| name.into_iter().chain([0])).collect();
        sized(list, size)
    }

    async fn removexattr(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let [name] = strings(body.rest())?;
        let name = xattr_name(name)?;
        self.node(header.nodeid, Errno::EPERM)?.1.removexattr(name).await?;
        Ok(Vec::new())
    }

    async fn opendir(&self, header: &InHeader) -> Result<Vec<u8>> {
        let (open, open_flags) = match self.target(header.nodeid)? {
            // Stores come and go: the kernel caches none of its entries.
            Target::Stores(dir) => (self.open_stores(dir)?, 0),
            Target::Node(slot, dir) => {
                let open = self.open_dir(header.nodeid, slot, dir).await?;
                // The mount is the directory's only writer, so the kernel may cache its entries.
                (open, FOPEN_KEEP_CACHE | FOPEN_CACHE_DIR)
            }
        };
        let mut dirs = self.dirs.lock().unwrap();
        let fh = dirs.next;
        dirs.next += 1;
        dirs.open.insert(fh, Arc::new(open));
        Ok(OpenOut { fh, open_flags, padding: 0 }.as_bytes().to_vec())
    }

    /// The directory of the server's own whose node ID is `dir`, opened: its entries the
    /// directories of the server's in it and the stores attached there. `ESTALE` once it is gone.
    fn open_stores(&self, dir: u64) -> Result<OpenDir> {
        let stores = self.stores.lock().unwrap();
        let of = stores.dirs.get(&dir).ok_or(Errno::ESTALE)?;
        let entries = (of.entries.iter())
            .filter_map(|(name, entry)| {
                let inode = match *entry {
                    Entry::Dir(sub) if stores.shows(sub) => sub,
                    Entry::Dir(_) => return None,
                    Entry::Store(slot) => {
                        let State::Attached(fs) = &stores.slots[&slot].state else { return None };
                        // Object IDs too large fail as they are looked up.
                        self.inode(slot, fs.root().inode().0).unwrap_or(0)
                    }
                };
                let name = name.clone().into_bytes();
                Some(DirEntry { inode: Inode(inode), file_type: FileType::Directory, name })
            })
            .collect();
        let target = Target::Stores(dir);
        Ok(OpenDir { dir: target, nodeid: dir, inode: dir, parent: of.parent, entries })
    }

    /// The directory `dir` of the store in `slot`, whose node ID is `nodeid`, opened.
    async fn open_dir(&self, nodeid: u64, slot: u16, dir: Node) -> Result<OpenDir> {
        let mut entries = dir.entries().await?;
        for entry in &mut entries {
            entry.inode = Inode(self.inode(slot, entry.inode.0)?);
        }
        let inode = self.inode(slot, dir.inode().0)?;
        let parent = match dir.parent()? {
            // A store's root, which is its own parent, is in a directory of the server's own.
            parent if parent == dir.inode() && matches!(self.shape, Shape::Many { .. }) => {
                let stores = self.stores.lock().unwrap();
                stores.slots.get(&slot).map_or(ROOT_ID, |slot| slot.dir)
            }
            parent => self.inode(slot, parent.0)?,
        };
        Ok(OpenDir { dir: Target::Node(slot, dir), nodeid, inode, parent, entries })
    }

    /// Lists the open directory's entries from the offset asked, as many as fit in the size asked,
    /// each with its offset, the offset to list the next from: `.`, `..`, then those the
    /// directory held as it was opened. With `plus`, each has its node's entry, and a lookup of
    /// it, but `.` and `..`, which the kernel takes no lookups of, and entries removed since the
    /// directory was opened are left out.
    async fn readdir(&self, body: &mut Body<'_>, plus: bool) -> Result<Vec<u8>> {
        let ReadIn { fh, offset, size, .. } = body.read()?;
        if size > MAX_WRITE {
            return Err(Errno::EINVAL);
        }
        let open = self.dirs.lock().unwrap().open.get(&fh).cloned().ok_or(Errno::EBADF)?;
        let dots = [(&b"."[..], open.inode), (b"..", open.parent)].map(|(name, inode)| DirEntry {
            inode: Inode(inode),
            file_type: FileType::Directory,
            name: name.to_vec(),
        });
        let listed = dots.iter().chain(&open.entries).enumerate().skip(offset as usize);
        let mut reply = Vec::new();
        for (index, entry) in listed {
            let dirent_len = (size_of::<Dirent>() + entry.name.len()).next_multiple_of(8);
            let len = dirent_len + if plus { size_of::<EntryOut>() } else { 0 };
            if reply.len() + len > size as usize {
                break;
            }
            let mut entry = entry.clone();
            if plus {
                let found = match index {
                    // The kernel takes these from its own dentries.
                    0 | 1 => EntryOut::default(),
                    _ => match self.lookup_in(&open.dir, &entry.name).await {
                        Ok(Found::Stores(sub)) => entry_out(sub, sub, 0, &self.stores_attr(sub)),
                        Ok(Found::Node(slot, found)) => {
                            match self.entry(slot, (open.nodeid, &entry.name), found) {
                                Ok(found) => {
                                    entry.inode = Inode(found.attr.ino);
                                    found
                                }
                                Err(_) if !reply.is_empty() => break,
                                Err(errno) => return Err(errno),
                            }
                        }
                        // Removed since the directory was opened.
                        Err(Errno::ENOENT) => continue,
                        // The entries so far took lookups, which the kernel forgets only if it
                        // gets them.
                        Err(_) if !reply.is_empty() => break,
                        Err(errno) => return Err(errno),
                    },
                };
                reply.extend_from_slice(found.as_bytes());
            }
            push_dirent(&mut reply, &entry, index as u64 + 1, dirent_len);
        }
        Ok(reply)
    }

    fn releasedir(&self, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let ReleaseIn { fh, .. } = body.read()?;
        let closed = self.dirs.lock().unwrap().open.remove(&fh);
        // Its handle on the directory goes: not under the table's lock.
        drop(closed);
        Ok(Vec::new())
    }

    async fn fallocate(&self, header: &InHeader, body: &mut Body<'_>) -> Result<Vec<u8>> {
        let FallocateIn { offset, length, mode, .. } = body.read()?;
        // Only the default mode, 0, which allocates.
        if mode != 0 {
            return Err(Errno::ENOTSUP);
        }
        self.node(header.nodeid, Errno::EISDIR)?.1.allocate(offset, length).await?;
        Ok(Vec::new())
    }
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stores = self.stores.lock().unwrap();
        let names: Vec<&str> = stores.slots.values().map(|slot| slot.name.as_str()).collect();
        f.debug_struct("Server").field("stores", &names).finish_non_exhaustive()
    }
}

/// A slot and name taken for a store being opened ([`Server::reserve`]): given back unless it is
/// attached.
struct Reserved<'a> {
    server: &'a Server,
    slot: Option<u16>,
}

impl Reserved<'_> {
    /// Attaches `fs` in the slot: `ENOTCONN` if the server serves no more.
    fn attach(mut self, fs: Arc<Filesystem>) -> Result<()> {
        let mut stores = self.server.stores.lock().unwrap();
        if stores.closed {
            return Err(Errno::ENOTCONN);
        }
        let slot = self.slot.take().expect("attached once");
        stores.slots.get_mut(&slot).expect("a reserved slot").state = State::Attached(fs);
        Ok(())
    }
}

impl Drop for Reserved<'_> {
    fn drop(&mut self) {
        let Some(slot) = self.slot else { return };
        self.server.stores.lock().unwrap().remove(slot);
    }
}

/// A notification that the kernel drop the entry of `name` in the directory `parent`.
fn inval_entry(parent: u64, name: &[u8]) -> Vec<u8> {
    let body = NotifyInvalEntryOut { parent, namelen: name.len() as u32, flags: 0 };
    let len = size_of::<OutHeader>() + size_of::<NotifyInvalEntryOut>() + name.len() + 1;
    let header = OutHeader { len: len as u32, error: notify::INVAL_ENTRY, unique: 0 };
    let mut message = Vec::with_capacity(len);
    message.extend_from_slice(header.as_bytes());
    message.extend_from_slice(body.as_bytes());
    message.extend_from_slice(name);
    message.push(0);
    message
}

/// What remains of a request's body, read from its front.
struct Body<'a>(&'a [u8]);

impl<'a> Body<'a> {
    /// The message at the front: `EINVAL` if too little remains.
    fn read<T: FromBytes>(&mut self) -> Result<T> {
        let (message, rest) = T::read_from_prefix(self.0).map_err(|_| Errno::EINVAL)?;
        self.0 = rest;
        Ok(message)
    }

    /// The `len` bytes at the front: `EINVAL` if fewer remain.
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(len).ok_or(Errno::EINVAL)?;
        self.0 = rest;
        Ok(taken)
    }

    /// What remains.
    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.0)
    }
}

/// A reply to the request `unique`.
struct Reply {
    unique: u64,
}

impl Reply {
    fn ok(&self, payload: &[u8]) -> Vec<u8> {
        let mut reply = self.header(payload.len(), 0);
        reply.extend_from_slice(payload);
        reply
    }

    fn error(&self, errno: Errno) -> Vec<u8> {
        self.header(0, -errno.raw())
    }

    fn header(&self, payload_len: usize, error: i32) -> Vec<u8> {
        let len = (size_of::<OutHeader>() + payload_len) as u32;
        OutHeader { len, error, unique: self.unique }.as_bytes().to_vec()
    }
}

/// The `N` NUL-terminated strings that make up `body`, which holds no more: `EINVAL` otherwise.
fn strings<const N: usize>(body: &[u8]) -> Result<[&[u8]; N]> {
    let mut strings = body.split_inclusive(|&byte| byte == 0);
    let found: [&[u8]; N] = std::array::from_fn(|_| strings.next().unwrap_or_default());
    (found.iter().all(|string| string.last() == Some(&0)) && strings.next().is_none())
        .then(|| found.map(|string| &string[..string.len() - 1]))
        .ok_or(Errno::EINVAL)
}

/// The namespaces of extended attributes served: those the guest's Fxfs has handlers of. POSIX
/// ACLs' (`system.posix_acl_*`) the kernel refuses itself, as INIT doesn't offer `POSIX_ACL`.
const XATTR_NAMESPACES: [&[u8]; 3] = [b"user.", b"trusted.", b"security."];

/// `name`, an extended attribute's, of a namespace served, as Linux's VFS resolves a name to its
/// filesystem's handler (`xattr_resolve_name`), which FUSE leaves to its servers: `EOPNOTSUPP` of
/// another namespace, `EINVAL` for a namespace's prefix alone.
fn xattr_name(name: &[u8]) -> Result<&[u8]> {
    match XATTR_NAMESPACES.iter().find(|prefix| name.starts_with(prefix)) {
        Some(prefix) if name.len() == prefix.len() => Err(Errno::EINVAL),
        Some(_) => Ok(name),
        None => Err(Errno::ENOTSUP),
    }
}

/// `value` for a request that asked for at most `size` bytes of it, or, asking for 0, its size:
/// `ERANGE` if it is longer, as getxattr(2) and listxattr(2) have it.
fn sized(value: Vec<u8>, size: u32) -> Result<Vec<u8>> {
    match size {
        0 => Ok(GetxattrOut { size: value.len() as u32, padding: 0 }.as_bytes().to_vec()),
        size if value.len() > size as usize => Err(Errno::ERANGE),
        _ => Ok(value),
    }
}

/// The permission bits of `mode`.
fn perm(mode: u32) -> u16 {
    (mode & 0o7777) as u16
}

/// The time `secs` and `nsecs` after the epoch. The kernel's seconds are signed, and times before
/// the epoch, which Fxfs cannot store, are invalid, as are nanoseconds of a second or more.
fn time(secs: u64, nsecs: u32) -> Result<SystemTime> {
    if (secs as i64) < 0 || nsecs >= 1_000_000_000 {
        return Err(Errno::EINVAL);
    }
    UNIX_EPOCH.checked_add(Duration::new(secs, nsecs)).ok_or(Errno::EINVAL)
}

/// The seconds and nanoseconds of `time` since the epoch, which fxdax's times are not before.
fn timespec(time: SystemTime) -> (u64, u32) {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    (since_epoch.as_secs(), since_epoch.subsec_nanos())
}

/// The kernel's attributes of a node whose are `attr`, its inode number `inode`.
fn fuse_attr(attr: &Attr, inode: u64) -> super::abi::Attr {
    let (atime, atimensec) = timespec(attr.atime);
    let (mtime, mtimensec) = timespec(attr.mtime);
    let (ctime, ctimensec) = timespec(attr.ctime);
    super::abi::Attr {
        ino: inode,
        size: attr.size,
        blocks: attr.blocks,
        atime,
        mtime,
        ctime,
        atimensec,
        mtimensec,
        ctimensec,
        mode: attr.mode(),
        nlink: attr.nlink,
        uid: attr.uid,
        gid: attr.gid,
        rdev: attr.rdev,
        blksize: attr.blksize,
        flags: 0,
    }
}

/// The entry of the node `nodeid`, whose attributes are `attr`, its inode number `inode`, valid
/// for `valid` seconds.
fn entry_out(nodeid: u64, inode: u64, valid: u64, attr: &Attr) -> EntryOut {
    EntryOut {
        nodeid,
        generation: 0,
        entry_valid: valid,
        attr_valid: TIMEOUT_SECS,
        entry_valid_nsec: 0,
        attr_valid_nsec: 0,
        attr: fuse_attr(attr, inode),
    }
}

fn attr_out(attr: &Attr, inode: u64) -> AttrOut {
    let attr = fuse_attr(attr, inode);
    AttrOut { attr_valid: TIMEOUT_SECS, attr_valid_nsec: 0, dummy: 0, attr }
}

/// A created file's open: it needs no handle, nor flushing as it is closed, and its cached pages
/// stay valid, as the mount is the file's only writer.
fn open_out() -> OpenOut {
    OpenOut { fh: 0, open_flags: FOPEN_KEEP_CACHE | FOPEN_NOFLUSH, padding: 0 }
}

/// Appends `entry`'s dirent, whose next entry is at `next_offset`, padded to `dirent_len`.
fn push_dirent(reply: &mut Vec<u8>, entry: &DirEntry, next_offset: u64, dirent_len: usize) {
    let start = reply.len();
    let dirent = Dirent {
        ino: entry.inode.0,
        off: next_offset,
        namelen: entry.name.len() as u32,
        type_: entry.file_type.dirent_type().into(),
    };
    reply.extend_from_slice(dirent.as_bytes());
    reply.extend_from_slice(&entry.name);
    reply.resize(start + dirent_len, 0);
}

#[cfg(test)]
mod fuzz_tests;

#[cfg(test)]
mod stores_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    /// The root's object ID and FUSE's root ID swap places as node IDs, and each other object ID
    /// is its own: whatever Fxfs numbers 1 has the root's object ID for its node ID, so no two
    /// nodes share one.
    #[test]
    fn the_root_and_fuses_root_id_swap_places() {
        let fs = block_on(Filesystem::format(crate::Storage::memory(8 << 20))).unwrap();
        let server = Server::new(fs);
        let root = server.root();
        assert_eq!((server.swap(root), server.swap(ROOT_ID)), (ROOT_ID, root));
        for id in [ROOT_ID, root, root + 1, 1 << 40] {
            assert_eq!(server.swap(server.swap(id)), id);
        }
        block_on(server.into_filesystem().close()).unwrap();
    }
}
