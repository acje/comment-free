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
rule "no non-doc comments in Rust source". It preserves doc comments,
`// SAFETY:` lines, and `AUTO-TRAIT-POLICY-*` markers.

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

`main` is currently unprotected. Prefer a branch + PR so CI gates the
change before it lands.
