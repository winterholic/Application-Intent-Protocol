#!/usr/bin/env python3
"""Preview an explicit original-ID import, or emit its approval-bound SQL.

Input is {manifest, snapshot, target:{schema,facts_digest,structure_digest}}.
Default: JSON preview. --apply DIGEST: SQL for psql -X -v ON_ERROR_STOP=1.
--snapshot-sql takes {schema,source} and emits a read-only export query.
No connection or activation is performed by this stdlib tool. Source provenance
must be reviewed externally; copied namespace/revision claims are not proof.
Only an initialized, AIP-owned PostgreSQL schema with CACHE 1 is supported.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import sys

_spec = importlib.util.spec_from_file_location('post_order_manifest', Path(__file__).with_name('post_order_manifest.py'))
manifest_tool = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(manifest_tool)


def _schema(value):
    if not isinstance(value, str) or not re.fullmatch(r'[a-z_][a-z0-9_]{0,42}', value):
        raise ValueError('schema: expected a lowercase AIP schema identifier, at most 43 characters')
    return value


def _literal(value):
    return "'" + str(value).replace("'", "''") + "'"


def _json(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True)


def _snapshot_expr(schema, source):
    q = '"' + _schema(schema) + '"'
    source = manifest_tool._source(source)
    return f"""jsonb_build_object('version',1,'source',{_literal(_json(source))}::jsonb,
 'posts',COALESCE((SELECT jsonb_agg(jsonb_build_object('post_uuid',id::text,'published',published,
   'source_order',source_order_id) ORDER BY id) FROM {q}.post),'[]'::jsonb),
 'mappings',COALESCE((SELECT jsonb_agg(jsonb_build_object('source_id',id,'post_uuid',post_id::text)
   ORDER BY id) FROM {q}.post_order),'[]'::jsonb),
 'generator',(SELECT jsonb_build_object('last_value',last_value,'is_called',is_called)
   FROM {q}.post_order_id_seq))"""


def snapshot_sql(schema, source):
    return 'SET standard_conforming_strings=on;\nSELECT ' + _snapshot_expr(schema, source) + ';\n'


def preview(request):
    manifest_tool._object(request, 'manifest snapshot target', 'request')
    target = manifest_tool._object(request['target'], 'schema facts_digest structure_digest', 'target')
    _schema(target['schema'])
    for key in ('facts_digest', 'structure_digest'):
        if not isinstance(target[key], str) or not re.fullmatch('[a-f0-9]{64}', target[key]):
            raise ValueError(key + ': expected lowercase SHA-256')
    plan = manifest_tool.preflight(request['manifest'], request['snapshot'])
    payload = {'version': 1, 'target': dict(target), 'import_plan': plan,
               'activation_authorized': False, 'source_provenance_verified': False}
    return {**payload, 'digest': manifest_tool._digest(payload)}


def apply_sql(request, acknowledgement):
    report = preview(request)
    if acknowledgement != report['digest']:
        raise ValueError('PLAN_CHANGED: reviewed target, snapshot or manifest changed')
    plan, target = report['import_plan'], report['target']
    schema = target['schema']
    q = '"' + schema + '"'
    snapshot = request['snapshot']
    # Match the SQL export's deterministic row ordering, independent of JSON key order.
    expected = {**snapshot, 'posts': sorted(snapshot['posts'], key=lambda row: row['post_uuid']),
                'mappings': sorted(snapshot['mappings'], key=lambda row: row['source_id'])}
    body = f"""DECLARE actual jsonb;
BEGIN
 IF NOT EXISTS (SELECT 1 FROM {q}.aip_migrate_meta WHERE singleton=true
   AND facts_digest={_literal(target['facts_digest'])} AND structure_digest={_literal(target['structure_digest'])}
   AND (facts->'externalStorage' IS NULL OR facts->'externalStorage'='null'::jsonb)) THEN
   RAISE EXCEPTION 'POST_ORDER_TARGET_CHANGED';
 END IF;
 IF NOT EXISTS (SELECT 1 FROM pg_sequence WHERE seqrelid={_literal(q+'.post_order_id_seq')}::regclass
   AND seqincrement=1 AND seqcache=1 AND NOT seqcycle AND seqmax=9223372036854775807) THEN
   RAISE EXCEPTION 'POST_ORDER_ALLOCATOR_UNSUPPORTED';
 END IF;
 SELECT {_snapshot_expr(schema, plan['source'])} INTO actual;
 IF actual IS DISTINCT FROM {_literal(_json(expected))}::jsonb THEN
   RAISE EXCEPTION 'POST_ORDER_SNAPSHOT_CHANGED';
 END IF;
"""
    for row in plan['insert_mappings']:
        body += f" INSERT INTO {q}.post_order(id,post_id) OVERRIDING SYSTEM VALUE VALUES ({row['source_id']},{_literal(row['post_uuid'])}::uuid);\n"
    for row in plan['link_posts']:
        body += f" UPDATE {q}.post SET source_order_id={row['source_id']} WHERE id={_literal(row['post_uuid'])}::uuid;\n"
    # Transactional restart avoids setval's non-transactional rollback behavior.
    # The approved live generator is rechecked above while holding the product
    # deployment fence; CACHE 1 prevents another listener retaining older IDs.
    body += f" ALTER SEQUENCE {q}.post_order_id_seq RESTART WITH {plan['next_id_min']};\nEND;"
    return f"""BEGIN;
SET LOCAL standard_conforming_strings=on;
SET LOCAL lock_timeout='2s';
SET LOCAL statement_timeout='20s';
SELECT pg_advisory_xact_lock(1095323725,hashtext({_literal(schema)}));
LOCK TABLE {q}.aip_migrate_meta, {q}.post, {q}.post_order IN ACCESS EXCLUSIVE MODE;
DO {_literal(body)};
COMMIT;
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument('--apply', metavar='DIGEST')
    modes.add_argument('--snapshot-sql', action='store_true')
    args = parser.parse_args()
    try:
        payload = sys.stdin.buffer.read(manifest_tool.MAX_INPUT_BYTES + 1)
        if len(payload) > manifest_tool.MAX_INPUT_BYTES:
            raise ValueError('input too large')
        request = manifest_tool.loads(payload)
        if args.snapshot_sql:
            manifest_tool._object(request, 'schema source', 'snapshot request')
            result = snapshot_sql(request['schema'], request['source'])
        elif args.apply is not None:
            result = apply_sql(request, args.apply)
        else:
            result = _json(preview(request)) + '\n'
    except (ValueError, RecursionError) as error:
        print(f'PostOrder import rejected: {error}', file=sys.stderr)
        return 1
    sys.stdout.write(result)
    return 0


if __name__ == '__main__':
    sys.exit(main())
