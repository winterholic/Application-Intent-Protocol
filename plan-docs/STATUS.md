# 진행 현황과 쌓인 질문

> 현재 기준: [창시자 통합 지침](sources/founder-integrated-directive-2026-10-03.md). 2026-10-03 새 지침 반영.

## 현재 제품 작업 (2026-10-05)

사용자의 “운영인증, 일반마이그레이션, 제품통합 다 해” 지시로 범위를 확장했다. 기존 PoC는 보존하고 root CLI의 `aip service`와 `aip-auth`, `aip-migrate`, `aip-service`를 추가했다. caller 정의·서버 권한·기존 실행 엔진·하나의 SDK를 재사용한다.

운영 JWT 검증과 지속 actor 연결, 일반 DDL 계획·적용·배포 저널, 데이터와 멱등 결과를 보존하는 명시적 prototype 이관, 설치 SDK를 제품 경로에 연결했다. 재귀 제한은 source·token·AST·predicate 확장·path·SQL 생성·복제 payload에 함께 적용한다. 활성 서버의 구 정책 실행, 연결 재활성화 시 폐기 토큰 복구, Id wire 변경에 따른 멱등 namespace 변화도 회귀 검증 대상으로 고정한다.

2026-10-06 최종 제품 검증은 [제품 결과](../product/VERIFICATION.md)와 [배포 묶음](../product/dist/2026-10-06T11-19-25-091Z/README.md)을 따른다. 현재 실행 절차·검증 명령·미지원 범위는 [제품 서비스](../crates/aip-service/README.md), [마이그레이션](../crates/aip-migrate/README.md), [SDK](../product/SDK.md)를 따른다. 외부 IdP 실배포·TLS proxy·다른 OS·무중단 대형 backfill은 검증하지 않았다. 최종 DSL·Id wire·쓰기 조합·라이선스 결정은 유지한다.

## 이전 단계: 2026-10-04 밤 작업 요약

설계 문서를 바탕으로 **작은 검증 실험 V1부터 V15까지 실행했다.** 본 `crates/`는 건드리지 않았고(통합 지침 §16·17), 실험은 `spikes/`의 독립 Rust crate에 있다. V1부터 V7까지는 기존 Codex 6.1-sol 검토 이력이 있고, V8·V9·V10·V11·V12·V13 후속 작업은 Codex 메인과 Luna high·Sol high가 검증했다. V14는 메인이 테스트·구현·회귀를 수행하고 Sol high가 독립 검토했다. Luna는 V14에서 사용 한도로 참여하지 못했다. V15에서는 다시 참여해 생성기 테스트·범위 대조를 맡았고, 메인이 구현·HTTP/DB·회귀를, Sol high가 설계·독립 검증을 맡았다. 찾은 문제는 재발 테스트로 고정했다.

1. **문법(V1)**: 독립 `.aip`, TS/Python 안 선언 블록, TS/Python 정형 데이터 5가지 작성 방식이 같은 의미(typed facts)로 바뀐다. 잘못된 기호·동적 문자열·주석 속 가짜 선언은 거부한다. 형식을 고르는 근거(작성 경험)는 아직 측정하지 않았다.
2. **호출자 읽기(V2)**: 화면이 고른 필드·관계·필터·정렬을 서버 정책 아래 매개변수 SQL 하나로 실행했다. 열린 필드는 요청만 바꾸면 되고, 닫힌 필드는 서버 정의 변경이 필요하다는 경계가 실제로 동작한다.
3. **쓰기(V3)**: 알림 읽음·모집 마감 같은 단순 쓰기는 표준 전이 하나로 원자성·동시성까지 처리됐다. 일괄 승인과 관리자 위임에서는 **서버 정의 전이(W2)가 순서까지 서버 안에 두고 업무 불변식을 지켰다.** 공개 동작 묶음(W0)과 호출자 조합(W1)은 커밋 시점 불변식·사후조건이 있어야 안전했고, W1은 업무마다 새 조합 개념(값 출처, 자기 행)이 필요했다. 어느 쪽을 표준으로 둘지는 아직 정하지 않는다.
4. **확장(V4)**: Node·Python 확장이 서버가 묶어 준 권한 안에서만 데이터를 읽고, 쓰기 확장은 서버 트랜잭션 안에서 공개 전이만 불러 실패·기한 초과 시 전부 되돌려졌다. 다만 **worker를 별도 프로세스로 띄우는 것만으로는 확장이 DB에 직접 붙는 것을 막지 못했다.** macOS에서는 네트워크 차단으로 막혔지만 운영 환경용 격리 방식은 아직 정하지 않았다. 후속 프로토타입 준비에서 공통 scalar·bounded stdout/stderr·Result 시작·읽기 Grant 정리를 연결했고 Rust23개와 실제 Node/Python 초과 쓰기 롤백·외부 취소·반복 호출, 음성 대조7종을 검증했다. 후속 READ 연결은 SafeNumber/DecimalString을 ctx와 입출력에 적용하고 생성 SDK·HTTP·명시적 MacNetDeny worker로8개 실제 흐름을 실행했다. 당시 V4 전체25개 실패0이며 후속 bootstrap과 WRITE 입력 전송 기한 보완은 프로토타입 최신 기록을 따른다. 후속 WRITE HTTP/생성 SDK는 같은 공개 전이·멱등 저장소·원자적 결과 저장을 사용하며 [prototype 최신 결과](../prototype/README.md#write-공식-확장의-실행-결과)를 따른다. 운영 격리는 남는다. 외부 효과(알림)는 outbox로 "커밋된 것만 전달 시도, 실패 재시도 뒤 격리, 멱등 키로 중복 효과 억제"를 확인했다.
5. **프론트 SDK(V5)**: 화면이 고른 필드에서 결과 타입이 추론되고 계약 밖 요청은 컴파일 오류가 된다. 캐시는 정책이 읽는 권한 테이블을 포함한 읽기 의존 resource로 무효화하며, 같은 연결의 쓰기·계정 전환·결과 미확정·늦은 응답을 처리한다. 다른 연결의 변경·권한 회수는 V9의 수명 만료 뒤 재조회까지 지연될 수 있다.
6. **전송(V6)**: 최소 HTTP 서버와 TS 클라이언트를 연결했다. 쓰기 응답이 유실돼도 같은 멱등 키로 다시 보내 결과를 확정하고, 두 번 실행되지 않는다. 인증은 V8에서 서버 서명 토큰으로 붙였다(위조·변조·만료·다른 키 거부, 실패 토큰은 익명으로 낮추지 않음).
7. **마이그레이션(V7)**: 두 계약 버전의 변경을 안전·호출자 영향·보안 검토·파괴적·데이터 위반으로 나누고, 행 정책 변경은 실제 회원별로 보이는 행을 측정해 검토 근거로 붙인다(측정만으로 안전을 확정하지 않음).
8. **수명·복구(V9)**: 캐시는 검증한 세션 수명과 실험용 신선도 상한 안에서만 재사용한다. 시간 의존 조회는 현재 시각으로 실행하고 미캐시한다. 같은 멱등 키의 반복·동시 호출, 인증 만료 뒤 미확정 쓰기 보존, 잘못된 성공 응답, 헤더 크기 상한을 반례로 검증했다. [실행 결과·한계](alignment/V9-cache-lifetime-results.md).
9. **세션 갱신(V10)**: 첫 read/apply 전에 서버 principal을 확인한다. 같은 actor의 새 토큰으로 pending을 자동 복구하고 다른 actor 교체는 원래 정보를 유지한 채 거부한다. 캐시와 진행 중 요청을 인증 세대로 분리하고 signed -1과 익명 멱등 scope를 분리했다. 실제 토큰 만료·승인 응답 유실 뒤 replay와 DB 단일 효과를 확인했다. [실행 결과·한계](alignment/V10-session-recovery-results.md).
10. **선택적 검사(V11)**: 개발 조언 off/on을 실행 계약과 분리했다. docs 없음·변경·소속 이동이 실행 facts·생성 TS·DB 권한을 바꾸지 않는지 대조했고, 조언이 있어도 정상 실행을 막지 않는다. raw 요청의 타입·권한·비용 검사는 필수다. [실행 결과·한계](alignment/V11-optional-checks-results.md).

11. **계약 타입 연결(V12)**: 앱별 생성 binding만 전달하면 실제 HTTP 결과의 선택형 타입을 얻는다. 계약 식별값과 성공 envelope를 캐시 저장 전에 검사하고, 구세대 오류·미확정 쓰기 경계를 유지한다. typed facade의 캐시 주입 우회는 닫았다. 큰 Id 표현·typed apply·전체 행 decoder는 남는다. [실행 결과·한계](alignment/V12-typed-transport-results.md).

12. **큰 Id 왕복(V13)**: Legacy 반올림으로 이웃 행을 변경하는 실제 DB 반례를 재현했다. 숫자 안전범위 제한과 십진 문자열 후보는 서버 공통 경계에서 처리하며, 계약 불일치는 첫 쓰기도 DB 전에 거부한다. 공개 Id 표현은 아직 선택하지 않았다. [실행 결과·한계](alignment/V13-id-boundary-results.md).

13. **공개 쓰기 타입(V14)**: 같은 binding에서 공개 전이 입력과 readonly 결과가 추론된다. 두 Id 후보·읽기 없는 쓰기·응답 유실/손상 복구·실제 결과 동결을 검증했다. 공개 쓰기 변경도 지문에 포함하며 서버 최종 검사는 유지한다. V14 당시 미지원 scalar where는 후속 V15에서 연결했다. 조합/worker 타입·최종 API는 남는다. [실행 결과·한계](alignment/V14-typed-apply-results.md).

14. **공개 필터 실행(V15)**: Url·Enum·Time read/where 공통 검사와 Text.prefix를 실제 PG/HTTP에서 대조했다. 날짜·offset·소수초 반올림·NUL 선거부·타입/지문·권한을 확인했다. [실행 결과·한계](alignment/V15-filter-value-results.md).

15. **실행 프로토타입 연결**: [check/gen/init/serve CLI·단일 SDK](../prototype/README.md)를 기존 엔진으로 연결했다. CLI22개와 데이터 보존·전체 DDL 롤백·DB 설정·종료·세 음성 대조를 검증했다. safe/decimal 각각 실제 생성 SDK→서버→공개 정의 변경→재생성·재기동에서 구 binding 첫 쓰기 거부와 새 binding 실행, 기존 데이터·멱등 결과 보존을 확인했다. Sol은 소스 SDK decimal 흐름을 독립 재실행했다. 로컬 private SDK tarball은 새 소비자의 offline 설치·일반 JS 실행·strict TS 검사와 출력 보호를4개 테스트로 검증했고, source/package × decimal/safe4개 실제 서버 흐름을 실행했다. 새 Luna가 패키지4개·설치 SDK 두 wire를 독립 재실행했다. 공식 READ 확장도 단일 생성 binding/SDK에서 source/package × safe/decimal × Node/Python8개 조합을 실행했다. 당시 SDK 경계9개·Rust88개와 실제 child 수명/동시4개 상한·실행 guard8종 음성 대조를 검증했다. 후속 HTTP Origin/파싱5초/연결64개·캐시 전 표준 read 검사·관계 대상 집계·내장 worker를 보완했다. WRITE까지 생성 계약·HTTP·SDK에 연결해 실제8개 source/package/wire/lang 조합·동시 같은 key 단일 실행·pre-commit rollback·실제 COMMIT_UNKNOWN 뒤 재생을 확인했다. 정본 Event.confirm과 bundle의 두 언어 구현을 제공하며 CLI27개·SDK/패키지37개·builder5개·Chrome8개·재배치 bundle8개를 검증했다. CPU loop의 동시4개/5번째 거부·기한·child/row lock 반환과 상한/key lock 음성 대조도 확인했다. DB connect/clock 각각5초·owned driver·SQL operation deadline·init/serve SQL5초와 실제 catalog 구조 대조까지 보완했다. 실제 COMMIT 응답 손실과 다음 retry BEGIN 정지에도 pending/cache unknown을 보존하고 같은 key 재생으로 복구했다. 실행 중 만료된 세션이 이미 커밋한 WRITE의 성공을 실패로 바꾸는 반례도 Node/Python·두 wire에서 수정·검증했다. 정의 숫자 overflow·중첩 abort·캐시의 WRITE 완료 간격·닫힌 stdout/stderr도 보완했다. 최신 Rust175개(V1 포함)·SDK47개·builder5개·WRITE8개·Chrome8개·재배치 bundle8개는 실패0이다. [DB 후속 결과](../prototype/README.md#정의-입력캐시-완료cli-출력-후속-결과)의 marker 호환·검증 범위를 따른다. 최신 명령·범위는 prototype 후속 결과를 따른다. 이 항목은 제품 통합 이전 host의 private 실험 이력이다. 이후 운영 인증·일반 migration·제품 조립의 현재 결과는 위 현재 제품 작업을 따른다. 최종 WRITE API/조합은 남는다.

아리아리 원본과 다르게 동작한 점(코드 읽기 + 실험 비교, 원본은 실행 확인 안 함): 일괄 승인에서 없는 지원 id를 조용히 빼고 나머지만 승인하는 것으로 보임, 권한보다 동아리 검사가 먼저, 회원 중복을 막는 DB 제약 없음, 자기 자신에게 관리자 위임 시 관리자가 사라질 것으로 보임.

**지금 답이 필요한 질문은 없다.** 아래는 기술 비교가 더 진행되면 창시자에게 갈 수 있는 선택이다(아직 질문 아님).
- 권한 때문에 숨긴 필드를 null로 줄지, 빼고 줄지, 요청을 거부할지(V2-R1)
- 쓰기 결과가 불확실할 때(커밋 중 시간 초과) 호출자 경험: 재시도·상태 조회·멱등 키(V3-R6)
- 초기 표준 쓰기에 어느 정도의 호출자 조합 자유를 줄지(V3 §6·§7)
- 확장의 외부 네트워크 허용 범위와 운영 환경 격리 수준(V4 §3)
- Id 공개 표현: 큰 정수 Id를 문자열로 주고받을지, 숫자로 두고 범위를 제한할지(V5 §7 R5-05)
- 화면 데이터의 신선도: 다른 사용자의 변경을 얼마나 빨리 반영할지, 마감 시각처럼 시간이 지나면 바뀌는 조회를 얼마나 낡게 허용할지(V5 §6·§7)

## 현재 상태

요청된 A~E 설계 산출물을 작성했다. 창시자 목적과 기술 상세 승인은 분리했고, 기존 결정 문서의 확인 대기를 답변/검증 상태로 갱신했다. 코드·신규 문법 실행 검증은 이 문서 작업에 포함하지 않았다.

| 읽을 곳 | 현재 내용 |
|---|---|
| [A](alignment/A-founder-intent-matrix.md) | 최초 7원칙·각 FOUNDER 구절의 실제 code/doc 대응·누락 |
| [B](alignment/B-decision-reclassification.md) | 25개 질문·Q0~Q9·14개 의제 재분류, DD별 목적/상세 상태 정본 |
| [C](alignment/C-syntax-proposal.md) | 서버 A/E/H·프론트 read/apply·조합/확장/metadata 후보 |
| [D](alignment/D-development-experience.md) | 아리아리 실제 소스와 10개 변경 시나리오 비교. 절감률/성능 수치 미측정 |
| [E](alignment/E-technical-risks.md) | RK-01~10 반례·초기 실험 순서·V1부터 V15까지 실행 범위와 남은 항목 |
| [독립 대조](reviews/INTEGRATED-codex-luna-r1.md) | Luna high의 의미 누락 3건과 메인 확인/문법 반영 |
| [후속 판단](90-open-questions.md) | 비교 후 남을 출시/DX/호환/라이선스 선택. 이미 답한 25개를 재질문하지 않음 |

확인된 기준: 호출자 읽기·단순 쓰기·풍부한 표준 동작, Rust 자체 포트 서버+공식 프론트 SDK, JS/TS·Python 공식 확장, 선택적 구조화 설명, 선택적 개발 검사와 필수 실행 안전 검사 분리, 오픈소스·외부 기여.

미정 또는 검증 대상: 독립 파일 여부·구체 문법/Typed IR·안전한 쓰기 조합·초기 언어/프론트/서드파티 범위·비용 제어·worker/ABI·DB·호환 기간·라이선스. DIRECTION의 운영 입장·승인·롤백·동기화·오류 정책을 확정으로 승격하지 않았다.

이번 협업: Claude Code는 토큰 소진으로 미참여. Codex 메인이 설계·구현·판정하고 Luna high가 조사·인증 검증·테스트를, Sol high가 설계·동시성·수명 경계 비평을 맡았다. 기존 Claude/Codex 합의는 이전 지침 검토 이력이다.

문서 구조 검증·미실행 범위는 [V0 검증 결과](alignment/V0-validation-results.md)에 남긴다.

### 2026-10-04 밤 자율 작업 (spike 실험, 본 crates 미변경)

| 실험 | 결과 | 독립 검토 |
|---|---|---|
| [V1 의미 fixture](alignment/V1-semantic-fixture-results.md) | `spikes/spike-v1-fixture/`. A·E-TS·E-Py·H-TS·H-Py 5형식이 같은 typed facts. 음성 대조 49건 + r1·r2 반례 + 손으로 쓴 기대 facts 대조. 최신 전체 재실행 17 passed | Codex 6.1-sol r1 "부분 성립"(P1 8건), r2 3건 → 모두 반영·재발 테스트 고정 |
| [V2 호출자 읽기 slice](alignment/V2-caller-read-results.md) | `spikes/spike-v2-read/`. facts만으로 매개변수 SQL 생성, 로컬 PG에서 행·필드·관계·집계 정책, 열린/닫힌 capability, filter/sort 별도 허용, depth/rows/cost/deadline·출력 상한 거부 확인. 5 passed | Codex r2b(실제 DB 재현) 12건 중 읽기 쪽 반영 |
| [V3 표준 쓰기](alignment/V3-standard-write-results.md) | `spikes/spike-v3-write/`. V3-1 알림 읽음·모집 마감(대상·상한·원자성·동시성·권한 의존 행 공유 잠금·기한). V3-2 일괄 승인을 W2(서버 정의 전이)·W0(공개 동작 묶음)·W1(서버 선언 범위 조합)로 같은 반례 표에 비교. 5 passed. V3-3 관리자 위임: 교대형 불변식에 커밋 시점 제약 필요, W0·W1은 순서 민감. W1은 compose에 selfRow를 더해 표현했고 반례 결과가 W0과 같음 | V3-1 Codex r2b, V3-2 Codex r3 반영, V3-3 r4b 발견 없음, W1 selfRow r6 반영 |
| [V4 공식 확장](alignment/V4-official-extension-results.md) | `spikes/spike-v4-worker/`. Rust 서버 + Node/Python worker. ctx 토큰(actor·허용 집계·입력 고정·기한), 출력 검사, 기한, 토큰 폐기, worker 장애. **발견: 환경 변수를 지워도 worker가 로컬 DB에 직접 연결 가능(격리 없음)**. V4-3 쓰기 확장: 서버 트랜잭션 안 공개 전이만, 실패·오류 삼킴·기한·늦은 ctx 모두 rollback, 커밋 결과 미확정 구분 | Codex r4(필터 중단)·r4b·r6 반영 |
| [V5 SDK](alignment/V5-sdk-results.md) | `spikes/spike-v5-sdk/`. facts에서 TS 계약 타입 생성, 화면 select에서 결과 행 타입 추론(관계 포함), 닫힌 필드·허용 밖 filter/sort·값 타입·루트 조회 불가 8가지를 컴파일 오류로. tsc 음성 대조 포함. V5-2 읽기 deps 건전성, V5-3 캐시 + V9 수명(node 10). V12 공개 읽기 식별값·V13 wire·V14 공개 쓰기 projection 대조 포함 최신 Rust 17 passed | V5-1·2 Codex r4b, V5-3 Codex r5 반영 |
| [V6 전송](alignment/V6-transport-results.md) | `spikes/spike-v6-transport/`. Rust 최소 HTTP 서버 + TS 클라이언트(V5 캐시). 쓰기 태그로 캐시 무효화, 응답 유실 시 같은 키 재요청으로 확정, 재실행 없음, 확정 전 캐시 저장 보류. node e2e 7 | Codex r7 반영 |
| [V7 마이그레이션](alignment/V7-migration-results.md) | `spikes/spike-v7-migrate/`. facts 두 버전 비교 → 변경 9종 분류, 행 정책 확대/축소를 회원별 보이는 행으로 측정, 불변식·enum·필수 필드·범위는 기존 데이터 위반 수를 적용 전에 계산. 분류하지 못한 차이는 모두 검토 | Codex r8 반영 |
| [V8 인증](alignment/V8-auth-results.md) | V6에 서버 서명 세션 토큰(HMAC, 기동 시 무작위 키, 만료). 시험용 actor 헤더 제거. 8사례 + 단위. V9에서 입력 크기와 캐시 만료 경계 추가 | Luna high 단위·서버 검증, 메인 반례·보강 |
| [V9 수명·복구](alignment/V9-cache-lifetime-results.md) | V5/V6 재사용. 당시 캐시·복구 Node 17, 실제 서버/DB Node 4. 세션 만료·시간 조회·다른 연결의 변경/권한 회수·같은 키 동시 호출·미확정 복구·입력 상한 | Sol high 설계·독립 비평, Luna high 인증 대조·테스트 작성 |
| [V10 세션 복구](alignment/V10-session-recovery-results.md) | 서버 `/session`, SDK `replaceSession`, 같은 actor pending 복구, 인증 세대 경합, signed -1/익명 분리. 최신 캐시/복구 단위 Node 27, 새 HTTP/DB Node 3 | Sol high 설계·최초 거부 반례, Luna high 테스트·문서 대조, 메인 실제 만료·회귀 검증 |
| [V11 선택적 검사](alignment/V11-optional-checks-results.md) | 기존 V1/V2/V5 재사용. 옵션/metadata/생성 TS/필수 거부 단위 8개·PG 24조합 1개. 고장 주입 3종 검출 | Luna high 단위 실패 대조, Sol high 설계·구현 리뷰, 메인 구현·PG·고장 주입 검증 |
| [V12 계약 타입 연결](alignment/V12-typed-transport-results.md) | V5 generic core·V6 typed facade. 생성 계약 식별값·실제 HTTP/PG·tsc 정상/음성·Node 11개. binding 누락/캐시 주입 반례 수정, 고장 주입 3종 검출 | Luna high 테스트·V5/V11 회귀, Sol high 설계·독립 우회 재현·수정 후 재검증 |
| [V13 큰 Id 왕복](alignment/V13-id-boundary-results.md) | Legacy 잘못된 행 재현·SafeNumber/DecimalString PG/HTTP·첫 apply 계약 거부·모드별 멱등·고장 주입 6종. V2/V3/V5/V6 전체 Rust 12·5·9·10개 실패 0 | Luna high 단위/회귀·범위 대조, Sol high 첫 apply 비평·guard Node 3개 독립 재검증 |
| [V14 공개 쓰기 타입](alignment/V14-typed-apply-results.md) | V5/V6 후보 binding·action/target/where 타입·두 Id mode·PG/HTTP Node 14개씩·형식 오류 pending·실제 결과 동결·고장 주입 6종. 전체 Rust V5 13·V6 11·V11 9개 실패 0 | 메인 테스트/구현/회귀, Sol high 초과 키·결과 변조 재현과 수정 후 독립 tsc/Node 재검증 |
| [V15 공개 필터 실행](alignment/V15-filter-value-results.md) | Url/Enum/Time read·where와 리터럴 prefix, 필수 값 검사·PG 반올림·NUL·타입/지문·권한·7종 고장 주입. 전체 Rust V2 19·V3 5·V5 17·V6 12·V11 9개 실패0 | Luna high 생성기 테스트·범위 검토, Sol high Time/PG·독립 검사, 메인 구현·HTTP/DB·전체 회귀 |

V3-2 관찰(검증 필요): 세 후보 모두 공통 반례(다른 동아리·권한 없음·누락 id·이미 회원·거절됨·동시 승인)를 같은 안전한 결과로 막았다. W0은 커밋 검사 없이는 지원하지 않은 회원 생성이 가능했고, W1은 허용 단계·값 출처·생성 allow(역할 제한 포함)·사후조건 두 개를 선언해야 W2와 같은 업무를 지켰다. 이 fixture의 선언 구성에서는 W1이 W2보다 서버 정의를 줄이지 못했다(11줄 vs 8줄). W1이 항상 불리하다는 근거는 아니며, 관리자 위임은 이후 V3-3에서 실행했다. 순환 전이와 다른 업무에서 조합의 장점이 유지되는지는 남은 비교다.

V13은 큰 Id의 두 후보를 실제 왕복으로 비교했다. 적용은 V6 HTTP read/apply와 후속 공식 READ worker이며 bundle/compose/WRITE worker·최종 표현 선택·일반 Int 정밀도는 남는다. V14는 direct 공개 전이의 생성 타입·응답 검사·쓰기 식별값을 실행했다. 후속 [V15 공개 필터 값 정합](alignment/V15-filter-value-results.md)은 Url/Enum/Time read/where·Text.prefix를 연결했다. [프로토타입](../prototype/README.md)은 초기화→생성 SDK→서버 호출→공개 정의 변경·재기동을 한 실행 흐름으로 검증했다. SDK tarball의 새 소비자 설치와 현재 macOS host의 재배치 bundle은 검증했으나 전체 제품 clean install, create/compose 값 타입·WRITE 확장 통합 등의 공백은 남는다. prototype 표준 read의 공개 선택 행·관계 decoder는 캐시 저장 전에 연결했다. 모든 값/API의 최종 domain을 확정한 것은 아니다. 공식 READ는 생성 타입/descriptor·서명 세션·필수 지문·worker 값/ctx 검사로 연결했다. 시간 의존 조회의 명시적 유효 시각, 다른 연결 변경 통보·재연결 복구, 운영 환경 worker 격리 비교도 남는다. V10은 토큰 교체 후 복구만 검증했으며 로그인·refresh-token 제공자와 브라우저 재시작 뒤 영속 복구는 남는다. V11은 직접 참조 부재 조언 한 가지와 resource docs만 검증했으며 제품 검사 전체·설명 소속 전체는 남는다. 기존 독립 검토·쓰기 확장·SDK 작업은 위 결과 문서를 따른다.

이 실험들은 문법·planner·쓰기 모델 결정이 아니다. spike가 임의로 정한 규칙(V1-R*, V2-R*, V3-R*, V4-R*)은 각 결과 문서에 기술 후보로 적었다.

## 이전 진행 현황과 로그

아래 합의/질문 수·사용량·자료 요청·아리아리 결함은 당시 작업 이력이다. 현재 대기 상태·실행 검증 결과로 재사용하지 않는다.

<details>
<summary>통합 지침 이전 진행 기록 보기</summary>

> 창시자가 한 번에 확인하는 문서. 위에서부터 읽으면 된다. 마지막 갱신 시각은 각 절 머리에 적는다.
> 작업 방식: 결정 문서마다 Claude 초안 → Codex(gpt-5.6-sol) 비판 → Claude 반영 → 합의까지 반복. Codex가 연결되지 않으면 Claude Sonnet 서브에이전트로 대체한다.

## 0. 저녁에 먼저 읽을 것 (2026-10-03 19시 기준)

1. **결정 문서 14개를 만들었고, 14개 모두 에이전트 간 합의에 도달했다.** 문서 간 교차 정합성 검토도 1회 거쳤다(P0 0, P1 14건 반영). 합의는 Claude와 비판자(Codex gpt-5.6-sol 또는 Claude Sonnet) 사이의 합의일 뿐이고, 결정 상태는 모두 미결정이다. 창시자 답이 있어야 확정된다.
2. **답해 주실 질문은 25개.** `90-open-questions.md` 맨 위 표에 순서대로 있다. 핵심은 앞의 1~7번(읽기·쓰기 모델)이고, 8~13번(작성 방식, 자연어 문법화)은 예시를 보고 고르게 돼 있다.
3. **원본 자료 요청 3건**: "최초 7가지 원칙" 원문, A형·B형 문법 예시, "자연어의 문법화" 설명(예시 문장 하나면 충분).
4. **아리아리 코드에서 찾은 결함(별도 확인 권장, 모두 코드 읽기 추정, 실행 확인 안 함)**:
   - 서비스 공지·FAQ 등록·수정·삭제 8개 API에 역할 검사가 없음(`SystemNoticeService`, `SystemFaqService`에 "검증 로직 추가 필요" 주석). 설명은 "운영 관리자만"
   - 진행 중 모집 조회·모집 존재 여부 조회에 학교 가시성 검사 없음
   - 조회수 일별 집계 키에 현재 조회수 값이 들어가 "최근 14일 조회수"가 의도대로 계산되지 않을 가능성(`ViewsManager.java` 87행)
   - 지원자 일괄 승인에서 권한 검사보다 동아리 검사가 먼저라 응답 차이로 id 존재를 추측할 수 있음
   - 오류 응답: 커스텀 예외는 HTTP 400 고정인데 본문 code는 403·404 등, DB 오류는 SQL 오류 메시지를 클라이언트에 그대로 돌려줌(`ExceptionControllerAdvice.java`)
5. 토론 기록은 `reviews/`에 문서별·라운드별로 남아 있다. 각 결정 문서 변경 이력에 무엇이 왜 바뀌었는지 적혀 있다.

## 1. 한눈에 보기

| 문서 | 주제 | 상태 | 창시자 질문 |
|---|---|---|---|
| DD-01 | 화면이 새 데이터를 원할 때 서버에 조회를 추가해야 하나(읽기 조합) | 에이전트 합의(3라운드) | F1~F4 |
| DD-02 | 쓰기에서 호출자는 무엇을 표현하고 서버는 무엇을 쥐나 | 에이전트 합의(3라운드) | F1·F2 |
| DD-03 | 조합 읽기를 운영 서버가 언제부터 받아 주나 | 에이전트 합의(3라운드) | F1 |
| DD-04 | 서버 쪽 정의를 무엇으로 쓰나(`.aip`·TS·Python) | 에이전트 합의(Codex 3라운드) | F1~F3 |
| DD-05 | "자연어의 문법화"가 무엇인가 | 에이전트 합의(Sonnet 3라운드) | F1~F3 (예시 먼저 → 원하는 효과 → 실행 영향) |
| DD-06 | Rust 엔진과 JS/Python 앱의 관계(독립 서버 / 임베드 / 설치 도구가 서버 자동 기동) | 에이전트 합의(Sonnet 2라운드) | F1~F3 (배포 환경 → 엔진 위치 → 확장 작성자) |
| DD-08 | 호출자 요청을 어떤 형태로 보내나 | 에이전트 합의(Sonnet 2라운드). 추천: JSON 트리 + 값 분리 | 없음(기술 제안) |
| DD-09 | 상세를 열면 올라가는 익명 통계(조회수) | 에이전트 합의(Sonnet 2라운드). 열람 지점 선언 방식은 DD-01과 함께 미결정 | F1 |
| DD-10 | "내가 북마크했나" 같은 보는 사람 기준 값 | 에이전트 합의(Sonnet 3라운드) | 없음(DD-01 F1·DD-04에 종속) |
| DD-11 | 쓰기 뒤 화면의 어떤 데이터를 다시 불러올지(프론트 캐시 무효화) | 에이전트 합의(Sonnet 3라운드). 추천: 읽기·쓰기 양쪽 태그 | F1(남의 변경이 얼마나 빨리 보여야 하나) |
| DD-12 | 서버 정의가 바뀔 때 무엇을 사람이 확인한 뒤 반영하나(스키마 진화·롤백) | 에이전트 합의(Sonnet 3라운드) | F1~F3 |
| DD-13 | 프론트 SDK가 무엇을 맡나 | 에이전트 합의(Sonnet 3라운드) | F1(라이브러리 범위: 데이터만 / UI 조각까지), F2(첫 출시 지원 프레임워크) |
| DD-14 | 응답과 오류의 모양 | 에이전트 합의(Sonnet 2라운드) | F1(권한 부족을 얼마나 알릴지) |
| DD-07 | 여러 건을 한 번에 처리하다 일부가 실패하면 | 에이전트 합의(Sonnet 2라운드). 추천: 전부 취소 + 안 되는 항목 한 번에 모두 알림 | F1 |

상태 어휘: 확정(창시자 승인) / 잠정 합의 / 검증 필요 / 미결정. "에이전트 합의"는 Claude와 Codex 사이의 합의일 뿐 결정 상태는 모두 **미결정**이다.

## 2. 쌓인 질문 (답하기 쉬운 순서)

`90-open-questions.md` 맨 위 표가 정본이다. 새 질문은 거기에 추가하고 여기에는 요약만 둔다.

- 총 25개: DD-01 F1~F4, DD-02 F1·F2, DD-03 F1, DD-04 F1~F3, DD-05 F1~F3, DD-06 F1~F3, DD-07 F1, DD-09 F1, DD-11 F1, DD-12 F1~F3, DD-13 F1·F2, DD-14 F1. `90-open-questions.md` 맨 위 표에 순서대로
- 원본 자료 요청: Q0 "최초 7가지 원칙" 원문, Q3 A형·B형 문법 예시, Q5 "자연어의 문법화" 설명

## 3. 진행 로그

- 2026-10-03 오후: DD-01~03 각각 Codex와 3라운드 토론, 에이전트 합의. 질문 7개 정리
- 2026-10-03 오후: 창시자 외출. 자율 진행 시작(다음 결정 문서 DD-04부터)
- 15:30 DD-04·05·07 초안. DD-04는 Codex 1라운드. Codex 사용량 5시간 한도 31% 남음(19:27 리셋) → DD-07부터 Sonnet 서브에이전트 병행
- DD-05 실험 E1: 아리아리 API 설명 문장 중 권한 규칙 36개, 그중 12개 코드 대조 → 불일치 2, 예외 누락 1, 강제 불완전 1, "관리자" 용어 모호
- DD-04 Codex 1라운드: 초안 추천(TS로 쓰고 `.aip`는 읽기 표기)이 사실상 고유 문법을 쓰는 형식에서 없애는 것이라는 지적, 예시가 같은 내용이 아니었음, TS 정의 실행 시 보안 경계 누락 → v2에서 추천 철회, 같은 9개 항목을 담은 예시 4개, "TS/Python 프로젝트 안 AIP 선언 블록"을 1순위 검증 후보로. Codex 사용량 24% 남음
- DD-07 Sonnet 1라운드: 사전 점검(U3)은 점검과 실행 사이 상태 변화로 같은 실패가 남고, 점검 자체가 정보 노출 통로가 된다는 지적 + 아리아리 코드의 권한 검사 앞 동아리 검사 오라클 발견 → v2 추천 변경
- DD-04 Codex 2라운드: 예시에 필드·간선 정책 누락, 확장 참조 규칙 충돌, F1·F2 모순 가능 → v3. Codex 사용량 18% 남음, 이후 문서는 Sonnet으로
- DD-01 실험 E1: 아리아리 모집 GET 11개 모두 수정 대안 2로 표현 가능. 새 요구 2개 발견(읽기에 딸린 조회수 증가, "내 동아리인가" 같은 행위자 기준 값) → DD-09·DD-10 예정. 원본 코드에서 학교 가시성 검사가 빠진 조회 2곳 발견(존재 여부, 진행 중 단건. 코드 읽기 추정)
- **중요 발견(DD-05 Sonnet 대조, 코드 확인)**: 아리아리 서비스 공지·FAQ 8개 API가 설명은 "운영 관리자만"인데 서비스 코드에 역할 검사가 없고 `// 검증 로직 추가 필요` 주석만 있음. 로그인 회원이면 서비스 공지 등록 가능으로 보임(실행 확인 안 함, 프론트·게이트웨이 보호 여부 확인 필요). 권한 설명 36개 중 불일치 13개. 아리아리 운영 중이라면 별도로 확인 권장
- DD-09 Sonnet 1라운드: 아리아리 조회수 일별 키에 현재 조회수 값이 들어가 14일 창이 의도대로 동작하지 않을 가능성(`ViewsManager.java` 87행, 코드 확인), PoC는 창 집계 미구현·중복 제거 키가 호출자 헤더 → v2(선언한 상세 형태만 집계, 서버 파생 키, 식별 값 해시)
- Codex가 DD-11 실행 전 사용량 한도에 걸림(화면 안내: 19:27 이후 재시도). 한도 안내 창에서 모델이 `GPT-6-Luna medium`으로 바뀐 상태로 보임(확인 필요). Codex에 gpt-6 계열이 있다는 것이 드러남 → 처음 말씀하신 "6.1sol"이 gpt-6 계열이었을 가능성(확인 필요). DD-11은 Sonnet으로 진행
- DD-11 Sonnet 1라운드: 아리아리 무효화 대조 26경로 중 빠짐 7·과잉 1, 모집 마감 시 프론트가 "CLOSED"를 직접 써 넣는 등 서버 파생 상태 재구현 → v2(읽기 의존 태그 + 쓰기 변경 태그, 동기 경로 한정 명시)
- 교차 정합성 검토(`reviews/CROSS-sonnet-r1.md`): P0 없음, P1 14건 반영. 확장 추가는 사람 확인 대상에서 제외(원문 승인 절차 금지), 확장 우회 방지 불변식의 보장 방식이 DD-06 답에 종속됨을 명시, 저장된 조회와 등록된 요청의 관계 정의, 질문 번호 체계 정리, 클라이언트 SDK 담당 문서(DD-13) 신설
- DD-14 Sonnet 1라운드: PoC도 DB 오류 원문을 응답에 넣고(`exec.rs` db.message()), 다른 테넌트 행은 본문 코드가 NOT_FOUND와 달라 존재 추측 가능(`codes.rs` TENANT_MISMATCH) 확인 → v2(존재 비노출 세 층, 운영에 개발 모드 없음, 문구는 코드+매개변수). PoC 부채로 기록
- 2차 교차 검토(`reviews/CROSS-sonnet-r2.md`): P0 0, P1 4 반영(호출자 요청 작성 API 담당을 DD-13으로, DD-13 오류 분류와 DD-14 category 일치, validUntil 정의 연결, 응답 형식 후속 문서를 DD-14로). 1차 P1 14건 반영 확인

</details>

## 최신 변경 이력

- 2026-10-03 창시자 통합 지침의 기준·산출물·미정 범위·실제 협업을 표시하고 이전 기록을 이력으로 전환.
