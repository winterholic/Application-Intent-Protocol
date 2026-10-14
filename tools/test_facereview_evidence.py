import copy
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).parent))
import test_facereview_oracle
from facereview_evidence import check_evidence


class EvidenceTest(unittest.TestCase):
    def setUp(self):
        fixture = test_facereview_oracle.OracleTest(); fixture.setUp()
        self.oracle, self.result = fixture.oracle, fixture.result
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name, value in [('result.json', self.result), ('oracle.json', self.oracle), ('inventory.json', {'repositories': [{'role': 'backend', 'sha': 'a' * 40}]})]:
            (self.root / name).write_text(json.dumps(value))
        (self.root / 'lock.txt').write_text('Flask==3.0\n')
        (self.root / 'compose.yaml').write_text('services: {}\n')
        self.manifest = {'kind': 'original-api', 'source_sha': 'a' * 40,
            'api_exit_code': 0, 'cleanup_exit_code': 0,
            'scope': {'api': 'executed', 'browser': 'not-executed', 'ml': 'not-executed', 'aip_port': 'not-executed'},
            'files': {name: {'path': name, 'sha256': hashlib.sha256((self.root / name).read_bytes()).hexdigest()} for name in ['result.json', 'oracle.json', 'inventory.json', 'lock.txt', 'compose.yaml']},
            'commands': [{'argv': ['python3', 'facereview_baseline.py'], 'exit_code': 0}]}

    def errors(self, manifest=None):
        return check_evidence(manifest or self.manifest, self.root)

    def test_good_evidence_passes(self):
        self.assertEqual(self.errors(), [])

    def test_api_failure_cleanup_failure_and_missing_exit_fail(self):
        for name in ['api_exit_code', 'cleanup_exit_code']:
            for value in [1, None]:
                manifest = copy.deepcopy(self.manifest); manifest[name] = value
                self.assertTrue(any(name in e for e in self.errors(manifest)))

    def test_missing_tampered_or_outside_artifact_fails(self):
        (self.root / 'result.json').write_text('{}')
        self.assertTrue(any('result.json' in e for e in self.errors()))
        (self.root / 'result.json').unlink()
        self.assertTrue(any('result.json' in e for e in self.errors()))
        manifest = copy.deepcopy(self.manifest); manifest['files']['result.json']['path'] = '../outside.json'
        self.assertTrue(any('outside' in e for e in self.errors(manifest)))

    def test_partial_result_fails_even_when_hash_is_updated(self):
        self.result['completed'] = False
        file = self.root / 'result.json'; file.write_text(json.dumps(self.result))
        self.manifest['files']['result.json']['sha256'] = hashlib.sha256(file.read_bytes()).hexdigest()
        self.assertTrue(any('completed' in e for e in self.errors()))

    def test_scope_and_source_cannot_be_promoted(self):
        manifest = copy.deepcopy(self.manifest); manifest['scope']['browser'] = 'executed'
        self.assertTrue(any('scope' in e for e in self.errors(manifest)))
        manifest = copy.deepcopy(self.manifest); manifest['source_sha'] = 'b' * 40
        self.assertTrue(any('source_sha' in e for e in self.errors(manifest)))

    def test_missing_required_provenance_fails(self):
        for name in ['oracle.json', 'inventory.json', 'lock.txt', 'compose.yaml']:
            manifest = copy.deepcopy(self.manifest); del manifest['files'][name]
            self.assertTrue(any(name in e for e in self.errors(manifest)))
        manifest = copy.deepcopy(self.manifest); manifest['commands'] = []
        self.assertTrue(any('commands' in e for e in self.errors(manifest)))

    def test_valid_json_with_wrong_shape_fails(self):
        file = self.root / 'result.json'; file.write_text('null')
        self.manifest['files']['result.json']['sha256'] = hashlib.sha256(file.read_bytes()).hexdigest()
        self.assertTrue(self.errors())

    def test_changed_lock_is_detected_even_with_updated_manifest_hash(self):
        file = self.root / 'lock.txt'; file.write_text('Flask==9.0\n')
        self.manifest['files']['lock.txt']['sha256'] = hashlib.sha256(file.read_bytes()).hexdigest()
        self.assertTrue(any('lock packages' in error for error in self.errors()))

    def test_sensitive_command_values_are_not_echoed(self):
        manifest = copy.deepcopy(self.manifest)
        manifest['commands'][0]['argv'] += ['Authorization=never-print-this-value']
        errors = self.errors(manifest)
        self.assertTrue(any('sensitive command' in e for e in errors))
        self.assertNotIn('never-print-this-value', json.dumps(errors))


if __name__ == '__main__':
    unittest.main()
