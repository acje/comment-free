#!/usr/bin/env sh
set -eu

# tripwire-regression: Four-step guard proof harness proving gates bite
# 1. plant violation -> 2. observe failure -> 3. revert -> 4. observe clean

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

echo "==> [tripwire-regression] Testing non-exhaustive-check gate bite..."
# 1. Plant violation: add #[derive(thiserror::Error)] #[non_exhaustive] pub enum PlantedTestEnum to src/
PLANTED_SRC="$ROOT/src/planted_test_enum.rs"
printf '#[derive(thiserror::Error, Debug)]\n#[error("planted")]\n#[non_exhaustive]\npub enum PlantedTestEnum {}\n' > "$PLANTED_SRC"

# 2. Observe failure
if sh scripts/verify.sh >/dev/null 2>&1; then
  rm -f "$PLANTED_SRC"
  echo "::error::tripwire-regression: gate failed to bite on planted #[non_exhaustive]" >&2
  exit 1
fi

# 3. Revert
rm -f "$PLANTED_SRC"

# 4. Observe clean
sh scripts/verify.sh >/dev/null 2>&1 || {
  echo "::error::tripwire-regression: gate failed to pass after revert" >&2
  exit 1
}

echo "OK: tripwire-regression all guard proofs verified clean."
