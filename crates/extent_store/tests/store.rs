//! The store's behaviour, in memory and in a directory of the host's.

use extent_store::{
    Committed, Description, Dir, HostDir, Lineage, MemDir, NoSpace, Options, RemoteData, Store,
};
use storage_device::{ExtentId, NoExtent, OtherBranch, SealedExtent};

const BLOCK: usize = 4096;
const BRANCH: u64 = 7;

/// Small segments and large extents, so that tests make several of each.
const OPTIONS: Options = Options {
    segment_size: 64 << 10,
    large_extent: 32 << 10,
    checkpoint_after: 4 << 10,
    open_files: 2,
};

fn extent(sequence: u64) -> ExtentId {
    ExtentId::new(BRANCH, sequence)
}

/// The branch's directory of `kind` (`segments`, `extents`, `index`).
fn of(kind: &str, branch: u64) -> String {
    format!("{kind}/{branch:016x}")
}

/// Bytes each their offset modulo 251, from `offset`, plus `seed`.
fn pattern(seed: u8, offset: usize, len: usize) -> Vec<u8> {
    (offset..offset + len).map(|i| (i % 251) as u8 ^ seed).collect()
}

fn create<D: Dir>(dir: D, capacity: u64) -> Store<D> {
    let description = Description { block_size: BLOCK as u32, capacity, branch: BRANCH };
    Store::create(dir, description, OPTIONS).unwrap()
}

fn read<D: Dir>(store: &Store<D>, extent: ExtentId, offset: u64, len: usize) -> Vec<u8> {
    let mut data = vec![0; len];
    store.read(extent, offset, &mut data).unwrap();
    data
}

fn error_is<E: std::error::Error + PartialEq + Send + Sync + 'static>(
    result: Result<(), anyhow::Error>,
    expected: E,
) {
    let error = result.expect_err("succeeded");
    assert_eq!(error.downcast_ref::<E>(), Some(&expected), "{error:?}");
}

/// Writes, reads, commits and reopens a store in `dir`, which `reopen` opens again.
fn reads_what_was_written<D: Dir>(dir: D, reopen: impl Fn(&D) -> D) {
    let store = create(dir, 64 << 20);
    // Small extents, in segments, one in pieces, and a large one, in a file of its own.
    store.write(extent(1), 0, &pattern(1, 0, 3 * BLOCK)).unwrap();
    store.write(extent(2), 2 * BLOCK as u64, &pattern(2, 0, BLOCK)).unwrap();
    store.write(extent(2), 0, &pattern(3, 0, BLOCK)).unwrap();
    store.write(extent(2), 2 * BLOCK as u64, &pattern(4, 0, BLOCK)).unwrap();
    store.write(extent(3), 0, &pattern(5, 0, 40 << 10)).unwrap();

    let check = |store: &Store<D>| {
        assert_eq!(read(store, extent(1), 0, 3 * BLOCK), pattern(1, 0, 3 * BLOCK));
        // Blocks never written read as zeros, within an extent and past its end; the later of
        // two writes of a block holds.
        assert_eq!(
            read(store, extent(2), 0, 4 * BLOCK),
            [pattern(3, 0, BLOCK), vec![0; BLOCK], pattern(4, 0, BLOCK), vec![0; BLOCK]].concat()
        );
        assert_eq!(
            read(store, extent(3), BLOCK as u64, 40 << 10),
            [pattern(5, BLOCK, 36 << 10), vec![0; BLOCK]].concat()
        );
        assert_eq!(
            store.extents().unwrap(),
            vec![
                (extent(1), 3 * BLOCK as u64),
                (extent(2), 3 * BLOCK as u64),
                (extent(3), 40 << 10)
            ]
        );
        assert_eq!(store.info().unwrap().used, 64 << 10);
        assert_eq!(store.info().unwrap().next_sequence, 4);
        error_is(store.read(extent(4), 0, &mut [0; BLOCK]), NoExtent(extent(4)));
    };
    check(&store);
    assert_eq!(store.usage().unwrap().own_bytes, 40 << 10);
    store.commit(Some(b"root")).unwrap();
    check(&store);

    // Uncommitted writes and frees are gone after reopening.
    store.write(extent(9), 0, &pattern(9, 0, BLOCK)).unwrap();
    store.free(extent(1)).unwrap();
    let dir = reopen(store.dir());
    drop(store);
    let store = Store::open(dir, OPTIONS).unwrap();
    check(&store);
    assert_eq!(store.root().unwrap(), b"root");
    assert!(store.epoch().unwrap() >= 1);
}

#[test]
fn reads_what_was_written_in_memory() {
    reads_what_was_written(MemDir::new(), MemDir::clone);
}

#[test]
fn reads_what_was_written_on_the_host() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().to_owned();
    reads_what_was_written(HostDir::new(&path, true).unwrap(), |dir| {
        HostDir::new(dir.path(), true).unwrap()
    });
}

/// Extents are written once: commits seal them. A store writes only its branch's extents.
#[test]
fn writes_once() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &[1; BLOCK]).unwrap();
    store.write(extent(1), BLOCK as u64, &[2; BLOCK]).unwrap();
    store.commit(None).unwrap();
    error_is(store.write(extent(1), 2 * BLOCK as u64, &[3; BLOCK]), SealedExtent(extent(1)));
    error_is(
        store.write(ExtentId::new(BRANCH + 1, 1), 0, &[3; BLOCK]),
        OtherBranch(ExtentId::new(BRANCH + 1, 1)),
    );
    assert!(store.write(extent(2), 1, &[3; BLOCK]).is_err(), "unaligned");
    assert_eq!(read(&store, extent(1), 0, 2 * BLOCK), [[1; BLOCK], [2; BLOCK]].concat());
}

/// The store holds at most its capacity, and frees make room.
#[test]
fn capacity() {
    let store = create(MemDir::new(), 64 << 10);
    store.write(extent(1), 0, &[1; 48 << 10]).unwrap();
    error_is(store.write(extent(2), 0, &[2; 32 << 10]), NoSpace);
    assert_eq!(store.info().unwrap().available, 16 << 10);
    store.free(extent(1)).unwrap();
    store.free(extent(1)).expect("freeing a freed extent failed");
    store.write(extent(2), 0, &[2; 32 << 10]).unwrap();
    // Freeing doesn't make sequence numbers available again.
    assert_eq!(store.info().unwrap().next_sequence, 3);
}

/// Freed extents' files, and segments whose extents are all freed, are removed once the frees are
/// committed.
#[test]
fn frees_remove_files() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    // Fills the first segment, so that the commit starts another.
    for sequence in 0..16 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    store.write(extent(16), 0, &pattern(16, 0, 32 << 10)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("segments", BRANCH)).unwrap().len(), 2);
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), vec!["0000000000000010"]);

    for sequence in 0..=16 {
        store.free(extent(sequence)).unwrap();
    }
    // Not until the frees are committed.
    assert_eq!(dir.list(&of("segments", BRANCH)).unwrap().len(), 2);
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap().len(), 1);
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("segments", BRANCH)).unwrap().len(), 1);
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), Vec::<String>::new());
    error_is(store.read(extent(16), 0, &mut [0; BLOCK]), NoExtent(extent(16)));
}

/// fxdax: an uploader takes what each durable commit changed, and reads the data of extents freed
/// since, whose files stay until it releases them.
#[test]
fn uploads_keep_freed_data_until_released() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    // Before uploading began: the uploader starts from the store's extents.
    let snapshot = store.upload().unwrap();
    assert_eq!((snapshot.epoch, snapshot.extents), (1, vec![(extent(1), BLOCK as u64)]));
    assert_eq!(store.committed().unwrap(), vec![]);

    for sequence in 2..=16 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
    }
    store.write(extent(17), 0, &pattern(17, 0, 32 << 10)).unwrap();
    store.commit_ordered(Some(b"root")).unwrap();
    // Not until durable.
    assert_eq!(store.committed().unwrap(), vec![]);
    store.flush().unwrap();
    let mut written: Vec<_> = (2..=16).map(|sequence| (extent(sequence), BLOCK as u64)).collect();
    written.push((extent(17), 32 << 10));
    let committed = Committed { epoch: 2, written, freed: vec![], root: Some(b"root".to_vec()) };
    assert_eq!(store.committed().unwrap(), vec![committed]);
    assert_eq!(store.unreleased_bytes().unwrap(), 15 * BLOCK as u64 + (32 << 10));

    for sequence in 1..=17 {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
    let committed = store.committed().unwrap();
    assert_eq!(committed.len(), 1);
    assert_eq!(committed[0].freed, (1..=17).map(extent).collect::<Vec<_>>());
    // Freed, and kept: the uploader reads them, the store's users don't.
    error_is(store.read(extent(17), 0, &mut [0; BLOCK]), NoExtent(extent(17)));
    let mut data = vec![0; 32 << 10];
    store.read_kept(extent(17), 0, &mut data).unwrap();
    assert_eq!(data, pattern(17, 0, 32 << 10));
    store.read_kept(extent(3), 0, &mut data[..BLOCK]).unwrap();
    assert_eq!(&data[..BLOCK], pattern(3, 0, BLOCK));
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap().len(), 1);
    let segments = dir.list(&of("segments", BRANCH)).unwrap().len();

    // Released, they go with the next commit.
    store.release(3).unwrap();
    assert_eq!(store.unreleased_bytes().unwrap(), 0);
    error_is(store.read_kept(extent(17), 0, &mut data), NoExtent(extent(17)));
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), Vec::<String>::new());
    assert!(dir.list(&of("segments", BRANCH)).unwrap().len() < segments);
}

/// fxdax: a store holds extents whose data is elsewhere: committed and recovered like others, and
/// freed so, but read from elsewhere.
#[test]
fn adopted_extents_are_held_without_their_data() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 1 << 20);
    store.upload().unwrap();
    store.adopt(extent(5), 3 * BLOCK as u64).unwrap();
    store.adopt(ExtentId::new(BRANCH + 1, 9), BLOCK as u64).unwrap();
    assert!(store.adopt(extent(5), BLOCK as u64).is_err());
    store.commit(None).unwrap();
    // Not uploaded again.
    assert_eq!(store.committed().unwrap()[0].written, vec![]);
    let check = |store: &Store<MemDir>| {
        assert_eq!(store.extent(extent(5)).unwrap(), Some((3 * BLOCK as u64, false)));
        error_is(store.read(extent(5), 0, &mut [0; BLOCK]), RemoteData(extent(5)));
        assert_eq!(store.info().unwrap().used, 4 * BLOCK as u64);
        assert_eq!(store.info().unwrap().next_sequence, 6);
        // Sealed.
        error_is(store.write(extent(5), 0, &[0; BLOCK]), SealedExtent(extent(5)));
    };
    check(&store);
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    check(&store);
    store.upload().unwrap();
    store.free(extent(5)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(store.committed().unwrap()[0].freed, vec![extent(5)]);
    error_is(store.read(extent(5), 0, &mut [0; BLOCK]), NoExtent(extent(5)));
}

/// Commits append deltas to the index, which a checkpoint in a new file replaces once they are
/// many.
#[test]
fn checkpoints() {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    for sequence in 0..200 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        if sequence % 3 == 0 {
            store.free(extent(sequence)).unwrap();
        }
        store.commit(Some(format!("root {sequence}").as_bytes())).unwrap();
    }
    assert_eq!(dir.list(&of("index", BRANCH)).unwrap().len(), 1);
    let first_index = dir.list(&of("index", BRANCH)).unwrap();
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    assert_ne!(dir.list(&of("index", BRANCH)).unwrap(), first_index);
    assert_eq!(dir.list(&of("index", BRANCH)).unwrap().len(), 1);
    assert_eq!(store.root().unwrap(), b"root 199");
    assert_eq!(store.epoch().unwrap(), 200);
    let held: Vec<u64> =
        store.extents().unwrap().iter().map(|(extent, _)| extent.sequence).collect();
    assert_eq!(held, (0..200).filter(|sequence| sequence % 3 != 0).collect::<Vec<_>>());
    for sequence in held {
        assert_eq!(read(&store, extent(sequence), 0, BLOCK), pattern(sequence as u8, 0, BLOCK));
    }
}

/// Cleaning copies the extents of segments mostly freed to the open segment, and removes them.
fn cleaning<D: Dir>(dir: D, reopen: impl Fn(&D) -> D) {
    let store = create(dir, 64 << 20);
    // 8 segments of 16 extents.
    for sequence in 0..128 {
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, BLOCK)).unwrap();
        if sequence % 16 == 15 {
            store.commit(None).unwrap();
        }
    }
    let full = store.usage().unwrap();
    assert_eq!((full.segments, full.segment_bytes), (9, 128 * BLOCK as u64));
    // Keep one extent in four in the first four segments, and every other in the rest.
    let kept: Vec<u64> = (0..128)
        .filter(|sequence| if *sequence < 64 { sequence % 4 == 0 } else { sequence % 2 == 0 })
        .collect();
    for sequence in (0..128).filter(|sequence| !kept.contains(sequence)) {
        store.free(extent(sequence)).unwrap();
    }
    store.commit(None).unwrap();
    assert_eq!(store.usage().unwrap().live_bytes, kept.len() as u64 * BLOCK as u64);

    // The first four segments are a quarter live, and the others half.
    let cleaned = store.clean(0.4).unwrap();
    assert_eq!(cleaned.segments, 4);
    assert_eq!(cleaned.bytes, 64 * BLOCK as u64);
    assert_eq!(cleaned.copied, 16 * BLOCK as u64);
    let usage = store.usage().unwrap();
    assert_eq!(usage.segment_bytes, full.segment_bytes - cleaned.bytes + cleaned.copied);
    assert_eq!(usage.live_bytes, kept.len() as u64 * BLOCK as u64);

    let check = |store: &Store<D>| {
        let held: Vec<u64> =
            store.extents().unwrap().iter().map(|(extent, _)| extent.sequence).collect();
        assert_eq!(held, kept);
        for sequence in &kept {
            assert_eq!(
                read(store, extent(*sequence), 0, BLOCK),
                pattern(*sequence as u8, 0, BLOCK)
            );
        }
    };
    check(&store);
    let dir = reopen(store.dir());
    drop(store);
    let store = Store::open(dir, OPTIONS).unwrap();
    check(&store);
    assert_eq!(store.usage().unwrap().segment_bytes, usage.segment_bytes);
}

#[test]
fn cleaning_in_memory() {
    cleaning(MemDir::new(), MemDir::clone);
}

#[test]
fn cleaning_on_the_host() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().to_owned();
    cleaning(HostDir::new(&path, true).unwrap(), |dir| HostDir::new(dir.path(), true).unwrap());
}

/// Every file in `dir`, with its size: the store's description, branches' records, and each
/// branch's segments, extents' files and index files.
fn files(dir: &MemDir) -> std::collections::BTreeMap<String, u64> {
    let mut files = std::collections::BTreeMap::new();
    let mut add = |path: String| {
        let size = extent_store::File::size(&dir.open(&path, false).unwrap()).unwrap();
        files.insert(path, size);
    };
    for name in dir.list("").unwrap() {
        add(name);
    }
    for name in dir.list("branches").unwrap() {
        add(format!("branches/{name}"));
    }
    for kind in ["segments", "extents", "index"] {
        for branch in dir.list(kind).unwrap_or_default() {
            for name in dir.list(&format!("{kind}/{branch}")).unwrap() {
                add(format!("{kind}/{branch}/{name}"));
            }
        }
    }
    files
}

/// A store of `count` small extents and `count / 8` large ones, committed.
fn holding(count: u64) -> (MemDir, Store<MemDir>) {
    let dir = MemDir::new();
    let store = create(dir.clone(), 64 << 20);
    for sequence in 0..count {
        let len = if sequence % 8 == 7 { 32 << 10 } else { BLOCK };
        store.write(extent(sequence), 0, &pattern(sequence as u8, 0, len)).unwrap();
    }
    store.commit(None).unwrap();
    (dir, store)
}

/// A fork writes its branch's record, whatever the store holds: no extent's data, nor its index.
#[test]
fn forks_take_constant_time_and_space() {
    let mut added = Vec::new();
    for count in [16, 1024] {
        let (dir, store) = holding(count);
        let before = files(&dir);
        store.fork(BRANCH + 1).unwrap();
        let after = files(&dir);
        let new: Vec<&String> = after.keys().filter(|path| !before.contains_key(*path)).collect();
        assert_eq!(new, ["branches/0000000000000008.0000000000000000"]);
        // And the fork's commit, which seals nothing, in the parent's index.
        let grown: Vec<(&String, u64)> = (before.iter())
            .filter(|(path, size)| after[*path] != **size)
            .map(|(path, size)| (path, after[path] - size))
            .collect();
        added.push((grown.len(), grown.iter().map(|(_, bytes)| bytes).sum::<u64>()));
    }
    assert_eq!(added[0], added[1]);
}

/// A fork seals its parent's open extents: the parent writes them no more, and the fork reads
/// them.
#[test]
fn forks_seal_their_parents_open_extents() {
    let store = create(MemDir::new(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.fork(BRANCH + 1).unwrap();
    error_is(store.write(extent(1), BLOCK as u64, &[0; BLOCK]), SealedExtent(extent(1)));
    let child = store.lineage().open_branch(BRANCH + 1).unwrap();
    assert_eq!(read(&child, extent(1), 0, BLOCK), pattern(1, 0, BLOCK));
    assert_eq!(child.extents().unwrap(), vec![(extent(1), BLOCK as u64)]);
}

/// A parent and its fork read every extent the parent held at the fork, and free them each on
/// its own; an extent both free goes. So it is whether the fork is opened before the parent frees
/// them or after, and across reopening the store.
fn forks_free_on_their_own(open_child_first: bool) {
    let child_branch = BRANCH + 1;
    let (dir, store) = holding(48);
    // 3 segments' worth, and 6 extents of their own.
    let all: Vec<u64> = (0..48).collect();
    let len = |sequence: u64| if sequence % 8 == 7 { 32 << 10 } else { BLOCK };
    store.fork(child_branch).unwrap();
    let lineage = store.lineage().clone();
    let child = open_child_first.then(|| lineage.open_branch(child_branch).unwrap());

    // The parent frees the even extents, and writes one of its own.
    for sequence in all.iter().filter(|sequence| *sequence % 2 == 0) {
        store.free(extent(*sequence)).unwrap();
    }
    store.write(extent(100), 0, &pattern(100, 0, BLOCK)).unwrap();
    store.commit(None).unwrap();
    let child = child.unwrap_or_else(|| lineage.open_branch(child_branch).unwrap());
    let check_inherited = |child: &Store<MemDir>, held: &[u64]| {
        let extents: Vec<u64> =
            child.extents().unwrap().iter().map(|(extent, _)| extent.sequence).collect();
        assert_eq!(extents, held);
        for &sequence in held {
            assert_eq!(
                read(child, extent(sequence), 0, len(sequence)),
                pattern(sequence as u8, 0, len(sequence))
            );
        }
    };
    check_inherited(&child, &all);
    error_is(child.read(extent(100), 0, &mut [0; BLOCK]), NoExtent(extent(100)));

    // The child frees the extents divisible by three, and writes one of its own.
    for sequence in all.iter().filter(|sequence| *sequence % 3 == 0) {
        child.free(extent(*sequence)).unwrap();
    }
    child.write(ExtentId::new(child_branch, 0), 0, &pattern(200, 0, BLOCK)).unwrap();
    child.commit(None).unwrap();
    let parent_held: Vec<u64> =
        all.iter().copied().filter(|sequence| sequence % 2 == 1).chain([100]).collect();
    let child_held: Vec<u64> = all.iter().copied().filter(|sequence| sequence % 3 != 0).collect();
    let check_parent = |store: &Store<MemDir>| {
        let extents: Vec<u64> =
            store.extents().unwrap().iter().map(|(extent, _)| extent.sequence).collect();
        assert_eq!(extents, parent_held);
        for &sequence in &parent_held {
            let len = if sequence == 100 { BLOCK } else { len(sequence) };
            assert_eq!(read(store, extent(sequence), 0, len), pattern(sequence as u8, 0, len));
        }
    };
    check_parent(&store);
    let check_child = |child: &Store<MemDir>| {
        let extents: Vec<ExtentId> =
            child.extents().unwrap().iter().map(|(extent, _)| *extent).collect();
        // Its own extents' IDs, of its own branch, sort after its parent's.
        let mut expected: Vec<ExtentId> =
            child_held.iter().map(|sequence| extent(*sequence)).collect();
        expected.push(ExtentId::new(child_branch, 0));
        expected.sort();
        assert_eq!(extents, expected);
        for &sequence in &child_held {
            assert_eq!(
                read(child, extent(sequence), 0, len(sequence)),
                pattern(sequence as u8, 0, len(sequence))
            );
        }
    };
    check_child(&child);

    // The extents of their own both freed are gone; those either holds stay.
    let own_files: Vec<String> = dir.list(&of("extents", BRANCH)).unwrap();
    let expected: Vec<String> = (all.iter())
        .filter(|sequence| *sequence % 8 == 7 && (*sequence % 2 == 1 || *sequence % 3 != 0))
        .map(|sequence| format!("{sequence:016x}"))
        .collect();
    assert_eq!(own_files, expected);

    // And so after reopening the store.
    drop((store, child, lineage));
    let lineage = Lineage::open(dir.clone(), OPTIONS).unwrap();
    assert_eq!(lineage.branches(), vec![(BRANCH, None), (child_branch, Some(BRANCH))]);
    check_parent(&lineage.open_branch(BRANCH).unwrap());
    check_child(&lineage.open_branch(child_branch).unwrap());
}

#[test]
fn forks_opened_at_once_free_on_their_own() {
    forks_free_on_their_own(true);
}

#[test]
fn forks_opened_later_free_on_their_own() {
    forks_free_on_their_own(false);
}

/// A fork's parent keeps what the fork needs until it is opened, across reopening the store, and
/// cleaning.
#[test]
fn forks_not_yet_opened_keep_what_they_inherited() {
    let child_branch = BRANCH + 1;
    let (dir, store) = holding(64);
    store.fork(child_branch).unwrap();
    // The parent lets every extent go, and cleans, and checkpoints its index anew.
    for sequence in 0..64 {
        store.free(extent(sequence)).unwrap();
    }
    for _ in 0..8 {
        store.commit(Some(b"later")).unwrap();
    }
    store.clean(0.5).unwrap();
    assert_eq!(store.extents().unwrap(), vec![]);
    drop(store);
    let lineage = Lineage::open(dir.clone(), OPTIONS).unwrap();
    let child = lineage.open_branch(child_branch).unwrap();
    assert_eq!(child.extents().unwrap().len(), 64);
    for sequence in 0..64 {
        let len = if sequence % 8 == 7 { 32 << 10 } else { BLOCK };
        assert_eq!(read(&child, extent(sequence), 0, len), pattern(sequence as u8, 0, len));
    }
    assert_eq!(child.root().unwrap(), b"");
    // Opened, its parent's index as of the fork goes.
    drop(child);
    let parent = lineage.open_branch(BRANCH).unwrap();
    assert_eq!(parent.root().unwrap(), b"later");
    assert_eq!(dir.list(&of("index", BRANCH)).unwrap().len(), 1);
}

/// A deleted branch's extents go, but for those another branch holds.
#[test]
fn deleting_a_branch_lets_its_extents_go() {
    let child_branch = BRANCH + 1;
    let (dir, store) = holding(16);
    store.fork(child_branch).unwrap();
    let lineage = store.lineage().clone();
    let child = lineage.open_branch(child_branch).unwrap();
    child.write(ExtentId::new(child_branch, 0), 0, &pattern(1, 0, 32 << 10)).unwrap();
    child.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", child_branch)).unwrap().len(), 1);
    assert!(lineage.delete(child_branch).is_err(), "deleted an open branch");
    drop(child);
    lineage.delete(child_branch).unwrap();
    assert_eq!(lineage.branches(), vec![(BRANCH, None)]);
    assert_eq!(dir.list(&of("extents", child_branch)).unwrap(), Vec::<String>::new());
    assert_eq!(dir.list(&of("index", child_branch)).unwrap(), Vec::<String>::new());
    // The parent's extents stay; freed, they go.
    let own = vec!["0000000000000007", "000000000000000f"];
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), own);
    store.free(extent(7)).unwrap();
    store.free(extent(15)).unwrap();
    store.commit(None).unwrap();
    assert_eq!(dir.list(&of("extents", BRANCH)).unwrap(), Vec::<String>::new());
    assert!(lineage.open_branch(child_branch).is_err());
}
