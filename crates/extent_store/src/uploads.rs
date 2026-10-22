// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what a store keeps for its uploader, which uploads its branch to object storage as a
//! writer of it (doc-7): what each commit changed, and the segments the store closed, for the
//! writer their footing names; and what the uploader starts from.

use crate::dir::File;
use crate::framed::read_whole;
use crate::segments::footer_last;
use extent_frame::segment::Footer;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::Arc;
use storage_device::ExtentId;

/// fxdax: what a commit changed, kept for an uploader ([`Store::upload`](crate::Store::upload)):
/// the extents it wrote, with their lengths, the extents it freed, and the root, if it set one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Committed {
    pub epoch: u64,
    pub written: Vec<(ExtentId, u64)>,
    pub freed: Vec<ExtentId>,
    pub root: Option<Vec<u8>>,
}

/// fxdax: what a store held when its uploader began: its extents, with their lengths, and root, as
/// of `epoch`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub epoch: u64,
    pub extents: Vec<(ExtentId, u64)>,
    pub root: Vec<u8>,
}

/// fxdax: who the store's closed segments are for, which their footers say: the writer of a
/// branch in object storage, by its lease's epoch, which uploads them as they are (doc-7, decision
/// 1). Its segments are numbered on from `next_seq`, the first following `after`, the segment
/// before it; their footers say the branch's next sequence number is `next_sequence` at least,
/// the writer's floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footing {
    pub epoch: u64,
    pub next_seq: u64,
    pub after: Option<(u64, u64)>,
    pub next_sequence: u64,
}

/// fxdax: a segment the store closed, for its uploader to upload: its footer, its file, framed,
/// and the files of the extents stored on their own its footer names, by their keys.
pub struct Closed<F> {
    /// Its number in the store.
    pub number: u64,
    pub footer: Footer,
    pub file: Arc<F>,
    pub own: Vec<(String, Arc<F>)>,
}

impl<F: File> Closed<F> {
    /// The segment's bytes: the object it is.
    pub fn bytes(&self) -> io::Result<Vec<u8>> {
        read_whole(&*self.file)
    }

    /// The bytes of the files of its extents stored on their own, by their keys.
    pub fn own_bytes(&self) -> io::Result<Vec<(String, Vec<u8>)>> {
        self.own.iter().map(|(key, file)| Ok((key.clone(), read_whole(&**file)?))).collect()
    }

    /// The store's epoch of the last commit it holds.
    pub fn last(&self) -> u64 {
        footer_last(&self.footer)
    }
}

/// fxdax: what a store keeps for its uploader.
#[derive(Default)]
pub(crate) struct Uploads {
    /// The commits ordered and not yet taken, oldest first.
    pub committed: VecDeque<Committed>,
    /// The last epoch durable.
    pub durable: u64,
    /// The bytes each commit not yet released wrote.
    pub unreleased: BTreeMap<u64, u64>,
    /// The segments closed and not yet taken, oldest first, by their numbers, with their footers.
    pub closed: VecDeque<(u64, Footer)>,
    /// The segments closed and not yet uploaded, which stay until they are.
    pub unuploaded: BTreeSet<u64>,
}
