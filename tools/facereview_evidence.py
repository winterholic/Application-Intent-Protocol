"""Fail closed on missing, partial or altered original-API evidence."""
import argparse
import hashlib
import json
from pathlib import Path

from facereview_oracle import compare_result, sensitive_fields

REQUIRED = ['result.json', 'oracle.json', 'inventory.json', 'lock.txt', 'compose.yaml']
SCOPE = {'api': 'executed', 'browser': 'not-executed', 'ml': 'not-executed', 'aip_port': 'not-executed'}


def check_evidence(manifest, directory):
    errors = []
    if not isinstance(manifest, dict):
        return ['manifest must be an object']
    errors.extend(sensitive_fields(manifest, 'manifest'))
    if manifest.get('kind') != 'original-api' or manifest.get('scope') != SCOPE:
        errors.append('scope must be original API only')
    for key in ['api_exit_code', 'cleanup_exit_code']:
        if type(manifest.get(key)) is not int or manifest[key] != 0:
            errors.append(f'{key} must be zero')
    commands = manifest.get('commands')
    if not isinstance(commands, list) or not commands:
        errors.append('commands are required')
    else:
        for command in commands:
            argv = command.get('argv') if isinstance(command, dict) else None
            if not isinstance(argv, list) or not argv or not all(isinstance(a, str) for a in argv):
                errors.append('commands argv invalid')
            elif any(any(term in a.lower() for term in ['authorization=', 'bearer ', 'password=', 'access_token=', 'refresh_token=']) for a in argv):
                errors.append('sensitive command argument')
            if not isinstance(command, dict) or type(command.get('exit_code')) is not int or command['exit_code'] != 0:
                errors.append('commands contain failed or missing exit_code')
    files = manifest.get('files')
    if not isinstance(files, dict):
        files = {}
    verified = {}
    directory = Path(directory).resolve()
    for name in REQUIRED:
        record = files.get(name)
        if not isinstance(record, dict) or not isinstance(record.get('path'), str):
            errors.append(f'missing artifact {name}')
            continue
        path = (directory / record['path']).resolve()
        if not path.is_relative_to(directory):
            errors.append(f'outside artifact directory: {name}')
            continue
        try:
            data = path.read_bytes()
        except OSError:
            errors.append(f'missing artifact {name}')
            continue
        if hashlib.sha256(data).hexdigest() != record.get('sha256'):
            errors.append(f'artifact hash mismatch: {name}')
            continue
        if name.endswith('.json'):
            try:
                verified[name] = json.loads(data)
            except (ValueError, UnicodeError):
                errors.append(f'malformed artifact {name}')
        elif name == 'lock.txt':
            try:
                verified[name] = sorted(line.strip() for line in data.decode().splitlines() if line.strip())
            except UnicodeError:
                errors.append('malformed artifact lock.txt')
    if 'result.json' in verified and 'oracle.json' in verified:
        errors.extend(compare_result(verified['result.json'], verified['oracle.json']))
        if not isinstance(verified['result.json'], dict) or verified['result.json'].get('source_sha') != manifest.get('source_sha'):
            errors.append('manifest source_sha mismatch')
    if 'lock.txt' in verified and isinstance(verified.get('oracle.json'), dict):
        if verified['lock.txt'] != verified['oracle.json'].get('packages'):
            errors.append('lock packages differ from reviewed oracle')
    if 'inventory.json' in verified:
        inventory = verified['inventory.json']
        repositories = inventory.get('repositories') if isinstance(inventory, dict) else None
        backend = [r for r in repositories if isinstance(r, dict) and r.get('role') == 'backend'] if isinstance(repositories, list) else []
        if len(backend) != 1 or backend[0].get('sha') != manifest.get('source_sha'):
            errors.append('inventory source_sha mismatch')
    return sorted(errors)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    args = parser.parse_args()
    try:
        manifest = json.loads(args.manifest.read_text())
        errors = check_evidence(manifest, args.manifest.parent)
    except (OSError, ValueError, TypeError):
        errors = ['manifest missing or malformed']
    print(json.dumps({'ok': not errors, 'errors': errors}, ensure_ascii=False))
    return 1 if errors else 0


if __name__ == '__main__':
    raise SystemExit(main())
