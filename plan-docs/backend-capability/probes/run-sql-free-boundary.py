"""Run the capability audit probe against existing local crates; no DDL is applied."""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

probes = Path(__file__).resolve().parent
repo = probes.parents[2]
crates = ["spike-v1-fixture", "spike-v2-read", "spike-v5-sdk", "spike-v6-transport"]
for crate in crates:
    if not (repo / "spikes" / crate / "Cargo.toml").is_file():
        raise SystemExit(f"missing local crate: {crate}")

with tempfile.TemporaryDirectory(prefix="aip-capability-probe-") as scratch:
    work = Path(scratch)
    (work / "src").mkdir()
    (work / "extensions").mkdir()
    shutil.copyfile(probes / "sql-free-boundary.rs", work / "src" / "main.rs")
    for name in ["compute.mjs", "compute.py"]:
        shutil.copyfile(probes / name, work / "extensions" / name)
    manifest = '''[package]
name = "aip-sql-free-probe"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
serde_json = "1"
tokio = { version = "1", features = ["full"] }
'''
    for crate in crates:
        location = json.dumps(str(repo / "spikes" / crate))
        manifest += f"{crate} = {{ path = {location} }}\n"
    (work / "Cargo.toml").write_text(manifest)
    command = [
        "cargo", "run", "--offline", "--quiet", "--manifest-path", str(work / "Cargo.toml"),
        "--target-dir", str(repo / "spikes" / "spike-v6-transport" / "target"),
    ]
    subprocess.run(command, cwd=repo, check=True, timeout=180)
