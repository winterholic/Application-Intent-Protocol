#!/usr/bin/env python3
"""Validate a FlaskBB source-ID/UUID import against an exported target snapshot.

Read {"manifest": ..., "snapshot": ...} from stdin; emit a deterministic offline
plan or exit 1 without a plan. No DB connections, SQL, files, sequence changes,
writer-fence acquisition or activation are performed. The snapshot and source
metadata must come from a verified export: matching claims do not prove their
provenance or freshness. Revalidate under the product writer fence before apply.

All published posts, including hidden/deleted tombstones, need explicit source
IDs. Drafts are excluded. UUID/date order never supplies a missing source ID.
This is the preflight for initial import (or exact replay before native writes),
not a tool to append a second source namespace to an already active allocator.
"""
import hashlib
import json
import re
import sys
import uuid

MAX_INT64 = (1 << 63) - 1
MAX_ROWS = 100_000
MAX_INPUT_BYTES = 16 * 1024 * 1024


def _object(value, keys, label):
    if not isinstance(value, dict) or set(value) != set(keys.split()):
        raise ValueError(f"{label}: expected fields {keys}")
    return value


def _int(value, label, minimum=1):
    if type(value) is not int or not minimum <= value <= MAX_INT64:
        raise ValueError(f"{label}: expected integer {minimum}..{MAX_INT64}")
    return value


def _bool(value, label):
    if type(value) is not bool:
        raise ValueError(f"{label}: expected boolean")
    return value


def _uuid(value):
    if not isinstance(value, str):
        raise ValueError("post_uuid: expected canonical UUID")
    try:
        parsed = uuid.UUID(value)
    except ValueError as error:
        raise ValueError("post_uuid: expected canonical UUID") from error
    if str(parsed) != value:
        raise ValueError("post_uuid: expected canonical lowercase UUID")
    return value


def _rows(value, label):
    if not isinstance(value, list) or len(value) > MAX_ROWS:
        raise ValueError(f"{label}: expected array of at most {MAX_ROWS} rows")
    return value


def _source(value):
    _object(value, "namespace revision high_water", "source")
    namespace = value["namespace"]
    if not isinstance(namespace, str) or not 1 <= len(namespace) <= 200 or any(ord(c) < 32 for c in namespace):
        raise ValueError("source namespace: expected nonempty name without control characters")
    if not isinstance(value["revision"], str) or not re.fullmatch(r"(?:[a-f0-9]{40}|[a-f0-9]{64})", value["revision"]):
        raise ValueError("source revision: expected lowercase SHA-1 or SHA-256")
    _int(value["high_water"], "source high_water", minimum=0)
    return dict(value)


def _pairs(value, label):
    by_id, by_uuid = {}, {}
    for row in _rows(value, label):
        _object(row, "source_id post_uuid", label)
        key, target = _int(row["source_id"], label + " source_id"), _uuid(row["post_uuid"])
        if key in by_id or target in by_uuid:
            raise ValueError(f"{label}: duplicate source_id or post_uuid")
        by_id[key], by_uuid[target] = target, key
    return by_id, by_uuid


def _pair_rows(by_id):
    return [{"source_id": key, "post_uuid": value} for key, value in sorted(by_id.items())]


def _digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()).hexdigest()


def preflight(manifest, snapshot):
    """Return an offline plan. Neither input is mutated; every mismatch is fatal."""
    _object(manifest, "version source posts", "manifest")
    _object(snapshot, "version source posts mappings generator", "snapshot")
    for obj in (manifest, snapshot):
        if type(obj["version"]) is not int or obj["version"] != 1:
            raise ValueError("version: expected 1")
    source = _source(manifest["source"])
    if source != _source(snapshot["source"]):
        raise ValueError("source namespace/revision/high_water mismatch")
    desired, desired_by_uuid = _pairs(manifest["posts"], "manifest")
    if max(desired, default=0) > source["high_water"]:
        raise ValueError("source high_water is below an imported source_id")
    existing, existing_by_uuid = _pairs(snapshot["mappings"], "mapping")
    posts = {}
    for row in _rows(snapshot["posts"], "snapshot posts"):
        _object(row, "post_uuid published source_order", "snapshot post")
        target = _uuid(row["post_uuid"])
        _bool(row["published"], "published")
        if row["source_order"] is not None:
            _int(row["source_order"], "source_order")
        if target in posts:
            raise ValueError("snapshot posts: duplicate post_uuid")
        posts[target] = dict(row)
    if set(desired_by_uuid) != {key for key, row in posts.items() if row["published"]}:
        raise ValueError("coverage: manifest must cover exactly all published posts; drafts remain unmapped")
    for key, target in existing.items():
        if desired.get(key) != target or desired_by_uuid.get(target) != key:
            raise ValueError("mapping conflict: existing pair is different, unclaimed or dangling")
    for target, row in posts.items():
        current = row["source_order"]
        if current is not None and (desired_by_uuid.get(target) != current or existing_by_uuid.get(target) != current):
            raise ValueError("link conflict: source_order does not match the existing reverse mapping")
    generator = _object(snapshot["generator"], "last_value is_called", "generator")
    _int(generator["last_value"], "generator last_value")
    _bool(generator["is_called"], "generator is_called")
    # Conservatively reserve last_value even if is_called=false. Gaps are allowed;
    # a preflight must never suggest moving a sequence backwards after rollback.
    floor = max(source["high_water"], generator["last_value"], max(existing, default=0))
    if floor == MAX_INT64:
        raise ValueError("generator exhausted: no greater signed int64 ID remains")
    normalized_manifest = {"version": 1, "source": source, "posts": _pair_rows(desired)}
    normalized_snapshot = {
        "version": 1, "source": source, "posts": [posts[key] for key in sorted(posts)],
        "mappings": _pair_rows(existing), "generator": dict(generator),
    }
    plan = {
        "version": 1, "source": source,
        "manifest_digest": _digest(normalized_manifest), "snapshot_digest": _digest(normalized_snapshot),
        "insert_mappings": _pair_rows({key: value for key, value in desired.items() if key not in existing}),
        "link_posts": _pair_rows({key: value for key, value in desired.items() if posts[value]["source_order"] is None}),
        "generator_floor": floor, "next_id_min": floor + 1,
        "activation_authorized": False,
    }
    return {**plan, "digest": _digest(plan)}


def _unique_object(pairs):
    out = {}
    for key, value in pairs:
        if key in out:
            raise ValueError("duplicate JSON object key")
        out[key] = value
    return out


def loads(text):
    return json.loads(text, object_pairs_hook=_unique_object)


def main():
    try:
        payload = sys.stdin.buffer.read(MAX_INPUT_BYTES + 1)
        if len(payload) > MAX_INPUT_BYTES:
            raise ValueError(f"input exceeds {MAX_INPUT_BYTES} bytes")
        request = _object(loads(payload), "manifest snapshot", "request")
        plan = preflight(request["manifest"], request["snapshot"])
    except (ValueError, RecursionError) as error:
        print(f"PostOrder preflight rejected: {error}", file=sys.stderr)
        return 1
    print(json.dumps(plan, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
