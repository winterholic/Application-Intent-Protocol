"""Compare a complete ORIGINAL API run to an independently reviewed oracle.

Mismatch diagnostics name fields, never actual responses or credentials.
"""
import argparse
import json
import re
from pathlib import Path


def semantic_checks(label, data):
    """Export only explicitly selected synthetic semantics, never auth payloads."""
    if not isinstance(data, dict):
        return {'json_object': False}
    if label in {'login-user', 'login-admin', 'source-revoked-refresh-replay'}:
        return {'access_present': isinstance(data.get('access_token'), str) and bool(data['access_token'])}
    if label in {'me', 'source-access-token-lives-until-expiry'}:
        return {key: data.get(key) for key in ['user_id', 'role', 'name', 'is_tutorial_done']}
    if label == 'health':
        return {key: data.get(key) for key in ['status', 'service']}
    if label == 'admin-users':
        return {'total': data.get('total'), 'roles': sorted(user.get('role', '') for user in data.get('users', []))}
    if label == 'guest-video-detail':
        return {key: data.get(key) for key in ['video_id', 'title', 'category', 'duration', 'like_count', 'comment_count', 'user_is_liked', 'is_bookmarked']}
    if label == 'create-comment':
        return {'comment_id_is_null': data.get('comment_id') is None}
    if label == 'list-comments':
        return {'total': data.get('total'), 'comments': [{key: row.get(key) for key in ['user_id', 'content', 'is_modified', 'is_mine']} for row in data.get('comments', [])]}
    if label.startswith('like-toggle-'):
        return {key: data.get(key) for key in ['is_liked', 'like_count']}
    if label.startswith('bookmark-toggle-'):
        return {'is_bookmarked': data.get('is_bookmarked')}
    if label in {'tutorial', 'edit-comment', 'delete-comment', 'logout'}:
        return {'result': data.get('result')}
    return {}


def canonical_path(path):
    if not isinstance(path, str):
        return path
    return re.sub(r'(comment_id=)[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}(?=&|$)', r'\1$comment', path)


def sensitive_fields(value, path='result'):
    errors = []
    if isinstance(value, dict):
        for key, item in value.items():
            if key.lower() in {'access_token', 'refresh_token', 'authorization', 'cookie', 'set-cookie', 'password', 'password_hash'}:
                errors.append(f'sensitive field {path}.{key}')
            else:
                errors.extend(sensitive_fields(item, f'{path}.{key}'))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            errors.extend(sensitive_fields(item, f'{path}[{index}]'))
    return errors


def indexed(rows, noun, errors):
    if not isinstance(rows, list):
        errors.append(f'{noun} rows must be an array')
        return {}
    found = {}
    for row in rows:
        label = row.get('label') if isinstance(row, dict) else None
        if not isinstance(label, str) or not label:
            errors.append(f'{noun} row has no label')
        elif label in found:
            errors.append(f'duplicate {noun} {label}')
        else:
            found[label] = row
    return found


def compare_result(result, oracle):
    errors = []
    if not isinstance(result, dict) or not isinstance(oracle, dict):
        return ['result and oracle must be objects']
    if not oracle.get('cases') or not isinstance(oracle.get('states'), dict) or not oracle.get('states'):
        return ['oracle must contain planned cases and states']
    errors.extend(sensitive_fields(result))
    if result.get('completed') is not True:
        errors.append('completed must be true')
    if result.get('scope') != 'original-api' or result.get('aip_ported') is not False:
        errors.append('scope must remain original-api, not an AIP port')
    for key in ['source_sha', 'runtime', 'packages', 'expected_product_http']:
        if result.get(key) != oracle.get(key):
            errors.append(f'{key} mismatch')
    if result.get('missing_registered_routes') != []:
        errors.append('missing_registered_routes must be empty')
    plan = indexed(oracle['cases'], 'planned case', errors)
    actual = indexed(result.get('observations'), 'case', errors)
    for label in plan.keys() - actual.keys():
        errors.append(f'missing case {label}')
    for label in actual.keys() - plan.keys():
        errors.append(f'unplanned case {label}')
    for label in plan.keys() & actual.keys():
        wanted, got = plan[label], actual[label]
        for key in ['method', 'path', 'status', 'response_keys', 'code', 'checks']:
            value = canonical_path(got.get(key)) if key == 'path' else got.get(key)
            expected = canonical_path(wanted.get(key)) if key == 'path' else wanted.get(key)
            # JSON bool and int must not compare as equal (True != 1 in this contract).
            if json.dumps(value, sort_keys=True) != json.dumps(expected, sort_keys=True):
                errors.append(f'{label}.{key} mismatch')
    states = indexed(result.get('state_checks'), 'state', errors)
    for label, wanted in oracle['states'].items():
        if label not in states:
            errors.append(f'missing state {label}')
        elif json.dumps(states[label].get('observed'), sort_keys=True) != json.dumps(wanted, sort_keys=True):
            errors.append(f'state {label} mismatch')
    for label in states.keys() - oracle['states'].keys():
        errors.append(f'unplanned state {label}')
    anomalies = result.get('original_anomalies')
    if not isinstance(anomalies, list) or sorted(a.get('case', '') for a in anomalies if isinstance(a, dict)) != sorted(oracle.get('anomalies', [])):
        errors.append('original_anomalies mismatch')
    return sorted(errors)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--result', type=Path, required=True)
    parser.add_argument('--oracle', type=Path, required=True)
    args = parser.parse_args()
    try:
        result = json.loads(args.result.read_text())
        oracle = json.loads(args.oracle.read_text())
        errors = compare_result(result, oracle)
    except (OSError, ValueError, TypeError):
        errors = ['result/oracle file missing or malformed']
    print(json.dumps({'ok': not errors, 'errors': errors}, ensure_ascii=False))
    return 1 if errors else 0


if __name__ == '__main__':
    raise SystemExit(main())
