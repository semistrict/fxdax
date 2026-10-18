# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Checks of the repo beyond its tests (scripts/test.sh runs those).

.PHONY: check-license-headers fix-license-headers check-tla

# Each source file carries the license header its origin needs: Fuchsia's copyright, and any
# other its Fuchsia sources carry, on what was copied or adapted from Fuchsia, fxdax's on its own
# code, under the license of what it comes from (scripts/license-headers.py). Reads the Fuchsia
# checkout ($FUCHSIA, by default ~/src/fuchsia), as scripts/upstream-changes.py does.
check-license-headers:
	scripts/license-headers.py

# Adds the headers missing.
fix-license-headers:
	scripts/license-headers.py --fix

# Model-checks the TLA+ specs (specs/), and that each mutation of them fails (backlog doc-8,
# TASK-44): a few minutes, on Java.
check-tla:
	scripts/check-tla.sh
