# CF-0001. Bootstrap ADR Governance

Date: 2026-09-06
Last-reviewed: 2026-09-06
Tier: B
Status: Draft
Crates: comment-free

## Related

Root: CF-0001

## Context

The repository already records its operating constraints in AGENTS.md,
README.md, Cargo.toml, rust-toolchain.toml, deny.toml, and
docs/record-format.md. A repository-owned corpus enables ADR validation
and discovery without importing unrelated upstream decisions or implying
that a new governance policy has been accepted.

## Decision

R1 [5]: While this bootstrap remains Draft, existing repository documentation and configuration remain authoritative; registering this corpus does not accept new policy or supersede existing decisions.

## Consequences

Local development and CI can validate and discover a nonempty CF corpus.
This Draft leaves the single-crate architecture, Rust toolchain and lexer
pins, pedantic lint posture, comment-hygiene policy, JSON Lines record
contract, and supply-chain checks unchanged. Future policy proposals still
require explicit review rather than acceptance inferred from successful lint.
