#!/usr/bin/env bash
# find-lint-deviations.sh — enumerate explicit lint-deviation sites.
#
# The fleet's Rust intent: "-D warnings, clippy::pedantic as the standing bar,
# no blanket suppressions." Each of these is an explicit deviation from that
# intent, and a reviewer or agent must be able to find and judge every one:
#   #[allow(L)] / #[expect(L)]                 outer, item-level
#   #![allow(L)] / #![expect(L)]               inner, module-wide (the razor)
#   #[cfg_attr(C, allow/expect(L))]            conditional distortion
# (and the inner forms of each).
#
# Multiline-aware (rg -U --json): a single-line regex cannot see
#   #![allow(
#       dead_code
#   )]
# which is exactly the false-clean that motivated this tool (cf. comment-free-5uk).
#
# Usage: find-lint-deviations.sh [PATH]        (default: '.')
# Output (one line per site, sorted by scope+kind):
#   file:line  SCOPE  KIND  lint(s)  [reason]  <flattened snippet>
set -euo pipefail

target="${1:-.}"

# Capture the WHOLE attribute through its closing ']' so the JSON submatch is the
# complete `#[allow(...)]` block. Two shapes:
#   1) direct allow/expect (outer or inner)
#   2) cfg_attr wrapping an allow/expect
# We deliberately exclude #[cfg_attr(C, doc="...")], #[cfg_attr(C, non_exhaustive)]
# and feature gates — those are semantic cfg_attr, not lint suppressions.
head_re='#!?\[[ \t]*(allow|expect)\([^]]*\]|#!?\[[ \t]*cfg_attr\([^]]*,(allow|expect)\([^]]*\]'

rg -U --json --color never --glob '*.rs' "$head_re" "$target" \
| python3 -c '
import sys, json
records = []

def parse(text):
    m = re_search_allow_expect(text)
    if not m:
        return None
    qual = m["qual"]
    is_cfg = bool(re_search(r"#!?\[\s*cfg_attr\b", text))
    is_inner = text.lstrip().startswith("#![")
    o = text.find("(", m["pos"])
    c = text.rfind(")")            # its own closing paren
    args = text[o + 1:c] if c != -1 and c > o else ""
    # Slice off any `reason = "..."` tail first; a reason string may contain
    # commas and even a paren, so never split the raw arg text on ",".
    ri = args.find("reason")
    args_core = args[:ri] if ri != -1 else args
    toks = [t.strip() for t in args_core.split(",") if t.strip()]
    return {
        "is_cfg": is_cfg, "qual": qual, "is_inner": is_inner,
        "lints": ",".join(toks),
        "reason": bool(re_search(r"\breason\b", text)),
        "dead_code": bool(re_search(r"\bdead_code\b", text)),
    }

import re as _re
def re_search(pat, text): return _re.search(pat, text, _re.S)
def re_search_allow_expect(text):
    m = _re.search(r"\b(allow|expect)\s*\(", text, _re.S)
    if not m: return None
    return {"qual": m.group(1), "pos": m.start()}

for ln in sys.stdin:
    ln = ln.strip()
    if not ln: continue
    o = json.loads(ln)
    if o.get("type") != "match": continue
    data = o["data"]
    path = data["path"]["text"]
    line = data["line_number"]
    sms = data.get("submatches") or []
    if not sms: continue
    text = sms[0]["match"]["text"]
    p = parse(text)
    if not p: continue
    scope = "INNER-MODULE" if p["is_inner"] else "OUTER-ITEM"
    kind = ("cfg_attr-" + p["qual"]) if p["is_cfg"] else p["qual"]
    flat = _re.sub(r"\s+", " ", text).strip()
    records.append((path, line, scope, kind, p["lints"] or "?", "reason" if p["reason"] else "", flat))

# de-dup exact same (path,line,scope,kind) — json can emit repeats on overlap
seen = set()
for r in records:
    key = (r[0], r[1], r[2], r[3])
    if key in seen: continue
    seen.add(key)
    print("%s:%d  %-12s %-14s %-30s %s  %s" % (
        r[0], r[1], r[2], r[3], r[4], r[5], r[6][:104]))
'
