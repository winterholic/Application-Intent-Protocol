"""Pinned, source-only Infisical porting inventory. Never imports or runs the application.

Scans an Infisical source checkout (pinned revision recorded in output) and
writes plan-docs/porting/infisical-inventory.json:
- revision + license boundary (MIT root vs ee/ Enterprise dirs)
- functional modules (I01..I14) with service dirs, router files and
  extracted METHOD + url route entries (Fastify server.route blocks)
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

ROUTE_RE = re.compile(r"server\.route\(\{(?P<body>.*?)\}\s*,?\s*\)", re.DOTALL)
URL_RE = re.compile(r'url:\s*"([^"]+)"')
METHOD_RE = re.compile(r'method:\s*"([A-Z]+)"')


def entry_id(*parts):
    return "IN-" + hashlib.sha256("|".join(parts).encode()).hexdigest()[:12]


# module id, Korean name, backend/src service-dir prefixes (trailing "-" = prefix family)
MODULES = [
    ("I01", "사용자 인증", ["services/auth", "services/user", "services/user-activation", "services/user-alias",
                             "services/totp", "services/webauthn", "services/mfa-", "services/account-recovery",
                             "services/oauth-client", "services/auth-token"]),
    ("I02", "머신 아이덴티티", ["services/identity", "services/service-token"]),
    ("I03", "시크릿·폴더", ["services/secret", "services/folder-", "services/project-env",
                             "services/project-folder-grant", "services/project-key",
                             "services/approval-policy", "services/secret-access-approval-"]),
    ("I04", "조직·RBAC", ["services/org", "services/membership", "services/role", "services/project-role",
                           "services/project-membership", "services/group-project", "services/additional-privilege",
                           "services/folder-permission", "services/super-admin",
                           "services/external-group-org-role-mapping"]),
    ("I05", "암호화·KMS", ["lib/crypto", "services/kms", "services/cmek",
                            "services/encryption-key-rotation", "services/hsm-connector"]),
    ("I06", "비동기·알림", ["services/secret-queue", "services/reminder", "services/resource-cleanup",
                            "services/certificate-cleanup", "services/telemetry", "services/update-check",
                            "services/notification", "services/webhook", "services/smtp", "services/slack",
                            "services/microsoft-teams", "services/health-alert",
                            "services/secret-reminder-recipients", "services/offline-usage-report"]),
    ("I07", "연동", ["services/integration", "services/app-connection", "services/workflow-integration",
                     "services/github-app", "services/cert-manager-"]),
    ("I08", "PKI·인증서", ["services/certificate", "services/pki-", "services/signer"]),
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


def extract_routes(router_file):
    text = router_file.read_text(encoding="utf-8", errors="replace")
    routes = []
    for match in ROUTE_RE.finditer(text):
        body = match.group("body")
        url = URL_RE.search(body)
        method = METHOD_RE.search(body)
        if url and method:
            line = text[:match.start()].count("\n") + 1
            routes.append({"method": method.group(1), "url": url.group(1), "line": line})
    return routes


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

    mod_keys = []
    for mod_id, name, prefixes in MODULES:
        dirs = resolve_dirs(backend, prefixes)
        mod_keys.append((mod_id, name, dirs, stem_keys(dirs)))

    # Each MIT router file belongs to exactly one module: longest key wins,
    # ties broken by module order.
    assignments = {}
    unassigned = []
    for router_path, rel in router_files:
        if rel.startswith("ee/"):
            continue
        stem = router_path.stem
        if stem.endswith("-router"):
            stem = stem[:-len("-router")]
        if stem in TEST_ONLY_STEMS:
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
                mod_routes.append({"method": route["method"], "url": route["url"],
                                   "file": rel, "line": route["line"],
                                   "id": entry_id(mod_id, route["method"], route["url"], rel)})
        mod_routes.sort(key=lambda r: (r["file"], r["line"]))
        modules.append({"id": mod_id, "name": name, "kind": "mit",
                        "paths": dirs, "service_files": file_count,
                        "routes": mod_routes, "route_count": len(mod_routes),
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
    go_base = root / "backend-go"
    go_files = [p for p in go_base.rglob("*.go")] if go_base.is_dir() else []
    modules.append({"id": "I13", "name": "EE 전용(범위 밖)", "kind": "ee-out-of-scope",
                    "paths": ["backend/src/ee/", "backend-go/internal/ee/"],
                    "ee_services": ee_services, "ee_service_count": len(ee_services),
                    "ee_routes": ee_routes, "ee_route_files": len(ee_routes),
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
            "router_files": len(router_files),
            "mit_routes": sum(m.get("route_count", 0) for m in modules),
            "unassigned_router_files": unassigned_inventory,
        },
    }
    pathlib.Path(args.output).write_text(json.dumps(inventory, indent=2, ensure_ascii=False) + "\n",
                                          encoding="utf-8")
    print(f"modules={len(modules)} routers={len(router_files)} "
          f"routes={inventory['totals']['mit_routes']}")


if __name__ == "__main__":
    main()
