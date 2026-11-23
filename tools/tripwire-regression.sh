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

echo "==> [tripwire-regression] Testing the closed-error gate bite via a manual Error impl..."

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
PLANT

expect_status 0 "plant builds (cargo build --all-targets --locked)" cargo build --all-targets --locked
expect_status 101 "verify.sh with plant" sh scripts/verify.sh

restore_lib

expect_status 0 "verify.sh after revert" sh scripts/verify.sh

if ! cmp -s "$BACKUP" "$LIB"; then
  echo "::error::tripwire-regression: src/lib.rs was not restored byte-for-byte" >&2
  exit 1
fi
rm -f "$BACKUP"

echo "OK: tripwire-regression all guard proofs verified clean."
