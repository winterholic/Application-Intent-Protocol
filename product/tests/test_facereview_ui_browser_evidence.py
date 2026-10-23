import unittest

from product.tests.facereview_ui_browser_evidence import (
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
)


VIDEO_ID = "aaaaaaaa-0000-4000-8000-000000000001"


class BrowserEvidenceTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
