# 다음 구조 변경: resource 독립 durable Job 실행기

2026-10-10 **설계 제안, 미구현**. 첫 동기 경계의 실제 문법은 [제품 operation](../../product/EXECUTION.md)이다. 아래 상태·테이블·HTTP·SDK 이름은 구현 후보다. SQL 없는 계산부터 검증하고, 파일이나 외부 효과의 미해결 보장을 Job 지원에 합산하지 않는다.

## 현재 코드에서 재사용할 것과 바꿀 것

| 접점 | 확인한 현재 구조 | 제안 |
|---|---|---|
| V1/V5 계약 | top-level operation의 입출력·정책·기한, 공개 descriptor·생성 SDK | 수명 선택과 서버가 정한 retry/총 기한/보관 예산 추가. 화면별 DTO 재작성 방지 |
| V6 operation | 요청 안에서 worker를 만들고 결과를 반환 | 동기 실행을 유지하고 acceptance와 runner를 별도 모듈로 분리 |
| V2 인가 | 기존 정책 compiler로 typed 입력과 actor를 평가 | acceptance·claim·결과 공개의 정책 평가에 재사용. worker 실행 중 DB tx 유지 금지 |
| V4 worker | one-shot Node/Python, typed 검증·NoCtx·bounded framing·메모리 Grant | 프로세스 실행 기능 재사용. 메모리 토큰과 Instant를 lease/영속 실행권으로 취급하지 않음 |
| aip-migrate | 생성 metadata와 resource 중심 derive/structure/verify | 초기화와 기존 schema 업그레이드를 함께 설계. Job 테이블만 초기 DDL에 추가하면 기존 배포가 빠짐 |
| root PoC Job | SQL snapshot·chunk tx·규칙 정산·CSV·object store가 Engine에 결합 | claim/retry의 아이디어·반례만 참조. Engine/Program/PoC 테이블 전체를 제품에 연결하지 않음 |
| root PoC 결과 파일 | object GET은 active 상태 확인만 하고 owner 인증을 하지 않음 | 이 경로를 제품 결과 ACL로 재사용하지 않음. 파일 다운로드까지 별도 ACL 필요 |

근거: `spikes/spike-v6-transport/src/operation.rs`, `spikes/spike-v4-worker/src/lib.rs`, `crates/aip-runtime/src/jobs.rs`, `crates/aip-runtime/src/http.rs`의 object handler, `crates/aip-pg/src/plan/job.rs`, `crates/aip-migrate/src/lib.rs`. 소스 판독이며 새 durable executor의 동작 실험 결과가 아니다.

## 첫 구현의 사용 경험

후보 SDK 예시이며 현재 실행 불가:

```ts
const run = await client.enqueue("normalizeDocument", input, { key });
const state = await client.jobStatus(run.id);
await client.cancelJob(run.id);
const output = await client.jobResult(run.id);
```

caller는 등록된 durable capability와 typed 값, 안정 key만 선택한다. 임의 queue·파일 경로·secret·URL·retry 횟수·owner는 고르지 않는다. 결과 handle은 권한을 부여하지 않는 불투명 식별자다. 첫 실행은 bounded JSON 입력→순수 JS/Python 계산→typed JSON 결과다. 파일 upload/download와 대용량 streaming은 후속 전문 경계다.

공통 계약은 이름·입출력·효과·정책·의존성·예산까지만 공유한다. sync operation 호출을 자동으로 durable로 바꾸거나 모든 read/apply를 Job으로 감싸지 않는다. 새 문법은 기존 다섯 작성 형식에서 같은 facts로 내려가야 하고 sync 기본값의 기존 계약 지문을 바꾸지 않아야 한다.

## 수락과 저장

제안 run store의 최소 필드는 run ID, operation·공개 계약 지문·서버 facts 지문·구현 digest, owner principal·허용 scope, 검증한 입력과 digest, idem key, 상태, attempt, lease epoch·owner·만료, 실행·전체 기한, 다음 실행 시각, 취소 요청 시각, typed 결과 또는 공개 오류, 결과 만료 시각이다. 토큰 원문·secret·임의 extension 경로는 저장하지 않는다.

1. 세션·현재 principal·배포 fence·입력·allow·수락 quota를 검사한다.
2. 같은 트랜잭션에서 실행 의도와 멱등성을 기록한다. unique scope는 배포/schema·principal·tenant가 있으면 검증된 tenant·operation·계약·key다. 동일 key/입력은 같은 run을 반환하고 다른 입력은 충돌이다.
3. commit 뒤 handle을 반환한다. 응답 유실이면 같은 key/원 입력으로 재제출해 수락 여부를 복구한다. 커밋 결과 불명을 미수락으로 단정하지 않는다.
4. HTTP 연결 종료는 수락된 작업을 취소하지 않는다. 취소는 명시 동작이다.

같은 DB의 업무 변경과 제출을 원자적으로 묶는 기능은 후속 DB executor 접점이다. 첫 Job API의 별도 제출만으로 주문 변경과 queue 수락의 원자성을 주장하지 않는다. 운영 quota는 run 수락과 claim의 상태 변화에 묶고, 다른 tenant의 key나 run ID로 결과를 확인할 수 없어야 한다. 첫 버전은 schema와 principal 범위만 지원한다면 tenant 지원으로 표기하지 않는다.

## 실행권과 복구

짧은 claim tx가 due QUEUED 또는 만료 RUNNING을 잠그고 epoch를 증가시켜 RUNNING과 lease를 저장한다. claim 뒤 tx/DB 연결을 반환하고 worker를 시작한다. heartbeat·진행·실패·완료·재큐잉은 모두 `id + epoch + lease owner + 기대 상태 + 유효한 lease`로 compare-and-set한다. 0행 갱신은 소유권 상실이며 성공으로 보고하지 않는다.

PostgreSQL은 `SKIP LOCKED`를 queue 소비자 사이 경합을 피하는 용도로 설명하며 일반 조회에는 불일치한 뷰를 제공한다고 명시한다. 이를 claim 후보로 사용하는 것은 AIP의 설계 추론이다. 이 구문만으로 stale worker의 완료 쓰기가 차단되지는 않는다. [PostgreSQL 17 SELECT](https://www.postgresql.org/docs/17/sql-select.html#SQL-FOR-UPDATE-SHARE)

재시작은 QUEUED와 만료 lease를 다시 발견한다. pure 작업은 중복 계산될 수 있으나 유효한 epoch 하나만 결과를 확정한다. 서버가 정한 총 attempt·전체 기한·backoff 상한을 넘으면 FAILED로 끝낸다. validation·정책 거부·출력 오류를 자동 재시도하지 않고 worker crash·일시적 store 오류와 구분한다. attempt 기한과 전체 run 기한, queue 대기 기한은 서로 다른 예산이다.

제품 배포는 현재 facts fence를 사용한다. 첫 버전은 저장한 facts/구현 digest가 달라지면 재실행을 보류하거나 계약화 실패로 끝내고 최신 코드로 조용히 재해석하지 않는다. 이전 구현 보관·버전별 worker routing은 별도 기능이다. 공개 SDK 지문만 같다는 이유로 내부 정책 변경을 무시할 수 없다.

## 신원·취소·결과

수락 때 확인한 issuer/subject/actor와 credential 발급 시각 또는 동등한 폐기 세대를 보관한다. background 실행은 만료한 JWT 원문을 다시 사용하는 방식이 아니다. claim·retry·결과 저장 전 현재 principal enabled/매핑/폐기 세대와 정책을 검사한다. revoke 후 같은 actor를 rebind해도 과거 run 권한이 자동 부활하지 않아야 한다. 토큰 만료와 명시적 principal 권한 회수는 구분하며 status/result/cancel에는 새 유효 세션을 요구한다.

상태 후보는 QUEUED, RUNNING, CANCEL_REQUESTED, SUCCEEDED, FAILED, CANCELLED와 결과 보관 만료다. QUEUED 취소는 즉시 terminal로 바꿀 수 있다. RUNNING 취소는 요청을 먼저 저장하고 감독자가 worker 종료·정리를 확인한 뒤 CANCELLED로 확정한다. 종료 확인 전 취소 완료를 반환하지 않는다. 완료와 취소가 경합하면 같은 row CAS에서 먼저 확정된 상태를 따른다. lease 상실만으로 실제 프로세스 종료까지 확인됐다고 기록하지 않는다.

Temporal 공식 문서도 cancellation의 전달과 worker의 수락을 구분하고 heartbeat를 통한 전달을 설명한다. 이 사실은 AIP에서도 요청/확인을 분리할 근거다. Temporal 전체 workflow replay 엔진을 채택한다는 뜻은 아니다. [Activity Execution](https://docs.temporal.io/activity-execution#cancellation)

상태·진행·결과에는 모두 현재 owner/scope 인가를 적용한다. 결과 만료 뒤 payload를 반환하지 않고 tombstone/멱등 보관 기간을 구분한다. 파일을 추가할 때는 jobStatus ACL에 더해 다운로드 경로의 owner·scope·만료·안전한 storage handle 검사가 필요하다. 원시 로컬 경로나 인증 없는 object URL을 반환하지 않는다.

## 변경 순서와 반례

| 작은 구현 단위 | 변경 표면 | 먼저 실패시킬 실험 |
|---|---|---|
| run-store 상태 기계 | 새 독립 Job 모듈, typed 저장 계약 | 동시 claim, lease 만료 뒤 이전 epoch의 결과/실패/heartbeat 거부 |
| schema 설치·upgrade | aip-migrate creation/derive/structure/verify | 기존 앱 init 없이 upgrade, 재적용, 변조 검출, 실패 rollback, 기존 데이터/idem 보존 |
| acceptance와 SDK | V1/V5/V6 + 설치 SDK | 중복 submit·다른 입력 충돌, commit 뒤 응답 유실, 잘못된 계약·다른 principal·범위 위반 |
| runner와 감독 | service lifecycle + V4 one-shot worker | HTTP 종료 뒤 실행, worker/server crash, 재시작 재claim, 전체 기한·retry 상한·종료 시 회수 |
| 취소와 결과 ACL | 별도 Job 경로 | QUEUED/RUNNING 취소, 완료와 취소 경합, revoke/rebind, 다른 사용자, 만료 결과, raw ID 공격 |
| 파일/외부 효과 | 후속 전문 executor | 파일 직접 다운로드 ACL, cleanup 실패, provider 효과 성공/ack 유실과 unknown 구분 |

`/status`는 기존 `aip_idem` 기반 WRITE 복구 계약을 유지한다. Job 상태 endpoint는 별도로 두고 SDK의 쓰기 pending/retry 의미를 변경하지 않는다. 연속 실행 worker는 HTTP 요청용 semaphore만 공유해 읽기 전체를 굶기지 않도록 별도 server 한도를 둔다. audit에는 run ID·attempt·epoch·상태 전이·거부 이유만 남기고 입력·출력·토큰은 기본 로그에서 제외한다.

첫 종료 기준은 **업무 resource 0개인 Node/Python 계산 Job이 응답 유실·재시작·중복·권한 회수·취소·결과 만료를 견디는 실제 설치 SDK 경로**다. 처리량·생산성 개선은 이 제안에서 측정하지 않았다. provider exactly-once·보상·DAG·Cron·stream·tenant 전체 격리는 이 종료 기준에 포함하지 않으며 각각 후속 검증이 필요하다.
