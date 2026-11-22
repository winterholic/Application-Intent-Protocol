"""provenance.py 단위 시험. SHA·엔진·SDK가 어긋나는 반례를 넣어 판정이 실패하는지 고정한다.

실행: python3 -m unittest verification/1.0-consumers/test_provenance.py
"""
import io
import sys
import tarfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import provenance as p

HEAD = "a" * 40
OTHER = "b" * 40
BIN = "c" * 64


def build(**overrides):
    base = {"source_revision": HEAD, "source_dirty": False, "binary_sha256": BIN, "engine_version": "aip 1.0.0"}
    return {**base, **overrides}


def tgz(files):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, body in files.items():
            info = tarfile.TarInfo("package/" + name)
            info.size = len(body)
            archive.addfile(info, io.BytesIO(body))
    return buffer.getvalue()


def whl(version, files):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("x-" + version + ".dist-info/METADATA", f"Name: x\nVersion: {version}\n")
        for name, body in files.items():
            archive.writestr(name, body)
    return buffer.getvalue()


class Target(unittest.TestCase):
    def test_input_wins_and_head_is_default(self):
        self.assertEqual(p.resolve_target(HEAD, {}, OTHER), (HEAD, "--core-sha"))
        self.assertEqual(p.resolve_target(None, {"AIP_CONSUMER_CORE_SHA": HEAD}, OTHER)[0], HEAD)
        self.assertEqual(p.resolve_target(None, {}, HEAD)[0], HEAD)

    def test_rejects_short_or_upper_case_sha(self):
        for bad in ("4e132564", "A" * 40, "main", "a" * 41):
            with self.assertRaises(p.ProvenanceError):
                p.resolve_target(bad, {}, HEAD)

    def test_head_that_differs_from_target_fails(self):
        with self.assertRaises(p.ProvenanceError) as caught:
            p.require_all(p.source_checks(OTHER, HEAD, ""))
        self.assertIn("source-head-matches-target", str(caught.exception))

    def test_dirty_tree_fails(self):
        with self.assertRaises(p.ProvenanceError):
            p.require_all(p.source_checks(HEAD, HEAD, " M crates/aip-cli/src/main.rs"))

    def test_matching_clean_source_passes(self):
        p.require_all(p.source_checks(HEAD, HEAD, ""))


class Engine(unittest.TestCase):
    def run_checks(self, builds, version="aip 1.0.0", binary=BIN):
        return p.engine_checks(HEAD, "1.0.0", version, binary, builds)

    def test_all_matching_passes(self):
        p.require_all(self.run_checks({"taskshare": build(), "stockdesk": build()}))

    def test_package_built_from_another_commit_fails(self):
        with self.assertRaises(p.ProvenanceError) as caught:
            p.require_all(self.run_checks({"taskshare": build(source_revision=OTHER)}))
        self.assertIn("engine-build-source:taskshare", str(caught.exception))

    def test_package_with_another_binary_fails(self):
        with self.assertRaises(p.ProvenanceError) as caught:
            p.require_all(self.run_checks({"stockdesk": build(binary_sha256="d" * 64)}))
        self.assertIn("engine-build-binary:stockdesk", str(caught.exception))

    def test_dirty_package_fails(self):
        with self.assertRaises(p.ProvenanceError):
            p.require_all(self.run_checks({"taskshare": build(source_dirty=True)}))

    def test_wrong_engine_version_fails(self):
        with self.assertRaises(p.ProvenanceError):
            p.require_all(self.run_checks({"taskshare": build(engine_version="aip 0.9.0")}, version="aip 0.9.0"))


class Sdk(unittest.TestCase):
    NPM = {"index.js": p.sha256_bytes(b"one")}
    PY = {"aip_sdk/client.py": p.sha256_bytes(b"client")}

    def checks(self, **overrides):
        args = dict(npm_name="aip-sdk-1.0.0.tgz", expected_npm_name="aip-sdk-1.0.0.tgz", npm_version="1.0.0", workspace_version="1.0.0",
                    tarball_hashes=self.NPM, fresh_hashes=self.NPM, wheel_name="a-1.0.0-py3-none-any.whl", expected_wheel_name="a-1.0.0-py3-none-any.whl",
                    wheel_version="1.0.0", python_version="1.0.0", wheel_hashes=self.PY, source_hashes=self.PY)
        args.update(overrides)
        return p.sdk_checks(**args)

    def test_matching_artifacts_pass(self):
        p.require_all(self.checks())

    def test_stale_tarball_name_fails(self):
        with self.assertRaises(p.ProvenanceError) as caught:
            p.require_all(self.checks(npm_name="aip-sdk-0.9.0.tgz"))
        self.assertIn("sdk-npm-file-name", str(caught.exception))

    def test_tarball_content_that_differs_from_fresh_build_fails(self):
        with self.assertRaises(p.ProvenanceError) as caught:
            p.require_all(self.checks(tarball_hashes={"index.js": p.sha256_bytes(b"old")}))
        self.assertIn("sdk-npm-content-equals-fresh-build", str(caught.exception))

    def test_wheel_content_that_differs_from_source_fails(self):
        with self.assertRaises(p.ProvenanceError):
            p.require_all(self.checks(wheel_hashes={"aip_sdk/client.py": p.sha256_bytes(b"stale")}))

    def test_wheel_version_mismatch_fails(self):
        with self.assertRaises(p.ProvenanceError):
            p.require_all(self.checks(wheel_version="0.9.0"))

    def test_empty_content_does_not_pass_as_equal(self):
        with self.assertRaises(p.ProvenanceError):
            p.require_all(self.checks(tarball_hashes={}, fresh_hashes={}))

    def test_archive_readers(self):
        self.assertEqual(p.tar_file_hashes(tgz({"index.js": b"one"})), self.NPM)
        version, files = p.wheel_info(whl("1.0.0", {"aip_sdk/client.py": b"client", "other/x.py": b"x"}))
        self.assertEqual((version, files), ("1.0.0", self.PY))


if __name__ == "__main__":
    unittest.main()
