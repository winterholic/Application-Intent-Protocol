import unittest

from product.tests.facereview_ui_browser_evidence import extension_request_evidence, extension_response_evidence


VIDEO_ID = "aaaaaaaa-0000-4000-8000-000000000001"


class BrowserEvidenceTests(unittest.TestCase):
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
