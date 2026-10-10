# 반복 검증과 보완 목록

이 목록은 현재 코드의 검증 범위와 다음 조사 순서를 관리한다. 테스트 수가 전체 안전성이나 운영 준비를 보증하지는 않는다. 변경마다 재현 또는 음성 대조를 먼저 확인하고, 좁은 회귀 테스트와 관련 통합 검증 뒤 커밋한다.

## 새 checkout 준비

Rust/Cargo와 rustfmt·clippy, Node.js 26, npm, Python 3, PostgreSQL client/server가 필요하다. 현재 검증은 Rust 1.98.1과 PostgreSQL 17에서 실행했다. Python 3은 호스트 문자열의 실제 의미와 정적 추출 결과를 대조하는 core 검사에도 사용한다. 아래 명령은 저장소 루트에서 실행한다.

```sh
npm ci --prefix spikes/spike-0-ts --ignore-scripts --no-audit --no-fund
psql -X -d 'host=localhost dbname=postgres' -Atc 'SELECT 1'
bash tools/verify.sh
```

테스트는 현재 OS 사용자로 로컬 PostgreSQL에 접속한다. DB와 schema 생성 권한이 필요하다. 기존 PoC E2E는 고정 이름의 `aip_*` 테스트 DB를 초기화하므로 전용 개발·CI PostgreSQL에서 실행한다. 제품 경로 테스트는 임시 owned schema를 사용한다. 같은 PostgreSQL에서 검증 명령을 동시에 여러 번 실행하지 않는다.

`tools/verify.sh`는 누락된 도구·compiler·DB 연결 또는 검사 실패 시 즉시 실패한다. 의존성이 준비된 로컬 환경에서는 `CARGO_NET_OFFLINE=true bash tools/verify.sh`로 Cargo의 network 사용을 막을 수 있다.

## 자동 검사 범위

| 검사 | 확인하는 것 | 별도 확인이 필요한 것 |
|---|---|---|
| `cargo fmt --all --check` | root workspace 포맷 | workspace 밖 spike·prototype 포맷 |
| workspace `clippy --lib --bins -D warnings` | 제품 및 PoC 실행 코드 | CLI 통합 테스트의 `unwrap_used` 진단 정리 |
| 제품 3 crate `clippy --all-targets` | 인증·migration·service와 그 테스트 | root CLI 전체 all-targets 검사 |
| `cargo test --workspace --locked` | compiler, PoC E2E, JWT, migration/adoption, 제품 CLI | workspace에서 제외된 엔진의 독립 테스트 |
| 제품 엔진 v1·v2·v3·v5의 독립 Cargo suite | 다섯 작성 형식, 스칼라·SQL·읽기·전이·정원 경쟁·생성 SDK 타입 | worker·HTTP transport의 전체 독립 suite |
| 선택한 SDK/transport Node 단위 테스트 | cache TTL·무효화, 세션 교체, pending 복구, 계약/Id 응답 검사 | 실제 HTTP transport E2E, 브라우저 |
| 설치 SDK 테스트 | offline pack/install, declaration closure, strict 타입 음성 대조, 출력 보호 | 실제 IdP·배포 환경 |
| 마지막 CLI build | 테스트 feature 조합 뒤 제품 실행 파일 복원 | 실행 파일 재배치와 worker 실행 |

`.github/workflows/verify.yml`은 push/PR마다 같은 core 검증을 Ubuntu와 폐기 가능한 PostgreSQL 17 fixture에서 실행한다. macOS worker 테스트를 Linux 통과로 대신하지 않는다. GitHub Actions 구성의 근거는 [PostgreSQL service container 공식 문서](https://docs.github.com/en/actions/tutorials/use-containerized-services/create-postgresql-service-containers)다.

## 제품 실행 및 엔진 회귀

```sh
node --test product/tests/service-smoke.test.mjs
cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked
```

첫 명령은 재배치한 binary와 설치한 SDK로 decimal/safe 두 wire를 검사하고 Node/Python worker도 실행한다. worker 단계는 현재 macOS `sandbox-exec` 환경이 필요하다. Core 검증 뒤에 실행해야 마지막 build의 제품 binary를 사용한다.

v1·v2·v3·v5의 전체 독립 suite는 `tools/verify.sh`에서도 실행한다. root target 디렉터리를 공유해 이미 빌드한 의존성을 재사용한다. 그 밖의 `spikes/spike-v4-worker`, `spike-v7-migrate`, `spike-v11-dev-checks`의 Cargo.toml에도 같은 독립 테스트 명령을 적용할 수 있다. 이 세 엔진과 v6의 전체 suite는 자동 core 검증과 별도다.

## 2026-10-06 보완

- CLI가 FIFO를 열며 멈추는 반례를 재현했다. Unix에서 nonblocking open 후 실제 descriptor의 일반 파일 여부를 검사한다. 정의·설정·migration·DB CA 입력에 회귀 검사를 연결했다. 정상 파일, 크기 상한과 생성 출력 원본 보호도 같은 테스트 모음에서 확인한다.
- Compiler가 없는 checkout에서 SDK builder가 일반 `BUILD_FAILED`만 반환하던 반례를 재현했다. 이제 설치 명령과 구체 오류를 반환하고 출력 부모 디렉터리도 만들지 않는다. 기존 node_modules가 없는 임시 checkout에서 lockfile 설치와 SDK build를 별도로 실행했다.
- 기존 root workspace의 포맷 차이를 정리하고 반복 검증 명령과 CI를 추가했다. 실제 결과는 [VERIFICATION](VERIFICATION.md)에 별도 회차로 남긴다.
- V7 migration의 lockfile이 현재 읽기 엔진의 TLS 의존성과 맞지 않아 `--locked`가 검증 시작 전에 실패했다. 기존 패키지 버전을 유지하고 빠진 의존성을 동기화한 뒤 같은 `--locked` 명령으로 재검증했다.

## 다음 조사 순서

| 순서 | 영역 | 현재 근거와 다음 검증 |
|---|---|---|
| 1 | 깨끗한 Linux checkout | [첫 CI](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/37482679158)는 core Rust 230·Node 53개 통과. workspace 밖 엔진의 Linux 회귀와 worker 격리 지원은 별도 확인 |
| 2 | HTTPS JWKS rotation·장애·동시성 | 파일 rotation과 HTTPS loader는 기존 테스트가 있으나 remote cache 갱신 전체 흐름은 별도 검증 필요. 새 kid, 같은 kid 교체, unknown-kid 연속 요청, timeout 뒤 복구를 검사 |
| 3 | 실제 TLS reverse proxy | 현재 loopback와 HTTPS fixture까지만 검증. proxy 뒤 origin·header·body limit·종료 중 요청을 확인 |
| 4 | migration 장애 복구 | rollback·동시 적용·재시도 테스트는 있음. 실제 backend/process 종료 및 커밋 응답 유실 시 journal/catalog/data 비교를 추가 |
| 5 | 부하와 자원 회수 | 요청/DB/worker 기한 테스트는 있음. 혼합 부하·느린 클라이언트·취소 누적에서 연결·프로세스·메모리 회수를 측정 |
| 6 | SDK 및 브라우저 | 현재 설치·단위·로컬 smoke와 과거 prototype Chrome 기록을 구분. 제품 서비스의 실제 브라우저 CORS·세션 갱신·pending 복구를 재실행 |
| 7 | 전체 테스트 lint | root `clippy --all-targets -D warnings`는 CLI 통합 테스트의 기존 `unwrap_used`로 실패. 메시지가 유용한 실패 진단으로 보완하고 전체 검사를 연결 |
| 8 | 배포 메타데이터 | 현재 LICENSE는 MIT, Cargo workspace는 UNLICENSED로 불일치. 공개 배포 메타데이터 정합성을 별도 확인 |

외부 IdP 계정·운영 proxy·다른 OS worker 격리·대형 테이블 운영 변경은 이 로컬 검증으로 확인하지 않았다. 최종 문법·Id wire·쓰기 조합의 Open 결정을 테스트 결과만으로 확정하지 않는다.

## 2026-10-10 operation 회귀

현재 지원과 실제 예시는 [EXECUTION](EXECUTION.md)을 따른다. `tools/verify.sh`에 client `operations.test.ts`를 연결했다. V1의 다섯 작성 형식·실제 principal resource 호환·범위 metadata, migration의 관리형 신원·actorMode 전환 차단, CLI의 실제 JWT·worker 실행 중 revoke·배포 wire 불일치·정책 migration이 core 검사에 포함된다. CLI operation worker 회귀는 macOS에서만 실행하므로 Linux core가 이 worker 경계를 증명하지 않는다.

V6의 별도 `operations` 테스트는 Node/Python × safe/decimal에서 계약·입출력·Unicode 범위·정수 안전성·false allow·ctx 호출·deadline·다른 사용자·catalog를 검사한다. 설치 SDK smoke도 같은 네 조합에서 typed operation을 호출하고 범위 밖 입력을 거부한다. strict 소비자에는 잘못된 operation 이름/입력의 타입 음성 대조를 포함한다. worker·transport 전체 suite와 설치 smoke는 위와 같이 core 뒤 순서대로 실행한다.

아직 durable acceptance·HTTP disconnect 뒤 실행/정리·crash/reclaim·Job 취소·파일 ACL·tenant 전 경로는 이 검사 범위가 아니다. [다음 Job 제안](../plan-docs/backend-capability/durable-job-proposal.md)의 별도 장애 실험으로 닫는다.
