# AIP 실행 프로토타입 통합

> 상태: check/gen/init/serve CLI와 단일 SDK 진입점, 두 Id 후보의 정의 변경·재기동 smoke와 로컬 설치형 SDK를 구현·검증했다. 공식 READ/WRITE 확장의 생성 계약·HTTP·Node/Python 연결, 실제 Chrome·재배치 bundle도 검증했다. WRITE 범위는 [확장 결과](#write-공식-확장의-실행-결과), 최신 입력·캐시·CLI 검증은 [후속 결과](#정의-입력캐시-완료cli-출력-후속-결과)를 따른다. 제품 설치 배포·운영 인증·일반 마이그레이션은 남는다. [값 검사 실행 증거](../plan-docs/alignment/V15-filter-value-results.md), [창시자 기준](../docs/PRINCIPLES.md).

목표는 개발자가 한 앱 정의에서 계약을 생성하고 서버를 띄운 뒤, 프론트에서 필요한 필드와 공개 동작을 표현하는 실제 흐름이다. 기존 main CLI의 이름난 Intent 호출을 caller 표현의 대안으로 제시하지 않는다. 검증한 V1/V2/V3/V5/V6을 얇게 조립하고 새 엔진·planner·SDK 복제본을 만들지 않는다.

## 실행

`prototype/`에서 실행한다. `example/app.aip`가 샘플 앱의 정본이며 V15 회귀도 이 정의를 읽는다. SDK는 기존 구현을 재사용하는 `sdk/index.ts`의 connect다. 생성 계약은 같은 파일에서 읽기와 공개 쓰기 타입을 제공한다.

```sh
cargo run --offline -- check example/app.aip
cargo run --offline -- check example/app.aip --dev-checks
cargo run --offline -- gen example/app.aip --wire decimal --out example/contract.ts --sdk-import ../sdk/index.ts
cargo test --offline
node --test tests/sdk-package.test.mjs
python3 tools/smoke.py
```

wire는 명시해야 한다. safe 또는 decimal은 비교 후보이며 최종 Id 기본값을 선택하지 않았다. 개발 조언은 off/on에서 실행 digest·생성 계약을 바꾸지 않는다. 실패한 정의·원본/하드링크와 같은 출력 경로·잘못된 옵션은 거부한다. 출력은 같은 디렉터리의 임시 파일을 완전히 쓴 뒤 교체한다. 전원 상실 후 디렉터리 엔트리 지속성까지 검증한 것은 아니다.

CLI 테스트는 check/gen13개·init6개·serve10개·DB startup1개·READ 생성/설정2개·WRITE 생성/설정2개·브라우저 옵션3개, 총37개다. 최신 실행 결과는 아래 후속 검증을 따른다. 실제 생성 모듈의 SDK import를 tsc로 확인하고 음성 marker를 제거하면 undeclared Enum 값에서 실패한다. smoke는 소스 SDK와 설치한 SDK 각각에서 safe/decimal의 실제 계약 생성·TypeScript 검사·서버 호출·정의 변경·재기동, 총4개 조합을 수행한다. Sol은 소스 SDK의 decimal 흐름을 별도 컨텍스트에서 재실행했고, 새 Luna는 패키지 테스트4개와 설치 SDK 두 wire 흐름을 독립 재실행했다.

기동만 하려면 새 로컬 전용 schema를 지정한다. init은 행을 넣지 않으며 기본 serve는 개발 토큰을 발급하지 않는다.

```sh
cargo run --offline -- init example/app.aip --schema aip_local_demo --db-url 'host=localhost dbname=postgres'
cargo run --offline -- serve example/app.aip --schema aip_local_demo --db-url 'host=localhost dbname=postgres' --listen 127.0.0.1:8080 --wire decimal
```

서버는 실제 bind 주소·계약 지문을 listening JSON으로 출력하고 SIGINT/SIGTERM에서 요청 task와 listener를 종료한 뒤 stopped JSON을 출력한다. 명시한 `--dev-actor 1`은 **이미 존재하는 actor 행**에 개발용 서명 토큰을 발급한다. `--dev-token-ttl`은 actor 옵션과 함께 쓰며 1~300초, 기본60초다. 신원 확인·로그인·refresh 제공자는 아니다. 기동마다 서명 키가 바뀌므로 이전 토큰은 거부한다. DB 주소는 localhost·loopback·로컬 Unix socket만 허용하며 현재 adapter는 TLS 필수 연결을 지원하지 않는다. URL 오류 원문은 출력하지 않지만 CLI argv에 넣은 값의 로컬 프로세스 노출까지 막는 설정 전달 방식은 아직 없다.

## 구현 전 8개 관문

1. 원칙 1·2·3·6을 실제 기동·호출·정의 변경으로 대조한다. 설정 조립층 증가와 앱별 반복 감소를 구분한다. 생산성 절감률을 미리 선언하지 않는다.
2. 기존 typed read/direct apply 표현을 유지한다. 화면별 사전 정의 Intent를 추가하지 않는다.
3. 앱 정의와 생성 계약을 한 정본에서 관리한다. 화면 변경에 서버 endpoint를 새로 만드는지 실제 파일 변경으로 확인한다.
4. V1 의미 검사·V2/V3 권한/값/비용/트랜잭션·V6 인증/계약 검사를 유지한다. 개발용 seed 토큰과 운영 로그인을 구분한다.
5. 실제 TS 사용 예제를 제공하고 JS/Python worker의 제품 연결·격리와 Python SDK를 별도 잔여 요구로 추적한다.
6. 한 binding과 한 SDK 소스를 사용한다. 구 main의 Intent 이름 전용 API와 새 caller 표현을 혼동하지 않는다.
7. 기존 코드를 재사용하며 DB 접속·listen·안전 초기화의 작은 설정 접점만 추가한다. IR 정교화가 목적이 아니다.
8. wire 모드·작성 형식을 명시적 실험 옵션으로 둔다. 최종 Id·파일 형식·배포/인증·쓰기 조합의 OPEN을 승격하지 않는다.

## 연결한 코드 경계와 범위

- V2 legacy `connect_with_url`은 유지하며 HTTP와 CLI는 공통 `connect_owned_with_url`로 Client와 driver task를 함께 소유한다. drop은 task 취소를 예약하며 DB 커밋 결과를 확정하지 않는다. CLI는 bind 전에 DB·초기화 지문·실제 구조를 확인한다. V6 listener primitive 단독 호출에는 DB 사전 검사 보장이 없다.
- V6 owned server는 listener와 JoinSet 요청 task를 함께 소유한다. 선택 포트·충돌·종료 후 신규 연결 거부와 미완성 HTTP 요청 종료를 확인했다. 기존 start 래퍼는 동일한 시험 동작을 유지한다. 일반 프로토타입 listener에서는 `x-spike-drop-response`를 무시하고 기존 실험 래퍼에서만 고장 주입을 허용한다.
- V2 기존 `ddl`의 DROP은 실험용 API에 남는다. CLI init은 공통 생성 본체의 `create_ddl`만 사용한다. `aip_` 뒤 영문 소문자로 시작하는 ASCII 식별자·63바이트 제한을 검사하고, plain CREATE SCHEMA·앱 테이블·멱등 저장소·marker를 단일 트랜잭션에서 생성한다. 재초기화는 SCHEMA_EXISTS, 중간 DDL 충돌은 DB_INIT과 전체 롤백이다. serve에는 DDL/seed가 없다.
- marker의 DDL digest는 정의 호환, 새 structure digest는 init 이후 실제 구조 변경을 검사한다. 구조 불일치는 SCHEMA_MISMATCH, 빠진 테이블이나 이전 marker-only schema는 SCHEMA_NOT_READY다. schema/데이터를 자동 변경하지 않는다. marker까지 바꿀 수 있는 DB 관리자나 기동 뒤 외부 DDL을 통제하는 보장은 아니다. 실제 마이그레이션은 구현하지 않았다.
- 서버 재시작은 Keyring을 바꾼다. 구 binding 거부는 새 기동의 유효 토큰과 함께 시험해 인증 실패와 구분했다. 같은 actor·wire의 커밋된 key/request는 재기동 후 status로 조회했다. 구 계약의 미확정 pending 전체를 새 계약으로 이전하는 문제는 남는다. 종료 중 실행된 쓰기는 취소만으로 커밋 여부를 확정하지 않는다.

## 실제 사용 흐름의 실행 증거

`python3 tools/smoke.py`는 UUID 전용 schema를 init한 뒤 테스트 행을 별도 SQL로 넣는다. 같은 앱 정본에서 생성한 binding과 단일 SDK로 선택형 read·캐시 hit·direct apply·캐시 무효화·동일 key 재생을 실행한다. 앱의 bulk 상한20을10으로 바꾸고 계약 재생성·재기동 후, 새 토큰+구 binding의 첫 쓰기를 CONTRACT_MISMATCH로 거부하고 새 binding의 쓰기를 실행한다. 최종 DB는 id11/12만 true, 다른 actor99는 false다. 기존 데이터·멱등 결과와 구 토큰 거부도 확인한다. SDK source/package와 두 wire 후보의 조합마다 다른 소유 schema를 쓰며 성공한 init의 schema만 finally에서 정리한다. init의 커밋 결과가 불명인 경우 자동 삭제 대신 상태 확인이 필요하다.

일부러 fault header를 다시 활성화하거나, 설정 DB를 기본 DB로 바꾸거나, init에 DROP 경로를 연결하면 해당 행동 테스트가 실제 실패한다. 세 음성 대조 후 소스를 복원하고 정상 테스트를 재실행했다. V6 전체16개·V2/V3/V5 전체41개 Rust 회귀도 실패0이며 기존 HTTP·세션·복구·타입 대조를 포함한다.

정의 변경은 앱 파일의 bulk 선언과 자동 생성 binding을 바꾸며 서버 endpoint/DTO를 추가하지 않았다. 실제 사용자 업무의 시간 절감률은 측정하지 않았다. 로컬 SDK tarball 빌드·새 소비자의 오프라인 설치·JS 실행·strict TS 타입 검사는 실행했다. 이 단계에서는 전체 제품의 깨끗한 설치 환경·브라우저·WRITE worker 서버 연결·Python SDK·운영 로그인 제공자의 실행 증거가 없었다. 브라우저와 로컬 bundle은 아래 후속 검증으로 보충했다. 공식 READ 연결은 아래 후속 검증을 따른다. V4의 무격리 worker를 이 서버에 그대로 활성화하지 않았다.

## 로컬 SDK 패키지 연결의 구현 전 관문

1. 원칙3·6의 자연스러운 JS/TS 프론트 사용을 위해 repo 밖 소비자 설치를 검증한다. 기존 runtime/type 소스를 재사용하고 빌드 산출물만 만든다.
2. read/apply 표현 범위는 동일하며 패키징이 권한이나 공개 계약을 늘리지 않는다.
3. 서버 파일을 추가하지 않는다. 앱별 생성 binding의 SDK import는 패키지 이름 하나로 연결한다.
4. 기존 서버 최종 검사·SDK 런타임 검사·캐시·pending을 유지한다. 실제 서버 smoke에서 소스 대신 설치 JS를 호출한다.
5. 로컬 npm tarball로 JS와 strict TS 소비자를 확인한다. Python SDK·브라우저 bundler 호환은 별도다.
6. 독립 엔진·SDK fork 없이 기존 다섯 TS 소스의 빌드 결과를 사용한다.
7. 새 빌드 의존성을 네트워크로 설치하지 않고 현재 로컬 TypeScript 컴파일러로 JS와 선언을 출력한다. 기존 출력 디렉터리를 삭제하지 않는다.
8. private 실험 패키지이며 registry publish·최종 패키지 구조·공개 API·기본 Id 후보를 정하지 않는다. 깨끗한 새 소비자 설치와 기존 출력 보호의 행동 테스트를 먼저 실행한다.

## 공식 확장 값 검사 정합의 구현 전 관문

1. 원칙1·4의 서버 최종 권한을 입출력 경계에서도 유지한다. Node/Python 공식 확장의 값 검사를 공통 서버 scalar 규칙으로 연결한다.
2. 기존 공식 확장의 표현 범위와 caller read/apply 범위를 유지한다. 새 HTTP 확장 endpoint를 이 변경에 포함하지 않는다.
3. 앱별 DTO·추가 선언 없이 기존 input/output 계약을 사용한다. 검사 규칙의 복제를 줄인다.
4. Text/Url의 NUL·Time의 실제 날짜/시각을 입력 BAD_VALUE와 출력 OUTPUT_INVALID로 거부한다. nullable 키·Enum·정확한 키 검사와 권한·deadline은 유지한다.
5. JS와 Python의 실제 worker 호출에서도 같은 거부와 정상 값 전달을 확인한다. 원래 입력값을 변환하지 않고 검증한다.
6. V2 scalar parser를 재사용한다. worker 전용 날짜 parser나 별도 값 엔진을 만들지 않는다.
7. 순수 경계 테스트를 먼저 실패시키고, 실제 두 언어 호출과 기존 read/write/outbox 회귀로 실행 경로를 확인한다.
8. 이 변경은 Id 최종 표현·범위·worker HTTP 통합·운영 격리·일반 Int 정밀도의 OPEN을 결정하지 않는다. 기존 V4 Id 동작을 유지한다.

## 로컬 설치형 SDK 사용과 검증

`tools/build-sdk.mjs`는 기존 SDK 다섯 TS 소스의 의존 경로를 컴파일해 JS5개와 선언5개를 만든다. 새 출력 디렉터리만 소유하며 기존 디렉터리·파일·symlink는 OUTPUT_EXISTS로 거부한다. 컴파일 실패 시 자신이 만든 출력만 정리한다. 출력 package는 private인 `@aip/prototype-sdk`이고 registry에 publish하지 않았다. 빌드 컴파일러는 현재 checkout의 로컬 TypeScript를 사용하므로 제품의 독립 설치 도구는 아니다.

```sh
# prototype/에서, 아직 없는 디렉터리를 지정한다.
node tools/build-sdk.mjs --out /tmp/aip-sdk-local
npm pack --offline --ignore-scripts /tmp/aip-sdk-local --pack-destination /tmp
# 별도 소비자 디렉터리에서 생성된 tarball을 설치한다.
npm install --offline --ignore-scripts /tmp/aip-prototype-sdk-0.0.0.tgz
# prototype/에서 앱 binding의 import를 패키지 이름으로 지정한다.
cargo run --offline -- gen example/app.aip --wire decimal --out /tmp/contract.ts --sdk-import @aip/prototype-sdk
```

`node --test tests/sdk-package.test.mjs`의4개 테스트는 실제 offline pack/install, package export의 JS 실행, SDK 캐시, 생성 binding의 strict NodeNext 검사와 undeclared Enum 거부, 출력 파일 보존을 검증한다. `python3 tools/smoke.py --sdk package`는 설치 SDK와 생성 binding을 일반 JS로 컴파일하고 실제 서버에 호출한다. 기본 smoke는 source/package × decimal/safe를 모두 실행한다. transport 파일을 tarball에서 빼거나 컴파일러에 잘못된 옵션을 넣은 음성 대조는 실패했으며 복원 후 테스트4개와 smoke4개 조합을 다시 실행했다. Python SDK·브라우저 bundler·최종 package 구조와 publish 정책은 이 실험의 검증 범위 밖이다.

## worker 프레임 자원 경계의 구현 전 관문

1. 원칙4의 서버 통제를 worker stdout/stderr 수신에도 적용한다. 한 줄 전체를 무제한 할당하는 현재 read/write 경로를 대조한다.
2. 공식 확장 표현과 input/output 계약은 유지한다. HTTP 확장 공개나 새 동작을 추가하지 않는다.
3. 앱별 통신 코드 없이 read/write가 같은 bounded framing helper를 사용한다.
4. newline 전 최대 바이트를 검사한다. 초과 stdout은 worker 실패·폐기로 처리하고 쓰기 트랜잭션은 기존 롤백 경로를 따른다. stderr는 고정 진단을 남기고 잔여 스트림을 버려 pipe 대기를 막는다.
5. Node/Python 실제 초과 프레임·정상 echo와 읽기/쓰기 회귀로 확인한다. 시간 제한에 따른 부분 읽기 취소 후 프레임 복원도 시험한다.
6. 기존 stdin/stdout JSON ABI·권한·deadline·scalar 검사를 유지한다. 프레임 helper만 공유한다.
7. 명시적 WorkerLimits를 사용하고 실험 기본 stdout1MiB·stderr16KiB는 최종 제품 budget으로 승인하지 않는다. 기동 실패를 Result로 받을 수 있는 접점을 추가하고 legacy wrapper를 유지한다.
8. reader 실패·경계 바이트·UTF-8·EOF·취소의 테스트를 먼저 실행한다. 파일/네트워크 격리·worker CPU 전체 제한·최종 배포 구조는 별도 OPEN이다.

## 읽기 worker grant 정리의 구현 전 관문

1. 원칙4의 권한 수명을 호출 종료와 맞춘다. 비활성 Grant가 actor·now·입력 복사본과 함께 호출마다 남는 현재 경로를 실제 호출로 대조한다.
2. 확장 표현·권한·access binding은 유지한다. 새 capability나 API를 추가하지 않는다.
3. 앱별 정리 코드 없이 Worker가 완료·send 실패·후속 호출 시작을 처리한다.
4. 성공·실패 뒤 Grant를 제거하고 최근 만료 토큰 식별자만 최대64개 보관한다. 더 오래된 토큰은 TOKEN_INVALID, 최근 토큰은 TOKEN_EXPIRED로 동일하게 거부한다. 권한을 복구하는 경로는 없다.
5. Node/Python 반복 실제 호출·send 실패·기존 늦은 ctx 재사용 회귀로 확인한다.
6. 현재 단일 mutable Worker 호출 수명과 기존 토큰 매핑을 재사용한다. 별도 메모리 서버나 영속 grant 저장소를 만들지 않는다.
7. externally cancelled 읽기 호출은 다음 유효 읽기 호출 시작에서 이전 Grant를 정리한다. 호출 중 현재 Grant와 작은 최근 토큰 목록만 남기며 업무 입력을 tombstone에 복사하지 않는다.
8. 최대64개는 private 실험의 bounded 진단 기록이고 최종 token/API/worker pooling 정책을 정하지 않는다. 반복 호출 테스트를 먼저 실패시킨다.

## 공식 worker 기반의 사전 검증 기록

V4의 공통 scalar 검사·입출력 프레임 한도·기동 Result·읽기 Grant 정리를 연결했다. Node/Python의 잘못된 값 거부·원문 보존, 초과 stdout의 worker 폐기·실제 쓰기 롤백, stderr 배출·짧은 진단, 반복 호출과 입력 전송 실패·외부 취소 후 Grant 정리를 확인했다. 기존 ctx 권한·deadline·읽기·쓰기·outbox 회귀도 유지했다. `spikes/spike-v4-worker/`에서 `cargo test --offline -- --nocapture`의 Rust23개 실패0, fmt/clippy exit0이다. 독립 코드 검토에서 발견한 Grant 누적은 먼저 실제72회 호출로 재현했다. 고장 주입7종도 행동 실패 후 원복·전체 재실행했다. [상세 결과와 남은 범위](../plan-docs/alignment/V4-official-extension-results.md#9-프로토타입-연결-전-값프레임grant-수명-정합).

이 사전 검증 단위는 HTTP/생성 SDK·safe/decimal 연결을 포함하지 않았다. 후속 READ 연결과 최신 전체 회귀는 아래 기록을 따른다. `Isolation::None`의 직접 DB 연결 가능성은 기존 회귀에서 다시 확인됐고, MacNetDeny는 네트워크만 차단한다. 기존 Legacy Id와 행 가시성에 기대는 쓰기를 새 typed API의 계약으로 승격하지 않는다. 프레임 기본 한도·최근 만료 토큰64개는 private 실험 설정이며 제품의 최종 비용·오류·격리 정책은 아니다.

## 공식 READ 확장 연결의 구현 전 관문

1. 원칙1·2·3·7에 따라 표준 read/apply와 공식 확장을 한 서버·생성 계약·SDK에서 실행한다. 앱별 HTTP endpoint를 새로 작성하지 않는다.
2. 선언한 read/effect none 확장만 보충한다. caller의 선택형 read/direct apply는 유지하며 WRITE 확장을 자동 공개하지 않는다.
3. 기존 V4 worker·V5 타입 생성·V6 세션 전송을 조립한다. 앱 정의의 입출력과 서버가 선택한 구현에서 타입을 생성한다.
4. 새 경로는 유효 서명 세션·필수 계약 지문·정확한 본문·선언된 입출력을 DB 연결/worker 실행 전에 검사한다. actor·access binding·deadline은 서버가 결정한다. SDK도 응답 계약과 인증 세대를 검사한다.
5. Node/Python의 실제 READ 호출과 생성 TS를 시험한다. SafeNumber/DecimalString을 동일하게 입력·출력·ctx 집계에 적용한다. Legacy read/write 계약은 유지한다.
6. 추가 generator family만 사용하고 기존 산출물·지문을 바꾸지 않는다. 새 지문은 공개 확장 이름·입출력 descriptor를 포함하며 내부 구현/정책은 공개하지 않는다.
7. 호출마다 worker를 소유하고 취소·서버 종료에서 정리한다. 명시적 worker-dir/lang opt-in과 macOS 네트워크 차단만 연결한다. 이 로컬 실험의 동시 확장 호출은4개로 제한하며 DB 접속 전에 슬롯을 확보한다. 이 READ 연결 단위는 HTTP 전체 기한·전체 서버 연결 상한을 다루지 않았다. 후속 로컬 실험은 accept부터 파싱5초·동시 연결64개를 적용하며 DB 실행 전체 기한은 보장하지 않는다. 실행 전에 module.function 문법과 실제 모듈 경로를 검증한다. 신뢰한 로컬 확장 코드의 데모이며 파일/Unix socket 격리를 일반 보장하지 않는다.
8. Id 기본값·확장 API·ABI·운영 격리·OS/배포·Python SDK·WRITE 조합은 OPEN이다. wire 경계, 생성 타입, 무실행 선행 거부, 실제 HTTP/SDK 호출을 먼저 실패시키고 검증한다.

## 공식 READ 확장 사용과 실제 검증

`gen`은 앱 정본의 read/effect none 확장 이름·입출력·Enum/nullable descriptor에서 `ExtensionContract`와 binding을 생성한다. 공개 확장 계약은 지문에 포함하며 implementation·access 식·내부 정책은 공개하지 않는다. 현재 prototype family는 확장 유무와 관계없이 공개 readDescriptors를 포함한다. 이 family의 지문이 바뀌므로 binding을 재생성해야 한다. 기존 read/apply-only generator family와 V13/V14/V15 비교 실험의 계약은 유지했다.

SDK의 같은 connect에서 표준 read/apply와 확장을 사용한다. 확장은 결과를 캐시하거나 pending 쓰기를 만들지 않는다. 입력 복사본을 검증해 전송하고 연결 시 wire·descriptor를 고정한다. 반환 객체는 정확한 키·nullable·Enum·NUL·Time·Id를 검사한 뒤 동결한다. `Int`는 JS에서 정밀도를 보존할 수 있는 안전 정수만 허용하며 더 큰 값은 입력 BAD_VALUE/출력 PROTOCOL_ERROR다. 서버의 일반 Int/i64와 최종 숫자 domain은 별도 OPEN이다.

```ts
import {connect} from '@aip/prototype-sdk';
import {contract} from './contract.ts';
const aip = connect(serverUrl, signedToken, contract);
// recruitment fixture를 decimal 모드로 생성한 binding의 예시다.
const stats = await aip.extension('Recruitment.stats', {clubId: '10'});
console.log(stats.approvedApplicants);
```

서버 연결은 `serve ... --worker-dir <trusted-directory> --worker-lang node` 또는 `python`을 명시해야 한다. 두 옵션은 함께 필요하며 현재 macOS 로컬 네트워크 차단 실험으로 제한한다. 없으면 `/extension`은 DB 접속 전에 NOT_FOUND다. 설정 시 module.function 식별자와 모듈 파일의 canonical 경로를 bind 전에 검사하고 디렉터리 밖 모듈 symlink도 거부한다. imported 모듈·파일 전체를 격리하는 보장은 아니다. 신뢰한 demo 코드만 사용하는 범위다.

`POST /extension {extension,input}`은 서명 세션·필수 x-aip-contract·정확한 본문·선언된 입력 검사를 DB 접속/worker 실행 전에 처리한다. 서버의 actor·access/input binding·출력·deadline을 유지하며 호출마다 worker를 소유한다. 로컬 동시 확장4개 상한은 DB 접속 전에 적용하고 완료 뒤 슬롯을 반환한다. worker invoke의 외부 timeout은 막힌 stdin도 포함하지만 DB 접속·spawn·stop·HTTP 전체 기한 보장은 아니다. 네트워크 차단은 TCP와 실제 Unix socket 양성 대조에서 확인했으며 파일·프로세스 후손·다른 OS의 일반 격리를 검증한 것은 아니다.

```sh
# prototype/에서, 설치 SDK와 실제 서버까지 실행한다. 성공한 소유 schema만 정리한다.
node --experimental-strip-types --test tests/extension-sdk.test.mjs
python3 tools/extension_smoke.py
```

extension smoke는 source/package SDK × safe/decimal × Node/Python8개 조합에서 strict TS 생성 타입과 실제 HTTP ctx 집계를 실행한다. 표준 read도 같은 SDK에서 유지하며 캐시/pending 변화 없음, raw 잘못된 Id 거부, 구 지문 거부와 데이터 보존을 검사한다. unknown 확장 이름·잘못된 wire·readonly 대입의 expected-error marker를 각각 제거한3종 타입 음성 대조도 실제 실패한다. 기존 정의 변경·재기동 smoke4개 조합도 다시 실행했다.

이 READ 연결 단계의 Rust 회귀는 V4 25개·V5 20개·V6 21개·CLI22개, 합88개 실패0이었다. 최신 전체 회귀는 아래 후속 검증을 따른다. SDK 경계9개와 설치 패키지4개도 실패0이다. 실제 child PID로 CPU 루프 timeout·서버 shutdown·동시 worker4개/5번째 BUSY·슬롯 반환 뒤 호출과 child 종료를 확인했다. 처음 비동기 대기 fixture는 종료 코드를 꺼도 자연 종료해 음성 대조가 통과했고, CPU 루프로 강화한 뒤 같은 변이는 실패했다. 인증·필수 지문·입력 선검사·네트워크 격리·모듈 경로·동시 상한·child 종료·SDK 출력 검사8종을 끈 고장 주입이 각각 실패한 뒤 소스를 복원하고 전체 회귀를 재실행했다.

공식 WRITE 연결·전체 제품 clean install·TS worker 빌드 전달·Python SDK·운영 인증·마이그레이션·최종 확장 API/ABI/격리는 남는다. 실제 Chrome과 현재 host용 재배치 bundle은 아래에서 검증했다. 이 연결은 실험 후보이며 최종 wire 기본값과 창시자 결정의 OPEN을 승격하지 않는다.


## 브라우저·요청 수명·worker 배포 실험의 구현 전 관문

1. 원칙1·3·6·7을 실제 브라우저 호출과 저장소 밖 worker 실행으로 검증한다. 로컬 CORS 설정·연결 상한의 작은 조립층과 개발자 반복 작업 감소를 구분한다.
2. typed read/direct apply/공식 READ extension 표현을 유지한다. Origin은 실행 권한이나 새 Intent 종류가 아니다.
3. 서버 endpoint나 앱별 DTO를 추가하지 않는다. 별도 로컬 프론트 출처는 serve의 반복 가능한 명시 옵션으로 허용한다.
4. 서버가 Origin·preflight·헤더·본문·세션·계약을 검사한다. 허용된 Origin도 인증·인가를 우회하지 못한다. 요청 파싱 deadline은 DB 실행 전에만 적용해 쓰기 결과를 임의로 확정하지 않는다.
5. 설치한 동일 JS SDK를 실제 headless Chrome에서 실행한다. Node/Python bootstrap은 신뢰된 빌드 소스로 내장하고 사용자 입력을 eval하지 않는다. Python sibling import 계약은 확대하지 않는다.
6. 기존 listener와 SDK를 재사용한다. CORS는 하나의 owned server 옵션이며 별도 브라우저 SDK를 복제하지 않는다.
7. HTTP framing·요청 시간/연결 상한·bootstrap 절대경로 의존만 수정한다. 새로운 IR나 범용 웹 프레임워크를 도입하지 않는다.
8. CORS는 명시적 loopback 개발 설정, 동시 연결64개·파싱5초는 private prototype 실험 기본값이다. 운영 인증·OS 격리·최종 Id/ABI·제품 배포 정책·WRITE/Python SDK OPEN을 확정하지 않는다.


## 로컬 실행 bundle의 구현 전 관문

1. 원칙1·3·6의 실제 사용성을 저장소 밖 실행으로 검사한다. 설치 비용 감소를 수치로 미리 선언하지 않는다.
2. 기존 생성 계약·caller read/apply/READ extension 표현을 그대로 포장한다. 배포 과정이 공개 계약을 바꾸지 않는다.
3. 앱 개발자는 endpoint를 추가하지 않는다. 빌드 binary·SDK tarball·정본 예제/확장·해시 manifest만 새 출력에 담는다.
4. init/serve의 schema 소유·인증·인가·값·비용 경계를 유지한다. bundle은 로그인 제공자나 격리 정책이 아니다. 기존 출력/링크를 덮지 않는다.
5. 로컬 Rust 빌드 결과를 옮긴 뒤 소비자는 Cargo·checkout 없이 JS SDK와 Node/Python worker를 사용한다. 소비자의 Node/Python/PostgreSQL 의존은 명시한다.
6. 원본 엔진/SDK/예제를 재사용하고 산출물만 복사한다. 별도 런타임이나 두번째 앱 정본을 만들지 않는다.
7. packaging과 검증 manifest에만 집중한다. 설치 관리자·운영 배포 프레임워크를 새로 도입하지 않는다.
8. macOS 현재 host용 private 개발 bundle이다. 공개 배포·라이선스·릴리스 ABI·다른 OS·첫 출시 언어 동등성을 확정하지 않는다.


## HTTP 선실행 거부와 SDK 미확정 쓰기 관문

1. 원칙2·6의 서버 최종 판단과 실제 복구를 유지한다. 새 HTTP 거부 코드가 과거 커밋 유무를 알려주지는 않는다.
2. read/direct apply 표현과 같은 key/request 재시도를 유지한다.
3. 앱 개발자는 새로운 복구 endpoint나 DTO를 추가하지 않는다.
4. 첫 응답의 파싱 timeout/Origin 거부는 실행 전 거부다. 이전 응답을 잃은 뒤 같은 거부를 받아도 과거 쓰기가 롤백됐다고 신뢰하지 않는다.
5. 동일 JS/TS SDK pending/cache 규칙을 실제 재시도 테스트로 검증한다.
6. 기존 NOT_EXECUTED 집합과 복구 경로를 재사용한다. 별도 재시도 모델을 만들지 않는다.
7. 새 두 오류 코드의 분류만 보완한다. 캐시·멱등성 구조를 재설계하지 않는다.
8. 확정 근거 부족은 WriteUnsettled로 남는다. 세션·운영 배포·WRITE 조합 OPEN을 임의로 확정하지 않는다.


## 표준 read 결과 검사 구현 전 관문

1. 원칙2·3·6의 생성 계약·실제 SDK 결과 일치를 검사한다. TypeScript cast를 실행 검증 근거로 쓰지 않는다.
2. 동일 선택형 read·공개 관계·필터·정렬을 유지한다. 선택하지 않은 응답 필드를 새 권한으로 받아들이지 않는다.
3. 정본 exposeRead에서 공개 scalar·nullable·관계 선택 descriptor만 생성한다. 앱별 응답 DTO를 추가하지 않는다.
4. 서버의 인가·비용 검사와 SDK의 응답 검사를 구분한다. SDK는 일치 지문을 확인한 뒤 정확한 선택 키·값을 캐시 저장 전에 검사한다.
5. 같은 TypeScript 런타임에서 wire·Bool·Int·Enum·문자열·nullable·관계 결과를 확인한다. Time read 출력은 PostgreSQL 출력 범위가 입력 RFC3339보다 넓어 기존 입력 검사로 임의 축소하지 않는다.
6. 기존 prototype with_extensions family를 하나의 공개 read/apply/extension projection으로 보완한다. 이전 read/apply-only 실험 family는 유지하며 prototype 지문 변경에는 binding 재생성이 필요하다.
7. 기존 fetch/cache 경계에 검증 hook과 요청 snapshot을 추가한다. 캐시 저장·TTL·세션 epoch 구조를 복제하지 않는다.
8. safe/decimal 비교를 유지한다. JS safe Int 출력 밖 값은 정확한 number로 보장할 수 없어 PROTOCOL_ERROR로 거부한다. read Time은 현행 string 타입·NUL 금지를 확인하고 날짜 도메인을 임의로 확정하지 않는다. 최종 값 표현/API/ABI는 OPEN이다.


## 관계 대상의 공개 집계 실행 관문

1. 원칙2·6의 선언·생성 타입·실제 실행 일치를 검증한다. 유효한 caller 표현이 내부 panic을 일으키면 안전한 실행 환경을 제공하지 못한다.
2. V1이 이미 허용한 공개 관계의 대상 집계 선택을 구현한다. 새 문법/관계 깊이를 추가하지 않는다.
3. 서버 endpoint나 DTO를 늘리지 않고 기존 행별 집계 SQL 생성을 재사용한다.
4. 관계 대상 rowRead와 집계 sourceAccess·guard·비용·권한을 모두 유지한다. 단순 column으로 가정하지 않고 공개 선택 종류를 검사한다.
5. 같은 generated TS 선택과 실제 PostgreSQL 결과를 safe/decimal에서 비교한다.
6. root와 관계의 집계 생성 코드를 공유해 같은 의도에 다른 권한 규칙을 만들지 않는다.
7. planner의 기존 표현 해석 오류만 고친다. IR/관계 시스템을 재설계하지 않는다.
8. 이미 의미 검사한 표현의 실행 일치를 보완한다. 새로운 공개 집계·쓰기 조합·최종 문법/API/ABI를 확정하지 않는다.


## WRITE worker 입력 전송 기한 관문

1. 원칙4·7의 확장 실행 통제를 입력 파이프에도 적용한다. READ 공개 계약과 표준 caller 표현은 유지한다.
2. 기존 WRITE 확장의 표현을 늘리지 않는다. 이미 선언한 deadline 뒤에 입력 전송만 계속 대기하는 경로를 고친다.
3. 앱별 timeout·복구 코드를 추가하지 않는다. 동일 서버 Worker 전송 helper를 사용한다.
4. 초기 invoke와 ctx reply에 기존 호출의 남은 기한을 적용한다. 부분 프레임을 남긴 전송 취소는 protocol을 폐기하고 child 종료를 요청한다. 커밋 전 실패는 DEADLINE_EXCEEDED이며 커밋 결과 불명과 구분한다.
5. 실제 Node/Python에서 ctx 쓰기 뒤 CPU 루프·worker 재사용·큰 입력의 pipe 막힘을 재현하고 DB rollback·후속 재사용 거부를 확인한다.
6. 기존 JSON 줄 ABI와 트랜잭션 실행을 재사용한다. 공개 WRITE API나 별도 전송 모델을 만들지 않는다.
7. worker 입력 전송의 기한만 보완한다. DB 접속·트랜잭션 시작·rollback 정리 전체의 절대 시간 보장으로 확대하지 않는다.
8. 최종 WRITE API·wire·worker 격리·조합은 OPEN이다. 실제 실패 테스트 뒤 수정하며 기존 늦은 ctx 거부와 COMMIT_UNKNOWN 분류를 보존한다.


## 브라우저·read decoder·로컬 bundle 후속 검증

`serve --allow-origin http://localhost:<프론트-port>`를 반복 지정하면 명시한 loopback HTTP 출처를 허용한다. 기본 동일 출처도 실제 bind 포트와 Host를 확인한다. wildcard·null·비loopback·경로·빈 명시 포트와 중복 Origin/Host를 거부하며 OPTIONS는 DB·인증 전 처리한다. 허용 출처의 정상 응답과 파싱된 인증/계약 오류에는 ACAO를 포함한다. 불완전하거나 상한을 넘긴 헤더의 모든 오류를 브라우저에 노출한다는 보장은 없다. CORS는 인증·인가나 완전한 CSRF 방어가 아니다.

owned listener는 동시에64개 요청 연결을 소유하고 accept부터 헤더/본문 파싱을 하나의5초 기한으로 제한한다. drip 전송으로 기한을 갱신할 수 없다. 응답 전송도 별도5초이며 이때 이미 커밋됐을 수 있다. 파싱 기한을 DB 실행 취소나 rollback 증명으로 쓰지 않는다. 부분 헤더·미완성 body·drip·65번째 연결 종료·슬롯 반환·unknown route DB 선실행 거부를 실제 TCP로 확인했다.

현재 prototype 생성 binding의 필수 readDescriptors는 공개 scalar·nullable·관계 선택만 포함한다. SDK는 query/descriptor snapshot을 사용해 정확한 선택 키와 값·wire·단일 단계 관계를 캐시 저장 전에 확인한다. 잘못된 Id/Bool/Enum/Int·누락/추가 필드·비객체 행은 PROTOCOL_ERROR이며 캐시에 넣지 않는다. 기존 관계의 빈/중복 선택은 집합 의미를 유지한다. read Time은 PostgreSQL 정상 출력 범위를 임의로 좁히지 않고 string/NUL을 확인한다. 확장 Time 입출력의 RFC3339 검사는 별도다. JS Int 안전 정수 밖의 값은 정확한 number로 보장하지 않는다. 기존 apply-only SDK 비교 실험은 같은 검증 보장을 제공하지 않으며 새 prototype binding을 재생성해야 한다.

V2는 이미 V1/TS가 허용한 관계 대상의 공개 집계를 root 집계와 같은 helper로 실행한다. 실제 PG에서 대상 rowRead/fieldRead·sourceAccess·guard·비용·의존성과 두 wire를 검증했다. sourceAccess의 대상 this 입력은 현 V1이 허용하지 않으므로 그 사례까지 검증한 것은 아니다.

실제 설치한 SDK를 headless Chrome에서 실행한 base2개·READ extension4개 흐름은 허용 출처의 읽기·쓰기·캐시 무효화·Node/Python 집계, 인증/구 지문 거부·불허 출처의 preflight 거부와 DB 무변경을 확인했다. 다른 브라우저·모바일·운영 인증을 검증한 것은 아니다. Playwright는 별도 임시 venv와 이미 설치된 Chrome을 사용했다.

```sh
# prototype/에서 실행한다. 브라우저 smoke에는 Playwright와 설치된 Chrome이 필요하다.
node --experimental-strip-types --test tests/extension-sdk.test.mjs tests/read-decoder.test.mjs tests/http-recovery.test.mjs tests/sdk-package.test.mjs
python3 -m unittest discover -s tests -p test_bundle.py -v
python3 tools/browser_smoke.py
python3 tools/build-bundle.py --out <없는-출력-디렉터리>
python3 tools/bundle_smoke.py --bundle <bundle-절대경로>
```

`tools/build-bundle.py`는 현재 macOS host용 binary·SDK tarball·정본 예제/확장·명시 seed·SHA manifest를 fresh 경로에 포장한다. 출력 파일/디렉터리/symlink를 보존하며 실패 시 자신이 만든 경로만 정리한다. trusted worker bootstrap은 바이너리에 내장해 checkout의 절대 경로에 의존하지 않는다. Python3.11+의 safe-path로 cwd 표준 모듈 shadowing을 거부하며 Python sibling import 계약을 확대하지 않았다.

`tools/bundle_smoke.py --bundle dist/local-macos-20261005-validated`는 공백이 있는 별도 경로로 복사하고 manifest/hash를 검사한 뒤 Cargo·checkout runtime asset 없이 실제6개 흐름을 실행했다. Event safe/decimal은 read/cache/apply/invalidation/replay·구 지문 첫 쓰기 거부·pending0과 DB를 확인했다. Recruitment safe/decimal × Node/Python은 실제 집계1·club11 EXTENSION_ERROR·중첩 read/no-cache·비공개 close NOT_EXPOSED·DB PUBLISHED 보존을 확인했다. 예제 권한을 넓히거나 가짜 응답으로 통과시키지 않았다. manifest의 로컬 입력 SHA는 외부 registry cache나 bit-for-bit 재현성을 보증하지 않는다. 제품 clean install·다른 OS/CPU·최종 패키지 정책은 남는다.

이 후속 단위의 새 회귀는 V2/V3/V4/V5/V6/CLI 순서로20/5/29/21/29/25, 총129개 실패0이다. SDK 경계/설치 패키지25개와 bundle builder5개도 실패0이다. source/package 정의 변경 smoke4개·READ extension smoke8개·Chrome6개·재배치 bundle6개를 실제 실행했다. Origin/연결 상한/read decoder/미확정 복구/대상 집계 guard5종 고장 주입은 각각 행동 실패를 확인한 뒤 source bytes를 복원하고 회귀를 재실행했다. WRITE worker 입력 전송의 뒤이은 보완과 최신 전체 수는 아래 기록을 따른다.


## WRITE 확장의 호출자 소유 트랜잭션 실험 관문

1. 원칙4·6·7에 따라 확장 효과와 transport의 멱등 결과를 한 트랜잭션에 담을 준비를 한다. HTTP 통합 없이 자체 commit을 전제로 두면 원자성을 검증할 수 없다.
2. 기존 extension write 선언·공개 전이·Legacy Id 표현은 그대로다. 새 프론트 API나 쓰기 조합 표현을 추가하지 않는다.
3. 앱 개발자의 정의/구현을 바꾸지 않는다. 기존 V4 실행 본체를 공유하고 caller가 연 transaction에서 실행할 내부 실험 함수를 보충한다.
4. actor·허용 access·ctx 실패 오염·출력 계약·커밋 검사·deadline을 유지한다. 보충 경로는 commit하지 않고 기한을 caller에 돌려주며 caller가 후속 기록과 commit 결과를 책임진다.
5. Node/Python 실제 확장의 성공 뒤 caller rollback/commit과 공개 전이 권한 거부를 PostgreSQL에서 검증한다. worker ABI와 구현은 재사용한다.
6. Legacy invoke_write는 같은 본체를 호출해 기존 auto-commit 결과를 유지한다. 실행 엔진을 복제하지 않는다.
7. 트랜잭션 소유 경계만 분리한다. Id 변환·생성 계약·HTTP/SDK 노출은 이 단위에 넣지 않고 후속 테스트로 다룬다.
8. private Rust spike 함수이며 최종 WRITE API/ABI·조합·운영 격리의 OPEN을 확정하지 않는다. 성공 뒤 caller rollback이 실제 DB를 보존하는 테스트부터 실행한다.


## WRITE 확장 wire·출력 선커밋 검사 관문

1. 원칙3·4·7의 JS/Python 실행과 서버 값 통제를 WRITE에도 적용한다. Id 후보를 다르게 보내도 DB 권한과 원자성이 달라지면 안 된다.
2. 기존 확장 선언과 ctx.data.apply 공개 전이만 쓴다. 새로운 호출자 조합이나 문법은 추가하지 않는다.
3. 앱별 Id 변환 코드를 요구하지 않는다. 공식 worker의 기존 문자열 target Id를 서버 경계에서 선택한 wire로 바꾼다.
4. 선언한 입력·ctx target·ctx 결과·최종 출력을 서버가 검사한다. 선택 wire 밖 Id나 JS 안전 정수 밖 Int 출력은 caller commit 전에 실패한다. 호출자 소유 transaction의 실패는 caller가 전체 rollback/drop해야 한다.
5. Node/Python과 safe/decimal에서 입력과 ctx 결과가 같은 Id를 보존하는지 실제 PG로 확인한다. 큰 decimal Id와 잘못된 wire/출력의 롤백을 검증한다.
6. V2 Id parser/encoder·V3 apply_in_with_wire·기존 V4 scalar checker와 실행 본체를 공유한다. 기존 Legacy 함수의 입출력을 유지한다.
7. 보충 Rust 함수만 추가하며 HTTP·SDK의 WRITE 공개 API는 후속 단위로 분리한다. 별도 값 엔진이나 worker ABI를 만들지 않는다.
8. 두 wire는 실험 후보다. 최종 Id·Int domain·WRITE API/ABI·운영 격리의 OPEN을 확정하지 않는다. typed SDK의 현재 safe Int 경계와 같은 값만 이 경로에서 commit 준비 상태로 인정한다.


## WRITE 확장 실행·멱등 결과의 원자적 저장 관문

1. 원칙2·4·6·7에 따라 응답 손실 뒤 재시도에도 서버 확장을 중복 실행하지 않는다. 자체 commit을 분리한 기반 위에서 기존 transport 멱등 저장을 재사용한다.
2. extension write와 공개 전이의 기존 의미를 사용한다. 이 단계는 Rust transport 실험 경계이며 새로운 frontend 조합이나 문법을 추가하지 않는다.
3. 앱 정의·worker 구현·endpoint를 추가하지 않는다. 같은 principal/wire/key와 저장소를 표준 direct apply와 공유한다.
4. canonical request/key·선언 입력·worker 설정을 검사하고 key lock 후 저장 결과를 확인한다. 확장 효과·출력 계약·멱등 결과를 한 transaction에 묶으며 실패/기한 초과에는 전체 drop/rollback한다. commit 결과 불명만 COMMIT_UNKNOWN이다.
5. actual Node/Python·PG에서 두 wire의 최초 실행/재생·같은 key 다른 요청·worker 실행 횟수·실패 뒤 data/idem 부재를 확인한다.
6. 기존 V4 caller-owned 실행·V6 principal/key/태그/저장소를 사용한다. 별도 idem DB나 worker 실행 엔진을 만들지 않는다.
7. idempotency 기록/commit까지 남은 확장 기한을 적용한다. 공개 HTTP/typed SDK는 검증 뒤 별도 접점으로 연결하며 이번 내부 함수만으로 인증을 제공한다고 주장하지 않는다.
8. private 실험 함수이며 trusted Caller/typed facts를 받는다. 최종 WRITE API·wire·ABI·조합·운영 격리의 OPEN을 확정하지 않는다. 커밋과 재생의 actual PG 테스트부터 실행한다.


## WRITE 공식 확장의 private HTTP·생성 SDK 연결 관문

1. 원칙1·2·3·6·7을 기존 표준 caller read/apply와 공식 WRITE 호출을 같은 서버/SDK에서 쓰는 흐름으로 검증한다. 앱별 endpoint나 복구 코드 반복을 줄인다.
2. 기존 extension write 선언과 access의 공개 전이만 제공한다. caller의 자유로운 write composition이나 새 문법은 추가하지 않는다.
3. 앱 정본에서 공개 WRITE 입출력 descriptor/type을 생성한다. 명시적 실행 opt-in은 서버 설정이며 앱별 DTO를 작성하지 않는다.
4. 서명 세션·필수 생성 계약 지문·정확한 request/key·선언 입력·worker 경로·동시 상한을 DB 실행 전에 확인한다. 기존 /apply의 key lock/status/pending을 공유하며 효과·출력·멱등 기록을 한 transaction에서 commit한다.
5. 같은 connect의 실험 writeExtension(name,input,{key})와 생성 타입을 JS/TS에서 확인하고 실제 Node/Python 서버를 두 wire로 실행한다. 잘못된 출력은 pending 확정·캐시 무효화 전에 거부한다.
6. V4/V6의 준비된 실행 본체·멱등 저장·재시도·세션·캐시를 재사용한다. WRITE 전용 SDK/별도 endpoint/복구 저장소를 만들지 않는다.
7. 소유한 worker는 호출별 정리하고 READ와 같은 로컬 동시4개 슬롯을 DB 접속 전에 확보한다. 최초 거부와 이전 응답 손실 뒤의 선실행 거부를 구분한다. 공개 생성 지문은 input/output/access의 공개 전이 이름만 포함하며 구현/내부 정책은 포함하지 않는다.
8. 현재 macOS·trusted code의 private prototype 후보 API다. 최종 API/ABI·Id/Int domain·파일 형식·WRITE 조합·운영 인증/격리·다른 OS 정책을 확정하지 않는다. 타입·HTTP·SDK 미확정 복구·실제 서버 흐름을 먼저 실패시킨 뒤 연결한다.


## WRITE 실행 제한과 정본 예제 구현 전 관문

1. 원칙2·4·6·7에 따라 실제 worker의 기한·소유·멱등 결과와 사용 가능한 예제를 검증한다. 통합 코드 존재만으로 실행 제한을 보증하지 않는다.
2. 기존 Event 공개 mark와 extension write 선언만 사용한다. 새 쓰기 조합·문법을 추가하지 않는다.
3. 정본 example/app.aip에 작은 공식 WRITE 구현을 연결하고 bundle에 같은 파일을 담는다. 앱별 endpoint·DTO·재시도 엔진을 만들지 않는다.
4. 실제 Node/Python의 ctx 쓰기 뒤 CPU 루프에서 동시4개 상한·기한 초과·child 종료·rollback·슬롯 반환·server 종료를 확인한다. 응답 손실 뒤 복구에는 같은 key/request를 사용한다.
5. 같은 generated JS SDK를 실제 Chrome과 checkout 밖 재배치 bundle에서 실행한다. 두 wire와 worker 언어의 WRITE 출력·재생·캐시 무효화·권한 거부를 검사한다.
6. V4/V6 실행 본체·SDK·기존 smoke와 bundle builder를 재사용한다. 원본 엔진이나 두번째 예제 정본을 복사하지 않는다.
7. WRITE는 명시적 --enable-write-extensions 설정에서만 실행한다. 개발 seed는 사용자 선택 schema에 명시적으로 넣으며 기존 schema/output은 덮지 않는다.
8. 현재 macOS trusted-code private prototype 후보다. 최종 API/ABI·Id·격리·로그인·다른 OS·WRITE 조합 OPEN을 유지한다. 배포본 완성이나 전체 절대 기한을 주장하지 않는다.


## bootstrap 캡처 완료 시점 검증 관문

1. 원칙4·6의 실제 검증 근거를 보존한다. 생성된 파일의 존재만으로 쓰기가 끝났다고 판단하지 않는다.
2. 기존 worker bootstrap·공개 계약·caller 표현은 유지한다.
3. 앱 코드나 endpoint를 추가하지 않으며 test-owned fake runtime만 조정한다.
4. 캡처 helper는 모든 argv를 기록한 ready marker를 기다린 뒤 owned worker를 종료한다.
5. 실제 Node/Python bootstrap 인자를 순차 지연 기록해 partial file race를 결정적으로 재현한다.
6. 기존 테스트와 Worker를 사용하며 production bootstrap을 복제하거나 완화하지 않는다.
7. 시험 기록의 완료 경계만 보완한다. 다른 런타임/프로세스 격리 보장으로 확대하지 않는다.
8. 결정적 RED 뒤 기존4개 테스트·전체 회귀·clippy를 실행한다. 최종 ABI/API·배포 OPEN은 그대로다.


## WRITE 공식 확장의 실행 결과

정본 `example/app.aip`는 공개 Event.mark를 호출하는 `Event.confirm`을 선언한다. Node/Python의 `example/extensions/event.mjs`·`event.py`는 같은 ctx.data.apply를 사용한다. count는 이번 의도에서 변경한 행 수다. 동일 key/request는 최초 output을 재생하며, 새 key로 이미 checked인 행을 확인하면 repeat unchanged에 따라 count 0이다. 생성 binding은 CLI로 재생성했으며 SDK 소스·strict TS 소비자가 같은 계약을 사용한다.

```ts
import {connect} from '../sdk/index.ts';
import {contract} from './contract.ts';
const client = connect('http://127.0.0.1:8080', devToken, contract);
const result = await client.writeExtension('Event.confirm', {id:'11'}, {key:'event-confirm-11'});
if (result.ok) console.log(result.output.id, result.output.count);
```

서버는 paired `--worker-dir example/extensions --worker-lang node`와 `--enable-write-extensions`를 명시해야 WRITE를 실행한다. Python은 worker-lang만 바꾼다. opt-out·서명 세션·필수 생성 지문·정확한 body/key·선언 입력·동시4개 제한을 DB 접속 전에 검사한다. 호출별 worker는 기존 macOS 네트워크 차단을 사용한다. 기본 표준 read/apply는 이 opt-in 없이도 동작한다. 개발 토큰과 데이터 seed는 별도이며 `init`이나 `serve`가 예제 행을 자동으로 넣지 않는다.

기존 V4 실행 본체를 caller-owned transaction으로 공유했다. 확장 효과·출력 계약·멱등 결과를 같은 transaction에 묶으며 principal/wire/key와 저장소는 표준 apply와 같다. key lock 이후 저장 결과가 있으면 worker 없이 재생한다. HTTP /status도 같은 key/request의 결과를 반환한다. 내부 prepare 함수의 Err는 trusted Rust caller가 transaction 전체를 rollback/drop해야 한다. Legacy invoke_write는 기존 자동 commit 경로를 유지한다.

실제 PG/Node/Python에서 같은 key의 동시 호출은 worker 한 번만 실행했다. 결과 INSERT 제약 오류·foreign actor ctx 실패·안전 정수 밖 출력은 효과와 멱등 기록 모두 롤백했다. 실제 deferred commit trigger로 응답 기한을 넘긴 경우 COMMIT_UNKNOWN을 반환했고, 뒤이은 같은 key 호출은 커밋된 결과를 worker 재실행 없이 재생했다. commit 전 기한 실패와 commit 결과 불명은 구분한다. DB 접속·트랜잭션 시작·key lock·rollback 정리까지 전체 절대 기한으로 감쌌다는 보장은 없다.

WRITE의 selected wire는 입력·ctx target·ctx reply·최종 출력에 같은 V2/V3 parser/encoder를 쓴다. decimal의 i64::MAX Id는 실제 worker/PG 왕복을 확인했다. Int는 현재 JS SDK가 정확히 표현할 안전 정수만 commit 준비 상태로 인정한다. SDK는 알려진 WRITE 이름·input snapshot·공개 output descriptor·지문을 검사한 뒤 pending을 확정하고 read cache를 무효화한다. 잘못된 성공 응답은 WriteUnsettled로 남긴다. `post`는 기존 고급 transport helper이며 임의 raw 호출에 facade의 모든 검증 보장을 적용한다는 뜻은 아니다.

actual WRITE smoke8개는 source/package × safe/decimal × Node/Python에서 read/cache·최초 WRITE·재생·공유 key 충돌·실제 커밋 응답 손실과 retryPending·권한/unsafe output 거부를 확인했다. 각 조합의 DB는 11/12=true, 13/99=false이며 멱등 기록2개·worker 실제 호출4회다. 응답 손실은 소비자가 실제 응답을 읽고 버리는 fetch wrapper로 주입하며 일반 listener에 고장 header를 활성화하지 않는다.

실제 Chrome8개는 Event의 표준 apply+WRITE4개와 Recruitment READ4개를 실행했다. 허용 출처에서 WRITE 손실 복구·재생·캐시 무효화·권한 거부·인증/지문 오류를 읽고, 불허 출처의 preflight와 raw WRITE는 거부되며 DB가 그대로였다. invalid token의 SDK 호출은 /session에서 Error를 던지므로 성공/실패 WRITE envelope를 가정하지 않고 실제 error code를 확인했다.

[WRITE 포함 로컬 bundle](dist/local-macos-20261005-write-integrated/README.md)은 CLI·offline SDK·두 정본 예제·Node/Python 구현·명시 seed와 source/output SHA manifest를 담는다. `bundle_smoke.py`는 공백이 있는 fresh 경로에서 bundled binary/SDK/worker만으로8개 흐름을 실행했다. Event4개는 실제 WRITE 응답 손실 복구·동일 key 재생·새 key count 0·foreign actor 거부를, Recruitment4개는 기존 집계/정책을 확인했다. 실행 중 Cargo나 checkout runtime asset은 쓰지 않는다. 다른 host·제품 clean install·운영 인증/파일/후손 격리는 검증 범위 밖이다.

실제 CPU loop4개 후 다섯 번째 WRITE는 WORKER_BUSY이며 새 worker를 만들지 않았다. 기한 뒤 child 종료·효과/결과 rollback·슬롯 반환 뒤 정상 WRITE·서버 shutdown 중 child 종료와 row lock 반환을 Node/Python에서 확인했다. MVCC의 false만으로 rollback을 증명하지 않고 FOR UPDATE로 shutdown 뒤 row lock이 풀렸는지도 검사했다. 동시 슬롯 제한과 key lock을 각각 끈 음성 대조는 초과 worker 실행과 중복 실행으로 행동 FAILED였으며 원본 bytes를 복원했다.

회귀 중 bootstrap argv 테스트의 파일 존재/완료 race도 actual 부분 기록으로 발견했다. 순차 지연 기록에서 Node/Python 모두 결정적 RED였으며, 캡처 완료 marker를 기다리도록 test-owned helper만 보완한 뒤 기존4개가 GREEN이었다. production bootstrap이나 ABI를 완화하지 않았다.

```sh
# prototype/에서 실행한다. Chrome smoke는 격리된 Playwright 환경과 설치된 Chrome이 필요하다.
node --experimental-strip-types --test tests/extension-sdk.test.mjs tests/read-decoder.test.mjs tests/http-recovery.test.mjs tests/sdk-package.test.mjs tests/write-extension-sdk.test.mjs
python3 -m unittest discover -s tests -p test_bundle.py -v
python3 tools/smoke.py
python3 tools/extension_smoke.py
python3 tools/write_extension_smoke.py
python3 tools/browser_smoke.py
python3 tools/bundle_smoke.py --bundle dist/local-macos-20261005-write-integrated
```

WRITE 연결 단위의 회귀는 V2/V3/V4/V5/V6/CLI 순서로20/5/32/22/32/27, 총138개 실패0이다. DB 연결 후속은 당시139개였으며 최신 범위와 수치는 마지막 후속 결과를 따른다. SDK/패키지37개·bundle builder5개도 실패0이다. source/package 정의 변경 smoke4개·READ8개·WRITE8개·실제 Chrome8개·재배치 bundle8개를 확인했다. 변경 package의 fmt/clippy -D warnings와 정본 binding strict TS 검사도 exit0이다. plan-docs155개 구조 검사와 변경 문서5개의 로컬 링크/WRITE anchor 검사는 오류0이었다. 이번 WRITE 동시 상한/key lock 음성 대조2종은 행동 실패 후 원본 bytes를 복원했다. 앞 절의129/25/Chrome6/bundle6은 이전 후속 단위의 당시 수치다.

이 연결은 private prototype의 후보 API다. 본 crates 제품 통합·최종 작성 형식/API/ABI·Id 기본값·WRITE 조합·Python 프론트 SDK·운영 로그인·일반 마이그레이션·다른 OS/격리·제품 설치 정책은 OPEN으로 유지한다.


## HTTP DB 연결 대기 경계 구현 전 관문

1. 원칙4·6·7에 따라 DB가 TCP만 받아 응답하지 않아도 HTTP 요청·확장 슬롯을 무기한 붙잡지 않는다.
2. 기존 read/apply/WRITE 표현과 실행 기한·커밋 의미를 유지한다.
3. 앱별 DB timeout·복구 코드를 추가하지 않고 V6의 공통 연결 경계만 보완한다.
4. 실행/transaction 이전 connect handshake를 별도5초로 제한해 DB_UNAVAILABLE을 반환한다. 이전 미확정 쓰기의 commit 여부는 이 오류로 확정하지 않는다.
5. test-owned loopback 소켓이 접속만 받고 protocol 응답을 하지 않는 실제 반례에서 read/WRITE의 기한·socket EOF·동시4개 제한·슬롯 반환을 확인한다.
6. 기존 V2 connector·V6 오류·SDK NOT_EXECUTED를 사용한다. 별도 DB adapter나 retry 엔진을 만들지 않는다.
7. connect/auth handshake만 제한한다. 연결 뒤 SQL·transaction·clock query·전체 HTTP의 절대 deadline 보장으로 확대하지 않는다.
8. private prototype의 기존 CLI 연결 상한과 정합한다. DB/운영 TLS/격리·최종 API 결정을 임의로 바꾸지 않는다. actual TCP RED 뒤 구현하고 최신 bundle을 fresh 경로에 다시 만든다.


## HTTP DB 연결 대기 후속 검증

CLI startup의5초 상한과 달리 개별 HTTP의 DB connect/auth handshake는 무기한 대기할 수 있었다. test-owned loopback DB가 TCP만 받고 아무 PostgreSQL 응답도 보내지 않는 반례에서 read1개·WRITE4개가 계속 대기하고 socket/확장 슬롯도 반환되지 않는 actual RED를 확인했다.

V6의 공통 connector 호출만 별도5초 timeout으로 감쌌다. 기한 뒤에는 DB_UNAVAILABLE이며 transaction/worker 실행 전이다. 같은 연결 대기 중 WRITE4개는 기존 상한을 차지하고5번째를 WORKER_BUSY로 거부한다. 기한 뒤 DB socket5개 EOF와 슬롯 반환을 확인했고, 이어진 WRITE도 BUSY 대신 DB_UNAVAILABLE로 반환하며 여섯 번째 socket이 닫혔다. SQL·clock query·transaction·commit·HTTP 전체에 이5초 상한을 적용했다는 보장은 없다. SDK는 첫 DB_UNAVAILABLE과 과거 응답 손실 뒤의 같은 오류를 이미 구분하므로 미확정 쓰기를 임의로 확정하지 않는다.

```sh
# spikes/spike-v6-transport/에서
cargo test --offline --test db_connect_lifetime -- --nocapture
# prototype/에서
python3 tools/write_extension_smoke.py
python3 tools/browser_smoke.py
python3 tools/bundle_smoke.py --bundle dist/local-macos-20261005-db-bounded
```

[당시 실행 bundle](dist/local-macos-20261005-db-bounded/README.md)은 이 연결 상한을 포함한다. source64개/output11개 manifest의 현재 source mismatch0을 확인했다. fresh 경로에 생성해 이전 bundle을 덮지 않았다. 새 코드에서 WRITE source/package8개·실제 Chrome8개·재배치 bundle8개를 다시 실행해 모두 ok:true/exit0이었다. V6 전체33개·CLI27개는 실패0이며, 변경 없는 V2/V3/V4/V5의 최신20/5/32/22개와 합쳐139개다. SDK37개·builder5개도 앞선 동일 코드의 검증을 유지한다. V6 fmt/clippy -D warnings는 exit0이다. 이전 WRITE 통합 bundle/138개와 이 DB handshake 단위139개는 당시 snapshot이다. 현재 bundle과 회귀 수치는 마지막 후속 결과를 따른다.


### 후속 관문: 요청 소유 DB 연결과 실행 전 SQL 대기

1. 원칙 2·6·7의 서버 실행 책임을 구현한다. 취소된 요청의 DB driver를 서버가 소유하며 연결 수명과 SQL 대기를 제한한다. 기한을 새 제품 계약으로 확정하지 않는다.
2. 허용되는 Intent 범위는 그대로이고 실행 전 DB 장애가 무한 대기로 바뀌는 경계만 닫는다.
3. 앱별 서버 코드는 늘지 않는다. 기존 V2 connector를 재사용하고 수명 소유 타입만 추가한다.
4. 인증·계약·확장 권한 검사는 유지한다. 연결 성공을 SQL 응답 보장으로 신뢰하지 않는다. 연결 취소가 DB 롤백 완료를 뜻한다고 주장하지 않는다.
5. Node/Python 공식 확장과 SDK의 동일 key 복구 흐름을 유지한다. 실행 전 조회 실패는 DB_UNAVAILABLE, 이미 불명확한 커밋은 COMMIT_UNKNOWN 의미를 보존한다.
6. legacy Client connector는 유지하고 HTTP·프로토타입 기동에서는 owned connector 하나를 사용한다. 새 호출 표현·worker 구현 방식을 만들지 않는다.
7. task 소유권은 자원 누수 방지를 위한 내부 수단이다. 독립 엔진·SDK·planner를 복제하지 않는다.
8. 최종 API·Id·ABI·WRITE 조합·운영 인증·OS 격리 선택은 OPEN 그대로 둔다. 실제 SQL 응답 정지 테스트를 먼저 실패시킨 뒤 구현한다.


### 후속 관문: 초기화 구조와 실제 catalog 대조

1. 원칙 2·4·6의 정형 계약과 서버 무결성 책임을 구현한다. init이 실제 생성한 구조와 serve 시점의 구조를 비교하며 일반 마이그레이션을 대체하지 않는다.
2. 유효한 Intent 표현은 유지한다. 같은 이름이지만 다른 컬럼·제약·인덱스·객체 종류가 되어 계약을 집행할 수 없는 DB의 기동만 거부한다.
3. 앱별 검증 SQL은 늘지 않는다. 공통 PostgreSQL catalog snapshot을 init/serve가 재사용한다. DDL parser·shadow schema·새 엔진을 만들지 않는다.
4. 예상 DDL digest는 정의 호환, 실제 구조 digest는 초기화 이후 외부 SQL drift 검사로 역할을 분리한다. marker까지 변경할 수 있는 DB 관리자에 대한 보안 증명은 아니다.
5. Node/Python·생성 SDK·safe/decimal 계약은 그대로다. 데이터와 시퀀스 현재값을 digest에 넣지 않아 정상 호출·seed·재기동을 유지한다.
6. 호출 표현은 추가하지 않는다. schema 전용 marker 내부 형식만 보완하며 이전 marker-only schema를 자동 변경하거나 재초기화하지 않는다.
7. catalog fingerprint는 구조 확인의 내부 수단이다. 행 가시성·정책·서버 런타임 검증을 대신하지 않는다.
8. 최종 DB migration/호환/API/파일 형식은 OPEN이다. private init marker 형식 변경과 검사 범위를 명시하며 소유한 새 테스트 schema에서 실제 drift RED를 먼저 확인한다.


### 후속 관문: CLI 초기화·기동 SQL 시간 상한

1. 원칙 2·4·6에 따라 서버 기동·초기화의 DB 응답 대기도 호출자가 예측 가능한 실패로 받는다.
2. 표현할 Intent는 바꾸지 않는다. 초기화와 읽기 전용 기동 검사의 무한 대기를 닫는다.
3. 앱별 기동 timeout 구현 없이 CLI 공통 SQL stage만 감싼다.
4. owned 연결을 재사용하고 timeout 뒤 driver 취소를 예약한다. 초기화 커밋 결과를 추측하지 않고 INIT_UNSETTLED로 보존하며 serve 검사는 DB_PREFLIGHT로 거부한다.
5. Node/Python worker·SDK·세션 발급은 검사가 끝난 뒤 기존 경로를 따른다.
6. 새 API·호환 경로·자동 migration을 만들지 않는다. connect5초와 SQL stage5초는 별도이며 전체 실행 기한은 아니다.
7. timeout은 자원 수명 통제를 위한 수단이다. socket EOF를 DB rollback 증명으로 사용하지 않는다.
8. 최종 비용·운영 기동·migration 정책은 OPEN이다. actual PG 인증 이후 SQL을 정지시킨 CLI init/serve 행동 테스트를 먼저 RED로 확인한다.


### 후속 관문: 실행 SQL 대기와 커밋 불명 분리

1. 원칙 2·6·7의 실행 통제를 연결 후 전체 operation(SQL begin/설정/key lock/읽기·쓰기/결과 저장/commit)에도 적용한다.
2. 허용한 Intent·선택형 타입은 유지한다. 이미 선언한 read/확장 deadline 및 V3 공통 쓰기 기한을 실제 transport transaction까지 적용한다.
3. 앱별 timeout이나 재시도 저장소를 추가하지 않는다. 기존 planner deadline·WRITE_DEADLINE_MS·확장 선언을 재사용한다.
4. commit 시작 전 취소는 DEADLINE_EXCEEDED, commit 시작 후는 COMMIT_UNKNOWN으로 구분한다. read-only status 조회 정지는 DB_UNAVAILABLE이다. 소켓 정리를 DB rollback/commit 결과 확정으로 사용하지 않는다.
5. Node/Python worker는 같은 권한·기한·owned cleanup이며 SDK는 기존 같은 key/request 복구를 유지한다.
6. 표현·최종 API·write 조합은 추가하지 않는다. 전송 transaction wrapper의 누락된 기한만 보완한다.
7. operation timer는 driver·확장 슬롯 수명 통제 수단이다. HTTP 파싱·연결·clock·응답은 별도 stage 기한이다.
8. 최종 비용·Id·인증·운영 격리·전체 request 비용 계약은 OPEN이다. real PG clock 응답 이후 SQL을 정지시키는 반례를 RED로 확인하고 실제 커밋 불명/동일 key 복구 회귀를 유지한다.


## DB 수명과 구조 검증 결과

실제 PostgreSQL 인증은 relay하고 그 뒤 SQL 응답만 멈추는 중계 테스트를 추가했다. clock 조회에서5개 요청이 무기한 대기하고 detached driver socket과 확장 슬롯이 남는 RED를 확인했다. HTTP/CLI가 공통 owned connector를 사용하고 clock 조회를 별도5초로 제한한 뒤 DB_UNAVAILABLE·eventual socket EOF·슬롯 반환·shutdown 후 EOF를 확인했다. driver abort는 취소 예약이며 stopped 시점의 EOF 완료나 DB rollback 완료를 뜻하지 않는다.

clock DataRow와 ReadyForQuery까지 전달한 뒤 다음 SQL을 멈춘7개 요청에서도 RED가 재현됐다. read1개·표준 apply1개·WRITE4개·status1개가 계속 대기했다. read는 planner deadline, 표준 apply는 기존 V3의2초와 commit margin, WRITE는 선언 deadline으로 BEGIN/설정/key lock/replay/효과/결과 저장/commit stage를 제한한다. status 조회는5초다. commit 시작 전은 DEADLINE_EXCEEDED, 시작 뒤에는 COMMIT_UNKNOWN이며 연결/clock/파싱/응답의 별도 기한을 HTTP 전체 절대 기한으로 합쳐 주장하지 않는다.

실제 COMMIT 성공 CommandComplete만 중계에서 숨긴4개 HTTP case(표준 apply/WRITE × 두 wire)는 COMMIT_UNKNOWN 뒤에도 효과와 idem 결과1개가 실제 남았다. status와 같은 key/request 재생으로 복구했고 worker는 WRITE당1회였다. 생성 binding·실제 Node SDK의 추가 두 wire case는 첫 read cache hit → commit 응답 정지 → 재시도의 BEGIN 응답 정지 → pending 유지와 cache 저장 보류 → 정상 replay → cache 재사용을 확인했다. SDK가 재시도 DEADLINE_EXCEEDED를 확정 실패로 지우는 actual RED를 수정했으며 CONFLICT도 같은 이전 미확정 보존 규칙을 따른다. 첫 시도의 기한/충돌 거부는 pending을 남기지 않는다. COMMIT_UNKNOWN 자체를 성공이나 rollback으로 임의 확정하지 않는다.

유효 세션으로 시작한 WRITE가 실행 중 만료되면 실제 commit과 idem 결과1개가 남아도 TOKEN_EXPIRED로 뒤집히는 추가 RED를 재현했다. 생성 SDK는 첫 실패로 오인해 pending을 비웠다. 성공한 WRITE/status 결과는 그대로 보고하도록 후처리를 좁혔다. Node/Python × safe/decimal4개 실제 지연 worker case는 성공 결과·pending 해소·실제 commit을 확인하고, 다음 READ는 만료 토큰으로 거부됐다. 최초 인증과 성공 READ/extension의 남은 세션 수명 검사는 유지한다. Sol이 이 경계를 별도 문맥으로 재검토했으며 최신 전체 V6 회귀에는 기존 세션/읽기 수명 시험도 포함했다.

CLI init/serve도 인증 뒤 첫 SQL 응답이 없으면 무기한 대기하던 RED를 확인했다. 연결5초 뒤 SQL stage를 별도5초로 제한한다. init timeout은 커밋 여부를 단정할 수 없어 INIT_UNSETTLED, read-only serve 검사는 DB_PREFLIGHT다. init timeout 시험은 첫 SQL 정지만 검증했으며 CLI init의 실제 commit 응답 손실을 검증했다고 주장하지 않는다.

init은 marker까지 만든 같은 transaction에서 실제 catalog structure digest를 저장하고 serve는 read-only repeatable-read snapshot으로 대조한다. relation kind/inventory, live column/type/NULL/default/identity/collation, PK/FK/CHECK/EXCLUDE와 지연·검증 상태, standalone/partial/backing index 정의·valid/ready, sequence 정의·소유 관계, user trigger/rule/RLS policy, 관리 schema와 맞닿은 상속 edge를 정렬해 비교한다. 직접 OID·행·통계·sequence 현재값을 digest에 넣지 않는다. PostgreSQL [column catalog](https://www.postgresql.org/docs/17/catalog-pg-attribute.html), [index catalog](https://www.postgresql.org/docs/17/catalog-pg-index.html), [constraint catalog](https://www.postgresql.org/docs/17/catalog-pg-constraint.html), [sequence catalog](https://www.postgresql.org/docs/17/catalog-pg-sequence.html)의 의미를 확인하고 현재 PG17.11에서 실행했다.

실제13개 drift(타입/NULL/default/identity/CHECK/FK/PK/PK 재정의/partial index/sequence 정의/RLS/동명 table/view)는 bind 전에 SCHEMA_MISMATCH다. 외부 schema의 상속 child row가 managed SELECT에 들어오는 추가 반례도 검출한다. row INSERT·sequence position 변경·동일 PK 재생성은 거부하지 않았다. 13개 구조 변경과 구형 marker 거부 시험에서 기존 member42를 보존했다. 외부 상속 시험은 child row1개가 managed SELECT에 포함되는 반례를 별도로 확인했다. 구형 marker-only schema는 SCHEMA_NOT_READY이며 자동 소급 등록·ALTER·DROP·migration을 하지 않는다. 이는 private marker 형식 변경이며 기존 schema로 serve할 수 없다는 호환 한계를 포함한다. 다른 PG 버전의 지문 호환, owner/ACL·DB role 권한 검사, DB 관리자의 marker 동시 변조, listening 뒤 외부 DDL 감시는 범위 밖이다. 정상 init은 상속을 만들지 않아 빈 edge baseline과 비교한다. 일반 상속/partition 시스템을 구현한 것은 아니다.

```sh
# 저장소 루트에서
cargo test --offline --manifest-path spikes/spike-v6-transport/Cargo.toml --test db_query_lifetime --test db_commit_lifetime --test write_session_outcome
cargo test --offline --manifest-path prototype/Cargo.toml --test serve --test db_startup
# prototype/에서
node --experimental-strip-types --test tests/read-decoder.test.mjs tests/extension-sdk.test.mjs tests/http-recovery.test.mjs tests/sdk-package.test.mjs tests/write-extension-sdk.test.mjs
python3 tools/smoke.py
python3 tools/extension_smoke.py
python3 tools/write_extension_smoke.py
python3 tools/browser_smoke.py
python3 tools/bundle_smoke.py --bundle dist/local-macos-20261005-session-outcomes
```

DB/SQL 후속 당시 V2/V3/V4/V5/V6/CLI 회귀는20/5/32/22/37/32, 총148개 실패0이다. SDK/패키지41개·builder5개, 정의 변경4개·READ8개·WRITE8개·실제 Chrome8개·재배치 bundle8개도 실패0이다. Sol high는 DB 수명·transaction 취소·SDK pending 경계를 별도 문맥으로 검토하고 recovery12개를 독립 실행했다. Luna high는 구조 검사를 비평해 외부 상속 반례를 제시하고 serve9개를 독립 실행했다. 주장을 코드와 actual RED/GREEN으로 확인했으며 일반적인 구조 snapshot 완결성 의견을 현재 정상 init의 취약점으로 과장하지 않았다.

[당시 실행 bundle](dist/local-macos-20261005-session-outcomes/README.md)은 새 소유권·SQL 기한·catalog 비교·SDK 복구·커밋 이후 세션 만료 결과 처리를 포함한다. 그 검증 당시 source65개/output11개의 bytes와 manifest digest가 모두 일치했다. 최신 입력·캐시 결과는 마지막 절을 따른다. 내장 schema_catalog.sql을 source manifest에 추가했으며 이 입력 누락도 actual builder test RED로 확인했다. 이전 bundle은 당시 코드 snapshot으로 보존했다. owned driver abort·commit 불명 분류·catalog 비교를 끈3종 음성 대조에서 각각 행동 FAILED를 확인하고 original bytes를 복원했다. 운영 인증·격리·일반 migration·Python frontend SDK·제품 clean install·본 crates 통합과 최종 Id/API/ABI/WRITE 조합의 OPEN은 유지한다.


### 후속 관문: 커밋한 WRITE와 세션 만료 응답

1. 원칙 2·6·7에 따라 서버가 실행한 쓰기 결과를 정확히 전달한다. 인증 실패를 이미 커밋한 효과의 부재로 오인하지 않는다.
2. Intent 범위와 서버 권한은 유지한다. 유효 세션으로 시작한 호출의 commit 이후 성공 결과를 TOKEN_EXPIRED로 뒤집는 경계를 확인한다.
3. 앱별 인증·보상·pending 로직은 늘리지 않는다. 공통 transport의 결과 후처리만 보완한다.
4. 요청 시작의 서명·principal·정책 검증은 필수로 유지한다. 이미 커밋한 결과는 보고하며 새 요청의 만료 토큰은 계속 DB 전에 거부한다. 읽기 캐시에는 기존 남은 세션 수명을 적용한다.
5. Node/Python·두 Id 후보·생성 SDK에서 실제 지연 worker와 DB/idem 상태를 함께 확인한다.
6. 로그인·refresh·최종 세션 정책이나 새 API를 만들지 않는다. 결과 보고와 다음 요청의 인증을 구분한다.
7. postprocessing은 캐시 수명·결과 전달을 위한 수단이다. 미확정/커밋된 결과를 편의상 실패로 확정하지 않는다.
8. 운영 인증·최종 expiry/reauth 정책은 OPEN이다. 실제 커밋한 WRITE가 TOKEN_EXPIRED로 응답되는 RED를 먼저 재현하며 미인증 요청 우회는 추가하지 않는다.


### 후속 관문: 정의 숫자와 시간의 범위 진단

1. 원칙 2·4·6의 정형 정의 검증을 적용한다. 지원 범위를 넘는 숫자로 CLI가 panic하거나 시간 값이 wrap하지 않게 한다.
2. 기존 정수·duration 표현과 범위는 유지한다. 새 숫자 타입이나 작성 문법을 정하지 않는다.
3. 앱별 오류 처리 없이 기존 공통 lexer의 진단으로 연결한다. check/gen/init/serve의 load 경계를 재사용한다.
4. i64 정수 변환과 u64 millisecond 단위 곱셈을 검사한다. 실패는 위치를 포함한 정의 오류이며 기존 binding과 DB에는 반영하지 않는다.
5. A와 TS/Python 작성 형식의 공통 정책·duration lexer를 유지한다. H 숫자 리터럴의 기존 자체 검사는 바꾸지 않는다.
6. 대체 parser나 숫자 decoder를 만들지 않는다. 기존 Tok/Span/Diag를 재사용한다.
7. debug panic과 release wrap을 모두 검사한다. 경계값·원본 위치와 실제 CLI 실패 시 binding 보존을 행동 테스트로 고정한다.
8. 최종 numeric 범위·Id·API·작성 형식은 OPEN이다. 실제 정수 parse와 시간 곱셈 panic RED를 먼저 확인했다.


### 후속 관문: 읽기 캐시 저장 직전의 쓰기 완료

1. 원칙 2·4·6의 안전한 실행 결과를 SDK의 실제 읽기 최신성으로 연결한다. 완료된 쓰기 뒤 옛 읽기가 캐시에 다시 저장되는 반례를 닫는다.
2. 선택형 read/direct apply/공식 WRITE 표현은 유지한다. 호출자 표현이나 서버 계약을 늘리지 않는다.
3. 앱별 캐시 보정 없이 기존 SDK 캐시와 epoch/deps를 재사용한다.
4. 비동기 fetch 내부 판정 뒤 실제 반환·저장 직전에 actor/global/resource epoch를 재검사한다. 같은 응답이 여전히 최신일 때만 전달·저장한다.
5. 두 Id 후보와 표준/공식 WRITE facade에서 같은 마이크로태스크 순서로 확인한다. 서버·worker 언어별 정책은 바꾸지 않는다.
6. 새 캐시·저장소·요청 형식을 만들지 않는다. 기존 최대3회 읽기 재시도와 unknown 보존을 유지한다.
7. HTTP 응답 resolve→WRITE resolve 사이 스케줄링은 실제 JS Promise 순서로 재현한다. 임의 sleep이나 통과하는 스트레스 반복으로 대체하지 않는다.
8. 최종 캐시 정책/API는 OPEN이다. Sol의 구값 cached:true 반례를 메인이 actual RED로 재검증한다.

### 후속 관문: 정의 중첩의 실행 가능한 진단

1. 원칙 2·4·6에 따라 잘못되거나 과도하게 중첩된 정의를 panic/abort 대신 위치가 있는 진단으로 거부한다.
2. 현재 재귀 parser의 중첩을 private 실험 상한으로 제한한다. 기존 정상 정의와 작성 형식의 의미는 유지하며 최종 표현 한계로 확정하지 않는다.
3. check/gen/init/serve가 사용하는 공통 정의 parser를 보완한다. 앱별 pre-parser는 추가하지 않는다.
4. 괄호·not·호출 인자·exists 등의 공통 재귀 깊이를 검사한다. 지원 상한 이하의 정상식과 실패 뒤 parser 상태 복원을 함께 확인한다.
5. A/E의 정책식과 H의 정책 문자열은 동일 parser를 사용한다. H 리터럴의 재귀 경계도 실제 반례가 확인되면 같은 단계에서 고정한다.
6. 새 DSL/IR 또는 호스트 eval을 만들지 않는다. 현재 Tok/Expr/Diag와 literal reader를 유지한다.
7. 실제 CLI abort RED를 먼저 재현하고 diagnostics JSON/기존 binding 보존으로 검증한다. 큰 thread stack으로 문제를 숨기지 않는다.
8. 작성 형식·최종 비용/중첩 정책·제품 API는 OPEN이다. private 상한과 검증 범위를 결과에 명시한다.


### 후속 관문: 출력 소비자 종료와 CLI 결과

1. 원칙 4·6의 검증 가능한 실행 결과를 CLI 출력에도 적용한다. 성공한 gen 뒤 출력 pipe 종료가 panic으로 결과를 오인시키는 실제 반례를 닫는다.
2. check/gen/init/serve 명령과 호출자 표현은 유지한다. 새 실행·설치 API를 만들지 않는다.
3. 공통 stdout 보고 함수를 사용하고 stderr 출력 오류도 panic으로 올리지 않는다.
4. 유한 명령의 성공 뒤 BrokenPipe는 소비자 취소로 종료0이며 수행한 효과를 되돌리지 않는다. 다른 보고 오류는 실패로 알린다. 정의 실행 실패의 stderr가 닫혀도 원래 실패 상태는 유지한다.
5. macOS 실제 닫힌 pipe/socket writer로 CLI 종료와 생성 파일 상태를 확인한다. SDK/worker 프로토콜은 변경하지 않는다.
6. stdout에 다른 형식이나 로그 channel을 추가하지 않는다. 기존 JSON 줄과 flush를 유지한다.
7. serve는 listening 결과를 전달하지 못하면 새 서버를 종료하고 기동 출력 실패를 반환한다. 유한 명령의 stdout 취소를 실행 중 서버의 지속 정책과 혼동하지 않는다.
8. 최종 CLI/운영 logging 정책은 OPEN이다. check/gen의 panic과 생성 완료 파일 반례를 actual RED로 확인하고 정상 출력/종료 회귀를 유지한다.


## 정의 입력·캐시 완료·CLI 출력 후속 결과

정본 app.aip의 rows에 i64 범위 밖 정수를 넣거나 duration을 millisecond로 변환할 때 u64가 넘으면 CLI가 panic하던 실제 RED를 확인했다. 공통 lexer가 각각 LEX_NUMBER_RANGE/LEX_DURATION_RANGE를 위치가 있는 진단으로 돌려준다. debug와 release의 경계값·정상 단위·후속 token 위치를 확인했으며 check/gen은 INVALID_DEFINITION과 종료1이다. 실패한 생성은 기존 binding과 임시 출력 상태를 보존한다. H 숫자 리터럴의 기존 자체 진단은 바꾸지 않았다.

과도한 괄호·not·호출 인자 재귀와 H의 중첩 배열/객체는 기존 CLI의 stack overflow/abort 반례였다. 공통 expression parser와 H literal reader에 각각128의 private 활성 재귀 상한을 추가했다. expression 상한은 root expr와 중첩 expr/not 호출을 함께 세며, H root define object는 counter 밖이다. 정확히 괄호128개를 허용한다는 최종 문법 계약이 아니다. A/E/TS/Python H8개 fixture의 check/gen16개 동작은 정상 JSON 진단과 기존 binding 보존을 확인한다. 63단 괄호/not/호출 인자 및 H 리터럴도 확인했으며 기존5개 작성 형식의 동일 facts/digest 회귀를 유지했다. 이 시험은 정의의 전체 메모리·크기·시간 비용이나 모든 의미 그래프 재귀를 제한했다고 주장하지 않는다.

SDK의 READ 응답 뒤 WRITE가 확정되는 Promise 순서에서, fetch 내부의 최신성 판정과 cache.read 저장 사이에 옛 행이 다시 저장되는 실제 RED를 확인했다. fetch 시작 actor/global/deps snapshot을 반환·저장 직전에 검사하며 그 뒤에는 await가 없다. 두 wire × 표준/공식 WRITE4개와 actor/resource epoch 경계2개가 GREEN이다. 같은6개를 최신 tarball의 오프라인 설치 소비자에서도 실행했다. 기존 TTL 시작 시각·최대3회 재조회·unknown 중 저장 보류·쓰기 tag 무효화를 유지한다. 모의 HTTP가 Promise 순서를 제어하는 시험이며 새 race를 실제 DB 스케줄링에서 재현했다고 하지 않는다. 별도로 실제 생성 SDK·DB·Chrome·bundle 흐름을 다시 실행했다.

성공한 gen은 binding을 이미 교체했는데 stdout 소비자가 연결을 닫으면 panic/종료101이던 반례도 수정했다. check/gen/init의 성공 뒤 BrokenPipe는 소비자 취소로 종료0이며 수행한 효과를 되돌리지 않는다. 다른 출력 오류는 OUTPUT_IO이고 stderr가 닫힌 기존 실행 오류는 원래 종료1을 유지한다. serve는 listening 출력 실패 시 server shutdown을 기다린 뒤 OUTPUT_IO를 반환하며 schema는 보존한다. 닫힌 실제 Unix socket writer의3개 시험과 정상 기동/종료 회귀를 확인했다.

V1 parser 검증에서 기존 fmt drift와5개 clippy 오류가 드러났다. 동일 타입 tuple의 alias·불필요한 lifetime·실제로 반복하지 않던 assignments loop·동일 의미의 Option 검사를 정리하고 V1 formatter를 적용했다. 기존 H 단일 대입 의미와5개 작성 형식의 facts 동등성은 유지했다. 새 테스트 준비 단계의 Debug bound·누락 import 컴파일 오류는 수정했으며 행동 RED로 세지 않았다.

```sh
# 저장소 루트에서
cargo test --offline --manifest-path spikes/spike-v1-fixture/Cargo.toml
cargo test --offline --release --manifest-path spikes/spike-v1-fixture/Cargo.toml --test number_boundaries --test nesting
cargo test --offline --manifest-path prototype/Cargo.toml
node --experimental-strip-types --test prototype/tests/*.test.mjs
node --experimental-strip-types --test spikes/spike-v5-sdk/sdk/cache.test.ts
# prototype/에서. Chrome 시험은 앞 절의 Playwright 환경 Python을 사용한다.
python3 tools/smoke.py
python3 tools/extension_smoke.py
python3 tools/write_extension_smoke.py
python3 tools/bundle_smoke.py --bundle dist/local-macos-20261005-input-cache
```

최신 V1/V2/V3/V4/V5/V6/CLI 전체 회귀는22/20/5/32/22/37/37, 총175개 실패0이다. V1 release 숫자/중첩5개는 별도이며175에 중복 합산하지 않는다. SDK/패키지47개·공통 cache6개·설치형 cache6개·builder5개도 실패0이다. 정의 변경4개·READ8개·WRITE8개·Chrome8개·재배치 bundle8개는 모두 ok:true다. 변경 V1/V5/V6/CLI의 fmt --check와 clippy --all-targets -D warnings는 exit0이다. V6 전체 실행은 exit0이며 보존 로그 일부가 출력 상한에 걸려 전체 test inventory37개도 별도로 남겼다. Sol은 새 cache6개와 기존 cache6개, 출력 CLI2개/serve1개를 독립 실행했고 Luna는 숫자/중첩 CLI 각1개를 독립 실행했다. Luna는 그 실행 결과를 전달한 뒤 quota 오류가 나 추가 호출을 진행하지 않았다. 메인이63단 양성 대조와 전체 최종 회귀를 직접 실행했다.

[최신 실행 bundle](dist/local-macos-20261005-input-cache/README.md)은 기존 DB/쓰기 복구와 이번 입력·cache·CLI 출력 처리를 포함한다. source65개/output11개의 현재 digest mismatch0이다. 이전 bundle/148개 기록은 당시 snapshot으로 보존한다. 최종 숫자/중첩 정책·Id/API/ABI·운영 인증/격리·일반 migration·Python frontend SDK·제품 clean install·본 crates 통합의 OPEN은 유지한다.
