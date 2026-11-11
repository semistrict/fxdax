# Runbook: upgrading a host's extent daemon by draining it

For whoever operates fxdax hosts: how a host's `fxdax-extentd` and Firecracker are upgraded, and what
a format change refuses on the way. The design behind it is backlog doc-11 (TASK-141); the VM
manager's calls named here are `crates/fxdax/proto/fxdax/extentd/v1/extentd.proto`'s.

## Why a drain

The daemon holds its VMs' memory, their stores and their connections, and hands none of it to
another process. When it exits, every VM on the host exits with 158 (`ExtentdLost`). So a daemon
is upgraded by emptying its host first: its VMs move to other hosts or stop, and only then does the
daemon stop.

fxdax migrates nothing (AGENTS.md, "No migrations, ever"). Each format says its version, and a
reader refuses any other. The components of one build run together, and two builds meet only where
their formats are the same. A VM moves only to a host whose build has the same formats as its own.

## What a format change refuses

Read the release's notes for which of these changed. Each one changed means what its row says:

| Format | Refused when | How it shows |
|---|---|---|
| VM process and daemon wire (`wire::VERSION`) | A VMM and a daemon of different versions meet: the daemon refuses its `OPEN` | The VMM fails to make its extent device: "the extent daemon refused this VM process's wire version N". The daemon's log says "refusing a VM process of wire version N: this daemon's is M". |
| Device protocol (`VIRTIO_EXTENT_VERSION`) | A guest's driver meets a device of another version, or a snapshot of another version is restored | The guest's kernel log says "the device is of protocol version N, this driver of M: refused", and its extent filesystem doesn't mount. The VMM refuses the snapshot's load: "The snapshot's extent device is of protocol version N, this VMM's M". |
| Store files (`extent_store` `FORMAT`) | The new daemon opens a store of the old format | `ENOTSUP`. The store is made again, from its branch in the bucket if that format didn't change too. |
| Branches in the bucket (`extent_remote` `layout::FORMAT`) | A daemon reads a branch of another format | The branch is refused, and the collector's pass stops, deleting nothing. |
| Fxfs (`LATEST_VERSION`) | A guest mounts a filesystem of another version (TASK-141.1) | The mount fails. |
| VM snapshots (Firecracker's) | A VMM restores a snapshot of another snapshot version | The load fails. |

A wire or device protocol change alone leaves stores and branches readable: VMs move by snapshot
to a host of the new build only if the device protocol is the same, and stop and boot again there
otherwise. A store or bucket format change means its stores are made again, not upgraded.

## Steps

1. **Cordon the host.** Have the scheduler place no new VMs on it.
2. **Move its VMs off, or stop them.** Move each by snapshot and restore through the bucket, as
   `scripts/migrate-host.sh` does, to a host whose build has the VM's formats (above); the
   integrator guide's "Restoring a snapshot" has the calls. Stop a VM whose tenant accepts that.
   Followers go with or after their writer. When the host is empty, its daemon's `/metrics` says
   `fxdax_extentd_connections 0`.
3. **Stop the daemon with SIGTERM.** It drains, then exits (`fxdax::extentd::drain`, TASK-141.4):
   - its health service reports NOT_SERVING;
   - its VM sockets refuse connections, and its control API refuses calls with `UNAVAILABLE`;
   - any VM still connected is cut off and exits 158, its last commits made durable and its store
     closed;
   - the points it kept open for forks are closed;
   - every store's uploader uploads what its store made durable, and stops.

   It does all of that within `FXDAX_EXTENTD_DRAIN` seconds (60 by default). If it finishes, it logs
   "drained in ...: every store closed" and exits 0. If not, it logs "not drained within Ns: ...
   left", naming how many connections, control calls and uploaders were left, and exits 1. Then the
   stores' durable commits are still on the host's disk, and the next daemon to open each store
   uploads the rest.
4. **Install the new daemon and Firecracker together.** They are one build. A guest kernel is
   upgraded as its VM next boots, with the Firecracker it boots on. It isn't part of this.
5. **Start the daemon, and check it.** Its health service says SERVING, and `/metrics` says the new
   build: `fxdax_extentd_build_info{commit="..."} 1`. Call `Grant` again for each tenant, because
   grants don't survive a restart.
6. **Uncordon the host.** Moving VMs back is step 2 again, onto this host. Each one restored here
   then runs on the new Firecracker, which `fxdax_extentd_vm_build_info` shows by branch.

## Rolling back

A rollback is the same drain, to the previous build. It works only if no format changed between the
two builds. Past a format change, the old build refuses what the new one wrote, by the rows above:
that isn't supported, by design.
