#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Regenerate run notes and custom-run provenance from the investigation inputs."""
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
inputs = json.loads((HERE / 'run-notes.json').read_text())
for name, data in inputs['runs'].items():
    directory = ROOT / name
    directory.mkdir(exist_ok=True)
    text = data['description'].strip() + '\n\nOutcome: ' + data['outcome'] + '\n'
    (directory / 'note.md').write_text(text)
    if 'source' in data:
        (directory / 'source.txt').write_text(data['source'])
    if data.get('same_instance_as'):
        (directory / 'instance.json').write_bytes((ROOT / data['same_instance_as'] / 'instance.json').read_bytes())
