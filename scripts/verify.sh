#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

echo "==> Running closed-error gate check (non-exhaustive-check / RST-0006)..."
# Public error enums must not carry #[non_exhaustive] (C4.5/C4.6)
if command -v non-exhaustive-check >/dev/null 2>&1; then
  non-exhaustive-check "$ROOT/src"
elif [ -f "$ROOT/../tripwires/Cargo.toml" ]; then
  cargo run --manifest-path "$ROOT/../tripwires/Cargo.toml" --locked -p non-exhaustive-check -- "$ROOT/src"
else
  echo "::error::non-exhaustive-check: missing gate executable" >&2
  exit 1
fi

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
