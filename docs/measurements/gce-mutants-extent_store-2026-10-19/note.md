TASK-44.1: cargo-mutants over extent_store on GCE n2-standard-8 (scripts/mutants-gce.sh, 3 jobs).

- sweep/: every mutant, the workspace tests without features: 1,260 mutants in 2 h: 830 caught, 209 missed, 11 timeouts, 210 unviable.
- shuttle/: the missed again, with the `shuttle` feature, its tests alone: 220 in 2 h: 22 more caught, 196 missed.
