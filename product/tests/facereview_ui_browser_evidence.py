"""Small allowlist-only summaries for the live FaceReview browser journey."""

import json
from typing import Any
from urllib.parse import urlsplit


LIST_EXTENSIONS = {
    "Video.homeVideoAll",
    "Video.homeCategory",
    "Video.homePersonalized",
}
EMOTIONS = {"all", "happy", "sad", "neutral", "surprise", "angry"}


def fixture_thumbnail_redirect_url(
    url: str, resource_type: str, fixture_youtube_id: str, local_thumbnail_url: str
) -> str | None:
    """Route only the fixture's availability-check image to its local static image."""
    request = urlsplit(url)
    local = urlsplit(local_thumbnail_url)
    if (
        resource_type == "image"
        and request.scheme == "https"
        and request.hostname == "img.youtube.com"
        and request.path == f"/vi/{fixture_youtube_id}/mqdefault.jpg"
        and not request.query
        and local.scheme == "https"
        and local.hostname == "127.0.0.1"
        and local.port is not None
        and local.path == "/fixture-thumbnail.svg"
    ):
        return local_thumbnail_url
    return None


def extension_request_evidence(body: Any) -> dict[str, object]:
    """Return only the allowlisted extension name and page/emotion inputs."""
    if not isinstance(body, dict):
        return {}
    name = body.get("extension", body.get("name"))
    if name not in LIST_EXTENSIONS:
        return {}
    inputs = body.get("input")
    if not isinstance(inputs, dict):
        inputs = {}
    row: dict[str, object] = {"extension": name}
    page = inputs.get("page")
    if isinstance(page, int) and not isinstance(page, bool) and 1 <= page <= 100000:
        row["page"] = page
    emotion = inputs.get("emotion")
    if isinstance(emotion, str) and emotion in EMOTIONS:
        row["emotion"] = emotion
    return row


def _list(value: Any) -> list[Any] | None:
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except (TypeError, ValueError):
            return None
    return value if isinstance(value, list) else None


def _contains_video(items: list[Any], fixture_video_id: str) -> bool:
    return any(isinstance(item, dict) and item.get("video_id") == fixture_video_id for item in items)


def extension_response_evidence(extension: str, payload: Any, fixture_video_id: str) -> dict[str, object]:
    """Summarize only list lengths and whether the dedicated fixture video is present."""
    if extension not in LIST_EXTENSIONS or not isinstance(payload, dict):
        return {}
    output = payload.get("output")
    if not isinstance(output, dict):
        output = {}

    if extension == "Video.homeCategory":
        categories = _list(output.get("categories"))
        videos = [video for category in categories or [] if isinstance(category, dict)
                  for video in (_list(category.get("videos")) or [])]
        return {
            "response_list_length": len(videos) if categories is not None else None,
            "category_count": len(categories) if categories is not None else None,
            "contains_fixture_video": _contains_video(videos, fixture_video_id),
        }

    videos = _list(output.get("videos"))
    return {
        "response_list_length": len(videos) if videos is not None else None,
        "contains_fixture_video": _contains_video(videos or [], fixture_video_id),
    }
