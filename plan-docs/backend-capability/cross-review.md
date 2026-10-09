# Claude Code·Codex 독립 분석과 상호 비평

2026-10-09. 합의가 아니라 코드·반례로 판정했다. 현재 감사 정본은 [SQL 편향 감사](sql-bias-audit.md)·[실행 모델 제안](execution-model-proposal.md)이고, 아래 독립 초안은 오류 이력까지 보존한다.

## 방법과 한계

- **Claude Code**: 로컬 CLI `2.1.285`, 별도 세션에서 최신 원문과 코드만 읽어 독립 분석. Read/Glob/Grep/WebFetch/WebSearch만 허용했다. 최초 분석 때 Codex 결론을 주지 않았다. 이후 Codex 반론을 전달해 별도 비평을 받았다. [독립 초안](reviews/claude-independent.md), [상호 비평 원문](reviews/claude-cross-review.md).
- **Codex**: 부모와 별개의 native `gpt-6-sol high` 읽기 전용 컨텍스트가 제품/PoC 도달 경로를 독립 조사했다. 최초 결과 뒤에만 Claude 초안을 전달했다. 주요 코드 근거와 반박을 아래에 보존했다. 이 검토자도 테스트는 실행하지 않았다.
- **부모 실행 검증**: 실제 parser·HTTP·Node/Python·PG와 PoC E2E를 직접 실행했다. agent의 소스 판독은 검토 의견, 실행 관찰은 [validation](validation.md)의 별도 근거다.
- 우선 요청한 Luna high는 기존 사용 한도로 실행되지 못해 Sol high와 부모가 처리했다. Luna가 수행한 감사라고 기록하지 않는다.
- Claude 읽기 전용 하위 세션의 Stop hook이 work-history 쓰기를 요구해 최종 JSON을 기록 안내로 덮었다. 쓰기는 수행하지 않았고 초기 감사 본문은 해당 세션의 assistant text에서 회수했다. 부모가 수기 work-history를 대신 쓰지 않았다. 후속 상호 비평 본문은 별도 JSON으로 확보했다.

## 독립 Codex 판독의 핵심

제품 서비스→V1 facts→V2/V3/V4/V5/V6와 root Core IR PoC는 별개다. 제품의 실제 요청은 PG 연결·fence·clock을 선행하고, `/status`는 apply 복구이며, notify는 outbox 기록에서 멈춘다. worker ctx에는 aggregate/apply만 있고 파일·HTTP·progress broker가 없다. PoC에는 Job/schedule/webhook/object/subscribe가 있지만 제품 serve가 기동하지 않는다. 구조 원인은 resource 종속 이름공간, PG에 결합된 정책/배포 문맥, 제한된 효과 종류, 좁은 worker capability, durable/구독/catalog 계약 부재다. 초기 broad 분류를 그대로 지원율로 사용하지 않고 아래 반례로 정밀화했다.

## 상호 반박과 부모 판정

| 쟁점 | 최초/상호 주장 | 코드·실행 반례 | 채택 판정 |
|---|---|---|---|
| 순수 계산 | Claude 초안: SQL 없는 계산 방법 없음 | Codex: worker는 ctx 사용 의무 없음; `read_wire.rs` echo. 부모 T01 빈 access 계산이 Node/Python 모두 반환 | “업무 SQL 없는 계산 가능, PG 없는 제품 서비스 불가”로 정정 |
| 긴 작업 | 두 초기 판독의 5초 전체 상한 해석 | Claude 비평에서 REQUEST_TIMEOUT 적용 지점 정정; 부모 T01 6초 HTTP 계산 성공, 1초 선언은 DEADLINE_EXCEEDED | 긴 동기 실행 가능. durable/progress/cancel 부재와 연결 보유 비용을 별개로 기술 |
| SaaS 사용량 | Claude 초안: 사용량 집계 없음 | Codex: parser/plan count/sum/min/max와 limit atMost. Claude 후속 동의 | 사용량 조회·정수 건수 상한 가능; 과금주기 계량·동적 요금제 quota와 구분 |
| PoC Step | Claude 초안: 모든 Step이 SQL | Codex와 부모가 `aip-plan::Step` NewId/Encrypt/Upload/Fail를 직접 확인 | 사실 오류. 제품 결합 한계는 유지하되 PoC를 전부 SQL로 부르지 않음 |
| PoC 재사용 | Claude: 의미 명세만; Codex: 알고리즘·모듈도 후보 | outbound 순수 서명/주소 검사 vs Engine/plan 결합 구독·Job을 구분 | 모듈별 결합 분석 후 결정. 전부 폐기/그대로 이식 양쪽 미채택 |
| `/status` | Claude: run id 상태로 변경 | Codex: COMMIT_UNKNOWN/idem 복구 계약, 입력 mismatch 검사 존재 | 기존 의미 유지. durable 상태는 별도/호환 확장 제안 |
| fence와 effect | Claude: db 효과일 때만 DB fence | Codex: effect none도 aggregate를 읽고 운영 principal도 PG 조회 | 미채택. capability의 검증된 실제 dependency로 gating 설계 |
| named operation | 부모: 화면별 DTO/spec 반복 가능; Claude: 기존 envelope로 endpoint 추가 불필요 | transport envelope 하나와 업무별 spec 수는 다른 문제 | 기존 read/apply와 L1/L2 조합 우선. 실제 제거 코드/추가 spec 비교 관문 유지 |
| 관측성 | Claude 초안: eprintln 한 줄뿐 | Codex/부모: service startup/shutdown JSON report도 존재 | 요청 로그/metrics/trace 부족으로 범위를 좁힘 |
| 플랫폼·파일 | Claude: non-macOS 확장 불가, network-only sandbox | Codex/부모: server validate와 MacNetDeny 문자열 확인 | 실제 경계로 기록. Linux 실행 시험과 파일 ACL 보장은 미검증 |
| outbox | 양쪽: 기록과 전달 다름; Claude: snapshot 문제 추가 | DDL 6열+제품 consumer 미기동 확인 | delivery 완료로 주장하지 않음. payload/버전/재조회 의미는 설계 과제 |

## 남은 의견 차이

PG 없는 서비스 자체의 우선순위에는 차이가 있다. 부모/Codex는 계산·파일 등 비데이터 기능이 저장형 resource와 함께 묶이는 점을 포괄성 반례로 본다. Claude는 durable 상태저장소로 PG가 자연스럽고, 먼저 fence·DB 연결 수명 결합을 줄이는 것이 핵심이라고 본다. 이번 제안은 둘을 구분한다. **업무 SQL 독립은 필수 검증**, **PG 없는 배포는 인증/폐기/계약 의존을 포함한 별도 목표**다. PG 제거를 기본 구조 변경으로 확정하지 않는다.

외부 효과와 DB의 sync 조합을 일률 금지하자는 Claude 의견도 완전 채택하지 않았다. DB lock 보유 중 외부 I/O는 피하되, 외부 사전 조회·예약·확정·보상 순서는 업무 의미로 검증한다. “transaction 전에 결제하면 안전하다”는 일반 규칙은 만들지 않는다.

검토자가 실행하지 않은 Linux·실제 provider·durable crash/reconnect 보장은 여전히 미검증이다. 두 agent의 같은 의견이나 정상 문장은 실행 증거를 대신하지 않는다.

## 최종 문서 독립 검수

Sol high가 원문 105행과 CSV/읽기용 표, 주요 제안의 제품 경계를 다시 대조했다. 상품 시나리오의 “DB 실행과 멱등만 연결”은 타입·권한·outbox 기록도 연결된 실제 코드로 정정했다. A12의 현재 Level은 PG 단일 지원을 DB별 차이 처리 지원으로 오해하지 않도록 `—`로 정밀화했다. 추가 중대 불일치는 보고되지 않았으나 외부 license hash와 부모 실행 로그는 이 검토자가 재실행하지 않았다. 최종 artifact integrity 검증은 부모가 수행한다.

실행 증거 후속 검수에서는 8개 transport 상황·제품 4개·PoC 25개의 log/assertion 일치를 확인했다. CSV 결과 expiry 주장은 실제 삭제가 아니라 `s3.delete`의 미래 예약 시각 검사였음을 코드로 재확인해 validation/source 범위를 정정했다. 이전 core/smoke 전체 원시 로그는 `/tmp`에만 있고 저장소에는 신규 핵심 probe 원시 출력만 보존한 한계도 명시한다.
