import unittest
from copy import deepcopy

from facereview_socket_baseline import check_socket_report, run_baseline


def valid_report():
    return {
        'kind': 'original-socket',
        'source_sha': '7948b7e721837fdb73c427cb54e12cad4ec2d5af',
        'scope': 'original-socket',
        'aip_ported': False,
        'ml': 'not-executed',
        'transport': 'polling',
        'origin': 'http://localhost:5173',
        'connect': 'pass',
        'disconnect': 'transport-closed',
        'watch_frame': {'status': 'error', 'code': 'Missing required fields'},
        'frame_payload_sent': False,
        'writes_expected': False,
        'complete': False,
    }


class SocketBaselineTests(unittest.TestCase):
    def test_accepts_only_original_polling_missing_frame_baseline(self):
        self.assertEqual(check_socket_report(valid_report()), [])

    def test_rejects_incomplete_or_scope_inflated_socket_evidence(self):
        for name, change in [
            ('wrong source', lambda r: r.update(source_sha='different')),
            ('connect failed', lambda r: r.update(connect='failed')),
            ('disconnect failed', lambda r: r.update(disconnect='failed')),
            ('wrong handler result', lambda r: r.update(watch_frame={'status': 'success'})),
            ('frame sent', lambda r: r.update(frame_payload_sent=True)),
            ('ML claimed', lambda r: r.update(ml='executed')),
            ('whole gate claimed', lambda r: r.update(complete=True)),
            ('wrong origin', lambda r: r.update(origin='http://host.docker.internal:23173')),
        ]:
            report = deepcopy(valid_report())
            change(report)
            with self.subTest(name=name):
                self.assertTrue(check_socket_report(report))

    def test_rejects_sensitive_values_and_unexpected_fields(self):
        report = valid_report()
        report['token'] = 'should-not-be-recorded'
        self.assertTrue(check_socket_report(report))

    def test_runner_rejects_non_loopback_endpoint_and_unapproved_origin(self):
        for endpoint, origin in [
            ('https://facereview.net', 'http://localhost:5173'),
            ('http://127.0.0.1:23175', 'http://host.docker.internal:23173'),
        ]:
            with self.subTest(endpoint=endpoint, origin=origin):
                with self.assertRaises(ValueError):
                    run_baseline(endpoint, origin)


if __name__ == '__main__':
    unittest.main()
