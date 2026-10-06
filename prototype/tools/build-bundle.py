#!/usr/bin/env python3
"""Build a private, relocatable macOS development bundle from this checkout."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile


PROTOTYPE = Path(__file__).resolve().parents[1]
ROOT = PROTOTYPE.parent
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


class BundleError(Exception):
    pass


def run(argv, cwd, timeout_secs=30):
    """Run one trusted tool with an argv vector; never pass through a shell."""
    try:
        result = subprocess.run(
            [str(arg) for arg in argv], cwd=cwd, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=timeout_secs,
        )
    except subprocess.TimeoutExpired:
        raise BundleError(f"{Path(argv[0]).name} 제한 시간({timeout_secs}초) 초과") from None
    except OSError as error:
        raise BundleError(f"{Path(argv[0]).name} 실행 실패: {error.strerror or error.__class__.__name__}") from None
    if result.returncode != 0:
        detail = result.stderr.strip().splitlines()
        tail = "\n".join(detail[-8:])
        raise BundleError(f"{Path(argv[0]).name} 종료 코드 {result.returncode}" + (f"\n{tail}" if tail else ""))
    return result.stdout


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_pack_filename(filename):
    if not isinstance(filename, str) or Path(filename).name != filename or filename != SDK_PACKAGE:
        raise BundleError("npm pack이 예상하지 않은 tarball 파일명을 반환했습니다")
    return filename


def source_inputs():
    paths = set(EXPLICIT_SOURCE_FILES)
    for crate in LOCAL_CRATES:
        paths.add(f"{crate}/Cargo.toml")
        paths.add(f"{crate}/Cargo.lock")
        source_dir = ROOT / crate / "src"
        paths.update(path.relative_to(ROOT).as_posix() for path in source_dir.rglob("*.rs"))
    return sorted(paths)


def toolchain_versions():
    compiler = ROOT / "spikes/spike-0-ts/node_modules/.bin/tsc"
    commands = {
        "cargo": ["cargo", "--version"],
        "rustc": ["rustc", "--version"],
        "node": ["node", "--version"],
        "npm": ["npm", "--version"],
        "typescript": [compiler, "--version"],
    }
    return {name: run(argv, cwd=PROTOTYPE, timeout_secs=10).strip() for name, argv in commands.items()}


def copy_input(source, destination):
    if not source.is_file() or source.is_symlink():
        raise BundleError(f"필수 입력 파일이 없거나 링크임: {source.name}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)


def seed_sql():
    return '''-- 로컬 데모 데이터만 넣습니다. schema는 사용자가 새로 만든 이름을 전달하세요.
INSERT INTO :"schema".school (id, name) VALUES (1, 'A대');
INSERT INTO :"schema".member (id, school_id) VALUES (1, 1), (2, 1), (3, 1);
INSERT INTO :"schema".club (id, name, logo, school_id) VALUES
  (10, 'A동아리', NULL, 1), (11, 'B동아리', NULL, 1);
INSERT INTO :"schema".club_member (club_id, member_id, role) VALUES
  (10, 1, 'MANAGER'), (10, 2, 'MEMBER'), (11, 3, 'ADMIN');
INSERT INTO :"schema".recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
  (100, 'A 모집', '2099-10-10T00:00:00Z', 'PUBLISHED', 0, 10, NULL),
  (101, 'B 모집', '2099-10-12T00:00:00Z', 'PUBLISHED', 0, 11, NULL);
INSERT INTO :"schema".apply (id, recruitment_id, status) VALUES
  (200, 100, 'APPROVE'), (201, 100, 'PENDING'),
  (202, 101, 'APPROVE'), (203, 101, 'APPROVE');
'''


def event_seed_sql():
    return '''INSERT INTO :"schema".member(id) VALUES(1),(2);
INSERT INTO :"schema".event(id,member_id,checked,title,link,phase,date) VALUES
  (11,1,false,'first','https://example.test/1','READY','2026-10-04Z'),
  (12,1,false,'second','https://example.test/2','READY','2026-10-04Z'),
  (99,2,false,'private','https://example.test/3','DONE','2026-10-04Z');
'''


def readme(profile):
    rebuild = f"python3 tools/build-bundle.py --out <새-출력-경로> --profile {profile}"
    return f'''# AIP 로컬 개발 bundle

이 산출물은 현재 macOS host용 **private 로컬 개발 실험**입니다. 제품 배포본이나 운영 보안 경계가 아닙니다. 다른 OS·CPU, 최종 Id/ABI, 운영 로그인, 확장 격리, release 정책은 이 bundle에서 결정하지 않습니다.

이 버전의 init은 실제 DB 구조 지문을 marker에 저장합니다. serve는 컬럼·제약·인덱스·시퀀스 정의와 객체 종류 등의 변경을 기동 전에 거부합니다. 이전 marker-only schema는 SCHEMA_NOT_READY로 거부하며 데이터·구조를 자동 변경하지 않습니다. 일반 migration과 PostgreSQL 버전 간 지문 호환은 아직 제공하지 않습니다.

DB 연결과 실행 전 시각 조회는 각각5초, init/serve의 SQL stage도 별도5초입니다. HTTP 읽기·쓰기 operation은 선언된 기한을 적용하고 status 조회는5초입니다. 커밋 응답이 불명확하면 COMMIT_UNKNOWN 또는 INIT_UNSETTLED로 보존합니다. HTTP 전체가 하나의5초 기한이라는 뜻은 아닙니다.

유효한 세션으로 시작한 WRITE가 커밋됐다면 실행 중 세션 만료를 실패 응답으로 바꾸지 않습니다. 다음 요청은 만료 토큰을 거부하며 읽기 응답·캐시에는 남은 세션 수명을 적용합니다.

`sourceSha256`는 manifest에 열거된 로컬 입력 파일의 체크섬입니다. 외부 registry cache의 내용이나 Cargo/npm 의존성을 증명하지 않으며, bit-for-bit 재현성을 보장하지 않습니다.

## 들어 있는 파일

- `bin/aip-prototype`: Cargo로 빌드한 CLI. Node/Python worker bootstrap을 바이너리에 포함하므로 checkout의 worker 경로에 의존하지 않습니다.
- `examples/app.aip`: `prototype/example/app.aip`의 정본 예제.
- `extensions/node/event.mjs`, `extensions/python/event.py`, `examples/event-seed.sql`: 정본 Event.confirm WRITE 확장과 명시적 개발 seed.
- `examples/recruitment.aip`: Recruitment READ 확장 예제와 계약.
- `extensions/node/recruitment.mjs`, `extensions/python/recruitment.py`: 예제에서 선언한 `Recruitment.stats` 구현 파일.
- `examples/recruitment-seed.sql`: 개발자가 선택한 schema에 넣을 소량의 로컬 데모 행. bundle이나 `init`은 seed를 자동 실행하지 않습니다.
- `sdk/{SDK_PACKAGE}`: 기존 JS SDK를 컴파일해 만든 offline npm tarball.
- `manifest.json`: host platform/architecture, build profile, checkout 밖에서 다시 실행할 상대 명령, source input과 bundle 파일의 SHA-256 목록. `files` 목록은 manifest 자신을 제외합니다.

## 요구 도구

Bundle을 만들 때는 저장소 checkout, Cargo 의존성의 offline cache, Node/npm, checkout의 로컬 TypeScript 컴파일러가 필요합니다. 이 bundle을 사용할 때는 macOS, Node, Python 3.11 이상, PostgreSQL과 `psql`이 필요합니다. 사용 단계에는 Cargo·AIP checkout·네트워크 설치가 필요하지 않습니다. Worker 네트워크 차단은 현재 macOS `sandbox-exec` 로컬 실험입니다.

## bundle 다시 만들기

AIP 저장소 checkout의 `prototype/` 디렉터리에서 아래 명령으로 fresh 출력 경로를 지정합니다. 출력 경로는 기존 파일·디렉터리·symlink를 덮어쓰지 않습니다. profile은 `dev` 또는 `release`입니다.

```sh
python3 tools/build-bundle.py --out <새-출력-경로> --profile {profile}
```

`manifest.json`의 `rebuildCommand`는 출력 경로만 바꿔 재현할 명령을 기록합니다. 산출물 SHA가 일치하는지 `shasum -a 256`과 manifest 목록으로 확인할 수 있습니다. 빌드가 실패하면 이 실행에서 새로 만든 출력 디렉터리만 제거합니다.

## Event WRITE 흐름 실행

`Event.confirm`은 공개 `Event.mark` 전이를 호출하고 결과를 돌려줍니다. 실행하려면 명시적 WRITE 설정이 필요합니다. 표준 apply와 같은 멱등 key·상태 조회·SDK pending·읽기 캐시 무효화 경로를 사용합니다.

```sh
schema=aip_event_demo
./bin/aip-prototype gen examples/app.aip --wire decimal --out /tmp/event.contract.ts --sdk-import @aip/prototype-sdk
./bin/aip-prototype init examples/app.aip --schema "$schema" --db-url 'host=localhost dbname=postgres'
psql 'host=localhost dbname=postgres' -v ON_ERROR_STOP=1 --single-transaction -v schema="$schema" -f examples/event-seed.sql
./bin/aip-prototype serve examples/app.aip --schema "$schema" --db-url 'host=localhost dbname=postgres' --listen 127.0.0.1:8080 --wire decimal --dev-actor 1 --worker-dir extensions/node --worker-lang node --enable-write-extensions
```

생성 binding과 설치한 SDK에서 서버 readiness의 `devToken`으로 호출합니다.

```ts
import {{connect}} from '@aip/prototype-sdk';
import {{contract}} from './event.contract.ts';
const client = connect('http://127.0.0.1:8080', devToken, contract);
const result = await client.writeExtension('Event.confirm', {{id:'11'}}, {{key:'event-confirm-11'}});
if (result.ok) console.log(result.output.id, result.output.count);
```

동일 key/request 재시도는 저장한 결과를 재생합니다. 새 key는 새 의도이며, 이미 checked인 행은 repeat unchanged에 따라 count 0입니다. 최종 WRITE API·격리·운영 인증은 이 private 예제에서 확정하지 않습니다.

## Recruitment 흐름 실행

Bundle 루트에서 실행합니다. 예제 schema는 비어 있어야 하며 `init`은 테이블만 만듭니다. seed는 사용자가 명시적으로 실행한 뒤에만 개발 actor 토큰 발급이 가능합니다.

```sh
schema=aip_bundle_demo
./bin/aip-prototype check examples/app.aip
./bin/aip-prototype gen examples/recruitment.aip --wire decimal --out /tmp/recruitment.contract.ts --sdk-import @aip/prototype-sdk
./bin/aip-prototype init examples/recruitment.aip --schema "$schema" --db-url 'host=localhost dbname=postgres'
psql 'host=localhost dbname=postgres' -v ON_ERROR_STOP=1 --single-transaction -v schema="$schema" -f examples/recruitment-seed.sql
./bin/aip-prototype serve examples/recruitment.aip --schema "$schema" --db-url 'host=localhost dbname=postgres' --listen 127.0.0.1:8080 --wire decimal --dev-actor 1 --worker-dir extensions/node --worker-lang node
```

Python worker를 확인하려면 마지막 명령의 `extensions/node --worker-lang node`를 `extensions/python --worker-lang python`으로 바꿉니다. 두 server는 같은 fixture의 `Recruitment.stats`를 호출합니다. 서버는 loopback에만 bind하며 로그인을 제공하지 않습니다. `--dev-actor 1`은 seed에 이미 있는 Member 1의 단기 로컬 토큰입니다.

생성 binding을 별도 소비자에서 쓰려면 SDK를 offline 설치합니다. `bundle-root`를 실제 bundle 절대 경로로 정하고 소비자 디렉터리를 새로 만드세요.

```sh
bundle_root=/absolute/path/to/bundle # 실제 bundle 경로로 바꾸세요
consumer_dir="$(mktemp -d /tmp/aip-bundle-consumer.XXXXXX)"
cd "$consumer_dir"
npm init -y
npm install --offline --ignore-scripts --no-audit --no-fund "$bundle_root/sdk/{SDK_PACKAGE}"
```

생성된 binding은 `@aip/prototype-sdk`를 import합니다. TypeScript 컴파일러와 프론트 런타임은 bundle에 넣지 않았으며 소비자 프로젝트가 정합니다.

로컬에서 더 이상 쓰지 않을 때는 먼저 서버를 `Ctrl-C`로 종료하고, 본인이 만든 schema만 직접 정리합니다. 예제는 기존 schema를 자동 삭제하지 않습니다.
'''


def bundle_files(out, profile):
    binary_relative = Path("target/release/aip-prototype") if profile == "release" else Path("target/debug/aip-prototype")
    binary = PROTOTYPE / binary_relative
    if not binary.is_file() or binary.is_symlink():
        raise BundleError("Cargo가 예상 위치에 실행 파일을 만들지 않았습니다")
    copy_input(binary, out / "bin/aip-prototype")

    inputs = {
        "examples/app.aip": PROTOTYPE / "example/app.aip",
        "extensions/node/event.mjs": PROTOTYPE / "example/extensions/event.mjs",
        "extensions/python/event.py": PROTOTYPE / "example/extensions/event.py",
        "examples/recruitment.aip": ROOT / "spikes/spike-v1-fixture/fixture/recruitment.aip",
        "extensions/node/recruitment.mjs": ROOT / "spikes/spike-v4-worker/extensions/recruitment.mjs",
        "extensions/python/recruitment.py": ROOT / "spikes/spike-v4-worker/extensions/recruitment.py",
    }
    for relative, source in inputs.items():
        copy_input(source, out / relative)

    (out / "examples/event-seed.sql").write_text(event_seed_sql(), encoding="utf-8")
    seed_path = out / "examples/recruitment-seed.sql"
    seed_path.write_text(seed_sql(), encoding="utf-8")
    sdk_dir = out / "sdk"
    sdk_dir.mkdir()
    with tempfile.TemporaryDirectory(prefix="aip-bundle-sdk-") as temp:
        temp_dir = Path(temp)
        built_sdk = temp_dir / "package"
        run(["node", PROTOTYPE / "tools/build-sdk.mjs", "--out", built_sdk], cwd=PROTOTYPE, timeout_secs=90)
        packed_json = run(
            ["npm", "pack", "--offline", "--ignore-scripts", "--json", "--pack-destination", temp_dir, built_sdk],
            cwd=PROTOTYPE,
            timeout_secs=30,
        )
        try:
            packed = json.loads(packed_json)
            filename = validate_pack_filename(packed[0]["filename"])
            package_file = temp_dir / filename
        except (IndexError, KeyError, TypeError, json.JSONDecodeError) as error:
            raise BundleError("SDK tarball 정보를 읽을 수 없습니다") from error
        if not package_file.is_file():
            raise BundleError("SDK tarball이 생성되지 않았습니다")
        copy_input(package_file, sdk_dir / SDK_PACKAGE)

    (out / "README.md").write_text(readme(profile), encoding="utf-8")

    source_hashes = {relative: sha256(ROOT / relative) for relative in source_inputs()}
    output_hashes = {}
    for path in sorted(out.rglob("*")):
        if path.is_symlink():
            raise BundleError("bundle에 symlink가 들어갈 수 없습니다")
        if path.is_file():
            output_hashes[path.relative_to(out).as_posix()] = sha256(path)

    manifest = {
        "format": "aip-local-bundle-v1",
        "bundleKind": "private-local-macos-development",
        "profile": profile,
        "host": {"platform": "macos", "architecture": platform.machine()},
        "rebuildCommand": ["python3", "tools/build-bundle.py", "--out", "<새-출력-경로>", "--profile", profile],
        "toolchainVersions": toolchain_versions(),
        "sourceSha256": source_hashes,
        "files": output_hashes,
    }
    (out / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def build(out_arg, profile):
    if platform.system() != "Darwin":
        raise BundleError("현재 host용 private bundle은 macOS에서만 생성합니다")
    out = Path(os.path.abspath(out_arg))
    if os.path.lexists(out):
        raise BundleError("출력 경로는 없는 fresh 경로여야 합니다")
    try:
        out.parent.mkdir(parents=True, exist_ok=True)
    except OSError:
        raise BundleError("출력 경로의 부모 디렉터리를 준비하지 못했습니다 (OUTPUT_IO)") from None
    try:
        out.mkdir()
    except FileExistsError:
        raise BundleError("출력 경로는 없는 fresh 경로여야 합니다") from None
    owned = True
    try:
        cargo_args = ["cargo", "build", "--offline", "--manifest-path", PROTOTYPE / "Cargo.toml"]
        if profile == "release":
            cargo_args.append("--release")
        run(cargo_args, cwd=PROTOTYPE, timeout_secs=600 if profile == "release" else 180)
        bundle_files(out, profile)
        owned = False
        return out
    finally:
        if owned and out.is_dir() and not out.is_symlink():
            shutil.rmtree(out)


def main(argv=None):
    parser = argparse.ArgumentParser(description="Build a private macOS local AIP development bundle")
    parser.add_argument("--out", required=True, help="새 출력 디렉터리")
    parser.add_argument("--profile", choices=("dev", "release"), default="dev")
    args = parser.parse_args(argv)
    try:
        out = build(args.out, args.profile)
    except BundleError as error:
        print(json.dumps({"ok": False, "code": "BUNDLE_FAILED", "msg": str(error)}, ensure_ascii=False), file=sys.stderr)
        return 1
    print(json.dumps({"ok": True, "out": str(out), "profile": args.profile}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
