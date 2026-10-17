#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run scripts/bench.sh on Linux inside a Lima VM (see scripts/lib/lima.sh).
# The results stay in the VM, under the directory the script prints.
exec "$(dirname "$0")/lib/lima.sh" bench.sh "$@"
