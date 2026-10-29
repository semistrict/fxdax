// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A mounted volume: an Fxfs volume of an image, which a [`Filesystem`](crate::Filesystem) serves
//! through Fxfs's POSIX layer (crates/fxfs-posix), and the nodes that handles to them
//! ([`Node`](crate::Node)s) hold, as a VFS holds the inodes its files are of.
//!
//! The POSIX layer is the guest kernel's too, whose VFS checks what it can before calling it,
//! keeps its inodes, and locks its directories' names while it changes them. The host's
//! filesystem does as the VFS does ([`HeldNode`], [`Volume::namespace`]).
//!
//! A volume is mounted until its unmount begins, and from then on every operation on it fails with
//! `ENOTCONN`. Operations run on Fxfs's executor ([`Volume::run`]), where they complete whoever
//! waits for them; the unmount waits for those under way as it begins, then closes the volume
//! there, and writes the filesystem back. Whichever begins it, of the filesystem's
//! [`unmount`](crate::Filesystem::unmount), its drop, or its image's close, the others wait for
//! that one unmount.

use crate::error::{Errno, Error, Result};
use crate::image::ImageShared;
use crate::node::FileType;
use event_listener::Event;
use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future::Shared;
use fxfs::filesystem::SyncOptions;
use fxfs::object_store::ObjectStore;
use fxfs_posix::{Fxfs, Platform};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};

/// What the host gives Fxfs's POSIX layer: the host's VFS keeps nothing the layer writes back,
/// and its devices make each commit durable as they write it.
pub(crate) struct Host;

impl Platform for Host {
    const KEEPS_DIRECTORY_TIMES: bool = false;

    type Durable = ();

    fn durable(&self) {}

    fn spawn_urgent(&self, future: impl Future<Output = ()> + Send + 'static) {
        fuchsia_async::Task::spawn(future).detach();
    }
}

/// A volume of Fxfs, as the host serves it.
pub(crate) type HostFxfs = Fxfs<Host>;

/// A mounted volume of an image. See the module docs.
pub(crate) struct Volume {
    /// The image whose volume it is.
    pub image: Arc<ImageShared>,
    /// The object ID of its store in the image.
    store_id: u64,
    /// Locked while a name in the volume changes, and shared while one is looked up, as a VFS
    /// locks a directory's names: an operation checks what a name holds, then changes it, with
    /// nothing changing it in between, and a lookup holds the node it finds before the name can
    /// go, whose node would go with it if nothing held it.
    pub namespace: tokio::sync::RwLock<()>,
    /// The operations under way, which the unmount waits for.
    operations: Arc<Operations>,
    state: Mutex<State>,
}

struct State {
    /// The volume and its nodes held, until the unmount, the operations under way as it began
    /// done, takes them.
    mounted: Option<Mounted>,
    /// Once the unmount has begun: completes once it is unmounted.
    unmounting: Option<Shared<oneshot::Receiver<()>>>,
}

impl State {
    /// The volume, while it is mounted: until the unmount begins.
    fn mounted(&mut self) -> Result<&mut Mounted> {
        match self.unmounting {
            None => self.open(),
            Some(_) => Err(Errno::ENOTCONN),
        }
    }

    /// The volume, for the operations under way: until the unmount takes it.
    fn open(&mut self) -> Result<&mut Mounted> {
        self.mounted.as_mut().ok_or(Errno::ENOTCONN)
    }
}

/// A mounted volume, and its nodes that handles hold.
struct Mounted {
    fxfs: Arc<HostFxfs>,
    /// The nodes held, by object ID.
    nodes: HashMap<u64, HeldNode>,
}

/// A node that handles hold: an inode of the volume's VFS. Fxfs's POSIX layer keeps what it
/// opened of it until it is let go of (`Fxfs::evict`), which frees it if no name links it.
pub(crate) struct HeldNode {
    /// The node's type, which never changes.
    pub file_type: FileType,
    /// Whether no name links it any more: it is freed once it is let go of.
    pub unlinked: bool,
    /// The directory a directory is in, which it holds, so that a rename can find whether it
    /// would move a directory into itself.
    parent: Option<u64>,
    /// How many holds there are on it: handles' holds, and the directories in it that are held.
    holds: u64,
    children: u64,
}

/// A handle's hold on one of a volume's nodes, released as it is dropped. It keeps the volume
/// no more than the node: once the volume's filesystem is gone, it holds nothing.
pub(crate) struct Hold {
    volume: Weak<Volume>,
    object_id: u64,
}

impl Hold {
    /// The object ID of the node held.
    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    /// The volume of the node held, unless its filesystem is gone (`ENOTCONN`).
    pub fn volume(&self) -> Result<Arc<Volume>> {
        self.volume.upgrade().ok_or(Errno::ENOTCONN)
    }

    /// Whether `other` holds a node of the same volume.
    pub fn same_volume(&self, other: &Hold) -> bool {
        Weak::ptr_eq(&self.volume, &other.volume)
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(volume) = self.volume.upgrade() {
            volume.release(self.object_id);
        }
    }
}

/// The operations under way on a volume, which its unmount waits for.
#[derive(Default)]
struct Operations {
    running: Mutex<usize>,
    /// Notified as the last ends.
    idle: Event,
}

impl Operations {
    /// Waits until none is under way. None begins once the unmount has begun.
    async fn idle(&self) {
        loop {
            let idle = self.idle.listen();
            if *self.running.lock().unwrap() == 0 {
                return;
            }
            idle.await;
        }
    }
}

/// An operation under way, until it is dropped.
struct Operation(Arc<Operations>);

impl Drop for Operation {
    fn drop(&mut self) {
        let mut running = self.0.running.lock().unwrap();
        *running -= 1;
        if *running == 0 {
            self.0.idle.notify(usize::MAX);
        }
    }
}

impl Volume {
    /// The volume of `image` whose store is `store`, just mounted, and the hold on its root
    /// directory.
    pub fn new(image: Arc<ImageShared>, store: Arc<ObjectStore>) -> (Arc<Self>, Hold) {
        let (store_id, object_id) = (store.store_object_id(), store.root_directory_object_id());
        let fxfs = HostFxfs::new(store.filesystem(), store, false, Host);
        let held = HeldNode {
            file_type: FileType::Directory,
            unlinked: false,
            parent: None,
            holds: 1,
            children: 0,
        };
        let mounted = Mounted { fxfs: Arc::new(fxfs), nodes: HashMap::from([(object_id, held)]) };
        let volume = Arc::new(Self {
            image,
            store_id,
            namespace: tokio::sync::RwLock::new(()),
            operations: Arc::default(),
            state: Mutex::new(State { mounted: Some(mounted), unmounting: None }),
        });
        let root = Hold { volume: Arc::downgrade(&volume), object_id };
        (volume, root)
    }

    /// The size of the blocks of the volume's image.
    pub fn block_size(&self) -> u32 {
        self.image.block_size
    }

    /// Whether the volume is mounted: its unmount hasn't begun.
    pub fn is_mounted(&self) -> bool {
        self.state.lock().unwrap().unmounting.is_none()
    }

    /// Runs the future `f` makes of the volume's Fxfs on Fxfs's executor
    /// ([`Executor::run`](crate::executor::Executor::run)), which runs it to completion whether or
    /// not the caller waits for it, and the unmount waits for. It fails with `ENOTCONN` once the
    /// unmount has begun.
    pub async fn run<T, F>(&self, f: impl FnOnce(Arc<HostFxfs>) -> F) -> Result<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T>> + Send + 'static,
    {
        let (fxfs, operation) = {
            let mut state = self.state.lock().unwrap();
            let fxfs = state.mounted()?.fxfs.clone();
            *self.operations.running.lock().unwrap() += 1;
            (fxfs, Operation(self.operations.clone()))
        };
        let future = f(fxfs);
        self.image
            .executor
            .run(async move {
                // The future, and the volume's Fxfs it holds, go before the operation ends.
                let output = future.await;
                drop(operation);
                output
            })
            .await
    }

    /// The volume's Fxfs, until the unmount begins.
    #[cfg(test)]
    pub fn fxfs(&self) -> Result<Arc<HostFxfs>> {
        Ok(self.state.lock().unwrap().mounted()?.fxfs.clone())
    }

    /// A new hold on the node `object_id` of `file_type`, until the unmount begins (`ENOTCONN`),
    /// found in the directory `parent`, which is held.
    pub fn hold(
        self: &Arc<Self>,
        object_id: u64,
        file_type: FileType,
        parent: Option<u64>,
    ) -> Result<Hold> {
        let mut state = self.state.lock().unwrap();
        let mounted = state.open()?;
        let held = mounted.nodes.entry(object_id).or_insert_with(|| HeldNode {
            file_type,
            unlinked: false,
            parent: None,
            holds: 0,
            children: 0,
        });
        held.holds += 1;
        // A directory held holds the directory it is in, which it was found in.
        let new = held.holds == 1 && held.children == 0;
        if new && held.file_type == FileType::Directory {
            held.parent = parent;
            if let Some(parent) = parent {
                mounted.held(parent).children += 1;
            }
        }
        Ok(Hold { volume: Arc::downgrade(self), object_id })
    }

    /// Runs `f` on the node that `hold`, one of the volume's holds, holds, until the unmount
    /// begins (`ENOTCONN`).
    pub fn with_held<T>(&self, hold: &Hold, f: impl FnOnce(&HeldNode) -> T) -> Result<T> {
        let mut state = self.state.lock().unwrap();
        Ok(f(state.mounted()?.held(hold.object_id)))
    }

    /// Whether no name links the node that `hold` holds any more, for an operation under way:
    /// until the unmount takes the nodes held (`ENOTCONN`).
    pub fn is_unlinked(&self, hold: &Hold) -> Result<bool> {
        let mut state = self.state.lock().unwrap();
        let mounted = state.open()?;
        Ok(mounted.nodes.get(&hold.object_id).is_some_and(|held| held.unlinked))
    }

    /// The held node `object_id`, which no name links any more, removed as `replaced` says
    /// (`fxfs_posix::REPLACED_*`): freed at once unless it is held, and once it is let go of
    /// otherwise. A file another name links is let be.
    pub async fn unlinked(&self, fxfs: &HostFxfs, object_id: u64, replaced: u32) -> Result<()> {
        let gone = match replaced {
            fxfs_posix::REPLACED_DIRECTORY => true,
            fxfs_posix::REPLACED_OBJECT => match fxfs.in_graveyard(object_id).await {
                Ok(in_graveyard) => in_graveyard,
                Err(error) => return Err(Errno::of_fxfs(&error)),
            },
            _ => false,
        };
        if !gone {
            return Ok(());
        }
        // Under the lock that holds take: a node found by a lookup before its name went is held
        // already, as lookups hold what they find before the name can go (`namespace`).
        let mut state = self.state.lock().unwrap();
        match state.open()?.nodes.get_mut(&object_id) {
            Some(held) => held.unlinked = true,
            None => fxfs.evict(object_id, true),
        }
        Ok(())
    }

    /// Moves the directory `object_id`, if it is held, into the directory `parent`, held too.
    pub fn moved(&self, object_id: u64, parent: u64) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let mounted = state.open()?;
        let Some(held) = mounted.nodes.get_mut(&object_id) else { return Ok(()) };
        let Some(old) = held.parent.replace(parent) else { return Ok(()) };
        mounted.held(parent).children += 1;
        mounted.release_child(old);
        Ok(())
    }

    /// Whether the directory `directory`, which is held, is `ancestor` or in it, however deep.
    pub fn is_within(&self, directory: u64, ancestor: u64) -> Result<bool> {
        let mut state = self.state.lock().unwrap();
        let mounted = state.open()?;
        let mut at = Some(directory);
        while let Some(directory) = at {
            if directory == ancestor {
                return Ok(true);
            }
            at = mounted.nodes.get(&directory).and_then(|held| held.parent);
        }
        Ok(false)
    }

    /// Releases a hold on the node `object_id`. The last lets it go, unless the unmount has taken
    /// it.
    fn release(&self, object_id: u64) {
        let mut state = self.state.lock().unwrap();
        let Ok(mounted) = state.open() else {
            return;
        };
        let held = mounted.held(object_id);
        held.holds -= 1;
        if held.holds == 0 && held.children == 0 {
            mounted.let_go(object_id);
        }
    }

    /// Begins unmounting the volume, unless that has begun: waits for the operations under way,
    /// lets go of the nodes held, closes the volume, and writes the filesystem back, as Fxfs's
    /// volumes do as they terminate. The unmount runs on Fxfs's executor, where it completes even
    /// if no one waits for it.
    pub fn begin_unmount(self: &Arc<Self>) -> Unmount {
        let mut state = self.state.lock().unwrap();
        if let Some(unmounted) = &state.unmounting {
            return Unmount::Waits(unmounted.clone());
        }
        let (done, unmounted) = oneshot::channel();
        state.unmounting = Some(unmounted.shared());
        drop(state);
        let (result, began) = oneshot::channel();
        let volume = self.clone();
        self.image.executor.spawn(async move {
            let unmounted = async {
                volume.operations.idle().await;
                let Mounted { fxfs, nodes } =
                    volume.state.lock().unwrap().mounted.take().expect("unmounted once");
                let fxfs = Arc::into_inner(fxfs).expect("an operation holds the volume after it");
                // What no name links any more, the nodes held freed now: nothing holds them now.
                for (object_id, held) in nodes {
                    fxfs.evict(object_id, held.unlinked);
                }
                let filesystem = fxfs.filesystem().clone();
                let Host = fxfs.close().await;
                filesystem.graveyard().flush().await;
                filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await
            }
            .await;
            volume.image.unmounted(volume.store_id);
            // Its beginner may have given up on it.
            let _ = result.send(unmounted);
            let _ = done.send(());
        });
        Unmount::Began(began)
    }

    /// The nodes held, by object ID, and how many handles' holds there are on each.
    #[cfg(test)]
    pub fn held(&self) -> std::collections::BTreeMap<u64, u64> {
        match self.state.lock().unwrap().mounted() {
            Ok(mounted) => {
                mounted.nodes.iter().map(|(object_id, held)| (*object_id, held.holds)).collect()
            }
            Err(_) => Default::default(),
        }
    }
}

impl Mounted {
    /// The node `object_id`, which is held.
    fn held(&mut self, object_id: u64) -> &mut HeldNode {
        self.nodes.get_mut(&object_id).expect("a node is held while a hold on it lives")
    }

    /// Releases a held directory's hold on `parent`, the directory it is in.
    fn release_child(&mut self, parent: u64) {
        let held = self.held(parent);
        held.children -= 1;
        if held.holds == 0 && held.children == 0 {
            self.let_go(parent);
        }
    }

    /// Lets go of the node `object_id`, which nothing holds any more, and of the directory it is
    /// in, if that held it alone.
    fn let_go(&mut self, object_id: u64) {
        let held = self.nodes.remove(&object_id).expect("a node let go of is held");
        self.fxfs.evict(object_id, held.unlinked);
        if let Some(parent) = held.parent {
            self.release_child(parent);
        }
    }
}

/// An unmount of a volume, which [`Volume::begin_unmount`] began, or found under way.
pub(crate) enum Unmount {
    /// Begun by the call that gave it: completes with its result.
    Began(oneshot::Receiver<anyhow::Result<()>>),
    /// Under way already: completes once the volume is unmounted.
    Waits(Shared<oneshot::Receiver<()>>),
}

impl Unmount {
    /// Whether the call that gave it began the unmount.
    pub fn began(&self) -> bool {
        matches!(self, Self::Began(_))
    }

    /// Waits until the volume is unmounted. Only the unmount's beginner is given its error.
    pub async fn wait(self) -> Result<(), Error> {
        match self {
            Self::Began(result) => {
                result.await.expect("the executor dropped a future").map_err(Error::new)
            }
            Self::Waits(unmounted) => {
                // The sender is dropped unsent if the unmount panicked: done either way.
                let _ = unmounted.await;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Errno, Filesystem, Storage};
    use futures::executor::block_on;

    /// The unmount a volume's first call begins gives its result; later ones wait for it, and the
    /// volume is unmounted from the first.
    #[test]
    fn the_first_unmount_gives_its_result_and_later_ones_wait() {
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let volume = fs.volume().clone();
            let first = volume.image.unmount_volume(&volume);
            let second = volume.image.unmount_volume(&volume);
            assert_eq!((first.began(), second.began()), (true, false));
            assert_eq!(volume.fxfs().err(), Some(Errno::ENOTCONN));
            assert_eq!(fs.root().getattr().await.err(), Some(Errno::ENOTCONN));
            second.wait().await.expect("waiting failed");
            first.wait().await.expect("unmounting failed");
            drop(volume);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }
}
