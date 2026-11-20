# 설치 안내 (1.0.0 초안)

이 문서는 AIP 1.0.0의 설치 방법이다. **초안이며 공개 전이다.** 아직 어떤 registry에도 publish하지 않았고 태그와 GitHub Release도 없다. 아래에서 `TBD`는 소유자 결정이나 첫 공개 뒤에야 정해지는 값이다. 패키지 이름은 잠정이며 최종 이름은 **소유자 승인 대상**이다([버전과 이름의 원천](SDK.md#버전과-이름의-원천)).

릴리스 노트는 [RELEASE-NOTES-1.0.0](RELEASE-NOTES-1.0.0.md), 버전 대응 표는 [COMPATIBILITY](../COMPATIBILITY.md)이고, 무엇이 지원 범위인지는 [1.0 지원 범위](SUPPORT.md)다.

## 무엇을 설치하나

| 구성 | 배포 위치 | 이름(잠정) | 설치 |
| --- | --- | --- | --- |
| `aip` CLI·서비스 엔진 | GitHub Release 아카이브 | `aip-<version>-<target>.tar.gz` | 아래 "CLI" |
| TypeScript SDK | npm | `@aip/sdk` | `npm install @aip/sdk` |
| Python SDK | PyPI | `application-intent-protocol-sdk` (import는 `aip_sdk`) | `pip install application-intent-protocol-sdk` |

설치하지 않는 방법이 있다.

- **`cargo install aip-cli`, `cargo install aip`를 쓰지 않는다.** crates.io의 `aip`, `aip-cli`, `aip-sdk`는 이 프로젝트와 무관한 제3자 crate다. 이 저장소의 Cargo workspace는 crates.io에 올리지 않는다. CLI는 GitHub Release 아카이브나 소스 빌드(`cargo build -p aip-cli --release`)로만 얻는다.
- **`pip install aip-sdk`를 쓰지 않는다.** PyPI의 `aip-sdk`는 제3자(Patrik Kjaerran) 소유의 다른 프로젝트다. 이 SDK의 배포 이름은 `application-intent-protocol-sdk`이고 import 이름만 `aip_sdk`다.
- **공개 Docker 이미지는 1.0에 없다.** 이미지는 앱 단위이며 앱 패키지를 받아 직접 `docker build`한다([DOCKER](DOCKER.md)).

## 지원 플랫폼과 최소 요구

CLI 아카이브는 아래 네 target을 `release.yml`이 각 플랫폼의 runner에서 빌드한다(크로스 컴파일은 지원하지 않는다). 각 job은 아카이브를 풀어 `aip --version`이 Cargo 버전과 같은지, `service check example.aip`가 통과하는지 확인한다. 이 검사가 CI가 증명하는 범위의 전부다. PostgreSQL, worker 격리, SDK smoke 같은 전체 gate는 Linux x86_64 CI와 소유자 Mac의 수동 실행이 증명한다([RELEASE-GATE](RELEASE-GATE.md)).

| target | 빌드 환경 | 최소 요구 | 이 문서 작성 시점의 증거 |
| --- | --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `rust:1.98.1-bookworm` 컨테이너(glibc 2.36) | glibc 2.34 이상(Debian 12, Ubuntu 22.04, RHEL 9 계열) | 로컬 컨테이너 측정은 aarch64뿐이다. x86_64 값은 첫 CI 실행에서 확인한다(TBD) |
| `aarch64-unknown-linux-gnu` | 같은 컨테이너, `ubuntu-24.04-arm` runner | glibc 2.34 이상 | aarch64 `rust:1-bookworm` 컨테이너에서 빌드해 `objdump -T`로 `GLIBC_2.34`가 최댓값임을 확인 |
| `aarch64-apple-darwin` | `macos-15` | macOS 11 이상(`minos 11.0`, 측정은 이전 조사의 로컬 빌드) | 시스템 프레임워크와 libSystem만 링크 |
| `x86_64-apple-darwin` | `macos-15-intel` | TBD | 로컬 실행 불가. Intel runner의 제공 종료 시점은 확인 필요이며, 끊기면 이 target을 빼고 이 표를 고친다 |

Windows는 지원하지 않는다.

### glibc 하한

Linux 바이너리는 빌드한 환경의 glibc 심볼 버전만큼 요구한다. 빌드 이미지를 바꿔 같은 소스(`aip-cli` release 빌드)를 aarch64에서 측정했다.

| 빌드 이미지 | glibc | 바이너리가 요구하는 최댓값 | 비고 |
| --- | --- | --- | --- |
| `rust:latest`(trixie) | 2.41 | `GLIBC_2.39` | 이전 조사 측정. Debian 12와 Ubuntu 22.04에서 실행되지 않는다 |
| `rust:1-bookworm` | 2.36 | `GLIBC_2.34` | **채택.** `release.yml`이 이 이미지로 Linux를 빌드하고 2.34 상한을 넘으면 실패한다 |
| `rust:1-bullseye` | 2.31 | `GLIBC_2.30` | 더 낮지만 Debian 11은 2026-08에 LTS가 끝났다. RHEL 8(glibc 2.28)은 여전히 못 쓴다 |
| `rust:1-alpine`(musl) | 해당 없음 | 정적 링크 시도 | **채택하지 않음.** 아래 참고 |

musl 정적 링크(`aarch64-unknown-linux-musl`, `crt-static`)는 `spike-v4-worker`가 컴파일되지 않아 실패했다(오류는 아래 "musl 시도" 참고, aarch64만 시도했고 x86_64 musl은 시도하지 않았다). 코드 수정 없이는 쓸 수 없고, worker 격리(Bubblewrap)와 DNS·사용자 조회처럼 glibc 동작에 기대는 기능의 동등성도 검증되지 않았다. RHEL 8 같은 더 오래된 배포판을 지원하려면 별도 결정과 컨테이너 기반 실행(Docker)이 현실적이다.

## CLI

공개 전에는 아래 URL을 쓸 수 없다. 형식만 적는다(`TBD`: 첫 Release 뒤 확정).

```sh
version=1.0.0
target=x86_64-unknown-linux-gnu   # aarch64-unknown-linux-gnu, aarch64-apple-darwin, x86_64-apple-darwin
base=https://github.com/winterholic/Application-Intent-Protocol/releases/download/v$version   # TBD
curl -fsSLO "$base/aip-$version-$target.tar.gz"
curl -fsSLO "$base/SHA256SUMS"
shasum -a 256 -c SHA256SUMS --ignore-missing          # Linux에서는 sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify "aip-$version-$target.tar.gz" --repo winterholic/Application-Intent-Protocol
tar -xzf "aip-$version-$target.tar.gz"
"./aip-$version-$target/bin/aip" --version     # aip 1.0.0
```

`SHA256SUMS`는 파일 무결성을, `gh attestation verify`는 이 저장소의 `release.yml`이 만든 산출물임을 확인한다(GitHub artifact attestation, 키 없는 OIDC 서명). 둘은 같은 일이 아니다. 해시만으로는 만든 주체를 알 수 없다.

아카이브에는 `bin/aip`, `config.example.json`, `example.aip`, `extensions/`, `tools/dev-jwt.mjs`(개발용 토큰 도구, 운영 금지), `LICENSE`, `COMPATIBILITY.md`, 이 문서가 들어 있다. 첫 실행은 [시작 가이드](GETTING-STARTED.md)를 따르며, 개발 토큰은 압축을 푼 디렉터리에서 `node tools/dev-jwt.mjs --dir ./dev-idp --subject user-42`로 만든다(저장소 checkout 불필요, Node.js 필요). PostgreSQL 17에서 검증했고 다른 버전은 확인하지 않았다.

## TypeScript SDK

```sh
npm install @aip/sdk            # 이름 잠정(TBD)
```

공개 전에는 같은 Release의 `aip-sdk-<version>.tgz`로 오프라인 설치할 수 있다.

```sh
npm install --offline ./aip-sdk-1.0.0.tgz
```

`connect(baseUrl, accessToken, generatedBinding)`와 생성 binding 사용법은 [SDK](SDK.md)를 본다. binding은 `aip service gen ... --sdk-import <패키지 이름>`으로 만들며 기본 import 이름은 `@aip/sdk`다. 이름이 바뀌면 이 인자도 바꾼다. SDK와 binding은 같은 배포(계약 지문)에서 만든 것을 쓴다.

## Python SDK

```sh
pip install application-intent-protocol-sdk     # 이름 잠정(TBD)
python -c "import aip_sdk; print(aip_sdk.__version__, aip_sdk.PROTOCOL)"
```

공개 전에는 Release의 wheel을 쓴다.

```sh
pip install ./application_intent_protocol_sdk-1.0.0-py3-none-any.whl
```

소스에서는 `pip install ./product/python`이다. Python 3.9 이상, 외부 의존성은 없다.

### Python wheel을 직접 빌드할 때(네트워크 요구사항)

`python -m build`는 기본으로 격리 환경을 만들고 `setuptools>=77`을 인덱스에서 내려받으므로 **네트워크가 필요하다.** 네트워크 없이 빌드하려면 `scripts/build-python.sh`를 쓴다. 소스 디렉터리를 더럽히지 않도록 복사본에서 빌드한다.

| 경로 | 필요한 것 | 명령 |
| --- | --- | --- |
| 로컬 wheelhouse | 빌드하는 Python에 `build` 패키지, wheelhouse에 `setuptools>=77` wheel | `scripts/build-python.sh --out <새 디렉터리> --wheelhouse <dir>` |
| 격리 없음 | 빌드하는 Python에 `build`와 `setuptools>=77`이 이미 설치됨 | `scripts/build-python.sh --out <새 디렉터리> --no-isolation` |
| 기본 | 인덱스 접근 | `scripts/build-python.sh --out <새 디렉터리>` |

`wheel` 패키지는 필요 없다(setuptools 77이 wheel을 직접 만든다). wheelhouse는 네트워크가 되는 곳에서 `pip download "setuptools>=77" -d <dir>`로 한 번 준비한다. 두 오프라인 경로 모두 프록시를 죽인 상태에서 wheel과 sdist가 만들어지는 것을 확인했고, 격리 없음 경로의 산출물은 `check-versions.mjs --wheel --sdist`도 통과했다.

## 이름 확정 전에 남은 것

| 항목 | 상태 |
| --- | --- |
| npm scope `@aip`의 소유 | 확인되지 않음. 소유자가 npm에 로그인해 확인한다. 불가하면 `@application-intent-protocol` 등으로 바꾼다 |
| PyPI 배포 이름 | `application-intent-protocol-sdk`는 조회 시점에 미등록이었다. 선점 등록은 하지 않았다 |
| trusted publisher 등록 | npm과 PyPI 모두 워크플로 `release.yml`, environment `release`로 소유자가 등록해야 한다 |
| GitHub environment `release` | 소유자가 만들고 승인자와 `v*` 태그 제한을 지정해야 한다 |
| Intel macOS 지원 | `macos-15-intel` runner에 의존한다. 제공 종료 시 target을 뺀다 |

이름을 바꾸는 절차는 `product/release.config.json`을 고치고 `node scripts/sync-version.mjs`, `node scripts/check-versions.mjs`를 실행하는 것이다.

## musl 시도

aarch64 `rust:1-alpine`에서 `RUSTFLAGS="-C target-feature=+crt-static" cargo build --release -p aip-cli --locked --target aarch64-unknown-linux-musl`을 실행했다. 첫 시도(`--target` 없이 `crt-static`)는 `cannot produce proc-macro for async-trait ... target aarch64-unknown-linux-musl does not support these crate types`로 실패했고, `--target`을 주자 `spike-v4-worker`가 `error[E0425]: cannot find function \`getentropy\` in crate \`libc\``로 컴파일되지 않았다(musl 대상의 `libc`에 해당 함수가 없다). 이 두 번째 오류를 고친 뒤 이어지는 오류가 더 있는지는 확인하지 않았다.
