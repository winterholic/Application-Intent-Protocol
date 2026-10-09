# 코드·오픈소스 근거 목록

2026-10-09 조사. 제품 코드 기준 `ecf3369`(원문 보관 커밋 `69cbaaa`는 코드 변경 없음). 경로는 저장소 루트 기준이다. `[P]` 제품 경로, `[Q]` 별도 root PoC, `[T]` 이번 실행 실험이다. 문서의 부재 판정은 열거된 parser/HTTP 경로/worker capability에서 확인한 범위다. 저장소 어디에도 비슷한 코드가 없다는 주장이 아니다.

## 제품과 PoC

| ID | 직접 확인한 코드 | 확인 범위 |
|---|---|---|
| P01 | `spikes/spike-v1-fixture/src/{parser,sema,ast}.rs` | 최상위 enum/actor/predicate/access/limit/resource, 리소스 전이·집계·확장·docs. sema의 `extension`은 read/none, write/db로 제한. 필수 의미 검사는 dev-checks와 구분 |
| P02 | `spikes/spike-v2-read/src/{plan,sqlgen,exec}.rs`, `tests/aggregate_measures.rs` | 공개 select/filter/sort/관계/집계의 계획·PG 실행·가시성. count/sum/min/max 지원. 저장형 resource의 DDL 생성 |
| P03 | `spikes/spike-v3-write/src/lib.rs` | `effects`의 create/update/notify, `create_row`, apply/check/commit, 행·권한·advisory 잠금. 외부 I/O 실행 아님 |
| P04 | `spikes/spike-v4-worker/src/{lib,write,value}.rs`, `workers/worker.{mjs,py}`, `tests/read_wire.rs` | 입력/출력·grant·binding·deadline·frame 제한, READ aggregate/WRITE apply ctx. ctx 없는 JS/Python 계산 가능. `MacNetDeny`는 network 차단이며 파일시스템 capability sandbox가 아님 |
| P05 | `spikes/spike-v5-sdk/src/lib.rs`, `spikes/spike-v6-transport/client/{transport,typed}.ts` | 정적 descriptor/TS 계약·지문·캐시·타입검사. 생성 public docs 전파 범위 전체는 확인 필요 |
| P06 | `spikes/spike-v6-transport/src/lib.rs:353–355,383–447,471–546`, `src/server.rs:14–17,145–173,318–325` | 다섯 HTTP 경로, 서명 세션/계약 검사, DB 연결/fence/DB clock 선행, 확장 실행. `/status`는 멱등 쓰기 결과 복구. REQUEST_TIMEOUT은 전체 worker 실행 상한이 아님. macOS 밖 확장 설정 거부 |
| P07 | `crates/aip-auth/src/lib.rs:280–301`, `crates/aip-service/src/lib.rs:307–350` | JWT 검증과 PG principal 연결·활성·폐기 검사. 이 인증 의존성도 DB 없는 경로 설계에서 다뤄야 함 |
| P08 | `crates/aip-migrate/src/`, `crates/aip-service/src/lib.rs:338–345` | 마이그레이션 plan/preflight/bind wire/journal; 서비스 기동시 DB 필수. deploy fence는 호환·권한 보호이며 임의 제거 대상 아님 |
| P09 | `spikes/spike-v2-read/src/sqlgen.rs:598`, `spikes/spike-v4-worker/src/outbox.rs` | 제품 outbox는 id/topic/recipient_id/source/source_id/created_at 6열. 소비 실험은 MockProvider와 별도 ALTER로 delivery 열을 추가. 제품 serve는 이를 기동하지 않음 |
| P11 | `crates/aip-service/src/lib.rs:376–385`, `spikes/spike-v6-transport/src/server.rs` | 기동·정지 report와 요청 panic 로그가 있음. 요청 전체 구조화 로그/metrics/trace/health와 구분 |
| Q01 | `crates/aip-cli/src/main.rs:248–293`, `crates/aip-runtime/src/{lib,http,engine,exec,crypto}.rs`, `crates/aip-plan/src/lib.rs:304–407` | root `aip run`은 별도 Core IR/PG engine. Step 모두 SQL이라는 주장은 틀림: NewId/Encrypt/Upload/Fail 등은 SQL 필드 없음. 단 상당수 Let/Check/When/ForEach는 SQL 의미에 결합 |
| Q02 | `crates/aip-cli/tests/{shop,ariari,saas,cms}_e2e.rs` | 재고 경합, 다단계 승인·만료, tenant 참조·transaction pin, 파일/CSV Job 등. 이번 재실행 근거는 validation 참조 |
| Q03 | `crates/aip-runtime/src/{jobs,schedule}.rs` | 영속 Job claim/SKIP LOCKED/heartbeat/backoff와 scheduler advisory lock. 업무 항목 스냅샷과 PG/Engine 결합 존재 |
| Q04 | `crates/aip-runtime/src/{dispatch,outbound,webhook}.rs`, `crates/aip-cli/tests/outbound_e2e.rs` | 별도 이벤트/서명·재시도/공개 주소 검사 구현. outbound의 signing_secret/signature_header/retry_offset_ms/is_public는 재사용 검토 후보. 전달 순서 보장 없음 |
| Q05 | `crates/aip-runtime/src/objects.rs`, `http.rs:136–172`, `ariari_e2e.rs:389–445,460–546` | 개발용 local store, multipart·staging, CSV 결과/삭제 예약 시각(실제 만료 미실행). multipart field.bytes는 통째로 적재하므로 대용량 streaming 검증 아님 |
| Q06 | `crates/aip-runtime/src/subscribe.rs`, `crates/aip-cli/tests/subscribe_e2e.rs` | WS·LISTEN/NOTIFY 후 subscriber로 전체 재조회, bounds와 가시성 재검사. durable event cursor/replay는 이번에 확인 못함 |
| T01 | [SQL 없는 계산·시간 제한 실험](validation.md), `probes/sql-free-boundary.rs` | 실제 parser/V5 지문/V6 listener/격리 Node·Python worker. 순수 계산 가능, DB 불가시 DB_UNAVAILABLE, 6초 동기 실행 가능, 선언 1초 deadline 거부 |

## 공식 문서로 확인한 사실과 AIP 적용 추론

아래는 조사 시점 공식 페이지다. 버전 숫자·광고 성능을 AIP 보장으로 가져오지 않는다. 각 행의 적용 아이디어는 **추론**이며 의존성 채택 결정이 아니다.

| 프로젝트·공식 문서 | 확인한 사실 | 적용 아이디어 / 가져오지 않을 부분 |
|---|---|---|
| [Spring Boot Actuator](https://docs.spring.io/spring-boot/reference/actuator/endpoints.html) / [task execution](https://docs.spring.io/spring-boot/reference/features/task-execution-and-scheduling.html) | health/metrics/운영 endpoint와 접근·노출 설정을 구분; 실행·스케줄러 설정은 별도 영역 | 운영 capability도 공통화하되 공개 업무 catalog와 분리. endpoint 존재만으로 공개 허용을 뜻하지 않음 |
| [Spring Framework integration](https://docs.spring.io/spring-framework/reference/integration.html) | REST clients, messaging, email, scheduling, cache, observability가 별도 통합 영역 | 데이터 밖 반복 기능도 catalog 대상. framework 구성요소 전부를 DSL 노드로 번역하지 않음 |
| [NestJS queues](https://docs.nestjs.com/techniques/queues) | BullMQ/Bull 통합에서 producer/consumer/listener와 별도 queue 인프라를 구성 | queue mechanics와 업무 handler 분리. decorator·service층을 그대로 생성하는 AIP는 지양 |
| [FastAPI background tasks](https://fastapi.tiangolo.com/tutorial/background-tasks/) / [WebSockets](https://fastapi.tiangolo.com/advanced/websockets/) | 응답 이후 background task와 WS를 지원; 무거운 분산 작업은 별도 도구를 안내 | in-process 작업을 durable Job이라고 부르지 않음. transport와 실행 수명 구분 |
| [Django tasks](https://docs.djangoproject.com/en/6.0/ref/tasks/) | task 정의/queue/결과 계약과 실제 외부 실행 인프라를 구분 | AIP 계약이 특정 scheduler 구현을 강제할 필요 없음. 계약만 있고 worker가 없으면 지원 상태에 표시 |
| [Laravel queues](https://laravel.com/framework/docs/13.x/queues) | after_commit 설정으로 열린 DB transaction의 commit 뒤 job dispatch 가능, rollback이면 폐기 | commit 경계 표준화. after-commit callback만으로 crash-safe outbox가 보장된다고 추론하지 않음 |
| [ASP.NET hosted services](https://learn.microsoft.com/en-us/aspnet/core/fundamentals/host/hosted-services?view=aspnetcore-10.0) | hosted background service, cancellation token, bounded Channel queue 예제 | backpressure/종료 계약 표준화. 메모리 queue 예제를 durable 분산 queue로 취급하지 않음 |
| [Temporal activity failure detection](https://docs.temporal.io/encyclopedia/detecting-activity-failures) | heartbeat에 progress 정보를 싣고 재시도에 활용; activity 취소 전달도 heartbeat에 의존 | 취소 요청과 완료 확인 분리, checkpoint 의미 명시. Temporal replay 엔진 전체 재구현은 범위 밖 |
| [BullMQ flows](https://docs.bullmq.io/guide/flows/) | parent는 기본적으로 성공한 child들을 기다리고 waiting-children 상태를 가짐 | dependency/실패 전파는 계약이어야 함. 임의 DAG·무제한 fanout을 기본 primitive로 만들지 않음 |
| [Celery tasks](https://docs.celeryq.dev/en/stable/userguide/tasks.html) | ack 시점·redelivery와 멱등성의 관계, I/O timeout 필요를 설명 | retry는 효과 중복 문제와 같이 설계. acks_late 하나로 exactly-once를 주장하지 않음 |
| [Dapr pub/sub](https://docs.dapr.io/developing-applications/building-blocks/pubsub/pubsub-overview/) | at-least-once, dead letter topic, transactional state store와 outbox 기능을 구분 | publish·commit·consumer 효과 계약 분리. broker 종류만 바꾸면 순서 의미도 같다는 가정 금지 |
| [OpenFGA conditional tuples](https://openfga.dev/blog/conditional-tuples-announcement) / [project](https://openfga.dev/project) | 관계 모델과 조건 context로 인가를 평가. model/API/RFC 저장소가 분리 | 호출자 입력과 신뢰된 policy context를 구분. 외부 권한 엔진 추가가 필수라는 결론은 아님 |

검색 결과만 보고 주장하지 않고 위 본문을 열어 대조했다. Temporal 예전 workflow-execution URL과 OpenFGA www URL은 도구에서 열리지 않아 공식 대체 페이지로 확인했다. 프레임워크별 전체 issue·테스트 실행은 **미실행**. 새로운 불일치가 생기면 대상 subsystem의 pinned source/test/issue까지 조사한다.

## 실제 애플리케이션 소스 대조

외부 애플리케이션 전체를 실행한 결과가 아니다. 아래 pinned 소스와 테스트를 읽고 반복 문제를 추출했고, AIP 쪽 검증 범위는 validation에 따로 기록한다. 외부 코드 복사·새 의존성 추가 없음.

| 실제 프로젝트 | pinned 소스·테스트 | 확인한 업무와 적용 추론 |
|---|---|---|
| InvenTree `3ae99490fca22799848d5618ea8cc055829d6723` | [importer/tasks.py](https://github.com/inventree/InvenTree/blob/3ae99490fca22799848d5618ea8cc055829d6723/src/backend/InvenTree/importer/tasks.py), [data_exporter/tasks.py](https://github.com/inventree/InvenTree/blob/3ae99490fca22799848d5618ea8cc055829d6723/src/backend/InvenTree/data_exporter/tasks.py), [importer/tests.py](https://github.com/inventree/InvenTree/blob/3ae99490fca22799848d5618ea8cc055829d6723/src/backend/InvenTree/importer/tests.py) | import session 기반 비동기 처리·오래된 session 정리, export에서 user/plugin/output를 재조회하고 request를 재구성. AIP는 Job 권한 context/파일 handle/result 계약으로 반복 glue를 줄일 수 있다는 가설. 누락 user/plugin에서 단순 return하는 방식을 일반 성공 계약으로 복사하지 않음 |
| Frappe `d82dc2e4a8ae2db61da065f3e5da43898e42d1f6` | [model/workflow.py](https://github.com/frappe/frappe/blob/d82dc2e4a8ae2db61da065f3e5da43898e42d1f6/frappe/model/workflow.py), [test_workflow.py](https://github.com/frappe/frappe/blob/d82dc2e4a8ae2db61da065f3e5da43898e42d1f6/frappe/workflow/doctype/workflow/test_workflow.py), [user_invitation.py](https://github.com/frappe/frappe/blob/d82dc2e4a8ae2db61da065f3e5da43898e42d1f6/frappe/core/doctype/user_invitation/user_invitation.py) | workflow의 전이·자기승인 검사·동기 task와 commit 뒤 비동기 task 분리, 초대 허용 role·중복 수락·만료 검사. AIP는 L3 재사용 승인/초대 specification과 공통 lifecycle을 분리할 후보. safe_eval 표현식이나 권한 무시 옵션을 가져오지 않음 |
| Twenty `c168183b17e89fbc8747f8ed333d90466ebf85f4` | [workspace throttling](https://github.com/twentyhq/twenty/blob/c168183b17e89fbc8747f8ed333d90466ebf85f4/packages/twenty-server/src/modules/workflow/workflow-runner/workflow-run-queue/workspace-services/workflow-throttling.workspace-service.ts), [staled-run recovery](https://github.com/twentyhq/twenty/blob/c168183b17e89fbc8747f8ed333d90466ebf85f4/packages/twenty-server/src/modules/workflow/workflow-runner/workflow-run-queue/workspace-services/workflow-handle-staled-runs.workspace-service.ts) | workspace별 token bucket/대기 counter, stale run 재큐잉과 counter 재계산. AIP도 tenant quota·복구를 공통화할 후보. system context/permission bypass를 caller 기능으로 노출하지 않음. 혼합 라이선스라 관찰·독립 구현 재료로만 사용 |
| Saleor `bb75f87973abe24056a5d0dff1708ca097e95599` | [product/models.py](https://github.com/saleor/saleor/blob/bb75f87973abe24056a5d0dff1708ca097e95599/saleor/product/models.py), AIP `spikes/spike-v3-write/tests/oss_product_variants.rs` | Product.default_variant와 ProductVariant.product 순환 참조에서 이중 FK 생성·가시성·unique/rollback를 AIP에서 검증. 결제/환불·전체 주문 흐름을 검증한 것은 아님. 같은 상품의 variant라는 불변조건은 FK+unique만으로 보장되지 않음 |

## 라이선스·특허 확인 범위

아래 표는 실제 읽은 pinned root LICENSE와 SHA256을 기록한다. 하위 패키지·종속성·상용 기능까지 같은 라이선스라는 의미가 아니다. MIT/BSD 파일에는 명시적 patent grant 항목을 확인하지 못했고, Apache-2.0 파일에는 patent grant/termination 조항이 있다. Twenty의 AGPL 본문에도 patent 관련 조항이 있으나 enterprise 별도 조건이 존재한다. 제3자 특허 존재 여부·호환성 법률 판단은 **확인 필요**이며 검색·법률 검토를 수행하지 않았다. 코드 이식이나 의존성 채택을 승인한 결과가 아니다.

| 대상 | 읽은 root 라이선스 | 파일 SHA256 |
|---|---|---|
| [spring-projects/spring-boot](https://github.com/spring-projects/spring-boot/blob/535d133bdaa0ed46bbfb7ce578c14261806f39ac/LICENSE.txt) | Apache-2.0 | `7200000367e0e8b47822d563a5ddd2dd3a97cc05b32bd7dbcc7b4eeb92b76628` |
| [spring-projects/spring-framework](https://github.com/spring-projects/spring-framework/blob/2f2008a6e7c6003a104b5a4cf9dce1d7779bb349/LICENSE.txt) | Apache-2.0 | `56dfc19e0dc836e30177332f73e8e6fbc297941acf3d906eec6eaaa46c2c452a` |
| [nestjs/nest](https://github.com/nestjs/nest/blob/b6295c7994890a82dbeaaccd69530bc66194181a/LICENSE) | MIT | `56967de93525b12a30717e4446ccef3bbe32fafdbe2350f143c1cc6971b19ed3` |
| [fastapi/fastapi](https://github.com/fastapi/fastapi/blob/f5c6e9b4f9cadc1bf0f9a1fd672e621206b63007/LICENSE) | MIT | `4ec89ffc81485b97fec584b2d4a961032eeffe834453894fd9c1274906cc744e` |
| [django/django](https://github.com/django/django/blob/1b4d021b6525bff6439b9d3b8d5388a1a1906ecd/LICENSE) | BSD-3-Clause | `b846415d1b514e9c1dff14a22deb906d794bc546ca6129f950a18cd091e2a669` |
| [laravel/framework](https://github.com/laravel/framework/blob/848f3edfc03fc5b235344126340543281950bfb4/LICENSE.md) | MIT | `7b4f9149597fbae2ce689bc9317d1869e17f1b3fa5f01c289998c108f8c37cf7` |
| [dotnet/aspnetcore](https://github.com/dotnet/aspnetcore/blob/47df67a4e60255d71b25b3f71938fa3069e1260f/LICENSE.txt) | MIT | `cfc21f5e8bd655ae997eec916138b707b1d290b83272c02a95c9f821b8c87310` |
| [temporalio/temporal](https://github.com/temporalio/temporal/blob/bae8771853cff3f127b979310e22dcd61c28375c/LICENSE) | MIT(서버 core) | `6aab9afd99ceb1dcaf6a0d91386b46baef9bce17ae4006044e0f231e31430b69` |
| [taskforcesh/bullmq](https://github.com/taskforcesh/bullmq/blob/4bb572c67018e60fd0c82dfb9754f075ea934da6/LICENSE) | MIT(공개 core) | `ad9e1382b9c055365f7a6a659a0e94543c7424a0272b8b44a54f79d6c961d321` |
| [celery/celery](https://github.com/celery/celery/blob/357e629eb6bc07284bc6677fb32ffb471e2faf62/LICENSE) | BSD-3-Clause; docs는 CC BY-SA 4.0(같은 LICENSE에 명시) | `c358cdf77f28bbab50d4a754e123118339c15230349e82bf66489cece3ac093a` |
| [dapr/dapr](https://github.com/dapr/dapr/blob/ff053f73a59362d3fa6321aab9c9a29081412801/LICENSE) | Apache-2.0 | `9523eacf5421b65637420755c00b5175f7804b6a6eb736b86cd10ea6b3937dcf` |
| [openfga/openfga](https://github.com/openfga/openfga/blob/526995eb202464e2cace266fb5be56c212af9ecf/LICENSE) | Apache-2.0 | `1c46d7b2bed94d457d745f28cabeb31f8d6c81dd9035bc5d24039989ee1e1bff` |
| [inventree/InvenTree](https://github.com/inventree/InvenTree/blob/3ae99490fca22799848d5618ea8cc055829d6723/LICENSE) | MIT | `70f21a7a9bb007581b06860ad76d623c95d5c92663a2326b5fa79a198aed55e0` |
| [frappe/frappe](https://github.com/frappe/frappe/blob/d82dc2e4a8ae2db61da065f3e5da43898e42d1f6/LICENSE) | MIT | `bc6001a54ffcc4ab520424d7dbb85b293578efcdcb7d8f8055e00dddf942e5d7` |
| [twentyhq/twenty](https://github.com/twentyhq/twenty/blob/c168183b17e89fbc8747f8ed333d90466ebf85f4/LICENSE) | AGPLv3 + exception / MIT packages / enterprise 조건 | `caece25ff05e7058a8fd98c34313757ed48d5e9304e0aeb7875d4b11de77bc01` |
| [saleor/saleor](https://github.com/saleor/saleor/blob/bb75f87973abe24056a5d0dff1708ca097e95599/LICENSE) | BSD-3-Clause | `70e2aa4451206e213dc32078c07ca703c7c2b6e9a8e63803ee11db4deea7313b` |

Apache-2.0은 notice/license 보존과 특허 관련 조건, MIT/BSD는 고지·면책 보존 등 원문 조건을 동반한다. Twenty의 license header와 패키지별 LICENSE 확인 없이 파일을 옮기면 안 된다. 이번 작업은 개념 조사와 AIP 자체 실험만 수행했다.
