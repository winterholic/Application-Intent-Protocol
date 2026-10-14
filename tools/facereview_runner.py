"""Run the original API baseline in a fresh owned Compose project, then verify evidence.

Requires Docker Compose and the separately prepared baseline-api.lock Python venv.
Does not install packages, read .env or modify the original repository.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import uuid

from facereview_evidence import check_evidence, SCOPE


def clean_project(container_ids):
    return not container_ids.strip()


def owned_lifecycle(start, execute, stop):
    api, cleanup = -1, -1
    try:
        if start() == 0:
            api = execute()
    finally:
        cleanup = stop()
    return {'api_exit_code': api, 'cleanup_exit_code': cleanup}


def safe_extract(data, directory):
    directory = Path(directory).resolve()
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        members = archive.getmembers()
        for member in members:
            target = (directory / member.name).resolve()
            if member.name.startswith('/') or not target.is_relative_to(directory) or not (member.isfile() or member.isdir()):
                raise ValueError('Pinned source archive contains an unsafe member')
        for member in members:
            target = directory / member.name
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(archive.extractfile(member).read())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--backend-repo', type=Path, required=True)
    parser.add_argument('--python', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True, help='new evidence directory; never overwrites a previous run')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    config = root / 'plan-docs/porting'
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=False)
    # New project name makes cleanup exclusive even when another run starts after preflight.
    project = 'aip-facereview-api-' + uuid.uuid4().hex[:12]
    manifest = {'kind': 'original-api', 'scope': SCOPE.copy(), 'project': project,
                'api_exit_code': -1, 'cleanup_exit_code': -1, 'commands': [], 'files': {}}
    commands = manifest['commands']

    def command(argv, *, timeout=60):
        p = subprocess.run([str(a) for a in argv], cwd=root, capture_output=True, timeout=timeout)
        commands.append({'argv': [str(a) for a in argv], 'exit_code': p.returncode})
        # Never echo subprocess stderr or captured auth responses to artifacts.
        return p

    inputs = {'oracle.json': config/'facereview-api-oracle.json', 'inventory.json': config/'facereview-inventory.json',
              'lock.txt': config/'baseline-api.lock', 'compose.yaml': config/'baseline.compose.yaml'}
    for name, source in inputs.items():
        shutil.copyfile(source, output/name)
    oracle = json.loads((output/'oracle.json').read_text())
    inventory = json.loads((output/'inventory.json').read_text())
    backend = next(r for r in inventory['repositories'] if r['role'] == 'backend')
    manifest['source_sha'] = backend['sha']
    compose = ['docker', 'compose', '-p', project, '-f', output/'compose.yaml']
    error = None
    try:
        preflight = command(['docker', 'compose', '-f', config/'baseline.compose.yaml', 'ps', '-aq'])
        if preflight.returncode or not clean_project(preflight.stdout.decode()):
            raise RuntimeError('Default baseline project already exists or Docker preflight failed; no cleanup attempted')
        version_code = 'import importlib.metadata as m,json;print(json.dumps(sorted(d.metadata["Name"]+"=="+d.version for d in m.distributions())))'
        versions = command([args.python, '-c', version_code])
        locked = sorted(line.strip() for line in (output/'lock.txt').read_text().splitlines() if line.strip())
        if versions.returncode or locked != oracle['packages'] or json.loads(versions.stdout) != oracle['packages']:
            raise RuntimeError('Python environment differs from baseline-api.lock/oracle; prepare the documented venv')
        archived = command(['git', '-C', args.backend_repo.resolve(), 'archive', backend['sha'], '--', 'app', 'common', 'tests', 'requirements.txt'])
        if archived.returncode:
            raise RuntimeError('Pinned source revision cannot be archived')
        with tempfile.TemporaryDirectory(prefix='aip-face-runner-') as temp:
            source = Path(temp)/'source'; source.mkdir()
            safe_extract(archived.stdout, source)
            if (source/'.env').exists():
                raise RuntimeError('Pinned source unexpectedly contains .env')
            def start():
                return command([*compose, 'up', '-d', '--wait', '--wait-timeout', '180'], timeout=240).returncode
            def execute():
                return command([args.python, root/'tools/facereview_baseline.py', '--source', source,
                                '--inventory', output/'inventory.json', '--output', output/'result.json'], timeout=120).returncode
            def stop():
                p = command([*compose, 'down', '--volumes'], timeout=60)
                if p.returncode == 0:
                    remaining = command([*compose, 'ps', '-aq'])
                    if remaining.returncode or not clean_project(remaining.stdout.decode()):
                        manifest['cleanup_exit_code'] = -1
                        return -1
                manifest['cleanup_exit_code'] = p.returncode
                return p.returncode
            manifest.update(owned_lifecycle(start, execute, stop))
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as exc:
        # Explicit messages have no captured values. Timeouts/OS errors use only a type label.
        error = str(exc) if type(exc) is RuntimeError else type(exc).__name__
    finally:
        for name in [*inputs, 'result.json']:
            path = output/name
            if path.is_file():
                manifest['files'][name] = {'path': name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        (output/'manifest.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2)+'\n')
    errors = check_evidence(manifest, output)
    if error:
        errors.append(error)
    print(json.dumps({'ok': not errors, 'project': project, 'manifest': str(output/'manifest.json'), 'errors': sorted(errors)}, ensure_ascii=False))
    return 1 if errors else 0


if __name__ == '__main__':
    raise SystemExit(main())
