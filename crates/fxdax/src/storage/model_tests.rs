// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: random sequences of what a filesystem's user asks of an image and of its clones, all
//! kept in the image's own bucket, among passes of the bucket's collector, checked against a model
//! of their files (TASK-291.4). fxfs-posix's model holds Fxfs to a model on a device of memory,
//! under which no bucket is: this one holds the images' extents, in their stores and their
//! bucket, to what their files were written, as the collector compacts and deletes under them.
//!
//! Files are made, written over and past their ends, truncated, removed and synced; images are
//! cloned, and remounted; and the bucket collected, as an operation of the sequence's, a pass
//! alone with no grace period, and after each commit, as often as passes end, compacting what is
//! half freed and deleting it at once ([`BUSY`]). Each operation reads back what it changed; a
//! remount compares every file of the image, and the sequence's end every file of every image,
//! then again opened anew after passes alone, which leave nothing they should have collected
//! (TASK-390).
//!
//! Seeded by proptest, which says the sequence if one fails, shrunk. FXDAX_COLLECT_MODEL_CASES
//! asks for more sequences than 4: each takes seconds, in remounts and clones, which wait for the
//! file bucket's syncs.

use super::Storage;
use super::collect::{Collecting, Pace, quicken};
use crate::node::SetAttr;
use crate::{Filesystem, Owner};
use futures::executor::block_on;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const OWNER: Owner = Owner { uid: 1000, gid: 100 };
const SIZE: u64 = 64 << 20;
const MIB: usize = 1 << 20;
/// How many files each image may have.
const FILES: usize = 4;
/// How many clones a sequence may make.
const CLONES: usize = 2;

/// Passes as often as one ends, after commits, compacting what is half freed and deleting it
/// at once.
pub(super) const BUSY: Pace = Pace {
    every: Duration::ZERO,
    options: extent_remote::collector::Options { compact_below: 0.5, grace: 0 },
};

/// The files of a filesystem, by their names: what they hold.
pub(super) type Files = BTreeMap<Vec<u8>, Vec<u8>>;

/// Checks that `fs` holds `files`, each as long and of the same bytes, and no other.
pub(super) async fn holds(fs: &Filesystem, files: &Files, what: &str) {
    for (name, contents) in files {
        file_holds(fs, name, Some(contents), what).await;
    }
    let entries = fs.root().entries().await.unwrap().into_iter().map(|entry| entry.name);
    let mut names: Vec<Vec<u8>> =
        entries.filter(|name| !matches!(&name[..], b"." | b"..")).collect();
    names.sort();
    assert_eq!(names, files.keys().cloned().collect::<Vec<_>>(), "{what}: its files");
}

/// Checks that `fs`'s file `name` holds `contents`, as long and of the same bytes, or, if none,
/// that there is no such file.
async fn file_holds(fs: &Filesystem, name: &[u8], contents: Option<&Vec<u8>>, what: &str) {
    let Some(contents) = contents else {
        assert!(fs.root().lookup(name).await.is_err(), "{what}: {name:?} is there");
        return;
    };
    let (node, attr) = fs.root().lookup(name).await.unwrap();
    assert_eq!(attr.size, contents.len() as u64, "{what}: {name:?}'s size");
    let mut read = vec![0; contents.len()];
    assert_eq!(node.read(0, &mut read).await.unwrap(), contents.len(), "{what}");
    assert!(read == *contents, "{what}: {name:?} differs");
}

/// What a sequence asks: of the image or clone `on` (of those made so far, wrapping), of its file
/// `file`.
#[derive(Clone, Debug)]
enum Op {
    /// `mibs` MiB of noise of `seed`, at `at` MiB, the file made if it has none.
    Write {
        on: usize,
        file: usize,
        at: usize,
        mibs: usize,
        seed: u64,
    },
    /// The file cut, or grown, to `quarters` quarter MiB.
    Truncate {
        on: usize,
        file: usize,
        quarters: usize,
    },
    Unlink {
        on: usize,
        file: usize,
    },
    /// The file synced: a commit, which may start a pass.
    Fsync {
        on: usize,
        file: usize,
    },
    /// A clone of the image or clone, mounted, which later operations go to as well.
    Clone {
        on: usize,
    },
    /// The image or clone closed, and mounted again.
    Remount {
        on: usize,
    },
    /// A pass of the bucket's collector, alone, with no grace period.
    Collect,
}

fn op() -> impl Strategy<Value = Op> {
    let on = || 0..=CLONES;
    let file = || 0..FILES;
    prop_oneof![
        8 => (on(), file(), 0usize..4, 1usize..=5, any::<u64>())
            .prop_map(|(on, file, at, mibs, seed)| Op::Write { on, file, at, mibs, seed }),
        2 => (on(), file(), 0usize..32)
            .prop_map(|(on, file, quarters)| Op::Truncate { on, file, quarters }),
        2 => (on(), file()).prop_map(|(on, file)| Op::Unlink { on, file }),
        4 => (on(), file()).prop_map(|(on, file)| Op::Fsync { on, file }),
        1 => on().prop_map(|on| Op::Clone { on }),
        2 => on().prop_map(|on| Op::Remount { on }),
        3 => Just(Op::Collect),
    ]
}

/// An image, or a clone, mounted, and what its files hold.
struct Mounted {
    path: PathBuf,
    fs: Filesystem,
    files: Files,
}

/// A sequence's images, in its directory, and their bucket's collector.
struct Images {
    dir: PathBuf,
    mounted: Vec<Mounted>,
    collecting: Arc<Collecting>,
}

fn name(file: usize) -> Vec<u8> {
    format!("file{file}").into_bytes()
}

impl Images {
    fn new(dir: &Path) -> Self {
        let path = dir.join("image");
        let storage = Storage::create(&path, SIZE).unwrap();
        let super::Kind::Image { image, .. } = &storage.kind else { panic!("memory") };
        let collecting = image.collecting().expect("its own bucket's").clone();
        let fs = block_on(Filesystem::format(storage)).unwrap();
        let mounted = vec![Mounted { path, fs, files: Files::new() }];
        Self { dir: dir.to_owned(), mounted, collecting }
    }

    fn on(&mut self, on: usize) -> &mut Mounted {
        let len = self.mounted.len();
        &mut self.mounted[on % len]
    }

    /// Applies `op`, and checks what it changed: the file it was of, or, cloning, remounting or
    /// collecting, every file of every image.
    fn apply(&mut self, op: &Op, step: usize) {
        let what = format!("step {step}, {op:?}");
        let (on, file) = match *op {
            Op::Write { on, file, at, mibs, seed } => {
                let Mounted { fs, files, .. } = self.on(on);
                let (name, data) = (name(file), crate::test_data::noise(seed, mibs * MIB));
                block_on(async {
                    let node = match fs.root().lookup(&name).await {
                        Ok((node, _)) => node,
                        Err(_) => fs.root().create(&name, 0o644, OWNER).await.unwrap().0,
                    };
                    node.write((at * MIB) as u64, &data).await.unwrap();
                });
                let contents = files.entry(name).or_default();
                let (start, end) = (at * MIB, at * MIB + data.len());
                contents.resize(contents.len().max(end), 0);
                contents[start..end].copy_from_slice(&data);
                (on, file)
            }
            Op::Truncate { on, file, quarters } => {
                let Mounted { fs, files, .. } = self.on(on);
                let Some(contents) = files.get_mut(&name(file)) else { return };
                let len = quarters * MIB / 4;
                block_on(async {
                    let (node, _) = fs.root().lookup(&name(file)).await.unwrap();
                    let size = SetAttr { size: Some(len as u64), ..SetAttr::default() };
                    node.setattr(&size).await.unwrap();
                });
                contents.resize(len, 0);
                (on, file)
            }
            Op::Unlink { on, file } => {
                let Mounted { fs, files, .. } = self.on(on);
                if files.remove(&name(file)).is_some() {
                    block_on(fs.root().unlink(&name(file))).unwrap();
                }
                (on, file)
            }
            Op::Fsync { on, file } => {
                let Mounted { fs, files, .. } = self.on(on);
                if files.contains_key(&name(file)) {
                    block_on(async {
                        let (node, _) = fs.root().lookup(&name(file)).await.unwrap();
                        node.fsync().await.unwrap();
                    });
                }
                (on, file)
            }
            Op::Clone { on } => {
                if self.mounted.len() > CLONES {
                    return;
                }
                let path = self.dir.join(format!("clone{}", self.mounted.len()));
                let Mounted { fs, files, .. } = self.on(on);
                let files = files.clone();
                let storage = block_on(fs.clone_storage_to(&path)).unwrap();
                let fs = block_on(Filesystem::mount(storage)).unwrap();
                self.mounted.push(Mounted { path, fs, files });
                return self.check(&what);
            }
            Op::Remount { on } => {
                let len = self.mounted.len();
                let Mounted { path, fs, files } = self.mounted.remove(on % len);
                let storage = block_on(fs.close()).unwrap();
                let fs = block_on(Filesystem::mount(storage)).unwrap();
                self.mounted.insert(on % len, Mounted { path, fs, files });
                return self.check(&what);
            }
            Op::Collect => {
                self.collecting.pass_now();
                return self.check(&what);
            }
        };
        let Mounted { fs, files, .. } = self.on(on);
        block_on(file_holds(fs, &name(file), files.get(&name(file)), &what));
    }

    /// Checks every file of every image.
    fn check(&self, what: &str) {
        for Mounted { fs, files, .. } in &self.mounted {
            block_on(holds(fs, files, what));
        }
    }

    /// Closes every image, collects alone, leaving nothing to collect, and checks each, opened
    /// anew, holds what it did.
    fn end(self) {
        let Self { mounted, collecting, .. } = self;
        let mut closed = Vec::new();
        for Mounted { path, fs, files } in mounted {
            block_on(holds(&fs, &files, &format!("{} at the end", path.display())));
            drop(block_on(fs.close()).unwrap());
            closed.push((path, files));
        }
        assert_eq!(collecting.collected_alone(), Vec::<String>::new(), "collected alone");
        drop(collecting);
        for (path, files) in closed {
            let fs = block_on(Filesystem::mount(Storage::open(&path).unwrap())).unwrap();
            block_on(holds(&fs, &files, &format!("{} opened anew", path.display())));
            drop(block_on(fs.close()).unwrap());
        }
    }
}

/// TASK-291.4: whatever an image and its clones are asked, and whenever their bucket is
/// collected, among their commits and as an operation of their own, what their files were written
/// they hold, mounted, remounted and opened anew.
#[test]
fn images_hold_their_files_whatever_is_collected() {
    quicken(BUSY);
    let cases =
        std::env::var("FXDAX_COLLECT_MODEL_CASES").map_or(4, |cases| cases.parse().unwrap());
    let config = Config { cases, failure_persistence: None, ..Config::default() };
    TestRunner::new(config)
        .run(&proptest::collection::vec(op(), 1..24), |ops| {
            let dir = tempfile::tempdir().unwrap();
            let mut images = Images::new(dir.path());
            for (step, op) in ops.iter().enumerate() {
                images.apply(op, step);
            }
            images.end();
            Ok(())
        })
        .unwrap();
}
