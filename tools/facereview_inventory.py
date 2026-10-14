"""Pinned, source-only FaceReview porting inventory. Never imports the application."""

import argparse
import ast
import hashlib
import json
import pathlib
import re
import subprocess
from collections import Counter


def name(node):
    return ast.unparse(node)


def literal(node):
    try:
        return ast.literal_eval(node)
    except (ValueError, TypeError) as exc:
        raise ValueError(f'Expected literal: {name(node)}') from exc


def entry(path, line, kind, endpoint, **extra):
    key = f'{path}:{kind}:{endpoint}:{line}'
    return dict(id='FR-' + hashlib.sha256(key.encode()).hexdigest()[:12],
                source=path, line=line, kind=kind, endpoint=endpoint,
                status='unported', evidence=[], **extra)


def python_entries(path, source):
    tree = ast.parse(source, filename=path)
    blueprints = {}
    rows = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and isinstance(node.value, ast.Call):
            call = node.value
            if name(call.func) == 'Blueprint':
                prefix = next((literal(k.value) for k in call.keywords if k.arg == 'url_prefix'), '')
                for target in node.targets:
                    blueprints[name(target)] = prefix
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        policy = [name(d) for d in node.decorator_list if not isinstance(d, ast.Call)]
        calls = sorted({name(n.func) for n in ast.walk(node) if isinstance(n, ast.Call)
                        and ('Service.' in name(n.func) or 'Repository.' in name(n.func))})
        for dec in node.decorator_list:
            if not isinstance(dec, ast.Call) or not isinstance(dec.func, ast.Attribute):
                continue
            owner, method = name(dec.func.value), dec.func.attr
            common = dict(handler=node.name, policy=policy, calls=calls, scope='product')
            if method == 'route':
                if not dec.args:
                    raise ValueError(f'{path}:{dec.lineno}: route requires literal path')
                suffix = literal(dec.args[0])
                if owner != 'app' and owner not in blueprints:
                    raise ValueError(f'{path}:{dec.lineno}: unknown blueprint {owner}')
                prefix = blueprints.get(owner, '')
                methods = next((literal(k.value) for k in dec.keywords if k.arg == 'methods'), ['GET'])
                if 'test' in owner or path == 'app/routes/test.py':
                    common['scope'] = 'development-only'
                summary = next((literal(k.value) for d in node.decorator_list if isinstance(d, ast.Call)
                                and isinstance(d.func, ast.Attribute) and d.func.attr == 'doc'
                                for k in d.keywords if k.arg == 'summary'), node.name)
                for verb in methods:
                    rows.append(entry(path, dec.lineno, 'http', f'{verb} {prefix}{suffix}',
                                      summary=summary, **common))
            elif owner == 'socketio' and method in ('on', 'event'):
                event = literal(dec.args[0]) if dec.args else node.name
                rows.append(entry(path, dec.lineno, 'socket', event, **common))
            elif owner == 'celery_app' and method == 'task':
                job = next((literal(k.value) for k in dec.keywords if k.arg == 'name'), node.name)
                rows.append(entry(path, dec.lineno, 'job', job, **common))
    return sorted(rows, key=lambda r: (r['line'], r['endpoint']))


def frontend_entries(path, source):
    rows = []
    unsupported = re.search(r'\b(?:fetch|axios)\s*\(|\b(?:api|axios)\.(?:request|head|options)\s*[<(]', source)
    if unsupported:
        raise ValueError(f'Unsupported transport call in {path}:{source[:unsupported.start()].count(chr(10))+1}')
    # Only the pinned client idioms are recognized; new call forms fail closed.
    start = re.compile(r'\b(?:api|axios)\.(get|post|put|patch|delete)\b')
    for match in start.finditer(source):
        tail = source[match.end():].lstrip()
        arguments = re.match(r'(?:<[^()]*?>)?\s*\(', tail)
        if not arguments:
            raise ValueError(f'Unparsed HTTP generic/call in {path}')
        tail = tail[arguments.end():].lstrip()
        url = re.match(r"(['\"`])(.+?)\1", tail, re.S)
        if not url:
            variable = re.match(r'(\w+)\s*[,)]', tail)
            if variable:
                prefix = source[:match.start()].rsplit('export const ', 1)[-1]
                assignments = list(re.finditer(r'\bconst\s+' + re.escape(variable[1]) + r"\s*=\s*(['\"`])(.+?)\1\s*;", prefix, re.S))
                url = assignments[-1] if assignments else None
        if not url:
            raise ValueError(f'Unparsed HTTP call in {path}:{source[:match.start()].count(chr(10))+1}')
        rows.append(entry(path, source[:match.start()].count('\n')+1, 'client-http',
                          f'{match[1].upper()} {url[2]}'))
    for match in re.finditer(r'<Route\b(?:(?!<Route)[\s\S])*?\b(path\s*=\s*["\']([^"\']+)["\']|index\b)', source):
        rows.append(entry(path, source[:match.start()].count('\n')+1, 'screen', match[2] or '[index]'))
    for match in re.finditer(r"\bsocket\.(emit|on|off)\(\s*['\"]([^'\"]+)['\"]", source):
        rows.append(entry(path, source[:match.start()].count('\n')+1, 'client-socket', f'{match[1]} {match[2]}'))
    return rows


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args]).decode()


def mapping(row):
    source, endpoint, kind = row['source'], row['endpoint'], row['kind']
    if row.get('scope') == 'development-only':
        return 'excluded-development', '7.1.1', 'Development-only route; production registration excludes it.'
    if kind in ('screen', 'client-http', 'client-socket'):
        return 'official-sdk-client', '7.2.10', 'Preserve UI states; SDK replacement; HAR must show zero original-backend fallback.'
    if kind == 'socket':
        return 'gap-socketio-and-ml', '7.2.6', 'Socket.IO compatibility + authorized capability; real ML runtime, Mongo timeline and reconnect oracle.'
    if kind == 'job':
        return 'durable-job-and-provider', '7.2.7', 'Managed job + explicit provider capability; retry/idempotency and real Beat workload comparison.'
    if 'admin' in source:
        return 'policy-read-apply-and-extension', '7.2.9', 'ADMIN/SUPER_ADMIN policies, transactional transitions; Mongo/Redis/host metrics capability gaps.'
    if 'auth' in source:
        return 'auth-extension-and-principal', '7.2.2', 'Password/JWT/cookie issuer adapter + subject binding/revoke; no original auth API proxy.'
    if 'mypage' in source:
        return 'read-apply-and-specialized-extension', '7.2.8', 'Owner policy + SQL/Mongo aggregation; SMTP receipts and cross-store compensation.'
    if any(s in endpoint for s in ('comment', 'like', 'bookmark')):
        return 'read-apply-with-delete-toggle-gap', '7.2.4', 'Preserve physical user delete/toggle versus admin soft delete; locking and unique race oracles.'
    if 'watch' in source or 'home' in source:
        return 'read-apply-and-recommendation-extension', '7.2.3', 'Policy-controlled MySQL reads + Python score aggregation; actual Mongo/Redis capability, not SQL bypass.'
    return 'runtime-route', '7.1.2', 'Baseline health/landing route; not an app replacement claim.'


def generate(root):
    roles = {
        'new-facereview': 'backend',
        'facereview-front-joowon': 'user',
        'new-facereview-admin-front': 'admin',
    }
    result = dict(version=1, source_mode='git HEAD; LF-normalized working tree equivalence checked',
                  app_ported=False, repositories=[], entries=[], models=[], redis=[], integrations=[], services=[], schedules=[])
    for repo_name, role in roles.items():
        repo = root / repo_name
        sha = git(repo, 'rev-parse', 'HEAD').strip()
        files = git(repo, 'ls-tree', '-r', '--name-only', sha).splitlines()
        selected = [p for p in files if ((role == 'backend' and p.endswith('.py') and p.startswith(('app/', 'common/')))
                                       or (role != 'backend' and p.startswith('src/') and p.endswith(('.ts', '.tsx')))
                                       or p in ('requirements.txt', 'package.json', 'package-lock.json', 'yarn.lock',
                                                'Dockerfile', 'docker-compose.yml', 'vite.config.ts', 'vercel.json'))]
        manifest = []
        for path in selected:
            raw = subprocess.check_output(['git', '-C', str(repo), 'show', f'{sha}:{path}'])
            normalized = raw.replace(b'\r\n', b'\n')
            current = (repo / path).read_bytes().replace(b'\r\n', b'\n')
            if current != normalized:
                raise ValueError(f'{repo_name}/{path}: substantive dirty change; choose revision explicitly')
            manifest.append(dict(path=path, sha256=hashlib.sha256(normalized).hexdigest()))
            source = normalized.decode()
            scoped = f'{repo_name}/{path}'
            if path.endswith('.py'):
                rows = python_entries(path, source)
                tree = ast.parse(source)
                for node in ast.walk(tree):
                    if isinstance(node, ast.ClassDef) and path.startswith('app/models/'):
                        fields = [name(n) for n in node.body if isinstance(n, (ast.Assign, ast.AnnAssign))]
                        result['models'].append(dict(source=scoped, line=node.lineno, name=node.name, declarations=fields))
                    if isinstance(node, ast.ClassDef) and path.startswith('app/services/'):
                        for method in node.body:
                            if isinstance(method, ast.FunctionDef):
                                refs = sorted({n.id for n in ast.walk(method) if isinstance(n, ast.Name)})
                                calls = sorted({name(n.func) for n in ast.walk(method) if isinstance(n, ast.Call)})
                                result['services'].append(dict(name=node.name+'.'+method.name, source=scoped,
                                                               line=method.lineno, refs=refs, calls=calls,
                                                               policy=[name(d) for d in method.decorator_list]))
                    if isinstance(node, ast.Assign) and any(name(t) == 'beat_schedule' for t in node.targets):
                        result['schedules'].append(dict(source=scoped, line=node.lineno, expression=name(node.value)))
                for line_no, line in enumerate(source.splitlines(), 1):
                    if 'facereview:' in line:
                        result['redis'].append(dict(source=scoped, line=line_no, expression=line.strip()))
                    if re.search(r'\b(smtplib|SMTP_SSL|googleapiclient|build\(|keras.models|cv.detect_face|MongoClient|redis.Redis|redis.from_url)', line):
                        result['integrations'].append(dict(source=scoped, line=line_no, expression=line.strip()))
            elif path.endswith(('.ts', '.tsx')) and not re.search(r'\.(test|spec)\.', path):
                rows = frontend_entries(path, source)
            else:
                rows = []
            for row in rows:
                row['source'] = scoped
                row['id'] = 'FR-' + hashlib.sha256((role+row['id']).encode()).hexdigest()[:12]
                row['role'] = role
                row['approach'], row['wbs'], row['gate'] = mapping(row)
                result['entries'].append(row)
        result['repositories'].append(dict(name=repo_name, role=role, sha=sha, files=manifest))
    result['excluded_repositories'] = [
        dict(name='facereview-front', reason='Older user frontend; Docker workflow/README versus active Vercel revision in -joowon. Retain as reference, not a second product.'),
        dict(name='facereview-front-ref', reason='Same upstream as -joowon; duplicate/reference checkout, not additional product scope.'),
    ]
    services = {s['name']: s for s in result['services']}
    models = {s['name'] for s in result['models']}
    for row in result['entries']:
        pending = list(row.get('calls', []))
        seen, refs = set(), set()
        while pending:
            call = pending.pop()
            if call in seen or call not in services:
                continue
            seen.add(call)
            service = services[call]
            refs.update(service['refs'])
            pending.extend(service['calls'])
        row['service_evidence'] = [dict(name=c, source=services[c]['source'], line=services[c]['line'],
                                       policy=services[c]['policy']) for c in sorted(seen)]
        row['data_models_static'] = sorted(refs & models)
        if row['kind'] == 'client-http':
            method, path = row['endpoint'].split(' ', 1)
            canonical = re.sub(r'\$\{[^}]+\}', '{}', '/api'+path)
            row['backend_ids'] = [r['id'] for r in result['entries'] if r['kind'] == 'http'
                                  and re.sub(r'<[^>]+>', '{}', r['endpoint']) == method+' '+canonical]
            if not row['backend_ids']:
                raise ValueError(f'Unmatched client request: {row["source"]} {row["endpoint"]}')
    result['counts'] = dict(Counter(r['kind'] for r in result['entries']))
    if not all(result['counts'].get(k, 0) for k in ('http', 'screen', 'job', 'socket', 'client-http')):
        raise ValueError('Incomplete inventory: expected entrypoint kind missing')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    result = generate(args.root)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps(dict(counts=result['counts'], models=len(result['models']), redis_refs=len(result['redis']),
                          app_ported=False), ensure_ascii=False))


if __name__ == '__main__':
    main()
