# 제품 통합 검증 (2026-10-06)

운영 인증·일반 마이그레이션·root 제품 통합을 사용자 요청 범위로 구현했다. AIP의 caller read/direct apply와 서버의 최종 권한, 같은 정의·같은 SDK·Node/Python 확장을 유지한다. 앱별 endpoint나 별도 실행 엔진을 추가하지 않았다.

## 백엔드 기능 포괄성 보완 회차 (2026-10-13)

job의 업무 데이터 접근, schedule·webhook·outbox trigger, health probe를 추가한 회차의 기록이다. macOS, 로컬 PostgreSQL에서 실행했다. 같은 시간에 다른 작업자가 같은 PostgreSQL에서 검증을 돌렸을 수 있다. 첫 v6 실행에서 `extension_lifetime`의 worker PID 기록 시간 초과가 한 번 났고, 단독 재실행 3회와 이후 전체 실행 2회에서는 통과했다. 부하에 민감한 기존 타이밍 테스트로 보고 기록만 남긴다.

| 방금 실행한 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true bash tools/verify.sh` | Rust 599 passed, 2 ignored, 0 failed. Node 105 passed, 0 failed | root workspace(새 cron·서명 단위 테스트 포함 안 됨), V1/V2/V3/V5 suite, 제품 lint·fmt, SDK package |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked --target-dir target --no-fail-fast` | 52 passed, 0 failed | worker ctx 변경(`ctx.data.read`, `apply` key) 뒤 기존 Node/Python 회귀 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --no-fail-fast` | 114 passed, 0 failed | 신규 `job_data`, `triggers`, `outbox`, `health`, cron·서명 단위 테스트와 기존 transport 전체 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed | 재배치 binary·설치 SDK·두 wire·Node/Python worker |

신규 e2e가 실제 PostgreSQL에서 확인한 것: 행 정책으로 숨은 행에 대한 job 전이가 `MISSING_TARGET`, 선언 없는 전이는 job 실패, 같은 key 재호출과 commit 뒤 worker crash 재시도에서 전이를 다시 실행하지 않고 journal 재생(Node·Python). webhook 정상·재전송·위조·무서명·주체 비활성(503). 두 instance가 같은 schedule slot과 같은 outbox 행을 경쟁해도 job 하나. 배포 digest 변경 뒤 `/ready` 503. 독립 컨텍스트 검토가 지적한 서명 밖 header key, 서비스 주체 보존 quota, outbox head-of-line, DST 반복 시간, grace 하한, `/ready` pool 사용, outbox id 재사용 7건을 고친 뒤 위 명령을 다시 실행했다. 다른 작업자의 커밋(`826028f`까지) 위로 rebase한 뒤 같은 명령을 다시 실행해 verify Rust 635 passed·2 ignored·0 failed, Node 108 passed, v4 53 passed, v6 114 passed, smoke 1 passed를 확인했다. AWS·GCP·실제 provider webhook 전송, Linux worker에서의 신규 e2e는 실행하지 않았다.

## resource 독립 operation 회차 (2026-10-10)

이 절은 2026-10-10 resource 독립 동기 operation 회차의 고정 검증 기록이다. 당시 기준 문법은 [EXECUTION](EXECUTION.md)에 적었다. 그 뒤 추가된 durable job, managed file/event, HTTP broker의 현재 계약은 [관리형 durable execution](MANAGED-EXECUTION.md)을 따른다. 이 절의 명령 결과를 새 변경의 실행 증거로 간주하지 않는다.

| 방금 실행한 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true bash tools/verify.sh` | Rust 487 passed, Node 72 passed, 실패·ignore·skip 0 | root workspace와 V1/V2/V3/V5 독립 suite, 제품 lint·fmt, SDK package, 마지막 CLI build. Rust 수치는 제품만의 테스트 수가 아님 |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked --target-dir target` | 34 passed, 0 failed/ignored | 공통 worker 호출 리팩터링 뒤 기존 Node/Python·framing·ctx·읽기/쓰기·기한·wire 회귀 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target` | 43 passed, 0 failed/ignored | 새 operation 네 조합, 기존 HTTP·인증·CORS·DB 기한·배포/WRITE 복구·세션 결과 보존 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed, 0 failed/skipped | 재배치 binary·설치 SDK strict 소비자, 실제 JWT·owned PG schema, Node/Python × safe/decimal operation·Unicode·범위 거부·타입 음성 대조 |
| root와 수정한 다섯 spike의 `cargo fmt … --check`, `bash -n tools/verify.sh`, `node --check product/tools/smoke.mjs`, 문서 상대 링크 검사 | exit 0 | 포맷·shell/JS 문법·로컬 문서 링크. 외부 링크 가용성 보장은 아님 |

먼저 재현한 실패는 실행 중 principal revoke 뒤 output 반환, safe 배포에 decimal 설정으로 큰 actor 등록, 안전 정수 초과 operation 입력 수락, SDK에서 표현할 수 없는 범위 metadata 수락이다. 수정 후 같은 음성 대조를 포함해 위 suite를 실행했다. revoke 테스트는 worker가 시작 파일을 쓴 것을 확인한 뒤 폐기해 수락 이전 거부와 구분한다. 기존 `principal` 이름의 실제 resource와 새 관리형 mode의 전환도 migration에서 차단한다.

첫 설치 smoke는 기존 CLI 실행 파일의 오래된 `dependencies: []` 생성 계약과 새 SDK 타입이 달라 실패했다. 현재 소스로 CLI를 build한 뒤 같은 pack/install/typecheck/runtime 검사가 통과했다. package 도구가 현재 source에서 binary를 빌드했다는 provenance를 보장하는 것은 아니므로 실행 순서를 SDK 문서에도 명시했다.

독립 Sol 검토의 실행 중 폐기·정수·배포 wire 지적을 부모가 실제 테스트로 재현해 보완했다. 마지막 재검토는 소스/문서 대조이며 별도 테스트 실행으로 합산하지 않는다. Luna는 legacy identity·패키징과 다음 Job의 PoC 재사용 경계를 읽기 전용으로 검토했다. 이번 회차 Claude Code의 새로운 독립 실행 검토는 미실행이며 과거 감사의 상호 검토 기록과 구분한다.

당시 미실행/미구현: Linux worker 격리, 동기 요청 disconnect의 프로세스 회수 실측, durable Job·파일·외부 효과·전체 tenant 경로·운영 provider 연결·부하/성능 측정. 업무 테이블 0개 실행은 운영 PostgreSQL이 없는 배포를 의미하지 않는다. durable 회차는 아래와 같이 별도 검사해야 하며, provider 실서비스 exactly-once나 Linux filesystem 격리로 확대 해석하지 않는다.

## 관리형 durable execution (2026-10-11)

현재 구현 계약은 [MANAGED-EXECUTION](MANAGED-EXECUTION.md)에 정리했다. 아래는 macOS arm64와 로컬 PostgreSQL에서 현재 source로 실행한 결과다. Cargo 명령에는 `CARGO_NET_OFFLINE=true`, `CARGO_TARGET_DIR=target`을 적용했다.

| 실행 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `bash tools/verify.sh` | exit 0, `AIP core verification passed` | workspace 테스트·제품 lint·SDK 검사·v1/v2/v3/v5 독립 suite·마지막 CLI build. 새 Job/file/event/HTTP store, durable principal, migration upgrade, SDK Job 회귀 포함 |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked` | exit 0, 실패 0 | 기존 worker 회귀와 Node/Python managed context, 선언 capability 및 안정 key 검사 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked` | exit 0, 실패 0 | 전체 HTTP transport 회귀와 새 durable 통합 시나리오 4개. 재기동·취소·owner 격리·HTTP 불확실성·중첩 예산·자식 실패 재생을 두 언어에서 검사 |
| `node product/tools/package.mjs --out /tmp/aip-managed-release-20261011-b` | `ok:true`, 파일 8개 | 현재 CLI, 설치 SDK tarball, Node/Python 예제와 source manifest 조립 |
| `node product/tools/smoke.mjs --bin <artifact>/bin/aip --sdk-package <artifact>/sdk/aip-sdk-0.1.0.tgz --workers node --extension-dir <artifact>/extensions` 및 `--workers python` | 두 명령 exit 0, 각 `ok:true` | 설치한 SDK strict 타입 검사와 실제 JWT/PG 실행. Node/Python × decimal/safe 네 조합에서 Job `SUCCEEDED`, Unicode 길이 3, 같은 key의 같은 id 재생. 기존 read/cache/write/revoke·동기 operation도 검사 |
| artifact에서 `shasum -a 256 -c SHA256SUMS`, source root에서 `shasum -a 256 -c <artifact>/SOURCE_SHA256SUMS` | artifact 8개 및 source 200개 일치 | 로컬 무결성. 공개 배포 서명이나 commit provenance 보장은 아님 |
| root 및 수정한 여섯 spike의 `cargo fmt … --check`, `bash -n tools/verify.sh`, `node --check product/tools/smoke.mjs`, 문서 상대 링크 검사 | exit 0, 문서 17개 중 누락 링크 0 | 포맷·문법·로컬 문서 연결 |

검증 중 순수 operation이 금지된 context 호출 오류를 잡고 성공을 반환할 수 있는 회귀를 기존 음성 테스트로 확인해 차단했다. 설치 소비자의 선택적 Job API와 smoke 결과 타입도 strict 검사에 맞췄다. 저장소 이동으로 드러난 v12 테스트의 개발자 절대 경로는 `CARGO_MANIFEST_DIR` 기준으로 바꾸고 해당 타입 테스트와 전체 v6 suite를 다시 실행했다. 이동 전 경로가 남은 로컬 crate 빌드 캐시는 재생성했다.

HTTP 통합 검사는 통제된 loopback provider fixture를 사용한다. 같은 안정 key 사이에 다른 capability 호출을 끼워도 provider mutation이 한 번만 전송되는 경우, 응답 유실 후 `EFFECT_UNKNOWN`, 전송 중 취소 후 `CANCELLATION_UNCONFIRMED`를 확인했다. 이 결과는 실제 운영 provider의 exactly-once 보장이 아니다. 실시간 경로는 cursor polling이며 SSE/WebSocket은 미구현이다. 외부 IdP·운영 provider·대용량 object storage·Linux worker 격리·부하 및 성능 측정은 미실행이다.

## 첫 공개 뒤 반복 검증 회차 (2026-10-06)

이 회차는 기존 구현의 재검증과 입력 경계·빌드 안내 보완이다. caller 표현이나 서버 권한, 최종 문법·wire 선택을 변경하지 않는다. 이전 실행 묶음은 당시 source 기록이며 이번 source 변경을 포함하지 않는다.

| 명령/실행 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true bash tools/verify.sh` | Rust 230 passed, Node 53 passed, 실패·skip 0 | 전체 workspace 테스트, SDK cache/session/pending/contract 단위 테스트, offline 설치 SDK, 전체 fmt, 실행 코드와 제품 3 crate lint, 마지막 제품 build |
| `cargo test -p aip-cli --test service_boundaries --locked --offline` | 5 passed, 0 failed | FIFO를 정의·설정·migration·DB CA로 넣었을 때 유한 시간 내 오류. 크기 제한·origin·출력 원본 보호 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed, 0 failed | 재배치 binary·설치 SDK, 두 wire, 실제 JWT/PG read/write/replay/revoke, Node/Python 확장 |
| node_modules가 없는 임시 checkout에서 `npm ci --prefix spikes/spike-0-ts --offline --ignore-scripts --no-audit --no-fund` 후 SDK build | 두 명령 exit 0 | 기존 설치 디렉터리를 공유하지 않고 lockfile과 npm cache로 compiler·타입 의존성을 준비 |
| `actionlint .github/workflows/verify.yml`, `bash -n tools/verify.sh` | exit 0 | workflow 문법과 shell 문법 |
| SDK compiler가 없는 임시 checkout에서 verify script 실행 | exit 1, 설치 명령 출력 | 검사 누락을 성공으로 보고하지 않는 음성 대조 |
| 각 독립 `Cargo.toml`의 `cargo test --manifest-path … --locked --offline` | V1 43·V2 33·V3 5·V4 32·V5 22·V6 40·V7 1·V11 9·prototype 37, 실패·ignore 0 | 정의 상한, 실제 SQL 정책, 쓰기·worker 원자성, 요청/DB/worker 기한, 응답 유실 복구, CORS·두 wire·Node/Python 확장 |
| [GitHub Linux CI #37482679158](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/37482679158), commit `06ff8d8` | success、Rust 230 passed、Node 53 passed、실패·skip 0 | Ubuntu x86_64, Rust 1.99.0, Node 26, PostgreSQL 17, 새 checkout 의존성 설치와 core 검증 |

FIFO 회귀는 수정 전 `waited for a FIFO writer`로 실패했다. SDK 회귀는 수정 전 `BUILD_FAILED`와 기대한 `SDK_TOOLCHAIN_MISSING`이 달라 실패했다. 두 반례를 확인한 뒤 해당 동작만 보완했다. 별도 9개 파일의 포맷 변경은 변경 전 source를 rustfmt한 결과와 바이트 단위로 대조했다.

V7의 최초 `--locked` 실행은 오래된 lockfile 때문에 테스트 시작 전에 실패했다. 현재 V2 읽기 엔진의 TLS 의존성 39개를 기록하되 기존 locked 패키지 버전은 제거·교체하지 않았다. 갱신 뒤 V7의 `--locked` 검사는 1 passed다. 이 회차 Rust 합계 452개는 workspace 230개와 독립 경로 222개의 합계이며, 제품 서비스 452개로 표현하지 않는다. Node 53개와 제품 smoke 1개도 서로 다른 검증 범위다.

Root `cargo clippy --workspace --all-targets --offline -- -D warnings`는 기존 CLI 통합 테스트의 `unwrap_used` 진단으로 실패했다. 실행 코드 `--lib --bins`와 제품 3 crate `--all-targets` 검사는 exit 0이다. 전체 테스트 lint 통과로 확대하지 않는다. Linux CI는 위 실제 실행 결과로 확인했으며 macOS worker 및 외부 운영 배포 검증을 포함하지 않는다.

보완 목록과 자동 검증의 경계는 [VALIDATION](VALIDATION.md)을 따른다. 아래 표와 bundle 해시는 이전 제품 조립 회차의 기록이다.

## 현재 제품 경로의 실제 검증

| 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `cargo test -p aip-auth -p aip-migrate -p aip-service --offline` | 39 passed, 0 failed | JWT/JWKS/TLS·actor 연결, DDL 계획·변환·backfill·rollback·동시 적용·drift·wire 원자성·이관 재시도 |
| `cargo test -p aip-cli --test service --test service_boundaries --test service_flow --test adoption_flow --offline` | 8 passed, 0 failed | 실제 JWT read/apply·재기동·live 정책 변경 거부·토큰 폐기와 재활성화·기존 서버 쓰기 결과 이관·설정 및 출력 보호 |
| `node --test product/tests/sdk-package.test.mjs product/tests/service-smoke.test.mjs` | 5 passed, 0 failed | offline SDK 설치·strict 타입 음성 대조·JWT·캐시·쓰기·멱등 재생·공백 경로 재배치·Node/Python 확장 |
| `cargo clippy -p aip-auth -p aip-migrate -p aip-service --all-targets --offline -- -D warnings` | exit 0 | 제품 조립 crate·인증·마이그레이션과 해당 테스트 경고 없음 |
| `cargo build -p aip-cli --offline` | exit 0 | root CLI 실행 파일 생성 |

재배치 확인은 [최종 묶음](dist/2026-10-06T11-19-25-091Z/README.md)의 binary·SDK tarball·확장 파일을 저장소 밖 공백 경로에 복사해 실행했다. 기본 설정·Node·Python 각각 decimal/safe 두 wire에서 실제 JWT/PG 흐름을 실행한다. SDK 읽기·캐시 hit·쓰기 후 무효화·같은 키 재생·인증 철회를 확인한다. 확장 검사는 이미 적용한 Event에 `Event.confirm`을 호출해 count 0과 같은 키 재생을 확인한다. 새 행 변경과 확장 rollback의 엔진 검증은 아래 기존 worker/전송 회귀 범위와 구분한다.

묶음의 SHA256SUMS 8개 파일과 SOURCE_SHA256SUMS 181개 source 입력을 메인이 다시 비교했으며 mismatch 0이다. 마지막 `cargo build`의 binary와 묶음 binary도 바이트 단위로 일치한다. Cargo 테스트가 다른 dependency feature 조합의 실행 파일을 target 경로에 복원할 수 있어, 최종 build를 테스트 뒤 실행하고 묶음을 새로 생성했다.

## 함께 확인한 기존 엔진 회귀

2026-10-05 이어서 작업 중 `cargo test --manifest-path <각 Cargo.toml> --offline`을 실행했다. V1 43·V2 33·V3 5·V4 32·V5 22·V11 9·V6 40·prototype 37개가 각각 실패 0이다. 기존 root CLI의 contract_golden/golden/codes/diff 19개도 실패 0이다. 이 숫자는 현재 제품 47개에 합쳐 제품 표면 전체를 보증하는 수치로 사용하지 않는다.

원격 DB TLS 연결의 오류 타입 변경에 맞춰 기존 V4/V6 테스트의 진단 분기를 갱신했다. 운영 JWT의 4KiB 한도에 맞춰 V6 기존 Bearer 경계 테스트를 공개 MAX_TOKEN+1로 바꿨다. 실제 오류·경계 검사 자체를 제거하지 않았다. V1/V2/V6과 제품 crate의 수정 파일 포맷을 확인했다. root CLI 전체 fmt 검사는 이번 작업에서 수정하지 않은 encrypt_e2e.rs의 기존 포맷 차이를 보고하므로, root CLI는 이번 수정 파일을 직접 rustfmt 검사했다.

## 독립 검토와 제한

Sol high가 별도 문맥에서 운영 중 구 정책 실행, 폐기 토큰 재활성화, 없는 신원 폐기 성공, 이관 멱등 namespace 누락, wire 고정의 별도 트랜잭션 문제를 발견했다. 메인은 실제 RED/GREEN과 코드로 대조했다. 최종 Sol 검토는 같은 SQL 연결의 배포 잠금·wire 포함 승인 지문·원자적 metadata 기록·폐기 cutoff를 읽고 추가 필수 결함을 확인하지 못했다. 마지막 독립 검토에서 DB 테스트를 재실행한 것은 아니다.

검증 환경은 현재 macOS arm64, 로컬 PostgreSQL, 통제된 HTTPS 인증서 fixture와 메모리 생성 RSA 키다. 외부 실제 IdP 계정·TLS reverse proxy 배포·다른 OS·대형 테이블 무중단 변경은 미실행이다. actor resource 교체·임의 SQL 변환·자동 역방향 migration·구버전 client 공존은 지원하지 않는다. 최종 작성 형식·Id 표현·쓰기 조합·라이선스의 미결정을 승인으로 승격하지 않는다.

실행 절차는 [OPERATIONS](OPERATIONS.md), 설계 관문은 [서비스](../crates/aip-service/README.md)·[인증](../crates/aip-auth/README.md)·[마이그레이션](../crates/aip-migrate/README.md)·[정의 한계](../spikes/spike-v1-fixture/PRODUCTION-LIMITS.md)·[정책 실행 한계](../spikes/spike-v2-read/PRODUCTION-RUNTIME.md)를 따른다.
