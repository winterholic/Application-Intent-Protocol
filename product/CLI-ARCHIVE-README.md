# AIP CLI 아카이브

`aip-<version>-<target>.tar.gz`를 푼 디렉터리다. 이 아카이브에는 `aip` 실행 파일과 첫 실행에 필요한 예시만 들어 있다. SDK 패키지와 체크섬 파일은 들어 있지 않다(아래 "들어 있지 않은 것").

## 파일

- `bin/aip`: `aip` CLI와 서비스 엔진. `./bin/aip --version`으로 버전을 확인한다.
- `example.aip`: `service check`로 검증할 수 있는 호출자 정의 예시
- `config.example.json`: issuer, schema, origin, JWKS 경로가 예시값인 비밀 없는 설정
- `extensions/event.mjs`, `extensions/event.py`: 정본 예제의 Node와 Python Event.confirm 구현
- `tools/dev-jwt.mjs`: 개발용 토큰 도구. 저장소 checkout 없이 `node tools/dev-jwt.mjs --dir ./dev-idp --subject user-42`로 개발용 JWKS와 access token을 만든다(Node.js 필요). **운영에 쓰지 않는다.**
- `INSTALL.md`: 설치 안내(플랫폼, SDK 설치 방법 포함)
- `COMPATIBILITY.md`: 버전 대응 표
- `LICENSE`: 라이선스

## 확인과 사용

`./bin/aip service check example.aip`로 정의를 확인하고, `./bin/aip service gen example.aip --wire decimal --out bindings.ts --sdk-import @aip/sdk`로 TypeScript binding을 생성할 수 있다. 배포 전에는 `config.example.json`의 schema, DB 환경 변수, issuer, audience, JWKS와 허용 origin을 운영 값으로 채운다.

기본 설정은 worker를 사용하지 않는다. `Event.confirm`을 활성화하려면 `workers.directory`를 `./extensions`로, `workers.language`를 `node` 또는 `python`으로, `workers.enable_writes`를 `true`로 설정한다. Node에는 Node.js, Python에는 Python 3가 필요하다. extension 기본 격리는 `auto`다. macOS는 `sandbox-exec` 기반 `MacSandbox`로 네트워크·파일 쓰기·사용자 디렉터리 읽기·프로세스 생성을 차단하며(네트워크만 차단하는 `MacNetDeny`도 선택 가능), Linux는 Bubblewrap으로 지정한 runtime/extension/dependency 경로를 읽기 전용으로 노출한다. 필요한 격리 도구가 없으면 실행을 거부한다. 신뢰한 extension source와 고정된 dependency를 사용한다.

## 들어 있지 않은 것

- TypeScript SDK 패키지: 같은 Release에서 따로 받거나 npm에서 설치한다. 방법은 `INSTALL.md`의 TypeScript SDK 절을 따른다.
- 체크섬: 아카이브 무결성은 아카이브를 받을 때 Release의 체크섬 파일과 `gh attestation verify`로 확인한다. 풀린 디렉터리 안에는 체크섬 파일이 없다.
- 소스 전체와 빌드 입력 해시.
