import io
import sys
import unittest
from unittest import mock

from product.providers.google_storage import create_google_storage_adapter, create_google_storage_provider


class FakeStorage:
    def __init__(self):
        self.objects = {}
        self.names = []

    def bucket(self, name):
        self.assert_bucket(name)
        return FakeBucket(self)

    @staticmethod
    def assert_bucket(name):
        if name != "bucket-a":
            raise AssertionError(name)


class FakeBucket:
    def __init__(self, storage):
        self.storage = storage

    def blob(self, name, generation=None):
        self.storage.names.append((name, generation))
        return FakeBlob(self.storage, name, generation)


class FakeBlob:
    def __init__(self, storage, name, generation):
        self.storage = storage
        self.name = name
        self.generation = generation
        self.chunk_size = None

    def upload_from_file(self, source, *, size, rewind, content_type, if_generation_match, checksum, timeout):
        assert rewind is False
        assert if_generation_match == 0
        assert checksum == "crc32c"
        payload = source.read()
        self.storage.objects[self.name] = {
            "data": payload,
            "content_type": content_type,
            "generation": "7",
            "etag": "etag-7",
        }
        self.reload()

    def reload(self, **_kwargs):
        obj = self.storage.objects[self.name]
        self.size = str(len(obj["data"]))
        self.content_type = obj["content_type"]
        self.generation = obj["generation"]
        self.etag = obj["etag"]

    def download_to_file(self, output, *, if_generation_match, checksum, timeout, single_shot_download):
        assert if_generation_match == 7
        assert checksum == "crc32c"
        assert single_shot_download is False
        output.write(self.storage.objects[self.name]["data"])

    def delete(self, *, if_generation_match, timeout):
        assert if_generation_match == 7
        del self.storage.objects[self.name]


class GoogleStorageProviderTest(unittest.TestCase):
    def test_streamed_object_uses_adc_client_api_generation_guards_and_owner_scope(self):
        storage = FakeStorage()
        provider = create_google_storage_adapter(storage=storage, bucket="bucket-a", max_bytes=1024)
        owner = provider.for_owner("tenant-a")
        payload = b"streamed bytes"
        result = owner.upload(
            "7de0de57-f817-43f6-9de0-715e92310acd",
            io.BytesIO(payload),
            size=len(payload),
            content_type="application/octet-stream",
        )
        self.assertEqual(result["size"], len(payload))
        self.assertEqual(storage.objects[storage.names[0][0]]["data"], payload)
        self.assertRegex(storage.names[0][0], r"^[a-f0-9]{64}/")

        output = io.BytesIO()
        metadata = owner.download("7de0de57-f817-43f6-9de0-715e92310acd", output)
        self.assertEqual(output.getvalue(), payload)
        self.assertEqual(metadata["generation"], "7")
        self.assertEqual(owner.head("7de0de57-f817-43f6-9de0-715e92310acd")["etag"], "etag-7")
        self.assertTrue(owner.delete("7de0de57-f817-43f6-9de0-715e92310acd"))

    def test_rejects_invalid_ids_size_and_content_type(self):
        provider = create_google_storage_adapter(storage=FakeStorage(), bucket="bucket-a", max_bytes=16)
        with self.assertRaisesRegex(ValueError, "object id"):
            provider.upload("../escape", io.BytesIO(b"x"), size=1, content_type="text/plain")
        with self.assertRaisesRegex(ValueError, "limit"):
            provider.upload("7de0de57-f817-43f6-9de0-715e92310acd", io.BytesIO(b"x"), size=17, content_type="text/plain")
        with self.assertRaisesRegex(ValueError, "content type"):
            provider.upload("7de0de57-f817-43f6-9de0-715e92310acd", io.BytesIO(b"x"), size=1, content_type="text/plain\r\n")


    def test_missing_official_sdk_reports_the_optional_package(self):
        with mock.patch.dict(sys.modules, {"google": None, "google.cloud": None}):
            with self.assertRaisesRegex(RuntimeError, "PROVIDER_PACKAGE_MISSING.*google-cloud-storage"):
                create_google_storage_provider(bucket="bucket-a")


if __name__ == "__main__":
    unittest.main()
