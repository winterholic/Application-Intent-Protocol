# 앱 패키지 설치와 변경배포

`package.mjs --manifest`로 생성한 패키지에는 독립 실행 도구 `tools/app-deploy.mjs`가 들어 있다. 대상 서버에는 패키지와 Node.js가 필요하다. Rust·npm 빌드 도구·원본 checkout은 필요하지 않다. 현재 지원하는 네이티브 target은 macOS arm64/x64와 Linux GNU arm64/x64이며, 실제 빌드 target과 서버 OS/CPU가 일치해야 한다.

## 설치

```sh
# 신뢰한 경로에서 받은 패키지를 새 디렉터리에 설치한다.
node /incoming/bundle/tools/app-deploy.mjs install --out /srv/my-app/1.0.0
node /srv/my-app/1.0.0/tools/app-deploy.mjs check
```

체크섬 전체와 실제 파일 목록을 대조하고 symlink·특수 파일·경로 탈출·기존 출력 덮어쓰기를 거부한다. 복사 후에도 원본 digest와 대조한다. 실패하면 이번 명령이 만든 설치 디렉터리만 회수한다. 체크섬은 무결성 확인용이며 서명이나 배포자 신원 인증을 대신하지 않는다. 실행 전에 신뢰한 배포 채널에서 패키지를 받아야 한다.

설치 디렉터리는 불변으로 유지한다. 운영 설정·JWKS·로그·의존성 환경·업로드 파일은 밖에 둔다. 패키지 안에 임의 파일이나 `node_modules`를 추가하면 무결성 검사에서 거부된다. 고정 명세에 필요한 의존성은 운영자가 별도 환경에 설치하고 worker의 `runtime_executable`, `read_only_paths`, 허용 환경으로 제공해야 한다. 이 도구는 패키지 관리자의 설치 스크립트를 자동 실행하지 않는다. 환경 준비는 사용하는 확장의 import 방식과 OS sandbox 경계까지 검증해야 한다.

## 설정과 사전 검사

외부 설정 JSON은 기존 [service 설정](../crates/aip-service/README.md)을 따른다. 예를 들어 `/srv/my-app/operator/service.json`:

```json
{
  "schema": "aip_my_app",
  "database_url_env": "APP_RUNTIME_DATABASE_URL",
  "migrate_database_url_env": "APP_MIGRATION_DATABASE_URL",
  "listen": "127.0.0.1:8080",
  "wire": "decimal",
  "allowed_origins": ["https://app.example.com"],
  "auth": {
    "issuer": "https://issuer.example",
    "audience": "aip-api",
    "jwks": { "kind": "file", "path": "jwks.json" }
  }
}
```

DSN은 위 두 환경 변수로 주입한다. 내장 DB 주소나 기본 schema를 선택하지 않는다. migration 역할과 runtime 역할의 실제 권한·접속 문자열은 운영자가 기존 최소 권한 절차로 준비한다. `init/plan/migrate`는 migration 환경 변수(없으면 명시한 runtime 변수), `serve/preflight`는 runtime 변수를 검사한다. 설정에 개인키·credential URL·직접 password/secret/token 값을 넣으면 거부하고 값은 출력하지 않는다.

worker가 있는 앱은 `workers.directory`를 **설치한 패키지의 `app/<extensions.directory>`**로 지정한다. 상대 경로는 운영 설정 파일 기준이다. 언어와 `enable_writes`가 manifest와 일치해야 한다. Linux는 `auto` 또는 `linux_bubblewrap`, macOS는 `auto` 또는 `mac_sandbox`만 허용한다. 사전 검사는 runtime 실행과 격리 도구 존재를 확인한다. 실제 sandbox 실행·의존성 import·DB/JWKS·schema fence는 제품 `serve`가 최종 검증한다. `preflight` 성공만으로 기동 성공을 주장하지 않는다.

```sh
node /srv/my-app/1.0.0/tools/app-deploy.mjs preflight --config /srv/my-app/operator/service.json
```

## 초기화·기동·종료

```sh
# 빈 schema 최초 1회만. 이미 있는 schema는 기존 엔진이 거부한다.
node /srv/my-app/1.0.0/tools/app-deploy.mjs init --config /srv/my-app/operator/service.json

# foreground. systemd 등의 서비스 관리자가 이 명령을 소유한다.
node /srv/my-app/1.0.0/tools/app-deploy.mjs serve --config /srv/my-app/operator/service.json

# 별도 프로세스에서 확인. HTTP 200만 성공이며 redirect는 따라가지 않는다.
node /srv/my-app/1.0.0/tools/app-deploy.mjs health --config /srv/my-app/operator/service.json
node /srv/my-app/1.0.0/tools/app-deploy.mjs ready --config /srv/my-app/operator/service.json
```

종료는 서비스 관리자가 supervisor에 SIGTERM을 보낸다. supervisor는 엔진에 전달하고 종료를 기다린다. 재기동은 종료 완료 후 같은 `serve`를 다시 실행한다. PID 파일로 다른 프로세스를 찾아 죽이거나 자동 백그라운드 daemon을 만들지 않는다. 사용자의 macOS backend를 재시작하는 명령은 아니다. 실제 운영 HTTPS·Origin·프로세스 관리자·재시작 정책은 서버 운영 설정에 속한다.

## 변경배포

```sh
node /incoming/new-bundle/tools/app-deploy.mjs install --out /srv/my-app/1.1.0
node /srv/my-app/1.1.0/tools/app-deploy.mjs plan --config /srv/my-app/operator/service.json

# 출력 JSON의 output 안에 제품 엔진의 원래 계획·digest가 있다.
# 계획과 백업을 검토하고 기존 서비스를 종료한 다음 정확한 digest만 적용한다.
node /srv/my-app/1.1.0/tools/app-deploy.mjs migrate --config /srv/my-app/operator/service.json --digest <reviewed-64-hex-digest>
node /srv/my-app/1.1.0/tools/app-deploy.mjs serve --config /srv/my-app/operator/service.json
node /srv/my-app/1.1.0/tools/app-deploy.mjs ready --config /srv/my-app/operator/service.json
```

`--migration /absolute/path/to/migration.json`은 `plan`과 `migrate`에 동일하게 지정할 수 있다. digest를 자동 승인하거나 파괴 DDL·principal·migration journal·schema fence를 우회하지 않는다. worker 경로를 쓰는 설정은 새 패키지의 경로로 갱신해야 한다. migration 실패 시 새 버전을 기동하지 않고 제품 DB 상태와 계획을 확인한다. 오류 출력은 비밀 보호를 위해 상세 subprocess 원문을 생략한다. 같은 설정으로 제품 명령을 직접 실행하면 엔진 진단을 볼 수 있다.

이전 디렉터리로 실행 파일을 바꾸는 것은 DB 롤백이 아니다. schema fence가 구버전을 거부할 수 있다. 운영자는 변경 전 애플리케이션 및 관련 인프라 schema의 일관된 백업과 복원 절차를 준비해야 한다. 복원 이후 쓰기는 소실될 수 있고 외부 효과는 DB 복원으로 취소되지 않는다. 무중단 전환·원격 서버 배포·자동 백업복원·OS 보안 설정 변경은 이 명령이 수행하지 않는다.

## 검증

```sh
node --test product/tests/app-deploy.test.mjs
node --test product/tests/app-package-e2e.test.mjs
# Docker가 필요하며 자신의 PostgreSQL 컨테이너만 만들고 회수한다.
node --test product/tests/app-deploy-live.test.mjs
```

live 검사는 원본 입력과 중간 번들을 지운 뒤 설치 디렉터리에서 init, 인증 HTTP read/apply, SIGTERM 종료·재기동, 잘못된 digest 거부, 새 nullable 필드 migration, 데이터·멱등 기록 보존, 구버전 정의의 fence 거부, 변경 전 PostgreSQL dump 복원 후 이전 패키지의 조회·멱등 기록을 확인한다. 이는 일회용 PostgreSQL에서의 제품 통합 검증이며 운영 서버 배포 증거가 아니다.

Linux에서 `AIP_TEST_APP_DOCKER=1 AIP_TEST_APP_WORKERS=1`을 설정하면 [Docker 이미지 실행](DOCKER.md)과 Node/Python의 설치 SDK·실제 HTTP·격리 worker·durable job을 두 Id wire로 추가 검증한다. worker 검사는 패키지의 정의·확장·binary·SDK를 사용하며 원본 앱 입력은 지워져 있다. 검증 harness와 TypeScript compiler·Python SDK 시험 helper는 checkout에 남아 있으므로 서버뿐 아니라 검사 도구도 checkout 없이 동작한다는 주장은 아니다. macOS에서는 두 Linux 전용 단계가 명시적으로 `NOT RUN`이다.
