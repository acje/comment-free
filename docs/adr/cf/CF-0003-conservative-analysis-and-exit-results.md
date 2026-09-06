# CF-0003. Conservative Analysis and Exit Results

Date: 2026-09-06
Last-reviewed: 2026-09-06
Tier: B
Status: Draft
Crates: comment-free

## Related

References: CF-0001, CF-0002

## Context

Syntactic analysis cannot expand macros or resolve arbitrary configuration
predicates. Conflating missing evidence with a clean result would hide
uninspected documentation. Sources:
[record semantics](../../record-format.md#doc_lint_undecided),
[exit codes](../../../README.md#usage), and
[CLI implementation](../../../src/main.rs) (`run_lint`, `strip_verdict`).
This Draft records the existing distinction between findings,
indeterminates, and processing errors.

## Decision

Keep uncertainty visible rather than treating unreadable payloads as zero
words or summing mutually exclusive configurations into a proven finding.

R1 [6]: Report unresolved configuration-dependent docs, unreadable doc payloads, and uninspected doc-bearing macro bodies as undecided, separately from findings; omit word counts when no reading produced them.

R2 [5]: Reserve default lint exit zero for runs with no findings, undecided items, or processing errors; return four for findings or undecided items and five for per-file processing errors.

R3 [5]: Return three for pending dry-run rewrites and zero for unchanged previews or successful writes; give processing errors exit five precedence over pending changes.

Constant configuration predicates are folded; real cfg keys remain
unresolved. An all-configurations count is an upper bound, not evidence
that a build with that doc set exists. Conditional fences can prevent a
finding even when unconditional text alone looks over budget. Unreadable
payloads take precedence because they may alter fence state. Invalid CLI
arguments exit two; catastrophic or unmapped I/O errors and exact lint-counter
overflow exit one. The CLI warning-file cap limits details, not the corpus or
verdict: hidden undecided items still cause exit four. `run_lint` and
`cap_verdict_matrix` preserve that distinction. CLI scope is the supplied regular
Rust file, a project allowlist for cwd/default or manifest roots, or recursive
Rust traversal for other explicit directories; no upward discovery occurs.

## Consequences

+ becomes easier: Automation distinguishes missing evidence from established findings.
− becomes harder: Some macro-using crates cannot obtain a clean default lint result.
risks/migration: Clean is limited to the syntactic coverage described in the
[README](../../../README.md#known-limitation-doc-attributes-inside-macro-bodies).
Procedural macros synthesizing docs without visible doc tokens remain
undetected. Successful write mode does not establish lint cleanliness.
Existing [tests](../../../tests/end_to_end.rs) include
`mutually_exclusive_cfg_docs_are_undecided_not_a_finding`,
`a_raw_spelled_doc_payload_is_not_reported_clean`, and
`a_raw_spelled_doc_inside_a_macro_body_is_not_reported_clean`.
