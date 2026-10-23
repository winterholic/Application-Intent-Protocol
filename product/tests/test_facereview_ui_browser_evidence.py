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
)


VIDEO_ID = "aaaaaaaa-0000-4000-8000-000000000001"


class BrowserEvidenceTests(unittest.TestCase):
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

    def test_workflow_uses_full_e2e_unless_member_diagnostic_is_explicit(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/verify.yml").read_text()
        step = workflow.split("      - name: Verify real HTTP to Node and Python workers to MySQL\n", 1)[1].split("      - name:", 1)[0]
        run = step.split("        run:", 1)[1].strip("\n")
        if run.strip().startswith("|"):
            command = "\n".join(line.removeprefix("          ") for line in run.splitlines()[1:])
        else:
            command = run.strip()
        for value, expected in [("", "run-ci"), ("false", "run-ci"), ("true", "run-ci-member")]:
            with self.subTest(member_ui_diagnostic=value):
                # Execute the workflow's actual shell branch without starting its database fixtures.
                shell = 'bash() { printf "%s\\n" "$2"; };\n' + command.replace("${{ inputs.member_ui_diagnostic }}", value)
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

    def test_focused_member_contract_stops_before_external_store_suites(self):
        repo = Path(__file__).resolve().parents[2]
        workflow = (repo / ".github/workflows/verify.yml").read_text()
        script = (repo / "scripts/verify-mysql-worker-e2e.sh").read_text()
        self.assertIn("scripts/verify-mysql-worker-e2e.sh run-ci-member", workflow)
        self.assertIn("run | run-ci | run-ci-member | down", script)
        self.assertLess(script.index('AIP_VERIFY_FAILURE_FILE="$AIP_E2E_FAILURE_FILE"'),
                        script.index('source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/verify-safe-locator.sh"'))
        member_auth_run = script.index('aip_verify_stage "FaceReview member authentication E2E test"')
        member_auth_check = script.index('member SDK journey:', member_auth_run)
        member_stop = script.index('if [[ "$mode" == run-ci-member ]]', member_auth_check)
        member_exit = script.index("exit 0", member_stop)
        recommendation_gate = script.index('needs AIP_TEST_MONGO_URL', member_exit)
        effects_gate = script.index('needs AIP_TEST_REDIS_URL', recommendation_gate)
        self.assertLess(member_auth_check, member_exit)
        self.assertLess(member_exit, recommendation_gate)
        self.assertLess(recommendation_gate, effects_gate)
        self.assertGreaterEqual(script.count('if [[ "$mode" == run-ci || "$mode" == run-ci-member ]]; then'), 2)
        self.assertIn('scripts/verify-mysql-worker-e2e.sh run-ci', script)
        self.assertIn('mysql_facereview_recommend', script)
        self.assertIn('mysql_external_effects', script)

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
