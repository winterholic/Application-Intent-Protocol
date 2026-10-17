"""Pinned, source-only Infisical porting inventory. Never imports or runs the application.

Scans an Infisical source checkout (pinned revision recorded in output) and
writes plan-docs/porting/infisical-inventory.json:
- revision + license boundary (MIT root vs ee/ Enterprise dirs)
- functional modules (I01..I14) with service dirs, router files and
  extracted METHOD + url route entries (every `<receiver>.route({...})` object found by a bracket-balanced
  scan, so double-quoted, single-quoted and template-literal urls all count; commented-out routes do not)
- route definitions outside router files, BullMQ QueueName classification, MIT-to-EE import coupling
- CLI/SDK external-repo pointers

Regenerate:
  python3 tools/infisical_inventory.py --root ~/development/projects/aip/infisical-upstream \\
      --output /tmp/infisical-inventory.json
  cmp /tmp/infisical-inventory.json plan-docs/porting/infisical-inventory.json
"""

import argparse
import hashlib
import json
import pathlib
import re
import subprocess

ROUTE_CALL_RE = re.compile(r"\b\w+\.route\(\s*\{")
QUEUE_ENUM_RE = re.compile(r"export enum QueueName \{(.*?)\n\}", re.DOTALL)
QUEUE_MEMBER_RE = re.compile(r'^\s+(\w+) = "([^"]+)"', re.MULTILINE)
QUEUE_REF_RE = re.compile(r"QueueName\.(\w+)\b")
EE_IMPORT_RE = re.compile(r"""["']((?:@app/ee/|(?:\.\./)+ee/|\./ee/)[^"']*)["']""")
REGEX_PREV = set("(,=:[!&|?{};")


def entry_id(*parts):
    return "IN-" + hashlib.sha256("|".join(parts).encode()).hexdigest()[:12]


# module id, Korean name, backend/src service-dir prefixes (trailing "-" = prefix family)
MODULES = [
    ("I01", "사용자 인증", ["services/auth", "services/user", "services/user-activation", "services/user-alias",
                             "services/totp", "services/webauthn", "services/mfa-", "services/account-recovery",
                             "services/oauth-client", "services/auth-token", "services/user-engagement"]),
    ("I02", "머신 아이덴티티", ["services/identity", "services/identity-", "services/service-token"]),
    ("I03", "시크릿·폴더", ["services/secret", "services/secret-", "services/folder-", "services/project-env",
                             "services/resource-metadata",
                             "services/project-folder-grant", "services/project-key",
                             "services/approval-policy", "services/secret-access-approval-"]),
    ("I04", "조직·RBAC", ["services/org", "services/membership", "services/role", "services/project-role",
                           "services/project-membership", "services/group-project", "services/additional-privilege",
                           "services/folder-permission", "services/super-admin",
                           "services/external-group-org-role-mapping", "services/org-membership",
                           "services/project", "services/identity-project", "services/membership-"]),
    ("I05", "암호화·KMS", ["lib/crypto", "services/kms", "services/cmek", "services/project-bot",
                            "services/encryption-key-rotation", "services/hsm-connector"]),
    ("I06", "비동기·알림", ["services/secret-queue", "services/access-token-queue", "services/connection-queue",
                            "services/reminder", "services/resource-cleanup",
                            "services/certificate-cleanup", "services/telemetry", "services/update-check",
                            "services/notification", "services/webhook", "services/smtp", "services/slack",
                            "services/microsoft-teams", "services/health-alert",
                            "services/secret-reminder-recipients", "services/offline-usage-report"]),
    ("I07", "연동", ["services/integration", "services/integration-auth", "services/secret-sync",
                     "services/app-connection", "services/workflow-integration",
                     "services/github-app", "services/cert-manager-"]),
    ("I08", "PKI·인증서", ["services/certificate", "services/certificate-", "services/pki-", "services/signer",
                         "services/signer-"]),
    ("I09", "관리·운영", ["services/org-admin", "services/announcement", "services/license-client",
                           "services/enrollment-config", "services/org-product-stats", "services/org-asset",
                           "services/convertor", "services/external-migration",
                           "services/legacy-pki-deprecation"]),
    ("I10", "감사·이벤트", ["services/event-outbox", "services/alert"]),
]


def resolve_dirs(backend, prefixes):
    """Expand service-dir prefixes to existing relative dirs (sorted, deduped)."""
    found = []
    for prefix in prefixes:
        if prefix.endswith("-"):
            stem = prefix[:-1]
            parent = backend / pathlib.PurePosixPath(stem).parent
            base = pathlib.PurePosixPath(stem).name
            if parent.is_dir():
                for child in sorted(parent.iterdir()):
                    if child.is_dir() and child.name.startswith(base + "-"):
                        found.append("backend/src/" + child.relative_to(backend).as_posix())
        else:
            candidate = backend / prefix
            if candidate.is_dir():
                found.append("backend/src/" + prefix)
    return sorted(set(found))


def assign_service_dirs(backend):
    """Map each service dir to exactly one module. Returns (module -> dirs, contested, unclassified)."""
    claims, contested = {}, {}
    for order, (mod_id, _name, prefixes) in enumerate(MODULES):
        for prefix in prefixes:
            rank = (1 if prefix.endswith("-") else 0, order)
            for found in resolve_dirs(backend, [prefix]):
                contested.setdefault(found, set()).add(mod_id)
                if found not in claims or rank < claims[found][0]:
                    claims[found] = (rank, mod_id)
    by_module = {mod_id: sorted(d for d, (_r, m) in claims.items() if m == mod_id) for mod_id, _n, _p in MODULES}
    contested = {d: sorted(mods) for d, mods in sorted(contested.items()) if len(mods) > 1}
    services = backend / "services"
    every = sorted("backend/src/services/" + c.name for c in services.iterdir() if c.is_dir()) \
        if services.is_dir() else []
    unclassified = [d for d in every if d not in claims]
    return by_module, {d: {"owner": claims[d][1], "claimed_by": mods} for d, mods in contested.items()}, unclassified


def stem_keys(dirs):
    """Hyphen-segment match keys from service dir basenames."""
    keys = set()
    for d in dirs:
        keys.add(pathlib.PurePosixPath(d).name)
    return keys


# Exact-stem overrides for routers whose names do not carry a service key.
# Suffix fallbacks apply afterwards ("-connection"/"-sync" providers -> I07).
EXTRA_STEM_MODULE = {
    "admin": "I09", "dashboard": "I09", "operations": "I09",
    "users": "I01", "login": "I01", "signup": "I01", "password": "I01",
    "mfa": "I01", "oauth": "I01", "sso": "I01",
    "organization": "I04", "organization-memberships": "I04", "project": "I04",
    "project-group-memberships": "I04", "groups": "I04", "roles": "I04",
    "identities": "I04", "bot": "I04",
    "event": "I10", "lifecycle": "I06",
    "signing": "I08", "terraform-cloud": "I07",
    "static-secrets-validation-rule": "I03", "dynamic-secrets-validation-rule": "I03",
    "acme-enrollment": "I08", "scep-enrollment": "I08", "est-enrollment": "I08",
    "api-enrollment": "I08", "enrollment-state": "I08",
    "deprecated-certificates": "I08", "deprecated-project": "I04",
    "approval-policy": "I08", "requests": "I08",
}

TEST_ONLY_STEMS = {"bdd-nock"}

MODULE_NOTES = {
    "I07": "대부분의 provider 라우터는 app-connection-endpoints.ts의 공용 helper로 "
            "provider별 CRUD 6종(GET /, GET /available, GET /:connectionId, "
            "GET /connection-name/:connectionName, POST /, PATCH /:connectionId)을 "
            "생성하므로, 실제 노출 endpoint 수는 정규식 집계보다 수십 배 많다. "
            "provider 수는 app-connection-routers 파일 수로 센다.",
}


def longest_key_match(stem, keys):
    """Length (in segments) of the longest key appearing as a contiguous
    hyphen-segment run inside the router stem, else 0."""
    segments = stem.split("-")
    best = 0
    for key in keys:
        parts = key.split("-")
        for i in range(len(segments) - len(parts) + 1):
            if segments[i:i + len(parts)] == parts:
                best = max(best, len(parts))
    return best


def git_sha(root):
    try:
        out = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"],
                             capture_output=True, text=True, check=True)
        return out.stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return "unknown"


def skip_quoted(text, i):
    """Index after the quoted string starting at text[i] (a ' or ")."""
    quote = text[i]
    i += 1
    while i < len(text) and text[i] != quote:
        i += 2 if text[i] == "\\" else 1
    return i + 1


def skip_template(text, i):
    """Index after the template literal starting at text[i] (a backtick), including nested ${...}."""
    i += 1
    while i < len(text):
        ch = text[i]
        if ch == "\\":
            i += 2
        elif ch == "`":
            return i + 1
        elif ch == "$" and text[i + 1:i + 2] == "{":
            i = match_brace(text, i + 1) + 1
        else:
            i += 1
    return i


def skip_regex(text, i):
    """Index after the regex literal starting at text[i] (a /), honoring character classes."""
    i += 1
    in_class = False
    while i < len(text):
        ch = text[i]
        if ch == "\\":
            i += 2
            continue
        if ch == "[":
            in_class = True
        elif ch == "]":
            in_class = False
        elif ch == "/" and not in_class:
            return i + 1
        elif ch == "\n":
            return i
        i += 1
    return i


def scan(text, start):
    """Yield (index, char) of code characters from start, skipping comments, strings, templates, regexes."""
    i = start
    prev = ""
    while i < len(text):
        ch = text[i]
        nxt = text[i + 1:i + 2]
        if ch == "/" and nxt == "/":
            i = text.find("\n", i)
            i = len(text) if i < 0 else i
            continue
        if ch == "/" and nxt == "*":
            i = text.find("*/", i + 2)
            i = len(text) if i < 0 else i + 2
            continue
        if ch in "\"'":
            i = skip_quoted(text, i)
            prev = "a"
            continue
        if ch == "`":
            i = skip_template(text, i)
            prev = "a"
            continue
        if ch == "/" and (prev == "" or prev in REGEX_PREV):
            i = skip_regex(text, i)
            prev = "a"
            continue
        yield i, ch
        if not ch.isspace():
            prev = ch
        i += 1


def match_brace(text, open_index):
    """Index of the } matching the { at open_index, or len(text) when unbalanced."""
    depth = 0
    for i, ch in scan(text, open_index):
        if ch in "{([":
            depth += 1
        elif ch in "})]":
            depth -= 1
            if depth == 0:
                return i
    return len(text)


def split_top_level(inner):
    """Split an object body on commas that are not nested inside brackets, strings or templates."""
    parts, depth, begin = [], 0, 0
    for i, ch in scan(inner, 0):
        if ch in "{([":
            depth += 1
        elif ch in "})]":
            depth -= 1
        elif ch == "," and depth == 0:
            parts.append(inner[begin:i])
            begin = i + 1
    parts.append(inner[begin:])
    return [p for p in parts if p.strip()]


LEADING_COMMENTS_RE = re.compile(r"\A(?:\s|//[^\n]*\n|/\*.*?\*/)*", re.DOTALL)
TRAILING_COMMENTS_RE = re.compile(r"(?:\s|//[^\n]*|/\*.*?\*/)*\Z", re.DOTALL)


def classify_expression(expr):
    """('string'|'template'|'dynamic', value) for a url/method expression.

    'template' is any backtick literal (with or without ${}), 'string' a quoted one.
    """
    expr = TRAILING_COMMENTS_RE.sub("", expr.strip()).strip()
    for quote in ('"', "'"):
        if len(expr) >= 2 and expr[0] == quote and skip_quoted(expr, 0) == len(expr):
            return "string", expr[1:-1]
    if len(expr) >= 2 and expr[0] == "`" and skip_template(expr, 0) == len(expr):
        body = expr[1:-1]
        return "template", body
    return "dynamic", " ".join(expr.split())[:120]


def mask_comments(text):
    """The same text with every comment replaced by spaces (newlines kept), so code positions stay valid."""
    out = []
    i = 0
    prev = ""
    while i < len(text):
        ch, nxt = text[i], text[i + 1:i + 2]
        if ch == "/" and nxt == "/":
            end = text.find("\n", i)
            end = len(text) if end < 0 else end
            out.append(" " * (end - i))
            i = end
            continue
        if ch == "/" and nxt == "*":
            end = text.find("*/", i + 2)
            end = len(text) if end < 0 else end + 2
            out.append("".join(c if c == "\n" else " " for c in text[i:end]))
            i = end
            continue
        if ch in "\"'":
            end = skip_quoted(text, i)
        elif ch == "`":
            end = skip_template(text, i)
        elif ch == "/" and (prev == "" or prev in REGEX_PREV):
            end = skip_regex(text, i)
        else:
            out.append(ch)
            if not ch.isspace():
                prev = ch
            i += 1
            continue
        out.append(text[i:end])
        prev = "a"
        i = end
    return "".join(out)


def extract_routes(router_file):
    """Every `<receiver>.route({...})` object in a file with its method and url expression kinds."""
    text = mask_comments(router_file.read_text(encoding="utf-8", errors="replace"))
    routes = []
    for match in ROUTE_CALL_RE.finditer(text):
        open_index = match.end() - 1
        close = match_brace(text, open_index)
        fields = {}
        for entry in split_top_level(text[open_index + 1:close]):
            entry = LEADING_COMMENTS_RE.sub("", entry, count=1)
            key = re.match(r"\s*[\"']?(\w+)[\"']?\s*:\s*(.*)\Z", entry, re.DOTALL)
            if key and key.group(1) in ("method", "url") and key.group(1) not in fields:
                fields[key.group(1)] = key.group(2)
        if "url" not in fields or "method" not in fields:
            continue
        url_kind, url = classify_expression(fields["url"])
        method_kind, method = classify_expression(fields["method"])
        routes.append({"method": method, "url": url, "url_kind": url_kind if method_kind != "dynamic" else "dynamic",
                       "line": text[:match.start()].count("\n") + 1})
    return routes


def url_kind_counts(routes):
    counts = {"string": 0, "template": 0, "dynamic": 0}
    for route in routes:
        counts[route["url_kind"]] += 1
    return counts


def outside_router_definitions(backend, router_rels):
    """route({...}) objects in MIT files that are not *-router.ts (helpers, plugins, the route index)."""
    found = []
    for sub in ("server/routes", "server/plugins"):
        base = backend / sub
        if not base.is_dir():
            continue
        for path in sorted(base.rglob("*.ts")):
            rel = path.relative_to(backend).as_posix()
            if rel in router_rels or path.name.endswith(".test.ts"):
                continue
            routes = extract_routes(path)
            if routes:
                found.append({"file": rel, "route_definitions": len(routes), "url_kinds": url_kind_counts(routes)})
    return found


def queue_inventory(backend):
    """BullMQ QueueName members classified by where QueueName.<Member> is referenced.

    mit: referenced only outside ee/; ee: only inside ee/; mixed: both; unreferenced: neither
    (kept in the enum, e.g. a legacy repeatable queue). A reference is a producer or a worker, so
    the class says where the queue is used, not that the work could be licensed or ported separately.
    """
    queue_file = backend / "queue" / "queue-service.ts"
    if not queue_file.is_file():
        return {"queues": [], "totals": {}}
    members = QUEUE_MEMBER_RE.findall(QUEUE_ENUM_RE.search(queue_file.read_text(encoding="utf-8")).group(1))
    refs = {name: {"mit": set(), "ee": set()} for name, _ in members}
    for path in sorted(backend.rglob("*.ts")):
        rel = path.relative_to(backend).as_posix()
        if rel == "queue/queue-service.ts":
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for name in set(QUEUE_REF_RE.findall(text)):
            if name in refs:
                refs[name]["ee" if rel.startswith("ee/") else "mit"].add(rel)
    queues = []
    for name, value in members:
        mit, ee = refs[name]["mit"], refs[name]["ee"]
        kind = "mixed" if mit and ee else "mit" if mit else "ee" if ee else "unreferenced"
        queues.append({"member": name, "queue": value, "class": kind,
                       "mit_files": len(mit), "ee_files": len(ee)})
    totals = {"queue_names": len(queues)}
    for kind in ("mit", "ee", "mixed", "unreferenced"):
        totals[kind] = sum(1 for q in queues if q["class"] == kind)
    return {"queues": queues, "totals": totals}


def ee_import_by_module(root, backend, modules, assignments):
    """Per module: how many of its non-test service files and assigned router files import from ee/."""
    result = {}
    for module in modules:
        if module.get("kind") != "mit":
            continue
        service_files = [p for rel in module["paths"] for p in (root / rel).rglob("*.ts")
                         if not p.name.endswith(".test.ts")]
        routers = [path for path, _rel in assignments.get(module["id"], [])]

        def importing(paths):
            return sum(1 for p in paths if EE_IMPORT_RE.search(p.read_text(encoding="utf-8", errors="replace")))

        result[module["id"]] = {"service_files": len(service_files),
                                "service_files_importing_ee": importing(service_files),
                                "router_files": len(routers),
                                "router_files_importing_ee": importing(routers)}
    return result


def ee_coupling(backend):
    """How many non-ee files import from ee/ (alias @app/ee/ or a relative path to an ee directory)."""
    importing, statements, permission_users, targets = [], 0, [], {}
    mit_files = [p for p in sorted(backend.rglob("*.ts")) if p.relative_to(backend).parts[0] != "ee"]
    for path in mit_files:
        found = EE_IMPORT_RE.findall(path.read_text(encoding="utf-8", errors="replace"))
        if not found:
            continue
        importing.append(path)
        statements += len(found)
        modules = set()
        for target in found:
            if target.startswith("@app/ee/"):
                modules.add("/".join(target.split("/")[2:4]))
        for module in modules:
            targets[module] = targets.get(module, 0) + 1
        if any(t.startswith("@app/ee/services/permission/") for t in found):
            permission_users.append(path)
    non_test = [p for p in importing if not p.name.endswith(".test.ts")]
    permission_dir = backend / "ee" / "services" / "permission"
    permission_files = sorted(p.relative_to(backend).as_posix() for p in permission_dir.rglob("*.ts")) \
        if permission_dir.is_dir() else []
    top = sorted(targets.items(), key=lambda kv: (-kv[1], kv[0]))[:20]
    return {
        "definition": "backend/src files outside ee/ that contain an import of @app/ee/... or a relative path into an ee directory",
        "non_ee_ts_files": len(mit_files),
        "non_ee_files_importing_ee": len(importing),
        "non_ee_non_test_files_importing_ee": len(non_test),
        "import_statements": statements,
        "non_test_files_importing_ee_permission": len([p for p in permission_users if not p.name.endswith(".test.ts")]),
        "top_imported_ee_modules": [{"module": m, "importing_files": n} for m, n in top],
        "ee_permission_files": permission_files,
        "ee_permission_file_count": len(permission_files),
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    root = pathlib.Path(args.root).expanduser().resolve()
    backend = root / "backend" / "src"

    router_files = []
    for sub in ("server/routes", "ee/routes"):
        base = backend / sub
        if base.is_dir():
            for path in sorted(base.rglob("*-router.ts")):
                router_files.append((path, path.relative_to(backend).as_posix()))

    dirs_by_module, contested_dirs, unclassified_dirs = assign_service_dirs(backend)
    mod_keys = []
    for mod_id, name, _prefixes in MODULES:
        dirs = dirs_by_module[mod_id]
        mod_keys.append((mod_id, name, dirs, stem_keys(dirs)))

    # Each MIT router file belongs to exactly one module: longest key wins,
    # ties broken by module order.
    assignments = {}
    unassigned = []
    test_only = []
    for router_path, rel in router_files:
        if rel.startswith("ee/"):
            continue
        stem = router_path.stem
        if stem.endswith("-router"):
            stem = stem[:-len("-router")]
        if stem in TEST_ONLY_STEMS:
            test_only.append(rel)
            continue
        if stem in EXTRA_STEM_MODULE:
            assignments.setdefault(EXTRA_STEM_MODULE[stem], []).append((router_path, rel))
            continue
        if stem.endswith("-connection") or stem.endswith("-sync"):
            assignments.setdefault("I07", []).append((router_path, rel))
            continue
        best = None
        for order, (mod_id, _name, _dirs, keys) in enumerate(mod_keys):
            length = longest_key_match(stem, keys)
            if length and (best is None or (length, -order) > (best[1], -best[2])):
                best = (mod_id, length, order)
        if best is None:
            unassigned.append(rel)
        else:
            assignments.setdefault(best[0], []).append((router_path, rel))

    modules = []
    for mod_id, name, dirs, _keys in mod_keys:
        file_count = 0
        for rel_dir in dirs:
            for path in (root / rel_dir).rglob("*.ts"):
                if not path.name.endswith(".test.ts"):
                    file_count += 1
        mod_routes = []
        for router_path, rel in sorted(assignments.get(mod_id, []), key=lambda t: t[1]):
            for route in extract_routes(router_path):
                mod_routes.append({"method": route["method"], "url": route["url"], "url_kind": route["url_kind"],
                                   "file": rel, "line": route["line"],
                                   "id": entry_id(mod_id, route["method"], route["url"], rel, str(route["line"]))})
        mod_routes.sort(key=lambda r: (r["file"], r["line"]))
        modules.append({"id": mod_id, "name": name, "kind": "mit",
                        "paths": dirs, "service_files": file_count,
                        "routes": mod_routes, "route_count": len(mod_routes),
                        "url_kinds": url_kind_counts(mod_routes),
                        "status": "unported", "evidence": []})
    unassigned_inventory = sorted(unassigned)

    fe = root / "frontend"
    tsx = [p for p in fe.rglob("*.tsx") if "/node_modules/" not in p.as_posix()] if fe.is_dir() else []
    ts = [p for p in fe.rglob("*.ts") if "/node_modules/" not in p.as_posix()] if fe.is_dir() else []
    modules.append({"id": "I11", "name": "프론트엔드", "kind": "frontend",
                    "paths": ["frontend/"], "tsx_files": len(tsx), "ts_files": len(ts),
                    "routes": [], "status": "unported", "evidence": []})

    for m in modules:
        if m["id"] in MODULE_NOTES:
            m["note"] = MODULE_NOTES[m["id"]]

    modules.append({"id": "I12", "name": "CLI·SDK", "kind": "external",
                    "note": "CLI(github.com/Infisical/cli)와 SDK는 별도 저장소. 분모 밖이며 연동 대상으로만 기록.",
                    "routes": [], "status": "unported", "evidence": []})

    ee_services = sorted(p.name for p in (backend / "ee" / "services").iterdir() if p.is_dir()) \
        if (backend / "ee" / "services").is_dir() else []
    ee_routes = sorted(str(p.relative_to(backend).as_posix())
                       for p in (backend / "ee" / "routes").rglob("*-router.ts")) \
        if (backend / "ee" / "routes").is_dir() else []
    ee_route_defs = sum(len(extract_routes(backend / rel)) for rel in ee_routes)
    go_base = root / "backend-go"
    go_files = [p for p in go_base.rglob("*.go")] if go_base.is_dir() else []
    modules.append({"id": "I13", "name": "EE 전용(범위 밖)", "kind": "ee-out-of-scope",
                    "paths": ["backend/src/ee/", "backend-go/internal/ee/"],
                    "ee_services": ee_services, "ee_service_count": len(ee_services),
                    "ee_routes": ee_routes, "ee_route_files": len(ee_routes),
                    "ee_route_definitions": ee_route_defs,
                    "routes": [], "status": "out-of-scope", "evidence": []})
    modules.append({"id": "I14", "name": "Go 사이드카", "kind": "go-sidecar",
                    "paths": ["backend-go/"], "go_files": len(go_files),
                    "routes": [], "status": "unported", "evidence": []})

    ee_dirs = []
    for candidate in (root / "backend" / "src" / "ee", root / "backend-go" / "internal" / "ee"):
        if candidate.is_dir():
            ee_dirs.append(candidate.relative_to(root).as_posix())

    inventory = {
        "revision": {"sha": git_sha(root), "source": "https://github.com/Infisical/infisical"},
        "license": {
            "root": "MIT Expat (ee/ 디렉터리 제외 전부)",
            "ee": "Enterprise License: production 사용에 구독 필요, 개발·테스트 수정 허용",
            "ee_dirs": sorted(ee_dirs),
        },
        "modules": modules,
        "totals": {
            "router_files_total": len(router_files),
            "router_files_ee": sum(1 for _p, rel in router_files if rel.startswith("ee/")),
            "router_files_mit": sum(1 for _p, rel in router_files if not rel.startswith("ee/")),
            "router_files_assigned": sum(1 for _p, rel in router_files if not rel.startswith("ee/")) - len(test_only),
            "mit_routes": sum(m.get("route_count", 0) for m in modules),
            "mit_route_url_kinds": url_kind_counts([r for m in modules for r in m.get("routes", [])]),
            "unassigned_router_files": unassigned_inventory,
            "excluded_test_only_router_files": sorted(test_only),
        },
        "service_dir_coverage": {
            "service_dirs_total": sum(1 for v in dirs_by_module.values() for d in v if d.startswith("backend/src/services/"))
                                  + len(unclassified_dirs),
            "unclassified": unclassified_dirs,
            "contested_resolved_by_exact_entry_then_module_order": contested_dirs,
        },
        "route_definitions_outside_router_files": outside_router_definitions(
            backend, {rel for _p, rel in router_files}),
        "bullmq": queue_inventory(backend),
        "ee_coupling": dict(ee_coupling(backend),
                            by_module=ee_import_by_module(root, backend, modules, assignments)),
        "counting_notes": [
            "Routes are `<receiver>.route({...})` objects found by a bracket-balanced scan, so double-quoted, "
            "single-quoted and template-literal urls are all counted; url_kind tells them apart.",
            "route_count is a count of route definitions in source. Helper functions such as "
            "app-connection-endpoints register the same shape once per provider router, so exposed endpoints "
            "exceed the definition count (see route_definitions_outside_router_files).",
            "A route whose method or url is not a literal is counted with url_kind 'dynamic'.",
        ],
    }
    pathlib.Path(args.output).write_text(json.dumps(inventory, indent=2, ensure_ascii=False) + "\n",
                                          encoding="utf-8")
    totals = inventory["totals"]
    print(f"modules={len(modules)} routers_mit={totals['router_files_mit']} routers_ee={totals['router_files_ee']} "
          f"routes={totals['mit_routes']} queues={inventory['bullmq']['totals'].get('queue_names')}")


if __name__ == "__main__":
    main()
