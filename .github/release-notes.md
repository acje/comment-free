# comment-free

`comment-free` strips non-doc comments from Rust sources, normalises rustdoc
link idioms, and lints doc-comment length. It is read-only by default: linting
never writes; `--rewrite` opts in (see below).

## Assets

Each target is published as a single binary plus a checksum file named for that
target. There is no combined checksum file; every `SHA256SUMS-<target>` is
written beside its binary by the build job and self-verified there before
upload.

| Asset | Target |
|---|---|
| `comment-free-<version>-x86_64-unknown-linux-musl` | Linux x86_64, statically linked musl |
| `SHA256SUMS-x86_64-unknown-linux-musl` | SHA-256 checksum for the Linux binary |
| `comment-free-<version>-aarch64-apple-darwin` | macOS arm64 |
| `SHA256SUMS-aarch64-apple-darwin` | SHA-256 checksum for the macOS binary |

The release workflow built both binaries from the tagged source after
`scripts/verify.sh` passed. The candidate gate runs once, in the `guard` job on
Ubuntu, before either target is built; the matrix build job only compiles and
proves the artifacts.

## Verify the download

From inside the directory that holds the downloaded files:

```sh
shasum -a 256 -c SHA256SUMS-x86_64-unknown-linux-musl
shasum -a 256 -c SHA256SUMS-aarch64-apple-darwin

gh attestation verify comment-free-<version>-x86_64-unknown-linux-musl --repo acje/comment-free
gh attestation verify comment-free-<version>-aarch64-apple-darwin --repo acje/comment-free
```

The build provenance attestation covers the two binaries only. Each per-target
`SHA256SUMS-<target>` is a checksum over that same binary and is not attested.

## Install from source

Installing from the git source requires an explicit toolchain, because
`cargo install --git` ignores the source repository's `rust-toolchain.toml` and
builds with the invoking toolchain:

```sh
cargo +1.100 install --git https://github.com/acje/comment-free --rev <version> --locked comment-free
```

`--rev <version>` pins the tagged source rather than the moving branch head, and
`--locked` uses the committed `Cargo.lock`.

MSRV is **1.99**; the pinned toolchain and CI compiler is **1.100**.

## Exit contract

Default lint mode:

| Exit | Meaning |
|---|---|
| `0` | clean — every doc payload under ROOT was read and decided against the budget |
| `1` | catastrophic / unmapped IO error, or counter overflow |
| `2` | invalid CLI arguments |
| `4` | not clean — at least one doc-lint finding, or at least one undecided item |
| `5` | per-file parse or I/O error during processing, or a write conflict |

`--rewrite`:

| Exit | Meaning |
|---|---|
| `0` | wrote the rewritten file(s), or the tree was already unchanged |
| `3` | `--rewrite --dry-run` previewed at least one pending change |
| `5` | a per-file processing error or write conflict |

Opt-in two-threshold policy gate (`--check-doc-budget`):

| Exit | Meaning |
|---|---|
| `0` | bounded pass, including advisory findings |
| `1` | decided enforced breach |
| `2` | unknown/error — unknown dominates a breach, and absent output is never a pass |

## Machine-readable records

All summaries are JSON Lines. Record versions:

| Family | Version |
|---|---|
| `doc_lint_*` | `3` |
| run diagnostics (`run_error`, `doc_file_warning`, `rewrite_file`, `strip_summary`, `lint_summary`) | `3` |
| `rewrite_summary` | `2` |
| opt-in policy records (`kind`/`version`) | `2` |

The schema and field contract are in
[`docs/record-format.md`](https://github.com/acje/comment-free/blob/<version>/docs/record-format.md).
A consumer rejects a record whose version exceeds the one it understands.

## Writing files

`comment-free` reads and lints only unless `--rewrite` is passed; `--rewrite
--dry-run` previews the diff and writes nothing. Rewrites are atomic
replacements with deliberate limits (symlinks, hard links, concurrent writers,
crash durability, temporary-file cleanup) documented in the README section
"How `--rewrite` writes, and what it does not promise".
