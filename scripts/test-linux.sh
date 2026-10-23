#!/usr/bin/env bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Run scripts/test.sh on Linux inside a Lima VM (see scripts/lib/lima.sh).
exec "$(dirname "$0")/lib/lima.sh" test.sh "$@"
