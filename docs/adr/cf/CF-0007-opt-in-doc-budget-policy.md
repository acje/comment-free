# CF-0007. Opt-In Doc Budget Policy

Date: 2026-09-07
Last-reviewed: 2026-09-07
Tier: B
Status: Accepted
Crates: comment-free

## Related

References: CF-0003, CF-0005

## Context

Repositories need advisory and enforced prose budgets without conflating
uncertainty with pass. Explicit feature authorization in mission
`comment-free-8kf` accepts this prospective capability; CF-0001 through
CF-0006 remain retrospective records of the legacy behavior. A two-run
wrapper duplicates reads/parses; filtering advisory findings loses
threshold-dependent undecided results.

The authoritative [record specification](../../record-format.md) and
[usage](../../../README.md) define the opt-in contract. Implementation lives
in [policy evaluation](../../../src/policy.rs),
[runner](../../../src/policy_run.rs), and
[typed records](../../../src/policy_records.rs).

## Decision

R1 [5]: Require explicit ordered nonnegative advisory and enforced thresholds for the opt-in gate; preserve legacy lint, rewrite, library, and record behavior outside that mode.

R2 [5]: Return zero for decided pass including advisory findings, one for decided enforced breach, and two for unknown or error; either threshold's uncertainty, processing faults, or empty Rust scope outranks breach.

R3 [6]: Read and parse each discovered file once, independently evaluate the unchanged analyzer at both thresholds on the same AST, and never derive enforced findings by filtering advisory results.

R4 [6]: Emit independently versioned policy summary and detail families using kind/version envelopes; retain legacy record/v run diagnostics, exact checked counters, and threshold labels. Suppress exact summaries after accounting failure.

R5 [5]: Apply warning-file admission independently per threshold without limiting analysis; retain at most fifty admitted hints per threshold and preserve workload-dependent path, source, AST, report, and payload sizes without process-memory guarantees.

## Consequences

+ becomes easier: Repository CI can consume a native truthful policy verdict.
− becomes harder: Consumers must handle mixed record envelopes and reconcile
summary delivery with process exit, including partial output failures.
risks/migration: Unknown dominates enforced breach; both budgets are cumulative,
not unique-item partitions. No macro expansion, global quotas, concurrency or
durability guarantees are introduced. Acceptance lives in
[CLI tests](../../../tests/policy_gate.rs) and
[strict differential checks](../../../tests/policy_schema.py).
