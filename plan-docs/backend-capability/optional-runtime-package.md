# 기존 PoC runtime을 선택적 패키지로 분리하는 설계

> 2026-10-13. [INFRASTRUCTURE의 선택적 패키지 구조](../../product/INFRASTRUCTURE.md#선택적-패키지-구조)에서 남긴 후속 후보다. 원칙과 계약은 바꾸지 않고 기본 빌드가 끌어오는 의존성만 줄인다.

## 현재 상태

`aip-cli`는 `aip-runtime`을 항상 의존한다. `aip-runtime`은 [README](../../README.md)가 "기존 PoC"로 구분한 경로(`aip run`, `aip migrate`, `aip rekey`, `aip token`, `aip diff --against deployed`)의 서버다. 제품 경로 `aip service`는 이 crate를 쓰지 않는다. 그런데 제품만 쓰는 사용자도 다음 의존성을 컴파일한다.

| 의존성 | 쓰는 곳 | 제품 경로 사용 |
| --- | --- | --- |
| `axum`(multipart, ws) | `aip-runtime/src/http.rs`, `subscribe.rs` | 없음 |
| `deadpool-postgres` | `aip-runtime/src/lib.rs`의 `pool`과 전 모듈 | 없음. 제품은 `tokio-postgres`와 자체 pool을 쓴다. |
| `chrono-tz` | `aip-runtime/src/schedule.rs` | 없음 |
| `aes-gcm` | `aip-runtime/src/crypto.rs` | 없음 |

`aip-cli`가 `aip_runtime`을 부르는 곳은 `Cmd::{Run, Migrate, Rekey, Token}`과 `Cmd::Diff`의 `--against deployed` 분기뿐이다. 이 명령은 모두 PoC 경로다. 제품의 마이그레이션은 `aip service migrate`이며 `aip-migrate` crate를 쓴다.

## 선택한 경계

crate 경계는 유지한다. `aip-runtime`은 같은 workspace member로 남고 내부 구조와 보안 작업자가 수정 중인 파일은 건드리지 않는다. 경계를 `aip-cli`의 Cargo feature로 둔다.

| 항목 | 결정 |
| --- | --- |
| feature 이름 | `poc-runtime` (`aip-cli`). `["dep:aip-runtime"]`이며 기본 꺼짐 |
| 기본 빌드 | `check`, `parse`, `core`, `ddl`, `ir`, `explain`, `contract`, `gen-ts`, `diff <파일>`, `diagnostics`, `service …`는 그대로 |
| feature 필요한 명령 | `run`, `migrate`, `rekey`, `token`, `diff --against deployed` |
| 꺼진 빌드에서 그 명령 | 명령은 도움말에 남기고 실행 시 `RUNTIME_FEATURE_DISABLED`와 빌드 명령을 출력하고 종료 코드 1 |
| 테스트 | 런타임을 쓰는 e2e는 `[[test]] required-features = ["poc-runtime"]` |
| 검증 | `tools/verify.sh`가 feature 없이 한 번, `--features poc-runtime`으로 CLI e2e를 한 번 실행 |

crate를 새로 쪼개지 않는 이유는 두 가지다. `aip-runtime`을 `engine`과 `http`로 쪼개면 `exec`, `schema`, `outbound` 같은 파일을 옮겨야 하는데 보안 작업자가 `outbound.rs`, `validate.rs`, `webhook.rs`를 동시에 고치고 있어 충돌이 크다. 또 `deadpool`은 `pool()`과 엔진 전체에 쓰여서 HTTP만 따로 분리해도 기본 빌드에서 빠지지 않는다. 의존성을 기본 빌드에서 빼는 목표는 CLI feature 하나로 달성된다.

feature 이름을 `external-brokers`처럼 기능 단위로 하지 않은 이유는 PoC 경로를 한 묶음으로 켜기 때문이다.

## 마이그레이션 단계

1. 이 설계 문서를 커밋한다.
2. `aip-cli/Cargo.toml`에 feature와 optional 의존성, `[[test]] required-features`를 추가하고 `main.rs`의 PoC 명령을 `cfg`로 나눈다. 꺼진 빌드의 오류 경로와 테스트를 넣는다. `verify.sh`도 같은 단계에서 수정해 한 커밋이 항상 통과하게 한다.
3. `INFRASTRUCTURE`, `README`, `OPERATIONS`의 기본 빌드 설명과 의존 crate 수를 갱신한다.
4. Python 공급 경로의 의존성 구조를 확인한다. 선택 패키지가 필요하면 별도 단계로 다룬다.

## 깨질 수 있는 지점

| 지점 | 대응 |
| --- | --- |
| 기본 `cargo test --workspace`가 PoC e2e를 건너뜀 | `verify.sh`가 `--features poc-runtime`을 따로 실행. CI는 `verify.sh`를 쓰므로 함께 바뀐다. |
| `target/debug/aip`를 feature 조합마다 덮어씀 | 이미 `verify.sh` 마지막이 `cargo build -p aip-cli --locked`로 기본 빌드를 다시 만든다. e2e가 기본 바이너리를 쓰는 곳은 확인한다. |
| `aip run`을 쓰던 사용자가 기본 빌드에서 실패 | 오류가 빌드 명령을 알려준다. [OPERATIONS](../../product/OPERATIONS.md)가 이미 이 경로를 이전 구현으로 표기한다. |
| 동시 작업자가 `aip-cli`의 PoC 테스트를 추가 | 새 테스트는 `required-features`가 없으면 기본 빌드에서 컴파일 오류가 난다. 단계마다 rebase 후 `verify.sh`로 확인한다. |
| `product/tools/package.mjs`가 만드는 배포 산출물 | 제품 경로만 쓰는지 확인하고 `aip run` 의존이 있으면 feature를 명시한다. |
| `Cargo.lock` 충돌 | 의존성을 추가하지 않고 optional로만 바꾼다. 잠금 파일 내용은 변하지 않아야 한다. |

## 하지 않는 것

- `aip-runtime` 내부의 axum 분리와 crate 분할. 필요해지면 보안·포괄성 작업이 `aip-runtime`을 끝낸 뒤 별도 설계한다.
- 제품 경로의 동작·계약·권한 변경.
