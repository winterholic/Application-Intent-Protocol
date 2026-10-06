import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import subprocess
import sys
import tarfile
import tempfile
import unittest


PROTOTYPE = Path(__file__).resolve().parents[1]
ROOT = PROTOTYPE.parent
BUILDER = PROTOTYPE / "tools" / "build-bundle.py"
SDK_PACKAGE = "aip-prototype-sdk-0.0.0.tgz"
LOCAL_CRATES = (
    "prototype",
    "spikes/spike-v1-fixture",
    "spikes/spike-v2-read",
    "spikes/spike-v3-write",
    "spikes/spike-v4-worker",
    "spikes/spike-v5-sdk",
    "spikes/spike-v6-transport",
    "spikes/spike-v11-dev-checks",
)
EXPLICIT_SOURCE_FILES = (
    "prototype/src/schema_catalog.sql",
    "prototype/tools/build-bundle.py",
    "prototype/tools/build-sdk.mjs",
    "prototype/example/app.aip",
    "prototype/example/extensions/event.mjs",
    "prototype/example/extensions/event.py",
    "spikes/spike-v1-fixture/fixture/recruitment.aip",
    "spikes/spike-v4-worker/extensions/recruitment.mjs",
    "spikes/spike-v4-worker/extensions/recruitment.py",
    "spikes/spike-v4-worker/workers/worker.mjs",
    "spikes/spike-v4-worker/workers/worker.py",
    "prototype/sdk/index.ts",
    "spikes/spike-v5-sdk/sdk/generic.ts",
    "spikes/spike-v5-sdk/sdk/cache.ts",
    "spikes/spike-v6-transport/client/typed.ts",
    "spikes/spike-v6-transport/client/transport.ts",
)


def expected_source_paths():
    paths = set(EXPLICIT_SOURCE_FILES)
    for crate in LOCAL_CRATES:
        crate_path = ROOT / crate
        paths.add(f"{crate}/Cargo.toml")
        paths.add(f"{crate}/Cargo.lock")
        paths.update(path.relative_to(ROOT).as_posix() for path in (crate_path / "src").rglob("*.rs"))
    return paths


def shell_quote(value):
    return "'" + str(value).replace("'", "'\\''") + "'"


class BundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="aip-bundle-test-")
        self.addCleanup(self.temp.cleanup)
        self.temp_dir = Path(self.temp.name)

    def run_builder(self, out, *args, env=None):
        return subprocess.run(
            [sys.executable, str(BUILDER), "--out", str(out), *args],
            cwd=PROTOTYPE,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            timeout=180,
        )

    def fake_cargo_path(self, directory, log, exit_code=37):
        bindir = directory / "fake-bin"
        bindir.mkdir()
        cargo = bindir / "cargo"
        cargo.write_text(f"#!/bin/sh\nprintf '%s\\0' \"$@\" > {shell_quote(log)}\nexit {exit_code}\n")
        cargo.chmod(0o755)
        path = f"{bindir}:{os.environ.get('PATH', '')}"
        return path

    def test_refuses_existing_directory_file_and_symlink_without_touching_them(self):
        marker = self.temp_dir / "cargo-called"
        path = self.fake_cargo_path(self.temp_dir, marker)
        env = dict(os.environ, PATH=path)

        existing_dir = self.temp_dir / "existing-dir"
        existing_dir.mkdir()
        (existing_dir / "keep.txt").write_text("directory marker\n")
        result = self.run_builder(existing_dir, env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((existing_dir / "keep.txt").read_text(), "directory marker\n")

        existing_file = self.temp_dir / "existing-file"
        existing_file.write_text("file marker\n")
        result = self.run_builder(existing_file, env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(existing_file.read_text(), "file marker\n")

        target = self.temp_dir / "symlink-target"
        target.mkdir()
        (target / "keep.txt").write_text("symlink target marker\n")
        existing_link = self.temp_dir / "existing-link"
        existing_link.symlink_to(target, target_is_directory=True)
        result = self.run_builder(existing_link, env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(existing_link.is_symlink())
        self.assertEqual((target / "keep.txt").read_text(), "symlink target marker\n")
        self.assertFalse(marker.exists(), "existing outputs must be rejected before any build subprocess")

    def test_build_failure_removes_only_its_new_output_and_uses_argv(self):
        out = self.temp_dir / "partial bundle"
        parent_marker = self.temp_dir / "parent-marker.txt"
        parent_marker.write_text("keep parent\n")
        cargo_log = self.temp_dir / "cargo-argv.bin"
        env = dict(os.environ, PATH=self.fake_cargo_path(self.temp_dir, cargo_log))

        result = self.run_builder(out, "--profile", "release", env=env)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists(), "only the builder-owned partial output should be removed")
        self.assertEqual(parent_marker.read_text(), "keep parent\n")
        args = cargo_log.read_bytes().split(b"\0")
        self.assertIn(b"build", args)
        self.assertIn(b"--offline", args)
        self.assertIn(b"--manifest-path", args)
        self.assertIn(b"--release", args)
        self.assertIn(str(PROTOTYPE / "Cargo.toml").encode(), args)

    def test_parent_file_returns_json_error_without_running_cargo_or_touching_marker(self):
        parent_file = self.temp_dir / "existing-parent-file"
        parent_file.write_text("preserve this marker\n")
        marker = self.temp_dir / "cargo-called"
        env = dict(os.environ, PATH=self.fake_cargo_path(self.temp_dir, marker))

        result = self.run_builder(parent_file / "child", env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertNotIn("Traceback", result.stderr)
        error = json.loads(result.stderr)
        self.assertFalse(error["ok"])
        self.assertEqual(error["code"], "BUNDLE_FAILED")
        self.assertNotIn(str(parent_file), result.stderr)
        self.assertEqual(parent_file.read_text(), "preserve this marker\n")
        self.assertFalse(marker.exists(), "parent validation must happen before Cargo")

    def test_bundle_is_relocatable_manifested_and_uses_the_bundled_binary_for_gen(self):
        if platform.system() != "Darwin":
            self.skipTest("the local extension bundle targets the current macOS host")

        out = self.temp_dir / "relocated bundle with spaces"
        result = self.run_builder(out)
        self.assertEqual(result.returncode, 0, f"stdout={result.stdout}\nstderr={result.stderr}")

        manifest_path = out / "manifest.json"
        manifest = json.loads(manifest_path.read_text())
        self.assertEqual(manifest["bundleKind"], "private-local-macos-development")
        self.assertEqual(manifest["profile"], "dev")
        self.assertEqual(manifest["host"]["platform"], "macos")
        self.assertEqual(manifest["host"]["architecture"], platform.machine())
        self.assertEqual(set(manifest["sourceSha256"]), expected_source_paths())
        self.assertEqual(
            set(manifest["toolchainVersions"]),
            {"cargo", "rustc", "node", "npm", "typescript"},
        )
        self.assertTrue(all(manifest["toolchainVersions"].values()))
        files = manifest["files"]
        self.assertNotIn("manifest.json", files)
        for relative, expected_hash in files.items():
            path = PurePosixPath(relative)
            self.assertFalse(path.is_absolute())
            self.assertNotIn("..", path.parts)
            target = out.joinpath(*path.parts)
            self.assertTrue(target.is_file(), relative)
            self.assertFalse(target.is_symlink(), relative)
            self.assertEqual(hashlib.sha256(target.read_bytes()).hexdigest(), expected_hash, relative)
        self.assertIn("prototype/src/schema_catalog.sql", manifest["sourceSha256"], "embedded SQL is a binary build input")
        for relative, expected_hash in manifest["sourceSha256"].items():
            path = PurePosixPath(relative)
            self.assertFalse(path.is_absolute())
            self.assertNotIn("..", path.parts)
            source = ROOT.joinpath(*path.parts)
            self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), expected_hash, relative)

        self.assertEqual((out / "examples/app.aip").read_bytes(), (PROTOTYPE / "example/app.aip").read_bytes())
        self.assertEqual(
            (out / "examples/recruitment.aip").read_bytes(),
            (ROOT / "spikes/spike-v1-fixture/fixture/recruitment.aip").read_bytes(),
        )
        self.assertEqual(
            (out / "extensions/node/recruitment.mjs").read_bytes(),
            (ROOT / "spikes/spike-v4-worker/extensions/recruitment.mjs").read_bytes(),
        )
        self.assertEqual(
            (out / "extensions/python/recruitment.py").read_bytes(),
            (ROOT / "spikes/spike-v4-worker/extensions/recruitment.py").read_bytes(),
        )
        self.assertTrue((out / "examples/recruitment-seed.sql").is_file())
        self.assertTrue((out / "examples/event-seed.sql").is_file())
        for lang, suffix in (("node", "mjs"), ("python", "py")):
            relative = f"extensions/{lang}/event.{suffix}"
            self.assertIn(relative, files)
            self.assertEqual((out / relative).read_bytes(), (PROTOTYPE / f"example/extensions/event.{suffix}").read_bytes())
        event_contract = self.temp_dir / "event.contract.ts"
        event_gen = subprocess.run([str(out / "bin/aip-prototype"), "gen", str(out / "examples/app.aip"), "--wire", "decimal", "--out", str(event_contract), "--sdk-import", "@aip/prototype-sdk"], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        self.assertEqual(event_gen.returncode, 0, event_gen.stderr)
        self.assertIn('"Event.confirm"', event_contract.read_text())
        self.assertIn("--enable-write-extensions", (out / "README.md").read_text())

        tarball = out / "sdk/aip-prototype-sdk-0.0.0.tgz"
        self.assertIn("sdk/aip-prototype-sdk-0.0.0.tgz", files)
        with tarfile.open(tarball, "r:gz") as package:
            names = package.getnames()
        self.assertIn("package/package.json", names)
        self.assertTrue(any(name.endswith("/prototype/sdk/index.js") for name in names))

        readme = (out / "README.md").read_text()
        self.assertIn("./bin/aip-prototype", readme)
        self.assertIn("--worker-dir extensions/node --worker-lang node", readme)
        self.assertIn("`extensions/python --worker-lang python`", readme)
        self.assertIn("-v ON_ERROR_STOP=1 --single-transaction", readme)
        self.assertIn('consumer_dir="$(mktemp -d /tmp/aip-bundle-consumer.XXXXXX)"', readme)
        self.assertIn("bundle_root=/absolute/path/to/bundle", readme)
        self.assertNotIn(str(ROOT), readme)
        self.assertNotIn(str(ROOT), manifest_path.read_text())
        self.assertIn("registry cache", readme)
        self.assertIn("bit-for-bit", readme)

        binary = out / "bin/aip-prototype"
        check = subprocess.run([str(binary), "check", str(out / "examples/app.aip")], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        self.assertEqual(check.returncode, 0, check.stderr)
        self.assertTrue(json.loads(check.stdout)["ok"])

        generated = self.temp_dir / "recruitment.contract.ts"
        gen = subprocess.run(
            [str(binary), "gen", str(out / "examples/recruitment.aip"), "--wire", "decimal", "--out", str(generated), "--sdk-import", "@aip/prototype-sdk"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=10,
        )
        self.assertEqual(gen.returncode, 0, gen.stderr)
        output = generated.read_text()
        self.assertIn('from "@aip/prototype-sdk"', output)
        self.assertIn('Recruitment.stats', output)
        self.assertTrue(json.loads(gen.stdout)["fingerprint"])

    def test_sdk_pack_filename_must_be_expected_basename(self):
        spec = __import__("importlib.util", fromlist=["spec_from_file_location"]).spec_from_file_location(
            "build_bundle", BUILDER
        )
        module = __import__("importlib.util", fromlist=["module_from_spec"]).module_from_spec(spec)
        spec.loader.exec_module(module)
        self.assertEqual(module.validate_pack_filename(SDK_PACKAGE), SDK_PACKAGE)
        for unsafe in ("../" + SDK_PACKAGE, "/tmp/" + SDK_PACKAGE, "other.tgz", ""):
            with self.subTest(filename=unsafe):
                with self.assertRaises(module.BundleError):
                    module.validate_pack_filename(unsafe)


if __name__ == "__main__":
    unittest.main()
