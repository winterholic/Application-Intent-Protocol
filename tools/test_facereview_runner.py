from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).parent))
from facereview_runner import owned_lifecycle, safe_extract, clean_project


class RunnerTest(unittest.TestCase):
    def run_lifecycle(self, up=0, api=0, down=0):
        events = []
        def start(): events.append('up'); return up
        def execute(): events.append('api'); return api
        def stop(): events.append('down'); return down
        result = owned_lifecycle(start, execute, stop)
        return result, events

    def test_success_runs_in_order_and_cleans_up(self):
        result, events = self.run_lifecycle()
        self.assertEqual(events, ['up', 'api', 'down'])
        self.assertEqual(result, {'api_exit_code': 0, 'cleanup_exit_code': 0})

    def test_up_failure_skips_api_but_still_cleans_owned_project(self):
        result, events = self.run_lifecycle(up=1)
        self.assertEqual(events, ['up', 'down'])
        self.assertNotEqual(result['api_exit_code'], 0)

    def test_api_and_cleanup_failures_are_preserved(self):
        result, events = self.run_lifecycle(api=7, down=8)
        self.assertEqual(events, ['up', 'api', 'down'])
        self.assertEqual(result, {'api_exit_code': 7, 'cleanup_exit_code': 8})

    def test_exception_during_api_still_cleans_up(self):
        events = []
        def fail(): raise RuntimeError('fixture failure')
        with self.assertRaises(RuntimeError):
            owned_lifecycle(lambda: 0, fail, lambda: events.append('down') or 0)
        self.assertEqual(events, ['down'])

    def test_existing_stopped_or_running_resources_are_not_ours(self):
        self.assertFalse(clean_project('abc123\n'))
        self.assertFalse(clean_project(' abc123 '))
        self.assertTrue(clean_project(''))

    def test_archive_extract_rejects_escape_and_links(self):
        import io, tarfile
        for name, link in [('../escape.py', False), ('/absolute.py', False), ('app/link', True)]:
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode='w') as tar:
                member = tarfile.TarInfo(name)
                if link: member.type = tarfile.SYMTYPE; member.linkname = '../../outside'
                tar.addfile(member)
            with tempfile.TemporaryDirectory() as directory:
                with self.assertRaises(ValueError): safe_extract(archive.getvalue(), Path(directory))

    def test_normal_archive_extracts(self):
        import io, tarfile
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode='w') as tar:
            member = tarfile.TarInfo('app/test.py'); member.size = 4
            tar.addfile(member, io.BytesIO(b'pass'))
        with tempfile.TemporaryDirectory() as directory:
            safe_extract(archive.getvalue(), Path(directory))
            self.assertEqual((Path(directory)/'app/test.py').read_text(), 'pass')


if __name__ == '__main__': unittest.main()
