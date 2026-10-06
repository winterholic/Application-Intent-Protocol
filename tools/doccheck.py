#!/usr/bin/env python3
"""Checks ```aip example blocks in docs against canonical rules of spec/grammar.md.

This is a stopgap until the real parser exists (M1). Each rule encodes one
canonical-form decision; a hit means either the example or the spec is wrong.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = [ROOT / "docs/design", ROOT / "spec"]

RULES = [
    ("EQ2", r"==", "equality is '=' (grammar §0.3)"),
    ("EXISTS_CALL", r"\bexists\s*\(", "exists takes a set-expr without parens (§0.5)"),
    ("LAMBDA", r"=>", "use set-expr binder 'all(xs x: p)' instead of lambdas (§0.4)"),
    ("DURATION_WORD", r"\b\d+\s+(seconds?|minutes?|hours?|days?|weeks?|months?|years?)\b", "durations are '7d', '6mo' (§0.6)"),
    ("COUNT_STAR", r"count\(\*\)", "use count() in group-by context"),
    ("OLD_POLICY", r"^\s*policy\s*:", "spike-0 syntax; use 'allow'"),
    ("OLD_TX", r"^\s*transaction\s*\{", "spike-0 syntax; transaction is implicit"),
    ("OLD_LOAD", r"^\s*load\s+\w+\s*:", "spike-0 syntax; entity params are loaded"),
    ("TZ_BARE", r"every\s+\w+.*\bat\s+\d\d:\d\d\s+[A-Z][a-z]+/", "timezone needs 'tz \"Area/City\"'"),
    ("WEEKDAY_UPPER", r"every\s+week\s+on\s+[A-Z]{3}\b", "weekday is lower-case: mon, tue ..."),
    ("TIMERANGE", r"\bTimeRange\b", "use Range<Time>"),
    ("CURSOR_PARAM", r":\s*Cursor\b", "cursor is implicit from the page clause"),
    ("SAME_NO_BINDER", r"\bsame\((?![^)]*\w+\s+\w+\s*:)", "same(<set-expr> x: <expr>)"),
    ("LATEST_NO_BY", r"\blatest\b(?![^)\n]*\bby\b)", "latest <set-expr> by <expr> (§8)"),
]

def blocks(text):
    for m in re.finditer(r"```aip\n(.*?)```", text, re.S):
        start = text[: m.start()].count("\n") + 2
        yield start, m.group(1)

def main():
    hits = 0
    total = 0
    for d in DOCS:
        for f in sorted(d.rglob("*.md")):
            text = f.read_text()
            for start, body in blocks(text):
                total += 1
                for i, line in enumerate(body.splitlines()):
                    code = line.split("//")[0]
                    for rid, pat, why in RULES:
                        if re.search(pat, code):
                            hits += 1
                            print(f"{f.relative_to(ROOT)}:{start + i}: [{rid}] {why}\n    {line.strip()}")
                    # a query whose entity param is re-bound by 'from Entity param'
    # param re-binding check (whole-block)
    for d in DOCS:
        for f in sorted(d.rglob("*.md")):
            for start, body in blocks(f.read_text()):
                for q in re.finditer(r"query\s+\w+\(([^)]*)\)[^{]*\{(.*?)\n\}", body, re.S):
                    params = dict(re.findall(r"(\w+)\s*:\s*([A-Z]\w*)", q.group(1)))
                    for ent, alias in re.findall(r"from\s+([A-Z]\w*)\s+(\w+)", q.group(2)):
                        if params.get(alias) == ent:
                            hits += 1
                            print(f"{f.relative_to(ROOT)}:{start}: [PARAM_REBIND] 'from {ent} {alias}' re-binds entity param '{alias}'; write 'from {alias}'")
    # every intent with a real body must declare allow (E-ALLOW-MISSING)
    for d in DOCS:
        for f in sorted(d.rglob("*.md")):
            for start, body in blocks(f.read_text()):
                for m in re.finditer(r"^(?:internal\s+)?(command|query|subscribe|job)\s+(\w+)[^\n]*\{(.*?)^\}", body, re.S | re.M):
                    inner = m.group(3).strip()
                    if inner in ("...", "") or inner.startswith("..."):
                        continue
                    if not re.search(r"^\s*allow\b", inner, re.M):
                        hits += 1
                        print(f"{f.relative_to(ROOT)}:{start}: [ALLOW_MISSING] {m.group(1)} {m.group(2)} has no allow clause")
    print(f"\n{total} aip blocks checked, {hits} finding(s)")
    return 1 if hits else 0

if __name__ == "__main__":
    sys.exit(main())
