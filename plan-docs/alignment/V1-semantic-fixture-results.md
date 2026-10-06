# V1. 의미 fixture 실험 결과

> 상태: 검증 필요 (2026-10-04, Codex r1 반영).  [E §4](E-technical-risks.md#4-작은-실험의-선후-관계) V1의 실행 기록이다. 문법·작성 방식 결정이 아니며, 창시자 승인 항목을 닫지 않는다.
> 위치: `../../spikes/spike-v1-fixture/` (독립 crate, 본 `crates/` 미변경, 외부 의존은 캐시된 `serde_json`·`sha2`만).

## 실제 실행 출력

명령: `cd spikes/spike-v1-fixture && cargo test --offline -q`

```text
negative controls: 49 cases
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
```

두 번째 줄은 `tests/v1_r1.rs`(Codex r1 반례 재발 방지 + 독립 기대 facts)다.

명령: `for f in fixture/recruitment.*; do ./target/debug/spike-v1-fixture check $f; done`

```text
ok A exec=17612191e77a3a1c meta=c765763dff76324c
ok EPy exec=17612191e77a3a1c meta=c765763dff76324c
ok ETs exec=17612191e77a3a1c meta=c765763dff76324c
ok HPy exec=17612191e77a3a1c meta=c765763dff76324c
ok HTs exec=17612191e77a3a1c meta=c765763dff76324c
```

테스트가 실패할 수 있는지 먼저 확인했다. 기대 코드를 틀리게 바꾼 음성 대조, digest를 바꾸지 않는 변형, H(Python) fixture의 cost 값 변조를 각각 넣었을 때 해당 테스트 3개가 실패했고 복원 후 7개 모두 통과했다.

## 1. 무엇을 만들었나

| 구성 | 파일 | 줄 수 | 역할 |
|---|---|---|---|
| A fixture | `fixture/recruitment.aip` | 97 | C B.1 네 resource 원문 그대로 + 빠진 기호 선언 |
| E fixture | `recruitment.e.ts`, `.e.py` | 101 / 101 | A 전체를 TS tagged template, Python 삼중 따옴표에 그대로 넣음 |
| H fixture | `recruitment.h.ts`, `.h.py` | 93 / 93 | C B.3 정형 데이터 + 같은 추가 선언. 손으로 작성 |
| lexer·parser | `src/lexer.rs`, `parser.rs` | 114 / 598 | A 문법. H의 정책·타입 문자열도 같은 parser로 읽음 |
| 의미 검사 | `src/sema.rs` | 854 | 기호 해석·타입 검사·typed facts 생성 |
| E 추출기 | `src/extract.rs`, `src/host.rs` | r1 후 갱신 | 모듈 실행 없이 공식 바인딩 리터럴 하나만 읽음 |
| H 추출·매핑 | `src/hlit.rs`, `hmap.rs` | 203 / 326 | 리터럴 부분집합 parser + AST 매핑 |
| 테스트 | `tests/v1.rs` | 226 | 동등성·EQ 사실·민감도·불변성·docs·음성 대조 49건·E 진단 위치 |

출력은 두 덩어리다. `execution`(권한·SQL·업무 규칙에 쓰이는 사실)과 `metadata`(docs). 각각 SHA-256 digest를 낸다. source span은 둘 다에서 빼고 별도 `spans`로 둔다.

## 2. 결과

| 확인 항목 | 결과 | 근거 |
|---|---|---|
| A·E-TS·E-Py·H-TS·H-Py 같은 typed facts | 같음. execution·metadata digest 모두 일치 | `eq_all_forms_same_typed_facts` |
| 동등성이 공허하지 않음 | 형식별 의미 변경 7건이 모두 digest를 바꾸거나 거부됨 | `sensitivity_changes_change_execution_digest` |
| EQ-01~11이 facts에 실림 | EQ마다 대표 사실을 단언 | `eq_facts_cover_eq01_to_eq11` |
| 공백·주석·허용목록 순서·H 키 순서 | execution digest 불변 | `invariance_whitespace_comments_and_allowlist_order` |
| docs 변경·삭제(SY-5) | 5형식 모두 execution 불변, metadata 변경 | `docs_change_or_removal_keeps_execution_digest` |
| 잘못된 기호·타입·연산 | 26건 기대 코드로 거부 | `negative_controls_…` A 행 |
| E 동적 평가 | `${}`·f-string·결합·`.format`·별칭·escape·두 번째 블록 거부(10건) | 같은 테스트 E 행 |
| H 실행 가능 값 | 변수·spread·template·arrow·계산 키·실수·모르는 키 거부(13건) | 같은 테스트 H 행 |
| E 진단 위치 | 호스트 파일 줄 번호로 보고 | `e_diagnostics_point_into_host_file` |

## 3. fixture를 완전하게 만들면서 드러난 것

C B.1만으로는 typed facts를 만들 수 없었다. 아래 선언 23줄을 더해야 했다. 모두 spike 가정이며 문법 확정이 아니다.

| 빠진 것 | 추가한 선언 | 의미 |
|---|---|---|
| enum 값 | `enum RecruitmentStatus/ApplyStatus/ClubRole` | `PUBLISHED`, `APPROVE`를 타입 있는 값으로 해석 |
| actor 타입 | `actor Member` | `actor.school` 경로 타입 |
| 참조 resource | `School`, `Member`, `ClubMember` | 관계 끝과 managerOf의 근거 행 |
| `active`, `managerOf` | `predicate … = <식>` | 자유 자연어가 아니라 타입 검사되는 서버 기호(C B.1 요구) |
| sourceAccess 기호 | `access fixedTotalOfVisibleRecruitment = totalOfVisible(Recruitment)`, `access clubManagerOnly(a, c) = managerOf(a, c)` | 집계가 원본 행 정책을 자동으로 물려받지 않음 |
| invariant 실행식 | `limit atMostOnePublished on Recruitment = atMost 1 where status = PUBLISHED` | 정의가 없으면 `UNKNOWN_SYMBOL`로 실패(EQ-09) |

## 4. spike가 임의로 정한 의미 규칙

다음은 facts를 만들기 위해 spike가 정한 규칙이다. 기술 제안이고 검토 대상이다.

| ID | 규칙 | 이유 | 열린 점 |
|---|---|---|---|
| V1-R1 | 행 정책이 없는 resource는 `denyAll` | 정책 누락이 곧 노출이 되지 않게 | 신규 필드 기본 비공개 [DIRECTION]과 같은 방향. 확정 아님 |
| V1-R2 | budget 없는 `expose read`는 traverse 대상 전용(`rootQueryable: false`) | C B.1 Club에 budget이 없음. 무제한 기본값을 두지 않음 | Club 직접 조회가 필요하면 budget을 써야 함 |
| V1-R3 | 정책 안 `exists`는 대상 resource의 행 정책 없이 평가(`evaluatedAs: serverPolicy`) | managerOf가 ClubMember 행 정책(본인만)에 막히면 의미가 바뀜 | 정책 정의자가 다른 resource를 시스템 권한으로 읽는다는 사실을 문서화해야 함 |
| V1-R4 | `atMost 1 per <참조>` → partial unique index, 2 이상 → 잠금 count 검사 | DB 집행 가능한 사실로 남김 | 실제 DDL·잠금은 V3에서 실행 검증 |
| V1-R5 | `Ref<R>`과 `Id<R>`은 비교·인자에서 호환 | `managerOf(a, input.clubId)` | 암묵 변환을 허용할지 명시 `.id`를 요구할지 |
| V1-R6 | select/filter/sort 허용목록은 집합(순서 무의미) | 허용 여부 목록이지 정렬 우선순위가 아님 | 호출자 요청의 정렬 순서는 별도로 보존해야 함 |
| V1-R7 | traverse select는 대상의 expose select 부분집합이어야 함 | 관계를 타고 닫힌 필드를 여는 경로 차단(EQ-04) | 대상 정책 재적용 자체는 V2에서 SQL로 검증 |
| V1-R8 | filter 연산: Time/Int는 비교, Text는 eq/prefix, 참조는 불가 | 타입별 허용 연산 고정 | 연산 집합은 후보 |

## 5. 형식별 비용 관찰

- E는 A와 같은 parser를 쓰고 추출기 143줄이 추가됐다. 대신 블록 안 escape 금지, 파일당 블록 하나, `aip` 별칭 금지 같은 제약이 생겼다. 블록 안 자동완성·호스트 타입 검사는 없다.
- H는 리터럴 parser 203줄 + 매핑 326줄이 추가됐다. 정책·타입·filter·invariant는 결국 A 문법을 문자열에 담은 것이라 호스트 언어가 그 내용을 검사하지 않는다. predicate 매개변수 순서가 객체 키 순서에 의존한다.
- H 문자열 안 오류는 줄은 맞지만 열 위치가 문자열 시작 기준으로 다시 1부터 센다(확인 필요: 테스트로 고정하지 않음).
- 이 수치는 이 spike의 구현 비용이다. 에디터 지원·source map·Python 동등 표현의 실제 DX(SY-7)는 측정하지 않았다.

## 6. 하지 않은 것

- DB 실행 없음. partial unique index·정책 SQL은 facts로만 존재한다(V2·V3).
- 같은 digest는 이 spike가 정한 facts 정규화 기준의 동등성이다. 언어 일반의 의미 동등성 증명이 아니다.
- 호출자 요청·SDK·planner·worker 미구현.
- SY-2·SY-3·SY-4·SY-6·SY-7 미실행. SY-1·SY-5는 이 fixture 범위에서만 실행.

## 7. 독립 검토와 반영

Codex(gpt-6.1-sol, high) 비판 검토: [codex-v1-r1](../reviews/codex-v1-r1.md). 판정은 "V1 부분 성립". 최소 파싱·의미 출력, 5형식 일치, 음성 대조 49건, resource docs 분리에는 동의했다. 잘못된 기호·동적 평가를 일반적으로 거부한다는 완료 주장에는 반대했고 실제 반례를 냈다. 반례마다 코드로 재현을 확인한 뒤 고쳤다.

| 발견 | 내용 | 반영 |
|---|---|---|
| F01 P1 | 주석·문자열 안 `aip`/`define` 블록을 정의로 채택 | `src/host.rs` 호스트 토크나이저(주석·문자열·template 인식)로 교체 |
| F02 P1 | 다른 패키지 import, 숨은 두 번째 블록 승인 | 공식 import(`@aip/define`, `aip.define`) 정확히 1회 + export 선언 위치 강제. 위반 `WRONG_BINDING`/`NON_LITERAL`/`MULTIPLE_BLOCKS` |
| F03·F07 P2 | 다음 줄 `+ extra`, `define(...)(execute())` 같은 가공 승인 | 블록 뒤 토큰 검사. TS는 문장 키워드만 다음 줄 허용 |
| F04 P1 | 확장 입출력·집계 타입의 범위 제약이 facts에서 사라짐 | params는 `[이름, 타입, 범위]`, 집계는 `tyRange` 보존 |
| F05 P2 | `in` 원소 순서, `x.id`와 `x`가 다른 digest | `in`은 집합 정규화, `x.id`는 `x`로 접음(V1-R5). enum 선언 순서는 보존(서수 의미 미정) |
| F06 P1 | H totalOfVisible이 params를 조용히 버림 | `H_SHAPE` 거부 |
| F08 P2 | H predicate params가 객체 키 순서 ABI | 기록. 이름 인자 또는 배열 params는 다음 후보 |
| F09 P1 | 중복 정책·from·budget 키가 덮어써짐 | 블록 키·이름 중복 `DUPLICATE`. 집계와 필드 이름 충돌도 거부 |
| F10 P1 | nullable 값을 필수 필드에 대입, 역전 범위, budget 0 승인 | `TYPE_MISMATCH`, `BAD_RANGE`, `BAD_BUDGET` |
| F11 P1 | 자기 재귀 predicate 승인 | 호출 그래프 순환 `POLICY_CYCLE` |
| F12 P2 | `now` 의존 조건도 partial unique index로 표시 | 자기 행 직접 열과 상수 비교만 인덱스, 나머지는 `lockedCountCheck`(V3) |
| F13 P2 | H 오류 열·E 첫 줄 열 부정확 | lexer에 열 기준 추가, H 값 위치 기록. H·E-Py 오류 줄·열 테스트 |
| F14 P2 | CLI가 모르는 인수를 무시 | 인수 개수·명령 엄격 검사(exit 2). optional 검사 설정 표면은 여전히 없음(SY-6 미검증) |
| F15 P2 | spike 규칙 5개는 기술 후보 | §4 표에 실험 규칙으로 유지. 창시자 질문 대상 아님 |
| F16 P1 | 공유 sema끼리 일치만으로 누락 못 잡음 | `r1_independent_expected_facts`: C 문장에서 손으로 옮긴 기대 facts 12개 구획과 전체 비교 |

r2([codex-v1v2-r2](../reviews/codex-v1v2-r2.md), 콘텐츠 필터로 중단 전 3건): R2-01 Python raw 문자열 안 `\'`를 닫는 따옴표로 오인(P1) → 경계 판정에서 백슬래시 다음 글자를 항상 건너뜀. R2-02 매개변수·입출력 이름 중복(P1) → `DUPLICATE`. R2-03 65개 predicate 순환이 탐색 한도 밖(P2) → 방문 집합으로 한도 없이 검사. `tests/v1_r2.rs`에 고정했고 r2b가 원래 반례로 재확인했다.

반영 후 재실행 결과는 위 실행 출력이다. r1 반례 35사례와 양성 대조 3사례(정상 호스트 문자열·주석·멤버 이름에 같은 단어)를 `tests/v1_r1.rs`에 고정했다. 반영 결과의 재검토는 [codex-v1v2-r2](../reviews/codex-v1v2-r2.md)에서 한다.

## 변경 이력

- 2026-10-04 V1 spike 실행 결과 초안.
- 2026-10-04 Codex r1 반영: 호스트 토크나이저, 중복·범위·순환 검사, 정규화, 독립 기대 facts.
