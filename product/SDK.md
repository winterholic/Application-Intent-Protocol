# `@aip/sdk` 배포 빌드 계약

이 문서는 운영 앱이 TypeScript SDK를 로컬 tarball로 설치할 수 있게 하는 빌더의 검수 기준이다. 공개 registry publish는 범위에 포함하지 않는다. 패키지 API는 기존 typed transport와 cache를 그대로 재사용하며 새 API 복제본이나 spike 소스 이동을 만들지 않는다.

## 8개 검수 관문

1. **공개 API**: `product/sdk/index.ts`는 기존 prototype SDK가 노출하는 `connect`, `WriteUnsettled`, 그리고 generic SDK 타입만 공개한다. `connect`는 base URL, 운영 JWT access token, 생성된 binding, 선택적 fetch 구현을 공통 `connectTypedExtensions`에 전달한다.
2. **동일 구현 재사용**: 빌드 입력은 제품 entry와 기존 spike의 TypeScript 의존 closure다. SDK 로직을 복사하거나 spike 원본을 이동·수정하지 않는다.
3. **완전한 산출물**: 로컬 TypeScript compiler로 JavaScript와 declaration을 함께 만들고, package에 필요한 runtime/type 의존 closure를 모두 담는다. 산출물의 상대 import가 package 밖으로 나가지 않는다.
4. **설치 가능한 패키지 메타데이터**: tarball은 이름 `@aip/sdk`, 버전 `0.1.0`, ESM 형식이며 root `exports`가 JavaScript와 TypeScript declaration을 가리킨다. 빌드는 `npm publish`를 실행하지 않는다.
5. **오프라인 소비자**: `npm pack --offline` 후 별도 소비자 디렉터리에서 `npm install --offline`이 가능하고, 설치된 root export를 Node가 import할 수 있다.
6. **운영 인증과 전송 통합**: ephemeral RSA key로 만든 JWT access token을 설치 SDK가 전달하고 서비스가 검증한다. 테스트는 세션 확인, typed read, 두 번째 read의 공통 cache hit, typed write, write 뒤 cache 무효화, 멱등 키 재생을 실제 service endpoint로 확인한다.
7. **엄격한 타입 공개**: 설치된 package declaration으로 NodeNext strict consumer가 통과하고, 계약에 없는 field/enum value를 사용하면 음성 대조 typecheck가 실패한다.
8. **출력 소유권과 실패 처리**: `--out <fresh-directory>`만 허용한다. 기존 디렉터리·파일·symlink를 거부하고 내용을 보존한다. 빌드 중 실패하면 이번 실행이 만든 출력 디렉터리만 정리한다.

운영 통합 확인은 임시 owned PostgreSQL schema와 임시 JWKS 파일을 사용한다. principal 폐기 뒤 같은 JWT가 `/session`에서 거부되는지 확인하고, 테스트가 초기화한 schema만 정리한다. JWT 개인키는 메모리에서 생성·사용하고 출력하거나 파일로 저장하지 않는다.

## 실행

저장소 루트 `aip/`에서 다음과 같이 실행한다.

```sh
npm ci --prefix spikes/spike-0-ts --ignore-scripts --no-audit --no-fund
node product/tools/build-sdk.mjs --out /tmp/aip-sdk-0.1.0
node --test product/tests/sdk-package.test.mjs
node --test product/tests/package-boundaries.test.mjs
cargo build -p aip-cli --locked
node --test product/tests/service-smoke.test.mjs
node product/tools/package.mjs
```

첫 명령은 lockfile에 기록된 로컬 compiler와 타입 의존성을 설치한다. SDK 소비자에게 이 빌드 의존성이 필요하지는 않다. Compiler가 없거나 실행할 수 없으면 builder는 출력 디렉터리를 만들기 전에 `SDK_TOOLCHAIN_MISSING`과 설치 명령을 반환한다. 초기 의존성 설치에는 registry 또는 준비된 npm cache가 필요하며, 설치된 도구를 사용하는 pack/install 검사는 offline으로 실행한다.

테스트는 임시 디렉터리에서 offline pack/install과 독립 소비자를 구성하고 종료 시 임시 파일을 제거한다. 빌드 결과물은 호출자가 소유하는 fresh output 디렉터리에만 기록된다.

`package.mjs`는 `product/dist/<UTC timestamp>/`를 새로 만들며 root `target/debug/aip`, 설치 가능한 SDK tarball, 비밀 없는 설정 예시, 정본 app/Node/Python extension 예제와 artifact/source SHA-256 manifest를 담는다. `--out <fresh-directory>`로 별도 위치를 지정할 수 있다. artifact README에는 worker 기본 off, `Event.confirm` opt-in, 현재 macOS `MacNetDeny` 제약을 적는다.

패키징은 시작 시 소스 목록과 해시, 기존 실행 파일의 해시를 기록한다. SDK 빌드와 파일 복사 뒤 입력을 다시 확인하고, 복사한 실행 파일·예시의 해시도 비교한다. 입력 추가·삭제·수정이나 실행 파일 교체를 감지하면 변경 경로와 `SOURCE_CHANGED`를 반환하고 이번 실행의 출력을 정리한다. 필수 소스나 Cargo manifest가 없으면 SDK 빌드 전에 `SOURCE_MISSING`을 반환한다. `package-boundaries.test.mjs`는 독립 임시 저장소에서 이 경계를 재현하며 공용 DB와 `target/`를 사용하지 않는다.

이 검사는 workspace 잠금이나 빌드 provenance를 제공하지 않는다. 검사 사이에서 변경됐다가 원복된 소스는 감지하지 못할 수 있으며, 기존 실행 파일이 현재 소스에서 빌드됐다는 보증은 별도 검증이 필요하다.

## resource 독립 operation

생성 binding에 operation이 있으면 같은 `connect`가 `.operation(name, input)`을 제공한다. `OperationBinding`과 `OperationDescriptors`를 package root에서 export한다. 입력·출력·범위·계약 지문과 세션 교체 검사를 공통 transport에 연결한다. 실제 문법·인증·DB 의존·실패 경계는 [EXECUTION](EXECUTION.md)을 따른다. 설치 SDK smoke는 Node/Python과 safe/decimal 조합에서 Unicode 계산과 범위 밖 입력 거부도 확인한다.

CLI나 compiler 변경 뒤 service smoke·패키징 전에 위 `cargo build`를 실행한다. package 도구는 기존 실행 파일을 복사하므로 SDK 소스만 최신이라고 해서 생성 계약도 최신인 것은 아니다.
