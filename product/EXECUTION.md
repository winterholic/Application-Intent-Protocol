# 데이터 resource와 독립된 동기 operation

2026-10-10 후속 구현. 아래 `operation read`와 SDK 호출은 현재 제품 경로의 실제 문법이다. durable Job·관리형 파일·외부 효과·workflow는 [다음 구조 변경 제안](../plan-docs/backend-capability/durable-job-proposal.md)이며 아직 이 문법으로 실행할 수 없다.

## 선언과 호출

업무 테이블이 없는 앱은 다음처럼 작성한다. 독립 `.aip`, TS/Python 내 선언 블록, TS/Python 정형 데이터의 기존 다섯 작성 형식에서 같은 계약으로 읽는다.

```aip
actor principal
operation read textLength {
  input { text: Text(1..1000) }
  output { length: Int(1..1000) }
  allow actor != null
  effect none
  deadline 2s
  implementation "compute.textLength"
}
```

서버의 신뢰한 `compute.mjs`:

```js
export function textLength(input) {
  return { length: [...input.text].length };
}
```

또는 `compute.py`. Python worker는 비동기 함수를 기다린다.

```python
async def textLength(input, ctx):
    return {"length": len(input["text"])}
```

기존 `aip service gen`으로 생성한 binding을 같은 SDK에 전달한다.

```ts
import { connect } from "@aip/sdk";
import { contract } from "./bindings.js";

const client = connect(baseUrl, accessToken, contract);
const output = await client.operation("textLength", { text: "가😀A" });
// output.length === 3
```

caller는 등록된 이름과 typed 입력을 고른다. 구현 경로·정책·임의 코드·실행 권한을 요청에 넣을 수 없다. 기존 표준 read/apply로 충분한 업무에 화면별 operation을 다시 만들 필요는 없다. 공통으로 재사용할 계산이나 서버 정책이 필요한 동작에 사용한다.

## 신원과 저장소

`actor principal`은 **resource가 없고 operation이 있는 앱에서만** Runtime 관리 신원을 뜻한다. `principal.Id`와 `actor.id`를 사용할 수 있고 업무 필드나 `exists principal`은 사용할 수 없다. 실제 `resource principal`이 이미 있는 정의는 기존 actor resource 의미를 유지한다. `actor Member` 같은 기존 정의에도 operation을 추가할 수 있으며 기존 predicate와 actor 필드 정책을 재사용한다.

작업 전용 앱의 `aip service init`은 운영 신원용 `aip_actors`와 기존 인증·배포 metadata를 만든다. `service principal bind`가 명시적으로 actor를 등록하고 subject를 연결하며 요청에서 자동 등록하지 않는다. bind 충돌은 같은 트랜잭션의 actor 등록도 되돌린다. revoke는 actor row를 삭제하지 않는다. 저장된 wire와 다른 설정의 principal 명령은 거부한다.

업무 resource/table은 없어도 **PostgreSQL은 여전히 필요하다**. 인증 연결·폐기·배포 fence·clock·allow 평가는 DB를 사용한다. 실제 actor resource에서 관리형 신원으로 바꾸는 migration은 FK 재매핑 의미가 없어 `Unsupported`로 차단한다.

## 실행 경계

| 항목 | 현재 계약 |
|---|---|
| 종류·효과 | `read`, `effect none`, 빈 `access`만 허용 |
| 입력·출력 | 정확한 키, scalar 타입, nullable 값, Text/Int 범위 검사. nullable도 키 생략은 불가 |
| 정수 | 제품 worker 입출력의 Int는 JS 안전 정수 범위. Id는 설정된 safe/decimal wire를 별도로 사용 |
| 범위 metadata | 경계 숫자도 JS 안전 정수로 표현 가능해야 함. Text 길이는 Unicode 코드 포인트 수 |
| 정책 | `allow` 필수. 기존 BooleanExpr/predicate와 parameterized SQL compiler 재사용 |
| 반환 전 검사 | 새 DB clock으로 allow 재평가, 인증 provider로 현재 principal·토큰 재검증. 검사 시점 이후의 경쟁까지 원자적으로 잠그는 보장은 없음 |
| 기한 | 선언은 1ms 이상 30s 이하. operation 인가·worker 시작/전송/실행/종료·사후 인가·최종 인증을 포함. 앞선 HTTP 수신·초기 인증·DB 연결/fence는 기존 별도 기한 사용 |
| 자원 | 기존 worker 동시 실행 slot, 본문 상한, stdout/stderr framing 상한 재사용 |
| ctx | 데이터 ctx 호출 시 즉시 `ACCESS_NOT_DECLARED`. 확장에서 예외를 잡아도 성공으로 바뀌지 않음 |
| 배포 | 기존 shared deployment fence 유지. 오래된 listener는 정책 변경 뒤 `DEPLOYMENT_CHANGED` |
| 수명 | 동기 결과만 반환. 연결 유실 뒤 status 복구·지속 실행·자동 retry·cancel 계약 없음 |

Worker는 macOS `MacNetDeny`를 사용한다. 네트워크 차단은 파일 접근 격리나 순수성 증명이 아니다. 신뢰한 서버 extension만 등록한다. `effect none`은 요청자가 DB/외부 효과 capability를 얻지 않는다는 선언이며 임의 extension의 파일 부작용까지 막는 보장은 아니다. 다른 OS의 worker 격리는 아직 제공하지 않는다.

## HTTP와 발견

`POST /operation`의 본문은 정확히 `{ "operation": "textLength", "input": { "text": "가😀A" } }`다. 서명 세션과 생성 계약의 `x-aip-contract`가 필요하다. `/extension`으로 operation 이름을 호출하는 별칭은 없다.

`POST /capabilities`는 서명 세션과 빈 객체를 받는다. 초기 발견을 위해 예상 계약 지문 없이 호출할 수 있다. 현재 공개 목록은 operation만 포함하며 전체 read/apply catalog는 아니다. 계약 지문, 입력·출력·범위·기한·효과·수명·의존성과 worker 설정 여부에 따른 `available`을 반환한다. allow와 구현 경로는 공개하지 않는다. `authorization: "checked-on-call"`은 목록 등재가 실행 인가를 뜻하지 않음을 나타낸다.

의존성은 `worker: []`, `authorization: ["database", "principal", "deployment"]`로 구분한다. 업무 데이터가 필요 없는 worker와 DB를 사용하는 서버 권한 검사를 혼동하지 않는다. SDK binding과 HTTP catalog는 같은 descriptor projector를 사용한다. 공개 타입·범위·기한 등은 계약 지문에 포함되고 내부 구현 경로·allow는 제외한다. 내부 정책 변경은 별도 배포 facts 지문이 감지한다.

실패에는 `UNAUTHENTICATED`, `TOKEN_EXPIRED`, `CONTRACT_MISMATCH`, `NOT_EXPOSED`, `BAD_VALUE`, `ACCESS_DENIED`, `OUTPUT_INVALID`, `ACCESS_NOT_DECLARED`, `DEADLINE_EXCEEDED`, `DEPLOYMENT_CHANGED`, `DB_UNAVAILABLE` 등이 있다. SDK는 잘못된 출력과 계약 불일치, 세션 교체 이전 응답도 거부한다. 기존 `/status`와 WRITE의 `COMMIT_UNKNOWN` 복구 의미는 유지한다.

검증 명령과 이번 실행 결과는 [VERIFICATION](VERIFICATION.md), 전체 조사 기준은 [기능 감사](../plan-docs/backend-capability/README.md)를 따른다. 이 기능 하나가 여섯 시나리오 전체나 모든 backend 기능을 구현한 것은 아니다.
