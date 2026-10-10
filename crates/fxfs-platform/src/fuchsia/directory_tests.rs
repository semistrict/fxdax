// fxdax's tests of directories for what upstream tests only in the filesystem conformance suites
// (src/storage/conformance), which aren't copied: renames and hard links across directories, and
// their persistence. They drive FxDirectory through the in-process fuchsia.io, as upstream's tests
// in directory.rs do.

use crate::fuchsia::testing::{
    TestFixture, TestFixtureOptions, close_dir_checked, close_file_checked, open_dir,
    open_dir_checked, open_file, open_file_checked,
};
use fidl_fuchsia_io as fio;
use fuchsia_fs::directory::{DirEntry, DirentKind, readdir};
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

const DIR_FLAGS: fio::Flags = fio::Flags::FLAG_MAYBE_CREATE
    .union(fio::PERM_READABLE)
    .union(fio::PERM_WRITABLE)
    .union(fio::Flags::PROTOCOL_DIRECTORY);

const FILE_FLAGS: fio::Flags = fio::Flags::FLAG_MAYBE_CREATE
    .union(fio::PERM_READABLE)
    .union(fio::PERM_WRITABLE)
    .union(fio::Flags::PROTOCOL_FILE);

async fn create_dir(parent: &fio::DirectoryProxy, name: &str) -> fio::DirectoryProxy {
    open_dir_checked(parent, name, DIR_FLAGS, Default::default()).await
}

async fn write_file(parent: &fio::DirectoryProxy, name: &str, contents: &[u8]) {
    let file = open_file_checked(parent, name, FILE_FLAGS, &Default::default()).await;
    fuchsia_fs::file::write(&file, contents).await.expect("write failed");
    close_file_checked(file).await;
}

async fn read_file(parent: &fio::DirectoryProxy, name: &str) -> Vec<u8> {
    let file = open_file_checked(
        parent,
        name,
        fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
        &Default::default(),
    )
    .await;
    let contents = fuchsia_fs::file::read(&file).await.expect("read failed");
    close_file_checked(file).await;
    contents
}

async fn token(dir: &fio::DirectoryProxy) -> zx::Event {
    let (status, token) = dir.get_token().await.expect("FIDL call failed");
    zx::Status::ok(status).expect("get_token failed");
    zx::Event::from(token.expect("no token"))
}

async fn open_status(parent: &fio::DirectoryProxy, name: &str) -> zx::Status {
    *open_file(parent, name, fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE, &Default::default())
        .await
        .expect_err("open succeeded")
        .root_cause()
        .downcast_ref::<zx::Status>()
        .expect("no status")
}

fn entry(name: &str, kind: DirentKind) -> DirEntry {
    DirEntry { name: name.to_owned(), kind }
}

async fn link_count(dir: &fio::DirectoryProxy, name: &str) -> u64 {
    let file = open_file_checked(
        dir,
        name,
        fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
        &Default::default(),
    )
    .await;
    let (_, immutable) = file
        .get_attributes(fio::NodeAttributesQuery::LINK_COUNT)
        .await
        .expect("FIDL call failed")
        .expect("get_attributes failed");
    close_file_checked(file).await;
    immutable.link_count.expect("no link count")
}

#[fuchsia::test]
async fn test_rename_file_across_directories() {
    let fixture = TestFixture::new().await;
    let root = fixture.root();
    let a = create_dir(root, "a").await;
    let b = create_dir(root, "b").await;
    write_file(&a, "file", b"contents").await;

    a.rename("file", token(&b).await, "moved")
        .await
        .expect("FIDL call failed")
        .expect("rename failed");

    assert_eq!(read_file(&b, "moved").await, b"contents");
    assert_eq!(open_status(&a, "file").await, zx::Status::NOT_FOUND);
    assert_eq!(readdir(&a).await.expect("readdir failed"), vec![]);
    assert_eq!(readdir(&b).await.expect("readdir failed"), vec![entry("moved", DirentKind::File)]);

    close_dir_checked(a).await;
    close_dir_checked(b).await;
    fixture.close().await;
}

#[fuchsia::test]
async fn test_rename_file_over_file_across_directories() {
    let fixture = TestFixture::new().await;
    let root = fixture.root();
    let a = create_dir(root, "a").await;
    let b = create_dir(root, "b").await;
    write_file(&a, "file", b"new").await;
    write_file(&b, "file", b"old contents").await;

    a.rename("file", token(&b).await, "file")
        .await
        .expect("FIDL call failed")
        .expect("rename failed");

    assert_eq!(read_file(&b, "file").await, b"new");
    assert_eq!(readdir(&a).await.expect("readdir failed"), vec![]);

    close_dir_checked(a).await;
    close_dir_checked(b).await;
    fixture.close().await;
}

#[fuchsia::test]
async fn test_rename_directory_across_directories() {
    let fixture = TestFixture::new().await;
    let root = fixture.root();
    let a = create_dir(root, "a").await;
    let b = create_dir(root, "b").await;
    let sub = create_dir(&a, "sub").await;
    write_file(&sub, "file", b"contents").await;
    close_dir_checked(sub).await;

    a.rename("sub", token(&b).await, "moved")
        .await
        .expect("FIDL call failed")
        .expect("rename failed");

    let moved = open_dir_checked(
        &b,
        "moved",
        fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
        Default::default(),
    )
    .await;
    assert_eq!(read_file(&moved, "file").await, b"contents");
    assert_eq!(
        *open_dir(&a, "sub", fio::PERM_READABLE, &Default::default())
            .await
            .expect_err("open succeeded")
            .root_cause()
            .downcast_ref::<zx::Status>()
            .expect("no status"),
        zx::Status::NOT_FOUND
    );
    assert_eq!(
        readdir(&b).await.expect("readdir failed"),
        vec![entry("moved", DirentKind::Directory)]
    );

    close_dir_checked(moved).await;
    close_dir_checked(a).await;
    close_dir_checked(b).await;
    fixture.close().await;
}

#[fuchsia::test]
async fn test_rename_directory_into_itself_fails() {
    let fixture = TestFixture::new().await;
    let root = fixture.root();
    let a = create_dir(root, "a").await;
    let sub = create_dir(&a, "sub").await;

    assert_eq!(
        root.rename("a", token(&sub).await, "a").await.expect("FIDL call failed"),
        Err(zx::Status::INVALID_ARGS.into_raw())
    );

    close_dir_checked(sub).await;
    close_dir_checked(a).await;
    fixture.close().await;
}

#[fuchsia::test]
async fn test_link_file_across_directories() {
    let fixture = TestFixture::new().await;
    let root = fixture.root();
    let a = create_dir(root, "a").await;
    let b = create_dir(root, "b").await;
    write_file(&a, "file", b"contents").await;

    let status = a.link("file", token(&b).await.into(), "link").await.expect("FIDL call failed");
    zx::Status::ok(status).expect("link failed");

    assert_eq!(read_file(&b, "link").await, b"contents");
    assert_eq!(link_count(&a, "file").await, 2);

    a.unlink("file", &fio::UnlinkOptions::default())
        .await
        .expect("FIDL call failed")
        .expect("unlink failed");
    assert_eq!(read_file(&b, "link").await, b"contents");
    assert_eq!(link_count(&b, "link").await, 1);

    close_dir_checked(a).await;
    close_dir_checked(b).await;
    fixture.close().await;
}

#[fuchsia::test]
async fn test_rename_and_link_persist() {
    let mut device = DeviceHolder::new(FakeDevice::new(16384, 512));
    {
        let fixture =
            TestFixture::open(device, TestFixtureOptions { format: true, ..Default::default() })
                .await;
        let root = fixture.root();
        let a = create_dir(root, "a").await;
        let b = create_dir(root, "b").await;
        write_file(&a, "file", b"contents").await;
        a.rename("file", token(&b).await, "moved")
            .await
            .expect("FIDL call failed")
            .expect("rename failed");
        let status =
            b.link("moved", token(&a).await.into(), "link").await.expect("FIDL call failed");
        zx::Status::ok(status).expect("link failed");
        root.create_symlink("symlink", b"a/link", None)
            .await
            .expect("FIDL call failed")
            .expect("create_symlink failed");
        close_dir_checked(a).await;
        close_dir_checked(b).await;
        device = fixture.close().await;
    }

    let fixture =
        TestFixture::open(device, TestFixtureOptions { format: false, ..Default::default() }).await;
    let root = fixture.root();
    let a = open_dir_checked(
        root,
        "a",
        fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
        Default::default(),
    )
    .await;
    let b = open_dir_checked(
        root,
        "b",
        fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
        Default::default(),
    )
    .await;
    assert_eq!(read_file(&b, "moved").await, b"contents");
    assert_eq!(read_file(&a, "link").await, b"contents");
    assert_eq!(link_count(&a, "link").await, 2);
    assert_eq!(
        readdir(root).await.expect("readdir failed"),
        vec![
            entry("a", DirentKind::Directory),
            entry("b", DirentKind::Directory),
            entry("symlink", DirentKind::Symlink),
        ]
    );
    let (symlink, server_end) = fidl::endpoints::create_proxy::<fio::SymlinkMarker>();
    root.open("symlink", fio::PERM_READABLE, &Default::default(), server_end.into_channel())
        .expect("open failed");
    assert_eq!(
        symlink.describe().await.expect("FIDL call failed").target.expect("no target"),
        b"a/link"
    );

    close_dir_checked(a).await;
    close_dir_checked(b).await;
    fixture.close().await;
}
