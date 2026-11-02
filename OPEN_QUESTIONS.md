# Open questions

What the agents need from you, and nothing else. See AGENTS.md. Answer by editing an entry, or say so in any session.

## Push the batch?

`fix/refusal-gate` is ahead of main with: `generic/129`'s fix, transactions reporting their room, cross-host tenant restores, the corrupt image fixes, the fault limit, and three flaky test fixes. Main fails two tests intermittently until it lands.
**Recommend:** push once the batch's GCE run passes.
**Until then:** it stays local. Agents don't push without your word.

## Must one snapshot restore on several hosts at once? (TASK-158)

Today, once one host forks a point, every other host's copy fails with `EIO` until it's dropped and created again. Fixing that needs a protocol change.
**Recommend:** yes if you fan out from one base; no otherwise.
**Until then:** one host at a time works; the limit is documented in `extentd.proto`.

## Your VM manager needs a change for cross-host restores (TASK-154)

Only you can make it: the steps are in `extentd.proto`, on `CreateStore` and `DropStore`, and restores on another host fail without them.
**Until then:** those restores get `ENOENT`.

## How to close truncate on a full disk? (TASK-150.19)

Rarely, truncated bytes come back on a truly full filesystem. Either every non-DAX growth takes one path that zeroes the tail, or a mid-block shrink may return `NoSpace`, partly reversing TASK-60.
**Recommend:** the first, with one owner of file size (TASK-153.4).
**Until then:** the default model test could very rarely go red.

## May the format change? (TASK-153.9, TASK-150.6)

Freed extent IDs can be minted again after a take, and that needs a format change: spent runs in remote checkpoints. A branch owner and the collector's in-flight list would go in the same bump.
**Recommend:** yes, one bump, before there's a format to migrate.
**Until then:** the bug stays open, within one tenant.

## What should guard file data on DAX? (TASK-153.11)

About 84% of flipped file-data bytes read back wrong with no error, because no read checks a data checksum, and a DAX read goes straight to memory. Whether to add a scrub, a host-side check, or accept it is a product decision.
**Recommend:** read the layer checksums now, which needs no format change, and a scrub for data.
**Until then:** silent data corruption goes undetected.

## Raise the GCE CPU quota?

32 CPUs is 4 hosts, shared by every session, and it is what most slows this work.
**Recommend:** 64 or more.
**Until then:** runs queue.
