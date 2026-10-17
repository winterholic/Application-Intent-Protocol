# 시작 가이드: 정의 하나로 서버 띄우기

이 문서는 빈 디렉터리에서 시작해 정의 파일 하나를 PostgreSQL에 배포하고, 인증된 호출자가 읽기와 쓰기를 실행하기까지의 순서다. 아래 명령은 같은 순서로 실제 실행해 확인했다(2026-10-18, macOS, PostgreSQL 17). 운영 구성, 설정 전체, 마이그레이션은 [제품 서비스 실행](OPERATIONS.md)을 따른다.

AIP는 백엔드를 없애지 않는다. 컨트롤러·DTO·검증·권한·트랜잭션처럼 기능마다 반복되는 코드를 선언 하나와 검증된 실행 엔진으로 옮긴다. 아래 예제 정의(`prototype/example/app.aip`)에는 서버 코드가 한 줄도 없다. 호출자가 무엇을 읽고 바꿀 수 있는지는 정의가 정하고, 서버가 매 요청마다 최종 검사한다.

## 준비

| 필요한 것 | 비고 |
| --- | --- |
| Rust(1.98.1에서 검증) | `cargo build`로 `aip` 실행 파일을 만든다 |
| PostgreSQL(17에서 검증) | 전용 데이터베이스를 쓴다. 다른 버전은 이 가이드로 확인하지 않았다 |
| Node.js 26, Python 3 | 예제 호출 중 SDK와 worker 확장에만 필요하다 |

```sh
cargo build -p aip-cli --release
export AIP=$PWD/target/release/aip
export AIP_DATABASE_URL='postgres://user:password@127.0.0.1:5432/aip_demo'   # 비밀번호는 셸에만 둔다. 파일에 쓰지 않는다
mkdir demo && cd demo && cp ../prototype/example/app.aip .
```

서비스는 `.env`를 읽지 않는다. 연결 문자열은 설정에 적은 환경 변수 이름으로만 받는다.

## 1. 개발용 토큰 만들기

운영에서는 외부 인증 제공자가 RS256 access token을 발급한다. `aip service`는 토큰을 발급하지 않고 개발용 우회도 없다. 로컬에서는 제공자 대역을 하는 스크립트가 키를 만들고 토큰을 찍는다. 서비스가 검사하는 서명·issuer·audience·수명(최대 900초)은 그대로 적용된다. **운영에 쓰지 않는다.**

```sh
TOKEN=$(node ../product/tools/dev-jwt.mjs --dir ./dev-idp --subject user-42)
```

`./dev-idp/jwks.json`(공개 키)과 `dev-idp-private.pem`(개인 키, 모드 0600)이 생긴다. 토큰은 15분 뒤 만료되므로 만료되면 같은 명령을 다시 실행한다.

## 2. 설정과 배포

```sh
cat > service.json <<'EOF'
{
  "schema": "aip_demo",
  "database_url_env": "AIP_DATABASE_URL",
  "listen": "127.0.0.1:8080",
  "wire": "decimal",
  "auth": {
    "issuer": "https://issuer.example",
    "audience": "aip-api",
    "jwks": {"kind": "file", "path": "./dev-idp/jwks.json"}
  }
}
EOF
$AIP service check app.aip
$AIP service init app.aip --config service.json
```

`schema`는 `aip_`로 시작하는 소문자 ASCII 식별자(63바이트 이하)여야 한다. 다르면 `BAD_SCHEMA`로 거부한다. `check`는 정의를 검사하고, `init`은 새 schema에 테이블과 배포 저널을 만든다. 이미 있는 schema에는 `init`을 쓰지 않는다. 정의를 바꿀 때는 `service migrate`를 쓴다.

## 3. 데이터와 사용자 연결

토큰의 사용자(`sub`)를 앱의 actor 행에 연결해야 요청이 그 사용자로 실행된다. actor 행(여기서는 `member`)은 앱의 데이터이므로 먼저 있어야 한다.

```sh
psql "$AIP_DATABASE_URL" <<'EOF'
INSERT INTO aip_demo.member(id) VALUES (42);
INSERT INTO aip_demo.event(id, member_id, checked, title, link, phase, date)
VALUES (11, 42, false, 'First event', 'https://example.test', 'READY', '2026-01-01T00:00:00Z');
EOF
$AIP service principal --config service.json bind --subject user-42 --actor 42
```

토큰의 role이나 actor id는 권한으로 쓰이지 않는다. 연결은 서버 DB의 `bind`가 정한다.

## 4. 서버 시작과 호출

```sh
$AIP service gen app.aip --wire decimal --out binding.ts --sdk-import @aip/sdk
FP=$(sed -n 's/^export const contractFingerprint = "\(.*\)";/\1/p' binding.ts)
$AIP service serve app.aip --config service.json &
```

기동하면 JSON 한 줄이 나온다(`"event":"listening"`). 서버는 loopback HTTP만 연다. 외부에는 TLS reverse proxy를 앞에 둔다. `FP`는 이 정의의 계약 지문이다. 호출자는 자기가 아는 계약을 헤더로 보내고, 서버가 다르면 DB에 닿기 전에 거부한다.

```sh
curl -s localhost:8080/ready                       # 200이면 정의와 DB 구조 검사를 통과한 것
curl -s localhost:8080/read -H "authorization: Bearer $TOKEN" -H "x-aip-contract: $FP" \
  -H 'content-type: application/json' -d '{"query":{"read":"Event","select":["id","title","checked"]}}'
# {"ok":true,"rows":[{"checked":false,"id":"11","title":"First event"}],...}

curl -s localhost:8080/apply -H "authorization: Bearer $TOKEN" -H "x-aip-contract: $FP" \
  -H 'content-type: application/json' -d '{"key":"first-mark","request":{"apply":"Event.mark","target":{"ids":["11"]}}}'
# {"changed":["11"],"ok":true,...}      같은 key로 다시 보내면 "replayed":true, 두 번 실행되지 않는다
```

정의의 `transition mark`는 `member = actor`인 행의 `checked`를 `false`에서 `true`로 바꾸는 것만 허용한다. 호출자는 정의가 `expose`한 필드와 전이 밖의 것을 요청할 수 없다.

토큰 없이 보내면 익명 호출자로 취급한다. 이 예제의 행 정책(`rows read when member = actor`)은 익명에게 행을 주지 않으므로 `"rows":[]`를 받는다. 계약 지문이 다르면 `WIRE.BAD_REQUEST`, 정의가 열지 않은 필드를 고르면 `WIRE.FIELD_NOT_EXPOSED`다. 모든 오류 코드와 재시도 지침은 [`spec/wire-errors.md`](../spec/wire-errors.md)에 있고 `aip explain-code <코드>`로도 볼 수 있다.

## 5. SDK로 호출하기

HTTP를 직접 쓸 필요는 없다. TypeScript SDK는 생성한 binding으로 타입을 검사하고, 캐시와 미확정 쓰기 복구를 처리한다.

```sh
node ../product/tools/package.mjs --out ./artifact --profile release    # SDK tarball과 실행 파일 묶음
mkdir app && cd app && npm init -y >/dev/null && npm pkg set type=module >/dev/null
npm install --offline --ignore-scripts ../artifact/sdk/aip-sdk-0.1.0.tgz
cp ../binding.ts .
```

```ts
import { connect } from "@aip/sdk";
import { contract } from "./binding.ts";

const client = connect("http://127.0.0.1:8080", process.env.TOKEN!, contract);
const events = await client.read({ read: "Event", select: ["id", "title", "checked"] as const } as const);
const done = await client.apply({ apply: "Event.mark", target: { ids: ["11"] } } as const, { key: "mark-11" });
console.log(events.rows, done);
```

Node 26은 `.ts` 파일을 그대로 실행한다: `TOKEN=$TOKEN node app.ts`. 컴파일하려면 `tsc`로 `binding.ts`와 함께 빌드한다.

Python은 `product/python`을 `PYTHONPATH`에 두고 `from aip_sdk import connect`로 같은 계약 지문을 받아 쓴다(`connect(base, token, fingerprint).read({...})`). 두 SDK는 같은 멱등 key 공간을 쓴다. 전체 API는 [설치 SDK](SDK.md)를 따른다.

## 정리하고 다음으로

```sh
kill %1 && psql "$AIP_DATABASE_URL" -c 'DROP SCHEMA aip_demo CASCADE'
```

- 정의 문법: [`spec/definition-syntax.md`](../spec/definition-syntax.md). 진단 코드는 `aip explain-code DEF.<코드>`.
- 정의를 바꿔 배포: [변경 배포와 기존 데이터 이관](OPERATIONS.md#변경-배포와-기존-데이터-이관).
- 서버 확장(Node·Python), durable job, 파일, 이벤트: [관리형 실행](MANAGED-EXECUTION.md)과 [서버 인프라](INFRASTRUCTURE.md).
- 운영 인증과 TLS: [IdP 운영](IDP-RUNBOOK.md), [제품 서비스 실행](OPERATIONS.md#tls-reverse-proxy-검증).
- 새 릴리스로 올리기와 되돌리기: [릴리스 업그레이드와 롤백](OPERATIONS.md#릴리스-업그레이드와-롤백).
