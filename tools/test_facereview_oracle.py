import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).parent))
from facereview_oracle import compare_result, semantic_checks, canonical_path


class OracleTest(unittest.TestCase):
    def setUp(self):
        self.oracle = {'source_sha': 'a' * 40, 'cases': [{
            'label': 'read', 'method': 'GET', 'path': '/api/read',
            'status': 200, 'response_keys': ['total'], 'code': None,
            'checks': {'total': 2}}], 'states': {'rows': 2},
            'anomalies': [], 'runtime': {'mysql': '8.4'}, 'packages': ['Flask==3.0'],
            'expected_product_http': 49}
        self.result = {'source_sha': 'a' * 40, 'completed': True,
            'aip_ported': False, 'observations': copy.deepcopy(self.oracle['cases']),
            'state_checks': [{'label': 'rows', 'observed': 2}],
            'original_anomalies': [], 'runtime': {'mysql': '8.4'},
            'packages': ['Flask==3.0'], 'expected_product_http': 49,
            'missing_registered_routes': [], 'scope': 'original-api',
            'browser': 'not executed', 'ml': 'not executed'}

    def check_failure(self, mutate, culprit):
        result = copy.deepcopy(self.result)
        mutate(result)
        errors = compare_result(result, self.oracle)
        self.assertTrue(any(culprit in error for error in errors), errors)

    def test_good_result_passes(self):
        self.assertEqual(compare_result(self.result, self.oracle), [])

    def test_auth_projection_never_exports_token_values(self):
        self.assertEqual(semantic_checks('login-user', {'access_token': 'never-export'}), {'access_present': True})
        self.assertEqual(semantic_checks('login-user', {}), {'access_present': False})

    def test_only_the_generated_comment_query_id_is_normalized(self):
        self.assertEqual(canonical_path('/api/comments?comment_id=11111111-2222-3333-4444-555555555555'), '/api/comments?comment_id=$comment')
        self.assertEqual(canonical_path('/api/unknown?video_id=11111111-2222-3333-4444-555555555555'), '/api/unknown?video_id=11111111-2222-3333-4444-555555555555')

    def test_missing_duplicate_and_unplanned_cases_fail(self):
        self.check_failure(lambda r: r['observations'].clear(), 'missing case read')
        self.check_failure(lambda r: r['observations'].append(r['observations'][0]), 'duplicate case read')
        self.check_failure(lambda r: r['observations'].append({'label': 'extra'}), 'unplanned case extra')

    def test_wrong_status_shape_semantics_and_state_fail(self):
        self.check_failure(lambda r: r['observations'][0].update(status=403), 'read.status')
        self.check_failure(lambda r: r['observations'][0].update(response_keys=[]), 'read.response_keys')
        self.check_failure(lambda r: r['observations'][0]['checks'].update(total=0), 'read.checks')
        self.check_failure(lambda r: r['state_checks'][0].update(observed=0), 'state rows')

    def test_mutating_embedded_expected_does_not_override_oracle(self):
        self.check_failure(lambda r: r['observations'][0].update(status=403, expected=403), 'read.status')

    def test_partial_source_environment_and_scope_fail(self):
        self.check_failure(lambda r: r.update(completed=False), 'completed')
        self.check_failure(lambda r: r.update(source_sha='b' * 40), 'source_sha')
        self.check_failure(lambda r: r['runtime'].update(mysql='9.0'), 'runtime')
        self.check_failure(lambda r: r.update(packages=[]), 'packages')
        self.check_failure(lambda r: r.update(scope='aip-port', aip_ported=True), 'scope')
        self.check_failure(lambda r: r.update(missing_registered_routes=[['GET', '/missing']]), 'missing_registered_routes')

    def test_missing_and_duplicate_storage_oracles_fail(self):
        self.check_failure(lambda r: r['state_checks'].clear(), 'missing state rows')
        self.check_failure(lambda r: r['state_checks'].append(r['state_checks'][0]), 'duplicate state rows')

    def test_sensitive_fields_are_rejected_without_echoing_values(self):
        result = copy.deepcopy(self.result)
        result['observations'][0]['checks']['access_token'] = 'never-print-this-value'
        errors = compare_result(result, self.oracle)
        self.assertTrue(any('sensitive field' in error for error in errors), errors)
        self.assertNotIn('never-print-this-value', json.dumps(errors))

    def test_malformed_result_is_a_failure(self):
        self.assertTrue(compare_result({}, self.oracle))
        self.assertTrue(compare_result({'observations': 'wrong'}, self.oracle))


if __name__ == '__main__':
    unittest.main()
