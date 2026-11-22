"""독립 소비자 검증의 출처 판정. run.py가 쓰고 test_provenance.py가 반례로 고정한다.

여기 함수는 파일·프로세스를 건드리지 않는다. 입력(실행 대상 SHA, checkout HEAD, 빌드 메타, 아티팩트 해시)만 받아
일치 여부를 `Check`로 돌려주고, 하나라도 어긋나면 `require_all`이 ProvenanceError를 던진다.
"""
import hashlib
import io
import re
import tarfile
import zipfile
from dataclasses import dataclass, asdict

SHA40 = re.compile(r"^[0-9a-f]{40}$")


class ProvenanceError(Exception):
    pass


@dataclass
class Check:
    name: str
    ok: bool
    detail: str

    def as_dict(self):
        return asdict(self)


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def resolve_target(argument, environment, head):
    """실행 대상 SHA와 출처. 입력(--core-sha, AIP_CONSUMER_CORE_SHA)이 없으면 checkout HEAD를 대상으로 삼는다.

    CI는 항상 입력을 넘긴다(inputs.core_sha 또는 github.sha). 입력이 있으면 checkout HEAD가 그 값과 같아야 한다.
    """
    if argument:
        sha, source = argument, "--core-sha"
    elif environment.get("AIP_CONSUMER_CORE_SHA"):
        sha, source = environment["AIP_CONSUMER_CORE_SHA"], "AIP_CONSUMER_CORE_SHA"
    else:
        return head, "checkout HEAD (대상 입력 없음, 로컬 실행)"
    if not SHA40.match(sha):
        raise ProvenanceError(f"실행 대상 SHA가 40자리 소문자 16진수가 아니다: {sha!r}")
    return sha, source


def source_checks(expected, head, dirty):
    return [
        Check("source-head-matches-target", head == expected, f"checkout HEAD {head} / 실행 대상 {expected}"),
        Check("source-tree-clean", not dirty, "추적 파일 변경 없음" if not dirty else f"추적 파일 변경: {dirty.splitlines()[:3]}"),
    ]


def engine_checks(expected, workspace_version, version_output, binary_sha256, builds):
    """`aip --version`, 빌드한 바이너리 해시, 패키지 BUILD.json(source_revision·binary_sha256)을 실행 대상과 대조한다.

    builds: {앱 이름: BUILD.json 내용}. 패키지는 run.py가 방금 만든 바이너리를 그대로 복사하므로
    BUILD.json의 binary_sha256이 같고 source_revision이 대상 SHA면 엔진이 대상 소스에서 빌드된 것이다.
    """
    checks = [Check("engine-version", version_output == f"aip {workspace_version}", f"`aip --version` = {version_output!r}, Cargo.toml = {workspace_version}")]
    for app, build in sorted(builds.items()):
        checks.append(Check(f"engine-build-source:{app}", build.get("source_revision") == expected and build.get("source_dirty") is False,
                            f"BUILD.json source_revision={build.get('source_revision')} dirty={build.get('source_dirty')}"))
        checks.append(Check(f"engine-build-binary:{app}", build.get("binary_sha256") == binary_sha256,
                            f"BUILD.json binary_sha256={str(build.get('binary_sha256'))[:16]} / 빌드한 aip {binary_sha256[:16]}"))
        checks.append(Check(f"engine-build-version:{app}", build.get("engine_version") == version_output, f"BUILD.json engine_version={build.get('engine_version')!r}"))
    return checks


def tar_file_hashes(data):
    out = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive.getmembers():
            if member.isfile():
                name = member.name.split("/", 1)[1] if "/" in member.name else member.name
                out[name] = sha256_bytes(archive.extractfile(member).read())
    return out


def sdk_checks(npm_name, expected_npm_name, npm_version, workspace_version, tarball_hashes, fresh_hashes,
               wheel_name, expected_wheel_name, wheel_version, python_version, wheel_hashes, source_hashes):
    """SDK 아티팩트가 대상 소스의 현재 버전·내용인지 본다.

    tarball_hashes/fresh_hashes: 설치한 npm tarball 내용과, 같은 checkout에서 build-sdk.mjs로 새로 만든 내용(상대 경로 → sha256).
    wheel_hashes/source_hashes: 설치한 wheel 안의 aip_sdk 파일과 checkout의 product/python/aip_sdk 파일.
    """
    return [
        Check("sdk-npm-file-name", npm_name == expected_npm_name, f"{npm_name} / 기대 {expected_npm_name}"),
        Check("sdk-npm-version", npm_version == workspace_version, f"tarball package.json {npm_version} / Cargo.toml {workspace_version}"),
        Check("sdk-npm-content-equals-fresh-build", tarball_hashes == fresh_hashes and bool(fresh_hashes),
              f"{len(tarball_hashes)}개 파일, 차이 {sorted(set(tarball_hashes.items()) ^ set(fresh_hashes.items()))[:2]}"),
        Check("sdk-wheel-file-name", wheel_name == expected_wheel_name, f"{wheel_name} / 기대 {expected_wheel_name}"),
        Check("sdk-wheel-version", wheel_version == python_version, f"wheel METADATA {wheel_version} / 기대 {python_version}"),
        Check("sdk-wheel-content-equals-source", wheel_hashes == source_hashes and bool(source_hashes),
              f"{len(wheel_hashes)}개 파일, 차이 {sorted(set(wheel_hashes.items()) ^ set(source_hashes.items()))[:2]}"),
    ]


def wheel_info(data):
    """wheel(zip)의 METADATA Version과 aip_sdk/ 파일 해시."""
    version, files = None, {}
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        for name in archive.namelist():
            if name.endswith(".dist-info/METADATA"):
                match = re.search(r"^Version: (.+)$", archive.read(name).decode(), re.M)
                version = match[1].strip() if match else None
            elif name.startswith("aip_sdk/") and not name.endswith("/"):
                files[name] = sha256_bytes(archive.read(name))
    return version, files


def require_all(checks):
    failed = [check for check in checks if not check.ok]
    if failed:
        raise ProvenanceError("; ".join(f"{check.name}: {check.detail}" for check in failed))
