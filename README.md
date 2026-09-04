# comment-free

`comment-free` is a Rust-only hygiene tool for keeping source comments small,
structured, and intentional.

Its goal is to make coding with LLM agents more efficient by reducing stale or
misleading repository context. It removes ordinary line and block comments while
preserving the two comment-shaped signals that remain load-bearing here:
`AUTO-TRAIT-POLICY-*` markers and `// SAFETY:` lines. Doc comments are kept,
normalised to idiomatic rustdoc links, and linted when they grow too long.
Repository documentation files are reported but never rewritten. Output stays
terse, structured, and informative for automated agents.

Default mode is read-only: it walks Rust source files under `crates/` and `src/`
and reports doc comments whose prose exceeds the configured word budget. Fenced
code blocks are excluded from the count, and output is tab-separated so agents
and scripts can parse it reliably.

Rewrite mode performs two byte-preserving passes outside their target text:

1. canonicalise Rust intra-doc-link idioms in doc payloads, for example
   `[Type](Type)` to ``[`Type`]``;
2. strip ordinary non-doc `//` and `/* */` comments via the rustc lexer.

Doc comments are never deleted. `// SAFETY:` lines and
`AUTO-TRAIT-POLICY-BEGIN` / `AUTO-TRAIT-POLICY-END` marker lines are preserved.
Use `--dry-run` to inspect the unified diff before writing files.

## Install

```sh
cargo +1.98 install --git https://github.com/acje/comment-free --locked comment-free
```

The `+1.98` is load-bearing. `cargo install --git` **ignores the source
repository's `rust-toolchain.toml`** and builds with the *invoking*
toolchain; without the explicit `+1.98` the build fails on a older
default toolchain, or silently succeeds on a newer one that this
repository's CI never exercised. `--locked` is likewise mandatory: it
uses the committed `Cargo.lock`, so you get the dependency graph CI
tested.

To build from a local checkout instead:

```sh
cargo build --release --locked
```

## Usage

```sh
comment-free src
comment-free --doc-max-words 100 src
comment-free --rewrite --dry-run src
comment-free --rewrite src
```

From a checkout, `cargo run -- <args>` is equivalent.

Exit codes:

- `0`: clean
- `1`: catastrophic / unmapped IO error
- `2`: invalid CLI arguments
- `4`: doc-lint findings in default mode
- `5`: per-file parse or I/O errors during processing

`--rustdoc-link-idioms` is a deprecated alias for `--rewrite` and is retained
for one release.

## Development

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo deny check
```

`clippy::pedantic` is the standing bar (`[lints.clippy] pedantic = warn`
in `Cargo.toml`), not an elevation — new code passes it with zero
warnings. `rustfmt` runs on **stable defaults only**; do not add a
`rustfmt.toml` or `clippy.toml`. `rust-toolchain.toml` pins channel
1.98.0.

### Cargo.lock policy

`Cargo.lock` is committed deliberately. `comment-free` ships as a
binary, and a committed lockfile is the standard Rust convention for
binary crates: it pins exact transitive-dependency versions so two
clones produce byte-identical binaries given the same toolchain. It is
also what makes `cargo install --locked` reproducible.

## History

This crate was extracted from the `Mattilsynet/gh-report` monorepo,
where it lived at `crates/comment-free`. **Git history was preserved**
via `git subtree split --prefix=crates/comment-free`; the 19 commits
below the extraction commit are the crate's original monorepo history,
rewritten to be rooted at this repository's root. Commit hashes
therefore differ from the corresponding gh-report commits.
