# Failing every test whose VMM logs a DAX window refusal

**Tried:** 2026-10-29, TASK-44.12, 633ba8bc0, reverted by 06fffef19. r216.
**Idea:** A store the host refuses, with the guest believing the page writable, is an invariant
broken. Kept logs of failed runs before TASK-121's fixes held 21 and 72 refusals.
**Result:** Every DAX suite failed on refusals its VM survived: checks-dax 469, checks-dax-delay
501, crash-dax 3, pgbench-dax 104.
**Why it's dead:** Refusals the window resolves are routine. r241-r244 classified them: a store
reaching the window after a seal made its page read-only, before the guest's mapping did, is refused
and retried through the guest task's own fault. All 133 of r244's came from KVM's async page-fault
workers.
**Reopen if:** Nothing in this form. scripts/dax-refusals.py (5a2a6a12b) fails a run only on a
refused store no seal explains.
