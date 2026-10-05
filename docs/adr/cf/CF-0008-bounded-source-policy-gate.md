# CF-0008. Bounded Source Policy Gate

Date: 2026-10-05
Last-reviewed: 2026-10-05
Tier: B
Status: Accepted
Crates: comment-free

## Related

References: CF-0007, CF-0003, CF-0005

## Context

Mission code-1rx.2 authorizes a native bounded-source gate rather than another
mode or macro expansion. Strict legacy lint and rewrite remain unchanged.
The [record contract](../../record-format.md) owns protocol details.

## Decision

R1 [5]: Supersede CF-0007 R2 and R3 only for the opt-in gate: independently evaluate visible source at both thresholds on one parsed AST; macro coverage limitations do not dominate the bounded verdict.

R2 [6]: Count each contiguous literal doc-attribute block inside macro tokens independently, once at its source location; never infer generated item identity, expansion multiplicity, or synthesized prose.

R3 [5]: Preserve CF-0003 R1 for legacy analysis and required nonmacro uncertainty; gate macro uncertainty is separate coverage evidence, never a processing error or proof of complete documentation coverage.

R4 [6]: Version both policy record families to two; retain checked coverage counters, explicit coverage events and actionable next steps. Legacy record families and library lint remain unchanged.

R5 [5]: Read, parse, traversal, output, accounting, CLI faults and empty scope remain Unknown exit two; required nonmacro unreadable or configuration-dependent docs still dominate breaches.

## Consequences

+ becomes easier: Source-visible macro docs receive actionable budget checks.
− becomes harder: Consumers must migrate policy versions and separately inspect expansions.
risks/migration: Nonliteral macro attributes and synthesized docs are unevaluated;
interleaved non-doc attributes split contiguous blocks and can under-count one
expanded item's combined prose. This is an explicit source approximation.
coverage counters retain the old uninspected_macro_body spelling but now describe
source bodies, not verdict-blocking undecided items. No expansion, concurrency,
new buffering or process-memory bound is introduced. Acceptance is exercised by
[gate tests](../../../tests/policy_gate.rs) and
[schema checks](../../../tests/policy_schema.py).
