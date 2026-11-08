// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Every place the guide's content points into the code: a file and an exact snippet of it,
// unique in that file (tools/check.ts fails the build otherwise). Content names anchors by id,
// never by line: lines drift with every edit, a snippet only when its code changes.

export interface Anchor {
  path: string;
  snippet: string;
  /** What the reader finds there, in a line. */
  note: string;
}

export const anchors = {
  vfs_fsync: { path: "guest/linux/fs/fxfs/super.c", snippet: "int fxfs_vfs_fsync(struct file *file, loff_t start, loff_t end, int datasync)", note: "Linux's fsync of a guest Fxfs file: write back, then fxfs_fsync." },
  fxfs_fsync: { path: "guest/rust/fxfs-linux/src/lib.rs", snippet: "pub unsafe extern \"C\" fn fxfs_fsync(fs: *const Fxfs) -> c_int {", note: "Where the guest kernel's C calls into Fxfs's Rust." },
  posix_fsync: { path: "crates/fxfs-posix/src/lib.rs", snippet: "pub async fn fsync(&self) -> Result<(), Error> {", note: "Group commit: an fsync a sync already covers needs none of its own." },
  platform: { path: "crates/fxfs-posix/src/lib.rs", snippet: "pub trait Platform: Send + Sync + Sized + 'static", note: "What differs between the guest kernel and the host's tools, behind one POSIX layer." },
  journal_commit: { path: "crates/fxfs/src/object_store/journal.rs", snippet: "pub async fn commit(&self, transaction: &mut Transaction<'_>) -> Result<u64, Error>", note: "Fuchsia's journal: a transaction's mutations committed as a group." },
  commit_root_barrier: { path: "crates/fxfs/src/object_store/journal.rs", snippet: "device.commit_root(&root).await.map(|()| true)", note: "fxdax's change to Fuchsia's journal: its flush commits the root on the extent device." },
  transaction: { path: "crates/fxfs/src/object_store/transaction.rs", snippet: "pub struct Transaction<'a>", note: "Fuchsia's Transaction: mutation records committed together." },
  lsm: { path: "crates/fxfs/src/lsm_tree.rs", snippet: "pub struct LSMTree<K, V>", note: "Fuchsia's LSM tree, Fxfs's key/value store of layers." },
  object_store: { path: "crates/fxfs/src/object_store.rs", snippet: "pub struct ObjectStore", note: "Fuchsia's ObjectStore: files as objects." },
  device_trait: { path: "crates/storage_device/src/lib.rs", snippet: "pub trait Device: Send + Sync", note: "Fuchsia's Device trait, which fxdax extended with extents." },
  extent_id: { path: "crates/storage_device/src/extent.rs", snippet: "pub struct ExtentId {", note: "fxdax's own: an extent is named by (branch, sequence)." },
  extent_device: { path: "guest/rust/fxfs-linux/src/device.rs", snippet: "pub struct ExtentDevice", note: "The guest's extent-store device, which Fxfs runs on." },
  dev_commit_root: { path: "guest/rust/fxfs-linux/src/device.rs", snippet: "async fn commit_root(&self, root: &[u8]) -> Result<(), Error>", note: "Ordered like the device's other commits; durable only while a sync wants it." },
  device_submit: { path: "guest/linux/fs/fxfs/device.c", snippet: "int fxfs_device_submit(struct fxfs_device *dev, const struct fxfs_device_op *op,", note: "The guest C side of a device request." },
  virtio_submit: { path: "guest/linux/drivers/virtio_extent/driver.c", snippet: "int virtio_extent_submit(struct virtio_extent *vext, struct vext_request *req)", note: "Onto a virtqueue, one per CPU." },
  vmm_handle: { path: "vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs", snippet: "pub(super) async fn handle(store: &RemoteExtentStore, notices: &Notices, buffers: Buffers) -> u32", note: "Firecracker's worker serving one guest request through the daemon." },
  vmm_window: { path: "vendor/firecracker/src/vmm/src/devices/virtio/extent/window.rs", snippet: "pub(super) struct DaxWindow", note: "The DAX window in the VM process: guest physical addresses and the host memory KVM maps there." },
  remote_store: { path: "crates/fxdax/src/extentd/client.rs", snippet: "pub struct RemoteExtentStore", note: "The VMM's handle on its store in extentd." },
  remote_commit: { path: "crates/fxdax/src/extentd/client.rs", snippet: "pub fn commit(&self, root: Option<&[u8]>) -> Result<()>", note: "COMMIT with COMMIT_DURABLE, over the daemon socket." },
  server_serve: { path: "crates/fxdax/src/extentd/server.rs", snippet: "fn serve(&self, request: &Request<'_>, answer: Answer)", note: "extentd serving one request of a VM process." },
  flush_behind: { path: "crates/fxdax/src/extentd/server.rs", snippet: "async fn flush_behind(store: Arc<ExtentStore>, mut stopped: tokio::sync::oneshot::Receiver<()>)", note: "Every 50 ms: the commits ordered so far made durable." },
  server_fault: { path: "crates/fxdax/src/extentd/server.rs", snippet: "async fn fault(self: Arc<Self>, fault: Fault, answer: Answer)", note: "A guest fault on the window, which userfaultfd reported, resolved by the daemon." },
  extent_store: { path: "crates/fxdax/src/extents.rs", snippet: "pub struct ExtentStore", note: "An extent store as a VM's device serves it." },
  es_write: { path: "crates/fxdax/src/extents.rs", snippet: "pub fn write(&self, extent: ExtentId, offset: u64, buffers: &[VolatileSlice]) -> Result<()>", note: "WRITE: bytes into an open extent." },
  es_commit: { path: "crates/fxdax/src/extents.rs", snippet: "pub fn commit(&self, root: Option<&[u8]>) -> Result<()>", note: "COMMIT: seals the extents written before it; nothing in a window changes." },
  es_durable: { path: "crates/fxdax/src/extents.rs", snippet: "fn make_durable(&self) -> Result<()>", note: "Durable means synced locally, and uploaded if the store has a remote." },
  es_map: { path: "crates/fxdax/src/extents.rs", snippet: "pub fn map(&self, extent: ExtentId, offset: u64, len: u64, window_offset: u64) -> Result<()>", note: "MAP: a sealed extent's pages into the window, read-only." },
  es_fork: { path: "crates/fxdax/src/extents.rs", snippet: "pub fn fork(&self, path: impl AsRef<Path>, branch: u64) -> Result<(), Error>", note: "A fork: a new branch, as of the last commit made durable." },
  store: { path: "crates/extent_store/src/store.rs", snippet: "pub struct Store<D: Dir>", note: "The local extent store: the bucket's WAL and cache." },
  store_commit: { path: "crates/extent_store/src/store.rs", snippet: "pub fn commit(&self, root: Option<&[u8]>) -> Result<(), Error> {", note: "The local store's commit." },
  store_commit_fork: { path: "crates/extent_store/src/store.rs", snippet: "pub fn commit_fork(&self, child: Child) -> Result<u64, Error> {", note: "The commit that names a fork." },
  upload: { path: "crates/extent_remote/src/writer.rs", snippet: "pub async fn upload(&self, seq: u64) -> Result<(), Error>", note: "A segment to object storage." },
  acknowledge: { path: "crates/extent_remote/src/writer.rs", snippet: "pub async fn acknowledge(&self) -> Result<Option<u64>, Error>", note: "Acknowledged only once the lease is still the writer's." },
  bucket: { path: "crates/extent_remote/src/bucket.rs", snippet: "pub trait Bucket: Send + Sync + 'static", note: "Anything with conditional writes: S3, GCS, a directory." },
  fork_point: { path: "crates/extent_remote/src/branch.rs", snippet: "pub async fn fork_point(", note: "A fork's child record: it inherits its parent's extents without copying." },
  fetcher: { path: "crates/extent_remote/src/fetch.rs", snippet: "pub struct Fetcher<B: Bucket>", note: "Reads extents from a bucket by parts, under a budget." },
  dax_window: { path: "crates/fxdax/src/dax.rs", snippet: "pub struct DaxWindow(Arc<Window>);", note: "A DAX window of an extent-store device." },
  dax_fault: { path: "crates/fxdax/src/dax.rs", snippet: "pub async fn fault(&self, offset: u64, access: Access) -> Result<()>", note: "The guest's fault on the window: map the page." },
  cow: { path: "crates/fxdax/src/dax.rs", snippet: "pub(crate) struct CopyOnWrite", note: "How a window copies on write, and against which arena." },
  second_stage: { path: "crates/fxdax/src/dax.rs", snippet: "pub trait SecondStage: Send + Sync", note: "A window's translation outside the daemon: the VM process's memory." },
  uffd_serve: { path: "crates/fxdax/src/dax/uffd.rs", snippet: "pub fn serve(self: &Arc<Self>, window: Arc<dyn Faults>) -> Result<FaultThread, Error> {", note: "The VM process's thread taking KVM's faults by userfaultfd." },
  cached_extent: { path: "crates/fxdax/src/cache.rs", snippet: "pub(crate) struct CachedExtent", note: "One sealed extent in the cache: a table of its pages." },
  evict: { path: "crates/fxdax/src/cache.rs", snippet: "pub(crate) fn evict_least_used(arena: Option<ArenaId>, enough: impl FnMut(u64) -> bool) -> u64 {", note: "Eviction, least used first, windows' mappings last." },
  relieve: { path: "crates/physmem/src/pmm/relief.rs", snippet: "pub fn relieve_with(", note: "Relief of arenas starved at their caps." },
  arena: { path: "crates/fxdax/src/pages.rs", snippet: "pub struct Arena(ArenaId);", note: "An arena of physical memory: a tenant's." },
  guest_fault: { path: "guest/linux/fs/fxfs/dax.c", snippet: "static vm_fault_t fxfs_dax_do_fault(struct vm_fault *vmf, unsigned int order, bool write)", note: "The guest's DAX fault." },
  guest_writeback: { path: "guest/linux/fs/fxfs/dax.c", snippet: "int fxfs_dax_write_back(struct inode *inode)", note: "The guest's DAX writeback." },
  snapshot_volume: { path: "crates/fxfs/src/object_store/volume/snapshot.rs", snippet: "pub async fn snapshot_volume(", note: "fxdax's addition to Fuchsia's Fxfs: a volume snapshot sharing layers and extents." },
  es_commit_ordered: { path: "crates/fxdax/src/extents.rs", snippet: "pub fn commit_ordered(&self, root: Option<&[u8]>) -> Result<()>", note: "A barrier: ordered, not yet durable." },
  guest_dirty: { path: "guest/linux/fs/fxfs/dax.c", snippet: "static int fxfs_dax_dirty(struct inode *inode, struct fxfs_dax_range *range, unsigned long first,", note: "Before a page is dirty, the host makes it private (PRIVATE): dirty implies private." },
  guest_rebind_range: { path: "guest/linux/fs/fxfs/dax.c", snippet: "static long fxfs_dax_rebind_range(struct fxfs_dax *dax, struct fxfs *fs,", note: "Rebinding: clean private pages MAPped back to their sealed extents, the semaphore held for writing." },
  guest_swap: { path: "guest/linux/fs/fxfs/dax.c", snippet: "pages += fxfs_dax_pages_swap(range->state, batch[nr].pages);", note: "Writeback swaps the dirty set out for an empty one." },
  guest_write_back_keeping: { path: "guest/linux/fs/fxfs/dax.c", snippet: "static int fxfs_dax_write_back_keeping(struct inode *inode, bool keep)", note: "Writeback of a DAX file's dirty pages." },
  pages_swap: { path: "guest/linux/fs/fxfs/pages.c", snippet: "unsigned long fxfs_dax_pages_swap(struct fxfs_dax_pages *pages, unsigned long *set)", note: "The swap of a range's dirty set for its writeback set." },
  guest_device_private: { path: "guest/linux/fs/fxfs/device.c", snippet: "int fxfs_device_private(struct fxfs_device *dev, u64 window_offset, u64 len)", note: "PRIVATE: ask the host for a private copy of window pages." },
  window_make_private: { path: "crates/fxdax/src/dax.rs", snippet: "let copying = self.copy_on_write.as_ref().ok_or(Errno::EINVAL)?;", note: "The host side of PRIVATE: each page copied, at the same offset." },
  server_make_private: { path: "crates/fxdax/src/extentd/server.rs", snippet: "async fn make_private(self: Arc<Self>, window_offset: u64, len: u64, answer: Answer)", note: "extentd serving a PRIVATE." },
  vmm_private_max: { path: "vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs", snippet: "fn private_max(len: u64, configured: Option<u64>) -> u64 {", note: "private_max: 1 GiB by default, whatever the window's size." },
  cache_cached: { path: "crates/fxdax/src/cache.rs", snippet: "pub(crate) fn cached(", note: "A sealed extent in the cache, read in from a store that holds it if it isn't." },
  uffd_copy_on_write: { path: "crates/fxdax/src/dax/uffd.rs", snippet: "pub(crate) fn copy_on_write(&self) -> Result<(), Error> {", note: "The VM process's second stage made to copy on write." },
  extent_cache: { path: "crates/fxdax/src/cache.rs", snippet: "struct ExtentCache", note: "The process's one cache of sealed extents, shared by every store and window." },
  sim_swap_during_write: { path: "guest/sim/tests/scenarios.c", snippet: "static void cow_swap_during_write(void)", note: "guest/sim's scenario that kills the swap-shared mutant." },
} as const satisfies Record<string, Anchor>;

export type AnchorId = keyof typeof anchors;
