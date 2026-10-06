"""Exercise generated extension bindings against the real local HTTP server and workers."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import uuid

from smoke import ROOT, listening, psql, run, stop


SCRIPT = r"""
import {connect} from SDK_IMPORT;
import {contract} from './contract.ts';
declare const process: {env: Record<string,string|undefined>};
function expect(condition: boolean, message: string) { if (!condition) throw new Error(message); }
function equal(a: unknown, b: unknown) { return JSON.stringify(a) === JSON.stringify(b); }
const url = process.env.AIP_SMOKE_URL!;
const token = process.env.AIP_SMOKE_TOKEN!;
const wire = process.env.AIP_SMOKE_WIRE!;
const lang = process.env.AIP_SMOKE_LANG!;
const clubId = CLUB_ID;
const aip = connect(url, token, contract);

const cacheBefore = aip.cache.size();
const pendingBefore = JSON.stringify(aip.pending());
const first = await aip.extension('Recruitment.stats', {clubId});
expect(first.approvedApplicants === 1, 'extension result');
expect(aip.cache.size() === cacheBefore, 'extension leaves cache unchanged');
expect(JSON.stringify(aip.pending()) === pendingBefore, 'extension leaves pending writes unchanged');
const second = await aip.extension('Recruitment.stats', {clubId});
expect(second.approvedApplicants === 1, 'repeated extension result');
expect(aip.cache.size() === cacheBefore, 'repeated extension leaves cache unchanged');
expect(JSON.stringify(aip.pending()) === pendingBefore, 'repeated extension leaves pending writes unchanged');

const read = await aip.read({read: 'Recruitment', select: ['id'], sort: [{field: 'id', dir: 'asc'}]});
expect(equal(read.rows.map(row => row.id), [ID100, ID101]), 'standard read remains available');

const wrongId = wire === 'safe-number-v13' ? '10' : 10;
const badWire = await aip.post('/extension', {extension: 'Recruitment.stats', input: {clubId: wrongId}});
expect(!badWire.ok && badWire.code === 'BAD_VALUE', 'raw wrong-wire input is rejected');
const stale = connect(url, token, {...contract, fingerprint: '0'.repeat(64)});
try {
  await stale.extension('Recruitment.stats', {clubId});
  throw new Error('stale extension contract was accepted');
} catch (error) {
  expect((error as {code?: string}).code === 'CONTRACT_MISMATCH', 'stale extension contract is rejected');
}
expect(equal(stale.pending(), []), 'contract mismatch leaves no pending write');

if (false) {
  // @ts-expect-error only generated extension names are callable
  void aip.extension('Recruitment.unknown', {clubId});
  // @ts-expect-error extension input follows the selected Id wire
  void aip.extension('Recruitment.stats', {clubId: WRONG_CLUB_ID});
  // @ts-expect-error generated extension outputs are readonly
  first.approvedApplicants = 99;
}

console.log(JSON.stringify({ok: true, wire, lang, approvedApplicants: first.approvedApplicants}));
"""


def modes(selected, values):
    return values if selected == "both" else (selected,)


def install_package(directory):
    package = directory / "sdk-package"
    run(["node", str(ROOT / "tools/build-sdk.mjs"), "--out", str(package)])
    packed = json.loads(run([
        "npm", "pack", "--offline", "--ignore-scripts", "--json",
        "--pack-destination", str(directory), str(package),
    ]))
    run(["npm", "install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund",
         "--prefix", str(directory), str(directory / packed[0]["filename"])])


def seed(db_url, schema):
    psql(db_url, f"""INSERT INTO {schema}.school (id, name) VALUES (1, 'A대');
        INSERT INTO {schema}.member (id, school_id) VALUES (1, 1), (2, 1), (3, 1);
        INSERT INTO {schema}.club (id, name, logo, school_id) VALUES
          (10, 'A동아리', NULL, 1), (11, 'B동아리', NULL, 1);
        INSERT INTO {schema}.club_member (club_id, member_id, role) VALUES
          (10, 1, 'MANAGER'), (10, 2, 'MEMBER'), (11, 3, 'ADMIN');
        INSERT INTO {schema}.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
          (100, 'A 모집', '2099-10-10T00:00:00Z', 'PUBLISHED', 0, 10, NULL),
          (101, 'B 모집', '2099-10-12T00:00:00Z', 'PUBLISHED', 0, 11, NULL);
        INSERT INTO {schema}.apply (id, recruitment_id, status) VALUES
          (200, 100, 'APPROVE'), (201, 100, 'PENDING'),
          (202, 101, 'APPROVE'), (203, 101, 'APPROVE')""")


def exercise(db_url, binary, tsc, shared, sdk_mode, wire, lang):
    schema = "aip_extension_" + uuid.uuid4().hex
    owned = False
    servers = []
    try:
        with tempfile.TemporaryDirectory(prefix="case-", dir=shared) as temp:
            directory = Path(temp)
            source = directory / "recruitment.aip"
            canonical = ROOT.parent / "spikes/spike-v1-fixture/fixture/recruitment.aip"
            source.write_text(canonical.read_text())
            init = json.loads(run([binary, "init", str(source), "--schema", schema, "--db-url", db_url]))
            if not init.get("ok") or init.get("schema") != schema:
                raise RuntimeError("schema initialization did not report ownership")
            owned = True
            seed(db_url, schema)

            sdk_import = str(ROOT / "sdk/index.ts") if sdk_mode == "source" else "@aip/prototype-sdk"
            contract_path = directory / "contract.ts"
            generated = json.loads(run([
                binary, "gen", str(source), "--wire", wire, "--out", str(contract_path), "--sdk-import", sdk_import,
            ]))
            if not generated.get("ok") or not generated.get("fingerprint"):
                raise RuntimeError("extension binding generation failed")

            script = directory / "caller.mts"
            club_id = "10" if wire == "decimal" else "10"
            wrong_club_id = "10" if wire == "safe" else "10"
            row100 = "'100'" if wire == "decimal" else "100"
            row101 = "'101'" if wire == "decimal" else "101"
            caller = SCRIPT.replace("SDK_IMPORT", json.dumps(sdk_import))
            caller = caller.replace("WRONG_CLUB_ID", wrong_club_id if wire == "decimal" else f"'{wrong_club_id}'")
            caller = caller.replace("CLUB_ID", club_id if wire == "safe" else f"'{club_id}'")
            caller = caller.replace("ID100", row100).replace("ID101", row101)
            script.write_text(caller)

            if sdk_mode == "source":
                compile_args = [tsc, "--noEmit", "--strict", "--target", "esnext", "--module", "nodenext",
                                "--allowImportingTsExtensions", str(script)]
                executable = ["node", "--experimental-strip-types", str(script)]
            else:
                output = directory / "js"
                compile_args = [tsc, "--strict", "--target", "es2022", "--module", "nodenext", "--moduleResolution", "nodenext",
                                "--rewriteRelativeImportExtensions", "--rootDir", str(directory), "--outDir", str(output),
                                "--noEmitOnError", str(script)]
                executable = ["node", str(output / "caller.mjs")]
            run(compile_args)
            negative_cases = (
                ("only generated extension names are callable", "Recruitment.unknown", "not assignable"),
                ("extension input follows the selected Id wire", "WRONG_CLUB_ID", "not assignable"),
                ("generated extension outputs are readonly", "first.approvedApplicants = 99", "readonly"),
            )
            for marker, expression, diagnostic in negative_cases:
                negative = directory / ("negative-" + str(negative_cases.index((marker, expression, diagnostic))) + ".mts")
                mutated = caller.replace("// @ts-expect-error " + marker, "", 1)
                if mutated == caller:
                    raise RuntimeError("expected-error marker was not found")
                negative.write_text(mutated)
                try:
                    run([*compile_args[:-1], str(negative)])
                except RuntimeError as error:
                    detail = str(error).lower()
                    if diagnostic not in detail and not (diagnostic == "readonly" and "cannot assign to" in detail):
                        raise RuntimeError("type negative control failed for an unrelated diagnostic") from error
                else:
                    raise RuntimeError("type negative control unexpectedly compiled: " + expression)

            extensions = ROOT.parent / "spikes/spike-v4-worker/extensions"
            process = subprocess.Popen([
                binary, "serve", str(source), "--schema", schema, "--db-url", db_url,
                "--listen", "127.0.0.1:0", "--wire", wire, "--dev-actor", "1", "--dev-token-ttl", "60",
                "--worker-dir", str(extensions), "--worker-lang", lang,
            ], cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            servers.append(process)
            event = listening(process)
            if event.get("fingerprint") != generated["fingerprint"]:
                raise RuntimeError("generated and running extension fingerprints differ")
            env = os.environ.copy()
            env.update(
                AIP_SMOKE_URL="http://" + event["address"],
                AIP_SMOKE_TOKEN=event["devToken"],
                AIP_SMOKE_WIRE="safe-number-v13" if wire == "safe" else "decimal-string-v13",
                AIP_SMOKE_LANG=lang,
            )
            response = json.loads(run(executable, env=env, timeout=25))
            expected = {"ok": True, "wire": env["AIP_SMOKE_WIRE"], "lang": lang, "approvedApplicants": 1}
            if response != expected:
                raise RuntimeError("typed extension caller result mismatch")
            stop(process)
            rows = psql(db_url, f"SELECT id FROM {schema}.recruitment ORDER BY id").strip().splitlines()
            if rows != ["100", "101"]:
                raise RuntimeError("extension smoke changed fixture rows")
            print(json.dumps({"ok": True, "sdk": sdk_mode, "wire": wire, "lang": lang, "approvedApplicants": 1}))
    finally:
        for process in servers:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=10)
        if owned:
            psql(db_url, f"DROP SCHEMA {schema} CASCADE")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db-url", default="host=localhost dbname=postgres")
    parser.add_argument("--sdk", choices=("source", "package", "both"), default="both")
    parser.add_argument("--wire", choices=("safe", "decimal", "both"), default="both")
    parser.add_argument("--lang", choices=("node", "python", "both"), default="both")
    args = parser.parse_args()

    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    run([cargo, "build", "--offline"])
    binary = str(ROOT / "target/debug/aip-prototype")
    tsc = str(ROOT.parent / "spikes/spike-0-ts/node_modules/.bin/tsc")
    sdk_modes = modes(args.sdk, ("source", "package"))
    wires = modes(args.wire, ("safe", "decimal"))
    langs = modes(args.lang, ("node", "python"))

    with tempfile.TemporaryDirectory(prefix="aip-extension-smoke-") as temp:
        shared = Path(temp)
        if "package" in sdk_modes:
            install_package(shared)
        for sdk_mode in sdk_modes:
            for wire in wires:
                for lang in langs:
                    exercise(args.db_url, binary, tsc, shared, sdk_mode, wire, lang)


if __name__ == "__main__":
    main()
