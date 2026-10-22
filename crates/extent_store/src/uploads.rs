// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: what a store keeps for its uploader, which uploads its branch to object storage as a
//! writer of it (doc-7): the segments the store closed, their footers stamped by the writer's
//! chain (`extent_frame::segment::Chain`), which say what each commit changed, and the bytes each
//! commit wrote, which bound the uploader's lag; and what the uploader starts from.

use crate::dir::File;
use crate::framed::read_whole;
use crate::layers::Extents;
use crate::layout::Change;
use crate::segments::footer_last;
use extent_frame::segment::Footer;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::Arc;
use storage_device::ExtentId;

/// fxdax: what a store held when its uploader began: its extents, with their lengths, and root, as
/// of `epoch`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub epoch: u64,
    pub extents: Vec<(ExtentId, u64)>,
    pub root: Vec<u8>,
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

/// fxdax: what a store keeps for its uploader: the bytes each commit wrote, until released; each
/// segment closed, until taken once its commits are durable, and, until uploaded, the segment
/// itself.
#[derive(Default)]
pub(crate) struct Uploads {
    /// The bytes each commit not yet released wrote.
    unreleased: BTreeMap<u64, u64>,
    /// The segments closed and not yet taken, oldest first, by their numbers, with their footers.
    closed: VecDeque<(u64, Footer)>,
    /// The segments closed and not yet uploaded, which stay until they are.
    unuploaded: BTreeSet<u64>,
}

impl Uploads {
    /// Keeps the bytes commit `epoch`, whose changes are `changes`, wrote: the lengths as of it,
    /// which `extents`, the branch's, say, of the extents it wrote, an extent written in writes one
    /// after another once.
    pub fn record(&mut self, epoch: u64, changes: &[Change], extents: &Extents) {
        let (mut bytes, mut last) = (0, None);
        for change in changes {
            if let Change::Write { extent, .. } | Change::WriteOwn { extent, .. } = change
                && last != Some(*extent)
            {
                // Freed since, if not held: the commit frees it too.
                bytes += extents.get(extent).map_or(0, |entry| entry.len);
                last = Some(*extent);
            }
        }
        self.unreleased.insert(epoch, bytes);
    }

    /// Keeps segment `number`, closed, whose footer is `footer`, until uploaded.
    pub fn closed_segment(&mut self, number: u64, footer: Footer) {
        self.closed.push_back((number, footer));
        self.unuploaded.insert(number);
    }

    /// The segments closed not yet taken whose commits are durable, up to `durable`, oldest first,
    /// by their numbers, with their footers: up to the first `framing` says is being framed, whose
    /// framed file is what is uploaded.
    pub fn take_closed(
        &mut self,
        durable: u64,
        framing: impl Fn(u64) -> bool,
    ) -> Vec<(u64, Footer)> {
        let mut taken = Vec::new();
        while self
            .closed
            .front()
            .is_some_and(|(number, footer)| footer_last(footer) <= durable && !framing(*number))
        {
            taken.push(self.closed.pop_front().unwrap());
        }
        taken
    }

    /// The commits up to `epoch` are released: the uploader has what it needs of them.
    pub fn release(&mut self, epoch: u64) {
        self.unreleased.retain(|committed, _| *committed > epoch);
    }

    /// Segment `number` is uploaded: it may go once no extent is in it.
    pub fn uploaded(&mut self, number: u64) {
        self.unuploaded.remove(&number);
    }

    /// Whether segment `number`, closed, is yet to be uploaded.
    pub fn holds(&self, number: u64) -> bool {
        self.unuploaded.contains(&number)
    }

    /// The bytes the commits not yet released wrote.
    pub fn unreleased_bytes(&self) -> u64 {
        self.unreleased.values().sum()
    }
}
