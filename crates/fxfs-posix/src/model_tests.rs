// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Random sequences of what a VFS asks of Fxfs, checked against a plain model of files as pages:
//! no VM, no kernel, seconds. Offsets lean on the awkward ones, a block's edges, a chunk's, 4 GiB
//! and the largest a file may reach, and sequences remount and compact between their operations,
//! so that what holds until a file is reopened is found: xfstests found those in a guest, in
//! minutes, one at a time (generic/130, 303, 423, 525). Each step reads back what it changed; each
//! sequence ends with every file compared, across a remount, and fsck on both sides of it.
//! Seeded by proptest, which says the sequence if one fails, shrunk.

use super::tests::Tested;
use super::*;
use fxfs::fsck::fsck;
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use std::collections::BTreeMap;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

const PAGE: u64 = 4096;
const FILE: u32 = S_IFREG | 0o644;
/// The largest size a file may have.
const LARGEST: u64 = i64::MAX as u64;
/// The names a sequence uses: few, so that they meet.
const NAMES: usize = 6;

/// A file's model: its size and the pages written, which holes aren't.
#[derive(Clone, Debug, Default)]
struct Data {
    size: u64,
    pages: BTreeMap<u64, Vec<u8>>,
}

impl Data {
    fn write(&mut self, at: u64, bytes: &[u8]) {
        for (i, byte) in bytes.iter().enumerate() {
            let at = at + i as u64;
            let page = self.pages.entry(at / PAGE).or_insert_with(|| vec![0; PAGE as usize]);
            page[(at % PAGE) as usize] = *byte;
        }
        self.size = self.size.max(at + bytes.len() as u64);
    }

    fn truncate(&mut self, size: u64) {
        if size < self.size {
            self.pages.retain(|page, _| page * PAGE < size);
            if let Some(last) = self.pages.get_mut(&(size / PAGE)) {
                last[(size % PAGE) as usize..].fill(0);
            }
        }
        self.size = size;
    }

    fn punch(&mut self, pages: std::ops::Range<u64>) {
        self.pages.retain(|page, _| !pages.contains(page));
    }

    fn read(&self, at: u64, len: usize) -> Vec<u8> {
        let end = self.size.min(at.saturating_add(len as u64));
        (at..end.max(at))
            .map(|at| self.pages.get(&(at / PAGE)).map_or(0, |page| page[(at % PAGE) as usize]))
            .collect()
    }
}

#[derive(Clone, Debug)]
enum Entry {
    File(u64),
    Symlink(u64),
}

#[derive(Clone, Debug)]
enum Op {
    Create(usize),
    Symlink(usize),
    Link(usize, usize),
    Unlink(usize),
    Rename(usize, usize),
    Write { file: usize, at: u64, len: usize, byte: u8 },
    Read { file: usize, at: u64, len: usize },
    Truncate { file: usize, size: u64 },
    Punch { file: usize, page: u64, pages: u64 },
    Clone { from: usize, to: usize, page: u64, pages: u64, to_page: u64 },
    Sync,
    Compact,
    Remount,
    /// A crash just after a sync: what's on the device holds all there was, replayed.
    Crash,
}

/// Offsets that lean on the edges of things.
fn offset() -> impl Strategy<Value = u64> {
    prop_oneof![
        3 => (0u64..32).prop_map(|page| page * PAGE),
        3 => 0u64..(64 << 10),
        1 => (0u64..8192).prop_map(|near| (2 << 20) + near - 4096),
        1 => (0u64..8192).prop_map(|near| (1 << 32) + near - 4096),
        2 => (1u64..16384).prop_map(|back| LARGEST - back),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let name = || 0..NAMES;
    let file = || 0usize..16;
    let length = || prop_oneof![1usize..64, Just(PAGE as usize), 1usize..(3 * PAGE as usize)];
    prop_oneof![
        3 => name().prop_map(Op::Create),
        1 => name().prop_map(Op::Symlink),
        1 => (file(), name()).prop_map(|(file, name)| Op::Link(file, name)),
        2 => name().prop_map(Op::Unlink),
        1 => (name(), name()).prop_map(|(from, to)| Op::Rename(from, to)),
        6 => (file(), offset(), length(), any::<u8>())
            .prop_map(|(file, at, len, byte)| Op::Write { file, at, len, byte }),
        2 => (file(), offset(), length()).prop_map(|(file, at, len)| Op::Read { file, at, len }),
        3 => (file(), offset()).prop_map(|(file, size)| Op::Truncate { file, size }),
        2 => (file(), 0u64..32, 1u64..8)
            .prop_map(|(file, page, pages)| Op::Punch { file, page, pages }),
        2 => (file(), file(), 0u64..16, 1u64..8, 0u64..32).prop_map(
            |(from, to, page, pages, to_page)| Op::Clone { from, to, page, pages, to_page }
        ),
        1 => Just(Op::Sync),
        1 => Just(Op::Compact),
        1 => Just(Op::Remount),
        1 => Just(Op::Crash),
    ]
}

/// A filesystem and its model.
struct World {
    filesystem: fxfs::filesystem::OpenFxFilesystem,
    fs: Fxfs<Tested>,
    root: u64,
    names: BTreeMap<usize, Entry>,
    files: BTreeMap<u64, Data>,
}

fn name_of(name: usize) -> Vec<u8> {
    vec![b'a' + name as u8]
}

impl World {
    async fn new() -> Self {
        let device = DeviceHolder::new(FakeDevice::new(32768, 4096));
        let filesystem = FxFilesystem::new_empty(device).await.expect("formatting");
        let volumes = root_volume((*filesystem).clone()).await.expect("the root volume");
        let volume = volumes
            .new_volume("test", NewChildStoreOptions::default())
            .await
            .expect("the volume");
        let root = volume.root_directory_object_id();
        let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
        Self { filesystem, fs, root, names: BTreeMap::new(), files: BTreeMap::new() }
    }

    /// The ino of the `index`th file a name links, if any is.
    fn file(&self, index: usize) -> Option<u64> {
        let files: Vec<u64> = self.files.keys().copied().collect();
        (!files.is_empty()).then(|| files[index % files.len()])
    }

    fn links(&self, ino: u64) -> u32 {
        (self.names.values())
            .filter(|entry| matches!(entry, Entry::File(of) | Entry::Symlink(of) if *of == ino))
            .count() as u32
    }

    /// What's left of `entry`, its name gone: a file no name links goes as the kernel lets it go.
    fn forget(&mut self, entry: Entry) {
        let (Entry::File(ino) | Entry::Symlink(ino)) = entry;
        if self.links(ino) == 0 {
            self.files.remove(&ino);
            self.fs.evict(ino, true);
        }
    }

    /// Checks file `ino` against its model: its size, and what it reads back, around its pages
    /// and at its end.
    async fn check(&self, ino: u64) -> Result<(), String> {
        let data = &self.files[&ino];
        let attr = self.fs.attr(ino).await.map_err(|e| format!("attr of {ino}: {e:#}"))?;
        if attr.size != data.size {
            return Err(format!("{ino} is {} bytes, not {}", attr.size, data.size));
        }
        let mut places: Vec<u64> = data.pages.keys().map(|page| page * PAGE).collect();
        places.push(data.size.saturating_sub(2 * PAGE));
        for at in places {
            self.compare(ino, at, 3 * PAGE as usize).await?;
        }
        Ok(())
    }

    async fn compare(&self, ino: u64, at: u64, len: usize) -> Result<(), String> {
        let expected = self.files[&ino].read(at, len);
        let mut read = vec![0xa5; len];
        let got = self.fs.read(ino, at, &mut read).await;
        let got = got.map_err(|e| format!("read of {len} at {at} of {ino}: {e:#}"))?;
        if read[..got] != expected[..] {
            let first = (0..got.min(expected.len())).find(|i| read[*i] != expected[*i]);
            return Err(format!(
                "read of {len} at {at} of {ino}: {got} bytes, expected {}, differing at {first:?}",
                expected.len()
            ));
        }
        Ok(())
    }

    /// Checks everything, then fsck: of the names and of each file.
    async fn check_all(&self) -> Result<(), String> {
        let mut found = Vec::new();
        let listed = self.fs.readdir(self.root, ReaddirStart::Index(0), |name, _, _| {
            found.push(name.as_bytes().to_vec());
            true
        });
        listed.await.map_err(|e| format!("readdir: {e:#}"))?;
        found.sort();
        let names: Vec<Vec<u8>> = self.names.keys().map(|name| name_of(*name)).collect();
        if found != names {
            return Err(format!("the directory holds {found:?}, not {names:?}"));
        }
        for ino in self.files.keys() {
            self.check(*ino).await?;
        }
        self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
        fsck((*self.filesystem).clone()).await.map_err(|e| format!("fsck: {e:#}"))?;
        Ok(())
    }

    /// Unmounts and mounts again, as a guest's `umount` and `mount` do.
    async fn remount(self) -> Result<Self, String> {
        let Self { filesystem, fs, root, names, files } = self;
        fs.stop_delayed().await.map_err(|e| format!("stopping: {e:#}"))?;
        drop(fs);
        filesystem.close().await.map_err(|e| format!("closing: {e:#}"))?;
        let device = filesystem.take_device().await;
        device.reopen(false);
        let filesystem = FxFilesystem::open(device).await.map_err(|e| format!("opening: {e:#}"))?;
        let volumes = root_volume((*filesystem).clone()).await.map_err(|e| format!("{e:#}"))?;
        let volume = volumes.volume("test", StoreOptions::default()).await;
        let volume = volume.map_err(|e| format!("the volume: {e:#}"))?;
        drop(volumes);
        let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
        Ok(Self { filesystem, fs, root, names, files })
    }

    /// Checks what a crash leaves now, this filesystem going on: a copy of the device, as of a
    /// sync, replayed, holds what the model does, and passes fsck.
    async fn crash(&self) -> Result<(), String> {
        self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?;
        let device = self.filesystem.device().snapshot().map_err(|e| format!("snapshot: {e:#}"))?;
        let filesystem = FxFilesystem::open(device).await.map_err(|e| format!("replay: {e:#}"))?;
        let volumes = root_volume((*filesystem).clone()).await.map_err(|e| format!("{e:#}"))?;
        let volume = volumes.volume("test", StoreOptions::default()).await;
        let volume = volume.map_err(|e| format!("the volume: {e:#}"))?;
        drop(volumes);
        let fs = Fxfs::new((*filesystem).clone(), volume, false, Tested);
        let replayed = Self {
            filesystem,
            fs,
            root: self.root,
            names: self.names.clone(),
            files: self.files.clone(),
        };
        replayed.check_all().await?;
        replayed.fs.stop_delayed().await.map_err(|e| format!("stopping: {e:#}"))?;
        let Self { filesystem, fs, .. } = replayed;
        drop(fs);
        filesystem.close().await.map_err(|e| format!("closing: {e:#}"))
    }

    /// Does `op`, if it can be done of what there is, and checks what it changed.
    async fn step(mut self, op: &Op) -> Result<Self, String> {
        let root = self.root;
        match *op {
            Op::Create(name) if !self.names.contains_key(&name) => {
                let made = self.fs.create(root, &name_of(name), FILE, 0, 0, 0, &[]).await;
                let [attr, _] = made.map_err(|e| format!("create: {e:#}"))?;
                if attr.ctime_ns < attr.mtime_ns {
                    return Err(format!("a new file's ctime is before its mtime: {attr:?}"));
                }
                self.names.insert(name, Entry::File(attr.ino));
                self.files.insert(attr.ino, Data::default());
            }
            Op::Symlink(name) if !self.names.contains_key(&name) => {
                let made = self.fs.symlink(root, &name_of(name), b"target", 0, 0).await;
                let [attr, _] = made.map_err(|e| format!("symlink: {e:#}"))?;
                if attr.ctime_ns < attr.mtime_ns {
                    return Err(format!("a new symlink's ctime is before its mtime: {attr:?}"));
                }
                self.names.insert(name, Entry::Symlink(attr.ino));
            }
            Op::Link(file, name) if !self.names.contains_key(&name) => {
                let Some(ino) = self.file(file) else { return Ok(self) };
                let linked = self.fs.link(ino, root, &name_of(name), false).await;
                let [attr, _] = linked.map_err(|e| format!("link: {e:#}"))?;
                self.names.insert(name, Entry::File(ino));
                if attr.nlink != self.links(ino) {
                    return Err(format!("{ino} has {} links, not {}", attr.nlink, self.links(ino)));
                }
            }
            Op::Unlink(name) if self.names.contains_key(&name) => {
                let entry = self.names.remove(&name).expect("looked up");
                let (Entry::File(ino) | Entry::Symlink(ino)) = entry;
                let gone = self.fs.unlink(root, &name_of(name), ino).await;
                gone.map_err(|e| format!("unlink: {e:#}"))?;
                self.forget(entry);
            }
            Op::Rename(from, to) if from != to && self.names.contains_key(&from) => {
                let entry = self.names.remove(&from).expect("looked up");
                let (Entry::File(ino) | Entry::Symlink(ino)) = entry;
                let replaced = self.names.remove(&to);
                let new_ino = replaced.as_ref().map(|replaced| {
                    let (Entry::File(ino) | Entry::Symlink(ino)) = replaced;
                    *ino
                });
                let (from, moved) = (name_of(from), to);
                let renamed =
                    self.fs.rename(root, &from, ino, root, &name_of(moved), new_ino).await;
                renamed.map_err(|e| format!("rename: {e:#}"))?;
                self.names.insert(moved, entry);
                if let Some(replaced) = replaced {
                    self.forget(replaced);
                }
            }
            Op::Write { file, at, len, byte } => {
                let Some(ino) = self.file(file) else { return Ok(self) };
                let at = at.min(LARGEST - len as u64);
                let bytes = vec![byte; len];
                self.fs.write(ino, at, &bytes).await.map_err(|e| format!("write: {e:#}"))?;
                self.files.get_mut(&ino).expect("a file").write(at, &bytes);
                self.check(ino).await?;
            }
            Op::Read { file, at, len } => {
                let Some(ino) = self.file(file) else { return Ok(self) };
                self.compare(ino, at, len).await?;
            }
            Op::Truncate { file, size } => {
                let Some(ino) = self.file(file) else { return Ok(self) };
                let set = SetAttr { valid: SET_SIZE, size, ..SetAttr::default() };
                self.fs.setattr(ino, &set).await.map_err(|e| format!("truncate: {e:#}"))?;
                self.files.get_mut(&ino).expect("a file").truncate(size);
                self.check(ino).await?;
            }
            Op::Punch { file, page, pages } => {
                let Some(ino) = self.file(file) else { return Ok(self) };
                let data = self.files.get_mut(&ino).expect("a file");
                let end = (page + pages).min(data.size / PAGE);
                if page < end {
                    data.punch(page..end);
                    let range = page * PAGE..end * PAGE;
                    self.fs.punch(ino, range).await.map_err(|e| format!("punch: {e:#}"))?;
                    self.check(ino).await?;
                }
            }
            Op::Clone { from, to, page, pages, to_page } => {
                let (Some(src), Some(dst)) = (self.file(from), self.file(to)) else {
                    return Ok(self);
                };
                let end = (page + pages).min(self.files[&src].size / PAGE);
                let to_end = to_page + end.saturating_sub(page);
                let overlaps = src == dst && page < to_end && to_page < end;
                if page < end && !overlaps {
                    let size = self.files[&dst].size.max(to_end * PAGE);
                    let cloned = self.fs.clone_range(
                        src,
                        page * PAGE..end * PAGE,
                        dst,
                        to_page * PAGE,
                        size,
                    );
                    cloned.await.map_err(|e| format!("clone: {e:#}"))?;
                    let source = self.files[&src].pages.clone();
                    let data = self.files.get_mut(&dst).expect("a file");
                    data.punch(to_page..to_end);
                    for at in page..end {
                        if let Some(bytes) = source.get(&at) {
                            data.pages.insert(to_page + at - page, bytes.clone());
                        }
                    }
                    data.size = size;
                    self.check(dst).await?;
                }
            }
            Op::Sync => self.fs.sync().await.map_err(|e| format!("sync: {e:#}"))?,
            Op::Compact => {
                let journal = self.filesystem.journal();
                journal.force_compact().await.map_err(|e| format!("compacting: {e:#}"))?;
            }
            Op::Crash => self.crash().await.map_err(|e| format!("after a crash: {e}"))?,
            Op::Remount => {
                self = self.remount().await?;
                for ino in self.files.keys() {
                    self.check(*ino).await?;
                }
            }
            _ => {}
        }
        Ok(self)
    }
}

async fn run(ops: Vec<Op>) -> Result<(), String> {
    let mut world = World::new().await;
    for (i, op) in ops.iter().enumerate() {
        world = world.step(op).await.map_err(|e| format!("op {i}, {op:?}: {e}"))?;
    }
    world.check_all().await.map_err(|e| format!("at the end: {e}"))?;
    let world = world.remount().await.map_err(|e| format!("at the end: {e}"))?;
    world.check_all().await.map_err(|e| format!("remounted at the end: {e}"))?;
    world.fs.stop_delayed().await.map_err(|e| format!("stopping: {e:#}"))?;
    let World { filesystem, fs, .. } = world;
    drop(fs);
    filesystem.close().await.map_err(|e| format!("closing: {e:#}"))
}

/// FXDAX_MODEL_CASES asks for more, to hunt.
#[test]
fn whatever_a_vfs_asks_reads_back_as_a_model_says() {
    let cases = std::env::var("FXDAX_MODEL_CASES").map_or(96, |cases| cases.parse().unwrap());
    let config = Config { cases, failure_persistence: None, ..Config::default() };
    let sequences = proptest::collection::vec(op(), 1..48);
    TestRunner::new(config)
        .run(&sequences, |ops| {
            let done = fasync::LocalExecutorBuilder::new().build().run_singlethreaded(run(ops));
            prop_assert_eq!(done, Ok(()));
            Ok(())
        })
        .unwrap();
}
