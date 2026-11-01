# Theory: `git status` hashes every file when a user other than the index's runs it

**Tried:** 2026-10-31, TASK-136.4, in `fxfs-guest-shared-volumes`.
**Idea:** the index records a uid, so a shared volume's consumers would have to run as the
publisher's user, and a status by another user would prove the test can see a rehash.
**Result:** with uid 1001 reading files uid 1000 owns, most of the 200 tracked files were not opened,
and the check failed in both the plain and DAX cases.
**Why it's dead:** the index records the file's owner (`st_uid`), not who runs Git. Files rehash when
their owner changes: all 200 were opened after a `chown`, which is what the test checks instead. So
the rule is that files belong to whoever consumers run as from the clone on, not that readers match.
**Reopen if:** never: it is how Git's stat comparison works.
