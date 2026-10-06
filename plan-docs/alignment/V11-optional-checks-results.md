# V11. 선택적 개발 검사와 필수 실행 검사 분리 결과

> 상태: 독립 실험 검증, 기술 후보(2026-10-04). [계획·8개 설계 관문](V11-optional-checks-plan.md), [독립 검토](../reviews/codex-v11-sol-review.md). RK-07의 구체 분리 후보를 검증했으며 최종 문법·제품 lint 전체를 확정하지 않는다.
> 위치: `spikes/spike-v11-dev-checks/`. 기존 V1/V2/V5를 재사용하며 본 `crates/`는 보존했다.

## 개발자가 사용하는 경계

로컬 서버 정의 도구의 `load_checked(src, form, options)`는 V1의 구조·의미 검사를 항상 실행한다. 옵션은 `{}` 또는 `{"devChecks":false}`가 기본 비실행, `{"devChecks":true}`가 개발 조언 실행이다. 성공 출력은 V1 `Output`과 별도 `advisories`다. 조언이 있어도 계약 생성·정상 요청 실행을 막지 않는다.

검사 옵션은 호출자의 runtime 요청에 추가하는 값이 아니다. raw read에 `devChecks:false`를 보내면 `UNKNOWN_KEY`다. 서버 planner/executor는 `Output.execution`만 받고, 옵션이나 조언의 승인 결과를 입력받지 않는다.

현재 조언은 **typed execution에 직접 call 참조가 없는 predicate** 한 가지다. 설명 문자열의 이름 출현은 호출로 세지 않는다. 다른 미도달 predicate가 호출한 대상은 이 국소 검사에서 조언하지 않으며 전체 미도달 코드 분석을 주장하지 않는다.

## 확인한 동작

| 경계 | 대조와 결과 |
|---|---|
| 옵션의 실제 효과 | 같은 고립 predicate에서 기본/명시 off는 조언 0, on은 `predicate:orphan` 조언 1. 실행 digest·V5 생성 TS 동일 |
| 5개 작성 형식 | A·E-TS·E-Py·H-TS·H-Py의 기존 facts와 생성 TS가 옵션 변경 전후 동일 |
| 설명 선택성 | 5형식에서 docs 제거·내용 변경·빈 줄 위치 변경. execution과 생성 TS 불변, metadata 값/위치 변화는 별도 보존 |
| 설명 귀속 | A형 resource Recruitment의 docs를 Club으로 이동. metadata anchor만 Club으로 바뀌며 execution·생성 TS는 동일 |
| 자연어와 실행 | '모든 사용자와 익명이 내부 메모까지 읽을 수 있음'이라는 설명으로 바꿔도 실제 PG의 권한이 넓어지지 않음 |
| 옵션·설명 구조 오류 | 모르는 옵션은 `UNKNOWN_OPTION`, root null/배열/문자열 또는 잘못된 devChecks 타입은 `BAD_OPTION`. 모르는 docs 키도 명시 오류 |
| 필수 정의 검사 | 없는 field 타입은 default/off/on 모두 `UNKNOWN_TYPE`. 개발 조언을 끄는 것이 parser/sema를 생략하지 않음 |
| 필수 요청 검사 | 닫힌 select, 비공개 filter, 닫힌 filter 연산·sort, 값 타입, rows/depth/cost 오류 8사례가 off/on 동일 거부 |
| 실제 DB 정책 | docs 원문/없음/거짓 권한 설명 × off/on × 관리자/같은 학교 일반 회원/다른 학교 회원/익명, 24조합. 행 가시성과 관리자 메모 마스킹 동일 |
| 조언이 있는 정상 실행 | PG fixture에 고립 predicate를 추가하여 on에서는 실제 조언이 발생하지만 정상 조회 24조합은 유지. 종료 시 전용 schema 삭제 |

metadata 소속 검증은 문법상 실제 소속을 보존하는 대조다. 작성자가 어느 선언을 의도했는지, 자연어가 참인지까지 자동 판정하지 않는다. V1의 resource docs에는 source span이 있지만 predicate 조언은 이름 anchor만 제공하며 가짜 줄/열 위치를 만들지 않는다.

## 실패 대조와 실행 근거

기능 구현 전 V1만 호출하고 옵션을 무시하는 baseline에서 단위 7개 중 5개는 통과했고, 옵션 오류와 on 조언 2개는 예상대로 실패했다. 이후 옵션 검증과 읽기 전용 조언을 구현했다. Sol의 독립 대조로 빠진 private filter 반례를 확인하고, filter 연산·sort까지 추가했다.

다음 고장을 각각 주입했을 때 테스트 실패를 확인한 뒤 원본을 복원했다.

1. on이어도 조언을 생략: 고립 predicate 대조 실패.
2. 기본 옵션을 on으로 변경: 고립 predicate의 기본 off 대조 실패.
3. 조언이 있으면 정상 계약을 오류로 차단: PG 실행 대조 실패. schema 정리 후 실패를 보고.

```bash
/Users/winterholic/.cargo/bin/cargo test --offline --manifest-path spikes/spike-v11-dev-checks/Cargo.toml -- --nocapture
# optional_checks: 8 passed; runtime: 1 passed
# runtime policy cases: 24; schema removed: aip_v11
/Users/winterholic/.cargo/bin/cargo fmt --manifest-path spikes/spike-v11-dev-checks/Cargo.toml --check
/Users/winterholic/.cargo/bin/cargo clippy --offline --manifest-path spikes/spike-v11-dev-checks/Cargo.toml --all-targets -- -D warnings
# exit 0
```

의존 실험의 전체 cargo 회귀도 실행했다. V1은 17개, V2는 5개, V5는 4개가 실패 0건이었다. V5 타입 음성 대조는 의도한 컴파일 오류 검출을 포함한다. 새로운 runtime 테스트는 transport 모사가 아니라 서버에서 재사용하는 V2 planner/executor와 실제 PG를 호출한다. V6 wire 형식은 바꾸지 않았다.

## 남은 범위

- 개발 조언은 한 가지다. 정의 참조·중복·복잡도·표준 기능 우회·자동 수정 등 전체 제품 검사 범위와 기본값은 미정이다.
- docs는 기존 V1의 resource 수준 summary/visibility에 한정한다. command/field/expression별 설명, rationale, 전체 source map은 아직 구현하지 않았다.
- 자연어 의미 검사나 LLM 기반 실행 판단은 넣지 않았다. 잘못된 설명을 보안 계약으로 믿지 않는 경계만 대조했다.
- 구조·타입 오류는 실행할 수 없는 계약의 필수 진단이다. 별도 조언 off와 필수 서버 검사의 우회는 다른 기능이다.
- 최종 CLI/editor/API, metadata 공개 범위, 본 구현 통합, TS/Python 작성 경험과 생산성 개선 측정은 남는다.

원본 baseline은 `/var/folders/7_/5w7y5vq9329g81pk_fv8_mhh0000gn/T/aip-v11-baseline-07c0azkv`, 고장 주입 로그는 `/var/folders/7_/5w7y5vq9329g81pk_fv8_mhh0000gn/T/aip-v11-mutations-_iiw2j9g`에 보존했다.
