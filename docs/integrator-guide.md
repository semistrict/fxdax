# Integrator guide: running tenants' VMs on extentd

For whoever writes the VM manager, the trusted service on each host that talks to the extentd
daemon and starts Firecracker. `crates/fxdax/proto/fxdax/extentd/v1/extentd.proto` is the reference
for every call named here.

The VM manager is trusted. VM processes are not: treat one as compromised, and everything here
follows from that.

## Per-host setup

1. Start `fxdax-extentd` with `FXDAX_EXTENTD_GRPC` naming its control socket. Keep that socket away
   from jails, and set the daemon's umask so that only the VM manager can reach it: it can do
   everything, and unlike the VM socket it follows the umask (TASK-159).
2. For each tenant, call `Grant` with the tenant's name, the directory of its stores, and its jails'
   uid and gid. It returns the path of a socket.
3. Bind-mount **that socket alone** into the tenant's jails. Never mount the daemon's default VM
   socket into a jail: it can take any branch by its ID.
4. Run jails with Firecracker's jailer, as a uid other than the daemon's. The default VM socket is
   mode `0600`, owned by the daemon, so that a mistake in step 3 still can't connect.

Grants don't survive a restart of the daemon. After one, call `Grant` again, and bind-mount the new
socket.

## Restoring a snapshot, on any number of hosts at once

A snapshot's point is immutable. Any number of hosts read it, and none writes it: it has no lease, a
restore on one host changes nothing another host sees, and no copy of it goes stale. Each restore
is a fork of the point into a new branch of the VM's own.

A VM process can't name a branch: extentd ignores what it says, or any stolen ID would take another
tenant's data. The VM manager puts the snapshot's branch in the tenant's directory instead, and that
is the grant.

Without step 2 below, a restore on a host that has no copy fails with `ENOENT`.

### 1. On the snapshot's host, once it's taken

```
branch = StatStore(point).branch      # keep this with the snapshot
```

The point is in the bucket by the time the snapshot returns.

Use this `branch`. **Never use `point_branch` from the snapshot file: a VM process wrote it.**

### 2. On each host that will restore it, once, before its first `PUT /snapshot/load`

```
CreateStore(path = <tenant's directory>/<point>, capacity, branch)
```

| Result | Meaning | Do |
|---|---|---|
| OK | The store is made. The daemon takes the point at the first restore | Go on |
| `ALREADY_EXISTS` | A store is there, perhaps a VM process's | Use it only if `StatStore(path).branch == branch`. Otherwise stop |

A point needs no `token`, so ignore `PlaceStore` for it. Keep the copy for as many restores as you
like.

Only do this for the tenant whose snapshot it is: its VM processes can restore the point and read it.
They can't write it: opening a point to write fails with `EROFS`.

### 3. Load the snapshot

In `extent_overrides`:

| Field | Value |
|---|---|
| `point_on_host` | The path from step 2 |
| `path_on_host` | The VM's store, in the tenant's directory |
| `extentd_socket` | The tenant's socket, as the jail sees it |

Do this as often as you like, on any host, at the same time. Each restore makes a new branch for
the VM.

If a restore fails with `ENOENT` although the copy from step 2 exists, the snapshot was deleted.

### 4. When this host won't restore that snapshot any more

```
DropStore(point)
```

This ends the tenant's access on this host and frees the copy. Nothing needs it for restores to work
elsewhere, or to work again here: `CreateStore` takes it back.

`DropStore` fails with `FAILED_PRECONDITION` (`EBUSY`) while a VM process has the store open.
`fxdaxctl drop DIR` does the same from a shell.

### 5. When the snapshot is finished with, everywhere

```
DeleteStore(point)
```

**This deletes the snapshot on every host.** VMs already restored from it keep running and keep
their data. New restores fail with `ENOENT`, on every host. A restore racing the deletion either
fails or succeeds whole: one that succeeded keeps everything it inherited.

## Limits

**Only snapshots.** A point is what a snapshot takes (`PUT /snapshot/create`). A fork of anything
else, a VM's store or an imported image, is a commit on that store's branch, so its host needs to be
the branch's writer. Fork those on one host at a time.

**Memory.** Open pages can't be evicted until a guest seals them, and only a guest can seal. Several
VMs share the daemon's memory, so one idle VM with many open pages can leave another short, which
it sees as `ENOMEM`. This hasn't been run with several VMs at once.
