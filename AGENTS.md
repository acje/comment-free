# AGENTS.md — comment-free

Repo-specific operational notes. General agent/OODA doctrine, bash
hygiene, and the Rust no-`//`-comments house style live in the global
`~/.config/opencode/AGENTS.md` — not repeated here.

## What this repo is

A single-crate Rust binary (+ library `comment_free`): a Rust source
comment-hygiene tool. Default mode is read-only and lints doc-comment
length; `--rewrite` strips non-doc comments and canonicalises rustdoc
link idioms in place. See `README.md` for modes and exit codes.

This tool is the mechanical enforcement surface for the fleet house
rule "no non-doc comments in Rust source". It preserves doc comments and
nothing else: there is no `// SAFETY:` carve-out and no marker allowlist.

Machine-readable lint and rewrite records are JSON Lines; the grammar and
its compatibility rules live in `docs/record-format.md`.

## Build / test / lint

```sh
cargo build --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo deny check
cargo audit
```

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
CF-0001 and its child records are Draft, not accepted policy; existing governing documentation
remains authoritative. Do not import upstream AFM or example CHE decisions.

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

The current six-record Draft corpus produces exactly five L012 advisories:
CF-0002 through CF-0006 reference Draft parent CF-0001. These are expected;
investigate other diagnostics rather than accepting records or suppressing
warnings merely to make lint quiet.

`--context` emits rules only from Accepted, non-stale ADRs. With this
all-Draft corpus it exits 0 with introductory text but no rules; `--tree CF`
lists the connected Draft corpus. Use `docs/adr/TEMPLATE.md` for source-backed
records, not a duplicate index. Keep `docs/adr/stale/` present even without
retired decisions; `.gitkeep` preserves the empty directory.
