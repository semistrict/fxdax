// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// AWAITING DECISION (TASK-96.1): the unit tests of crates/fxdax/src/filesystem.rs as they were over
// fxfs-platform, unchanged, kept here, where cargo builds nothing (outside `tests`), until the
// user decides their fate. They test fxfs-platform's internals that host Fxfs
// no longer has: the volume's `ExecutionScope` and its guards, Fxfs's pager and its page requests,
// and files' streams. Those whose behaviour survives are ported in crates/fxdax/src/filesystem.rs:
// handles_hold_nodes_until_the_last_drops, dropping_an_unmounted_filesystem_on_the_executor_...,
// dropping_a_filesystem_mid_unmount_..., unmounting_waits_for_an_operation_under_way and
// unmounting_waits_for_writes_under_way (over Fxfs's locks rather than the scope and the pager),
// and unmounting_completes_once_a_write_under_way_is_dropped (as
// a_write_given_up_on_completes_before_the_unmount: a write given up on now completes).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::page_size;
    use fuchsia_async::ScopeHandle;
    use fuchsia_async_kernel::{EHandle, PacketReceiver};
    use futures::FutureExt as _;
    use futures::channel::oneshot;
    use futures::executor::block_on;
    use futures::task::ArcWake;
    use fxfs::object_store::transaction::{LockKey, lock_keys};
    use fxfs_platform::volume::{FxVolume, READ_AHEAD_SIZE};
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::task::{Context, Poll};

    const OWNER: Owner = Owner { uid: 1000, gid: 100 };

    /// A node is held while a handle to it lives: one entry, however many handles, which goes
    /// with the last of them. Clones are the same handle.
    #[test]
    fn handles_hold_nodes_until_the_last_drops() {
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let root = fs.root().inode().0;
            let held = || fs.volume().held();
            assert_eq!(held(), BTreeMap::from([(root, 1)]));

            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, b"contents").await, Ok(8));
            let id = file.inode().0;
            let clone = file.clone();
            assert_eq!(held(), BTreeMap::from([(root, 1), (id, 1)]));
            let (again, _) = fs.root().lookup(b"file").await.unwrap();
            assert_eq!(again.inode().0, id);
            assert_eq!(held(), BTreeMap::from([(root, 1), (id, 2)]));

            fs.root().unlink(b"file").await.unwrap();
            drop(file);
            assert_eq!(held(), BTreeMap::from([(root, 1), (id, 2)]));
            drop(clone);
            assert_eq!(held(), BTreeMap::from([(root, 1), (id, 1)]));
            let mut buf = [0; 16];
            assert_eq!(again.read(0, &mut buf).await, Ok(8));
            assert_eq!(&buf[..8], b"contents");
            drop(again);
            assert_eq!(held(), BTreeMap::from([(root, 1)]));
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// An unmounted filesystem dropped on Fxfs's executor neither blocks it nor panics: only a
    /// mounted one's drop unmounts it.
    #[test]
    fn dropping_an_unmounted_filesystem_on_the_executor_neither_blocks_nor_panics() {
        let fs = block_on(Filesystem::format(Storage::memory(8 << 20))).unwrap();
        block_on(fs.unmount()).unwrap();
        let image = fs.volume().image.clone();
        let dropped = image.executor.run_blocking(async move {
            drop(fs);
            true
        });
        assert!(dropped);
    }

    /// A filesystem of an image of its own, dropped while an unmount given up on is under way,
    /// closes its image once that unmount is done, not under it: the volume is unmounted when the
    /// drop returns, and the image is whole, with what was written.
    #[test]
    fn dropping_a_filesystem_mid_unmount_closes_its_image_after_the_unmount() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("image");
            let fs = Filesystem::format(Storage::create(&path, 64 << 20).unwrap()).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, b"written").await, Ok(7));
            drop(file);
            // Polled once, the unmount begins the volume's, and is given up on while it waits.
            assert!(fs.unmount().now_or_never().is_none(), "the volume unmounted at once");
            let unmounted = fs.volume.image.unmount_volume(&fs.volume);
            assert!(!unmounted.began());
            drop(fs);
            assert!(unmounted.wait().now_or_never().is_some(), "the image closed mid-unmount");

            let fs = Filesystem::mount(Storage::open(&path).unwrap().check().await.unwrap())
                .await
                .unwrap();
            let (file, attr) = fs.root().lookup(b"file").await.unwrap();
            let mut contents = vec![0; attr.size as usize];
            assert_eq!(file.read(0, &mut contents).await, Ok(7));
            assert_eq!(contents, b"written");
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Holds the lock of Fxfs's that a transaction changing the node `object_id` of `volume` takes,
    /// until the sender returned is sent on or dropped.
    async fn hold_lock(volume: &Volume, object_id: u64) -> oneshot::Sender<()> {
        let store = volume.fx_volume().unwrap().store().clone();
        let (locked, held) = oneshot::channel();
        let (release, released) = oneshot::channel::<()>();
        volume.image.executor.spawn(async move {
            let keys = lock_keys![LockKey::object(store.store_object_id(), object_id)];
            let transaction = store.new_transaction(keys, Default::default()).await.unwrap();
            let _ = locked.send(());
            let _ = released.await;
            drop(transaction);
        });
        held.await.unwrap();
        release
    }

    /// A handle to the scope of `volume`'s Fxfs volume, which has been given a task that ends only
    /// once the scope's shutdown wakes it: the handle's `on_no_tasks` completes once unmounting has
    /// shut the scope down, and its other tasks have ended. Fxfs's `ExecutionScope` has no
    /// `on_no_tasks`, as it waits for its tasks and guards both, and gives the handle only through
    /// a guard.
    async fn watch_shutdown(volume: &Volume) -> ScopeHandle {
        let scope = volume.fx_volume().unwrap().scope().try_active_guard().unwrap().to_handle();
        let (spawned, polled) = oneshot::channel();
        let mut spawned = Some(spawned);
        let task = std::future::poll_fn(move |_| match spawned.take() {
            Some(spawned) => {
                let _ = spawned.send(());
                Poll::Pending
            }
            None => Poll::Ready(()),
        });
        let spawner = scope.clone();
        volume.image.executor.run(async move { drop(spawner.spawn(task)) }).await;
        polled.await.unwrap();
        scope
    }

    /// Leaves `file`, of `volume`, dirty but with no connection, whose close would write it back:
    /// closes the file's connection, which writes the file back, then writes `data` at its start
    /// through the stream the connection gave, as a write under way as it closed would. Returns
    /// the stream, as such a write has it still.
    async fn dirty_unconnected(volume: &Volume, file: &Node, data: &[u8]) -> Arc<zx::Stream> {
        let connection = volume.with_held(&file.0, |held| held.connection.take()).unwrap();
        let (proxy, stream) = connection.as_ref().unwrap().file().unwrap();
        let (proxy, stream) = (Clone::clone(proxy), stream.clone());
        volume.image.executor.run(async move { proxy.close().await }).await.unwrap().unwrap();
        assert_eq!(write_stream(&stream, 0, data).await, Ok(data.len()));
        stream
    }

    /// Writes `data` at `offset` through `stream`, a file's, as the file's writes do.
    async fn write_stream(
        stream: &zx::Stream,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, zx::Status> {
        let bufs = [VolatileSlice::from(data).host_buffer()];
        zircon::syscalls::stream_writev_at_async(stream.raw_handle(), offset, &bufs).await
    }

    /// Holds Fxfs's pager off from answering page requests until the sender returned is sent on or
    /// dropped. The requests' packets arrive on the process's port, whose packets one thread hands
    /// to their receivers (fuchsia-async-kernel's `packets`), so a receiver of the test's own that
    /// waits on that thread keeps the pager's packets waiting too. Fxfs's executor registers it, as
    /// a receiver is registered through the handle of a runtime, which the test's thread lacks.
    async fn hold_pager(volume: &Volume) -> oneshot::Sender<()> {
        /// Receives one packet, and waits on the receiving thread until it is released.
        struct Waiting(std::sync::Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>);
        impl PacketReceiver for Waiting {
            fn receive_packet(&self, _: zx::Packet) {
                let (waiting, released) = self.0.lock().unwrap().take().expect("a second packet");
                waiting.send(()).unwrap();
                let _ = block_on(released);
            }
        }
        let (waiting, held) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let receiver = Waiting(std::sync::Mutex::new(Some((waiting, released))));
        let registered = async move { EHandle::local().register_receiver(receiver) };
        let registration = volume.image.executor.run(registered).await;
        let user = zx::UserPacket::from_u8_array([0; 32]);
        let packet = zx::Packet::from_user_packet(registration.key(), 0, user);
        registration.port().queue(&packet).unwrap();
        held.await.unwrap();
        release
    }

    /// Polls `future`, a read's or a write's, of a file of `volume`, once, which leaves it waiting
    /// on a page request, and waits until it is woken: until Fxfs's pager has answered the
    /// request, supplying the pages it asked for, or letting it dirty them. It has copied nothing
    /// to or from them yet. The pager is held off for the poll (see [`hold_pager`]), as an answer
    /// that comes before the future begins to wait for it, later in the poll that made the
    /// request, lets the future copy in that poll.
    async fn woken(volume: &Volume, future: &mut (impl Future + Unpin)) {
        struct Woken(std::sync::Mutex<Option<oneshot::Sender<()>>>);
        impl ArcWake for Woken {
            fn wake_by_ref(woken: &Arc<Self>) {
                if let Some(sender) = woken.0.lock().unwrap().take() {
                    let _ = sender.send(());
                }
            }
        }
        let (sender, receiver) = oneshot::channel();
        let waker = futures::task::waker(Arc::new(Woken(std::sync::Mutex::new(Some(sender)))));
        let release = hold_pager(volume).await;
        let polled = future.poll_unpin(&mut Context::from_waker(&waker));
        assert!(polled.is_pending(), "it needed no page request");
        release.send(()).unwrap();
        receiver.await.unwrap();
    }

    /// Whether the scope of `volume`, Fxfs's, has finished: shut down with no guard left, which
    /// unmounting waits for to write the volume back. Its `ExecutionScope` gives a guard under the
    /// lock it shuts the scope down under, so a shutdown under way is done once this asks: a scope
    /// that the shutdown found guarded, and has a guard still, has not finished.
    fn scope_finished(volume: &FxVolume) -> bool {
        volume.scope().try_active_guard().is_none()
    }

    /// Unmounting waits for an operation under way, as for the volume's fuchsia.io requests: the
    /// unmount shuts the scope of Fxfs's volume down, then waits for the scope to finish, with its
    /// guards gone, before it writes the volume back, and the operation holds one. An rmdir held
    /// by the lock of Fxfs's that it takes keeps the scope, shut down and with no tasks left, from
    /// finishing; let go, it removes the directory, and the unmount completes. Meanwhile, the
    /// image's volumes directory is free, as work under a guard may need it: dirtying a page under
    /// critical memory pressure does (`VolumesDirectory::report_pager_dirty`).
    #[test]
    fn unmounting_waits_for_an_operation_under_way() {
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            fs.root().mkdir(b"dir", 0o755, OWNER).await.unwrap();
            let volume = fs.volume().clone();
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let release = hold_lock(&volume, fs.root().inode().0).await;
            let mut rmdir = Box::pin(fs.root().rmdir(b"dir"));
            assert!(futures::poll!(&mut rmdir).is_pending());
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with an rmdir under way");
            // So the unmount waits for it still, with the volumes directory free.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under an rmdir");
            let volumes_directory = volume.image.volumes_directory().unwrap();
            assert!(
                volumes_directory.lock().now_or_never().is_some(),
                "the volumes directory is locked while the unmount waits for the scope"
            );

            release.send(()).unwrap();
            assert_eq!(rmdir.await, Ok(()));
            unmounted.await.expect("unmounting failed");
            drop((scope, fx_volume, volumes_directory, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            assert_eq!(fs.root().lookup(b"dir").await.err(), Some(Errno::ENOENT));
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// An operation that found the volume mounted, but finds its scope finished as it runs its
    /// work, or as it begins to copy, a read or write, as it does if the whole unmount comes
    /// between the two, fails with `ENOTCONN`, as one does once unmounting begins, and changes
    /// nothing. The test shuts the scope down itself, as the unmount does, leaving the volume
    /// mounted. The file's page is dirty, so that a write to it, as a read of it, needs no pager.
    #[test]
    fn operations_refuse_a_volume_whose_scope_is_shut_down() {
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            fs.root().mkdir(b"dir", 0o755, OWNER).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, b"written").await, Ok(7));
            // As the unmount shuts it down.
            let scope = fs.volume().fx_volume().unwrap().scope().clone();
            scope.shutdown();
            scope.wait().await;
            assert_eq!(fs.root().rmdir(b"dir").await, Err(Errno::ENOTCONN));
            assert_eq!(file.write(0, b"changed").await, Err(Errno::ENOTCONN));
            let mut contents = [0; 7];
            assert_eq!(file.read(0, &mut contents).await, Err(Errno::ENOTCONN));

            drop((scope, file));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            assert_eq!(fs.root().lookup(b"dir").await.unwrap().1.file_type, FileType::Directory);
            let (file, _) = fs.root().lookup(b"file").await.unwrap();
            assert_eq!(file.read(0, &mut contents).await, Ok(7));
            assert_eq!(&contents, b"written");
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Unmounting waits for writes under way, which hold their file open under a guard on the
    /// volume's scope while they copy (see [`Node::write_vectored`]), and writes back what they
    /// wrote: an overwrite of a file's page written back already, and a write extending another
    /// file at its end. Fxfs's pager let each dirty its page before the unmount began, and each
    /// copies to it only once the unmount is under way. The extending write was lost otherwise: it
    /// extends its file over its page only once it has copied, and the volume's last write-back
    /// leaves a dirty page past the file's end for a later one, which never comes. The writes are
    /// to files of their own, as the file's stream holds off a write extending it until those
    /// before it complete.
    #[test]
    fn unmounting_waits_for_writes_under_way() {
        let page = page_size() as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (overwritten, _) = fs.root().create(b"overwritten", 0o644, OWNER).await.unwrap();
            let (extended, _) = fs.root().create(b"extended", 0o644, OWNER).await.unwrap();
            for file in [&overwritten, &extended] {
                assert_eq!(file.write(0, &vec![1; page]).await, Ok(page));
                file.fsync().await.unwrap();
            }
            let volume = fs.volume().clone();
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let data = vec![2; page];
            let mut overwrite = Box::pin(overwritten.write(0, &data));
            woken(&volume, &mut overwrite).await;
            let mut extend = Box::pin(extended.write(page as u64, &data));
            woken(&volume, &mut extend).await;
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with writes under way");
            // So the unmount waits for them still.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under writes");

            assert_eq!(overwrite.await, Ok(page));
            assert_eq!(extend.await, Ok(page));
            unmounted.await.expect("unmounting failed");
            drop((overwritten, extended, scope, fx_volume, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let written = [
                (&b"overwritten"[..], data.clone()),
                (b"extended", [vec![1; page], data].concat()),
            ];
            for (name, written) in written {
                let (file, attr) = fs.root().lookup(name).await.unwrap();
                assert_eq!(attr.size, written.len() as u64);
                let mut contents = vec![0; written.len()];
                assert_eq!(file.read(0, &mut contents).await, Ok(written.len()));
                assert_eq!(contents, written);
            }
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// A write under way holds the unmount off only until it is dropped, which is what lets the
    /// unmount complete once the write's caller has stopped polling it (see
    /// [`Filesystem::unmount`]): the file it holds open, and its guard, go with it, and the
    /// unmount completes, leaving the file as it was, as the write copied nothing. Fxfs's pager
    /// let the write dirty its page, written back already, before the unmount began.
    #[test]
    fn unmounting_completes_once_a_write_under_way_is_dropped() {
        let page = page_size() as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, &vec![1; page]).await, Ok(page));
            file.fsync().await.unwrap();
            let volume = fs.volume().clone();
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let data = vec![2; page];
            let mut write = Box::pin(file.write(0, &data));
            woken(&volume, &mut write).await;
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with a write under way");
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under a write");

            drop(write);
            unmounted.await.expect("unmounting failed");
            drop((file, scope, fx_volume, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let (file, attr) = fs.root().lookup(b"file").await.unwrap();
            assert_eq!(attr.size, page as u64);
            let mut contents = vec![0; page];
            assert_eq!(file.read(0, &mut contents).await, Ok(page));
            assert_eq!(contents, vec![1; page]);
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// A write through a file's stream that holds no guard on the volume's scope, unlike the
    /// filesystem's own (see [`Node::write_vectored`]), and copies once the scope has finished,
    /// before the volume's last write-back reaches its file, is written back if its page is dirty
    /// already, and fails otherwise: what a write copies to a dirty page it copies under the lock
    /// of the file's VMO that the write-back takes to begin cleaning it, and to dirty a page it
    /// needs Fxfs's pager, which refuses once the scope has finished. The file is dirty and has no
    /// connection, whose close as the unmount begins would write it back, and the writes go
    /// through the stream its connection gave; the lock of Fxfs's that writing the file back
    /// takes holds the write-back off until they are done.
    #[test]
    fn writes_copying_once_the_scope_finishes_are_written_back_or_fail() {
        let page = page_size() as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            assert_eq!(file.write(0, &vec![1; 2 * page]).await, Ok(2 * page));
            let volume = fs.volume().clone();
            let stream = dirty_unconnected(&volume, &file, &vec![2; page]).await;
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let release = hold_lock(&volume, file.inode().0).await;
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(scope_finished(&fx_volume), "the scope is held");
            // The first page is dirty, the second clean.
            let data = vec![3; page];
            assert_eq!(write_stream(&stream, 0, &data).await, Ok(page));
            assert_eq!(write_stream(&stream, page as u64, &data).await, Err(zx::Status::BAD_STATE));
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under the lock");

            release.send(()).unwrap();
            unmounted.await.expect("unmounting failed");
            drop((file, stream, scope, fx_volume, volume));
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let (file, _) = fs.root().lookup(b"file").await.unwrap();
            let mut contents = vec![0; 2 * page];
            assert_eq!(file.read(0, &mut contents).await, Ok(2 * page));
            assert_eq!(contents, [vec![3; page], vec![1; page]].concat());
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Unmounting waits for a read under way, which holds its file open under a guard on the
    /// volume's scope while it copies (see [`Node::read_vectored`]), and Fxfs's pager supplies
    /// the pages it asks for meanwhile: the file is open though the unmount has closed it, and the
    /// scope's shutdown waits for the guard. Fxfs's pager reads in the whole window of
    /// `READ_AHEAD_SIZE` that a page it is asked for is in; the file's second window is in memory,
    /// its first and third not, so the read asks for the first before the unmount begins, and,
    /// once it has copied the second, for the third, as it alone holds the file open.
    #[test]
    fn unmounting_waits_for_a_read_under_way() {
        const WINDOW: usize = READ_AHEAD_SIZE as usize;
        let page = page_size() as usize;
        block_on(async {
            let fs = Filesystem::format(Storage::memory(8 << 20)).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            let data: Vec<u8> = (0..3 * WINDOW).map(|at| (at / page) as u8).collect();
            assert_eq!(file.write(0, &data).await, Ok(data.len()));
            drop(file);
            let fs = Filesystem::mount(fs.close().await.unwrap()).await.unwrap();
            let (file, _) = fs.root().lookup(b"file").await.unwrap();
            let mut byte = [0];
            assert_eq!(file.read(WINDOW as u64, &mut byte).await, Ok(1));
            let volume = fs.volume().clone();
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let mut contents = vec![0; data.len()];
            let mut read = Box::pin(file.read(0, &mut contents));
            woken(&volume, &mut read).await;
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with a read under way");
            // So the unmount waits for it still.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under a read");

            assert_eq!(read.await, Ok(data.len()));
            assert_eq!(contents, data);
            unmounted.await.expect("unmounting failed");
            drop((file, scope, fx_volume, volume));
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Unmounting waits for a clone of the image writing the volume back, as for an operation:
    /// the clone writes each volume mounted as it begins back under a guard on its scope. One held
    /// by the lock of Fxfs's that writing back a dirty file takes keeps the scope from finishing;
    /// let go, it clones the image, file and all, and the unmount completes. The file has no
    /// connection, whose close as the unmount begins would wait for the lock too.
    #[test]
    fn unmounting_waits_for_a_clone_writing_the_volume_back() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let storage = Storage::create(dir.path().join("image"), 64 << 20).unwrap();
            let fs = Filesystem::format(storage).await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            let volume = fs.volume().clone();
            dirty_unconnected(&volume, &file, b"written").await;
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let release = hold_lock(&volume, file.inode().0).await;
            let mut clone = Box::pin(fs.clone_storage_to(dir.path().join("clone")));
            assert!(futures::poll!(&mut clone).is_pending());
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(!scope_finished(&fx_volume), "the scope finished with a clone writing it back");
            // So the unmount waits for it still.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under a clone");

            release.send(()).unwrap();
            let clone = clone.await.expect("cloning failed");
            unmounted.await.expect("unmounting failed");
            drop((file, scope, fx_volume, volume));
            fs.close().await.unwrap().check().await.unwrap();
            let fs = Filesystem::mount(clone.check().await.unwrap()).await.unwrap();
            let (file, attr) = fs.root().lookup(b"file").await.unwrap();
            let mut contents = vec![0; attr.size as usize];
            assert_eq!(file.read(0, &mut contents).await, Ok(7));
            assert_eq!(contents, b"written");
            drop(file);
            fs.close().await.unwrap().check().await.unwrap();
        })
    }

    /// Unmounting waits for a snapshot of the volume writing it back, as for an operation: the
    /// snapshot writes its source back, if it is mounted as the snapshot begins, under a guard on
    /// its scope. One held by the lock of Fxfs's that writing back a dirty file takes keeps the
    /// scope from finishing; let go, it snapshots the volume, file and all, once the unmount, which
    /// the snapshot waits for, completes. The file has no connection, whose close as the unmount
    /// begins would wait for the lock too.
    #[test]
    fn unmounting_waits_for_a_snapshot_writing_its_source_back() {
        block_on(async {
            let image = Image::format(Storage::memory(64 << 20)).await.unwrap();
            let fs = image.create_volume("source").await.unwrap();
            let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
            let volume = fs.volume().clone();
            dirty_unconnected(&volume, &file, b"written").await;
            let fx_volume = volume.fx_volume().unwrap();
            let scope = watch_shutdown(&volume).await;

            let release = hold_lock(&volume, file.inode().0).await;
            let mut snapshot = Box::pin(image.snapshot_volume("source", "snapshot"));
            assert!(futures::poll!(&mut snapshot).is_pending());
            let mut unmounted = Box::pin(volume.image.unmount_volume(&volume).wait());
            scope.on_no_tasks().await;
            assert!(
                !scope_finished(&fx_volume),
                "the scope finished with a snapshot writing it back"
            );
            // So the unmount waits for it still.
            assert!(futures::poll!(&mut unmounted).is_pending(), "unmounted under a snapshot");

            release.send(()).unwrap();
            unmounted.await.expect("unmounting failed");
            snapshot.await.expect("snapshotting failed");
            drop((file, scope, fx_volume, volume, fs));
            let fs = image.open_volume("snapshot").await.unwrap();
            let (file, attr) = fs.root().lookup(b"file").await.unwrap();
            let mut contents = vec![0; attr.size as usize];
            assert_eq!(file.read(0, &mut contents).await, Ok(7));
            assert_eq!(contents, b"written");
            drop(file);
            fs.unmount().await.unwrap();
            image.close().await.unwrap().check().await.unwrap();
        })
    }
}
