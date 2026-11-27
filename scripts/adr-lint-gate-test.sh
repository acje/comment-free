#!/usr/bin/env bash
#
# Executable contract tests for scripts/adr-lint-gate.sh.
#
# The gate is enforcement tooling: a false-clean here disables
# AFM-0003:R3 while CI stays green. These tests pin the verdict
# contract (0 = at/under, 1 = over, 2 = no verdict) against a stub
# lint producer injected via ADR_LINT_CMD, so parsing and exit codes
# are exercised without a cargo build.
#
# Usage: scripts/adr-lint-gate-test.sh

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
gate="$repo_root/scripts/adr-lint-gate.sh"

failures=0
cases=0

expect() {
    local name="$1" want="$2" cmd="$3" mode="$4" threshold="${5-}"
    local got=0
    cases=$((cases + 1))
    case "$mode" in
        unset)
            ADR_LINT_CMD="$cmd" bash "$gate" >/dev/null 2>&1 || got=$?
            ;;
        set)
            ADR_LINT_CMD="$cmd" ADR_LINT_MAX_WARNINGS="$threshold" bash "$gate" >/dev/null 2>&1 || got=$?
            ;;
        *)
            printf 'FAIL %s: unknown mode %s\n' "$name" "$mode"
            failures=$((failures + 1))
            return
            ;;
    esac
    if [ "$got" -eq "$want" ]; then
        printf 'ok   %s (exit %s)\n' "$name" "$got"
    else
        printf 'FAIL %s: want exit %s, got %s\n' "$name" "$want" "$got"
        failures=$((failures + 1))
    fi
}

clean_output='## Diagnostics: 8 warning(s) across 32 ADR(s)'
over_output='## Diagnostics: 9 warning(s) across 32 ADR(s)'
no_header_output='some other output entirely'
huge='999999999999999999999999999999999999'
huge_output="## Diagnostics: $huge warning(s) across 32 ADR(s)"

expect "at threshold is clean"        0 "printf '%s\n' '$clean_output'"     set   "8"
expect "under threshold is clean"     0 "printf '%s\n' '$clean_output'"     set   "9"
expect "over threshold fails"         1 "printf '%s\n' '$over_output'"      set   "8"
expect "default threshold applies"    0 "printf '%s\n' '$clean_output'"     unset
expect "missing header is no verdict" 2 "printf '%s\n' '$no_header_output'" set   "8"
expect "producer failure no verdict"  2 "exit 3"                            set   "8"
expect "malformed threshold"          2 "printf '%s\n' '$clean_output'"     set   "bogus"
expect "negative threshold"           2 "printf '%s\n' '$clean_output'"     set   "-1"
expect "whitespace threshold"         2 "printf '%s\n' '$clean_output'"     set   " "
expect "float threshold"              2 "printf '%s\n' '$clean_output'"     set   "8.5"
expect "explicitly empty threshold"   2 "printf '%s\n' '$clean_output'"     set   ""
expect "plus-signed threshold"        2 "printf '%s\n' '$clean_output'"     set   "+8"
expect "oversized threshold"          2 "printf '%s\n' '$clean_output'"     set   "$huge"
expect "max in-range threshold"       0 "printf '%s\n' '$clean_output'"     set   "999999999"
expect "oversized parsed count"       2 "printf '%s\n' '$huge_output'"      set   "8"
expect "two headers clean then over"  2 "printf '%s\n%s\n' '$clean_output' '$over_output'" set "8"
expect "two headers over then clean"  2 "printf '%s\n%s\n' '$over_output' '$clean_output'" set "8"
expect "incomplete validation is no verdict" 2 "printf '%s\n%s\n' '## Diagnostics: 1 warning(s) across 2 ADR(s)' 'Validation incomplete: duplicate ADR IDs prevent rule checks; totals cover parser and duplicate-ID findings only.'" set "8"

printf '\nadr-lint-gate-test: %s case(s), %s failure(s)\n' "$cases" "$failures"
[ "$failures" -eq 0 ]
