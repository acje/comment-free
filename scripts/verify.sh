#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

echo "==> Running unsafe-code gate check (forbid-unsafe-total / RST-0005)..."
# All crate roots must enforce #![forbid(unsafe_code)]
grep -q 'forbid(unsafe_code)' "$ROOT/src/lib.rs" || {
  echo "::error::forbid-unsafe-total: src/lib.rs lacks #![forbid(unsafe_code)] (RST-0005)" >&2
  exit 1
}
grep -q 'forbid(unsafe_code)' "$ROOT/src/main.rs" || {
  echo "::error::forbid-unsafe-total: src/main.rs lacks #![forbid(unsafe_code)] (RST-0005)" >&2
  exit 1
}

echo "==> Running closed-error gate check (tests/closed_error_policy.rs / RST-0006)..."
cargo test --locked --test closed_error_policy

echo "==> Running cargo test..."
cargo test --locked

echo "==> Running cargo clippy..."
cargo clippy --all-targets --locked -- -D warnings

echo "==> Running cargo fmt check..."
cargo fmt --all -- --check

echo "==> Running native bounded-source doc-budget dogfood gate..."
cargo run --quiet --locked -- --check-doc-budget --doc-advisory-words 80 --doc-max-words 120 --max-warning-files 0 .

echo "==> Running read-only rewrite dogfood preview..."
cargo run --quiet --locked -- --rewrite --dry-run .

echo "==> All comment-free verification checks passed."
