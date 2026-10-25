// A DAX region's `mapped`, which its lock guards, read under the filesystem's regions lock where
// a path reaches the read without the region's own lock held (TASK-44.17): as
// fxfs_dax_region_open looked at it to act under the regions lock, while a MAP in flight changed
// it (TASK-99). READ_ONCE reads and WRITE_ONCE writes are not flagged.

@once@
struct fxfs_dax_region *R;
position q;
@@
(
READ_ONCE(R->mapped@q)
|
WRITE_ONCE(R->mapped@q, ...)
)

// The region's lock never taken before.
@from_start exists@
identifier f;
struct fxfs_dax *D;
struct fxfs_dax_region *R;
position p != once.q;
@@
f(...) {
... when != mutex_lock(&R->lock)
spin_lock(&D->regions_lock);
... when != spin_unlock(&D->regions_lock)
    when != mutex_lock(&R->lock)
* R->mapped@p
...
}

// The region's lock let go of before.
@after_unlock exists@
struct fxfs_dax *D;
struct fxfs_dax_region *R;
position p != once.q;
@@
mutex_unlock(&R->lock);
... when != mutex_lock(&R->lock)
spin_lock(&D->regions_lock);
... when != spin_unlock(&D->regions_lock)
    when != mutex_lock(&R->lock)
* R->mapped@p
