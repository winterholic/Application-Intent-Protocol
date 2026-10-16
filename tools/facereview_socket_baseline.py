"""Record a narrow, real Socket.IO polling baseline against pinned FaceReview."""
import argparse
import hashlib
import json
from pathlib import Path
import sys

PINNED_BACKEND_SHA = '7948b7e721837fdb73c427cb54e12cad4ec2d5af'
ALLOWED_ORIGIN = 'http://localhost:5173'
ALLOWED_ENDPOINT = 'http://127.0.0.1:23175'
EXPECTED = {
    'kind', 'source_sha', 'scope', 'aip_ported', 'ml', 'transport', 'origin',
    'connect', 'disconnect', 'watch_frame', 'frame_payload_sent',
    'writes_expected', 'complete',
}


def check_socket_report(report):
    errors = []
    if not isinstance(report, dict) or set(report) != EXPECTED:
        return ['report fields do not match the narrow socket contract']
    checks = {
        'kind': 'original-socket', 'source_sha': PINNED_BACKEND_SHA,
        'scope': 'original-socket', 'aip_ported': False, 'ml': 'not-executed',
        'transport': 'polling', 'origin': ALLOWED_ORIGIN, 'connect': 'pass',
        'disconnect': 'transport-closed',
        'watch_frame': {'status': 'error', 'code': 'Missing required fields'},
        'frame_payload_sent': False, 'writes_expected': False, 'complete': False,
    }
    for key, value in checks.items():
        if report.get(key) != value:
            errors.append(f'{key} does not match the expected original-only observation')
    return errors


def verify_source(source, inventory_path):
    inventory = json.loads(inventory_path.read_text())
    backend = next(repo for repo in inventory['repositories'] if repo['role'] == 'backend')
    if backend['sha'] != PINNED_BACKEND_SHA:
        raise RuntimeError('Inventory backend SHA differs from the pinned source')
    for file in backend['files']:
        if not file['path'].endswith('.py'):
            continue
        path = source / file['path']
        raw = path.read_bytes().replace(b'\r\n', b'\n')
        if hashlib.sha256(raw).hexdigest() != file['sha256']:
            raise RuntimeError('Source differs from pinned revision: ' + file['path'])
    if (source / '.env').exists():
        raise RuntimeError('Source copy must not contain .env')


def run_baseline(endpoint, origin):
    if endpoint != ALLOWED_ENDPOINT:
        raise ValueError('Endpoint must be the isolated loopback original server')
    if origin != ALLOWED_ORIGIN:
        raise ValueError('Origin must be the pinned allowed local frontend origin')
    import socketio

    client = socketio.Client(logger=False, engineio_logger=False)
    report = {
        'kind': 'original-socket', 'source_sha': PINNED_BACKEND_SHA,
        'scope': 'original-socket', 'aip_ported': False, 'ml': 'not-executed',
        'transport': 'polling', 'origin': origin, 'connect': 'failed',
        'disconnect': 'not-run',
        'watch_frame': {'status': 'not-run', 'code': 'not-run'},
        'frame_payload_sent': False, 'writes_expected': False, 'complete': False,
    }
    try:
        client.connect(endpoint, headers={'Origin': origin}, wait_timeout=8,
                       transports=['polling'])
        report['connect'] = 'pass'
        response = client.call('watch_frame', {}, timeout=8)
        if isinstance(response, dict) and response.get('status') == 'error' and \
                response.get('message') == 'Missing required fields':
            report['watch_frame'] = {'status': 'error', 'code': 'Missing required fields'}
        else:
            report['watch_frame'] = {'status': 'unexpected', 'code': 'unexpected-response'}
    finally:
        if client.connected:
            client.disconnect()
            report['disconnect'] = 'transport-closed'
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--inventory', type=Path, required=True)
    parser.add_argument('--endpoint', default=ALLOWED_ENDPOINT)
    parser.add_argument('--origin', default=ALLOWED_ORIGIN)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error('--output must not already exist')
    args.source = args.source.resolve()
    args.output = args.output.resolve()
    verify_source(args.source, args.inventory)
    report = run_baseline(args.endpoint, args.origin)
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    errors = check_socket_report(report)
    print(json.dumps({'ok': not errors, 'errors': errors, 'report': str(args.output)}, ensure_ascii=False))
    return 0 if not errors else 1


if __name__ == '__main__':
    sys.exit(main())
