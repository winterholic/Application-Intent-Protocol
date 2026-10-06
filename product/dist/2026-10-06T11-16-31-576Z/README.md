# AIP product artifact

이 디렉터리는 로컬 제품 통합을 위한 실행 파일과 SDK 패키지입니다.

## 파일

- `bin/aip`: 패키징 시점의 저장소 `target/debug/aip`에서 복사한 로컬 빌드 실행 파일
- `sdk/aip-sdk-0.1.0.tgz`: 로컬 TypeScript compiler로 만든 설치 가능한 `@aip/sdk@0.1.0` tarball
- `example.aip`: 제품 service에서 검증할 수 있는 호출자 정의 예시
- `extensions/event.mjs`, `extensions/event.py`: 정본 예제의 Node와 Python Event.confirm 구현
- `config.example.json`: issuer, schema, origin, JWKS 경로가 예시값인 비밀 없는 설정
- `SHA256SUMS`: 배포 파일과 source manifest의 SHA-256 해시
- `SOURCE_SHA256SUMS`: 현재 workspace의 CLI·SDK·예제 source 입력 해시

## 확인과 사용

`shasum -a 256 -c SHA256SUMS`로 파일을 확인하고, 앱 프로젝트에서 `npm install <이 디렉터리>/sdk/aip-sdk-0.1.0.tgz`로 SDK를 설치합니다.

`./bin/aip service check example.aip`로 정의를 확인하고, `./bin/aip service gen example.aip --wire decimal --out bindings.ts --sdk-import @aip/sdk`로 TypeScript binding을 생성할 수 있습니다. 배포 전에는 예시 설정의 schema, DB 환경 변수, issuer, audience, JWKS와 허용 origin을 운영 값으로 채우십시오.

기본 설정은 worker를 사용하지 않습니다. `Event.confirm`을 활성화하려면 `workers.directory`를 `./extensions`로, `workers.language`를 `node` 또는 `python`으로, `workers.enable_writes`를 `true`로 설정합니다. Node에는 Node.js, Python에는 Python 3가 필요합니다. extension 실행은 현재 macOS의 `sandbox-exec` 기반 `MacNetDeny`에 한정되며 네트워크 접근만 차단합니다. 파일 격리는 제공하지 않으므로 신뢰한 로컬 extension source를 사용하십시오.

## 산출 범위

이 번들은 현재 workspace의 로컬 빌드에서 가져온 실행 파일과 SDK tarball, 예시 입력을 포함합니다. 소스 전체, 공개 registry 배포, 빌드 서명 또는 외부 provenance는 포함하지 않습니다. 해시는 파일 무결성 확인용입니다.
`SOURCE_SHA256SUMS`의 경로는 생성 시점의 source workspace에 상대적입니다. 이 artifact는 Git commit provenance를 주장하지 않습니다.
원본 workspace가 있으면 workspace root에서 `shasum -a 256 -c <artifact>/SOURCE_SHA256SUMS`로 source 입력을 확인할 수 있습니다.
