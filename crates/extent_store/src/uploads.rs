// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a segment of the store's write-ahead log, for its uploader to upload ([`Closed`]).

use crate::dir::File;
use crate::framed::read_object;
use crate::segments::footer_last;
use extent_frame::segment::Footer;
use std::io;
use std::sync::Arc;

/// fxdax: a segment the store closed, for its uploader to upload: its footer, its file, framed,
/// and the files of the extents stored on their own its footer names, by their keys; and whether
/// its writer's chain stamped it anew after a takeover, which its framed file says not
/// (TASK-165.9).
pub struct Closed<F> {
    /// Its number in the store.
    pub number: u64,
    pub footer: Footer,
    pub file: Arc<F>,
    pub own: Vec<(String, Arc<F>)>,
    pub restamped: bool,
}

impl<F: File> Closed<F> {
    /// The segment's bytes: the object it is, checked ([`read_object`]), with its footer, stamped
    /// anew if it was (`extent_frame::restamp`).
    pub fn bytes(&self) -> io::Result<Vec<u8>> {
        let object = read_object(&*self.file)?;
        if !self.restamped {
            return Ok(object);
        }
        extent_frame::restamp(&object, &self.footer.encode()).map_err(io::Error::other)
    }

    /// The bytes of the files of its extents stored on their own, by their keys, checked.
    pub fn own_bytes(&self) -> io::Result<Vec<(String, Vec<u8>)>> {
        self.own.iter().map(|(key, file)| Ok((key.clone(), read_object(&**file)?))).collect()
    }

    /// The store's epoch of the last commit it holds.
    pub fn last(&self) -> u64 {
        footer_last(&self.footer)
    }

    /// The tag of the last commit it holds, which names the commit in object storage: each segment
    /// the store closes names one, the last of its commits.
    pub fn tag(&self) -> u64 {
        let commits = &self.footer.contents.commits;
        *commits.last().expect("a segment the store closed names its last commit")
    }
}
