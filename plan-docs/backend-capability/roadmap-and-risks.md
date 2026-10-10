# 우선순위·위험·미해결 문제

> 후속(2026-10-10): 이 문서의 감사 기준은 `ecf3369` 시점이다. 이후 [resource 독립 동기 operation](../../product/EXECUTION.md)을 제품에 연결했고 [durable Job 구조 제안](durable-job-proposal.md)을 구체화했다. 아래 역사적 미지원/제안 항목을 현재 구현으로 합산하지 않으며 최신 지원 범위는 후속 문서를 따른다.


2026-10-09 **제안 로드맵**. 이번 감사가 대규모 구조 변경의 승인은 아니다. 기능 개수나 미학적 통합보다 [시나리오](scenarios.md)의 반복 glue를 가장 많이 제거하는 단위를 먼저 검증한다. 수치화한 생산성·성능 향상이나 공수 견적은 아직 없다.

## 비교 기준

| 후보 | 반복 빈도에 대한 근거·추론 | 제거할 코드·계약 후보 | AI 선택 감소·재사용 | 위험 / 유지보수 비용 | 우선순위 |
|---|---|---|---|---|---|
| 실행 의존·효과·수명 계약과 SQL-free 경계 실험 | six scenarios와 제품의 실제 resource/PG 결합. 반복 빈도 수치는 미측정 | 가짜 resource/화면별 DTO·handler, 서로 다른 status 구현 | 데이터·Job·외부·파일을 같은 발견/검증 기준으로 안내 | 인증·폐기·배포 fence 호환이 핵심. 전체 엔진 교체 없이 실험 | P0 설계·실험 |
| outbox→전달/inbox + 등록 외부 capability | 제품 notify 미연결, Laravel/Dapr 및 앱의 queue glue | 발행/재시도/secret/timeout/dedupe wrapper | commerce·approval·sync·알림에 재사용 | 외부 unknown 효과/SSRF/보상/순서. provider별 차이 숨기지 않음 | P0 설계 → P1 구현 후보 |
| durable Job 공통 lifecycle | InvenTree import/export, Twenty stale runs, Temporal/Celery | queue acceptance/status/cancel/retry/result expiry | SQL 없는 F를 포함해 여러 앱 재사용 | lease·중복·권한 회수·배포중 버전·운영 worker 필요 | P0 설계 → P1 구현 후보 |
| 모든 executor의 principal·tenant·예산·관측 문맥 | PoC tenant negative control, Twenty workspace limits | 수기 tenant filter/queue context/감사·quota glue | 모든 표준·확장에 일관된 계약 | 경로 하나라도 빠지면 격리 의미가 다름. 실제 경로별 반례 필수 | P0 설계, executor와 동시 구현 |
| catalog/계약 metadata·생성 SDK 일관성 | 정적 binding은 있으나 runtime discovery 없음 | 수기 기능 탐색/별도 DTO·문서 동기화 | AI의 표준 선택·실패 대응 안내 | 권한·비밀 노출, available와 authorized 혼동 | P1, 실행 계약과 병행 |
| managed file/object handle + bounded 변환 | InvenTree export/user 재구성, F 실험의 JSON·resource 한계 | upload/result ACL/cleanup/converter status wrapper | 여러 변환에 같은 file·Job 계약 재사용 | 대용량·codec 격리·저장소 일관성·quota | P1–P2 |
| realtime 전문 executor | 실제 PoC subscribe tests와 E 전체 요구 | gateway/registry/권한 재조회/reconnect glue | 표준 public read 재사용 | 팬아웃·권한 회수·gap·보관 비용·편집 의미 | P2 |
| 표준 변환·조건부 bounded 조합 | AIP 로컬 확장 계산/업무 if·mapper의 반복은 후보 | 단순 mapping/format/batch loops | L4 fallback 줄임 | DSL 팽창·날짜/Decimal 의미 차이 | P2, 관찰된 반복만 추가 |
| 다중 DB·고급 DAG·추가 transport·중복 탐지 | 현재는 요구 범위 후보이며 사용자 빈도 미측정 | adapter별 반복/AI 선택 일부 | 앞 단계 계약에 의존 | 의미 차이/운영비·범용화 비용 큼 | P3 |

P0는 이번에 구조를 바로 바꾼다는 뜻이 아니라 **다른 설계가 의존하는 질문과 반례를 먼저 닫는다**는 뜻이다. quota/audit/discovery를 모든 executor 구현 뒤로 미루지 않는다. 가장 작은 실제 업무 한 단위에서 함께 검증한다.

## 승인 이후 구현 단위의 종료 조건

| 단위 | 최소 수직 검증 | 종료 조건 |
|---|---|---|
| 순수 작업 경계 | 동일 JS/Python 계산, 저장형 resource 0개, 업무 SQL 0, 폐기/계약/비용 음성 대조 | 새 우회 경로 없이 기존 read/apply 회귀 유지. DB 자체 없는 배포를 주장하려면 별도 인증 의존 실험 |
| Job+파일 수직 단위 | F 하나: 제출→진행→취소/재개→결과 만료 | HTTP disconnect/worker crash/중복/tenant 공격/cleanup 결과를 실제 상태로 확인 |
| 외부 효과 수직 단위 | D 또는 A 일부: outbox→provider 모사→reconcile | commit/효과/ack 각 crash 지점에서 중복·unknown·retry·DLQ 의미 확인. mock 검증과 실제 provider 별도 표시 |
| reusable 승인 | B: 다단계·조건·반려·기한·감사 | 재진입·자기승인·역할 회수·동시 결정·알림 실패 반례 |
| realtime 연결 | E: 공개 read 기반 subscription | 권한 회수·유실·resume 만료·재연결 snapshot·해제·느린 소비자 |

## 설계 위험과 독립 검토에서 남은 이견

| 문제 | 판정·대응 |
|---|---|
| SQL 편향을 없앤다며 PostgreSQL 자체를 문제로 삼음 | 업무 SQL 없는 기능과 상태 저장소 없는 기능을 구분. durable run에 PG 사용은 가능한 선택. 불필요한 데이터 실행 의존·연결 수명을 줄이는 이익부터 측정 |
| `effect none`이면 fence/DB를 빼도 된다는 제안 | 채택 안 함. aggregate와 운영 principal도 DB 의존. 검증된 dependency 집합으로 결정해야 함 |
| resource 독립 named operation이 Controller/DTO 반복을 부활시킴 | Claude는 기존 envelope로 transport 추가를 피할 수 있다고 반론. 부모는 transport가 하나여도 화면별 spec/DTO 유지보수가 남을 수 있다고 판단. 기존 표준 조합으로 충분한 경우 spec을 추가하지 않는 실험 관문 유지 |
| PoC 재사용 범위 | 전부 SQL/전부 폐기 주장은 정정. 서명·주소 검사는 유력 후보, 구독/Job은 Engine·IR·schema 결합 분석 뒤 판단. 모듈별 분리 비용은 확인 필요 |
| 실행 timeout 의미 | 5초 전체 상한이라는 주장은 실제 6초 실행으로 반박. 전역 예산과 단계별 deadline을 구분. 긴 동기 실행의 연결/DB/slot 비용 및 disconnect 취소는 추가 검증 |
| 외부 효과를 DB 원자성으로 가장 | local commit·전달·provider 완료·보상은 각각 계약. 정확히 한 번의 외부 효과는 provider 보장 없이 주장하지 않음 |
| snapshot 없는 notify | 현재 행은 topic/recipient/source 참조만 기록. 전달 시점 원본이 바뀌는 의미를 명시하거나 검증된 payload snapshot을 설계 |
| 안전한 L4의 플랫폼·파일 경계 | non-macOS 확장 설정이 거부됨. Linux 운영 격리 미구현. MacNetDeny는 file capability ACL 보장이 아님. 이를 단순 network 개방으로 해결하지 않음 |
| DSL·universal Runtime 팽창 | 표준 primitive의 의미와 전문 executor를 구분. 모든 provider·CRDT·가격 알고리즘을 DSL화하지 않음 |
| AI First가 기능 수 경쟁이 됨 | catalog·기본 선택·타입/효과 검사·에러 복구의 예측 가능성을 평가. 선택적 자연어 설명은 실행 근거가 아님 |
| 수치 근거 없는 생산성·성능 주장 | 이번 105행은 조사 범위 수, 실행 ms는 timeout 반례의 관찰값이다. 비용·속도·LOC 향상 수치로 사용하지 않음 |
| 라이선스/특허 | pinned root 파일을 대조했으나 하위 dependency/상용 코드/제3자 특허는 미검토. 코드 복사·새 OSS dependency 채택 없음 |

## 이미 재현했으나 새 범위에서 구현을 보류한 작은 제품 허점

이 항목은 SQL 밖 포괄성 작업을 SQL 수정으로 다시 축소하지 않기 위해 후속 목록으로 분리했다. 현재 유효 문법·계약과 범위를 대조한 뒤 작은 수정 단위로 진행할 수 있으며, 대규모 구조 변경과는 다르다.

| 항목 | 이번 관찰 | 후속 검증 기준 |
|---|---|---|
| 전이 effect의 nullable update/create | `Effect::Update`·`Effect::Create`의 타입 비교에서 Null→nullable 경로가 일반 전이와 다름. nullable update와 null match를 실제 loader에서 TYPE_MISMATCH로 재현 | 허용된 literal null의 sema/SQL 의미를 테스트. 동적 nullable `=`의 SQL 3값 의미를 일괄 변경하지 않음 |
| idless many effect | 의미 검사가 허용한 idless 대상에서 runtime `SELECT u.id`가 SQLSTATE 42703, 부모 전이 rollback 확인 | many의 identity 필요조건과 idless exact-one 지원을 구분 |
| W2 id-only empty create | 코드상 W0과 별도 effects 경로. SQL 빈 column 목록 가능성 **확인 필요** | 실제 PG 재현 전 결함 확정/수정 금지 |
| 생성 후보의 자동 id 참조 | nullable 생성 후보 보완은 id를 미리 생성하지 않음 | 서버 allow에서 generated id를 볼 의미·시점은 별도 설계 |

새 nullable create/참조 identity 수정은 감사 방향 전환 전에 진행하던 작은 배치로, 전체 core와 실제 SDK service smoke 검증 후 `ecf3369`에 따로 커밋했다. 이 배치가 비SQL 포괄성 기능을 구현했다는 뜻은 아니다.

## 다음 판단에 필요한 미검증

- 모든 framework/OSS의 전체 test suite·issue 조사, 실제 PSP/OAuth/cloud object/Kafka 연결: 미실행.
- Linux worker 격리, client disconnect 뒤 worker 정리, crash fault injection을 포함한 새 durable runtime: 미실행.
- 제품의 모든 tenant 경로·파일·subscription·quota 보장: 신규 executor가 없으므로 미구현.
- 현재 논의는 기술적 제안이며 창시자의 기존 철학을 재질문하지 않는다. 최종 문법·쓰기 모델 같은 기존 Open과 신규 대규모 구조 변경의 채택은 선점하지 않는다.
