"""Server-side adapter for the official google-cloud-storage Python SDK.

The default client uses Application Default Credentials. Do not import or call
this adapter inside AIP's network-denied extension workers; invoke it from a
trusted server-side provider bridge and pass streams through that bridge.
"""

import hashlib
import re

DEFAULT_MAX_BYTES = 5 * 1024 * 1024 * 1024
MAX_GCS_OBJECT_BYTES = 5 * 1024**4
CHUNK_BYTES = 8 * 1024 * 1024
DEFAULT_TIMEOUT_SECONDS = 60
_UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$", re.IGNORECASE)
_BUCKET = re.compile(r"^[a-z0-9][a-z0-9._-]{1,61}[a-z0-9]$")
_CONTENT_TYPE = re.compile(r"^[A-Za-z0-9.+-]+/[A-Za-z0-9.+-]+$")


def create_google_storage_adapter(*, storage, bucket, max_bytes=DEFAULT_MAX_BYTES, timeout_seconds=DEFAULT_TIMEOUT_SECONDS):
    """Create an adapter using an injected official SDK Client-compatible object."""
    if storage is None or not callable(getattr(storage, "bucket", None)):
        raise TypeError("storage client is required")
    if not isinstance(bucket, str) or not _BUCKET.fullmatch(bucket):
        raise ValueError("invalid bucket")
    if not isinstance(max_bytes, int) or isinstance(max_bytes, bool) or not 0 < max_bytes <= MAX_GCS_OBJECT_BYTES:
        raise ValueError("invalid object size limit")
    if not isinstance(timeout_seconds, (int, float)) or not 0 < timeout_seconds <= 600:
        raise ValueError("invalid timeout")
    return _ScopedAdapter(storage, bucket, max_bytes, timeout_seconds, "")


def create_google_storage_provider(*, bucket, max_bytes=DEFAULT_MAX_BYTES, timeout_seconds=DEFAULT_TIMEOUT_SECONDS):
    """Load the optional official SDK and use ADC (attached identity/WIF/local ADC)."""
    try:
        from google.cloud import storage
    except ImportError as error:
        raise RuntimeError(
            "PROVIDER_PACKAGE_MISSING: install google-cloud-storage in the server environment (python -m pip install google-cloud-storage)"
        ) from error

    client = storage.Client()
    return create_google_storage_adapter(storage=client, bucket=bucket, max_bytes=max_bytes, timeout_seconds=timeout_seconds)


class _ScopedAdapter:
    def __init__(self, storage, bucket, max_bytes, timeout_seconds, prefix):
        self._storage = storage
        self._bucket = bucket
        self._max_bytes = max_bytes
        self._timeout = timeout_seconds
        self._prefix = prefix

    def for_owner(self, owner):
        if not isinstance(owner, str) or not owner or len(owner) > 2048 or any(ord(char) < 32 or ord(char) == 127 for char in owner):
            raise ValueError("invalid owner")
        namespace = hashlib.sha256(owner.encode("utf-8")).hexdigest()
        return _ScopedAdapter(self._storage, self._bucket, self._max_bytes, self._timeout, f"{self._prefix}{namespace}/")

    def upload(self, object_id, source, *, size, content_type):
        name = self._name(object_id)
        if not isinstance(size, int) or isinstance(size, bool) or size < 0 or size > self._max_bytes:
            raise ValueError("object size exceeds configured limit")
        if not isinstance(content_type, str) or len(content_type) > 127 or not _CONTENT_TYPE.fullmatch(content_type):
            raise ValueError("invalid content type")
        if not callable(getattr(source, "read", None)):
            raise TypeError("binary readable stream is required")

        blob = self._storage.bucket(self._bucket).blob(name)
        blob.chunk_size = CHUNK_BYTES
        counted = _CountedReader(source, size)
        blob.upload_from_file(
            counted,
            size=size,
            rewind=False,
            content_type=content_type,
            if_generation_match=0,
            checksum="crc32c",
            timeout=self._timeout,
        )
        if counted.bytes_read != size:
            raise ValueError("stream size differs from declared size")
        blob.reload(timeout=self._timeout)
        return _metadata(blob)

    def download(self, object_id, destination):
        blob = self._storage.bucket(self._bucket).blob(self._name(object_id))
        blob.reload(timeout=self._timeout)
        generation = _generation(blob.generation)
        blob.chunk_size = CHUNK_BYTES
        blob.download_to_file(
            destination,
            if_generation_match=generation,
            checksum="crc32c",
            timeout=self._timeout,
            single_shot_download=False,
        )
        return _metadata(blob)

    def head(self, object_id):
        blob = self._storage.bucket(self._bucket).blob(self._name(object_id))
        blob.reload(timeout=self._timeout)
        return _metadata(blob)

    def delete(self, object_id):
        blob = self._storage.bucket(self._bucket).blob(self._name(object_id))
        try:
            blob.reload(timeout=self._timeout)
        except Exception as error:
            if getattr(error, "code", None) == 404:
                return False
            raise
        blob.delete(if_generation_match=_generation(blob.generation), timeout=self._timeout)
        return True

    def _name(self, object_id):
        if not isinstance(object_id, str) or not _UUID.fullmatch(object_id):
            raise ValueError("invalid object id")
        return f"{self._prefix}{object_id.lower()}"


class _CountedReader:
    def __init__(self, source, expected):
        self._source = source
        self._expected = expected
        self.bytes_read = 0

    def read(self, size=-1):
        remaining = self._expected - self.bytes_read
        request_size = size if size is not None and size >= 0 else remaining + 1
        chunk = self._source.read(min(request_size, remaining + 1))
        if not isinstance(chunk, (bytes, bytearray, memoryview)):
            raise TypeError("object stream must contain bytes")
        self.bytes_read += len(chunk)
        if self.bytes_read > self._expected:
            raise ValueError("stream size exceeds declared size")
        return chunk


def _generation(value):
    try:
        generation = int(value)
    except (TypeError, ValueError) as error:
        raise ValueError("object generation is unavailable") from error
    if generation < 0:
        raise ValueError("object generation is invalid")
    return generation


def _metadata(blob):
    size = int(blob.size)
    if size < 0:
        raise ValueError("object metadata has invalid size")
    return {
        "size": size,
        "contentType": blob.content_type or "application/octet-stream",
        "generation": str(_generation(blob.generation)),
        "etag": blob.etag,
    }
