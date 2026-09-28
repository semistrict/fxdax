---
id: m-1
title: "Step 7: libkrun DAX"
---

## Description

libkrun guests map Fxfs file memory directly through the virtio-fs DAX window: HVF on macOS, KVM (with userfaultfd where needed) on Linux.
