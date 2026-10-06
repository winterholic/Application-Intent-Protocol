# V12. 생성 계약과 실제 프론트 SDK 연결 결과

> 상태: 독립 실험 검증, 기술 후보(2026-10-04). [계획·8개 설계 관문](V12-typed-transport-plan.md), [독립 검토](../reviews/codex-v12-sol-review.md). V5의 선택형 Row 추론을 V6의 실제 HTTP·캐시·V10 세션 복구에 연결했다. 본 `crates/`와 최종 공개 프로토콜은 변경하지 않았다.

## 개발자가 사용하는 경계

같은 서버 facts에서 앱별 `Contract`와 `contract` binding을 생성한다. 공통 타입 core는 특정 앱의 Contract를 import하지 않는다. 화면은 별도 제네릭 인자·결과 단언 없이 선택한 필드의 타입을 얻는다.

```ts
import { connectTyped } from "./typed.ts";
import { contract } from "./generated-v12.ts";

const aip = connectTyped(url, token, contract);
const result = await aip.read({ read: "MemberAlarm", select: ["id", "isChecked"] });
const checked: boolean = result.rows[0].isChecked;
const cached: boolean = result.cached;
```

`rows`와 중첩 관계는 캐시의 실제 동결 동작에 맞춰 readonly다. 관계의 `null`, 조건부 select의 union, 동적 배열의 optional 필드, `cached/stale/stored` 정보도 유지한다. V5 fixture adapter는 같은 타입 core를 사용한다. 타입 테스트용 variant adapter는 자기 Contract와 상위 공통 core를 참조하며 타입 연산을 복제하지 않는다.

typed facade는 `apply`, `replaceSession`, `retryPending`, `pending`, raw `post`를 재사용한다. `apply` 입력·결과의 타입 연결은 이번 범위 밖이다. 캐시는 `size()` 조회만 공개한다. 실험 기반 low-level `connect`의 변경 가능한 캐시를 typed facade에 노출하면 검사하지 않은 행을 주입할 수 있어 독립 리뷰 후 닫았다.

## 계약 식별과 캐시 경계

식별값은 기존 `contract_ts(facts)` 바이트의 SHA-256 소문자 64자리 hex다. 이 산출물은 공개 **읽기** 타입·관계·filter/sort·행 상한을 담는다. binding 상수는 해시 계산 뒤 붙여 순환 입력을 피한다. 내부 권한 조건만 바뀌면 식별값은 같고 공개 필드가 바뀌면 달라진다. 설명 metadata도 입력이 아니다.

서버는 기동할 때 같은 helper로 한 번 계산하고 성공한 `/read` 응답에 `contractFingerprint`를 붙인다. 클라이언트는 연결 생성 시 binding 형식을 검증하고 문자열 값을 고정한다. 실제 응답은 다음 순서로 검사한다.

1. 이전 인증 세대의 응답이면 `ScopeChanged`. 구세대 계약 오류로 새 캐시를 비우지 않는다.
2. 명시 서버 오류는 기존 오류 코드를 유지한다. 잘못된 성공 표식은 `PROTOCOL_ERROR`다.
3. 성공 응답의 식별값이 누락·null·비문자열·불일치이면 `CONTRACT_MISMATCH`. 기존 캐시도 비우고 해당 응답은 저장하지 않는다.
4. 식별값이 같아도 rows 배열·문자열 deps 배열·선택적 유한 비음수 maxAgeMs 형식이 틀리면 `PROTOCOL_ERROR`다.
5. 위 검사를 마친 값만 기존 V5 캐시 경로에 넘긴다. 불일치로 비워도 미확정 쓰기 수는 유지한다. 같은 키 재시도 확정 뒤에야 저장을 다시 허용한다.

캐시 hit는 서버에 요청하지 않는다. 저장 당시 계약을 검사했으며 V9의 제한된 수명 안에서 재사용한다. 배포 직후 즉시 계약 변경 감지나 즉시 권한 회수를 보장하지 않는다. 불일치 감지 뒤 올바른 응답이 오면 재조회로 복구하며 영구 오류 latch를 두지 않는다.

식별값은 권한 승인·호환성 증명·행 내용 전체 검증·암호학적 서버 신원 증명이 아니다. 필드 값과 relation 구조 전체를 별도 decoder로 검사하지 않는다. 신뢰한 서버/생성기의 읽기 계약을 결합하는 후보이며 개발자가 TS 단언으로 다른 Contract를 꾸며 내는 것까지 방지하지 않는다. 생성기 출력 형식 변경도 보수적 불일치를 일으킬 수 있다.

## 실행 근거와 실패 대조

Luna high가 테스트 5개 파일을 먼저 작성하고 메인이 baseline을 실행했다. 타입을 붙이지 않은 adapter는 선택 결과를 `unknown`으로 남기고 잘못된 요청도 받아들였다. Node 8개 중 2개 통과·6개 실패, 서버 식별값 누락과 정상/음성 tsc 실패를 확인한 뒤 구현했다. helper의 초기 빈 fingerprint도 Rust 2개 반례로 검출했다.

Sol high의 실제 재현 두 건은 메인도 새 실패 테스트로 확인했다. binding 식별값 누락은 Node 9개 중 1개, 변경 가능한 캐시 노출은 Node 10개 중 1개가 실패했다. 각각 필수 binding 검사와 상태 조회 전용 facade를 적용한 뒤 통과했다. V5 전체 회귀에서는 variant가 옛 단일 파일 adapter만 복사하는 문제가 드러나 상대 import를 공통 core로 연결했다.

| 검증 | 실행 결과 |
|---|---|
| V5 공개 타입 식별 | Rust 2개. docs·내부 정책만 변경한 경우 동일, 공개 필드 변경은 불일치 |
| V12 실제 HTTP/PG | Rust 1개가 생성 모듈·tsc·Node 11개를 함께 실행. 실제 읽기·캐시 hit·쓰기 무효화·같은 actor 세션 교체·다른 actor 행 분리·관계 조회·raw 닫힌 필드 거부·잘못된 binding 거부 |
| 타입 추론 | 추가 Contract 인자·결과 cast 없이 MemberAlarm과 Recruitment 결과 타입 일치. readonly·관계 null·조건부 union·동적 optional·닫힌 root/field/filter 값 검증 |
| 타입 음성 대조 | expect-error 표본이 정상 컴파일되고 marker를 지우면 오류. 캐시 주입 API도 타입 오류 |
| 계약 단위 | Node 10개. 식별값/성공 envelope 오류·TTL·쓰기 경합·기존 캐시 비움·pending 보존 및 복구·구세대 오류 우선순위 |
| 독립 재검증 | Sol이 Node 10개와 strict tsc 정상·음성 파일을 직접 실행, 실패 0 |
| 기존 회귀 | V5 전체 Rust 6개, V6 전체 Rust 8개, V11 전체 단위 8개·PG 1개(24조합), 실패 0 |
| 정적 검사 | V5/V6 fmt check·clippy all-targets `-D warnings`, strict tsc 정상·음성 파일, exit 0 |

```bash
/Users/winterholic/.cargo/bin/cargo test --offline --manifest-path spikes/spike-v6-transport/Cargo.toml --test v12_typed -- --nocapture
# Node: tests 11; pass 11; fail 0
# Rust: test result: ok. 1 passed; 0 failed
```

고장 주입 세 가지를 실제 실행하고 복원했다. 식별값 검사를 생략하면 불일치 반례가 실패한다. 구세대 검사를 식별값 대조 앞에서 빼면 이전 응답이 새 캐시를 비우는 반례가 실패한다. 캐시 무효화 때 미확정 수를 0으로 바꾸면 보류 중 저장 반례가 실패한다. 복원 후 Node 10개 실패 0. 로그: `/var/folders/7_/5w7y5vq9329g81pk_fv8_mhh0000gn/T/aip-v12-mutations-rr1yeegn`.

## 남은 범위

- V12 당시 공개 Id는 JS `number`다. 후속 [V13 결과](V13-id-boundary-results.md)에서 Legacy 반올림 반례와 숫자 제한·십진 문자열 후보를 비교했다. 최종 표현은 선택하지 않았다.
- 후속 [V14](V14-typed-apply-results.md)는 direct typed apply를 연결했다. 조합/worker·Python SDK·실제 설치/빌드 패키지·editor 자동완성 체감·생산성/성능 측정은 남는다.
- 공개 계약 호환 기간·배포 협상·전체 decoder·영속 pending·변경 통보/재연결 복구는 이번 실험이 아니다.
- SHA-256 표현과 binding API는 후보다. 다른 언어의 생성 방식이나 최종 wire 표현을 확정하지 않는다.
