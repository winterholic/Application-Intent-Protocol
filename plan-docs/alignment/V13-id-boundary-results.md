# V13. 큰 Id의 손실 없는 왕복 비교 결과

> 상태: 독립 실험 검증, 기술 후보(2026-10-04). [계획·8개 설계 관문](V13-id-boundary-plan.md), [독립 검토](../reviews/codex-v13-sol-review.md). 숫자 안전범위 제한과 십진 문자열 wire를 실제 PG·HTTP·TS에서 비교했다. 본 `crates/`와 최종 공개 Id 표현은 변경하지 않았다.

## 관측한 문제와 두 후보

기존 Legacy 서버가 Id `9007199254740993`을 JSON 숫자로 보내면 JS는 `9007199254740992`로 읽는다. 이 값을 문자열로 바꿔 기존 apply에 보내면 이웃 행을 변경한다. 실제 DB에서 해당 이웃 행만 변경된 것을 단언했다. Legacy 기본 동작을 임의로 바꾸지 않고, 서버 기동 설정으로 두 독립 후보를 비교했다. 요청 본문은 모드를 선택하지 않는다.

| 경계 | Legacy 대조 | SafeNumber 후보 | DecimalString 후보 |
|---|---|---|---|
| Id/Ref 출력 | JSON number | 비음수·JS 안전 정수 범위 number | 비음수 i64의 정규 십진 문자열 |
| Id 입력 | 최대 18자리 숫자 문자열, 앞자리 0 허용 | JSON integer, `0`부터 `9007199254740991` | `"0"` 또는 0으로 시작하지 않는 숫자 문자열, i64 상한까지 |
| 큰 Id | 반올림과 잘못된 변경 반례 재현 | 서버가 `ID_OUT_OF_RANGE`로 거부 | 원래 행으로 정확하게 왕복 |
| 호출자 변환 | read 후 문자열 변환도 정밀도를 복구하지 못함 | 읽은 number를 그대로 filter/apply에 전달 | 읽은 string을 그대로 filter/apply에 전달 |
| domain 부담 | 기존 입력/출력 불일치 | 기존 DB에서 표현 가능한 큰 Id를 사용하지 못함 | 입력·출력·생성 TS 타입 변경 필요 |

PG 숫자 JSON은 Rust에서 i64로 보존된다. V2의 계획 출력 타입을 이용해 JS가 파싱하기 전에 루트·Ref·중첩 관계의 Id만 변환한다. nullable 관계는 null을 유지하고 일반 Int·Text를 변환하지 않는다. 문자열 따옴표 때문에 증가한 바이트도 기존 행 JSON 합계 상한에 다시 포함한다. 전체 HTTP envelope의 총 바이트 상한을 새로 보장한 것은 아니다.

음수는 두 후보에서 거부한다. 문자열 후보는 빈 값·앞자리 0·부호·공백·소수·i64 초과·숫자 입력을 거부한다. 이 domain은 비교용 규칙이며 최종 제품 결정을 대신하지 않는다. TS는 number와 string을 구분하지만 optional brand가 정규 형식이나 정수 범위를 증명하지 않는다. 서버의 필수 검사가 유지된다.

## 쓰기·재생·계약 불일치 경계

V3의 direct HTTP apply에 쓰는 `ids`와 허용된 `where`의 Id/Ref 조건에 같은 parser를 연결했다. 기존 bulk·중복 대상·타입·권한 검사는 유지한다. bundle·compose·공식 worker 경로는 Legacy로 남아 있으며 모든 쓰기 경로를 변환했다고 표시하지 않는다.

V6는 `changed`·`unchanged`를 wire로 변환한 뒤에만 멱등 결과를 기록하고 커밋한다. 숫자 후보에서 `where`가 큰 Id를 변경하려 해도 출력 범위 검사에 실패하면 트랜잭션을 되돌린다. 응답 유실 뒤 재생도 같은 표현을 유지한다. 후보별 principal namespace를 분리해 같은 actor·key·본문이 다른 wire 결과를 재생하지 않게 했다. 이는 모드를 넘나드는 전역 단일 실행이나 pending 이전을 보장하지 않는다.

생성 TS에 후보의 공개 `idWire` tag를 포함하고 공개 산출물의 SHA-256에 넣었다. Legacy와 SafeNumber가 모두 number 타입이어도 입력/출력 의미가 달라 식별값이 다르다. 내부 정책·docs는 식별값을 바꾸지 않는다. 변경 전 생성기와 현재 Legacy 생성기의 바이트도 두 fixture에서 직접 비교했다.

독립 리뷰에서 read보다 먼저 apply하면 V12의 성공 read 식별값 검사에 도달하기 전에 잘못된 모드의 쓰기가 커밋되는 반례를 발견했다. 실제 DB 재현 후 typed 연결이 `/read`·`/apply`·`/status`에 `x-aip-contract` 기대값을 보내도록 보강했다. 서버는 인증을 검증한 뒤 DB·멱등 조회·쓰기 전에 불일치를 거부한다. 중복·잘못된 헤더는 본문을 기다리기 전에 `BAD_REQUEST`다. `/session`은 계약 헤더를 요구하지 않는다.

첫 쓰기의 사전 거부는 pending을 정리한다. 이전 쓰기 응답이 유실된 뒤의 계약 거부는 앞선 커밋 여부를 증명하지 못하므로 `WriteUnsettled`와 pending을 유지한다. 서버 read 거부도 현재 세대 캐시를 비우며 이전 인증 세대 응답은 그 전에 거부한다. 헤더를 생략하는 raw 경로는 기존 서버 정책으로 실행한다. 이 헤더는 실험 후보이며 권한 승인·전체 행 decoder·호환성 증명은 아니다. 현재 식별값은 공개 읽기 계약과 Id 의미를 담고 공개 쓰기 계약 전체는 담지 않는다.

## 실행 증거

seed Id는 `0`, `42`, `9007199254740991`, `9007199254740992`, `9007199254740993`, `999999999999999999`, `9223372036854775807`이다. 다른 actor 행과 가려진 관계도 별도로 둔다. 각 모드 실행 뒤 실제 변경 Id 전체를 비교하고 flag를 초기화한다. 실패를 수집한 뒤 전용 schema를 제거하고 결과를 판정한다.

| 검증 | 결과 |
|---|---|
| V2 Id 단위 | 7개. parse/emit·중첩/null·scalar 출력·숫자 상한·변환 후 행 JSON 바이트 상한 |
| V5 후보 생성 | 3개. Id/Ref/filter 타입과 모드 식별·docs/정책 불변 |
| 정상/음성 tsc | 두 binding에서 자동 추론, number/string 잘못된 filter 거부. expect-error marker 제거 시 실제 오류 |
| V13 실제 PG/HTTP | Rust 1개, Node Legacy/mock 4개·숫자 1개·문자열 3개, 실패 0 |
| 실제 DB 변경 Id | Legacy는 반올림된 이웃 1개, 숫자 후보는 안전범위 안 3개, 문자열 후보는 seed 7개 정확히 변경. 다른 actor 행 불변 |
| 쓰기 원자성·복구 | 큰 Id where 거부 뒤 DB 불변, 문자열 최대 Id 응답 유실 후 동일 키 재생·unchanged 타입 유지 |
| 계약 헤더 | Rust 1개. 잘못된 5형식 본문 전 거부, read/apply/status 인증 우선·계약 사전 거부 |
| 독립 검토 | Sol이 binding mock Node 3개 직접 실행, 실패 0. 첫 apply 반례는 메인이 실제 DB로 재현·수정 |
| 기존 회귀 | V2 Rust 12개·V3 5개·V5 9개·V6 10개·V11 9개, 실패 0. V11 PG 정책 24조합 유지 |
| 정적 검사 | V2/V3/V5/V6 fmt·clippy all-targets `-D warnings`, strict tsc, exit 0 |

```bash
/Users/winterholic/.cargo/bin/cargo test --offline --manifest-path spikes/spike-v6-transport/Cargo.toml --test v13_idwire -- --nocapture
# V13 phase legacy: tests 4; pass 4; fail 0
# V13 phase safe: tests 1; pass 1; fail 0
# V13 phase string: tests 3; pass 3; fail 0
```

Luna가 단위 테스트를 먼저 작성했다. 메인은 현재 producer와 맞지 않는 scalar wrapper·Legacy mode export 기대를 코드로 대조해 바로잡고, 실제 실패 대조를 실행한 후 구현했다. V2 stub은 7개 실패, V5 baseline은 1개 통과·2개 실패였다. 첫 apply 계약 불일치도 방어 전에는 실제 행을 커밋했다.

고장 주입 6종을 각각 실행하고 복원했다. 문자열을 숫자로 출력, 안전범위 출력 검사 생략, 멱등 profile 공유, 계약 사전 거부 생략, 변환 후 바이트 상한 생략, 이전 유실 뒤 mismatch를 확정 결과로 처리하면 각각 관련 반례가 실패했다. 로그: `/var/folders/7_/5w7y5vq9329g81pk_fv8_mhh0000gn/T/aip-v13-mutations-o3nj0ut8`. 복원 후 전체 회귀를 재실행했다.

## 남은 범위

- 최종 Id 표현·음수/앞자리 0 domain·모드 전환·기존 pending/멱등 결과 이전·공개 호환 기간은 선택하지 않았다.
- 일반 Int의 bigint 정밀도, Python SDK, 전체 출력 decoder는 남는다.
- V13 당시 생성 쓰기 계약과 typed apply는 미연결이었다. 후속 [V14 결과](V14-typed-apply-results.md)에서 direct 공개 전이만 연결했다. 조합/worker는 남는다.
- bundle/compose/확장 쓰기, 운영 worker 격리, 변경 통보·재연결 복구·영속 pending, 실제 생산성·성능 비교와 본 구현 통합은 별도다.
