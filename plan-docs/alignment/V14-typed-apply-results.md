# V14. 공개 전이의 생성 쓰기 타입 연결 결과

> 상태: 독립 실험 검증, 기술 후보(2026-10-04). [계획·8관문](V14-typed-apply-plan.md), [독립 리뷰](../reviews/codex-v14-sol-review.md). direct HTTP 공개 전이의 입력·결과·복구 타입과 실제 응답 검사를 연결했다. 본 `crates/`와 최종 공개 API·Id 표현은 변경하지 않았다.

## 개발자가 사용하는 경계

같은 facts에서 읽기 Contract와 별도 ApplyContract를 생성해 하나의 binding으로 연결한다. 읽기 공개가 없는 WriteOnly.mark도 포함하고 닫힌 전이는 내보내지 않는다. 화면은 별도 결과 cast나 계약 제네릭 인자 없이 읽은 Id로 공개 전이를 호출한다.

```ts
import { contract } from "./generated-v14-string.ts";
import { connectTypedApply } from "./typed.ts";

const aip = connectTypedApply(url, token, contract);
const inbox = await aip.read({ read: "Inbox", select: ["id", "checked"] });
const result = await aip.apply({
  apply: "Inbox.mark",
  target: { ids: [inbox.rows[0].id] },
});
if (result.ok) {
  const ids = result.changed;
}
```

ApplyContract에는 action 이름, 입력/출력 Id 타입, 허용 ids/where, maxRows, 실제 eq 조건과 값 타입을 담는다. Bool·Int·Text와 후보 Id/Ref 값은 V3의 실행 검사에 맞춘다. 읽기의 gte 등을 쓰기에 허용하지 않는다. V14 당시 V3가 처리하지 못하는 Time·Url·Enum where 값은 never로 표시했다. 후속 [V15 결과](V15-filter-value-results.md)에서 공통 값 검사와 생성 타입을 연결했다. 당시 공백을 제품 규칙으로 정하지 않았다. 현재 선언 자체가 Ref.eq를 거부하므로 Ref filter의 전체 작성·실행 경로를 검증했다고 표시하지 않는다.

정적 검사는 닫힌 action·target 누락·ids/where 동시 지정·허용 밖 where·값 타입·다른 Id wire와 직접 객체 literal의 초과 키를 거부한다. 입력은 readonly 배열도 받는다. 빈 동적 ids·bulk 상한·Id 문자열 형식/숫자 범위·권한은 서버의 필수 검사다. TS의 구조적 타입이나 optional brand가 전체 런타임 값의 적합성을 증명하지 않는다.

성공 결과는 readonly changed/unchanged Id 배열·tags, 실패는 code·선택적 msg의 판별 union이다. recovered는 확정 오류에도 붙을 수 있으며 replayed는 성공 멱등 재생의 선택 정보다. retryPending와 같은 actor replaceSession 결과는 action 전체의 Id union과 로컬 key를 제공한다. 아직 미확정인 키는 복구 목록에 없고 pending에 남는다.

## 서버 계약과 실제 응답 확정

V14 후보는 공개 읽기+쓰기 산출물의 단일 SHA-256을 생성 binding과 서버 startup에서 사용한다. 읽기 없는 action·target·bulk·where 값 타입 변경도 지문을 바꾼다. 내부 allow/from/effects/sameScope 정책과 docs는 해시 입력이 아니다. 쓰기만 바뀌어도 읽기 binding 재생성이 필요한 보수적 비용이 있다. 호환성·업무 의미 동등성·권한 승인을 보장하지 않는다.

후보 API는 `contract_*_with_apply`, `start_with_apply_contract`, `connectTypedApply`다. 기존 Legacy/V13 생성기·start·read facade를 보존하기 위한 실험 분리다. 변경 전 생성기 소스와 현재 산출물을 두 fixture·세 wire에서 직접 대조해 타입·지문·모듈 바이트가 동일함을 확인했다. 최종 제품에 여러 connect API를 그대로 제공하기로 정하지 않았다. V14 실제 왕복 검증은 SafeNumber와 DecimalString이다. Legacy의 큰 Id 손실은 V13에 기록한 그대로다.

binding의 fingerprint와 idWire는 생성 시 필수 형식 검사 후 고정한다. idWire 누락으로 검사가 꺼지지 않는다. SDK는 기존 인증 세대 검사 후 send에서 tags·changed·unchanged 배열과 모든 Id 요소를 검사한다. 숫자는 안전 정수·비음수, 문자열은 정규 비음수 i64다. 선택적 replayed/recovered·오류 msg의 형식도 확인한다. 형식이 틀린 성공은 커밋 여부를 모르는 결과이므로 settle하지 않고 같은 키·같은 본문으로 재시도한다. 캐시 저장도 보류한다.

검증한 후보 결과 객체와 알려진 배열은 실제로 동결한다. 동일 키 single-flight 결과를 한 JS 호출자가 바꿔 다른 호출자에게 보이게 하지 않는다. recovered 결과·복구 목록과 항목·사전 멱등 충돌 오류도 동결한다. 알 수 없는 추가 속성 전체를 재귀 decoder로 검증한 것은 아니다. 복구 목록의 key는 서버 추가 속성으로 덮어쓰지 못하게 로컬 값을 마지막에 붙인다. 기존 low-level 연결에서 idWire 옵션을 쓰지 않는 경로는 V14 검사·동결을 적용하지 않는다.

## 실제 실행 증거와 실패 대조

fixture는 Inbox의 ids/where, 읽기 없는 WriteOnly의 ids, WhereOnly의 where를 구분한다. 각 후보 실행 뒤 실제 DB의 변경 Id 전체를 비교한다. 숫자 후보는 Inbox 42·43·44·45, 문자열 후보는 여기에 9007199254740993을 더한다. 두 후보 모두 WriteOnly 7·WhereOnly 9만 변경하고 다른 actor와 선택하지 않은 행은 유지한다. phase 뒤 flag를 초기화하고 실패 수집 뒤 schema를 삭제한다.

| 검증 | 실행 결과 |
|---|---|
| 생성 쓰기 계약 | Rust 4개. 읽기 없는 action·허용 target·eq projection·지문 민감도·내부 정책/docs 불변·미지원 where 값 never |
| 실제 HTTP/PG | Rust 1개, 숫자/문자열 각 Node 14개 실패 0. 생성 tsc·ids/where·반복 unchanged·replay·cache 무효화·권한·응답 유실 |
| 실제 커밋 응답 손상 | 응답 Id mode를 바꾸면 WriteUnsettled/pending 유지, 읽기는 committed=true지만 stored=false. 정상 원본 replay 뒤 확정 |
| 계약 사전 거부 | 공개 쓰기 bulk만 다른 이전 binding의 첫 apply를 DB 전에 거부. 올바른 binding의 changed 결과로 앞선 미커밋을 대조 |
| 타입 정상/음성 | 자동 결과 타입 equality·readonly·read-only root·action/target/where/Id wire·직접 literal 초과 키. expect-error 20개와 marker 제거 오류 대조 |
| 응답 단위 | Node 8개. 두 배열·canonical/i64·안전 정수·mode·metadata·세션 교체·복구 오류·로컬 key·결과 동결 |
| 독립 재검증 | Sol 직접 Node 7개 실패 0·strict tsc exit 0. 조건부 action union과 초과 키/동결 수정도 직접 대조 |
| 전체 회귀 | V5 Rust 13개·V6 11개·V11 9개, 실패 0. V11 PG 정책 24조합, V12/V13 및 인증·세션·수명·복구 유지 |
| 정적 검사 | V5/V6/V11 fmt·clippy all-targets -D warnings, V12/V13/V14 strict tsc, exit 0 |

```text
cargo test --offline --manifest-path spikes/spike-v6-transport/Cargo.toml --test v14_apply -- --nocapture
V14 phase safe: tests 14; pass 14; fail 0
V14 phase string: tests 14; pass 14; fail 0
test result: ok. 1 passed; 0 failed
```

메인이 테스트를 먼저 작성했다. Luna는 사용 한도로 테스트 작업을 수행하지 못했다. fixture의 Ref.eq가 V1에서 거부되는 초기 준비 오류를 수정한 뒤, 생성기 baseline Rust 4개 실패와 SDK 응답 Node 5개 실패를 실제 관측하고 구현했다. 통합 baseline은 각 후보 Node 11개 실패와 잘못된 startup 지문, tsc의 any/음성 검사 누락을 검출했다.

Sol이 generic subtype으로 직접 literal 초과 키가 허용되는 반례와 single-flight 결과 배열 변조를 재현했다. 메인도 새 음성 tsc 3개와 Node 실패를 관측한 뒤 action별 닫힌 요청 signature와 실제 동결로 보강했다. 로컬 복구 key 덮어쓰기와 빠른 멱등 충돌 결과의 미동결도 실패 대조 후 수정했다.

고장 주입 6종 모두 관련 행동 검사 실패 후 복원했다. 쓰기 projection 해시 생략, 결과 Id 배열 검사 생략, 결과 동결 생략, 초과 키 허용 signature 복원, 원격 key 덮어쓰기, binding idWire 필수 검사 생략이다. 로그: `/var/folders/7_/5w7y5vq9329g81pk_fv8_mhh0000gn/T/aip-v14-mutations-92vpp6jy`. 복원 후 전체 회귀를 재실행했다.

## 남은 범위

- V14 당시 scalar filter와 prefix의 공백은 후속 [V15](V15-filter-value-results.md)에서 연결했다. nullable 조건 표현·create/compose 값 타입 등은 남는다.
- W0/W1·worker·Python SDK·패키지 설치/배포·전체 출력 decoder·Id 최종 선택·호환 기간은 검증하지 않았다.
- 실제 DX·생산성 절감률·성능·운영 worker 격리·즉시 변경 통보·영속 pending과 본 구현 통합도 남는다. 타입 연결 실험을 전체 제품 완성으로 확대하지 않는다.
