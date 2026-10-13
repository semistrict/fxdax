//! How much stack Fxfs's operations take when one thread polls them all, as a guest kernel's
//! threads would (TASK-26.1). Linux gives each kernel thread 16 KiB of stack on x86-64 and arm64,
//! some of which the system call and the VFS have used before a filesystem is called.
//!
//! Prints, for each phase of a workload, the most stack it used below the executor's frames, as a
//! Markdown table. Only optimized builds say anything about a kernel's, so it runs only in one:
//!
//! ```sh
//! cargo run --release -p fxdax --example fxfs-stack
//! ```

use fuchsia_async::LocalExecutor;
use fxfs::filesystem::{FxFilesystem, SyncOptions};
use fxfs::fsck::fsck;
use fxfs::object_handle::{ObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::replace_child;
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{Directory, NewChildStoreOptions, ObjectStore};
use std::cell::Cell;
use std::sync::Arc;
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

const KIB: usize = 1024;
const BLOCK_SIZE: u32 = 4096;
/// Directories, and files in each, the workload creates.
const DIRS: usize = 16;
const FILES: usize = 64;

thread_local! {
    /// The lowest address of the current thread's stack.
    static STACK_LOW: usize = stack_low();
    /// Where `paint` was called from, and the range below it that it painted.
    static PAINTED: Cell<(usize, usize, usize)> = const { Cell::new((0, 0, 0)) };
}

const PAINT: u8 = 0xa5;
/// What `paint` leaves unpainted below its frame, for the painting's own frames.
const MARGIN: usize = 1024;

/// Paints the current thread's stack below the caller's frame, so that `deepest` can tell how
/// deep it is used from here on.
#[inline(never)]
fn paint() {
    let low = STACK_LOW.with(|low| *low);
    let here = std::hint::black_box(&low) as *const usize as usize;
    // From a page above the guard.
    let (start, end) = (low + 4 * KIB, here - MARGIN);
    // SAFETY: The range lies within this thread's stack, below every live frame.
    unsafe { std::ptr::write_bytes(start as *mut u8, PAINT, end - start) };
    PAINTED.set((here, start, end));
}

/// How deep the current thread's stack has been used below where `paint` was called: to the
/// deepest byte that changed, or to the end of the margin if none did.
#[inline(never)]
fn deepest() -> usize {
    let (here, start, end) = PAINTED.get();
    // SAFETY: `paint` painted the range, below the frames of its caller, which is this one's.
    let painted = unsafe { std::slice::from_raw_parts(start as *const u8, end - start) };
    let untouched = painted.iter().position(|&byte| byte != PAINT).unwrap_or(painted.len());
    assert!(untouched > 0, "the stack overflowed its painted range");
    here - (start + untouched)
}

/// Polls `phase` to completion and records how much stack it took as `name`'s.
async fn measure<T>(
    uses: &mut Vec<(&'static str, usize)>,
    name: &'static str,
    phase: impl Future<Output = T>,
) -> T {
    paint();
    let output = phase.await;
    uses.push((name, deepest()));
    output
}

/// The lowest address of the current thread's stack.
#[cfg(target_os = "linux")]
fn stack_low() -> usize {
    // SAFETY: Reads the current thread's attributes into storage this owns.
    unsafe {
        let mut attr = std::mem::zeroed();
        assert_eq!(libc::pthread_getattr_np(libc::pthread_self(), &mut attr), 0);
        let (mut low, mut size) = (std::ptr::null_mut(), 0);
        assert_eq!(libc::pthread_attr_getstack(&attr, &mut low, &mut size), 0);
        libc::pthread_attr_destroy(&mut attr);
        low as usize
    }
}

/// The lowest address of the current thread's stack.
#[cfg(target_os = "macos")]
fn stack_low() -> usize {
    // SAFETY: Queries the current thread.
    unsafe {
        let high = libc::pthread_get_stackaddr_np(libc::pthread_self()) as usize;
        high - libc::pthread_get_stacksize_np(libc::pthread_self())
    }
}

/// Stack used, by phase.
type Uses = Vec<(&'static str, usize)>;

/// What a guest's filesystem does, phase by phase: formats, makes a volume, creates directories
/// and files, writes, looks up, renames and unlinks, compacts the journal, remounts, replays,
/// checks and reads.
async fn workload() -> Uses {
    let mut uses = Uses::new();
    let device = DeviceHolder::new(FakeDevice::new(16384, BLOCK_SIZE));
    let fs = measure(&mut uses, "format", FxFilesystem::new_empty(device)).await.unwrap();
    populate(&mut uses, &fs).await;
    measure(&mut uses, "compact the journal", fs.journal().force_compact()).await.unwrap();
    measure(&mut uses, "sync", fs.sync(SyncOptions::default())).await.unwrap();
    measure(&mut uses, "close", fs.close()).await.unwrap();

    let device = fs.take_device().await;
    device.reopen(false);
    let fs = measure(&mut uses, "open and replay", FxFilesystem::open(device)).await.unwrap();
    measure(&mut uses, "fsck", fsck(fs.clone())).await.unwrap();
    check(&mut uses, &fs).await;
    fs.close().await.unwrap();
    uses
}

/// Makes a volume, and in it `DIRS` directories of `FILES` files of a block each, half of which
/// are renamed or unlinked.
async fn populate(uses: &mut Uses, fs: &Arc<FxFilesystem>) {
    let volume = root_volume(fs.clone()).await.unwrap();
    let store =
        measure(uses, "new volume", volume.new_volume("vol", NewChildStoreOptions::default()))
            .await
            .unwrap();
    let root = Directory::open(&store, store.root_directory_object_id()).await.unwrap();
    let lock = |id| LockKey::object(store.store_object_id(), id);
    let device = fs.device();
    let mut data = device.allocate_buffer(BLOCK_SIZE as usize).await;
    data.fill(7);
    for d in 0..DIRS {
        let dir = measure(uses, "mkdir", async {
            let mut transaction = store
                .new_transaction(lock_keys![lock(root.object_id())], Options::default())
                .await
                .unwrap();
            let dir = root.create_child_dir(&mut transaction, &format!("dir{d}")).await.unwrap();
            transaction.commit().await.unwrap();
            dir
        })
        .await;
        for f in 0..FILES {
            let file = measure(uses, "create", async {
                let mut transaction = store
                    .new_transaction(lock_keys![lock(dir.object_id())], Options::default())
                    .await
                    .unwrap();
                let file =
                    dir.create_child_file(&mut transaction, &format!("file{f}")).await.unwrap();
                transaction.commit().await.unwrap();
                file
            })
            .await;
            measure(uses, "write", file.write_or_append(Some(0), data.as_ref())).await.unwrap();
        }
        for f in (0..FILES).step_by(2) {
            let name = format!("file{f}");
            let (id, _, _) = measure(uses, "lookup", dir.lookup(&name)).await.unwrap().unwrap();
            let (phase, to) = if f % 4 == 0 { ("unlink", None) } else { ("rename", Some(&dir)) };
            measure(uses, phase, async {
                let mut transaction = store
                    .new_transaction(
                        lock_keys![lock(dir.object_id()), lock(id)],
                        Options::default(),
                    )
                    .await
                    .unwrap();
                match to {
                    None => replace_child(&mut transaction, None, (&dir, &name)).await.unwrap(),
                    Some(to) => {
                        let renamed = format!("renamed{f}");
                        replace_child(&mut transaction, Some((&dir, &name)), (to, &renamed))
                            .await
                            .unwrap()
                    }
                };
                transaction.commit().await.unwrap();
            })
            .await;
        }
    }
}

/// Opens the volume and reads back a renamed file.
async fn check(uses: &mut Uses, fs: &Arc<FxFilesystem>) {
    let volume = root_volume(fs.clone()).await.unwrap();
    let store =
        measure(uses, "open volume", volume.volume("vol", Default::default())).await.unwrap();
    let root = Directory::open(&store, store.root_directory_object_id()).await.unwrap();
    let (id, _, _) = root.lookup("dir3").await.unwrap().unwrap();
    let dir = Directory::open(&store, id).await.unwrap();
    let (id, _, _) = dir.lookup("renamed2").await.unwrap().unwrap();
    let file =
        measure(uses, "open file", ObjectStore::open_object(&store, id, Default::default(), None))
            .await
            .unwrap();
    let mut buf = file.allocate_buffer(BLOCK_SIZE as usize).await;
    assert_eq!(
        measure(uses, "read", file.read_aligned(0, buf.as_mut())).await.unwrap(),
        BLOCK_SIZE as usize
    );
    assert_eq!(buf.to_vec(), vec![7; BLOCK_SIZE as usize]);
}

fn main() {
    assert!(!cfg!(debug_assertions), "measure an optimized build: run with --release");
    let uses = std::thread::Builder::new()
        .stack_size(std::env::var("FXFS_STACK_KIB").map_or(1024, |kib| kib.parse().unwrap()) * KIB)
        .spawn(|| LocalExecutor::new().run_singlethreaded(Box::pin(workload())))
        .unwrap()
        .join()
        .unwrap();
    // The most each phase used, in the order they first ran.
    let mut most: Vec<(&str, usize)> = Vec::new();
    for (name, used) in uses {
        match most.iter_mut().find(|(phase, _)| *phase == name) {
            Some((_, most)) => *most = (*most).max(used),
            None => most.push((name, used)),
        }
    }
    println!("| Phase | Stack (KiB) |\n|---|---:|");
    for (name, used) in most {
        println!("| {name} | {} |", used.div_ceil(KIB));
    }
}
