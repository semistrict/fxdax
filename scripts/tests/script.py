# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""The scripts the tests in scripts/tests test, as they load them."""

import importlib.util
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent.parent


def load(name):
    """scripts/NAME.py, as a module."""
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module
