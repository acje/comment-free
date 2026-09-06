# CF-0006. Build and Dependency Posture

Date: 2026-09-06
Last-reviewed: 2026-09-06
Tier: B
Status: Draft
Crates: comment-free

## Related

References: CF-0001, CF-0002

## Context

This repository is a standalone binary and library crate, not the monorepo
from which it was extracted. Its lexer dependency is an unstable snapshot.
Sources: [Cargo.toml](../../../Cargo.toml),
[toolchain pin](../../../rust-toolchain.toml),
[supply-chain configuration](../../../deny.toml),
[operational guidance](../../../AGENTS.md#build--test--lint), and
[CI](../../../.github/workflows/ci-reusable.yml).
This Draft records existing build constraints without importing workspace
inheritance or policies from the reference corpus.

## Decision

Keep explicit local configuration and locked dependency resolution instead
of floating the lexer snapshot or inheriting a nonexistent workspace table.

R1 [5]: Retain the standalone crate's concrete pedantic Clippy lint table and stable-default formatting; preserve the unsafe-code prohibition in both library and binary crate roots.

R2 [5]: Preserve the Rust toolchain pin and exact lexer snapshot requirement in their authoritative configuration files; build and test with the committed Cargo.lock rather than silently changing dependency resolution.

R3 [5]: Apply the configured cargo-deny advisory, license, ban, and source checks alongside cargo-audit; retain local build, test, Clippy, and formatting checks documented in AGENTS.md.

The current values are Rust 1.98.0, edition 2024, and
`ra-ap-rustc_lexer = "=0.174.0"`. `deny.toml` denies yanked packages,
wildcards, unknown registries, and unknown Git sources; multiple versions
are warnings, not a universal ban. Its license allowlist is explicit.
The crate roots in [lib.rs](../../../src/lib.rs) and
[main.rs](../../../src/main.rs) use `forbid(unsafe_code)`.
Canonical Git installation uses an explicit invoking toolchain and
`--locked`, as documented in [AGENTS](../../../AGENTS.md#consumption).

## Consequences

+ becomes easier: Local checks and CI exercise an explicitly selected toolchain and dependency graph.
− becomes harder: Snapshot and toolchain upgrades require deliberate configuration changes.
risks/migration: Locked resolution alone is not proof of byte-identical
binaries or absence of vulnerabilities. No dependency, toolchain, lint,
license, or CI-policy change is made by recording this Draft.
