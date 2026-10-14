//! The store's behaviour, in memory and in a directory of the host's.

use extent_store::{Description, Dir, HostDir, MemDir, NoSpace, Options, Store, copy_store};
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
    assert_eq!(dir.list("segments").unwrap().len(), 2);
    assert_eq!(dir.list("extents").unwrap(), vec!["0000000000000010.0000000000000007"]);

    for sequence in 0..=16 {
        store.free(extent(sequence)).unwrap();
    }
    // Not until the frees are committed.
    assert_eq!(dir.list("segments").unwrap().len(), 2);
    assert_eq!(dir.list("extents").unwrap().len(), 1);
    store.commit(None).unwrap();
    assert_eq!(dir.list("segments").unwrap().len(), 1);
    assert_eq!(dir.list("extents").unwrap(), Vec::<String>::new());
    error_is(store.read(extent(16), 0, &mut [0; BLOCK]), NoExtent(extent(16)));
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
    assert_eq!(dir.list("index").unwrap().len(), 1);
    let first_index = dir.list("index").unwrap();
    drop(store);
    let store = Store::open(dir.clone(), OPTIONS).unwrap();
    assert_ne!(dir.list("index").unwrap(), first_index);
    assert_eq!(dir.list("index").unwrap().len(), 1);
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

/// A copy of a store is a store of its own, with a new branch, which holds the store's extents as
/// of its last commit, and none of its own yet.
#[test]
fn copies() {
    let temp = tempfile::tempdir().unwrap();
    let (path, copy_path) = (temp.path().join("store"), temp.path().join("copy"));
    std::fs::create_dir(&path).unwrap();
    let store = create(HostDir::new(&path, true).unwrap(), 1 << 20);
    store.write(extent(1), 0, &pattern(1, 0, BLOCK)).unwrap();
    store.write(extent(2), 0, &pattern(2, 0, 32 << 10)).unwrap();
    store.commit(Some(b"root")).unwrap();
    store.write(extent(3), 0, &pattern(3, 0, BLOCK)).unwrap();

    let mut copied = Vec::new();
    copy_store(&path, &copy_path, BRANCH + 1, |file, to| {
        copied.push(to.strip_prefix(&copy_path).unwrap().to_owned());
        std::io::copy(&mut &*file, &mut std::fs::File::create_new(to)?).map(|_| ())
    })
    .unwrap();
    copied.sort();
    let names = [
        "extents/0000000000000002.0000000000000007",
        "index/0000000000000001",
        "segments/0000000000000000",
    ];
    assert_eq!(copied, names.map(std::path::PathBuf::from));
    assert!(
        copy_store(&path, &copy_path, BRANCH + 2, |_, _| Ok(())).is_err(),
        "copied over a copy"
    );

    let copy = Store::open(HostDir::new(&copy_path, true).unwrap(), OPTIONS).unwrap();
    assert_eq!(copy.description().branch, BRANCH + 1);
    assert_eq!(copy.root().unwrap(), b"root");
    assert_eq!(copy.extents().unwrap(), vec![(extent(1), BLOCK as u64), (extent(2), 32 << 10)]);
    assert_eq!(read(&copy, extent(2), 0, 32 << 10), pattern(2, 0, 32 << 10));
    assert_eq!(copy.info().unwrap().next_sequence, 0);
    // Each writes its own branch's extents, and frees inherited ones on its own.
    copy.write(ExtentId::new(BRANCH + 1, 0), 0, &pattern(4, 0, BLOCK)).unwrap();
    copy.free(extent(2)).unwrap();
    copy.commit(None).unwrap();
    assert_eq!(read(&store, extent(2), 0, 32 << 10), pattern(2, 0, 32 << 10));
    assert_eq!(store.info().unwrap().next_sequence, 4);
}
