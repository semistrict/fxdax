// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A mounted volume: an Fxfs volume of an image, which a [`Filesystem`](crate::Filesystem) serves,
//! and the nodes that handles to them ([`Node`](crate::Node)s) hold open.
//!
//! A volume is mounted until its unmount begins, and from then on every operation on it fails with
//! `ENOTCONN`. The unmount closes the nodes held, the root directory among them, and unmounts the
//! volume from its image's volumes directory, on Fxfs's executor, where it completes whoever waits
//! for it. Whichever begins it, of the filesystem's [`unmount`](crate::Filesystem::unmount), its
//! drop, or its image's close, the others wait for that one unmount. The work that operations
//! under way when it begins run on Fxfs's executor ([`Volume::run`]), the reads and writes
//! copying through files' streams ([`Volume::open_guarded`]), and an image's clone or snapshot
//! writing the volume back ([`Volume::fx_volume_guarded`]), hold it off from writing the volume
//! back until they complete, as the volume's fuchsia.io requests do.

use crate::connection::Connection;
use crate::error::{Errno, Error, Result};
use crate::image::ImageShared;
use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future::Shared;
use fxfs::future_with_guard::FutureWithGuard;
use fxfs_platform::node::{FxNode, OpenedNode};
use fxfs_platform::volume::{FxVolume, FxVolumeAndRoot};
use fxfs_platform::volumes_directory::VolumesDirectory;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};
use vfs::execution_scope::{ActiveGuard, ExecutionScope};

/// A mounted volume of an image. See the module docs.
pub(crate) struct Volume {
    /// The image whose volume it is.
    pub image: Arc<ImageShared>,
    /// The volume's name in the image.
    name: String,
    /// Fxfs's volume's scope, which work on the volume runs in ([`run`](Self::run)), kept from the
    /// mount on, so that running some takes neither the state's lock nor Fxfs's volume.
    scope: ExecutionScope,
    state: Mutex<State>,
}

enum State {
    /// Mounted, until the unmount begins.
    Mounted(Mounted),
    /// Unmounting, or unmounted: completes once it is unmounted.
    Unmounting(Shared<oneshot::Receiver<()>>),
}

impl State {
    /// The volume, while it is mounted.
    fn mounted(&mut self) -> Result<&mut Mounted> {
        match self {
            Self::Mounted(mounted) => Ok(mounted),
            Self::Unmounting(_) => Err(Errno::ENOTCONN),
        }
    }
}

/// A mounted volume, and its nodes that handles hold.
struct Mounted {
    volume: FxVolumeAndRoot,
    /// The nodes held, by object ID.
    nodes: HashMap<u64, HeldNode>,
}

/// A node that handles hold.
pub(crate) struct HeldNode {
    /// Fxfs's node, open: as an open file does, the open count keeps a file that is unlinked while
    /// a handle holds it.
    pub node: OpenedNode<dyn FxNode>,
    /// A fuchsia.io connection to the node, once an operation needs one. Lookups and attributes
    /// need none, and opening one costs a request most of its time.
    pub connection: Option<Connection>,
    /// How many holds there are on it.
    holds: u64,
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

/// A node of a volume opened under a guard on Fxfs's volume's scope ([`Volume::open_guarded`]),
/// which a read or write of a file holds while it copies through the file's stream. The guard
/// holds the unmount off from writing the volume back until it is dropped, so that what a write
/// copies is written back: a write extending a file extends it only once it has copied, and the
/// volume's last write-back leaves a dirty page past a file's end for a later one, which never
/// comes. The node keeps the file open meanwhile, though the unmount closes the node and the
/// connection that handles hold, so that Fxfs's pager answers the copy's page requests, as the
/// guard keeps the scope it answers them in from finishing: one for a file closed goes
/// unanswered, and the copy would wait for it, and the unmount for the copy.
pub(crate) struct OpenGuarded {
    /// Closed first, under the guard: closing a file may leave it to be written back, or purged.
    _node: OpenedNode<dyn FxNode>,
    _guard: ActiveGuard,
}

impl Volume {
    /// The volume `name` of `image`, `volume`, just mounted, and the hold on its root directory,
    /// which `root` connects to.
    pub fn new(
        image: Arc<ImageShared>,
        name: &str,
        volume: FxVolumeAndRoot,
        root: Connection,
    ) -> (Arc<Self>, Hold) {
        let object_id = volume.volume().store().root_directory_object_id();
        let scope = volume.volume().scope().clone();
        let held = HeldNode {
            node: OpenedNode::new(volume.root().clone().as_node()),
            connection: Some(root),
            holds: 1,
        };
        let mounted = Mounted { volume, nodes: HashMap::from([(object_id, held)]) };
        let volume = Arc::new(Self {
            image,
            name: name.to_owned(),
            scope,
            state: Mutex::new(State::Mounted(mounted)),
        });
        let root = Hold { volume: Arc::downgrade(&volume), object_id };
        (volume, root)
    }

    /// The volume's name in its image.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The size of the blocks of the volume's image.
    pub fn block_size(&self) -> u32 {
        self.image.block_size
    }

    /// Runs `future` on Fxfs's executor ([`Executor::run`](crate::executor::Executor::run)) in
    /// Fxfs's volume's scope, as the volume's fuchsia.io connections handle their requests
    /// (`vfs::request_handler::handle`): it holds an active guard on the scope until the future
    /// completes, whether or not the caller waits for it, and Fxfs's volume, unmounting, shuts the
    /// scope down and waits for its guards before it writes the volume back.
    ///
    /// It fails with `ENOTCONN` once the scope has finished, shut down with no guard left.
    /// Operations find the unmount begun before they run anything, as they find the volume's nodes
    /// ([`with_held`](Self::with_held)): one that found the volume mounted runs its work before
    /// the volume is written back, or fails.
    pub async fn run<T: Send + 'static>(
        &self,
        future: impl Future<Output = Result<T>> + Send + 'static,
    ) -> Result<T> {
        let guard = self.guard()?;
        self.image.executor.run(FutureWithGuard::new(guard, future)).await
    }

    /// A guard on Fxfs's volume's scope, which holds the unmount off from writing the volume back
    /// until it is dropped, unless the scope has finished (`ENOTCONN`).
    fn guard(&self) -> Result<ActiveGuard> {
        self.scope.try_active_guard().ok_or(Errno::ENOTCONN)
    }

    /// Fxfs's volume, until the unmount begins.
    pub fn fx_volume(&self) -> Result<Arc<FxVolume>> {
        let mut state = self.state.lock().unwrap();
        Ok(state.mounted()?.volume.volume().clone())
    }

    /// Fxfs's volume, until the unmount begins, with a guard on its scope, for work on it that
    /// doesn't [`run`](Self::run) in it: as `run`'s does, the guard holds the unmount off from
    /// writing the volume back until it is dropped.
    pub fn fx_volume_guarded(&self) -> Result<(Arc<FxVolume>, ActiveGuard)> {
        Ok((self.fx_volume()?, self.guard()?))
    }

    /// Runs `f` on the node that `hold`, one of the volume's holds, holds, as
    /// [`with_held`](Self::with_held) does, and opens the node again, under a guard on Fxfs's
    /// volume's scope (see [`OpenGuarded`]), until the unmount begins (`ENOTCONN`): for a read or
    /// write of a file's, which copies through the file's stream on the caller's thread rather
    /// than in [`run`](Self::run)'s work, and finds the stream with `f`, under the same hold of
    /// the volume's lock.
    pub fn open_guarded<T>(
        &self,
        hold: &Hold,
        f: impl FnOnce(&mut HeldNode) -> T,
    ) -> Result<(T, OpenGuarded)> {
        // The guard first, so that the node opened closes under it.
        let guard = self.guard()?;
        let (found, node) = self.with_held(hold, |held| (f(held), held.node.dup()))?;
        Ok((found, OpenGuarded { _node: node, _guard: guard }))
    }

    /// A new hold on `node`, until the unmount begins (`ENOTCONN`), keeping `connection`, if
    /// given, as its connection unless it has one. Its first hold opens it, under the volume's
    /// lock, so that no node is opened once the unmount has taken those held.
    pub fn hold(
        self: &Arc<Self>,
        node: Arc<dyn FxNode>,
        connection: Option<Connection>,
    ) -> Result<Hold> {
        let object_id = node.object_id();
        let mut state = self.state.lock().unwrap();
        let held = state.mounted()?.nodes.entry(object_id).or_insert_with(|| HeldNode {
            node: OpenedNode::new(node),
            connection: None,
            holds: 0,
        });
        if held.connection.is_none() {
            held.connection = connection;
        }
        held.holds += 1;
        Ok(Hold { volume: Arc::downgrade(self), object_id })
    }

    /// Runs `f` on the node that `hold`, one of the volume's holds, holds, until the unmount begins
    /// (`ENOTCONN`).
    pub fn with_held<T>(&self, hold: &Hold, f: impl FnOnce(&mut HeldNode) -> T) -> Result<T> {
        let mut state = self.state.lock().unwrap();
        let held = state.mounted()?.nodes.get_mut(&hold.object_id);
        Ok(f(held.expect("a node is held while a hold on it lives")))
    }

    /// Releases a hold on the node `object_id`. The last closes its connection and releases its
    /// open count, unless the unmount has taken it.
    fn release(&self, object_id: u64) {
        let mut state = self.state.lock().unwrap();
        let Ok(mounted) = state.mounted() else {
            return;
        };
        let held = mounted.nodes.get_mut(&object_id).expect("a node is held while it has holds");
        held.holds -= 1;
        if held.holds == 0 {
            mounted.nodes.remove(&object_id);
        }
    }

    /// Begins unmounting the volume, unless that has begun: closes the nodes held, and unmounts
    /// the volume from `volumes_directory`, its image's, given while the image is open: once it is
    /// closed, so are its volumes. The unmount runs on Fxfs's executor, where it completes even
    /// if no one waits for it. It waits for the work under way on the volume, which holds guards
    /// on its scope, before it locks the volumes directory.
    pub fn begin_unmount(&self, volumes_directory: Option<Arc<VolumesDirectory>>) -> Unmount {
        let mut state = self.state.lock().unwrap();
        if let State::Unmounting(unmounted) = &*state {
            return Unmount::Waits(unmounted.clone());
        }
        let (done, unmounted) = oneshot::channel();
        let State::Mounted(Mounted { volume, nodes }) =
            std::mem::replace(&mut *state, State::Unmounting(unmounted.shared()))
        else {
            unreachable!("the volume is mounted");
        };
        drop(state);
        let (result, began) = oneshot::channel();
        let scope = self.scope.clone();
        self.image.executor.spawn(async move {
            let unmounted = async {
                // Closing every connection lets the volume's nodes be dropped.
                for (_, held) in nodes {
                    if let Some(Connection::Directory(proxy)) = held.connection {
                        let _ = proxy.close().await;
                    }
                }
                // Fxfs's volume shuts its scope down and waits for it to finish as it terminates,
                // under the volumes directory's lock (`VolumesDirectory::unmount`); but work under
                // a guard on the scope may need the lock, as dirtying a page under critical memory
                // pressure does (`VolumesDirectory::report_pager_dirty`), which a write under way
                // waits for. So the scope is finished first, and the termination finds it so.
                scope.shutdown();
                scope.wait().await;
                let store_id = volume.volume().store().store_object_id();
                drop(volume);
                if let Some(volumes_directory) = volumes_directory {
                    volumes_directory.lock().await.unmount(store_id).await?;
                }
                anyhow::Ok(())
            }
            .await;
            // Its beginner may have given up on it.
            let _ = result.send(unmounted);
            let _ = done.send(());
        });
        Unmount::Began(began)
    }

    /// The nodes held, by object ID, and how many holds there are on each.
    #[cfg(test)]
    pub fn held(&self) -> std::collections::BTreeMap<u64, u64> {
        match &*self.state.lock().unwrap() {
            State::Mounted(mounted) => {
                mounted.nodes.iter().map(|(object_id, held)| (*object_id, held.holds)).collect()
            }
            State::Unmounting(_) => Default::default(),
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
            assert_eq!(volume.fx_volume().err(), Some(Errno::ENOTCONN));
            assert_eq!(fs.root().getattr().await.err(), Some(Errno::ENOTCONN));
            second.wait().await.expect("waiting failed");
            first.wait().await.expect("unmounting failed");
            drop(volume);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }
}
