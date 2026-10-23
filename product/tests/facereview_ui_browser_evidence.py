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
READ_FIELDS = {
    "Video": {"id", "youtube_url", "title", "channel_name", "category", "duration", "view_count", "is_deleted"},
    "VideoBookmark": {"id", "video", "user", "created_at"},
    "VideoLike": {"id", "video", "user", "created_at"},
    "Comment": {"id", "video", "user", "content", "is_deleted", "created_at"},
    "User": {"id", "email", "name", "role", "is_deleted", "created_at"},
}
READ_QUERY_KEYS = {"read", "select", "filter", "sort", "limit", "offset", "after"}
FILTER_OPS = {"eq", "ne", "lt", "lte", "gt", "gte", "in", "contains", "like"}


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


def read_request_evidence(body: Any) -> dict[str, object]:
    """Summarize an allowlisted read query's structure without recording filter values."""
    if not isinstance(body, dict):
        return {"read_resource": "unrecognized", "recognized_query_keys": [], "unknown_query_key_count": 0}

    resource = body.get("read")
    fields = READ_FIELDS.get(resource) if isinstance(resource, str) else None
    summary: dict[str, object] = {
        "read_resource": resource if fields is not None else "unrecognized",
        "recognized_query_keys": sorted(key for key in body if key in READ_QUERY_KEYS),
        "unknown_query_key_count": sum(key not in READ_QUERY_KEYS for key in body),
    }
    select = body.get("select")
    if isinstance(select, list):
        safe_fields = [field for field in select if isinstance(field, str) and fields and field in fields]
        summary["select_fields"] = safe_fields
        summary["unknown_select_field_count"] = len(select) - len(safe_fields)

    filters = body.get("filter")
    if isinstance(filters, list):
        safe_filters: list[dict[str, str]] = []
        unknown_filter_key_count = 0
        for item in filters:
            if not isinstance(item, dict):
                unknown_filter_key_count += 1
                continue
            unknown_filter_key_count += sum(key not in {"field", "op", "value"} for key in item)
            field = item.get("field")
            op = item.get("op")
            if isinstance(field, str) and fields and field in fields:
                safe_filters.append({
                    "field": field,
                    "op": op if isinstance(op, str) and op in FILTER_OPS else "unrecognized",
                    "value_type": type(item.get("value")).__name__,
                })
            else:
                unknown_filter_key_count += 1
        summary["filter_fields"] = safe_filters
        summary["unknown_filter_key_count"] = unknown_filter_key_count

    limit = body.get("limit")
    if isinstance(limit, int) and not isinstance(limit, bool) and 0 <= limit <= 100000:
        summary["limit"] = limit
    return summary


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
