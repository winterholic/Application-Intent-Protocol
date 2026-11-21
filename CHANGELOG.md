# Changelog

형식은 [Keep a Changelog](https://keepachangelog.com/ko/1.1.0/)를 따르고 버전은 [Semantic Versioning](https://semver.org/lang/ko/)을 따른다.

## [Unreleased]

### Added
- `release-candidate` workflow: 필수 workflow 전부를 경로 필터 없이 한 SHA에서 실행하고 모두 `success`일 때만 통과하는 집계 job (`.github/workflows/release-candidate.yml`).
- `SECURITY.md`, `CONTRIBUTING.md`, 이 파일.
- 문서: [1.0 지원 범위](product/SUPPORT.md)(지원 11, 조건부 7, 제외 1, PostgreSQL 17 한정, 공개 count, 계약 헤더 오류 코드), [표현력 한계와 확장 우회](product/LIMITS.md)(E1~E9), COMPATIBILITY의 1.0 호환 약속과 정의 검사 변경.

### Changed
- `tools/verify.sh`의 엔진 clippy·test 목록에 `spike-v11-dev-checks`를 추가했다(제품 `load` 경로가 호출하는 엔진).
- README 맨 위에 1.0 상태를 명시했다: 정식 경로는 `aip service`, `.aipoc`는 PoC, tenant 자동 격리는 1.0 제외. 시작 가이드는 저장소 checkout 없이 릴리스 묶음(`AIP_HOME`)만으로 따라 하도록 고치고, 참조 열 이름 규칙(`<필드>_id`), `Time` 출력 표기, 소스 빌드의 SDK 도구 준비, `PG*` 환경 변수와 release 빌드의 관계를 적었다.

## [1.0.0] - 미정 (초안)

> **초안**: 1.0.0은 아직 출시되지 않았다. 아래 항목은 현재 `main`의 내용이며 태그 전에 다시 확인한다.
> 패키징 결과와 출시 게이트가 정해지면 갱신한다. 출시 전 차단 목록은
> 1.0 감사(브랜치 `release/1.0-audit-20261121`의 `plan-docs/reviews/2026-11-21-1.0-release-audit.md`)를 따른다.

### Added
- 제품 서비스(`aip service`): check, gen, init, adopt, migrate, serve, principal. RS256 JWT 인증과 공개 JWKS, PostgreSQL 17의 읽기·쓰기 엔진, Node/Python 확장 worker, 설치형 SDK(`@aip/sdk`, Python).
- 일반 마이그레이션(변경 계획, 정형 backfill/convert, DDL 트랜잭션과 배포 저널)과 기존 데이터 이관.
- **표현력 문법 4종**(PostgreSQL 제품 경로, 기준 `4e132564`..`be94d064`, 설계는 [결정표](plan-docs/expressiveness-1.0/decisions.md)). 사용법은 [일반·부분 수정](product/GENERAL-UPDATE.md), [SDK 묶음과 조합](product/SDK.md#묶음bundle과-조합compose), [표현력 한계](product/LIMITS.md)에 있다.
  - **결과 참조**(`refs`, `target.created`): 한 `bundle`에서 앞 create가 만든 행의 ID를 뒤 create의 참조 필드(`"refs":{"order":{"created":0}}`)나 뒤 전이의 대상(`"target":{"created":0}`)으로 넘긴다. 번호는 요청의 연산 위치다. TypeScript `client.bundle(b => ...)` 빌더와 `Created<R>` 핸들, Python `aip_sdk.created(i)`. 서버가 트랜잭션 전에 연산 순서, 타입, 가시성, create `allow`를 검사한다(`600b86ad`, `12421f9d`, `a825cadf`, `1a25bd32`).
  - **제한 산술**: 전이의 `input { qty: Int(1..100); amount: Decimal(12,2) }` 블록과 `to f = f ± 항`, 효과 `update` 값 `f = f ± 항`. 항은 하나이고 양수 리터럴, `input.p`, `this.g` 중 하나다. 서버가 잠근 행에서 같은 `UPDATE`로 계산하며 결과가 필드 범위·자릿수를 벗어나거나 NULL이면 `WIRE.BAD_VALUE`로 전체를 롤백한다. 곱셈·나눗셈·두 항 이상·`repeat unchanged`와의 조합은 없다(`acaed2ab`, `b1bda3b2`, `e76efbab`, `16819e3c`).
  - **bundle 안 `compose`**: `atomic[]` 원소로 `{"compose","targets","steps"}`를 넣어 compose와 apply(patch·expect·input)·create를 한 트랜잭션에 묶는다. 중첩(compose 안 compose)은 없고, `refs`·`target.created`는 compose 연산을 가리킬 수 없다(`WIRE.VALUE_SOURCE_NOT_ALLOWED`). compose가 든 묶음의 응답에만 `ops: [{kind, applied:[시작,끝), created:[시작,끝)}]`가 붙는다(`a947289e`, `952c4331`, `583c4e07`).
  - **쓰기 확장의 `ctx.data.create`와 `ctx.data.apply`의 `input`**: `access create.<Resource>`를 선언하면 호출자의 공개 create 경로(필드 허용 목록, `allow`, 참조 가시성)로 한 행을 만든다. `ctx.data.apply`는 마지막 선택 인자로 전이 `input`을 받는다. 확장이 `WIRE.CONFLICT`·`WIRE.ALREADY_EXISTS`를 그대로 내보내면 호출자가 그 코드를 받는다. Node `ctx.d.ts`와 Python `aip_ctx.py` 타입 파일을 더했다(`27f87d10`, `9907d1c9`, `c24bede4`, `3a6edf5f`). 타입 파일은 저장소 소스에만 있고 SDK 패키지에는 들어 있지 않다.
- **Decimal `sum`**: `release sum(f)`가 `Decimal(p,s)` 필드를 받는다. 결과 타입은 `Decimal(p',s)`(s 같음, p' >= p)이고 JSON 문자열로 나온다(`"50.70"`, 빈 집합은 `"0.00"`). 합이 p'를 넘으면 `WIRE.OUTPUT_INVALID`다. 반올림은 없다(`79f2d3e8`, `7e367ead`).
- **정책 작성 경고 3종**(조언만, 정의는 거부하지 않는다): `DEF.ADVISORY_TENANT_UNSCOPED_READ`, `DEF.ADVISORY_PUBLIC_READ_TENANT_FIELD`, `DEF.ADVISORY_WRITE_WIDER_THAN_READ`. `--dev-checks` 없이도 `aip service check`의 `advisories`에 나온다(`79f2d3e8`).
- **진단 JSON 고정**: `aip service check`의 성공(종료 0, stdout)과 실패(종료 1, stderr) JSON의 키 집합과 타입을 시험으로 고정했다. 경고 항목에 `severity: "warning"`, 정의 오류 항목에 `severity: "error"`가 더해졌다(`7e367ead`).
- 소비자 앱 3유형(협업형 SaaS, 콘텐츠·커뮤니티, 주문·재고)을 실제 HTTP·PostgreSQL 17·TypeScript·Python SDK로 실행하는 시험과 CI 단계, 실행 기록([consumer-apps](plan-docs/expressiveness-1.0/consumer-apps.md))을 더했다(`c59b8588`, `fc56293e`, `dc68e4e6`).
- 앱 패키지 설치·변경 배포, Docker·네이티브 서비스 실행 경로.
- Linux worker 격리(Bubblewrap, 선택적 cgroup v2 한도)와 macOS Seatbelt 격리(수동 검증).
- FlaskBB 포팅: Topic.move, Post.unhide, PostOrder 계약.
- 1.0 릴리스 게이트 스크립트(`scripts/release-gate.sh`): 새 clone 전체 회귀, 묶음 smoke, 업그레이드·롤백 드릴.

### Changed
- **호환 변경: `actor`를 둘 이상 선언하면 `DEF.MULTIPLE_ACTORS`로 거부한다.** 이전에는 마지막 선언이 조용히 이겼다. 정의에 `actor` 한 줄만 남긴다.
- **호환 변경: 원본에 `rows read`가 있는 `callerFilter read` 집계는 `where`가 그 정책을 함의해야 한다.** 아니면 `DEF.CALLER_FILTER_POLICY_REQUIRED` 오류다(호출자의 필터로 숨은 행의 값을 셀 수 있었다). 이전에는 `where`가 없을 때만 `--dev-checks`에서 조언이 나왔다. 저장소의 `.aip` 36개는 영향이 없었다. 저장소 밖 정의는 배포 전에 `aip service check`로 확인한다.
- **`where` 없는 `count`는 `--dev-checks` 없이도 `DEF.ADVISORY_PUBLIC_COUNT` 경고를 낸다**(정의는 통과한다). 공개 count의 응답 값과 계약 지문은 바뀌지 않았다.
- 정의 검사 진단 개선(모두 이전에도 거부되던 입력): `DEF.TRAVERSE_VIA_FORM`, Decimal 리터럴의 `DEF.BAD_VALUE`, `transition`의 `from`·`allow` 누락 안내. `to` 절의 구분자는 쉼표라는 안내로 문서와 진단을 맞췄다.
- 호환 약속(같은 protocol major의 허용 변경, 계약 지문, 구버전 동시 지원 없음, 롤백은 백업 복원)과 `aip-protocol/1.0`·`aip-core/0.11`을 함께 두는 이유를 COMPATIBILITY.md에 정리했다.
- worker runtime probe가 5초 단일 예산을 쓰고, 시간 초과 시 probe 프로세스 트리를 정리한다.
- Post.unhide가 upstream의 restore 의미와 일치한다.
- **같은 key 재전송은 이미 커밋된 응답을 worker 슬롯 없이 재생한다**(PostgreSQL 쓰기 확장 `/apply`). 이전에는 같은 주체의 worker 슬롯(기본 2)이 차 있으면 이미 저장된 key의 재전송도 `WIRE.WORKER_BUSY`였다. 지금은 배포 fence를 확인한 뒤 저장된 성공 결과를 `replayed: true`로 돌려주고 확장을 다시 실행하지 않는다. 같은 key에 다른 본문은 `WIRE.IDEMPOTENCY_MISMATCH`이고, 아직 진행 중인 key는 기존 경로(슬롯, `WORKER_BUSY`)를 그대로 지난다(`241ae37f`, `7cdfed93`).
- **행 잠금 모드**: 키 열을 바꾸지 않는 전이는 `FOR NO KEY UPDATE`, 키 열 변경·삭제·기존 자식 행을 갱신하거나 삭제하는 효과는 `FOR UPDATE`다. 같은 묶음에서 앞 create가 FK로 잡은 공유 잠금과 뒤 전이의 갱신 잠금이 서로 기다리는 교착을 없앤다. 응답 값은 바뀌지 않는다(`824cc0e1`, `dc68e4e6`).
- **암호화 리소스 거절**: `ctx.data.create`는 암호화 필드가 있는 리소스를 `WIRE.UNSUPPORTED`로 거절한다(없으면 평문이 저장됐다). 암호화가 켜진 서버는 묶음 안의 `compose`와 `refs`를 `WIRE.UNSUPPORTED`로 거절하고 아무것도 쓰거나 멱등 기록을 남기지 않는다. 암호화는 1.0 지원 범위 밖이므로 이 거절은 라이브러리 층 방어다(`3a6edf5f`, `952c4331`, `dc68e4e6`).
- `DEF.ACCESS_MISMATCH` 문구가 실제 판정과 맞게 고쳐졌다(`totalOfVisible(P)`의 `P`는 집계를 선언한 resource여야 한다). 이전에도 같은 정의를 거부했고 오류 코드는 같다(`79f2d3e8`).
- TypeScript SDK 빌드 실패가 컴파일러 출력 요약을 함께 알려 준다(`18bd6b32`).

### Fixed
- G6 effect 보존과 source-order import 검증.
- rustix 보안 권고에 따른 lockfile 갱신.

### Security
- 의존성 권고 점검을 CI에서 주간 실행한다. 허용된 예외는 `RUSTSEC-2023-0071`(RSA 서명 검증만 사용, 사유는 `deny.toml`).

### Known limitations
- PostgreSQL 17만 1.0 지원 범위다. MySQL은 출시 묶음에 포함되지 않는다.
- 공개 count는 `rows read`를 적용하지 않는 계약을 유지한다. 호출자별로 값을 줄이는 변경(명시 공개 표기, 모든 count에 `rows read` 적용)은 소유자 결정으로 1.0에 넣지 않았다.
- tenant 자동 격리는 1.0 지원 범위 밖이다(Core PoC에만 있다). 조직 경계는 정의의 조건으로 직접 쓴다.
- 선언만으로 표현하지 못하는 요구(계산 집계 `Σ 수량×단가`, `avg`, nullable 정렬과 cursor, 관련 필드의 필터·정렬, 확장 입력의 배열 등)는 [표현력 한계와 확장 우회](product/LIMITS.md)를 따른다. 부모·자식 동시 생성과 가변량 증감은 위 표현력 문법으로 선언만으로 된다.
- 새 표현력 문법은 PostgreSQL 전용이다. MySQL 쓰기는 `input`·제한 산술을 `WIRE.UNSUPPORTED`로, 묶음의 `refs`·`target.created`·`compose`를 `WIRE.UNKNOWN_KEY`로 거절하고, MySQL 읽기는 Decimal `sum`을 `WIRE.UNSUPPORTED`로 거절한다. MySQL은 1.0 지원 범위 밖이다.
- 구서버는 새 요청 키(`refs`, `target.created`, `input`, 묶음의 `compose`)를 `WIRE.UNKNOWN_KEY`로 거절한다(fail-closed). 새 SDK로 새 문법을 쓰려면 서버도 같은 릴리스여야 한다.
- 정책 작성 경고는 구조 비교라 놓치거나(`exists` 정책, 이름이 다른 테넌트 필드, 모든 정책에서 테넌트 조건을 한꺼번에 뺀 정의) 오탐할 수 있다(allow와 `rows read`가 같은 뜻을 다르게 쓴 경우). 상세는 [P1 sema 노트](plan-docs/expressiveness-1.0/p1-sema-notes.md)다.
- 쓰기 확장 안의 `ctx.data.create` 이후 PostgreSQL 오류(유일 제약 등)가 나면 트랜잭션이 중단되어 이어지는 ctx 호출은 계속할 수 없다. 확장 입력에 배열 타입이 없어 한 호출에서 여러 줄을 받으려면 `Json`으로 받고 확장이 검증한다.
- 공개 `count`의 `rows read` 비상속은 이번 변경에서도 바꾸지 않았다(소유자 결정 대기). Decimal `sum`은 `rows read`를 적용하므로 같은 정의에서 `count`와 `sum`이 호출자에 따라 어긋날 수 있다.
- 지원 범위(지원 11, 조건부 7, 제외 1)는 [1.0 지원 범위](product/SUPPORT.md)가 기준이다.

[Unreleased]: https://github.com/winterholic/Application-Intent-Protocol/commits/main
[1.0.0]: https://github.com/winterholic/Application-Intent-Protocol/releases/tag/v1.0.0
