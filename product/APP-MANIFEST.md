# AIP 앱 매니페스트 및 배포 산출물 계약 (App Manifest & Distribution Contract)

AIP 애플리케이션의 독립 빌드·패키징·배포를 위한 앱 매니페스트(`aip.app.json`) 규격과 배포 산출물(Distribution Artifact) 계약을 정의합니다.

---

## 1. 개요 및 설계 원약

AIP는 Spring의 Gradle/JAR/WAR 배포 모델처럼 임의의 AIP 앱을 선언 하나로 검증하고 패키징하여 안전하게 기동·운영하는 표준 경험을 제공합니다.
- **비밀 격리**: 매니페스트와 설정 템플릿에는 환경 변수 이름(`*_ENV`) 또는 공개 참조만 기록하며, 실제 credential·비밀번호·개인키(`.env`, `*.pem`)의 포함을 엄격히 금지합니다.
- **경계 안전**: 모든 파일 경로는 애플리케이션 루트 디렉터리 내부로 한정되며, 상위 경로 탈출(`..`)과 앱 외부를 가리키는 symlink를 전면 차단합니다.
- **의존성 고정**: Node 및 Python 확장에 필요한 의존성/lockfile만 명시적으로 수집하며, 전역 디렉터리의 무제한 재귀 복사를 금지합니다.
- **결정적 산출물 계약**: 검증된 매니페스트는 각 구성 파일의 역할과 SHA-256 해시를 포함하는 `AppContract`를 산출하여 후속 패키징(WBS 9.2) 및 런타임 기동(WBS 9.3)의 단일 신뢰 기준이 됩니다.

---

## 2. 매니페스트 규격 (`aip.app.json`)

매니페스트 파일은 UTF-8 인코딩의 JSON 형식이며, 허용되지 않은 알 수 없는 필드는 엄격히 거부(`deny_unknown_fields`)됩니다.

### 2.1 스키마 구조

```json
{
  "$schema": "https://aip.dev/schemas/app-manifest-v1.json",
  "name": "event-app",
  "version": "1.0.0",
  "description": "Event management service on AIP",
  "definition": "./app.aip",
  "extensions": {
    "directory": "./extensions",
    "language": "node",
    "entrypoints": ["event.mjs"],
    "enable_writes": true
  },
  "static_assets": [
    {
      "mount": "/public",
      "path": "./public"
    }
  ],
  "dependencies": {
    "node": {
      "package_json": "./package.json",
      "lockfile": "./package-lock.json"
    },
    "python": {
      "requirements": "./requirements.txt"
    }
  },
  "targets": [
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu"
  ],
  "features": [
    "realtime",
    "metrics"
  ],
  "config_template": "./config.example.json"
}
```

### 2.2 필드 정의

| 필드 | 필수 여부 | 형식 / 허용값 | 설명 |
| --- | --- | --- | --- |
| `name` | 필수 | 문자열 (`^[a-z0-9][a-z0-9_-]{0,62}$`) | 애플리케이션 고유 식별자 (소문자, 숫자, 하이픈, 언더스코어, 1~63자) |
| `version` | 필수 | 문자열 (SemVer `^\d+\.\d+\.\d+.*$`) | 애플리케이션 시맨틱 버전 (예: `1.0.0`) |
| `description` | 선택 | 문자열 (최대 500자) | 앱 설명 |
| `definition` | 필수 | 상대 경로 (`*.aip`) | AIP 제품 정의 파일 경로 (앱 루트 내부) |
| `extensions` | 선택 | 객체 | 비즈니스 로직 확장 worker 설정 |
| `extensions.directory` | 필수 (확장 시) | 상대 경로 | 확장 파일들이 위치한 디렉터리 경로 |
| `extensions.language` | 필수 (확장 시) | `"node"` \| `"python"` | 확장 worker 런타임 언어 |
| `extensions.entrypoints` | 필수 (확장 시) | 상대 경로 배열 (1개 이상) | 실행 가능한 진입점 파일 목록 (확장 디렉터리 기준 상대 경로) |
| `extensions.enable_writes` | 선택 | 불리언 (기본 `false`) | 확장 내 쓰기 트랜잭션 허용 여부 |
| `static_assets` | 선택 | 객체 배열 | 정적 파일 서빙 마운트 목록 |
| `static_assets[].mount` | 필수 | 문자열 (`/`로 시작) | HTTP 요청 경로 prefix (예: `/static`) |
| `static_assets[].path` | 필수 | 상대 경로 | 서빙할 디렉터리 또는 파일 경로 |
| `dependencies` | 선택 | 객체 | 확장 worker용 고정 의존성 명세 |
| `dependencies.node` | 선택 | 객체 | Node.js 의존성 명세 (`package_json`, `lockfile`) |
| `dependencies.python` | 선택 | 객체 | Python 의존성 명세 (`requirements` 또는 `lockfile`) |
| `targets` | 선택 | 문자열 배열 | 배포 지원 대상 OS/CPU 트리플 (예: `x86_64-unknown-linux-gnu`) |
| `features` | 선택 | 문자열 배열 | 앱이 요구하는 엔진 기능 (예: `realtime`, `outbox`, `rag`, `mysql`) |
| `config_template` | 선택 | 상대 경로 (`*.json`) | 배포 시 참조할 예시 설정 파일 (비밀 없는 템플릿) |

---

## 3. 입력 검증 및 보안 경계 규칙

매니페스트 검증기(`app-manifest.mjs`)는 다음 경계를 엄격히 검사하여 규칙 위반 시 즉시 구조화된 오류(`{ ok: false, code, stage, message }`)로 중단합니다.

1. **스키마 및 형식 검증 (`BAD_MANIFEST`)**:
   - 매니페스트 파일 부재: `MANIFEST_MISSING`
   - 유효하지 않은 JSON: `BAD_MANIFEST`
   - 필수 필드 누락 또는 알 수 없는 필드 존재: `BAD_MANIFEST`
   - `name` 또는 `version` 형식 위반: `BAD_MANIFEST`
2. **경로 탈출 차단 (`PATH_ESCAPES_APP`)**:
   - 절대 경로(`/...`) 또는 상위 디렉터리 탐색(`../`)으로 앱 루트 외부를 가리키는 모든 상대 경로는 거부됩니다.
3. **심볼릭 링크 경계 검사 (`SYMLINK_ESCAPES_APP`)**:
   - 매니페스트 파일 자체 또는 매니페스트가 참조하는 파일 또는 디렉터리가 앱 루트 외부를 가리키는 symlink인 경우 즉시 거부됩니다.
4. **참조 파일 실재성 (`*_MISSING`)**:
   - `definition`: 파일이 없으면 `DEFINITION_MISSING`, 확장자가 `.aip`가 아니면 `INVALID_DEFINITION_EXTENSION`.
   - `extensions`: 디렉터리가 없거나 entrypoint 파일이 없으면 `EXTENSION_MISSING`. 언어별 확장자 불일치(Node에 `.py` 진입점 지정 등)는 `INVALID_EXTENSION_LANGUAGE`.
   - `dependencies`: 지정된 `package.json`, lockfile, `requirements.txt` 부재 시 `DEPENDENCY_MISSING`.
   - `config_template`: 부재 시 `CONFIG_TEMPLATE_MISSING`.
   - `static_assets`: 소스 경로 부재 시 `STATIC_ASSET_MISSING`.
5. **허용 외 / 비밀 파일 혼입 방지 (`DISALLOWED_FILE`, `CREDENTIAL_LEAK`)**:
   - `.env`, `.git`, `node_modules`, `target` 등 비밀 파일이나 빌드 부산물을 참조하는 것은 거부됩니다.
   - 개인키 파일(`*.pem`, `*.key`, `id_rsa`) 참조는 `DISALLOWED_FILE`로 거부됩니다.
   - 경로 이름뿐만 아니라 심볼릭 링크 별칭의 실제 대상(canonical real target)의 앱 상대 경로 구성요소 전체에도 동일한 금지 규칙이 엄격히 적용됩니다.
   - `config_template` 내에 하드코딩된 비밀번호(`postgres://user:secret@...` 또는 `password`/`secret` 필드에 실제 값 포함)가 있으면 `CREDENTIAL_LEAK`로 거부됩니다. 환경 변수 이름(`database_url_env`)을 사용해야 합니다.
6. **고정 의존성 범위 한정**:
   - 지정된 의존성 명세 파일(`package.json`, `package-lock.json`, `requirements.txt`)만 번들에 포함 대상으로 등록하며, 빈 문자열이나 부적절한 falsy 값은 거부됩니다. 전역 디렉터리 무단 복사는 차단됩니다.
7. **출력 충돌 및 보존 (`OUTPUT_EXISTS`)**:
   - 패키징 출력 디렉터리가 이미 존재하고 비어있지 않으면 기존 파일 보존을 위해 `OUTPUT_EXISTS`로 즉시 중단합니다.
8. **정적 자산 파일 무결성 및 경계 (`STATIC_ASSET_MISSING`, `DISALLOWED_FILE`, `SYMLINK_ESCAPES_APP`)**:
   - `static_assets`에 지정된 파일 및 디렉터리 하위의 모든 파일은 유한·결정적으로 순회하여 `AppContract`의 `files` 목록(`role: "static_asset"`) 및 `contractDigest`에 반영됩니다. 동일 디렉터리를 가리키는 서로 다른 정상 별칭 경로는 모두 계약 `files`에 수집되며, 재귀 경로 조상(ancestry)을 참조하는 순환 심볼릭 링크는 무한 순환 방지를 위해 조상 재방문을 중단하는 유한 정책으로 결정적 종료를 보장합니다. 하위의 비밀·금지 파일(`.env` 등)이나 외부 symlink는 거부됩니다.

---

## 4. 배포 산출물 계약 (`AppContract`)

검증 통과 시 산출되는 정규화된 계약 데이터입니다:

```json
{
  "ok": true,
  "app": {
    "name": "event-app",
    "version": "1.0.0",
    "definition": "app.aip",
    "extensions": {
      "directory": "extensions",
      "language": "node",
      "entrypoints": ["event.mjs"],
      "enable_writes": true
    },
    "static_assets": [
      { "mount": "/public", "path": "public" }
    ],
    "dependencies": {
      "node": {
        "package_json": "package.json",
        "lockfile": "package-lock.json"
      }
    },
    "targets": ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"],
    "features": ["realtime", "metrics"],
    "config_template": "config.example.json"
  },
  "files": [
    { "role": "manifest", "path": "aip.app.json", "sha256": "..." },
    { "role": "definition", "path": "app.aip", "sha256": "..." },
    { "role": "extension", "path": "extensions/event.mjs", "sha256": "..." },
    { "role": "static_asset", "path": "public/index.html", "sha256": "..." },
    { "role": "config_template", "path": "config.example.json", "sha256": "..." },
    { "role": "dependency", "path": "package.json", "sha256": "..." },
    { "role": "lockfile", "path": "package-lock.json", "sha256": "..." }
  ],
  "contractDigest": "64_hex_sha256_of_contract_and_files",
  "packageLayout": {
    "bin": "bin/aip",
    "definition": "app.aip",
    "manifest": "aip.app.json",
    "config": "config.example.json",
    "extensions": "extensions/",
    "static": "static/",
    "sdk": "sdk/aip-sdk-0.1.0.tgz",
    "checksums": "SHA256SUMS",
    "appChecksums": "APP_SHA256SUMS"
  }
}
```

이 산출물 계약은 후속 WBS 9.2(패키징)에서 입력 무결성을 검증하고, WBS 9.3(기동)에서 서비스 구성 인자로 자동 바인딩됩니다.

---

## 5. CLI 제품 진입점 사용법

### 5.1 독립 매니페스트 검증기

```sh
node product/tools/app-manifest.mjs <app-directory-or-manifest.json>
```
- 성공 시: 검증된 `AppContract` JSON을 stdout으로 출력하고 exit code 0으로 종료.
- 실패 시: 구조화된 오류 JSON을 stderr로 출력하고 exit code 1로 종료.

### 5.2 패키징 도구 연결 (`package.mjs`)

```sh
# 매니페스트 검증만 실행 (검약 산출)
node product/tools/package.mjs --manifest <path/to/aip.app.json> --validate-only

# 기존 예제 패키징 (완전한 하위 호환성 유지)
node product/tools/package.mjs --out dist/bundle --profile release
```
