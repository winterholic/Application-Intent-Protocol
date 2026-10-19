"""Forum/topic unread computation for the FlaskBB port renderer.

Mirrors upstream ``flaskbb/utils/helpers.py`` ``topic_is_unread`` /
``forum_is_unread`` (fixed upstream ``0885238``), including the
``read_cutoff = now - TRACKER_LENGTH days`` rule and the disabled-tracker
(``TRACKER_LENGTH == 0``) case. ``tracker_length`` defaults to the fixture
default (upstream ``flaskbb/fixtures/settings.py`` ``tracker_length`` value 7):
the renderer serves viewers over their own session, which cannot read the
``Setting`` resource (admin/super_mod only), so a customized setting value is
not honored here. See PRODUCT-GAPS.md.
"""

from datetime import datetime, timedelta, timezone

DEFAULT_TRACKER_LENGTH_DAYS = 7


def parse_time(value):
    """Parse an SDK timestamp (ISO string or datetime) to aware UTC datetime."""
    if value is None or isinstance(value, datetime):
        parsed = value
    else:
        text = str(value).strip()
        if text.endswith("Z"):
            text = text[:-1] + "+00:00"
        parsed = datetime.fromisoformat(text)
    if parsed is None:
        return None
    if parsed.tzinfo is None:
        return parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def _now(now):
    return now if now is not None else datetime.now(timezone.utc)


def topic_is_unread(*, last_updated, topicsread_last_read=None,
                    forumsread_cleared=None, authenticated=True,
                    tracker_length=DEFAULT_TRACKER_LENGTH_DAYS, now=None):
    """Mirror of upstream ``topic_is_unread``.

    ``topicsread_last_read`` is None when the viewer never opened the topic (or
    marked the forum as read, which deletes topic markers); ``forumsread_cleared``
    is the forum-level cleared stamp, if any.
    """
    if not authenticated:
        return False
    if tracker_length == 0:
        return False
    updated = parse_time(last_updated)
    if updated is None:
        # Port guard: the upstream column is non-null and the renderer only
        # lists published topics, whose check constraint forces dates.
        return False
    if updated < _now(now) - timedelta(days=tracker_length):
        return False
    if topicsread_last_read is None:
        if forumsread_cleared is not None:
            return parse_time(forumsread_cleared) < updated
        return True
    return parse_time(topicsread_last_read) < updated


def forum_is_unread(*, topic_count, last_post_id=None, last_post_created=None,
                    forumsread=None, authenticated=True,
                    tracker_length=DEFAULT_TRACKER_LENGTH_DAYS, now=None):
    """Mirror of upstream ``forum_is_unread``.

    ``forumsread`` is None when the viewer has no marker row, else a mapping
    with ``last_read`` (non-null upstream) and ``cleared`` (nullable).
    """
    if not authenticated:
        return False
    if tracker_length == 0:
        return False
    if topic_count == 0:
        return False
    if last_post_id is None:
        return False
    created = parse_time(last_post_created)
    if created is None:
        return False
    cutoff = _now(now) - timedelta(days=tracker_length)
    if created < cutoff:
        return False
    if forumsread is None:
        return created > cutoff
    cleared = parse_time(forumsread.get("cleared"))
    last_read = parse_time(forumsread.get("last_read"))
    # Upstream wraps the cleared comparison in try/except TypeError so a NULL
    # cleared falls through to the last_read comparison below; a NULL
    # last_read (non-null in both schemas) counts as never read.
    if cleared is not None and created > cleared:
        if last_read is not None and created < last_read:
            return False
    if last_read is None:
        return True
    return created > last_read
