# AIP 1.0.0 릴리스 노트 (초안)

**초안이다. 소유자 승인 전이며 태그, GitHub Release, registry publish는 아직 없다.** 확정되지 않은 항목은 `TBD`로 적었다. 이 노트는 `release.yml`이 만드는 GitHub Release 초안의 본문으로 쓴다.

릴리스 날짜: TBD. 태그: `v1.0.0`(TBD).

## 요약

AIP(Application Intent Protocol) 1.0.0은 정의 파일 하나로 인증·정책·트랜잭션을 갖춘 서비스를 PostgreSQL 위에 띄우는 엔진(`aip` CLI)과 두 호출 SDK(TypeScript, Python)의 첫 안정 릴리스다. Core, TypeScript SDK, Python SDK는 같은 버전 1.0.0을 쓴다(lockstep).

## 구성과 버전

| 구성 | 값 |
| --- | --- |
| 엔진(`aip --version`) | `aip 1.0.0` |
| TypeScript SDK | `@aip/sdk@1.0.0`(이름 잠정, 소유자 승인 대상) |
| Python SDK | `application-intent-protocol-sdk==1.0.0`, import 이름 `aip_sdk`(이름 잠정, 소유자 승인 대상) |
| wire protocol | `aip-protocol/1.0` |
| Core IR | `aip-core/0.11`(1.0에서 올리지 않음, 이유는 [COMPATIBILITY](../COMPATIBILITY.md)) |

## 변경점

- **버전 원천 통일.** 루트 `Cargo.toml`의 workspace 버전 하나가 엔진, TypeScript SDK, Python SDK, 패키징 도구, smoke의 버전을 정한다. 어긋나면 `scripts/check-versions.mjs`가 실패한다.
- **protocol `aip-protocol/0.1` → `aip-protocol/1.0`.** 계약 문서의 `protocol` 필드와 생성 binding 헤더 주석이 바뀐다. 계약 지문은 바뀌지 않는다. 호환 경계는 protocol의 major다. 1.x 안에서는 계약 문서에 선택 필드를 더하는 변경만 있다.
- **패키지를 publish할 수 있게 정비.** TypeScript tarball은 더 이상 `private`이 아니고 `license`, `repository`, `files`, README, LICENSE를 갖췄으며 내부 `spikes/` 경로가 없다. Python wheel과 sdist에는 LICENSE와 README(long description)가 들어간다.
- **패키지 이름을 한 곳에서 정한다.** `product/release.config.json`. 기본값은 npm `@aip/sdk`, PyPI `application-intent-protocol-sdk`다.
- **제품 바이너리 빌드가 `PG*` 환경 변수에 막히지 않는다.** `aip-test-support`의 `build.rs`는 테스트 빌드에서만 `PG*` 변수를 거부하고, 테스트 DSN을 꺼내는 시점에도 같은 검사를 한다. 개발자 셸에 `PGHOST`가 있어도 `cargo build -p aip-cli`가 성공한다.
- **release 빌드가 테스트 안전장치와 분리됐다.** `aip-test-support`가 `aip-cli`의 release 의존 그래프에 없다(`spike-v2-read`와 `spike-v6-transport`의 `test-db` feature 뒤로 이동). `cargo tree -p aip-cli -e normal,build -i aip-test-support`가 비어 있고, 테스트 빌드의 `PG*` 거부는 그대로다.
- **개발용 토큰 도구 동봉.** 릴리스 아카이브와 `package.mjs` 묶음에 `tools/dev-jwt.mjs`가 들어 있어 저장소 checkout 없이 개발 토큰을 만든다(운영 금지). 앱 패키지에는 넣지 않는다.
- **앱 패키징 검사 강화.** 정의가 worker 확장을 선언했는데 manifest에 `extensions`가 없거나 모듈 파일이 없으면 `package.mjs --manifest`가 실패한다. `config.example.json`에는 `workers` 블록 예시가 들어간다.
- **배포 실패 원인 분류.** `app-deploy`의 `SERVICE_FAILED`가 `cause`(`DB_UNREACHABLE`, `SCHEMA_EXISTS`, `DB_PRIVILEGE`, `PLAN_DIGEST_MISMATCH` 등)와 `engineCode`를 알려 준다. 엔진 원문은 계속 전달하지 않는다. 엔진의 `init`은 이미 있는 schema를 `SCHEMA_EXISTS`로 보고한다(이전에는 `DB_INIT`).
- **SDK 버전 식별.** 두 SDK가 자기 버전을 노출하고 `x-aip-sdk` 헤더로 보낸다. 서버는 로그와 진단에만 쓰고 거부하지 않는다. 브라우저에서 SDK 1.0.0을 쓰려면 엔진도 1.0.0 이상이어야 한다(CORS 허용 헤더). 자세한 내용은 [COMPATIBILITY](../COMPATIBILITY.md#sdk-버전-식별).
- **Python wheel을 오프라인으로 빌드할 수 있다.** `scripts/build-python.sh --wheelhouse|--no-isolation`. 요구사항은 [INSTALL](INSTALL.md)에 있다.
- **릴리스 자동화.** 태그 push로 시작하는 `release.yml`: 4개 플랫폼 CLI 아카이브, `SHA256SUMS`, 빌드 provenance attestation, GitHub Release 초안, npm provenance publish, PyPI trusted publishing. 승인(environment `release`)과 태그 조건 없이는 publish하지 않는다.
- **Linux 바이너리의 glibc 하한을 낮췄다.** Debian 12(glibc 2.36) 이미지에서 빌드해 요구 glibc가 2.39에서 2.34로 내려갔다(aarch64 측정). 자세한 내용은 [INSTALL](INSTALL.md#glibc-하한).

## 설치

[INSTALL](INSTALL.md)을 따른다. 요약:

```sh
# CLI: GitHub Release 아카이브 (TBD: 공개 후 URL)
npm install @aip/sdk                                  # TBD: 이름 승인 후
pip install application-intent-protocol-sdk           # TBD: 이름 승인 후
```

`cargo install aip-cli`와 `pip install aip-sdk`는 제3자 패키지를 설치하므로 쓰지 않는다.

## 지원 플랫폼

| target | 상태 |
| --- | --- |
| `x86_64-unknown-linux-gnu` | glibc 2.34 이상. 첫 CI 실행에서 확인(TBD) |
| `aarch64-unknown-linux-gnu` | glibc 2.34 이상 |
| `aarch64-apple-darwin` | macOS 11 이상 |
| `x86_64-apple-darwin` | TBD. Intel runner 제공에 의존 |

CI가 네 target 모두에서 증명하는 것은 빌드, `aip --version`, `service check`다. PostgreSQL 전체 회귀와 worker 격리는 Linux x86_64 CI와 소유자 Mac의 수동 실행이 증명한다. Windows는 지원하지 않는다.

## 알려진 한계와 미정 항목

- **패키지 이름 확정**: npm scope와 PyPI 이름은 소유자 승인 전이다(TBD).
- **protocol 비교는 경고뿐**: 엔진이 `/capabilities`에 `protocol`을 싣고 Python SDK가 major 불일치를 `AipProtocolWarning`으로 알린다. 호출을 막지는 않는다(호환 판단은 계약 지문). TypeScript 클라이언트는 `/capabilities`를 부르지 않아 자동 경고가 없고 `protocolWarning()` 함수만 공개한다(자동 경고는 TBD).
- **Core IR 태그 `aip-core/0.11` 유지**: 배포 저널에 저장되는 값이라 올리지 않았다. IR 의미가 바뀔 때 올린다.
- **공개 Docker 이미지 없음**: 앱 패키지를 받아 직접 빌드한다.
- **릴리스 gate의 baseline**: 업그레이드·롤백 경로의 기준을 "직전 릴리스 태그"로 바꾸는 일은 첫 릴리스 뒤에 한다(TBD).
- **Intel macOS 지원**: runner 제공이 끊기면 제외한다(TBD).
- **SBOM attestation**: 1.0.1 이후에 검토한다(TBD).

## 검증

릴리스 gate와 이전 릴리스 업그레이드·롤백 증거는 [RELEASE-GATE](RELEASE-GATE.md)에 있다. 이번 패키징 변경의 검증 결과(로컬 명령과 PR 검사)는 PR 본문에 있다. 산출물 해시는 Release의 `SHA256SUMS`와 `aip-1.0.0-manifest.json`(TBD)에 있다.

## 업그레이드

0.1.0에서 올릴 때 바뀌는 점: 생성 binding의 헤더 주석(`aip-protocol/1.0`), 앱 패키지 안의 SDK 파일 이름(`aip-sdk-1.0.0.tgz`), Python 배포 이름(`aip-sdk` → `application-intent-protocol-sdk`, 0.1.0은 공개된 적이 없다). 계약 지문은 바뀌지 않고, Core IR 태그를 유지했으므로 배포 저널에 기록된 IR digest도 같다. 기존 배포가 "변경됨"으로 읽히지 않는다는 점은 업그레이드 시험(`upgrade-drill`)으로 확인해야 한다(미실행, TBD).
