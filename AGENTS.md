# AGENTS.md — comment-free

Repo-specific operational notes. General agent/OODA doctrine, bash
hygiene, and the Rust no-`//`-comments house style live in the global
`~/.config/opencode/AGENTS.md` — not repeated here.

## What this repo is

A single-crate Rust binary (+ library `comment_free`): a Rust source
comment-hygiene tool. Default mode is read-only and lints doc-comment
length; `--rewrite` strips non-doc comments and canonicalises rustdoc
link idioms in place. See `README.md` for modes and exit codes.

- House Style: `attended-app` (as mapped in `sf-sdlc.toml`).
- Exit codes adhere strictly to the fleet tri-state taxonomy:
  - `0`: clean pass / compliant / all assertions verified.
  - `1`: domain defect / violation / finding flagged.
  - `2`: unknown / environmental error / missing permission / indeterminate.
- Stream separation: structured machine-readable findings (TSV, JSON Lines)
  stream to `stdout`; operational telemetry, diagnostic logs, and error traces
  route to `stderr`.

This tool is the mechanical enforcement surface for the fleet house
rule "no non-doc comments in Rust source". It preserves doc comments and
nothing else: there is no `// SAFETY:` carve-out and no marker allowlist.

Machine-readable lint and rewrite records are JSON Lines; the grammar and
its compatibility rules live in `docs/record-format.md`.

## Verification tiers (three-tier cadence)

Verification is strictly tier-scoped. A claim is backed by the tier whose scope
matches the claim: sub-missions are backed by MID; epics and releases are backed
by BOUNDARY.

- **INNER** (every hopper TDD increment and per-review-round re-verification;
  changed crate ONLY; exit-code criterion: test + clippy exit 0):
  ```sh
  CARGO_TERM_PROGRESS_WHEN=never cargo test -p comment-free --locked --message-format=short
  CARGO_TERM_PROGRESS_WHEN=never cargo clippy -p comment-free --all-targets --locked --message-format=short -- -D warnings
  ```
  `--all-targets` is mandatory on clippy to catch test/bench/example lints.
  `--workspace` and `--all-features` are forbidden at this tier.

- **MID** (once at sub-mission completion before done-claim; changed crates
  plus their reverse-dependent closure; exit-code criterion: test + clippy exit 0):
  ```sh
  cargo test --all-targets --locked
  cargo clippy --all-targets --locked -- -D warnings
  cargo fmt --all -- --check
  ```
  `--workspace` is forbidden at this tier; verify stays scoped to the affected closure.

- **BOUNDARY** (once per epic before epic done-claim; full workspace; exit 0 across all):
  ```sh
  cargo build --all-targets --locked
  timeout 900 cargo test --locked --no-fail-fast
  cargo clippy --all-targets --locked -- -D warnings
  cargo fmt --all -- --check
  sh scripts/verify.sh
  ```
  - `timeout 900` is mandatory on the test line. Exit 124 is `Outcome::Surprise`,
    NEVER a test failure. Investigate the stall; do not fold it into a failure count.
  - `--no-fail-fast` is mandatory on the test line to ensure full blast-radius
    visibility in a single pass.

- `clippy::pedantic` is the standing bar (`[lints.clippy] pedantic =
  warn` in `Cargo.toml`), not an elevation — new code passes it with
  zero warnings. This is a **concrete** table, not `workspace = true`;
  this is a standalone repo with no workspace, so do not reintroduce
  the inherited form — it would silently drop the lint posture.
- `rustfmt` runs on **stable defaults only** — do not add a
  `rustfmt.toml` or `clippy.toml`.
- `rust-toolchain.toml` pins channel 1.98.0 (clippy + rustfmt). Use it;
  don't bump without cause.
- `ra-ap-rustc_lexer` is pinned **exactly** (`=0.174.0`). It is an
  unstable-by-policy rustc internal published as a snapshot; a floating
  requirement will break the lexer pass. Do not relax the pin.

## Consumption

Installed, not vendored:

```sh
cargo +1.98 install --git https://github.com/acje/comment-free --locked comment-free
```

`cargo install --git` ignores this repo's `rust-toolchain.toml` and
builds with the invoking toolchain, hence the explicit `+1.98`.

## Delivery

Use `main` as the sole local and remote branch. Do not create feature branches
or PRs. Verify locally and obtain required review before committing directly
to `main`; push only when authorized, without force, then inspect CI results.
Before deleting a leftover branch, record its tip and prove its final content
is preserved in `main`; stop if unique final content or remote divergence appears.

## ADR validation

`adr-fmt` ships guidelines and an example template, not a default decision
corpus. This repository owns `docs/adr/`, configured by `adr-fmt.toml`.
CF-0001 through CF-0006 are Accepted retrospective records; existing governing
documentation remains authoritative. Acceptance introduces no new policy.
Do not import upstream AFM or example CHE decisions.

CF-0007 is a separate prospective Accepted decision for the explicitly
authorized opt-in two-threshold gate. It does not rewrite the six retrospective
records or alter legacy lint/rewrite exits. Policy records deliberately use
`kind`/`version`; gate diagnostics may retain legacy `record`/`v` envelopes.
Policy acceptance tests require Python 3.9+ (standard library only) and the
pinned rustc; run `cargo test --locked --test policy_gate` locally.

Install the same canonical revision used by the dedicated ADR CI job:

```sh
cargo +1.98.0 install --git https://github.com/Mattilsynet/adr-fmt --rev 30d13bf9d6ada9ac170b29ae76a7d776109f5655 --locked adr-fmt
```

From the repository root, run the local ADR checks and discovery commands:

```sh
adr-fmt --lint
adr-fmt --tree CF
adr-fmt --context comment-free
```

CI runs the same `adr-fmt --lint` command. At the pinned revision, lint
findings are advisory warnings and exit 0; they do not fail CI. Configuration
or infrastructure errors can exit 1. This is not a zero-warning enforcement
gate; inspect the diagnostics as well as the exit code.

The six-record Accepted corpus was validated on 2026-09-06 with installed
canonical `adr-fmt` 0.3.0 at revision
`83fc20bd2cbe0976388ae74193226750d4afc471`, distinct from the unchanged CI pin
above. Lint exited 0 with four L015 advisories: CF-0003 through CF-0006 list
the root first while later references include same-domain non-root records.
The former five L012 Draft-parent advisories are gone. Reference order and
parentage are preserved; these advisories do not authorize reparenting.
Investigate diagnostics rather than changing lifecycle status or suppressing
warnings merely to make lint quiet.

`--context` emits rules only from Accepted, non-stale ADRs. Validation with
that installed revision exited 0 and emitted 17 tagged rules for `comment-free`;
`--tree CF` exited 0 and listed all six Accepted records.
Use `docs/adr/TEMPLATE.md` for source-backed
records, not a duplicate index. Keep `docs/adr/stale/` present even without
retired decisions; `.gitkeep` preserves the empty directory.

## Non-Interactive Shell Execution & Bash Hygiene

Subagents run non-interactively. Any command that could trigger an interactive
y/n prompt stalls execution indefinitely.
- Use explicit non-interactive flags: `cp -f`, `rm -f`, `rm -rf`.
- Git operations: use non-interactive commands; no interactive rebase (`git rebase -i`).
- Tooling CLI options: accept batch flags (`--batch`, `-y`, `--quiet`).

