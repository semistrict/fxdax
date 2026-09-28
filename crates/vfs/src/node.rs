// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An excerpt of `src/storage/lib/vfs/rust/src/node.rs`: the `Node` trait, without the method for
//! serving node connections, and `OpenNode`.

use crate::common::IntoAny;
use crate::directory::entry::GetEntryInfo;
use crate::directory::entry_container::MutableDirectory;
use crate::name::Name;
use fidl_fuchsia_io as fio;
use std::future::{Future, ready};
use std::sync::Arc;
use zx_status::Status;

/// All nodes must implement this trait.
pub trait Node: GetEntryInfo + IntoAny + Send + Sync + 'static {
    /// Returns node attributes (io2).
    fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> impl Future<Output = Result<fio::NodeAttributes2, Status>> + Send
    where
        Self: Sized;

    /// Called when the node is about to be opened as the node protocol.  Implementers can use this
    /// to perform any initialization or reference counting.  Errors here will result in the open
    /// failing.  By default, this forwards to the infallible will_clone.
    fn will_open_as_node(&self) -> Result<(), Status> {
        self.will_clone();
        Ok(())
    }

    /// Called when the node is about to be cloned (and also by the default implementation of
    /// will_open_as_node).  Implementations that perform their own open count can use this.  Each
    /// call to `will_clone` will be accompanied by an eventual call to `close`.
    fn will_clone(&self) {}

    /// Called when the node is closed.
    fn close(self: Arc<Self>) {}

    fn link_into(
        self: Arc<Self>,
        _destination_dir: Arc<dyn MutableDirectory>,
        _name: Name,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Returns information about the filesystem and/or volume.
    fn query_filesystem(&self) -> Result<fio::FilesystemInfo, Status> {
        Err(Status::NOT_SUPPORTED)
    }

    /// List extended attributes.
    fn list_extended_attributes(&self) -> impl Future<Output = Result<Vec<Vec<u8>>, Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Get the value for an extended attribute.
    fn get_extended_attribute(
        &self,
        _name: Vec<u8>,
    ) -> impl Future<Output = Result<Vec<u8>, Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Set the value for an extended attribute.
    fn set_extended_attribute(
        &self,
        _name: Vec<u8>,
        _value: Vec<u8>,
        _mode: fio::SetExtendedAttributeMode,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Remove the value for an extended attribute.
    fn remove_extended_attribute(
        &self,
        _name: Vec<u8>,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }
}

/// Keeps a node open while it is alive, closing it when dropped.
pub struct OpenNode<T: Node> {
    node: Arc<T>,
}

impl<T: Node> OpenNode<T> {
    pub fn new(node: Arc<T>) -> Self {
        Self { node }
    }
}

impl<T: Node> Drop for OpenNode<T> {
    fn drop(&mut self) {
        self.node.clone().close();
    }
}

impl<T: Node> std::ops::Deref for OpenNode<T> {
    type Target = Arc<T>;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}
