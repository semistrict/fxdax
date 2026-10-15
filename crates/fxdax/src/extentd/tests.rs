use super::client::RemoteExtentStore;
use super::server::{serve, serve_connection};
use crate::kernel::page_size;
use crate::{Errno, ExtentId, ExtentStore, VolatileSlice, VolatileSliceMut};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::ptr::NonNull;
use std::thread::JoinHandle;

const BLOCK: usize = ExtentStore::BLOCK_SIZE as usize;

/// A daemon serving one connection, on a thread, and its client, of the store at `path`.
fn connect(path: &Path, capacity: u64) -> (RemoteExtentStore, JoinHandle<()>) {
    let (client, server) = UnixStream::pair().unwrap();
    let served = std::thread::spawn(move || serve_connection(server));
    (RemoteExtentStore::over(client, path, capacity).unwrap(), served)
}

/// `len` bytes of `byte` plus their offset modulo 251.
fn pattern(byte: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| byte ^ (i % 251) as u8).collect()
}

fn write(
    store: &RemoteExtentStore,
    extent: ExtentId,
    offset: u64,
    data: &[u8],
) -> Result<(), Errno> {
    // In two buffers, as a request's descriptors may hold it.
    let (first, second) = data.split_at(data.len() / 2);
    store.write(extent, offset, &[VolatileSlice::from(first), VolatileSlice::from(second)])
}

fn read(
    store: &RemoteExtentStore,
    extent: ExtentId,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, Errno> {
    let mut data = vec![0; len];
    let (first, second) = data.split_at_mut(len / 2);
    store.read(
        extent,
        offset,
        &mut [VolatileSliceMut::from(first), VolatileSliceMut::from(second)],
    )?;
    Ok(data)
}

/// `pages` pages of anonymous memory, for a window.
fn memory(pages: usize) -> NonNull<u8> {
    // SAFETY: Maps anonymous memory, which the test owns.
    let memory = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            pages * page_size() as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(memory, libc::MAP_FAILED);
    NonNull::new(memory.cast()).unwrap()
}

/// The file and offset this process maps the page at `addr` from, as /proc/self/maps has them:
/// the device and inode, and the offset.
fn mapped_from(addr: usize) -> (String, u64) {
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    for line in maps.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (start, end) = fields[0].split_once('-').unwrap();
        let (start, end) =
            (usize::from_str_radix(start, 16).unwrap(), usize::from_str_radix(end, 16).unwrap());
        if (start..end).contains(&addr) {
            let offset = u64::from_str_radix(fields[2], 16).unwrap();
            return (format!("{} {}", fields[3], fields[4]), offset + (addr - start) as u64);
        }
    }
    panic!("{addr:#x} isn't mapped");
}

/// How /proc/self/maps names anonymous memory's file: none.
const ANONYMOUS: &str = "00:00 0";

/// A store served by the daemon is the store: what it writes it reads, commits, and keeps once
/// the VM process is gone; it fails as the store does; and no other VM process opens it meanwhile.
#[test]
fn serves_a_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let (store, served) = connect(&path, 1 << 20);
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    write(&store, extent, 0, &pattern(1, 2 * BLOCK)).unwrap();
    assert_eq!(read(&store, extent, 0, 2 * BLOCK), Ok(pattern(1, 2 * BLOCK)));
    store.commit(Some(b"root")).unwrap();
    assert_eq!(store.root(), Ok(b"root".to_vec()));
    let info = store.info().unwrap();
    assert_eq!((info.capacity, info.used, info.next_sequence), (1 << 20, 2 * BLOCK as u64, 2));
    assert_eq!(write(&store, extent, 2 * BLOCK as u64, &[0; BLOCK]), Err(Errno::EPERM));
    let (other, other_server) = UnixStream::pair().unwrap();
    let other_served = std::thread::spawn(move || serve_connection(other_server));
    let error = RemoteExtentStore::over(other, &path, 1 << 20).err().expect("opened it twice");
    assert!(format!("{error:#}").contains(&Errno::EBUSY.to_string()), "{error:#}");
    other_served.join().unwrap();

    drop(store);
    served.join().unwrap();
    let mut data = vec![0; 2 * BLOCK];
    let local = ExtentStore::open(&path).unwrap();
    local.read(extent, 0, &mut [VolatileSliceMut::from(&mut data[..])]).unwrap();
    assert_eq!(data, pattern(1, 2 * BLOCK));
}

/// The window is the daemon's, in this process's memory: a store to a writable mapping of a new
/// extent faults, which the daemon resolves, and the data is the extent's; sealed, the mapping is
/// read-only, of the sealed extent's pages, which another mapping of it maps too.
#[test]
fn maps_a_window_in_the_vm_process() {
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    let (store, served) = connect(&dir.path().join("store"), 1 << 24);
    let memory = memory(4);
    // SAFETY: The memory is the test's, and outlives the store.
    unsafe { store.create_dax_window_in_host_memory(memory, 4 * ps as u64, None) }.unwrap();
    let base = memory.as_ptr() as usize;
    let extent = ExtentId { branch: store.branch(), sequence: 1 };
    store.map(extent, 0, ps as u64, 0, true).unwrap();
    // SAFETY: The window's memory, whose faults the store serves.
    unsafe { std::ptr::copy_nonoverlapping(pattern(3, ps).as_ptr(), memory.as_ptr(), ps) };
    assert_eq!(read(&store, extent, 0, BLOCK), Ok(pattern(3, BLOCK)));

    store.commit(None).unwrap();
    store.map(extent, 0, ps as u64, 2 * ps as u64, false).unwrap();
    // SAFETY: As above.
    let window = unsafe { std::slice::from_raw_parts(memory.as_ptr(), 3 * ps) };
    assert!(window[..ps] == pattern(3, ps)[..], "the sealed extent holds other data");
    assert!(window[2 * ps..] == pattern(3, ps)[..], "its other mapping maps other data");
    assert_eq!(mapped_from(base + 2 * ps), mapped_from(base));
    assert_ne!(mapped_from(base).0, ANONYMOUS, "the window maps no file");
    // Unmapped, the page is gone from this process's memory once the daemon replies.
    store.unmap(2 * ps as u64, ps as u64).unwrap();
    assert_eq!(mapped_from(base + 2 * ps).0, ANONYMOUS);

    drop(store);
    served.join().unwrap();
    // SAFETY: Unmaps the memory, which nothing uses any more.
    assert_eq!(unsafe { libc::munmap(memory.as_ptr().cast(), 4 * ps) }, 0);
}

/// The environment a child process of [`two_processes_map_the_same_pages`] is told what to map in.
const CHILD_SOCKET: &str = "FXDAX_EXTENTD_TEST_SOCKET";
const CHILD_STORE: &str = "FXDAX_EXTENTD_TEST_STORE";
const CHILD_EXTENT: &str = "FXDAX_EXTENTD_TEST_EXTENT";

/// Maps `extent`'s first page into a new window of the store at `store` the daemon at `socket`
/// serves, checks it holds the pattern written, and returns the store, mapping it still, and where
/// the page is mapped from.
fn map_first_page(
    socket: &Path,
    store: &Path,
    extent: ExtentId,
) -> (RemoteExtentStore, (String, u64)) {
    let ps = page_size() as usize;
    let store = RemoteExtentStore::connect(socket, store, 1 << 24).unwrap();
    let memory = memory(1);
    // SAFETY: The memory is this function's, and outlives the store.
    unsafe { store.create_dax_window_in_host_memory(memory, ps as u64, None) }.unwrap();
    store.map(extent, 0, ps as u64, 0, false).unwrap();
    // SAFETY: The window's memory, whose faults the store serves.
    let page = unsafe { std::slice::from_raw_parts(memory.as_ptr(), ps) };
    assert!(*page == pattern(9, ps)[..], "the window maps other data");
    // The memory is leaked: the store's window maps it until the process exits.
    (store, mapped_from(memory.as_ptr() as usize))
}

/// A VM process, for [`two_processes_map_the_same_pages`]: prints where it maps the extent from.
#[test]
#[ignore = "run by two_processes_map_the_same_pages"]
fn child_maps_an_extent() {
    let (Ok(socket), Ok(store), Ok(extent)) =
        (std::env::var(CHILD_SOCKET), std::env::var(CHILD_STORE), std::env::var(CHILD_EXTENT))
    else {
        return;
    };
    let (branch, sequence) = extent.split_once(':').unwrap();
    let extent = ExtentId { branch: branch.parse().unwrap(), sequence: sequence.parse().unwrap() };
    let (_store, (file, offset)) = map_first_page(Path::new(&socket), Path::new(&store), extent);
    println!("MAPPED {file} {offset}");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    // As a VMM that dies does: with its store and window still open.
    std::process::exit(0);
}

/// Two VM processes, each of a store forked from one, mapping one extent of it at once map the
/// same pages: the daemon's, which cache it once.
#[test]
fn two_processes_map_the_same_pages() {
    let ps = page_size() as usize;
    let dir = tempfile::tempdir().unwrap();
    let (parent, child) = (dir.path().join("parent"), dir.path().join("child"));
    let extent = {
        let store = ExtentStore::create(&parent, 1 << 24).unwrap();
        let extent = ExtentId { branch: store.branch(), sequence: 1 };
        store.write(extent, 0, &[VolatileSlice::from(&pattern(9, ps)[..])]).unwrap();
        store.commit(None).unwrap();
        extent
    };
    extent_store::copy_store(&parent, &child, extent.branch + 1, |file, to| {
        std::io::copy(&mut &*file, &mut std::fs::File::create_new(to)?).map(|_| ())
    })
    .unwrap();
    let socket = dir.path().join("extentd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || serve(listener));

    let (_store, ours) = map_first_page(&socket, &parent, extent);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "extentd::tests::child_maps_an_extent", "--ignored", "--nocapture"])
        .env(CHILD_SOCKET, &socket)
        .env(CHILD_STORE, &child)
        .env(CHILD_EXTENT, format!("{}:{}", extent.branch, extent.sequence))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    let theirs = stdout
        .lines()
        .find_map(|line| line.strip_prefix("MAPPED "))
        .unwrap_or_else(|| panic!("the child mapped nothing: {stdout}"));
    let (file, offset) = theirs.rsplit_once(' ').unwrap();
    assert_eq!((file.to_string(), offset.parse::<u64>().unwrap()), ours);

    // The child's store goes with it, for another VM process to open, once the daemon has seen it
    // go.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match RemoteExtentStore::connect(&socket, &child, 1 << 24) {
            Ok(_) => break,
            Err(error) if std::time::Instant::now() < deadline => {
                assert!(format!("{error:#}").contains(&Errno::EBUSY.to_string()), "{error:#}");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("the child's store is still open: {error:#}"),
        }
    }
}
