---
id: m-3
title: "Guest metadata 1: guest Fxfs on a local extent store"
---

## Description

Fxfs ported into the guest kernel, over a new virtio extent-store device in libkrun backed by host memory and local disk; DAX reads; no object storage, no forks. Measures metadata operations and git status against ext4 in the guest, and settles the write path. See doc-2.
