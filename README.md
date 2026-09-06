# comment-free

`comment-free` is a Rust-only hygiene tool for keeping source comments small,
structured, and intentional.

Its goal is to make coding with LLM agents more efficient by reducing stale or
misleading repository context. It removes every non-doc line and block comment,
with no carve-outs. Doc comments are kept, normalised to idiomatic rustdoc
links, and linted when they grow too long. Repository documentation files are
reported but never rewritten. Output stays terse, structured, and informative
for automated agents.

Default mode is read-only: it walks Rust source files under the locations
cargo compiles as crate source — `benches/`, `crates/`, `examples/`, `src/`,
`tests/`, and a root `build.rs` —
and reports doc comments whose prose exceeds the configured word budget. Code is
excluded from the count: fenced blocks, indented code blocks, inline
code spans, and reference definitions. Findings, run diagnostics, errors and
summaries are all emitted as JSON Lines so agents and scripts can parse them
reliably even when a path or item label contains a tab or a newline. The one
exception is the `--dry-run` unified diff body, which is human-facing plain
text and is never machine-parsed; filtering output to lines starting with `{`
yields exactly the records. See [docs/record-format.md](docs/record-format.md)
for the record grammar and its compatibility rules.

Rewrite mode performs two byte-preserving passes outside their target text:

1. canonicalise Rust intra-doc-link idioms in doc payloads, for example
   `[Type](Type)` to ``[`Type`]``;
2. strip every non-doc `//` and `/* */` comment via the rustc lexer.

Doc comments are never deleted; nothing else is preserved. There is no
`// SAFETY:` exception and no marker allowlist — a comment is a doc comment or
it is removed. Use `--dry-run` to inspect the unified diff before writing
files.

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

- `0`: clean — every doc payload under ROOT was read, and every one of them
  was decided against the budget
- `1`: catastrophic / unmapped IO error
- `2`: invalid CLI arguments
- `3`: `--rewrite --dry-run` previewed at least one pending change — the
  tree is not already comment-free
- `4`: the tree did not come back clean in default mode — at least one
  doc-lint finding, or at least one undecided item
- `5`: per-file parse or I/O errors during processing, or a write conflict

Exit `3` is what makes `--rewrite --dry-run` usable as a check: it
modifies nothing and reports "already clean" in the exit code rather
than only in the `rewritten` field of `strip_summary`. Write mode is not
a check — it was asked to change the tree — and exits `0` whatever it
rewrote. Exit `5` outranks exit `3`, because a tree that could not be
fully read is a stronger signal than a pending change.

Exit `4` covers findings and indeterminates alike, because exit `0` means
*clean* and a run that could not read everything has not shown the tree to
be clean. The two are still distinguishable: read `findings` and
`undecided` in the `lint_summary` record, and the `outcome` field of each
`doc_lint_*` record.

### Known limitation: doc payloads this tool cannot read

A doc payload written as a macro call rather than a string literal
resolves only by macro expansion, which this tool does not perform:

```rust
#[doc = include_str!("overview.md")]
#[doc = concat!(" generated ", stringify!(prose))]
#[cfg_attr(all(), doc = concat!(" more"))]
```

Such an item is reported as a `doc_lint_undecided` record with `outcome`
`unreadable_doc_payload`, carrying no word count — no reading produced
one — and it never counts as clean. This is a reported gap, not a silent
one; until it is closed, a crate using these idioms cannot reach exit `0`
in default lint mode.

### Known limitation: doc attributes inside macro bodies

The same holds for a doc attribute inside a macro token body, whether a
`macro_rules!` definition or the tokens passed to an invocation:

```rust
macro_rules! noisy {
    () => {
        /// prose that the expansion attaches to a generated item
        pub fn inner() {}
    };
}
```

This tool does not expand macros, so it does not know which item the
prose documents, or how many times. The body is reported as
`outcome` `uninspected_macro_body`, naming the file and the macro, and
does not count as clean. The report is made once, at the outermost
opaque body.

A macro body carrying **no** doc attribute is not reported: the check is
for an attribute group containing `doc =`, so ordinary `println!` and
`vec!` bodies stay clean, and `#[doc(hidden)]` — rustdoc metadata, not
prose — does not trigger it either.

Doc text synthesised by a procedural macro from tokens that never spell
`doc` remains out of reach of a purely syntactic tool, and is not
detected at all.

`--rustdoc-link-idioms` is a deprecated alias for `--rewrite` and is retained
for one release.

## How `--rewrite` writes, and what it does not promise

A rewrite never truncates a source file in place. The new content is
written to a sibling temporary file in the same directory, given the
destination's permissions, flushed and `sync_all`'d, and only then
renamed over the destination. Immediately before the rename the tool
re-reads the destination and aborts if it no longer holds the bytes that
were originally read; the abort is reported as a
`{"record":"run_error","kind":"conflict",...}` record on stderr and exits
`5`, leaving the destination byte-identical.

Deliberate limits of that scheme:

- **Supported platforms.** Unix only. CI exercises `ubuntu-latest` and
  `macos-latest`. The atomic-replacement and permission-preservation
  behaviour described here is not claimed for Windows, whose
  `rename`-over-an-existing-file semantics differ and are untested.
- **Symbolic links.** A destination that is itself a symlink is refused
  with an I/O error rather than rewritten: renaming over it would
  replace the link with a regular file and leave the real target
  unchanged. Directory traversal does not follow links, so this only
  affects direct library calls.
- **Hard links.** Rename replaces one pathname's inode. A rewritten file
  with other hard-link aliases becomes a new inode for the walked path
  while the other aliases keep the old contents. Files that must be
  observed through all their aliases should not be rewritten by this
  tool.
- **Concurrent writers.** The re-read/rename pair is a check followed by
  an action, not an atomic compare-and-swap. It narrows, but cannot
  close, the window in which another writer's change is lost. There is
  no locking.
- **Crash durability.** The replacement bytes are synced before the
  rename, so no partial file can be observed. The parent directory is
  not synced, so the rename itself is not guaranteed to survive a power
  loss.
- **Temporary-file cleanup.** The temporary file is removed on every
  returning error path, and on a best-effort basis while a panic
  unwinds. An abort, a signal, or a failing removal can still leave a
  `.<name>.<pid>.<n>.comment-free-tmp` file behind.

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

### Architecture decisions

`adr-fmt` supplies governance guidelines and an example template, not a
shipped default decision corpus. Our repository-owned Draft corpus lives
in [docs/adr/](docs/adr/), discovered through `adr-fmt.toml`. Its root,
[CF-0001](docs/adr/cf/CF-0001-bootstrap-adr-governance.md), remains **Draft**;
its focused child records also remain Draft. They record existing contracts,
not new policy or replacements for existing governing documentation.
Upstream AFM decisions and the example CHE corpus are not imported.

Install the canonical revision pinned in CI, with an explicit invoking
toolchain because `cargo install --git` ignores the source toolchain pin:

```sh
cargo +1.98.0 install --git https://github.com/Mattilsynet/adr-fmt --rev 30d13bf9d6ada9ac170b29ae76a7d776109f5655 --locked adr-fmt
```

From the repository root:

```sh
adr-fmt --lint
adr-fmt --tree CF
adr-fmt --context comment-free
```

The dedicated ADR CI job runs the same `adr-fmt --lint` command. At the
pinned revision, lint findings are advisory warnings and exit 0; they do
not fail CI. Configuration or infrastructure errors can exit 1. This is
not a zero-warning enforcement gate: inspect the diagnostics too.

`--context` emits rules only from Accepted, non-stale ADRs. This all-Draft
corpus therefore produces introductory text but no rules, with exit 0.
`--tree CF` lists the connected Draft corpus without a hand-maintained index.
Use [the local template](docs/adr/TEMPLATE.md) for source-backed additions.
The `docs/adr/stale/.gitkeep`
placeholder keeps the required retirement directory present while there
are no retired decisions.

### Cargo.lock policy

`Cargo.lock` is committed deliberately. `comment-free` ships as a
binary, and a committed lockfile is the standard Rust convention for
binary crates: it pins the exact dependency graph exercised by CI.
`cargo install --locked` uses that graph rather than resolving fresh versions;
locked resolution alone does not guarantee byte-identical binaries.

## History

This crate was extracted from the `Mattilsynet/gh-report` monorepo,
where it lived at `crates/comment-free`. **Git history was preserved**
via `git subtree split --prefix=crates/comment-free`; the 19 commits
below the extraction commit are the crate's original monorepo history,
rewritten to be rooted at this repository's root. Commit hashes
therefore differ from the corresponding gh-report commits.
