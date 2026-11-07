# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The pjdfstest (BSD-2-Clause) every run of it builds, one pin for all: the guest's Fxfs's
# (pjdfstest-fxfs-guest, scripts/test-guest-kernel.sh and GCE's scripts/lib/test-linux-host.sh)
# and FUSE mounts' (scripts/pjdfstest-fuse.py, CI). Sourced; each may be set beforehand instead.
export PJDFSTEST_REPO=${PJDFSTEST_REPO:-https://github.com/pjd/pjdfstest}
export PJDFSTEST_COMMIT=${PJDFSTEST_COMMIT:-85a8aea9e685999ef0540392fd80535f873d7ff7}
