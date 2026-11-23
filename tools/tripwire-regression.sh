#!/usr/bin/env sh
set -eu

# tripwire-regression: Four-step guard proof harness proving gates bite
# 1. plant a compiling violation -> 2. observe failure -> 3. revert -> 4. observe clean

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

LIB="$ROOT/src/lib.rs"
BACKUP=$(mktemp "${TMPDIR:-/tmp}/comment-free-lib.XXXXXX")
cp -f "$LIB" "$BACKUP"

restore_lib() {
  if [ -f "$BACKUP" ]; then
    cp -f "$BACKUP" "$LIB"
  fi
}
trap 'restore_lib' 0 1 2 15

expect_status() {
  expected=$1
  shift
  label=$1
  shift
  set +e
  "$@"
  actual=$?
  set -e
  echo "==> [tripwire-regression] $label exited $actual (expected $expected)"
  if [ "$actual" -ne "$expected" ]; then
    echo "::error::tripwire-regression: $label exited $actual (expected $expected)" >&2
    exit 1
  fi
}

echo "==> [tripwire-regression] Testing the closed-error gate bite via derive and manual Error routes..."

cat >> "$LIB" <<'PLANT'

#[derive(Debug)]
#[non_exhaustive]
pub enum PlantedTestEnum {
    Planted,
}
impl std::fmt::Display for PlantedTestEnum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("planted")
    }
}
impl std::error::Error for PlantedTestEnum {}

#[derive(Debug, thiserror::Error)]
#[error("planted derive")]
#[non_exhaustive]
pub enum PlantedDeriveError {
    Planted,
}
PLANT

expect_status 0 "plant builds (cargo build --all-targets --locked)" cargo build --all-targets --locked

echo "==> [tripwire-regression] verify.sh with plant"
set +e
output=$(sh scripts/verify.sh 2>&1)
status=$?
set -e
echo "==> [tripwire-regression] verify.sh with plant exited $status (expected non-zero)"
if [ "$status" -eq 0 ]; then
  echo "::error::tripwire-regression: gate failed to bite on planted closed-error violations" >&2
  exit 1
fi
observed=$(printf '%s\n' "$output" | grep 'closed-error violations:' | head -n 1)
if [ -z "$observed" ]; then
  printf '%s\n' "$output" >&2
  echo "::error::tripwire-regression: closed-error gate reported no violation line" >&2
  exit 1
fi
echo "==> [tripwire-regression] observed: $observed"
if ! printf '%s' "$observed" | grep -q 'PlantedTestEnum'; then
  echo "::error::tripwire-regression: planted manual-impl enum was not reported" >&2
  exit 1
fi
if ! printf '%s' "$observed" | grep -q 'PlantedDeriveError'; then
  echo "::error::tripwire-regression: planted derive enum was not reported" >&2
  exit 1
fi

restore_lib

expect_status 0 "verify.sh after revert" sh scripts/verify.sh

if ! cmp -s "$BACKUP" "$LIB"; then
  echo "::error::tripwire-regression: src/lib.rs was not restored byte-for-byte" >&2
  exit 1
fi
rm -f "$BACKUP"

echo "OK: tripwire-regression all guard proofs verified clean."
