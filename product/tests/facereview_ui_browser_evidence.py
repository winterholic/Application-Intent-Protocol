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
    "Comment": {"id", "video", "user", "content", "is_modified", "is_deleted", "created_at", "updated_at"},
    "User": {
        "id", "email", "name", "role", "profile_image_id", "is_tutorial_done",
        "is_verify_email_done", "is_deleted", "created_at",
    },
    "UserFavoriteGenre": {"id", "user", "genre", "created_at", "updated_at"},
}
READ_RELATIONS = {"Comment": {"user": {"id", "name"}}}
SAFE_FAILURE_TYPES = {"AssertionError", "Error", "TimeoutError", "TargetClosedError", "RuntimeError"}
SAFE_BOOKMARK_LABELS = {"즐겨찾기 추가", "즐겨찾기 해제"}
READ_QUERY_KEYS = {"read", "select", "filter", "sort", "limit", "offset", "after"}
READ_QUERY_DIAGNOSTIC_KEYS = READ_QUERY_KEYS | {"traverse"}
READ_ENVELOPE_KEYS = {"query", "contractFingerprint"}
SAFE_WIRE_CODES = {
    "WIRE.UNKNOWN_KEY",
    "WIRE.CONTRACT_MISMATCH",
    "WIRE.UNAUTHENTICATED",
    "WIRE.TOKEN_EXPIRED",
    "WIRE.BAD_VALUE",
    "WIRE.NOT_EXPOSED",
    "WIRE.ACCESS_DENIED",
    "WIRE.FORBIDDEN",
    "WIRE.FIELD_NOT_EXPOSED",
    "WIRE.UNKNOWN_FIELD",
    "WIRE.READ_ONLY",
    "WIRE.INVALID_QUERY",
    "WIRE.OUTPUT_INVALID",
}


SAFE_NETWORK_FAILURES = {
    "net::ERR_ABORTED",
    "net::ERR_BLOCKED_BY_CLIENT",
    "net::ERR_CONNECTION_CLOSED",
    "net::ERR_CONNECTION_REFUSED",
    "net::ERR_FAILED",
    "net::ERR_NAME_NOT_RESOLVED",
    "net::ERR_TIMED_OUT",
}
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


def youtube_iframe_api_stub(url: str, resource_type: str) -> str | None:
    """Return a no-op player API only for YouTube's loader script; never fetch or play media."""
    parsed = urlsplit(url)
    if resource_type != "script" or parsed.hostname not in {"youtube.com", "www.youtube.com"} or parsed.path != "/iframe_api":
        return None
    return """(() => {
      const noop = () => {};
      const PlayerState = {UNSTARTED: -1, ENDED: 0, PLAYING: 1, PAUSED: 2, BUFFERING: 3, CUED: 5};
      function Player(_target, options) {
        Object.assign(this, {
          playVideo: noop, pauseVideo: noop, stopVideo: noop, seekTo: noop,
          mute: noop, unMute: noop, setVolume: noop, getVolume: () => 0,
          getCurrentTime: () => 0, getDuration: () => 0,
          getPlayerState: () => PlayerState.UNSTARTED, destroy: noop,
        });
        queueMicrotask(() => options?.events?.onReady?.({target: this}));
      }
      window.YT = {Player, PlayerState};
      window.onYouTubeIframeAPIReady?.();
    })();"""


def read_request_evidence(body: Any) -> dict[str, object]:
    """Summarize an allowlisted read query's structure without recording filter values."""
    if not isinstance(body, dict):
        return {
            "operation": "unrecognized",
            "read_resource": "unrecognized",
            "request_envelope_keys": [],
            "unknown_envelope_key_count": 0,
            "recognized_query_keys": [],
            "rejected_query_keys": [],
            "unknown_query_key_count": 0,
        }

    wrapped_query = body.get("query")
    if isinstance(wrapped_query, dict):
        query = wrapped_query
        envelope_keys = sorted(key for key in body if key in READ_ENVELOPE_KEYS)
        unknown_envelope_key_count = sum(key not in READ_ENVELOPE_KEYS for key in body)
    else:
        query = body
        envelope_keys = []
        unknown_envelope_key_count = 0

    resource = query.get("read")
    fields = READ_FIELDS.get(resource) if isinstance(resource, str) else None
    operation = "read" if fields is not None else "unrecognized"
    rejected_query_keys = sorted(
        key for key in query if key in READ_QUERY_DIAGNOSTIC_KEYS and key not in READ_QUERY_KEYS
    )
    summary: dict[str, object] = {
        "operation": operation,
        "read_resource": resource if fields is not None else "unrecognized",
        "request_envelope_keys": envelope_keys,
        "unknown_envelope_key_count": unknown_envelope_key_count,
        "recognized_query_keys": sorted(key for key in query if key in READ_QUERY_KEYS),
        "rejected_query_keys": rejected_query_keys,
        "unknown_query_key_count": sum(key not in READ_QUERY_KEYS for key in query),
    }
    select = query.get("select")
    if isinstance(select, list):
        safe_fields = [field for field in select if isinstance(field, str) and fields and field in fields]
        summary["select_fields"] = safe_fields
        relation_summaries: list[dict[str, object]] = []
        unknown_select_field_count = sum(
            1 for field in select if isinstance(field, str) and (not fields or field not in fields)
        )
        unknown_select_field_count += sum(1 for field in select if not isinstance(field, (str, dict)))
        relations = READ_RELATIONS.get(resource, {})
        for item in select:
            if not isinstance(item, dict):
                continue
            relation, nested = next(iter(item.items()), (None, None))
            relation_fields = relations.get(relation) if isinstance(relation, str) else None
            nested_fields = nested.get("select") if isinstance(nested, dict) else None
            if relation_fields is None or not isinstance(nested_fields, list):
                unknown_select_field_count += 1
                continue
            safe_nested = [field for field in nested_fields if isinstance(field, str) and field in relation_fields]
            unknown_select_field_count += len(nested_fields) - len(safe_nested)
            relation_summaries.append({"relation": relation, "fields": safe_nested})
        if relation_summaries:
            summary["select_relations"] = relation_summaries
        summary["unknown_select_field_count"] = unknown_select_field_count

    filters = query.get("filter")
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

    limit = query.get("limit")
    if isinstance(limit, int) and not isinstance(limit, bool) and 0 <= limit <= 100000:
        summary["limit"] = limit
    return summary


def safe_wire_code(value: Any) -> str | None:
    return value if isinstance(value, str) and value in SAFE_WIRE_CODES else None


def safe_browser_failure_type(value: Any) -> str:
    name = type(value).__name__
    return name if name in SAFE_FAILURE_TYPES else "unrecognized"


def safe_browser_route_class(url: Any, fixture_video_id: str) -> str:
    if not isinstance(url, str):
        return "unrecognized"
    path = urlsplit(url).path
    if path == "/main":
        return "main"
    if path in {"/auth/1", "/auth/2"}:
        return "auth"
    if path == f"/watch/{fixture_video_id}":
        return "watch_fixture"
    if path.startswith("/watch/"):
        return "watch_other"
    return "other"


def safe_bookmark_control_evidence(value: Any) -> list[dict[str, str]]:
    if not isinstance(value, list):
        return []
    result: list[dict[str, str]] = []
    for item in value[:10]:
        if not isinstance(item, dict):
            continue
        label = item.get("label")
        pressed = item.get("pressed")
        result.append({
            "label": label if isinstance(label, str) and label in SAFE_BOOKMARK_LABELS else "unrecognized",
            "pressed": pressed if isinstance(pressed, str) and pressed in {"true", "false"} else "unrecognized",
        })
    return result


def failed_read_requests(api_requests: list[dict[str, object]]) -> list[dict[str, object]]:
    """Select already-sanitized non-successful read rows; HTTP status alone is not success."""
    return [row for row in api_requests if row.get("path") == "/read" and row.get("ok") is not True]


def safe_browser_caller_frames(frames: Any) -> list[dict[str, object]]:
    """Keep only bounded source locations from the pinned local React dev server."""
    if not isinstance(frames, list):
        return []
    result: list[dict[str, object]] = []
    for frame in frames:
        if not isinstance(frame, dict) or not isinstance(frame.get("url"), str):
            continue
        parsed = urlsplit(frame["url"])
        path = parsed.path
        line = frame.get("lineNumber")
        column = frame.get("columnNumber")
        if (parsed.hostname not in {"localhost", "127.0.0.1"} or parsed.port != 5173
                or not path.startswith("/src/") or not isinstance(line, int) or line < 0
                or not isinstance(column, int) or column < 0):
            continue
        result.append({"path": path[:240], "line": min(line, 100000), "column": min(column, 100000)})
        if len(result) >= 4:
            break
    return result


def console_error_evidence(message_type: Any, message_text: Any, location: Any) -> dict[str, object]:
    """Classify a console error without storing its raw text or arbitrary source URL."""
    safe_type = message_type if message_type in {"error", "warning", "log", "debug", "info"} else "unrecognized"
    text = message_text if isinstance(message_text, str) else ""
    wire_code = next((code for code in SAFE_WIRE_CODES if code in text), None)
    if wire_code is not None:
        classification = "wire_response_error"
    elif "Failed to load resource" in text or "net::ERR_" in text:
        classification = "network_resource_error"
    elif "ResizeObserver loop" in text:
        classification = "resize_observer_error"
    else:
        classification = "opaque_console_error"

    source = "unmapped"
    if isinstance(location, dict) and isinstance(location.get("url"), str):
        parsed = urlsplit(location["url"])
        path = parsed.path
        line = location.get("lineNumber")
        column = location.get("columnNumber")
        if parsed.hostname in {"localhost", "127.0.0.1"} and parsed.port == 5173 and path.startswith("/src/"):
            source = path[:240]
            if isinstance(line, int) and 0 < line <= 100000:
                source += f":{line}"
                if isinstance(column, int) and 0 <= column <= 100000:
                    source += f":{column}"

    result: dict[str, object] = {
        "console_message_type": safe_type,
        "classification": classification,
        "source": source,
    }
    if wire_code is not None:
        result["wire_code"] = wire_code
    return result


def external_media_failure_evidence(url: Any, resource_type: Any, failure: Any) -> dict[str, str] | None:
    """Classify an intentionally blocked media request without retaining its URL or ID."""
    if not isinstance(url, str) or resource_type not in {"image", "media", "script", "other"}:
        return None
    host = (urlsplit(url).hostname or "").lower()
    host_class = next((name for suffixes, name in (
        (("youtube.com", "youtube-nocookie.com"), "youtube"),
        (("ytimg.com",), "ytimg"),
        (("googlevideo.com",), "googlevideo"),
    ) if any(host == suffix or host.endswith("." + suffix) for suffix in suffixes)), None)
    if host_class is None:
        return None
    failure_code = failure if isinstance(failure, str) and failure in SAFE_NETWORK_FAILURES else "unrecognized"
    return {"host_class": host_class, "resource_type": resource_type, "failure_code": failure_code}


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
