# 1.0 지원 범위

AIP 1.0이 무엇을 지원하고 무엇을 조건부로, 무엇을 지원하지 않는지 한 곳에 적은 문서다. README와 [시작 가이드](GETTING-STARTED.md)는 이 표를 가리킨다. 표의 근거는 2026-11-21 출시 감사(기준 커밋 `723fb474`)이고, 이후 반영된 변경(공개 count 오류와 경고, `actor` 중복 거부)은 아래에 맞춰 고쳤다. 감사는 제품 경로의 테스트와 실제 PostgreSQL 실행으로 판정했다. PoC 경로의 결과는 제품 지원 근거로 쓰지 않았다.

## 1.0의 정식 경로

| | 제품 `aip service` (**1.0 정식 경로**) | Core `.aipoc` (PoC) |
| --- | --- | --- |
| 정의 파일 | `.aip`(`resource`·`expose`·`transition`) | `.aipoc`(`entity`·`command`·`query`) |
| 문법 | [`spec/definition-syntax.md`](../spec/definition-syntax.md) | [`spec/grammar.md`](../spec/grammar.md) |
| 명령 | `aip service check / gen / init / migrate / serve / principal …` | `aip check / core / ddl / ir / explain / contract / gen-ts` |
| 1.0 출시 묶음 | 포함. `bin/aip`가 이 경로다 | **포함되지 않는다.** `run`, `migrate`, `rekey`, `token`은 `--features poc-runtime` 빌드에서만 동작하고, 출시 묶음은 이 feature 없이 빌드된다 |
| 지원 | 아래 표 | 지원하지 않는다. `aip --help` 루트에는 PoC 명령(`parse`, `check`, `ddl`, `run`, `migrate`, `token`, `gen-ts` 등)도 나열되지만 1.0 명령은 `aip service`, `explain-code`, `diagnostics`다 |

Core의 자동 tenant 격리, search, publishable, consent, subscribe 같은 기능은 PoC에서만 동작하며 제품 `.aip`에는 없다. 1.0 사용자 문서의 기능은 모두 제품 경로 기준이다.

## 지원 범위 표

19개 후보 기능을 **지원 11, 조건부 7, 제외 1**로 판정했다. 모든 데이터베이스 근거는 PostgreSQL 17이다.

- **지원**: 제품 경로(`aip service`와 실제 PostgreSQL, HTTP 또는 설치 SDK)에서 확인했고, 알려진 한계가 [표현력 한계](LIMITS.md)처럼 문서화돼 있다.
- **조건부**: 지원하되 표의 조건 안에서만 그렇다. 조건 밖의 사용은 1.0이 보장하지 않는다.
- **제외**: 1.0 지원 범위 밖이다. 동작하더라도 보장하지 않고 광고하지 않는다.

| # | 기능 | 판정 | 조건과 한계 |
| --- | --- | --- | --- |
| 1 | 정의 검사(`aip service check`) | 지원 | 다섯 작성 형식(`.aip`, `.e.ts`, `.e.py`, `.h.ts`, `.h.py`). 선택적 개발 조언 `--dev-checks`는 조건부 |
| 2 | 스키마·타입 | 지원 | Int, Text, Bool, Id/Uuid, Decimal(p,s), Time, Date, Email, Url, enum, Json. **Float 없음.** |
| 3 | resource·relation·constraint | 조건부 | 관계 읽기만 제품 HTTP 시험이 있다. FK 거부, unique, limit, invariant의 실DB 근거는 엔진 단위 시험이다 |
| 4 | 선언형 읽기·표준 쓰기 | 지원 | 필터 허용 목록, 정렬, cursor·offset 페이지, to-one·to-many 관계, `count`/`sum`/`min`/`max`. PostgreSQL의 `sum`은 단일 Decimal 필드를 지원한다. 결과의 scale은 원본과 같고 precision은 원본 이상이어야 한다. `avg`, 관련 필드 정렬·필터, 다중 `groupKey` 같은 표현은 선언만으로 안 된다([표현력 한계](LIMITS.md)) |
| 5 | partial update(null과 누락 구분) | 지원 | `patch`와 `expect`. 선언한 필드만 고칠 수 있다 |
| 6 | 계약 HTTP | 지원 | `/read`, `/apply`, `/status`, `/capabilities`, 오류 코드 [`spec/wire-errors.md`](../spec/wire-errors.md). 헤더 오류 코드는 아래 "계약 헤더 오류 코드" |
| 7 | 생성 binding(TypeScript, Python) | 지원 | `aip service gen`. Python 생성 타입은 공개 apply 입력과 helper가 중심이다 |
| 8 | JWT·principal | 지원 | **RS256 `at+jwt` 프로필 한정.** 실제 외부 IdP 계정과의 연결은 조건부(운영자가 확인한다) |
| 9 | row/field policy | 조건부 | `rows read`, `field … read`, 전이 `allow`. **`count`는 `rows read`를 상속하지 않는다**(`sum`·`min`·`max`는 상속). 아래 "공개 count" |
| 10 | server validation | 지원 | 입력 타입·범위·필터 허용 목록을 서버가 매 요청 검사한다 |
| 11 | 트랜잭션 | 지원 | PostgreSQL. 부재(absence)에 의존하는 쓰기는 SERIALIZABLE. 서버가 소유하는 선언형 SQL 트랜잭션은 안전하게 다시 실행할 수 있는 직렬화 실패·교착을 최대 3회 재시도한다. 동기 쓰기 확장은 호출자가 같은 key로 다시 보낸다 |
| 12 | idempotency·`COMMIT_UNKNOWN` | 지원 | **단일 DB 트랜잭션 한정.** 외부 효과(메일, 결제 호출)는 멱등이 보장되지 않는다 |
| 13 | tenant 자동 격리 | **제외** | 제품 경로에 구현이 없다(Core PoC에만 있다). 조직 경계는 `rows read when org = actor.org`처럼 정의에 직접 쓰고 모든 정책에 반복해야 한다. 이 명시 패턴 자체는 조건부로 동작한다 |
| 14 | fingerprint(계약 지문) | 지원 | 지문이 다르면 `WIRE.CONTRACT_MISMATCH`. 구버전 클라이언트 동시 지원은 없다 |
| 15 | 초기화·migration | 조건부 | **PostgreSQL 한정.** 롤백은 백업 복원뿐이며 역방향 migration은 없다. 대형 테이블 무중단 변경은 범위 밖 |
| 16 | Node/Python 확장 | 조건부 | 확장 ABI는 확정 전이다(결정 OI-X2). Linux Bubblewrap 격리는 CI가 있다. **macOS Seatbelt 격리는 CI 없이 수동 검증이다.** PostgreSQL 쓰기 확장은 허용 목록 안에서 `ctx.data.read`, `ctx.data.apply`, `ctx.data.create`를 지원한다. 암호화 리소스의 확장 create는 제외다 |
| 17 | durable job | 조건부 | **PostgreSQL job store만.** MySQL job store는 출시 묶음에 없다 |
| 18 | 진단·관측 | 조건부 | OTLP, 감사 기록, trace는 엔진 시험까지 확인했다. 대시보드와 SLO는 제공하지 않는다 |
| 19 | 설치 SDK·패키지 | 조건부 | 버전은 Cargo workspace 한 곳에서 파생해 CLI·TypeScript SDK·Python SDK가 같다(`1.0.0`). 이름은 `product/release.config.json`을 따른다. Windows는 지원하지 않는다([플랫폼](INSTALL.md#지원-플랫폼과-최소-요구)) |

요약: 지원은 1, 2, 4, 5, 6, 7, 8, 10, 11, 12, 14. 조건부는 3, 9, 15, 16, 17, 18, 19. 제외는 13이다.

## PostgreSQL 17 한정

1.0의 모든 지원 판정은 **PostgreSQL 17**에서 확인했다. 다른 버전은 확인하지 않았다.

- MySQL과 MariaDB는 1.0 지원 범위 밖이다. 출시 묶음(`bin/aip`)은 `--features mysql` 없이 빌드된다. MySQL 연결 문자열이나 어댑터 설정으로 기동하면 `aip service init`과 `serve`가 `DB_ADAPTER_FEATURE_DISABLED`로 거부한다(`MySQL/MariaDB adapter를 사용하려면 mysql feature로 빌드해야 함`). 이 문구는 소스 빌드 안내이며 출시 묶음에서 해결할 수 없다. MariaDB는 job store만 별도 시험이 있고 읽기·쓰기는 제품 경로에서 거부된다.
- 외부 broker(Kafka, RabbitMQ)는 `external-brokers` feature가 필요해 출시 묶음에 없다.
- 객체 저장소(S3·GCS)와 quota는 MinIO에서만 확인했다. 1.0이 보장하는 기능으로 쓰지 않는다.
- 암호화 필드는 활성 정의가 runtime 검사에서 거부된다는 기록이 있을 뿐 1.0이 지원하지 않는다.
- FaceReview, FlaskBB, Infisical 전체 포팅의 동등성은 1.0 조건이 아니다. 일부 시나리오만 CI 근거가 있다.

## 공개 count

`release count`는 `sourceAccess`와 `where`가 고른 모집단을 센다. 원본의 `rows read`를 적용하지 않는다. 1.0은 이 응답 값을 바꾸지 않았고, 정의 검사가 다음 두 가지를 알려 준다.

| 상황 | 결과 |
| --- | --- |
| `where` 없는 `count` | `aip service check`가 `DEF.ADVISORY_PUBLIC_COUNT` **경고**를 낸다(`--dev-checks` 없이도 나온다). 정의는 통과하고 값은 호출자가 읽을 수 없는 행도 센다 |
| 원본에 `rows read`가 있고 `callerFilter read`인데 `where`가 그 정책을 함의하지 않는 집계 | `DEF.CALLER_FILTER_POLICY_REQUIRED` **오류**. 호출자의 필터로 숨은 행의 값을 한 값씩 셀 수 있기 때문이다 |

계약 전문과 대안은 [SDK의 공개 count 계약](SDK.md#public-count-contract-공개-count-계약)에 있다. 응답 값을 호출자별로 줄이는 변경(명시 공개 표기, 모든 count에 `rows read` 적용)은 1.0에 넣지 않았다.

## 계약 헤더 오류 코드

`x-aip-contract` 헤더가 없거나 맞지 않을 때의 코드는 경로마다 다르다.

| 요청 | 코드 | 확인 |
| --- | --- | --- |
| `/read`, `/apply`(전이·묶음·조합), `/status`에서 헤더 없음 | `WIRE.CONTRACT_REQUIRED` | 실행 |
| 같은 경로에서 64자리 16진수가 아닌 값 | `WIRE.BAD_REQUEST` | 실행 |
| 같은 경로에서 형식은 맞고 서버와 다른 지문 | `WIRE.CONTRACT_MISMATCH` | 실행 |
| object API(`/objects/*`), RAG API(`/rag/*`), 쓰기 확장 `/apply`에서 헤더 없음 | `WIRE.CONTRACT_MISMATCH`(`CONTRACT_REQUIRED`가 아니다) | 코드 확인만, 실행하지 않음 |

직접 HTTP를 호출하는 클라이언트는 두 코드를 같은 방식으로 처리하면 된다. 서버의 지문을 얻어 헤더에 넣거나 binding을 다시 생성한다. 생성 binding과 SDK는 헤더를 자동으로 보낸다. 이 문서는 1.0의 현재 동작을 기록하며 코드는 바꾸지 않았다.

## 지원하지 않는 것

| 항목 | 상태 |
| --- | --- |
| Windows | 지원 target은 Linux x86_64·arm64(glibc 2.34 이상)와 macOS arm64·x86_64다 |
| 구버전 클라이언트 동시 지원 | 없다. 정의를 바꿔 배포하면 이전 binding의 호출은 `WIRE.CONTRACT_MISMATCH`로 거부된다([OPERATIONS](OPERATIONS.md#변경-배포와-기존-데이터-이관)) |
| 자동 역방향 migration, 자동 롤백 | 없다. 롤백은 백업 복원이다([릴리스 업그레이드와 롤백](OPERATIONS.md#릴리스-업그레이드와-롤백)) |
| 로그인, 가입, refresh | 외부 IdP가 한다. `aip service`는 토큰을 발급하지 않는다 |
| 공개 Docker 이미지 | 없다. 앱 단위로 직접 빌드한다([DOCKER](DOCKER.md)) |

## 이 표의 한계

- 표는 2026-11-21 감사 시점의 근거를 따른다. 이후 코드가 바뀌면 [COMPATIBILITY](../COMPATIBILITY.md)와 [CHANGELOG](../CHANGELOG.md)로 변경을 확인한다.
- 외부 IdP 실계정, 실제 AWS·GCS, macOS Seatbelt 격리는 감사에서 실행하지 않았다.
- 1.0 이후의 문법 추가가 기존 `.aip`를 깨지 않는다는 자동 보장 장치(문법 버전 필드, 호환 골든)는 감사에서 확인하지 못했다. 업그레이드 전에는 새 binary로 `aip service check`를 먼저 실행한다.
