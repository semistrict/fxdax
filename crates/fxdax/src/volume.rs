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
//! drop, or its image's close, the others wait for that one unmount.

use crate::connection::Connection;
use crate::error::{Errno, Error, Result};
use crate::image::ImageShared;
use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future::Shared;
use fxfs_platform::node::{FxNode, OpenedNode};
use fxfs_platform::volume::{FxVolume, FxVolumeAndRoot};
use fxfs_platform::volumes_directory::VolumesDirectory;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};

/// A mounted volume of an image. See the module docs.
pub(crate) struct Volume {
    /// The image whose volume it is.
    pub image: Arc<ImageShared>,
    /// The volume's name in the image.
    name: String,
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
        let held = HeldNode {
            node: OpenedNode::new(volume.root().clone().as_node()),
            connection: Some(root),
            holds: 1,
        };
        let mounted = Mounted { volume, nodes: HashMap::from([(object_id, held)]) };
        let volume = Arc::new(Self {
            image,
            name: name.to_owned(),
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

    /// Runs `future` on Fxfs's executor ([`Executor::run`](crate::executor::Executor::run)).
    pub async fn run<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> T {
        self.image.executor.run(future).await
    }

    /// Fxfs's volume, until the unmount begins.
    pub fn fx_volume(&self) -> Result<Arc<FxVolume>> {
        let mut state = self.state.lock().unwrap();
        Ok(state.mounted()?.volume.volume().clone())
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
    /// if no one waits for it.
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
        self.image.executor.spawn(async move {
            let unmounted = async {
                // Closing every connection lets the volume's nodes be dropped.
                for (_, held) in nodes {
                    if let Some(Connection::Directory(proxy)) = held.connection {
                        let _ = proxy.close().await;
                    }
                }
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
