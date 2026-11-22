#!/usr/bin/env python3
"""Compare what a definition declares for each extension (`access`) with what the extension code calls on `ctx`.

Usage (from the repository root, after `cargo build --manifest-path spikes/spike-v1-fixture/Cargo.toml`):
    python3 plan-docs/stabilization-20261122/e2/scan_extensions.py [--bin PATH] [--json] [DEFINITION ...]

With no definition it takes every `.aip` under the repository that has an `extension`, `operation` or job block, except the
legacy copy under porting/infisical/tests/legacy. The scan is textual: a call with a computed resource name is reported as
`dynamic`, and code outside the named function is not attributed to it. It is evidence for the table in `e2-extension-access.md`,
not a verifier. A read extension that never states `read.<Resource>` is expected to show `undeclared read`: that is the R1 gap.
"""
import json, os, re, subprocess, sys

ROOT = os.getcwd()
SKIP = ("node_modules", "/target/", "/.git/", "porting/infisical/tests/legacy")
CALL = re.compile(r"ctx\.data\.(read|aggregate|apply|create)\(\s*(?:[\"']([A-Za-z_][\w.]*)[\"'](?:\s*,\s*[\"']([A-Za-z_]\w*)[\"'])?)?")
READ_NAME = re.compile(r"[\"']?read[\"']?\s*[:=]\s*[\"']([A-Za-z_]\w*)[\"']")
MONGO = re.compile(r"ctx\.mongo\.(\w+)\(\s*[\"']([\w.:*-]+)[\"']")
REDIS = re.compile(r"ctx\.redis\.(\w+)\(\s*[\"']([\w.:*-]+)[\"']")


def find_definitions():
    out = []
    for base, dirs, files in os.walk(ROOT):
        if any(s in base + "/" for s in SKIP):
            dirs[:] = []
            continue
        for f in files:
            p = os.path.join(base, f)
            if f.endswith(".aip") and not any(s in p for s in SKIP):
                text = open(p, encoding="utf-8", errors="replace").read()
                if re.search(r"^\s*(extension|operation|job|on)\b", text, re.M):
                    out.append(p)
    return sorted(out)


def facts(binary, path):
    r = subprocess.run([binary, "facts", path], capture_output=True, text=True)
    if r.returncode != 0:
        return None
    return json.loads(r.stdout)["execution"]


def walk(node, trail=()):
    if isinstance(node, dict):
        if isinstance(node.get("access"), dict) and "implementation" in node:
            yield trail, node
        for k, v in node.items():
            yield from walk(v, trail + (k,))


def locate(defdir, module):
    d = defdir
    for _ in range(4):
        for base, dirs, files in os.walk(d):
            if any(s in base + "/" for s in ("node_modules", "/target/", "/.git/")):
                dirs[:] = []
                continue
            for ext in (".py", ".mjs", ".js"):
                if module + ext in files:
                    return os.path.join(base, module + ext)
        d = os.path.dirname(d)
        if len(d) < len(ROOT):
            break
    return None


def body_of(text, path, func):
    if path.endswith(".py"):
        pat = re.compile(r"^(?:async\s+)?def\s+%s\b.*?(?=^(?:async\s+)?def\s|\Z)" % re.escape(func), re.M | re.S)
    else:
        pat = re.compile(r"^export\s+(?:async\s+)?function\s+%s\b.*?(?=^export\s|\Z)" % re.escape(func), re.M | re.S)
    m = pat.search(text)
    return m.group(0) if m else None


def observed(body):
    seen = {"read": set(), "aggregate": set(), "apply": set(), "create": set(), "mongo": set(), "redis": set(), "dynamic": 0}
    for m in CALL.finditer(body):
        op, a, b = m.groups()
        if op == "read":
            seen["read"].add(a or "")
        elif op == "aggregate":
            (seen["aggregate"].add(f"{a}.{b}") if a and b else seen.__setitem__("dynamic", seen["dynamic"] + 1))
        elif op == "apply":
            (seen["apply"].add(f"{a}.{b}") if a and b else seen.__setitem__("dynamic", seen["dynamic"] + 1))
        else:
            seen["create"].add(a or "")
    # the resource of a read lives in the query: `{"read": "Video", ...}`
    names = set(READ_NAME.findall(body))
    seen["read"] = {n for n in seen["read"] if n} | names
    if any(m.group(1) == "read" and not m.group(2) for m in CALL.finditer(body)) and not names:
        seen["dynamic"] += 1
    seen["mongo"] = {m.group(2) for m in MONGO.finditer(body)}
    seen["redis"] = {m.group(2) for m in REDIS.finditer(body)}
    return seen


def declared(access):
    d = {"read": set(), "aggregate": set(), "apply": set(), "create": set(), "mongo": set(), "redis": set()}
    for key, val in access.items():
        head, _, rest = key.partition(".")
        if head in ("events", "files", "http", "messages", "jobs"):
            continue  # a server capability (outbound call, queue, file, job), not data access through ctx.data
        if head == "read":
            d["read"].add(rest)
        elif head == "create":
            d["create"].add(rest)
        elif head == "mongo":
            d["mongo"].add(rest)
        elif head == "redis":
            d["redis"].add(rest)
        elif head == "apply":
            d["apply"].add(rest)
        elif val == "transition":
            d["apply"].add(key)
        elif isinstance(val, dict):
            d["aggregate"].add(key)
        else:
            d["apply"].add(key)
    return d


def glob_match(grant, name):
    return grant == name or (grant.endswith("*") and name.startswith(grant[:-1]))


def compare(decl, seen, kind):
    notes = []
    for op in ("read", "aggregate", "apply", "create"):
        for name in sorted(seen[op] - decl[op]):
            if op == "read" and kind == "read":
                notes.append(f"undeclared read {name} (read extensions have no declaration for reads)")
            elif op == "aggregate" and any(a == name for a in decl["aggregate"]) is False:
                notes.append(f"undeclared {op} {name}")
            elif op != "read" or kind != "read":
                notes.append(f"undeclared {op} {name}")
        if op != "read" or kind != "read":
            for name in sorted(decl[op] - seen[op]):
                if op == "read" and seen["dynamic"]:
                    continue
                notes.append(f"declared {op} {name} not seen")
    for op in ("mongo", "redis"):
        for name in sorted(n for n in seen[op] if not any(glob_match(g, n) for g in decl[op])):
            notes.append(f"undeclared {op} {name}")
        for g in sorted(g for g in decl[op] if not any(glob_match(g, n) for n in seen[op])):
            notes.append(f"declared {op} {g} not seen")
    if seen["dynamic"]:
        notes.append(f"{seen['dynamic']} call(s) with a computed name")
    return notes


def main():
    args = sys.argv[1:]
    binary = "/tmp/aip-stab-e-target/debug/spike-v1-fixture"
    as_json = "--json" in args
    if "--bin" in args:
        binary = args[args.index("--bin") + 1]
    paths = [a for a in args if a.endswith(".aip")] or find_definitions()
    rows = []
    for p in paths:
        ex = facts(binary, p)
        if ex is None:
            continue
        for trail, x in walk(ex):
            module, _, func = x["implementation"].partition(".")
            src = locate(os.path.dirname(os.path.abspath(p)), module)
            body = None
            if src:
                body = body_of(open(src, encoding="utf-8", errors="replace").read(), src, func or "run")
                if body is None:
                    body = body_of(open(src, encoding="utf-8", errors="replace").read(), src, "run")
            kind = x.get("kind", "operation")
            row = {
                "definition": os.path.relpath(p, ROOT),
                "extension": ".".join(trail[-2:]) if trail and trail[0] != "operations" else ".".join(trail),
                "kind": kind,
                "implementation": x["implementation"],
                "declared": sorted(x["access"].keys()),
                "file": os.path.relpath(src, ROOT) if src else None,
            }
            if body is None:
                row["notes"] = ["implementation source not found" if not src else "function not found"]
            else:
                row["notes"] = compare(declared(x["access"]), observed(body), kind)
                row["observed"] = {k: sorted(v) if isinstance(v, set) else v for k, v in observed(body).items() if v}
            rows.append(row)
    if as_json:
        print(json.dumps(rows, indent=1))
        return
    print("| definition | extension (kind) | declared access | observed in code | gaps |")
    print("|---|---|---|---|---|")
    for r in rows:
        obs = "; ".join(f"{k}: {', '.join(v) if isinstance(v, list) else v}" for k, v in r.get("observed", {}).items()) or "-"
        print(f"| {r['definition']} | {r['extension']} ({r['kind']}) | {', '.join(r['declared']) or '-'} | {obs} | {'; '.join(r['notes']) or 'none'} |")
    print(f"\n{len(rows)} extension(s) in {len(paths)} definition(s)")


if __name__ == "__main__":
    main()
