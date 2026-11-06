"""Approval-bound SQL emitter; actual DB/fence tests live in post-order-import.test.mjs."""
import copy
import importlib.util
from pathlib import Path
import unittest

PATH = Path(__file__).resolve().parents[1] / 'seed/post_order_import.py'
A = 'ffffffff-ffff-4fff-8fff-ffffffffffff'
B = '00000000-0000-4000-8000-000000000001'

class ImportTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(PATH.is_file(), 'the historical import executor is missing')
        spec = importlib.util.spec_from_file_location('post_order_import', PATH)
        self.tool = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.tool)
        source = {'namespace': "original ' export", 'revision': 'a'*40, 'high_water': 80}
        self.request = {'manifest': {'version': 1, 'source': source, 'posts': [{'source_id': 41, 'post_uuid': A}]},
          'snapshot': {'version': 1, 'source': source, 'posts': [
            {'post_uuid': A, 'published': True, 'source_order': None},
            {'post_uuid': B, 'published': False, 'source_order': None}],
            'mappings': [], 'generator': {'last_value': 1, 'is_called': False}},
          'target': {'schema': 'aip_test_import', 'facts_digest': 'b'*64, 'structure_digest': 'c'*64}}

    def test_preview_binds_target_and_never_activates_or_mutates_input(self):
        before = copy.deepcopy(self.request)
        plan = self.tool.preview(self.request)
        self.assertFalse(plan['activation_authorized'])
        self.assertEqual(plan['target'], self.request['target'])
        for field, value in [('schema','aip_other_import'), ('facts_digest','d'*64), ('structure_digest','e'*64)]:
            changed = copy.deepcopy(self.request)
            changed['target'][field] = value
            self.assertNotEqual(self.tool.preview(changed)['digest'], plan['digest'])
        self.assertEqual(self.request, before)

    def test_wrong_approval_is_rejected_before_sql_generation(self):
        with self.assertRaisesRegex(ValueError, 'PLAN_CHANGED'):
            self.tool.apply_sql(self.request, '0'*64)

    def test_sql_fences_and_revalidates_before_any_effect(self):
        sql = self.tool.apply_sql(self.request, self.tool.preview(self.request)['digest'])
        fence = sql.index('pg_advisory_xact_lock(1095323725')
        lock = sql.index('LOCK TABLE')
        compare = sql.index('POST_ORDER_SNAPSHOT_CHANGED')
        insert = sql.index('INSERT INTO')
        update = sql.index('UPDATE ')
        restart = sql.index('RESTART WITH 81')
        self.assertLess(fence, lock)
        self.assertLess(lock, compare)
        self.assertLess(compare, insert)
        self.assertLess(insert, update)
        self.assertLess(update, restart)
        self.assertTrue(sql.rstrip().endswith('COMMIT;'))
        self.assertIn("END;';", sql)
        self.assertNotIn('setval(', sql)
        self.assertNotIn('date_created', sql)
        self.assertIn('POST_ORDER_TARGET_CHANGED', sql)
        self.assertIn('seqcache', sql)

    def test_unsafe_or_incomplete_target_and_partial_coverage_are_rejected(self):
        for target in [{}, {**self.request['target'], 'schema': 'x; DROP SCHEMA public'},
                       {**self.request['target'], 'facts_digest': 'not-a-digest'}]:
            with self.subTest(target=target), self.assertRaises(ValueError):
                self.tool.preview({**self.request, 'target': target})
        self.request['manifest']['posts'] = []
        with self.assertRaisesRegex(ValueError, 'coverage'):
            self.tool.preview(self.request)

    def test_digest_normalizes_row_order_and_sql_quotes_untrusted_namespace(self):
        plan = self.tool.preview(self.request)
        self.request['snapshot']['posts'].reverse()
        self.assertEqual(self.tool.preview(self.request)['digest'], plan['digest'])
        sql = self.tool.snapshot_sql(self.request['target']['schema'], self.request['manifest']['source'])
        self.assertIn("original '' export", sql)
        self.assertIn('ORDER BY id', sql)
        self.assertIn('post_order_id_seq', sql)

if __name__ == '__main__':
    unittest.main()
