// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Images for tests (the `testing` feature): layers, tar archives of [`Entry`]s, as image builders
//! write them, and OCI image layouts of them.

use sha2::{Digest as _, Sha256};
use std::io::{self, Write as _};
use std::path::Path;

/// The time entries are made at, unless they say otherwise: 2020-09-13.
pub const MTIME: u64 = 1_600_000_000;

/// What an entry of a layer makes.
#[derive(Clone, Copy, Debug)]
pub enum Kind<'a> {
    /// A file, with these contents.
    File(&'a [u8]),
    /// A directory.
    Dir,
    /// A symlink, to this target.
    Symlink(&'a str),
    /// A hard link, to the node at this path.
    HardLink(&'a str),
    /// A character device node, of this major and minor number.
    CharDevice(u32, u32),
    /// A block device node, of this major and minor number.
    BlockDevice(u32, u32),
    /// A FIFO.
    Fifo,
}

/// An entry of a layer: what it makes, at its path, with its attributes.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    path: &'a str,
    kind: Kind<'a>,
    perm: u32,
    uid: u64,
    gid: u64,
    mtime: u64,
    xattrs: &'a [(&'a str, &'a [u8])],
}

impl<'a> Entry<'a> {
    /// An entry making `kind` at `path`, owned by root, with permissions 0755 for a directory and
    /// 0644 otherwise, at [`MTIME`], without extended attributes.
    pub fn new(path: &'a str, kind: Kind<'a>) -> Self {
        let perm = if matches!(kind, Kind::Dir) { 0o755 } else { 0o644 };
        Self { path, kind, perm, uid: 0, gid: 0, mtime: MTIME, xattrs: &[] }
    }

    /// The entry, with permissions `perm`.
    pub fn perm(self, perm: u32) -> Self {
        Self { perm, ..self }
    }

    /// The entry, owned by `uid` and `gid`.
    pub fn owner(self, uid: u64, gid: u64) -> Self {
        Self { uid, gid, ..self }
    }

    /// The entry, made at `mtime`, in seconds since the Unix epoch.
    pub fn mtime(self, mtime: u64) -> Self {
        Self { mtime, ..self }
    }

    /// The entry, with the extended attributes `xattrs`, which a PAX record names each of.
    pub fn xattrs(self, xattrs: &'a [(&'a str, &'a [u8])]) -> Self {
        Self { xattrs, ..self }
    }
}

/// A whiteout of `path`: an entry removing what the layers below have there.
pub fn whiteout(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/.wh.{name}"),
        None => format!(".wh.{path}"),
    }
}

/// An opaque whiteout of the directory `dir`: an entry removing everything the layers below have
/// in it.
pub fn opaque(dir: &str) -> String {
    format!("{dir}/.wh..wh..opq")
}

/// A layer: a tar archive of `entries`, in turn.
pub fn layer(entries: &[Entry<'_>]) -> io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    for entry in entries {
        if !entry.xattrs.is_empty() {
            let mut records = Vec::new();
            for (name, value) in entry.xattrs {
                pax_record(&mut records, &format!("SCHILY.xattr.{name}"), value);
            }
            let mut header = tar::Header::new_ustar();
            header.set_entry_type(tar::EntryType::XHeader);
            header.set_size(records.len() as u64);
            header.set_mode(0o644);
            builder.append_data(&mut header, "PaxHeaders/entry", records.as_slice())?;
        }
        let mut header = tar::Header::new_gnu();
        header.set_mode(entry.perm);
        header.set_uid(entry.uid);
        header.set_gid(entry.gid);
        header.set_mtime(entry.mtime);
        let (entry_type, data, link): (_, &[u8], _) = match entry.kind {
            Kind::File(contents) => (tar::EntryType::Regular, contents, None),
            Kind::Dir => (tar::EntryType::Directory, &[], None),
            Kind::Symlink(target) => (tar::EntryType::Symlink, &[], Some(target)),
            Kind::HardLink(target) => (tar::EntryType::Link, &[], Some(target)),
            Kind::CharDevice(major, minor) | Kind::BlockDevice(major, minor) => {
                header.set_device_major(major)?;
                header.set_device_minor(minor)?;
                let entry_type = match entry.kind {
                    Kind::CharDevice(..) => tar::EntryType::Char,
                    _ => tar::EntryType::Block,
                };
                (entry_type, &[], None)
            }
            Kind::Fifo => (tar::EntryType::Fifo, &[], None),
        };
        header.set_entry_type(entry_type);
        header.set_size(data.len() as u64);
        match link {
            Some(target) => builder.append_link(&mut header, entry.path, target)?,
            None => builder.append_data(&mut header, entry.path, data)?,
        }
    }
    builder.into_inner()
}

/// Appends the PAX record of `key` and `value` to `records`: its length, in decimal, counting
/// itself, then `key=value` and a newline.
fn pax_record(records: &mut Vec<u8>, key: &str, value: &[u8]) {
    let rest = key.len() + value.len() + 3;
    let mut len = rest + rest.to_string().len();
    if len.to_string().len() + rest != len {
        len += 1;
    }
    records.extend_from_slice(format!("{len} {key}=").as_bytes());
    records.extend_from_slice(value);
    records.push(b'\n');
}

/// `layer`, gzipped, as images' layers usually are.
pub fn gzip(layer: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(layer)?;
    encoder.finish()
}

/// Writes an OCI image layout at `dir`, which must not exist, holding the image of `layers`,
/// bottom first, named `reference` in its index.
pub fn oci_layout(dir: &Path, reference: &str, layers: &[Vec<u8>]) -> io::Result<()> {
    std::fs::create_dir(dir)?;
    std::fs::create_dir_all(dir.join("blobs/sha256"))?;
    std::fs::write(dir.join("oci-layout"), br#"{"imageLayoutVersion":"1.0.0"}"#)?;
    let blob = |contents: &[u8]| -> io::Result<String> {
        let hex: String =
            Sha256::digest(contents).iter().map(|byte| format!("{byte:02x}")).collect();
        std::fs::write(dir.join("blobs/sha256").join(&hex), contents)?;
        Ok(format!("sha256:{hex}"))
    };
    let config =
        br#"{"architecture":"amd64","os":"linux","rootfs":{"type":"layers","diff_ids":[]}}"#;
    let config = serde_json::json!({
        "mediaType": "application/vnd.oci.image.config.v1+json",
        "digest": blob(config)?,
        "size": config.len(),
    });
    let mut descriptors = Vec::new();
    for layer in layers {
        descriptors.push(serde_json::json!({
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": blob(layer)?,
            "size": layer.len(),
        }));
    }
    let manifest = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": config,
        "layers": descriptors,
    }))?;
    let index = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": blob(&manifest)?,
            "size": manifest.len(),
            "annotations": { "org.opencontainers.image.ref.name": reference },
        }],
    });
    std::fs::write(dir.join("index.json"), serde_json::to_vec(&index)?)
}
