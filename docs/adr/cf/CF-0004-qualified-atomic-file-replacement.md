# CF-0004. Qualified Atomic File Replacement

Date: 2026-09-06
Last-reviewed: 2026-09-06
Tier: B
Status: Draft
Crates: comment-free

## Related

References: CF-0001, CF-0002, CF-0003

## Context

Truncating a destination before a rewrite finishes exposes partial files.
The existing implementation instead replaces one pathname through a sibling
temporary file and checks for intervening edits. Sources:
[write guarantees and limits](../../../README.md#how---rewrite-writes-and-what-it-does-not-promise)
and [implementation](../../../src/lib.rs) (`write_atomically`,
`TempFileGuard`, `process_file`). This Draft retains those qualifications
rather than promising transactional concurrent editing or power-loss durability.

## Decision

Keep staged replacement instead of in-place truncation on supported Unix
platforms. Dry-run mode produces a preview without writing.

R1 [5]: Write replacement bytes to a sibling temporary file, flush and sync them, apply destination permissions, and rename over the destination rather than truncating the source in place.

R2 [5]: Refuse a symbolic-link destination at the write boundary; re-read destination bytes before rename and report a conflict without replacing the destination when they differ from the original read.

R3 [5]: Limit atomic-replacement and permission-preservation claims to supported Unix platforms; retain the documented concurrent-writer race, hard-link behavior, unsynced parent directory, and best-effort temporary-file cleanup limits.

The re-read and rename are not an atomic compare-and-swap and use no lock.
Rename replaces only the selected pathname's inode; other hard links retain
old contents. Synced replacement bytes do not guarantee that the unsynced
directory entry survives power loss. The temporary-file guard attempts
removal on returning errors and panic unwinding; failed removal, aborts,
and signals can leave residue. Windows semantics are not claimed or tested.

## Consequences

+ becomes easier: A successful replacement avoids exposing a partially written destination.
− becomes harder: Callers cannot rely on hard-link alias updates or exclusion of concurrent writers.
risks/migration: No stronger durability or concurrency guarantee is accepted.
Existing [unit tests](../../../src/lib.rs) cover
`destination_changed_since_read_is_a_conflict_and_leaves_bytes_untouched`,
`rewrite_preserves_the_destination_file_mode`,
`a_symlink_destination_is_refused_and_its_target_is_left_alone`, and
`an_io_failure_after_temp_creation_leaves_no_residue`.
