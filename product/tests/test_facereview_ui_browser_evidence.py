import unittest

from product.tests.facereview_ui_browser_evidence import (
    extension_request_evidence,
    extension_response_evidence,
    fixture_thumbnail_redirect_url,
    read_request_evidence,
)


VIDEO_ID = "aaaaaaaa-0000-4000-8000-000000000001"


class BrowserEvidenceTests(unittest.TestCase):
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
