import unittest
from facereview_browser import allowed_request, request_record, check_browser_report, PLANNED
from copy import deepcopy


class BrowserBoundaryTests(unittest.TestCase):
    def test_only_owned_loopback_origins_are_allowed(self):
        self.assertTrue(allowed_request('http://127.0.0.1:23173/main'))
        self.assertTrue(allowed_request('ws://127.0.0.1:23175/socket.io'))
        self.assertTrue(allowed_request('http://host.docker.internal:23173/main'))
        for url in ['https://facereview.net', 'http://127.0.0.1:8000',
                    'http://127.0.0.1.evil.test:23173', 'http://user@127.0.0.1:23173']:
            self.assertFalse(allowed_request(url), url)

    def test_network_record_drops_query_auth_and_response_content(self):
        row = request_record('POST', 'http://127.0.0.1:23173/api?token=secret', 200)
        self.assertEqual(row, {'method':'POST', 'origin':'http://127.0.0.1:23173', 'path':'/api', 'status':200})
        self.assertNotIn('secret', str(row))

    def test_partial_browser_gate_rejects_missing_or_duplicate_cases_and_scope_inflation(self):
        valid=dict(scope='original-browser',aip_ported=False,ml='not-executed',complete=False,
                   cases=[dict(role=r,label=l,result='pass') for r,l in sorted(PLANNED)],
                   network=[request_record('POST','http://127.0.0.1:23173/api/v2/auth/login',200)],page_errors=[])
        self.assertEqual(check_browser_report(valid),[])
        for mutation in ['missing','duplicate','aip','complete','external-api','page-error']:
            bad=deepcopy(valid)
            if mutation=='missing': bad['cases'].pop()
            if mutation=='duplicate': bad['cases'].append(bad['cases'][0])
            if mutation=='aip': bad['aip_ported']=True
            if mutation=='complete': bad['complete']=True
            if mutation=='external-api': bad['network'].append(request_record('GET','https://facereview.net/api/private',200))
            if mutation=='page-error': bad['page_errors'].append({'kind':'Error'})
            with self.subTest(mutation=mutation): self.assertTrue(check_browser_report(bad))


if __name__ == '__main__':
    unittest.main()
