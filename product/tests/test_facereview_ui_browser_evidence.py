import json
import ast
from pathlib import Path
import unittest
from pathlib import Path
import subprocess
import tempfile

from product.tests.facereview_ui_browser_evidence import (
    admin_request_evidence,
    aggregate_count,
    safe_page_error_name,
    extension_request_evidence,
    extension_response_evidence,
    external_media_failure_evidence,
    fixture_thumbnail_redirect_url,
    failed_read_requests,
    console_error_evidence,
    read_request_evidence,
    safe_browser_caller_frames,
    safe_browser_failure_type,
    safe_browser_route_class,
    safe_bookmark_control_evidence,
    safe_wire_code,
    youtube_iframe_api_stub,
    response_envelope_evidence,
    auth_transport_trace_evidence,
    body_capture_evidence,
    body_failure_reason,
    profile_render_evidence,
    profile_content_animation_incomplete,
    app_bookmark_read_observation,
    compare_bookmark_read_capture,
    timeline_phase,
    wait_for_api_completion,
)


VIDEO_ID = "aaaaaaaa-0000-4000-8000-000000000001"


class BrowserEvidenceTests(unittest.TestCase):
    def test_admin_signup_trend_evidence_checks_every_bucket_and_names_wrong_date(self):
        from product.tests.facereview_ui_browser_evidence import admin_signup_svg_evidence, admin_signup_trend_evidence

        expected = [{"date": "2026-11-01", "count": 3}, {"date": "2026-11-02", "count": 0}]
        self.assertEqual(admin_signup_trend_evidence("7d", expected, expected), {
            "period": "7d", "expected_buckets": 2, "actual_buckets": 2,
            "expected_total": 3, "actual_total": 3, "expected_points": expected, "actual_points": expected,
            "verified": True, "mismatches": [],
        })
        changed = [{"date": "2026-11-01", "count": 2}, {"date": "2026-11-02", "count": 1}]
        result = admin_signup_trend_evidence("7d", changed, expected)
        self.assertFalse(result["verified"])
        self.assertEqual(result["mismatches"], [
            {"index": 0, "expected": {"date": "2026-11-01", "count": 3}, "actual": {"date": "2026-11-01", "count": 2}},
            {"index": 1, "expected": {"date": "2026-11-02", "count": 0}, "actual": {"date": "2026-11-02", "count": 1}},
        ])

        expected_render = [{"date": "2026-11-01", "count": 2}, {"date": "2026-11-02", "count": 0}]
        rendered = [{"x": 10, "y": 0}, {"x": 20, "y": 100}]
        ticks = [{"value": 0, "y": 100}, {"value": 2, "y": 0}]
        self.assertTrue(admin_signup_svg_evidence("7d", rendered, ticks, expected_render)["verified"])
        swapped_render = [{"x": 10, "y": 100}, {"x": 20, "y": 0}]
        negative = admin_signup_svg_evidence("7d", swapped_render, ticks, expected_render)
        self.assertFalse(negative["verified"])
        self.assertEqual([m["index"] for m in negative["mismatches"]], [0, 1])

    def test_recommend_form_request_requires_member_and_exact_deduplicated_fixture(self):
        from product.tests import facereview_ui_browser_evidence as evidence
        member_recommend_request_matches = getattr(evidence, 'member_recommend_request_matches', None)
        self.assertTrue(callable(member_recommend_request_matches), 'member_recommend_request_matches is missing')
        body = {'extension': 'VideoRequest.recommend', 'input': {'user': 'member', 'youtube_url_list': '["AipForm0001", "AipForm0002"]'}}
        self.assertTrue(member_recommend_request_matches(body, 'member', ['AipForm0001', 'AipForm0002']))
        self.assertTrue(member_recommend_request_matches({'key': 'k1', 'request': body}, 'member', ['AipForm0001', 'AipForm0002']))
        for broken in [None, {}, {**body, 'extension': 'Other.recommend'},
                       *[{**body, 'input': {**body['input'], key: value}} for key, value in
                         [('user', 'other'), ('youtube_url_list', 'invalid'), ('youtube_url_list', '[]'),
                          ('youtube_url_list', '["AipForm0001", "AipForm0001"]'), ('extra', True)]]]:
            self.assertFalse(member_recommend_request_matches(broken, 'member', ['AipForm0001', 'AipForm0002']))

    def test_recommend_form_acceptance_and_duplicate_refusal_need_complete_evidence(self):
        from product.tests import facereview_ui_browser_evidence as evidence
        member_recommend_evidence = getattr(evidence, 'member_recommend_evidence', None)
        self.assertTrue(callable(member_recommend_evidence), 'member_recommend_evidence is missing')
        row = {'path': '/apply', 'recommend_input_matches': True, 'status': 200,
               'body_captured': True, 'ok': True, 'contract_matches_runtime': True, 'requested_count': 2}
        self.assertTrue(member_recommend_evidence([row], True, True)['verified'])
        self.assertTrue(member_recommend_evidence([{**row, 'path': '/extension'}], True, True)['verified'])
        refused = {**row, 'ok': False, 'requested_count': None, 'code': 'WIRE.ALREADY_EXISTS'}
        self.assertTrue(member_recommend_evidence([refused], False, True)['verified'])
        for broken in [[], [row, row], [{**row, 'requested_count': True}], [{**row, 'requested_count': 1}],
                       *[[{**row, key: False}] for key in ('body_captured', 'contract_matches_runtime', 'recommend_input_matches')]]:
            self.assertFalse(member_recommend_evidence(broken, True, True)['verified'])
        self.assertFalse(member_recommend_evidence([{**refused, 'code': 'WIRE.INTERNAL'}], False, True)['verified'])
        self.assertFalse(member_recommend_evidence([row], True, False)['verified'])
        self.assertFalse(member_recommend_evidence([{**row, 'path': '/read'}], True, True)['verified'])

    def test_bookmark_listing_summary_requires_exact_request_shape_and_keeps_only_counts(self):
        from product.tests.facereview_ui_browser_evidence import bookmark_listing_request_matches, bookmark_listing_response_evidence

        request = {'extension': 'Video.homeBookmark', 'input': {'page': 1, 'size': 20, 'emotion': 'all'}}
        self.assertTrue(bookmark_listing_request_matches(request))
        for broken in [None, {}, {**request, 'extension': 'Video.homeVideoAll'},
                       *[{**request, 'input': {**request['input'], key: value}} for key, value in
                         [('page', True), ('page', 2), ('size', 10), ('emotion', 'sad'), ('user', 'private-user')]]]:
            self.assertFalse(bookmark_listing_request_matches(broken))
        response = {'output': {'videos': json.dumps([{'video_id': VIDEO_ID, 'title': 'private-title'}])}}
        self.assertEqual(bookmark_listing_response_evidence(response, VIDEO_ID),
                         {'response_list_length': 1, 'contains_fixture_video': True})
        self.assertEqual(bookmark_listing_response_evidence({'output': {'videos': '[]'}}, VIDEO_ID),
                         {'response_list_length': 0, 'contains_fixture_video': False})
        for invalid in [None, {}, {'output': {'videos': '{}'}}, {'output': {'videos': 'broken'}}]:
            self.assertEqual(bookmark_listing_response_evidence(invalid, VIDEO_ID),
                             {'response_list_length': None, 'contains_fixture_video': False})

    def test_bookmark_listing_gate_requires_completed_response_and_both_render_states(self):
        from product.tests.facereview_ui_browser_evidence import bookmark_listing_evidence

        row = {'path': '/extension', 'extension_request': {'extension': 'Video.homeBookmark'},
               'bookmark_listing_input_matches': True, 'status': 200, 'body_captured': True,
               'ok': True, 'contract_matches_runtime': True, 'response_list_length': 1,
               'contains_fixture_video': True}
        self.assertTrue(bookmark_listing_evidence([row], False, True)['verified'])
        empty = {**row, 'response_list_length': 0, 'contains_fixture_video': False}
        self.assertTrue(bookmark_listing_evidence([empty], True, True)['verified'])
        for changed in [{'body_captured': False}, {'ok': False}, {'status': 500},
                        {'bookmark_listing_input_matches': False}, {'contract_matches_runtime': False},
                        {'response_list_length': None}, {'response_list_length': True},
                        {'response_list_length': 2}, {'contains_fixture_video': False}]:
            self.assertFalse(bookmark_listing_evidence([{**row, **changed}], False, True)['verified'])
        for rows in [[], [row, row], [empty], [{**row, 'extension_request': {'extension': 'private-extension'}}]]:
            self.assertFalse(bookmark_listing_evidence(rows, False, True)['verified'])
        self.assertFalse(bookmark_listing_evidence([row], False, False)['verified'])
        self.assertFalse(bookmark_listing_evidence([row], True, True)['verified'])

    def test_recent_videos_request_requires_the_original_defaults(self):
        from product.tests import facereview_ui_browser_evidence as evidence
        matches = getattr(evidence, 'member_recent_request_matches', None)
        self.assertTrue(callable(matches), 'member_recent_request_matches is missing')
        request = {'extension': 'User.myRecentVideos', 'input': {'emotion': 'all', 'page': 1, 'size': 10}}
        self.assertTrue(matches(request))
        for broken in [None, {}, {**request, 'extension': 'Video.homeBookmark'},
                       *[{**request, 'input': {**request['input'], key: value}} for key, value in
                         [('page', True), ('page', 2), ('size', 20), ('emotion', 'sad'), ('user', 'private-user')]]]:
            self.assertFalse(matches(broken))

    def test_recent_videos_response_keeps_only_ids_emotions_and_shape_flags(self):
        from product.tests import facereview_ui_browser_evidence as evidence
        summarize = getattr(evidence, 'recent_videos_response_evidence', None)
        self.assertTrue(callable(summarize), 'recent_videos_response_evidence is missing')
        timeline = {emotion: [{'x': 1, 'y': 0.0}] for emotion in ('neutral', 'happy', 'surprise', 'sad', 'angry')}
        videos = [
            {'video_id': 'v1', 'youtube_url': 'private-url', 'title': 'private-title', 'dominant_emotion': 'sad',
             'dominant_emotion_per': 55.0, 'watched_at': 'private-time', 'timeline_data': timeline},
            {'video_id': 'v2', 'youtube_url': 'private-url', 'title': 'private-title', 'dominant_emotion': 'happy',
             'dominant_emotion_per': 60.0, 'watched_at': 'private-time', 'timeline_data': timeline},
        ]
        body = json.dumps({'videos': videos, 'total': 2, 'page': 1, 'size': 10, 'has_next': False})
        got = summarize({'ok': True, 'output': {'body': body}})
        self.assertEqual(got, {
            'recent_video_ids': ['v1', 'v2'], 'recent_emotions': ['sad', 'happy'],
            'recent_percentages': [55.0, 60.0], 'recent_timelines_complete': True,
            'recent_total': 2, 'recent_has_next': False})
        self.assertNotIn('private-', json.dumps(got))
        empty_timeline = json.dumps({'videos': [{**videos[0], 'timeline_data': {}}], 'total': 1, 'has_next': False})
        self.assertIs(summarize({'output': {'body': empty_timeline}})['recent_timelines_complete'], False)
        unusable = {'recent_video_ids': None, 'recent_emotions': None, 'recent_percentages': None,
                    'recent_timelines_complete': False, 'recent_total': None, 'recent_has_next': None}
        for invalid in [None, {}, {'output': {}}, {'output': {'body': 'broken'}}, {'output': {'body': '{}'}},
                        {'output': {'body': json.dumps({'videos': 'x'})}}]:
            self.assertEqual(summarize(invalid), unusable)

    def test_recent_videos_gate_needs_exact_order_values_and_visible_render(self):
        from product.tests import facereview_ui_browser_evidence as evidence
        gate = getattr(evidence, 'member_recent_evidence', None)
        self.assertTrue(callable(gate), 'member_recent_evidence is missing')
        expected = [{'video_id': 'v1', 'emotion': 'sad', 'percentage': 55.0},
                    {'video_id': 'v2', 'emotion': 'happy', 'percentage': 60.0}]
        row = {'path': '/extension', 'extension_request': {'extension': 'User.myRecentVideos'},
               'recent_input_matches': True, 'status': 200, 'body_captured': True, 'ok': True,
               'contract_matches_runtime': True, 'recent_video_ids': ['v1', 'v2'],
               'recent_emotions': ['sad', 'happy'], 'recent_percentages': [55.0, 60.0],
               'recent_timelines_complete': True, 'recent_total': 2, 'recent_has_next': False}
        self.assertTrue(gate([row], expected, True)['verified'])
        for changed in [{'body_captured': False}, {'ok': False}, {'status': 500}, {'recent_input_matches': False},
                        {'contract_matches_runtime': False}, {'recent_video_ids': ['v2', 'v1']},
                        {'recent_video_ids': ['v1']}, {'recent_video_ids': None},
                        {'recent_emotions': ['sad', 'sad']}, {'recent_percentages': [55.0, 61.0]},
                        {'recent_timelines_complete': False}, {'recent_total': 3}, {'recent_has_next': True}]:
            self.assertFalse(gate([{**row, **changed}], expected, True)['verified'], changed)
        for rows in [[], [row, row], [{**row, 'extension_request': {'extension': 'private-extension'}}]]:
            self.assertFalse(gate(rows, expected, True)['verified'])
        self.assertFalse(gate([row], expected, False)['verified'])
        self.assertFalse(gate([row], [], True)['verified'])

    def test_member_read_names_cover_detail_recent_related_and_bookmark_without_values(self):
        for name in ('Video.watchDetail', 'User.myRecentVideos',
                     'Video.watchRecommended', 'Video.homeBookmark'):
            with self.subTest(extension=name):
                body = {'extension': name, 'input': {
                    'video_id': 'private-fixture', 'user': 'private-user',
                    'page': 1, 'emotion': 'all', 'token': 'private-token'},
                    'token': 'private-token'}
                self.assertEqual(extension_request_evidence(body),
                                 {'extension': name, 'page': 1, 'emotion': 'all'})
                self.assertNotIn('private-', json.dumps(extension_request_evidence(body)))

    def test_unknown_or_malformed_member_extension_names_are_not_recorded(self):
        for name in ('private-fixture', 'Video.homeBookmark.private', None, 1, True, [], {}):
            with self.subTest(extension=name):
                self.assertEqual(extension_request_evidence({'extension': name}), {})

    def test_new_member_request_names_do_not_claim_the_list_response_shape(self):
        for name in ('Video.watchDetail', 'User.myRecentVideos',
                     'Video.watchRecommended', 'Video.homeBookmark'):
            self.assertEqual(extension_response_evidence(
                name, {'ok': True, 'output': {'videos': '[]'}}, VIDEO_ID), {})

    def test_pending_api_diagnostics_keep_only_allowlisted_paths_and_phases(self):
        from product.tests import facereview_ui_browser_evidence as evidence_module

        class Request:
            def __init__(self, url, method):
                self.url, self.method = url, method

        missing = Request('http://api.test/extension?secret=private-fixture', 'POST')
        body = Request('http://api.test/read', 'POST')
        other = Request('http://api.test/private-fixture', 'private-fixture')
        done = Request('http://api.test/auth/login', 'POST')
        result = evidence_module.api_completion_diagnostics(
            {missing, body, other, done}, {done}, {body: object()}, [],
            {missing: 'tutorial_action', body: 'reload_main', other: 'private-fixture'})
        self.assertEqual([result[k] for k in ('requests', 'finished', 'failed', 'unfinished')], [4, 1, 0, 3])
        self.assertEqual(result['unresolved'], [
            {'path': '/extension', 'method': 'POST', 'started_phase': 'tutorial_action', 'response_pending': False},
            {'path': '/read', 'method': 'POST', 'started_phase': 'reload_main', 'response_pending': True},
            {'path': 'other', 'method': 'other', 'started_phase': 'other', 'response_pending': False}])
        self.assertNotIn('private-fixture', json.dumps(result))

    def test_live_account_journeys_classify_the_actual_exception_without_private_text(self):
        error = TimeoutError("private fixture detail must not enter safe evidence")
        for journey in ('account', 'email', 'password', 'withdraw'):
            source = Path(__file__).with_name(f'facereview-ui-{journey}-browser.py')
            calls = [node for node in ast.walk(ast.parse(source.read_text()))
                     if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                     and node.func.id == 'safe_browser_failure_type']
            self.assertEqual(len(calls), 2, f'{journey}: both exception handlers must be checked')
            for call in calls:
                with self.subTest(journey=journey, line=call.lineno):
                    classified = eval(compile(ast.Expression(call), str(source), 'eval'),
                                      {'safe_browser_failure_type': safe_browser_failure_type, 'error': error})
                    self.assertEqual(classified, 'TimeoutError')

    def test_api_completion_waits_for_requests_without_responses_after_cached_idle(self):
        started, finished = {'old', 'new'}, {'old'}
        pending, failures = {}, []

        class Page:
            ticks = 0

            def wait_for_timeout(self, milliseconds):
                self.ticks += 1
                if self.ticks == 1:
                    pending['new'] = 'body'
                else:
                    pending.clear()
                    finished.add('new')

        page = Page()
        wait_for_api_completion(page, started, finished, pending, failures)
        self.assertEqual(finished, started)
        self.assertEqual(pending, {})
        self.assertEqual(page.ticks, 2)

    def test_account_drains_first_main_requests_before_reload_even_with_cached_networkidle(self):
        source = Path(__file__).with_name('facereview-ui-account-browser.py')
        tree = ast.parse(source.read_text())
        tutorial = next(node for node in ast.walk(tree) if isinstance(node, ast.If)
                        and any(isinstance(child, ast.Constant) and child.value == 'main_visible'
                                for statement in node.orelse for child in ast.walk(statement)))
        statements = tutorial.orelse
        start = next(index for index, statement in enumerate(statements)
                     if isinstance(statement, ast.Assign) and isinstance(statement.targets[0], ast.Subscript)
                     and isinstance(statement.targets[0].slice, ast.Constant)
                     and statement.targets[0].slice.value == 'main_visible')
        end = next(index for index in range(start+1,len(statements))
                   if isinstance(statements[index], ast.Expr) and isinstance(statements[index].value, ast.Call)
                   and isinstance(statements[index].value.func, ast.Attribute)
                   and statements[index].value.func.attr == 'reload')
        # Execute the actual synchronization and reload calls. The server has
        # accepted the three requests, but their response events are still queued.
        started, finished = {'extension-1','extension-2','extension-3'}, set()
        pending, failures = {}, []
        class Page:
            def wait_for_load_state(self, state):
                pass  # networkidle can still describe the preceding SPA document
            def wait_for_timeout(self, milliseconds):
                finished.update(started)
            def reload(self, **kwargs):
                assert started == finished, 'reload discarded outstanding first-main responses'
        calls = [statement for statement in statements[start+1:end+1] if isinstance(statement, ast.Expr)]
        code = compile(ast.fix_missing_locations(ast.Module(body=calls,type_ignores=[])),str(source),'exec')
        exec(code, {'page':Page(), 'started':started,'finished':finished,'pending':pending,'failures':failures,
                    'wait_for_api_completion':wait_for_api_completion})
        self.assertEqual(started, finished)

    def test_api_completion_never_accepts_a_failed_or_unfinished_request(self):
        class Page:
            def wait_for_timeout(self, milliseconds):
                pass

        for finished, pending, failures in [(set(), {}, []), ({'request'}, {'request': 'body'}, []),
                                            ({'request'}, {}, [{'path': '/read'}])]:
            with self.subTest(finished=finished, pending=pending, failures=failures):
                with self.assertRaises(AssertionError):
                    wait_for_api_completion(Page(), {'request'}, finished, pending, failures, timeout_ms=0)

    def test_withdraw_capture_keeps_a_reentrant_requestfinished_event(self):
        for name in ('withdraw', 'password'):
            with self.subTest(recorder=name):
                self._assert_reentrant_capture(name)

    def _assert_reentrant_capture(self, name):
        # Request.all_headers() yields to Playwright's event dispatcher. Execute
        # the real recorder functions with requestfinished delivered at that point.
        source = Path(__file__).with_name(f'facereview-ui-{name}-browser.py')
        tree = ast.parse(source.read_text())
        functions = [node for node in ast.walk(tree)
                     if isinstance(node, ast.FunctionDef) and node.name in {'capture', 'capture_body'}]
        self.assertEqual(len(functions), 2)
        from urllib.parse import urlsplit
        namespace = {'urlsplit': urlsplit, 'api_port': 58081, 'json': json,
                     'rows': [], 'pending': {}, 'finished': set()}
        factory = ast.parse('def make_recorders():\n    reset_token = None\n').body[0]
        factory.body.extend(functions)
        factory.body.extend(ast.parse('return capture, capture_body').body)
        module = ast.fix_missing_locations(ast.Module(body=[factory], type_ignores=[]))
        exec(compile(module, str(source), 'exec'), namespace)
        namespace['capture'], namespace['capture_body'] = namespace['make_recorders']()

        class Request:
            url = 'http://localhost:58081/auth/logout'
            method = 'POST'
            headers = {}
            post_data_json = {}

            def all_headers(self):
                namespace['capture_body'](self)
                return {'cookie': 'refresh_token=fixture'}

        request = Request()

        class Response:
            status = 200
            url = request.url

            def body(self):
                return b'{"ok":true}'

        response = Response()
        response.request = request
        namespace['capture'](response)
        self.assertIn(request, namespace['finished'])
        self.assertEqual(namespace['pending'], {})
        self.assertTrue(namespace['rows'][0]['body_captured'])
        self.assertTrue(namespace['rows'][0]['refresh_cookie_present'])

    def test_withdraw_success_selector_rejects_a_longer_failure_message(self):
        source = Path(__file__).with_name('facereview-ui-withdraw-browser.py')
        tree = ast.parse(source.read_text())
        message = '탈퇴가 완료되었어요'
        selectors = [node for node in ast.walk(tree)
                     if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                     and node.func.attr == 'get_by_text' and node.args
                     and isinstance(node.args[0], ast.Constant) and node.args[0].value == message]
        self.assertEqual(len(selectors), 1)
        selector = compile(ast.Expression(selectors[0]), str(source), 'eval')

        class Page:
            def __init__(self, text):
                self.text = text

            def get_by_text(self, text, *, exact=False):
                return self.text == text if exact else text in self.text

        self.assertTrue(eval(selector, {'page': Page(message)}))
        self.assertFalse(eval(selector, {'page': Page(message + ' — 실제 처리 실패')}))

    def test_tutorial_reload_checks_requeried_runtime_state_without_persisted_tutorial(self):
        # Execute the actual page predicate. The pinned store persists sign-in/id,
        # but App refreshes user_tutorial only in the live Zustand store on mount.
        tree = ast.parse(Path(__file__).with_name('facereview-ui-account-browser.py').read_text())
        predicates = [node.args[0].value for node in ast.walk(tree)
                      if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                      and node.func.attr == 'wait_for_function' and node.args
                      and isinstance(node.args[0], ast.Constant)
                      and 'user_tutorial' in str(node.args[0].value)]
        self.assertEqual(len(predicates), 1)
        predicate = predicates[0].replace("import('/src/store/authStore.ts')",
                                          "Promise.resolve({useAuthStorage: {getState: () => state}})")
        script = """
          const predicate = PREDICATE;
          const localStorage = {getItem: () => JSON.stringify({state: {
            is_sign_in: true, user_id: 'target'
          }})};
          let state = {is_sign_in: true, user_id: 'target', user_tutorial: 1};
          const check = () => eval('(' + predicate + ')')("target");
          const results = [await check()];
          state.user_tutorial = 0; results.push(await check());
          state.user_tutorial = 1; state.user_id = 'other'; results.push(await check());
          state.user_id = 'target'; state.is_sign_in = false; results.push(await check());
          console.log(JSON.stringify(results));
        """.replace('PREDICATE', json.dumps(predicate))
        result = subprocess.run(['node', '--input-type=module', '-e', script], capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(result.stdout), [True, False, False, False])

    def test_signup_requires_free_then_existing_check_and_exact_completed_signup(self):
        from product.tests.facereview_ui_browser_evidence import member_signup_evidence
        checks = [{"path": "/auth/check-email", "method": "POST", "status": 200, "ok": True,
                   "body_captured": True, "payload_matches": True, "is_duplicate": duplicate}
                  for duplicate in (False, True)]
        signup = {"path": "/auth/signup", "method": "POST", "status": 201, "ok": True,
                  "body_captured": True, "payload_matches": True}
        rows = [checks[0], signup, checks[1]]
        ui = {key: True for key in ("new_email_branch", "categories_selected", "returned_to_login", "existing_email_branch")}
        self.assertTrue(member_signup_evidence(rows, ui)["verified"])
        for i in range(len(rows)):
            for key, value in [("method", "GET"), ("status", 422), ("ok", False), ("body_captured", False), ("payload_matches", False)]:
                bad = [dict(r) for r in rows];bad[i][key] = value
                self.assertFalse(member_signup_evidence(bad, ui)["verified"], (i, key))
            self.assertFalse(member_signup_evidence(rows[:i]+rows[i+1:], ui)["verified"])
            self.assertFalse(member_signup_evidence(rows+[rows[i]], ui)["verified"])
        self.assertFalse(member_signup_evidence([checks[1], signup, checks[0]], ui)["verified"])
        for key in ui:self.assertFalse(member_signup_evidence(rows, {**ui,key:False})["verified"])

    def test_tutorial_requires_exact_target_one_success_and_both_route_and_reload(self):
        from product.tests.facereview_ui_browser_evidence import member_tutorial_evidence
        row = {"path": "/apply", "tutorial_action": True, "method": "POST", "status": 200, "ok": True,
               "body_captured": True, "payload_matches": True, "authorization_present": True, "contract_matches_runtime": True}
        ui = {key:True for key in ("initial_step", "action_clicked", "main_visible", "reloaded_tutorial_done")}
        self.assertTrue(member_tutorial_evidence([row], ui, "skip")["verified"])
        self.assertFalse(member_tutorial_evidence([row], ui, "complete")["verified"])
        self.assertTrue(member_tutorial_evidence([row], {**ui,"all_steps":True}, "complete")["verified"])
        self.assertFalse(member_tutorial_evidence([row], ui, "other")["verified"])
        for key,value in [("method","PATCH"),("status",400),("ok",False),("body_captured",False),
                          ("payload_matches",False),("authorization_present",False),("contract_matches_runtime",False)]:
            self.assertFalse(member_tutorial_evidence([{**row,key:value}], ui, "skip")["verified"],key)
        self.assertFalse(member_tutorial_evidence([],ui,"skip")["verified"])
        self.assertFalse(member_tutorial_evidence([row,row],ui,"skip")["verified"])
        for key in ui:self.assertFalse(member_tutorial_evidence([row],{**ui,key:False},"skip")["verified"])

    def test_email_verification_requires_single_completed_bodies_and_persistent_ui(self):
        from product.tests.facereview_ui_browser_evidence import member_email_evidence
        rows = [{"path": path, "method": "POST", "status": 200, "ok": True,
                 "body_captured": True, "payload_matches": True, "authorization_present": True}
                for path in ("/auth/email/verification", "/auth/email/verify")]
        ui = {key: True for key in ("unverified_banner", "code_modal", "verified_badge", "banner_removed", "reloaded_verified")}
        summary = {"path": "/apply", "method": "POST", "status": 200, "ok": True, "body_captured": True,
                   "write_extension": "User.myEmotionSummary"}
        served = rows + [summary]
        self.assertTrue(member_email_evidence(served, ui)["verified"])
        self.assertFalse(member_email_evidence(rows, ui)["verified"])
        refused = [dict(summary, ok=False, error_code="WIRE.NOT_FOUND")]
        self.assertFalse(member_email_evidence(rows + refused, ui)["verified"])
        for i in range(len(rows)):
            for key, value in [("method", "PATCH"), ("status", 400), ("ok", False),
                               ("body_captured", False), ("payload_matches", False), ("authorization_present", False)]:
                changed = [dict(row) for row in served]; changed[i][key] = value
                self.assertFalse(member_email_evidence(changed, ui)["verified"], (i, key))
            self.assertFalse(member_email_evidence(served[:i] + served[i+1:], ui)["verified"])
            self.assertFalse(member_email_evidence(served + [served[i]], ui)["verified"])
        for key in ui:
            self.assertFalse(member_email_evidence(served, {**ui, key: False})["verified"])

    def test_password_journey_requires_exact_completed_auth_bodies_and_ui(self):
        from product.tests.facereview_ui_browser_evidence import member_password_evidence
        rows = [{"path": path, "method": method, "status": 200, "ok": True, "body_captured": True,
                 "payload_matches": True, "authorization_present": True}
                for path, method in [("/auth/email/verification", "POST"), ("/auth/password/verification", "POST"),
                                     ("/auth/password", "POST"), ("/auth/logout", "POST")]]
        rows[1]["reset_token_received"] = True
        rows[3].update(authorization_present=False, refresh_cookie_present=True)
        rows += [{"path": "/auth/login", "method": "POST", "status": 200, "ok": True, "body_captured": True,
                  "payload_matches": True, "password_kind": kind} for kind in ("old", "new")]
        ui = {key: True for key in ("password_page", "code_form", "new_password_form", "signed_out", "new_login", "reloaded_profile")}
        summary = {"path": "/apply", "method": "POST", "status": 200, "ok": True, "body_captured": True,
                   "write_extension": "User.myEmotionSummary"}
        served = rows + [summary]
        self.assertTrue(member_password_evidence(served, ui)["verified"])
        self.assertFalse(member_password_evidence(rows, ui)["verified"])
        refused = [dict(summary, ok=False, error_code="WIRE.NOT_FOUND")]
        self.assertFalse(member_password_evidence(rows + refused, ui)["verified"])
        for i in range(len(rows)):
            for key, value in [("body_captured", False), ("ok", False), ("payload_matches", False), ("status", 400), ("method", "GET")]:
                changed = [dict(row) for row in served]; changed[i][key] = value
                self.assertFalse(member_password_evidence(changed, ui)["verified"], (i, key))
            self.assertFalse(member_password_evidence(served[:i] + served[i+1:], ui)["verified"])
        self.assertFalse(member_password_evidence(served + [served[0]], ui)["verified"])
        # The original backend's PATCH is not the AIP route; a browser preflight refuses it.
        original_method = [dict(row) for row in served]; original_method[2]["method"] = "PATCH"
        self.assertFalse(member_password_evidence(original_method, ui)["verified"])
        no_cookie = [dict(row) for row in served]; no_cookie[3]["refresh_cookie_present"] = False
        self.assertFalse(member_password_evidence(no_cookie, ui)["verified"])
        no_bearer = [dict(row) for row in served]; no_bearer[1]["authorization_present"] = False
        self.assertFalse(member_password_evidence(no_bearer, ui)["verified"])
        self.assertFalse(member_password_evidence([{**row, "reset_token_received": False} for row in served], ui)["verified"])
        for key in ui:
            self.assertFalse(member_password_evidence(served, {**ui, key: False})["verified"])

    def test_withdraw_journey_requires_single_login_action_logout_and_confirmed_ui(self):
        from product.tests.facereview_ui_browser_evidence import member_withdraw_evidence
        rows = [{"path": "/auth/login", "method": "POST", "status": 200, "ok": True, "body_captured": True, "payload_matches": True},
                {"path": "/apply", "method": "POST", "status": 200, "ok": True, "body_captured": True, "payload_matches": True,
                 "withdraw_action": True, "authorization_present": True, "contract_matches_runtime": True},
                {"path": "/auth/logout", "method": "POST", "status": 200, "ok": True, "body_captured": True, "payload_matches": True,
                 "refresh_cookie_present": True}]
        ui = {key: True for key in ("edit_page", "withdraw_modal", "withdraw_confirmed", "signed_out")}
        self.assertTrue(member_withdraw_evidence(rows, ui)["verified"])
        untagged = rows + [{"path": "/apply", "method": "POST", "status": 200, "ok": True, "body_captured": True,
                            "write_extension": "User.myEmotionSummary"}]
        self.assertTrue(member_withdraw_evidence(untagged, ui)["verified"])
        for i in range(len(rows)):
            for key, value in [("body_captured", False), ("ok", False), ("payload_matches", False), ("status", 400), ("method", "GET")]:
                changed = [dict(row) for row in rows]; changed[i][key] = value
                self.assertFalse(member_withdraw_evidence(changed, ui)["verified"], (i, key))
            self.assertFalse(member_withdraw_evidence(rows[:i] + rows[i+1:], ui)["verified"])
        self.assertFalse(member_withdraw_evidence(rows + [rows[1]], ui)["verified"])
        mistagged = [dict(row) for row in rows]; mistagged[1]["withdraw_action"] = False
        self.assertFalse(member_withdraw_evidence(mistagged, ui)["verified"])
        mistargeted = [dict(row) for row in rows]; mistargeted[1]["payload_matches"] = False
        self.assertFalse(member_withdraw_evidence(mistargeted, ui)["verified"])
        no_auth_header = [dict(row) for row in rows]; no_auth_header[1]["authorization_present"] = False
        self.assertFalse(member_withdraw_evidence(no_auth_header, ui)["verified"])
        no_contract = [dict(row) for row in rows]; no_contract[1]["contract_matches_runtime"] = False
        self.assertFalse(member_withdraw_evidence(no_contract, ui)["verified"])
        no_cookie = [dict(row) for row in rows]; no_cookie[2]["refresh_cookie_present"] = False
        self.assertFalse(member_withdraw_evidence(no_cookie, ui)["verified"])
        for key in ui:
            self.assertFalse(member_withdraw_evidence(rows, {**ui, key: False})["verified"])

    def test_emotion_summary_gate_counts_only_tagged_served_calls(self):
        from product.tests.facereview_ui_browser_evidence import emotion_summary_served
        tagged = {"path": "/apply", "method": "POST", "status": 200, "body_captured": True,
                  "write_extension": "User.myEmotionSummary"}
        self.assertTrue(emotion_summary_served([dict(tagged, ok=True)]))
        self.assertTrue(emotion_summary_served([dict(tagged, ok=False, error_code="WIRE.CONFLICT"), dict(tagged, ok=True)]))
        self.assertFalse(emotion_summary_served([]))
        self.assertFalse(emotion_summary_served([dict(tagged, ok=False, error_code="WIRE.NOT_FOUND")]))
        self.assertFalse(emotion_summary_served([dict(tagged, ok=True, write_extension="User.other")]))
        self.assertFalse(emotion_summary_served([{"path": "/apply", "ok": True}]))

    def test_search_request_requires_exact_fixture_input_without_recording_keyword(self):
        from product.tests.facereview_ui_browser_evidence import member_search_request_matches

        payload = {"extension": "Video.homeSearch", "input": {
            "emotions": "", "keyword": "private-fixture", "keyword_type": "title", "page": 1, "size": 20}}
        self.assertIs(member_search_request_matches(payload, "private-fixture"), True)
        for broken in [None, {}, {**payload, "extension": "Video.homeVideoAll"},
                       *[{**payload, "input": {**payload["input"], key: value}} for key, value in
                         [("keyword", "other"), ("keyword_type", "channel"), ("emotions", "happy"),
                          ("page", 2), ("page", True), ("size", 1), ("unknown", "secret")]]]:
            self.assertIs(member_search_request_matches(broken, "private-fixture"), False)

    def test_search_gate_requires_one_completed_response_and_exact_visible_result(self):
        from product.tests.facereview_ui_browser_evidence import member_search_evidence

        row = {"path": "/extension", "search_input_matches": True, "status": 200,
               "body_captured": True, "ok": True, "contract_matches_runtime": True,
               "extension_request": {"extension": "Video.homeSearch"},
               "response_list_length": 1, "contains_fixture_video": True}
        self.assertTrue(member_search_evidence([row], False, True)["verified"])
        empty = {**row, "response_list_length": 0, "contains_fixture_video": False}
        self.assertTrue(member_search_evidence([empty], True, True)["verified"])
        for changed in [{"body_captured": False}, {"ok": False}, {"status": 500},
                        {"search_input_matches": False}, {"contract_matches_runtime": False},
                        {"response_list_length": None}, {"response_list_length": True},
                        {"contains_fixture_video": False}, {"response_list_length": 2}]:
            self.assertFalse(member_search_evidence([{**row, **changed}], False, True)["verified"])
        for rows in [[], [row, row], [empty], [{**row, "extension_request": {"extension": "Video.homeVideoAll"}}]]:
            self.assertFalse(member_search_evidence(rows, False, True)["verified"])
        self.assertFalse(member_search_evidence([row], False, False)["verified"])
        self.assertFalse(member_search_evidence([row], True, True)["verified"])

    def test_search_list_summary_omits_raw_query_and_titles(self):
        request = {"extension": "Video.homeSearch", "input": {"keyword": "secret", "page": 1}}
        self.assertEqual(extension_request_evidence(request), {"extension": "Video.homeSearch", "page": 1})
        self.assertEqual(extension_response_evidence("Video.homeSearch", {"output": {"videos": "[]"}}, VIDEO_ID),
                         {"response_list_length": 0, "contains_fixture_video": False})

    def test_comment_action_accepts_only_single_member_mutations(self):
        from product.tests.facereview_ui_browser_evidence import member_comment_action

        create = {"atomic": [{"create": "Comment", "values": {"video": VIDEO_ID, "user": "member", "content": "fixture"}}]}
        edit = {"apply": "Comment.edit", "target": {"ids": ["comment"]}, "values": {"content": "edited"}}
        delete = {"apply": "Comment.delete", "target": {"ids": ["comment"]}}
        for action, request in [("create", create), ("edit", edit), ("delete", delete)]:
            self.assertEqual(member_comment_action({"request": request}), action)
        for request in [None, {}, {"apply": "Comment.remove"}, {**edit, "target": {"ids": []}},
                        {**edit, "target": {"ids": ["one", "two"]}}, {**edit, "values": {"content": "x", "user": "other"}},
                        {**delete, "values": {"is_deleted": True}}, {"atomic": create["atomic"] * 2},
                        {"atomic": [{"create": "Comment", "values": {"content": "x"}}]},
                        {"atomic": [{"create": "VideoLike", "values": create["atomic"][0]["values"]}]}]:
            with self.subTest(request=request):
                self.assertIsNone(member_comment_action({"request": request}))

    def test_comment_evidence_requires_body_target_and_reload_render(self):
        from product.tests.facereview_ui_browser_evidence import member_comment_evidence

        for action in ("create", "edit", "delete"):
            row = {"path": "/apply", "comment_action": action, "status": 200, "body_captured": True,
                   "ok": True, "comment_target_matches": True, "comment_content_matches": True}
            self.assertTrue(member_comment_evidence([row], action, True)["verified"])
            for rows in [[], [row, row], [{**row, "status": 500}], [{**row, "ok": False}],
                         [{**row, "body_captured": False}], [{**row, "comment_target_matches": False}],
                         [{**row, "comment_content_matches": False}], [{**row, "path": "/read"}]]:
                with self.subTest(action=action, rows=rows):
                    self.assertFalse(member_comment_evidence(rows, action, True)["verified"])
            self.assertFalse(member_comment_evidence([row], action, False)["verified"])
            other = {**row, "comment_action": "delete" if action != "delete" else "edit"}
            self.assertFalse(member_comment_evidence([row, other], action, True)["verified"])
        self.assertFalse(member_comment_evidence([], "remove", True)["verified"])

    def test_like_evidence_requires_exact_mutation_body_and_reloaded_state(self):
        from product.tests.facereview_ui_browser_evidence import member_like_action, member_like_evidence

        self.assertEqual(member_like_action({"request": {"atomic": [{"create": "VideoLike", "values": {"user": "private"}}]}}), "add")
        self.assertEqual(member_like_action({"request": {"apply": "VideoLike.remove", "target": {"ids": ["private"]}}}), "remove")
        for payload in [None, {}, {"request": {"apply": "VideoBookmark.remove"}}, {"request": {"atomic": []}},
                        {"request": {"atomic": [{"create": "VideoLike"}, {"create": "Comment"}]}}]:
            self.assertIsNone(member_like_action(payload))
        row = {"path": "/apply", "like_action": "add", "status": 200, "body_captured": True, "ok": True}
        self.assertTrue(member_like_evidence([row], "add", True)["verified"])
        self.assertTrue(member_like_evidence([{**row, "like_action": "remove"}], "remove", False)["verified"])
        for rows in [[], [row, row], [{**row, "ok": False}], [{**row, "body_captured": False}],
                     [{**row, "status": 500}], [{**row, "like_action": "remove"}]]:
            self.assertFalse(member_like_evidence(rows, "add", True)["verified"])
        self.assertFalse(member_like_evidence([row], "add", False)["verified"])
        self.assertFalse(member_like_evidence([row], "unknown", True)["verified"])

    def test_logout_requires_successful_body_and_cleared_client_state(self):
        from product.tests.facereview_ui_browser_evidence import member_logout_evidence

        row = {"path": "/auth/logout", "status": 200, "body_captured": True, "ok": True}
        cleared = {"signed_out": True, "access_token_cleared": True, "user_id_cleared": True}
        self.assertTrue(member_logout_evidence([row], cleared)["verified"])
        for broken in [[], [{**row, "status": 401}], [{**row, "ok": False}], [{**row, "ok": 1}],
                       [{**row, "body_captured": False}], [row, row]]:
            with self.subTest(requests=broken):
                self.assertFalse(member_logout_evidence(broken, cleared)["verified"])
        for key in cleared:
            with self.subTest(stale_state=key):
                self.assertFalse(member_logout_evidence([row], {**cleared, key: False})["verified"])
        self.assertFalse(member_logout_evidence([row], {})["verified"])

    def test_workflow_uses_only_the_selected_admin_or_member_diagnostic(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/verify.yml").read_text()
        step = workflow.split("      - name: Verify real HTTP to Node and Python workers to MySQL\n", 1)[1].split("      - name:", 1)[0]
        run = step.split("        run:", 1)[1].strip("\n")
        if run.strip().startswith("|"):
            command = "\n".join(line.removeprefix("          ") for line in run.splitlines()[1:])
        else:
            command = run.strip()
        for member, admin, expected in [("", "", "run-ci"), ("false", "false", "run-ci"),
                                        ("true", "false", "run-ci-member"), ("false", "true", "run-ci-admin")]:
            with self.subTest(member_ui_diagnostic=member, admin_metrics_diagnostic=admin):
                # Execute the workflow's actual shell branch without starting its database fixtures.
                shell = 'bash() { printf "%s\\n" "$2"; };\n' + command.replace("${{ inputs.member_ui_diagnostic }}", member).replace("${{ inputs.admin_metrics_diagnostic }}", admin)
                result = subprocess.run(["bash", "-eu", "-c", shell], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), expected)

    def test_playwright_exit_failure_reverses_success_return_negative_control(self):
        class DriverScope:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                raise RuntimeError("driver shutdown failed")

        report_written = []
        with self.assertRaisesRegex(RuntimeError, "driver shutdown failed"):
            with DriverScope():
                report_written.append("journey_stage=completed")
                return_code = 0
        self.assertEqual(report_written, ["journey_stage=completed"])
        self.assertEqual(return_code, 0)

    def test_reissue_browser_is_closed_inside_playwright_lifecycle(self):
        source = (Path(__file__).resolve().parent / "facereview-ui-profile-browser.py").read_text()
        reissue_report = source.index('"journey_stage": "completed"', source.index('"phase": "member_reissue"'))
        reissue_return = source.index("return 0", reissue_report)
        close_scope = source.rfind("context.close()", 0, reissue_return)
        close_browser = source.index("browser.close()", close_scope)
        clear_browser = source.index("browser = None", close_browser)
        finally_browser_close = source.index("if browser is not None:", source.index("finally:"))
        self.assertNotEqual(close_scope, -1)
        self.assertLess(close_scope, close_browser)
        self.assertLess(close_browser, clear_browser)
        self.assertLess(clear_browser, reissue_return)
        self.assertGreater(finally_browser_close, reissue_return)

    def test_safe_failure_locator_records_stage_literal_command_and_exit_only(self):
        repo = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as tmp:
            locator = Path(tmp) / "safe.txt"
            script = (
                f"export AIP_VERIFY_FAILURE_FILE='{locator}'; "
                f"source '{repo / 'scripts/verify-safe-locator.sh'}'; "
                "aip_verify_stage 'rust test loop'; false"
            )
            result = subprocess.run(["bash", "-Eeuo", "pipefail", "-c", script], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            content = locator.read_text()
            self.assertIn("stage=rust test loop", content)
            self.assertIn("command=false", content)
            self.assertIn("exit_status=1", content)
            self.assertNotIn("stdout", content)
            self.assertNotIn("stderr", content)

    def test_clippy_capture_preserves_stdout_stderr_command_and_failure_status(self):
        repo = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as tmp:
            output_dir = Path(tmp) / "clippy"
            script = (
                f"export AIP_VERIFY_CLIPPY_OUTPUT_DIR='{output_dir}'; "
                f"source '{repo / 'scripts/verify-safe-locator.sh'}'; "
                "cargo() { printf 'lint diagnostic on stdout\\n'; "
                "printf 'lint diagnostic on stderr\\n' >&2; return 23; }; "
                "aip_verify_capture_command \"$AIP_VERIFY_CLIPPY_OUTPUT_DIR\" "
                "cargo clippy --workspace --lib --bins --locked -- -D warnings"
            )
            result = subprocess.run(["bash", "-c", script], capture_output=True, text=True)
            self.assertEqual(result.returncode, 23)
            self.assertIn("lint diagnostic on stdout", result.stdout)
            self.assertIn("lint diagnostic on stderr", result.stderr)
            self.assertEqual((output_dir / "stdout.txt").read_text(), "lint diagnostic on stdout\n")
            self.assertEqual((output_dir / "stderr.txt").read_text(), "lint diagnostic on stderr\n")
            self.assertEqual((output_dir / "exit_status.txt").read_text(), "23\n")
            self.assertIn("cargo clippy --workspace --lib --bins --locked -- -D warnings",
                          (output_dir / "command.txt").read_text())

    def test_verify_workflow_always_uploads_captured_workspace_clippy_output(self):
        repo = Path(__file__).resolve().parents[2]
        verify_script = (repo / "tools/verify.sh").read_text()
        workflow = (repo / ".github/workflows/verify.yml").read_text()
        self.assertIn('verify_independent "workspace library and binary clippy" aip_verify_capture_command',
                      verify_script)
        self.assertIn("AIP_VERIFY_CLIPPY_OUTPUT_DIR: ${{ runner.temp }}/aip-verify-clippy", workflow)
        artifact = workflow.split("      - name: Preserve workspace Clippy diagnostics\n", 1)[1]
        self.assertIn("if: always()", artifact)
        self.assertIn("path: ${{ runner.temp }}/aip-verify-clippy", artifact)

    def test_safe_failure_locator_records_guarded_preflight_exit(self):
        repo = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as tmp:
            locator = Path(tmp) / "guarded.txt"
            script = (
                f"export AIP_VERIFY_FAILURE_FILE='{locator}'; "
                f"source '{repo / 'scripts/verify-safe-locator.sh'}'; "
                "aip_verify_stage 'namespace preflight'; "
                "if ! false; then aip_verify_fail 3 'bwrap --unshare-all --ro-bind / / --proc /proc --dev /dev true'; fi"
            )
            result = subprocess.run(["bash", "-Eeuo", "pipefail", "-c", script], capture_output=True, text=True)
            self.assertEqual(result.returncode, 3)
            content = locator.read_text()
            self.assertIn("stage=namespace preflight", content)
            self.assertIn("command=bwrap --unshare-all --ro-bind / / --proc /proc --dev /dev true", content)
            self.assertIn("exit_status=3", content)

    def test_safe_failure_locator_preserves_nested_first_failure(self):
        repo = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as tmp:
            locator = Path(tmp) / "nested.txt"
            child = Path(tmp) / "child.sh"
            child.write_text(
                f"set -Eeuo pipefail\nsource '{repo / 'scripts/verify-safe-locator.sh'}'\n"
                "aip_verify_stage 'inner mysql clippy'\nfalse\n"
            )
            script = (f"export AIP_VERIFY_FAILURE_FILE='{locator}'; "
                      f"source '{repo / 'scripts/verify-safe-locator.sh'}'; "
                      f"aip_verify_stage 'outer workflow'; bash '{child}'")
            result = subprocess.run(["bash", "-Eeuo", "pipefail", "-c", script], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            content = locator.read_text()
            self.assertIn("stage=inner mysql clippy\ncommand=false\nexit_status=1\n", content)
            self.assertNotIn("outer workflow", content)

    def test_safe_failure_locator_new_stage_resets_previous_failure(self):
        repo = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as tmp:
            locator = Path(tmp) / "new-stage.txt"
            script = (f"export AIP_VERIFY_FAILURE_FILE='{locator}'; "
                      f"source '{repo / 'scripts/verify-safe-locator.sh'}'; "
                      "aip_verify_locator_write 'old gate' 'old command' 7; "
                      "aip_verify_stage 'new gate'; false")
            result = subprocess.run(["bash", "-Eeuo", "pipefail", "-c", script], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            self.assertEqual(locator.read_text(), "stage=new gate\ncommand=false\nexit_status=1\n")

    def test_focused_member_contract_stops_before_external_store_suites(self):
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/verify.yml").read_text()
        script = (repo / "scripts/verify-mysql-worker-e2e.sh").read_text()
        self.assertIn("scripts/verify-mysql-worker-e2e.sh run-ci-member", workflow)
        self.assertIn("run | run-ci | run-ci-member | run-ci-admin | down", script)
        self.assertLess(script.index('AIP_VERIFY_FAILURE_FILE="$AIP_E2E_FAILURE_FILE"'),
                        script.index('source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/verify-safe-locator.sh"'))
        # Scope/failure behavior is executed by test_facereview_e2e_suites; here
        # retain only the shell/workflow wiring contract after extracting it.
        self.assertIn('python3 "$repo/scripts/facereview-e2e-suites.py" "$mode"', script)
        self.assertGreaterEqual(script.count('if [[ "$mode" == run-ci || "$mode" == run-ci-member || "$mode" == run-ci-admin ]]; then'), 2)
        self.assertIn('scripts/verify-mysql-worker-e2e.sh run-ci', script)

    def test_admin_metrics_diagnostic_starts_the_configured_redis_fixture(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/verify.yml").read_text()
        fixture_step = workflow.split("      - name: Start native driver fixtures\n", 1)[1].split("      - name:", 1)[0]
        self.assertIn('if [[ "${{ inputs.admin_metrics_diagnostic }}" == "true" ]]; then\n            stores+=(redis)', fixture_step)

    def test_admin_browser_checks_are_initialized_before_negative_control(self):
        source = Path(__file__).with_name("facereview-ui-admin-browser.py").read_text()
        self.assertLess(source.index("checks: dict[str, object] = {}"), source.index('checks["signup_checker_negative_control"]'))

    def test_admin_trend_rendered_svg_points_use_axis_ticks_and_save_period_screenshot(self):
        source = Path(__file__).with_name("facereview-ui-admin-browser.py").read_text()
        self.assertIn("admin_signup_svg_evidence(period, svg_data[\"points\"], svg_data[\"ticks\"], expected_points)", source)
        self.assertIn('checks["signup_trend_rendered_svg"] = trend_render_results', source)
        screenshot = source.index('page.screenshot(path=str(output / f"admin-dashboard-signup-')
        evidence = source.index("render_result = admin_signup_svg_evidence")
        self.assertLess(evidence, screenshot)

    def test_admin_permission_denial_rows_are_checked_after_clearing_member_filter(self):
        source = Path(__file__).with_name("facereview-ui-admin-browser.py").read_text()
        block = source.split('stage("admin_permission_denials")', 1)[1].split("except Exception:", 1)[0]
        self.assertLess(block.index('.fill("")'), block.index('has_text="e@example.com"'))

    def test_admin_diagnostic_prepares_its_own_definition_and_sdk(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/verify.yml").read_text()
        admin_step = workflow.split("      - name: Prepare the pinned FaceReview admin console browser journey\n", 1)[1].split("      - name:", 1)[0]
        self.assertIn('definition="$RUNNER_TEMP/aip-facereview-admin.aip"', admin_step)
        self.assertIn('echo "AIP_FACEREVIEW_DEFINITION_FILE=$definition" >> "$GITHUB_ENV"', admin_step)
        self.assertIn('node product/tools/build-sdk.mjs --out "$sdk"', admin_step)
        self.assertIn('npm pack --ignore-scripts --pack-destination "$RUNNER_TEMP" "$sdk"', admin_step)
        self.assertIn('apply-admin-ui-overlay.sh "$admin" "$definition"', admin_step)
        self.assertIn('python3 -m venv "$RUNNER_TEMP/aip-browser"', admin_step)
        self.assertIn('"$RUNNER_TEMP/aip-browser/bin/pip" install --no-cache-dir playwright==1.64.0', admin_step)
        self.assertIn('"$RUNNER_TEMP/aip-browser/bin/python" -m playwright install --with-deps chromium', admin_step)

    def test_profile_browser_trace_init_script_executes_instead_of_only_declaring_arrow(self):
        source = Path(__file__).with_name("facereview-ui-profile-browser.py").read_text()
        tree = ast.parse(source)
        scripts = [
            node.args[0].value
            for node in ast.walk(tree)
            if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr == "add_init_script"
            and node.args
            and isinstance(node.args[0], ast.Constant)
            and isinstance(node.args[0].value, str)
        ]
        self.assertEqual(len(scripts), 1)
        self.assertTrue(scripts[0].lstrip().startswith("(() => {"))
        self.assertTrue(scripts[0].rstrip().endswith("})();"))

    def test_profile_animation_gate_ignores_unrelated_page_motion(self):
        evidence = {"animations": [
            {"name": "other", "play_state": "running"},
            {"name": "fadeInUp", "play_state": "finished"},
        ]}
        self.assertFalse(profile_content_animation_incomplete(evidence))
        evidence["animations"].append({"name": "fadeInUp", "play_state": "running"})
        self.assertTrue(profile_content_animation_incomplete(evidence))

    def test_auth_trace_ruler_recognizes_the_live_reissue_branch_inputs(self):
        summary = auth_transport_trace_evidence([{
            "method": "POST", "path": "/apply", "status": 200,
            "authorization_header_present": True, "content_type": "application/json; charset=utf-8",
            "body_captured": True, "ok": False, "code": "WIRE.TOKEN_EXPIRED",
            "fetch_frames": ["fetchWithRefresh", "connectTypedExtensions", "typedClient", "postWithToken"],
            "request_started_ms": 10, "response_headers_ms": 12,
            "response_body_complete_ms": 13, "request_finished_ms": 13, "network_idle_completed_ms": 15,
            "authorization": "Bearer must-not-escape", "body": "must-not-escape",
        }])
        self.assertEqual(summary["reauth_branch_inputs_observed"], 1)
        self.assertEqual(summary["expired_refusal_count"], 1)
        self.assertEqual(summary["requests"][0]["content_type"], "application/json")
        self.assertEqual(summary["requests"][0]["envelope_shape"], "error")
        self.assertTrue(summary["requests"][0]["body_completed_before_network_idle"])
        self.assertNotIn("must-not-escape", json.dumps(summary))

    def test_auth_trace_ruler_rejects_negative_controls_for_auth_header_media_type_and_sdk_path(self):
        base = {
            "method": "POST", "path": "/apply", "status": 200,
            "authorization_header_present": True, "content_type": "application/json",
            "body_captured": True, "ok": False, "code": "WIRE.TOKEN_EXPIRED",
            "fetch_frames": ["fetchWithRefresh", "connectTypedExtensions", "typedClient", "postWithToken"],
        }
        for mutation in (
            {"authorization_header_present": False},
            {"content_type": "text/html"},
            {"fetch_frames": ["connectTypedExtensions", "typedClient", "postWithToken"]},
            {"body_captured": False},
            {"request_finished_ms": 16, "network_idle_completed_ms": 15},
        ):
            with self.subTest(mutation=mutation):
                summary = auth_transport_trace_evidence([{**base, **mutation}])
                self.assertEqual(summary["expired_refusal_count"], 1)
                self.assertEqual(summary["reauth_branch_inputs_observed"], 0)

    def test_auth_trace_ruler_redacts_unrecognized_paths(self):
        summary = auth_transport_trace_evidence([{
            "method": "GET", "path": "/users/private-member-id", "status": 200,
            "authorization_header_present": True, "content_type": "application/json",
        }])
        self.assertEqual(summary["requests"][0]["path"], "other")
        self.assertNotIn("private-member-id", json.dumps(summary))

    def test_profile_render_evidence_distinguishes_active_opacity_animation_from_hidden_content(self):
        evidence = profile_render_evidence({
            "path": "/my",
            "api_bodies_settled": True,
            "body_text_length": 133,
            "viewport": {"width": 1280, "height": 900},
            "nodes": {
                "my-page-container": {
                    "display": "flex", "visibility": "visible", "opacity": 0.02,
                    "effective_opacity": 0.02, "transform": "applied", "animation_name": "fadeInUp",
                    "animation_duration_ms": 500, "animation_delay_ms": 0, "animation_fill_mode": "both",
                    "animation_play_state": "running", "transition_properties": ["none"],
                    "transition_duration_ms": 0, "rect": {"width": 1200.5, "height": 700},
                },
                "my-page-user-container": {
                    "display": "flex", "visibility": "visible", "opacity": 1,
                    "effective_opacity": 0.02, "rect": {"width": 1000, "height": 90},
                },
            },
            "animations": [{
                "name": "fadeInUp", "play_state": "running", "current_ms": 10,
                "duration_ms": 500, "delay_ms": 50, "properties": ["opacity", "transform"],
            }],
        })
        self.assertEqual(evidence["route"], "my")
        self.assertTrue(evidence["api_bodies_settled"])
        self.assertFalse(evidence["nodes"][0]["visually_ready"])
        self.assertFalse(evidence["nodes"][1]["visually_ready"])
        self.assertEqual(evidence["nodes"][0]["rect"]["width"], 1201)
        self.assertEqual(evidence["nodes"][0]["animation_name"], "fadeInUp")
        self.assertEqual(evidence["nodes"][0]["animation_fill_mode"], "both")
        self.assertEqual(evidence["nodes"][0]["transform"], "applied")
        self.assertEqual(evidence["animations"][0]["name"], "fadeInUp")
        self.assertNotIn("body_text", evidence)

    def test_profile_render_evidence_keeps_only_allowlisted_css_and_route_metadata(self):
        evidence = profile_render_evidence({
            "path": "/my?access_token=must-not-escape",
            "api_bodies_settled": True,
            "body_text_length": 100000,
            "viewport": {"width": 1280, "height": 900},
            "nodes": {"my-page-container": {
                "display": "flex", "visibility": "visible", "opacity": 1,
                "effective_opacity": 1, "rect": {"width": 1200, "height": 700},
                "private_text": "must not escape",
            }},
            "animations": [{"name": "secret-animation", "play_state": "running",
                            "current_ms": 10, "duration_ms": 500, "properties": ["opacity"]}],
            "token": "must-not-escape",
        })
        self.assertEqual(evidence["route"], "my")
        self.assertTrue(evidence["nodes"][0]["visually_ready"])
        self.assertEqual(evidence["body_text_length"], 10000)
        self.assertEqual(evidence["animations"][0]["name"], "other")
        self.assertNotIn("must-not-escape", json.dumps(evidence))
        self.assertNotIn("private_text", json.dumps(evidence))

    def test_youtube_player_fixture_matches_only_the_provider_api_script(self):
        stub = youtube_iframe_api_stub("https://www.youtube.com/iframe_api", "script")
        self.assertIsNotNone(stub)
        self.assertNotIn("window.YT", stub)
        self.assertIsNone(youtube_iframe_api_stub("https://www.youtube.com/watch?v=abc", "document"))
        self.assertIsNone(youtube_iframe_api_stub("https://example.com/iframe_api", "script"))

    def test_response_shape_keeps_only_safe_envelope_metadata(self):
        self.assertEqual(
            response_envelope_evidence({"rows": [{"secret": "omit"}], "private": "omit"}),
            {"json_kind": "object", "known_keys": ["rows"], "unknown_key_count": 1, "ok_type": "missing", "rows_length": 1},
        )
        self.assertEqual(response_envelope_evidence({"ok": "missing"})["ok_type"], "str")

    def test_read_evidence_unwraps_sdk_envelope_and_identifies_known_rejected_query_key(self):
        summary = read_request_evidence({
            "query": {
                "read": "Comment",
                "select": ["id", "content", "is_modified", "created_at"],
                "filter": [{"field": "video", "op": "eq", "value": "fixture-id-must-not-escape"}],
                "limit": 100,
                "traverse": {"user": {"select": ["id", "name"]}},
            },
        })
        self.assertEqual(summary["request_envelope_keys"], ["query"])
        self.assertEqual(summary["operation"], "read")
        self.assertEqual(summary["read_resource"], "Comment")
        self.assertEqual(summary["rejected_query_keys"], ["traverse"])
        self.assertEqual(summary["unknown_query_key_count"], 1)
        self.assertEqual(summary["select_fields"], ["id", "content", "is_modified", "created_at"])
        self.assertNotIn("fixture-id-must-not-escape", repr(summary))

    def test_read_evidence_summarizes_only_allowlisted_nested_relation_fields(self):
        summary = read_request_evidence({"query": {
            "read": "Comment",
            "select": ["id", "content", {"user": {"select": ["id", "name", "private"]}}],
        }})
        self.assertEqual(summary["select_fields"], ["id", "content"])
        self.assertEqual(summary["select_relations"], [{"relation": "user", "fields": ["id", "name"]}])
        self.assertEqual(summary["unknown_select_field_count"], 1)

    def test_read_evidence_allows_the_fields_used_by_the_pinned_member_profile(self):
        summary = read_request_evidence({"query": {
            "read": "User",
            "select": ["id", "email", "name", "role", "profile_image_id", "is_tutorial_done", "is_verify_email_done", "created_at"],
            "limit": 1,
        }})
        self.assertEqual(summary["select_fields"], [
            "id", "email", "name", "role", "profile_image_id", "is_tutorial_done", "is_verify_email_done", "created_at",
        ])
        self.assertEqual(summary["unknown_select_field_count"], 0)

    def test_browser_failure_type_is_class_only(self):
        error = TimeoutError("must not be retained: member@example.invalid")
        self.assertEqual(safe_browser_failure_type(error), "TimeoutError")
        self.assertNotIn("member@example.invalid", safe_browser_failure_type(error))

    def test_browser_failure_state_keeps_only_route_and_allowlisted_bookmark_controls(self):
        self.assertEqual(safe_browser_route_class("http://localhost:5173/watch/" + VIDEO_ID + "?token=secret", VIDEO_ID), "watch_fixture")
        self.assertEqual(safe_browser_route_class("http://localhost:5173/account/private", VIDEO_ID), "other")
        self.assertEqual(safe_bookmark_control_evidence([
            {"label": "즐겨찾기 해제", "pressed": "true"},
            {"label": "private-value", "pressed": "other"},
        ]), [
            {"label": "즐겨찾기 해제", "pressed": "true"},
            {"label": "unrecognized", "pressed": "unrecognized"},
        ])

    def test_http_200_read_error_is_not_classified_as_success(self):
        rows = [
            {"path": "/read", "status": 200, "ok": False, "code": "WIRE.UNKNOWN_KEY"},
            {"path": "/read", "status": 200, "ok": True},
            {"path": "/auth/reissue", "status": 200, "ok": False, "code": "WIRE.TOKEN_EXPIRED"},
        ]
        self.assertEqual(failed_read_requests(rows), [rows[0]])
        self.assertEqual(safe_wire_code("WIRE.UNKNOWN_KEY"), "WIRE.UNKNOWN_KEY")
        self.assertIsNone(safe_wire_code("private text"))
        # Refusals the admin journey must tell apart: a from-state mismatch is not a maxRows overflow.
        self.assertEqual(safe_wire_code("WIRE.INVALID_STATE"), "WIRE.INVALID_STATE")
        self.assertEqual(safe_wire_code("WIRE.FORBIDDEN"), "WIRE.FORBIDDEN")
        self.assertEqual(safe_wire_code("WIRE.MISSING_TARGET"), "WIRE.MISSING_TARGET")

    def test_console_summary_keeps_only_classification_and_local_caller(self):
        summary = console_error_evidence(
            "error",
            "request failed WIRE.UNKNOWN_KEY for secret-member-value",
            {"url": "http://localhost:5173/src/api/watch.ts?t=secret", "lineNumber": 17, "columnNumber": 4},
        )
        self.assertEqual(summary, {
            "console_message_type": "error",
            "classification": "wire_response_error",
            "source": "/src/api/watch.ts:17:4",
            "wire_code": "WIRE.UNKNOWN_KEY",
        })
        self.assertNotIn("secret-member-value", repr(summary))
        self.assertEqual(
            console_error_evidence("error", "failed", {"url": "https://outside.example/user/private"})["source"],
            "unmapped",
        )

    def test_external_media_failure_records_only_host_class_resource_type_and_allowlisted_code(self):
        self.assertEqual(
            external_media_failure_evidence(
                "https://r7---sn.googlevideo.com/videoplayback?token=secret", "media", "net::ERR_ABORTED"
            ),
            {"host_class": "googlevideo", "resource_type": "media", "failure_code": "net::ERR_ABORTED"},
        )
        self.assertIsNone(external_media_failure_evidence("https://googlevideo.com.evil.test/x", "media", "secret"))

    def test_caller_trace_keeps_only_local_src_paths(self):
        self.assertEqual(safe_browser_caller_frames([
            {"url": "http://localhost:5173/src/api/watch.ts?t=secret", "lineNumber": 17, "columnNumber": 4},
            {"url": "https://outside.example/member/private.ts", "lineNumber": 1, "columnNumber": 1},
        ]), [{"path": "/src/api/watch.ts", "line": 17, "column": 4}])

    def test_read_evidence_keeps_query_shape_and_omits_filter_values(self):
        summary = read_request_evidence({
            "read": "VideoBookmark",
            "select": ["id"],
            "filter": [{"field": "video", "op": "eq", "value": "fixture-id-must-not-escape"}],
            "limit": 1,
            "token": "secret-must-not-escape",
        })
        self.assertEqual(summary["read_resource"], "VideoBookmark")
        self.assertEqual(summary["recognized_query_keys"], ["filter", "limit", "read", "select"])
        self.assertEqual(summary["unknown_query_key_count"], 1)
        self.assertEqual(summary["select_fields"], ["id"])
        self.assertEqual(summary["filter_fields"], [{"field": "video", "op": "eq", "value_type": "str"}])
        self.assertNotIn("fixture-id-must-not-escape", repr(summary))
        self.assertNotIn("secret-must-not-escape", repr(summary))

    def test_only_the_fixture_thumbnail_image_can_be_redirected_to_local_fixture(self):
        local_thumbnail = "https://127.0.0.1:49152/fixture-thumbnail.svg"
        self.assertEqual(
            fixture_thumbnail_redirect_url(
                f"https://img.youtube.com/vi/dQw4w9WgXcQ/mqdefault.jpg",
                "image",
                "dQw4w9WgXcQ",
                local_thumbnail,
            ),
            local_thumbnail,
        )

    def test_fixture_thumbnail_rule_rejects_other_urls_and_resource_types(self):
        local_thumbnail = "https://127.0.0.1:49152/fixture-thumbnail.svg"
        cases = (
            ("https://img.youtube.com/vi/anotherVideo/mqdefault.jpg", "image"),
            ("https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg", "image"),
            ("https://img.youtube.com/vi/dQw4w9WgXcQ/hqdefault.jpg", "image"),
            ("https://img.youtube.com/vi/dQw4w9WgXcQ/mqdefault.jpg", "media"),
            ("https://img.youtube.com.evil.example/vi/dQw4w9WgXcQ/mqdefault.jpg", "image"),
        )
        for url, resource_type in cases:
            with self.subTest(url=url, resource_type=resource_type):
                self.assertIsNone(
                    fixture_thumbnail_redirect_url(url, resource_type, "dQw4w9WgXcQ", local_thumbnail)
                )

    def test_request_evidence_keeps_only_allowlisted_extension_name_page_and_emotion(self):
        request = {
            "extension": "Video.homeVideoAll",
            "input": {"page": 2, "size": 20, "emotion": "happy", "user_note": "do not save"},
            "token": "must-not-escape",
        }
        self.assertEqual(
            extension_request_evidence(request),
            {"extension": "Video.homeVideoAll", "page": 2, "emotion": "happy"},
        )

    def test_response_evidence_summarizes_fixture_membership_without_values(self):
        payload = {
            "ok": True,
            "output": {
                "videos": '[{"video_id":"aaaaaaaa-0000-4000-8000-000000000001","title":"private title"}]',
                "total": 1,
            },
            "token": "must-not-escape",
        }
        summary = extension_response_evidence("Video.homeVideoAll", payload, VIDEO_ID)
        self.assertEqual(summary, {"response_list_length": 1, "contains_fixture_video": True})
        self.assertNotIn("private title", repr(summary))
        self.assertNotIn("must-not-escape", repr(summary))

    def test_category_response_counts_nested_videos_without_recording_titles(self):
        payload = {
            "output": {
                "categories": '[{"category_name":"animal","videos":[{"video_id":"aaaaaaaa-0000-4000-8000-000000000001","title":"private title"}]}]'
            }
        }
        self.assertEqual(
            extension_response_evidence("Video.homeCategory", payload, VIDEO_ID),
            {"response_list_length": 1, "category_count": 1, "contains_fixture_video": True},
        )

    def test_body_failure_reason_separates_playwright_transport_failures_without_keeping_messages(self):
        class TargetClosedError(Exception):
            pass

        class Error(Exception):
            pass

        class TimeoutError(Exception):  # noqa: A001 - mirrors the Playwright class name.
            pass

        self.assertEqual(body_failure_reason(TargetClosedError("Target page, context or browser has been closed")), "target_closed")
        self.assertEqual(
            body_failure_reason(Error("Response.json: Protocol error (Network.getResponseBody): No resource with given identifier")),
            "resource_evicted",
        )
        self.assertEqual(body_failure_reason(Error("Protocol error (Network.getResponseBody): No data found for resource")), "resource_evicted")
        self.assertEqual(body_failure_reason(Error("net::ERR_ABORTED secret-token-123")), "playwright_error")
        self.assertEqual(body_failure_reason(TimeoutError("slow")), "timeout")
        self.assertEqual(body_failure_reason(ValueError("anything")), "unrecognized")

    def test_body_capture_distinguishes_unavailable_from_decode_error_and_valid_json(self):
        unavailable = body_capture_evidence(None, "application/json", "resource_evicted")
        self.assertEqual(unavailable["body_captured"], False)
        self.assertEqual(unavailable["failure_reason"], "resource_evicted")
        self.assertEqual(unavailable["response_shape"], {"json_kind": "body_unavailable", "reason": "resource_evicted"})
        self.assertNotIn("byte_length", unavailable)

        valid = body_capture_evidence(b'{"ok":true,"rows":[{"secret":"omit"}]}', "application/json; charset=utf-8", None)
        self.assertEqual(valid["body_captured"], True)
        self.assertEqual(valid["content_type"], "application/json")
        self.assertEqual(valid["byte_length"], 38)
        self.assertEqual(len(valid["sha256_16"]), 16)
        self.assertEqual(valid["response_shape"]["json_kind"], "object")
        self.assertEqual(valid["response_shape"]["rows_length"], 1)
        self.assertNotIn("omit", json.dumps(valid))

        html = body_capture_evidence(b"<html>private</html>", "text/html", None)
        self.assertEqual(html["body_captured"], True)
        self.assertEqual(html["response_shape"], {"json_kind": "json_decode_error", "decode_error_class": "JSONDecodeError", "first_byte_class": "html"})
        self.assertEqual(html["content_type"], "text/html")
        self.assertNotIn("private", json.dumps(html))

        empty = body_capture_evidence(b"", "application/json", None)
        self.assertEqual(empty["response_shape"]["first_byte_class"], "empty")
        self.assertEqual(empty["byte_length"], 0)
        self.assertEqual(body_capture_evidence(b"\xff\xfe", None, None)["response_shape"]["decode_error_class"], "UnicodeDecodeError")
        self.assertEqual(body_capture_evidence(b"{}", "application/vnd.private", None)["content_type"], "other")
        self.assertEqual(body_capture_evidence(b"{}", None, None)["content_type"], "missing")

    def test_app_observation_keeps_only_allowlisted_fields(self):
        event = {
            "seq": 1, "phase": "received", "started_ms": 10, "ended_ms": 70, "status": 200,
            "content_type": "application/json", "byte_length": 60, "sha256_16": "0123456789abcdef",
            "parse": "object", "ok": True, "rows_length": 0, "token": "must-not-escape", "cookie": "x",
        }
        kept = app_bookmark_read_observation(event)
        self.assertEqual(kept["seq"], 1)
        self.assertEqual(kept["parse"], "object")
        self.assertNotIn("token", kept)
        self.assertNotIn("cookie", kept)
        self.assertIsNone(app_bookmark_read_observation({"seq": "1"}))
        self.assertIsNone(app_bookmark_read_observation("not an event"))
        rejected = app_bookmark_read_observation({"seq": 2, "phase": "fetch_rejected", "error_class": "SecretError", "ended_ms": 5})
        self.assertEqual(rejected["error_class"], "unrecognized")

    def test_compare_requires_matching_bytes_and_never_counts_missing_capture_as_match(self):
        app = {"seq": 1, "phase": "received", "status": 200, "byte_length": 60, "sha256_16": "aaaaaaaaaaaaaaaa", "parse": "object"}
        captured = {"body_captured": True, "byte_length": 60, "sha256_16": "aaaaaaaaaaaaaaaa"}
        self.assertEqual(compare_bookmark_read_capture(captured, app), "match")
        self.assertEqual(compare_bookmark_read_capture({**captured, "sha256_16": "bbbbbbbbbbbbbbbb"}, app), "mismatch")
        self.assertEqual(
            compare_bookmark_read_capture({"body_captured": False, "failure_reason": "target_closed"}, app),
            "playwright_body_unavailable",
        )
        self.assertEqual(compare_bookmark_read_capture(captured, None), "app_observation_missing")
        self.assertEqual(
            compare_bookmark_read_capture(captured, {"seq": 1, "phase": "fetch_rejected"}),
            "app_observation_incomplete",
        )

    def test_timeline_phase_places_an_event_relative_to_reload_and_close(self):
        marks = {"reload_start": 100, "reload_return": 180, "browser_close_start": 400}
        self.assertEqual(timeline_phase(50, marks), "before_reload")
        self.assertEqual(timeline_phase(150, marks), "during_reload")
        self.assertEqual(timeline_phase(200, marks), "after_reload")
        self.assertEqual(timeline_phase(500, marks), "after_close_start")
        self.assertEqual(timeline_phase(50, {}), "before_any_mark")
        self.assertEqual(timeline_phase(None, marks), "not_observed")

    def test_admin_request_evidence_keeps_only_schema_names(self):
        self.assertEqual(admin_request_evidence("/read", {"query": {"aggregate": "User.totalCount", "input": {}, "filter": [{"field": "name", "op": "like", "value": "secret"}]}}), {"aggregate": "User.totalCount"})
        self.assertEqual(admin_request_evidence("/read", {"query": {"aggregate": "Video.totalCount", "filter": [{"field": "category", "op": "eq", "value": "animal"}]}}), {"aggregate": "Video.totalCount", "category": "animal"})
        self.assertEqual(admin_request_evidence("/read", {"query": {"read": "VideoRequest", "select": ["id"], "filter": [{"field": "id", "op": "eq", "value": "v"}]}}), {"read": "VideoRequest"})
        self.assertEqual(admin_request_evidence("/extension", {"extension": "User.systemStatus", "input": {}}), {"extension": "User.systemStatus"})
        self.assertEqual(admin_request_evidence("/jobs/enqueue", {"operation": "approveVideoRequest", "key": "k", "input": {"youtube_title": "private title"}}), {"operation": "approveVideoRequest"})
        # The SDK wraps a write as {key, request}; the transition name is the only thing kept.
        self.assertEqual(admin_request_evidence("/apply", {"key": "k", "request": {"apply": "VideoRequest.reject", "values": {"admin_comment": "private reason"}}}), {"apply": "VideoRequest.reject"})
        self.assertEqual(admin_request_evidence("/apply", {"key": "k", "request": {"atomic": [{"create": "X"}]}}), {"apply": "atomic"})
        self.assertEqual(admin_request_evidence("/apply", {"key": "k", "request": {"create": "VideoLike", "values": {}}}), {"apply": "create"})

    def test_admin_request_evidence_marks_unrecognized_names_and_ignores_other_paths(self):
        self.assertEqual(admin_request_evidence("/read", {"query": {"read": "User; DROP TABLE user"}}), {"read": "unrecognized"})
        self.assertEqual(admin_request_evidence("/extension", {"extension": 7}), {"extension": "unrecognized"})
        self.assertEqual(admin_request_evidence("/auth/login", {"email": "a@example.com", "password": "never recorded"}), {})
        self.assertEqual(admin_request_evidence("/read", None), {})
        self.assertEqual(admin_request_evidence("/read", {"query": "not an object"}), {})

    def test_safe_page_error_name_uses_the_javascript_name_not_the_wrapper_class(self):
        class Wrapped(Exception):
            name = "TypeError"

        class Odd(Exception):
            name = "Custom message with a secret"

        self.assertEqual(safe_page_error_name(Wrapped("cannot read properties of null")), "TypeError")
        self.assertEqual(safe_page_error_name(Odd("x")), "unrecognized")
        self.assertEqual(safe_page_error_name(ValueError("no name attribute")), "unrecognized")

    def test_aggregate_count_reads_only_one_non_negative_integer_row(self):
        self.assertEqual(aggregate_count({"ok": True, "rows": [3]}), 3)
        self.assertEqual(aggregate_count({"ok": True, "rows": [0]}), 0)
        for refused in ({"ok": False, "code": "WIRE.FORBIDDEN"}, {"ok": True, "rows": []}, {"ok": True, "rows": [1, 2]},
                        {"ok": True, "rows": [True]}, {"ok": True, "rows": [-1]}, {"ok": True, "rows": [1.5]},
                        {"ok": True, "rows": ["3"]}, {"ok": True, "rows": [None]}, {"ok": True}, [], None):
            self.assertIsNone(aggregate_count(refused), refused)


if __name__ == "__main__":
    unittest.main()
