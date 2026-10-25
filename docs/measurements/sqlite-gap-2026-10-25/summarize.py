#!/usr/bin/env python3
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Regenerate the SQLite investigation summary from every retained raw observation."""
import json
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PREFIX = 'gce-fs-bench-sqlite-gap-2026-10-25-'
TRIALS = [
    ('baseline', 'original main', 'baseline'),
    ('candidate1', 'retained: sustained append reservation', 'baseline'),
    ('candidate2', 'discarded: overwrite lookahead increased checkpoint cost', 'candidate2'),
    ('candidate3', 'build failed: missing control-API counter fields', None),
    ('candidate3-run', 'discarded: server-side replacement added no throughput gain', 'candidate3-run'),
    ('candidate4', 'build failed: test snapshot had a stats type error', None),
    ('candidate4-run', 'retained: smaller extent buffers; allocation reduction, no throughput claim', 'candidate4-run'),
    ('candidate5', 'discarded: client replacement did not close gaps; usage unconfirmed with INFO disabled', 'candidate5'),
    ('candidate6', 'retained: adjacent acknowledged UNMAP batching', 'candidate6'),
    ('candidate7', 'protection batching discarded: no protection batches observed; cached-read kernel retained', 'candidate7'),
    ('final', 'quiet retained batch, reversed case order, five iterations', 'final'),
    ('final-control', 'fresh original-main binaries and kernel; identical current timed harness', 'final'),
]


def measurements(directory, backend):
    path = ROOT / (PREFIX + directory) / f'bench-fs-{backend}.json'
    if not path.exists():
        return None
    data = json.loads(path.read_text())
    if data.get('failures'):
        raise SystemExit(f'Correctness failures prevent comparison: {path}')
    return {row['workload']: row for row in data['results']}


def describe(row):
    values = row['values']
    median = statistics.median(values)
    return dict(unit=row['unit'], higher_is_better=row['higher_is_better'],
                observations=values, median=median, minimum=min(values), maximum=max(values),
                median_absolute_deviation=statistics.median(abs(x - median) for x in values))


def compare(candidate, reference):
    c, r = statistics.median(candidate['values']), statistics.median(reference['values'])
    return c / r if candidate['higher_is_better'] else r / c


trials = []
for directory, outcome, ext4_directory in TRIALS:
    entry = dict(directory=PREFIX + directory, outcome=outcome)
    fxfs = measurements(directory, 'fxfs-guest-dax')
    ext4 = measurements(ext4_directory, 'ext4-dax-pmem') if ext4_directory else None
    if fxfs:
        entry['metrics'] = {}
        for name, row in fxfs.items():
            metric = describe(row)
            if ext4 and name in ext4:
                metric['ext4'] = describe(ext4[name])
                metric['performance_ratio_to_ext4'] = compare(row, ext4[name])
                metric['ext4_reference'] = PREFIX + ext4_directory
            entry['metrics'][name] = metric
    trials.append(entry)

final = measurements('final', 'fxfs-guest-dax')
control = measurements('final-control', 'fxfs-guest-dax')
original_comparison = {
    name: dict(original=describe(control[name]), final=describe(row),
               final_performance_ratio=compare(row, control[name]))
    for name, row in final.items() if control and name in control
}
summary = dict(
    baseline_revision='33074daf7e5e476bf1ccbd3ec098f17108fa4f78',
    firecracker_revision='01cbb2fe3d40e584970ccac5c59b0fdfd6af3853',
    instance='fxdax-bench-sqlite-gap-20261025', zone='us-east4-a', machine='n2-standard-8',
    final_kernel_sha256='0f0eaeb5f4937cb62234e90fbb19e7afcdc36f18d5147373f424f0c455965784',
    retained_changes=[
        'Reserve up to 16 aligned pages only after sustained unsealed appends; publish actual EOF.',
        'Size DAX extent-result buffers to covered pages, including speculative reservation.',
        'Keep the range lookup read lock through cached-read iomap construction.',
        'Skip redundant LRU tail movement and empty wake queues with the paired memory barrier.',
        'Combine at most 64 adjacent complete acknowledged UNMAPs already in the socket; ACK only actual consumed taking IDs after applying the union.',
    ],
    interpretation=[
        'Ratios greater than one favor Fxfs; checkpoints use inverse elapsed-time ratios.',
        'All observations, including slower experiments, are retained. No profiler timings are used as unprofiled performance evidence.',
        'Five final observations and three fresh original-main observations have different sample counts; no statistical significance or tail-latency guarantee is claimed.',
        'The fresh original control uses frozen original daemon/VMM and original kernel with the same current runner, guest agent and unchanged timed SQLite SQL.',
        'Extent allocation bytes count requested buffer bytes over a phase, not resident memory or allocator-rounded sizes.',
        'Whole-host profiles include setup, recovery, verification and snapshot work; guest phase profiles isolate individual workload sections.',
    ],
    remaining_gaps={name: metric for name, metric in trials[-2].get('metrics', {}).items()
                    if metric.get('performance_ratio_to_ext4', 1) < 1},
    remaining_diagnosis=[
        'DELETE/FULL batch100 still makes about 23400 DAX map-write transactions per 200 SQLite transactions, roughly one per updated database page plus journal work.',
        'Guest DELETE profile attributes 6.7 percent of all guest samples to map-write and 6.4 percent to transaction commit, with about 72 percent idle. These are directional profiles, not latency attribution.',
        'Overwrite lookahead and both replacement approaches did not close the remaining gaps in measured runs and were removed.',
        'Small checkpoints remain variable; UNMAP batching reduces their mapping work but does not remove every remaining median or tail gap.',
    ],
    diagnostics=[PREFIX + x for x in ['profile', 'profile-delete', 'profile-full-checkpoint',
                                    'profile-scans', 'host-profile', 'host-profile2']],
    trials=trials, final_against_fresh_original=original_comparison,
)
(ROOT / 'sqlite-gap-2026-10-25' / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
