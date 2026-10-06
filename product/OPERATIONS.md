# 제품 서비스 실행

root의 `aip service`는 caller 정의와 공통 Rust 실행 엔진, 설치형 TypeScript SDK를 연결한다. 기존 `aip run/gen-ts` Core IR 경로와 `aip-prototype` 개발 토큰 경로는 이전 구현이다. 제품에서 개발 토큰을 발급하지 않는다.

## 설정

```json
{
  "schema": "my_app",
  "database_url_env": "AIP_DATABASE_URL",
  "listen": "127.0.0.1:8080",
  "wire": "decimal",
  "allowed_origins": ["https://app.example.com"],
  "auth": {
    "issuer": "https://issuer.example.com",
    "audience": "aip-api",
    "jwks": {"kind": "https", "url": "https://issuer.example.com/.well-known/jwks.json"}
  }
}
```

issuer·audience·JWKS URL은 실제 제공자에 맞춰 설정한다. 인증 프로필은 RS256, `typ=at+jwt`, 정확한 API audience와 issuer, `sub/iat/nbf/exp`, 최대 900초 수명이다. 이 프로필을 발급하도록 제공자를 설정해야 한다. 로그인·refresh token은 제공자와 앱이 담당한다. 토큰의 role이나 actorId는 AIP 권한으로 사용하지 않는다.

DB 연결 문자열은 설정의 환경 변수로 전달한다. 서비스는 `.env`를 자동으로 읽지 않는다. 원격 DB는 `sslmode=require`와 인증서·호스트명 검증이 필요하다. 사설 DB CA는 `database_ca_file`로 설정하며 설정 파일 기준 상대 경로와 최대 64KiB 인증서 PEM만 허용한다. 공개 JWKS 파일을 사용할 때는 `{"kind":"file","path":"./jwks.json"}`이며 역시 설정 파일 기준 경로다.

listener는 loopback HTTP에 한정된다. 외부 HTTPS는 reverse proxy를 구성한다. `allowed_origins`는 정규화된 정확한 HTTPS origin 또는 로컬 HTTP origin만 받는다. 알려지지 않은 설정 키는 오류다. 토큰·개인키·실제 DB 비밀번호를 설정 예제나 로그에 남기지 않는다.

## 초기 배포와 호출

저장소 루트에서 다음 순서로 실행한다. 아래 예시는 인증 제공자와 DB 연결 환경 변수를 구성한 뒤 사용한다.

```sh
cargo build -p aip-cli
./target/debug/aip service check app.aip
./target/debug/aip service init app.aip --config service.json
./target/debug/aip service principal --config service.json bind --subject user-42 --actor 42
./target/debug/aip service gen app.aip --wire decimal --out app.binding.ts
./target/debug/aip service serve app.aip --config service.json
```

actor 행은 실제 앱의 사용자 데이터가 먼저 존재해야 한다. `bind`는 이미 존재하는 actor와 제공자 subject를 연결한다. 같은 subject의 actor 재배정은 거부한다. `revoke --subject user-42`는 연결을 비활성화하고 해당 시각까지 발급한 토큰을 무효화한다. 같은 actor를 다시 활성화해도 폐기 토큰은 살아나지 않는다. 폐기와 같은 초에 발급한 토큰도 제외하므로 다음 초 이후 새 토큰을 사용한다. 없는 subject의 폐기는 `PRINCIPAL_NOT_FOUND`로 실패한다. `bind --not-before <epoch seconds>`로 기준을 높일 수 있고, 기준을 낮추지는 못한다.

SDK는 `npm install --offline <aip-sdk.tgz>`로 별도 소비자에 설치한다. `connect(baseUrl, accessToken, generatedBinding)`는 기존 typed transport·캐시·미확정 쓰기 복구를 사용한다. `wire` 설정과 binding 생성의 `--wire`를 맞춘다. 배포의 wire는 최초 연결 시 영속 고정하며 이후 설정만 바꿔 전환하지 않는다. 이 제한은 멱등 principal namespace가 바뀌어 이미 커밋한 쓰기가 재실행되는 것을 막는다.

## 변경 배포와 기존 데이터 이관

```sh
./target/debug/aip service migrate next.aip --config service.json
./target/debug/aip service migrate next.aip --config service.json --apply <reviewed-plan-digest>
```

첫 명령은 읽기 전용 계획이다. 변경 분류·SQL·데이터 영향을 검토하고 반환된 digest를 적용 명령에 전달한다. 필요한 경우 `--migration migration.json`을 양쪽에 똑같이 전달한다. backfill/convert는 [마이그레이션 계약](../crates/aip-migrate/README.md)의 정형 JSON만 받으며 임의 SQL은 받지 않는다. 적용 실패는 DDL·데이터·저널을 같은 트랜잭션에서 되돌린다. 커밋 응답이 불명확하면 같은 digest를 재시도해 최신 저널과 catalog로 확인한다.

요청의 실제 SQL 연결은 배포 shared advisory lock을 유지한다. 마이그레이션은 같은 키의 exclusive lock을 사용하므로 진행 중 요청과 직렬화한다. 적용 뒤 이전 정의를 가진 listener는 `DEPLOYMENT_CHANGED`로 작업을 거부한다. 새 정의로 서버를 재시작하고 생성 binding을 갱신한다. 구버전 클라이언트를 동시에 지원하는 프로토콜은 이번 구현에 포함하지 않는다.

현재 prototype의 구조 지문 marker가 있는 schema는 다음처럼 명시적으로 이관한다.

```sh
./target/debug/aip service adopt app.aip --config service.json
./target/debug/aip service adopt app.aip --config service.json --apply <reviewed-plan-digest>
```

먼저 기존 prototype을 중지한다. 기존 앱 행과 커밋된 멱등 결과는 유지하고 제품 저널·principal 연결을 만든다. 정의의 DDL과 기존 catalog marker가 다르면 이관을 거부한다. 원래 marker에는 정책 본문이 없으므로 제출한 정의의 execution digest가 새 정책 정본이다. 개발 토큰은 이관하지 않는다. 이전 wire와 같은 제품 설정을 사용한다. 구조 지문 없는 구형 marker나 임의 운영 schema를 자동으로 채택하지 않는다.

## 확장과 검증 범위

공식 확장은 `workers: {"directory":"./extensions","language":"node","enable_writes":true}`를 명시해 활성화한다. Python은 `language:"python"`이다. 기존 공통 엔진과 정본 구현을 사용하며 서버가 입력·권한·시간·출력·트랜잭션을 검사한다. 현재 worker 격리는 macOS의 `MacNetDeny` 네트워크 차단이고 파일시스템 전체 격리나 다른 OS 운영 격리를 보증하지 않는다.

실제 PostgreSQL의 소유 테스트 schema, 통제된 HTTPS 서버와 메모리 생성 RSA 키, offline 설치 SDK, 재배치 macOS binary로 검증한다. 외부 IdP 계정과 실제 reverse proxy 배포는 이 환경에서 실행하지 않았다. 대형 테이블 무중단 변경·자동 역방향 migration·actor resource 교체·임의 데이터 변환은 지원하지 않는다. 좁은 검증은 다음 명령으로 재실행한다.

```sh
cargo test -p aip-auth -p aip-migrate
cargo test -p aip-cli --test service --test service_boundaries --test service_flow --test adoption_flow
node --test product/tests/*.test.mjs
```
