# Track C 결과: 최신 Core 기준 독립 소비자 E2E (2026-11-22)

범위: baseline.md 3절 C-01~C-04, F-01의 독립 소비자 연결 부분. 기준선은 `origin/main` `4f44ca87`이고, 이 문서의 실행은 모두 아래 SHA의 작업 트리에서 했다.

- 최종 실행 SHA: `659e58440890b2357160dc0159c479c8b3b37f94` (main `cf429b31`까지의 A·B·E·F1 변경 위에 C 커밋 6개를 얹은 상태, 이 문서의 내용 커밋 포함. 이 줄의 SHA 갱신 커밋만 그 뒤에 온다)
- 환경: macOS arm64, Node v26.9.0, Python 3.14.7, `aip 1.0.0`(release 빌드), PostgreSQL 17.11(`run.py`는 docker `postgres:17`, `run.mjs`는 Homebrew `postgresql@17` 포트 55423)
- Core 코드는 고치지 않았다. 발견한 Core 결함은 7절에 재현 명령과 함께 적었다.

## 1. 변경

| 파일 | 목적 | 커밋 |
|---|---|---|
| `verification/1.0-consumers/provenance.py`(신규), `test_provenance.py`(신규) | 실행 대상 SHA·엔진·SDK 출처 판정을 파일·프로세스와 분리한 순수 함수로 두고 불일치 반례 17개로 고정 | `d627c4d7` |
| `verification/1.0-consumers/run.py` | `4e132564` 고정 단언 제거. 대상 SHA(`--core-sha` 또는 `AIP_CONSUMER_CORE_SHA`, 없으면 checkout HEAD)와 HEAD 불일치·더러운 트리는 실패. 엔진(`aip --version`, 바이너리 sha256, 패키지 `BUILD.json`의 `source_revision`·`binary_sha256`), SDK(tarball·wheel 이름·버전·내용) 대조. 결과 JSON(`--result`) 기록. SDK 파일명 하드코딩을 `version.mjs`로 대체 | `d627c4d7` |
| `.github/workflows/independent-consumer-rc.yml` | `workflow_call`(+`workflow_dispatch`) 입력 `core_sha`, 기본 `github.sha`. 별도 `core/` checkout과 `ref: 4e132564` 제거, 단일 checkout, 40자 SHA 검증, provenance 단위 시험, 결과 JSON·로그 artifact(이름에 SHA) | `b818613d` |
| `.github/workflows/release-candidate.yml` | `independent-consumers` job 추가와 `required.needs` 등록(F-01). 이 두 파일의 해당 부분만 수정 | `b818613d` |
| `verification/1.0-consumers/run.py`, `apps/taskshare/client/e2e.py`(신규), `apps/stockdesk/client/e2e.ts`(신규) | 앱마다 TypeScript·Python 클라이언트를 둘 다 설치·실행, 장애 후 복구 3종, 마이그레이션 뒤 새 필드 읽기 | `f1dc028e`, `9fe381fe` |
| `product/tests/fixtures/expr-apps/teamdocs/**`, `storefront/**`, `run.mjs` | C3 신규 시나리오 2개(PG17 실행) | `fbac2039`, `e4cf0fbf`, `7fee6dc5` |
| `plan-docs/stabilization-20261122/track-c.md`(이 문서), `baseline.md`, `plan-docs/expressiveness-1.0/audit.md`, `CHANGELOG.md` | 기록, 분류 갱신 | 이 문서의 커밋 |

업그레이드 baseline(`aip-release-gate.yml`의 `--baseline 4c27040c…`)은 그대로 두었다. `run.py`는 이 SHA를 결과 JSON의 `upgradeBaseline`(`usedByThisRun: false`)에만 따로 적고 이 실행의 대상으로 쓰지 않는다.

## 2. C1: 대상 SHA 실행과 출처 기록

### 2.1 RED: 대상 SHA 불일치를 주입하면 실패

과거 SHA를 대상으로 넘겼다(`--core-sha 4e1325644b267f2b3177a327258c0882ee5711c2`, checkout HEAD는 `6e62093a…`).

```
FAIL ProvenanceError: source-head-matches-target: checkout HEAD 6e62093a9f5e9ca64283f4368238d9b7eda2f9df / 실행 대상 4e1325644b267f2b3177a327258c0882ee5711c2
Result JSON: /tmp/c-red-sha.json          # status=fail, error=ProvenanceError…
exit=1
```

더러운 트리에서는 `source-tree-clean: 추적 파일 변경: ['M verification/1.0-consumers/run.py']`로 같은 방식으로 실패한다(첫 실행에서 관찰).

단위 시험의 RED: `provenance.py`에서 판정 3곳(`head == expected`, `binary_sha256 ==`, `tarball_hashes == fresh_hashes`)을 항상 참으로 바꾸고 `python3 verification/1.0-consumers/test_provenance.py`를 돌리면 `FAILED (failures=4)`(`test_head_that_differs_from_target_fails`, `test_package_with_another_binary_fails`, `test_tarball_content_that_differs_from_fresh_build_fails`, `test_empty_content_does_not_pass_as_equal`). 원복 뒤 `Ran 17 tests … OK`.

### 2.2 GREEN: 최신 HEAD 실행과 결과 JSON

```
$ python3 verification/1.0-consumers/run.py --core-sha 659e58440890b2357160dc0159c479c8b3b37f94 --result /tmp/c-final-consumers.json
실행 대상 Core 659e58440890b2357160dc0159c479c8b3b37f94 (--core-sha)
Core 659e58440890… / aip 1.0.0 / aip sha256 539644e29bff90f4
PASS two consumer paths on 659e58440890b2357160dc0159c479c8b3b37f94          exit=0
```

결과 JSON(`schema: aip-independent-consumers/1`)의 기록 값: `target.expectedSha == target.headSha`, `engine.version = aip 1.0.0`, `engine.sha256 = 539644e2…`(패키지 `BUILD.json.binary_sha256`과 같음), `sdk.npm.*.file = aip-sdk-1.0.0.tgz` sha256 `66d02998227c…`(이번 실행에서 `build-sdk.mjs`로 새로 만든 내용과 25개 파일 전부 동일), `sdk.python.file = application_intent_protocol_sdk-1.0.0-py3-none-any.whl` sha256 `7f7e4c591e2b…`(wheel의 `aip_sdk/` 내용이 checkout의 `product/python/aip_sdk`와 동일), `checks` 30개 전부 `ok`, `upgradeBaseline = {pinned: true, usedByThisRun: false, sha: 4c27040c…}`.

한계(정직하게): `aip --version`은 `aip 1.0.0`만 출력하고 커밋을 담지 않는다. 그래서 "빌드 엔진 SHA"는 (a) `cargo build --release --locked`를 run.py가 직접 실행(소스가 같으면 no-op, 다르면 재빌드), (b) 그 바이너리 sha256, (c) 패키저가 쓴 `BUILD.json`의 `source_revision`(= 패키징 시점 `git rev-parse HEAD`)과 `binary_sha256`이 대상 SHA·바이너리와 같을 것으로 확인한다. 바이너리 안에 커밋을 심는 것은 Core 코드(`crates/aip-cli`) 변경이라 하지 않았다. 후속 후보: `aip --version --verbose`에 빌드 커밋 노출(소유: E 또는 F1, 가산 변경).

### 2.3 F-01: required에 연결

`release-candidate.yml`에 `independent-consumers: uses: ./.github/workflows/independent-consumer-rc.yml`와 `required.needs`의 `independent-consumers`를 추가했다. 호출 시 입력이 없으므로 `core_sha`는 호출자의 `github.sha`다. 검증: Ruby `YAML.load_file`로 두 파일 파싱 성공(`actionlint`는 이 Mac에 없어 `미실행`). 실제 GitHub Actions 실행은 `미실행`(release 브랜치·dispatch가 필요하고 이 세션은 push만 했다). 환경 차이: 이 Mac은 macOS라 `bwrap`·AppArmor 단계는 실행하지 못했다.

## 3. C2: taskshare, stockdesk 최신 HEAD 실행

`run.py` 한 번(위 GREEN)의 앱별 결과다. 모든 항목은 실제 `aip service serve`, 설치된 앱 패키지(`bin/aip`는 빌드한 바이너리와 sha256이 같음), docker PG17에서 실행했다.

| 요구 | taskshare | stockdesk | 근거 출력 |
|---|---|---|---|
| 초기 설치 | `package.mjs` → `app-deploy.mjs install` | 같음 | `installed-1/bin/aip` sha256 일치 단언 |
| 정의 검사 | `service check` ok, 깨진 정의는 거부 | 같음 | run.py 단언 |
| 스키마 생성 | `service init`, 시드, `principal bind` | 같음 | |
| SDK 생성·설치(TS) | `bindings.ts` + `aip-sdk-1.0.0.tgz` 설치, 22개 단언 | 생성 binding + tgz, 19개 단언(신규 `e2e.ts`) | `taskshare: typescript SDK client, 22 assertions … ALL PASS` |
| SDK 생성·설치(Python) | `service gen --lang python` + wheel, 16개 단언(신규 `e2e.py`) | 23개 단언 | `stockdesk: python SDK client, 23 assertions … ALL PASS` |
| 인증 사용자 | JWT 3명, `session()` actor 확인 | 3명 | |
| 조회·쓰기 | 목록·항목 생성, 필터·정렬, patch+expect | 상품·주문 생성, 필터 | |
| 권한 거부 | 비공유자 `MISSING_TARGET`, VIEWER 쓰기 `FORBIDDEN`, 익명 0건 | 점원 상품 생성·다른 매장 취소·내부 전이 거부 | |
| 복합 트랜잭션 | 항목 2개 원자 생성, 확장 `reschedule` | 주문+줄 3개, Python 확장 `Order.place`(재고 차감+합계) | |
| 멱등 재전송 | 같은 key `replayed: true` | 확장 재전송이 재차감 안 함 | |
| 정의 변경·마이그레이션 | `color` 필드 추가 → `migrate` plan 적용, 행·멱등 수 `3|21` 보존 | `barcode` 추가, `4|22` 보존 | `…migrate preserved row/idempotency counts 3|21` |
| 이전 클라이언트 호환 | 이전 지문은 `WIRE.CONTRACT_MISMATCH`, 새 지문은 새 필드(`color`) 읽기 성공, 이전 지문으로 새 필드는 거부 | `barcode` 동일 | |
| 롤백 | 덤프 복원 + 이전 패키지 재설치로 원 계약·행 수 복원 | 동일 | `package reinstall and rollback restored active original binding and counts` |
| 장애 후 복구(신규) | 아래 | 아래 | 아래 |

장애 후 복구(`run.py`의 `recover`, 두 앱 모두 같은 결과):

1. 응답을 받은 직후 서버를 SIGKILL, 재시작, 같은 key 재전송 → `replayed: true`이고 행 1개.
2. 두 행을 한 요청으로 만드는 도중 SIGKILL(지연 0·5·20·60ms 4회), 재시작: 4회 모두 부분 커밋 없음(두 행이 같이 있거나 같이 없음). 지연 0은 커밋 전에 죽어 0행이고 재전송이 새로 실행(`replayed: false`), 나머지는 커밋 뒤라 재생(`replayed: true`). 재전송 뒤 각각 정확히 1행.
3. 서버는 두고 DB 연결만 전부 `pg_terminate_backend`: 첫 요청이 `WIRE.DB_UNAVAILABLE`, 같은 key 재시도가 서버 재시작 없이 성공, 행 1개.

RED(복구 시험이 실패할 수 있는가): 1번에서 재전송 key를 다르게(`…-other-key`) 바꾼 사본(저장소 밖)으로 실행하면 taskshare는 두 번째 목록이 생겨(`'created': [{'id': '4', 'resource': 'TodoList'}]`), stockdesk는 `WIRE.ALREADY_EXISTS`로 어서션이 깨졌고 `FAIL AssertionError: ['taskshare', 'stockdesk']`, `exit=1`이었다.

실패 판정(이번 실행에서 실제로 만난 것):

| 관찰 | 판정 | 근거 |
|---|---|---|
| 새 `taskshare/client/e2e.py`가 `AssertionError: dueAt '2026-12-08T00:00:00+00:00' != '…Z'` | 소비자 시험 가정 문제 | 확장 출력 `dueAt`은 확장이 만든 문자열(`Z`)이고 읽기는 offset 형식이다. 시각으로 비교하도록 시험을 고쳤다. 제품은 정상 |
| storefront `ctx.read.filter_not_exposed` | 소비자 정의 문제 | `MANAGED-EXECUTION.md:210`·`LIMITS.md:115`가 문서화한 규칙(확장의 `ctx.data.read` 필터는 `expose read filter`에 열려 있어야 함). `id.in`을 열었다 |
| teamdocs 첫 정의에서 조직을 떠난 사용자가 공유 문서를 계속 읽음(D8 INFO) | 소비자 정의 문제 | `canRead`의 공유 가지가 조직 소속을 보지 않았다. 소속과 AND로 고쳐 DB 상태(공유 행 1건 유지)와 읽기 차단을 함께 단언 |
| 필드 `grant` 등 SQL 예약어: `check` 통과, `init` 실패 | **제품 결함** | 7절 C-D1 |
| 확장 입력 `Json` 필드: 어떤 값도 거절 | **제품 결함(문서 포함)** | 7절 C-D2 |

그 외 두 소비자는 한 번에 통과했고, `4e132564` 이후 hardening 변경(숨은 필드 D1~D13, K4 등)이 두 앱을 깨뜨리지 않았다.

## 4. C3: 기존 expr-apps와의 커버리지 대조와 신규 시나리오

### 4.1 대조표

"기존"은 `expr-apps`의 `saas`·`community`·`orders`와 독립 소비자 `taskshare`·`stockdesk`다(둘 다 HEAD에서 PASS 확인). "신규"는 이번에 더한 `teamdocs`(D1~D8)·`storefront`(S0~S10)다.

| 패턴 | 기존 | 판정 | 신규 |
|---|---|---|---|
| 재고 동시 차감 | `orders` target-a A7a·A7b(재고 4, 2개 6건 동시 → 정확히 2건), 우회 변형(성공 1) | 있음 | 확장 경로의 동시 주문 불변식 S8a. 선언 경로는 중복 안 함 |
| 주문+항목 원자 생성 | `orders` A1~A3(bundle refs), stockdesk 줄 번들 | 있음 | 다중 줄 서버 계산 S1(확장) |
| 가격·수량 검증 | `orders` A5(`input` 범위·재고 초과), A6(단가 강제, **합계는 클라이언트 값**), B3 | 부분: 합계 서버 검증은 상품 하나짜리 변형 B뿐 | S1(줄 3개 합계), S2(수량 0·1001·1.5·"2"·음수·슬롯 오류·중복·없는 상품·owner 위조 11종), S10(직접 create로 합계·결제액·상태 위조, 합계 확정 뒤 줄 추가) |
| 실패 시 전체 롤백 | `orders` A2·A3·B2, `community` 일괄 게시 롤백, stockdesk 재고 부족 | 있음 | S3(둘째 줄 부족 → 첫 줄 재고도 불변), S5(결제 원장 create + `pay` 한 요청, `pay` 실패 시 원장 행도 없음) |
| 상태 전이와 권한 | `saas` Task·Project, `community` 운영자, taskshare 보관 | 있음(주문 상태 머신 없음) | S5~S7: PLACED→PAID→SHIPPED/CANCELLED, 결제 확정은 GATEWAY만, 발송은 STAFF만, 고객의 자가 결제·발송 거부 |
| 소유자·관리자·일반 | `saas` OWNER·ADMIN·MEMBER·VIEWER(역할은 사용자 행 한 칸), stockdesk MANAGER·CLERK | 있음 | D2: 조직별 역할(Membership), ADMIN은 MEMBER만 추가·OWNER만 승격 |
| 여러 조직 | `saas` Acme·Globex(사용자당 조직 하나, `actor.org`) | 부분: 한 사용자가 여러 조직에 속하는 경우 없음 | D1·D4·D8: 한 사용자가 조직마다 다른 역할, 조직 밖 문서 생성·수정·공유 거부, 탈퇴 뒤 접근 |
| 공유 권한 | taskshare ListShare(VIEWER/EDITOR, 조직 경계 없음) | 부분 | D5: VIEW→EDIT 격상, 낡은 version 충돌, 조직 밖 공유·재공유·자기 공유 거부, 해제 즉시 차단 |
| 관계 조회 | `saas` traverse, `community`, `orders` 줄, 두 소비자 | 있음 | S6(주문+줄), D7(소유자·공유 목록) |
| 집계·필드 제한 | `saas` 댓글 수·이메일, `community` count/sum, taskshare `openCount` | 있음(필드 제한은 이메일 한정) | S9(원가는 직원만), D6(내부 메모: 소유자·ADMIN만, 공유 편집자도 null이고 메모 쓰기 전이도 불가), D7(집계가 읽을 수 없는 문서의 값을 새지 않음, DB 행 수와 일치) |
| 외부 결제 | 없음 | 없음 | 외부 서비스 호출 없음. GATEWAY 역할 주체가 콜백을 대신하는 test double(S5, 금액 일치·중복 콜백·원장 유일성) |

### 4.2 실행 출력

```
$ env -u PGHOST -u PGPORT AIP_BIN=/tmp/aip-stab-c-target/release/aip AIP_TEST_DATABASE_URL='host=127.0.0.1 port=55423 dbname=aip_test_ci user=runner' node product/tests/fixtures/expr-apps/run.mjs
…
PASS teamdocs/ts    D5-share-view-edit-revoke-and-no-cross-org-share
INFO storefront/ts  S8a-concurrent-checkout  -- 재고 4, 2개짜리 4건 동시 -> 성공 2건 (WIRE.EXTENSION_ERROR|WIRE.EXTENSION_ERROR|OK|OK), 최종 재고 0, 406ms
INFO storefront/ts  S8b-double-cancel  -- OK|WIRE.EXTENSION_ERROR, 재고 4 -> 6
실제 PostgreSQL: PostgreSQL 17.11 (Homebrew) on aarch64-apple-darwin27.0.0 …
revision: 659e58440890, aip 1.0.0, HTTP=aip service serve, SDK=@aip/sdk(tarball 설치)+python aip_sdk
합계: PASS 87 / FAIL 0 / SKIP 0
```

기준선(HEAD `e29b1be5`, 신규 전)은 `PASS 64`였고, 신규로 teamdocs 9개(typecheck 1 + D1~D8), storefront 14개(typecheck 1 + S0~S10과 INFO 쌍)가 더해져 87이다. `verify.yml`의 "Run expressiveness consumer apps on PostgreSQL" step은 `run.mjs`를 인자 없이 돌리므로 신규 단위도 매 push에 실행된다(F1에 연결 요청 없음).

### 4.3 의미 단언(DB 상태로 확인)

| 단언 | 시나리오 | 확인 방법 |
|---|---|---|
| 재고가 정확히 한 번 감소 | S1·S4 | 주문 전후 `product.stock` 차이 = 수량, 같은 key 재전송 뒤에도 `d0 - 1`, 주문 1건 |
| 동시 요청에도 음수 없음, 과판매 없음 | S8a | 4건 동시: 성공 2건, 최종 재고 0, `재고 감소량 = 성공 주문 수량 합`, 줄 수 = 성공 수, 줄 없는 고아 주문 0 |
| 일부 실패 시 전체 취소 | S2·S3·S5·S7 | 거절 뒤 `snapshot()`(주문·줄·결제 수, 전체 재고 문자열)이 거절 전과 같음 |
| 취소 시 재고가 정확히 한 번 반환 | S7·S8b | PAID 취소 뒤 재고가 주문 전 값, 재전송·다른 key 재취소에도 불변, 동시 취소 2건 중 1건만 성공하고 재고는 `+2` 한 번 |
| 결제가 정확히 한 번 반영 | S5·S8c | `paid`가 두 번 더해지지 않음, 중복 콜백(다른 key, 같은 providerRef)은 `ALREADY_EXISTS`, 동시 중복 콜백은 한 건만 성공 |
| 숨은 값이 다른 경로로 새지 않음 | S9·D6·D7 | 원가·내부 메모는 읽기 `null`이고 DB에는 값 존재, 편집 권한자도 메모 쓰기 불가 |

### 4.4 시험이 실패할 수 있는가(RED, 정의 변이)

정의 한 줄씩 망가뜨려(`git checkout`으로 원복) 같은 시나리오를 돌렸다. 변이 스크립트는 `/tmp/mutate.py`(저장소 밖)다.

| 앱 | 변이 | 잡은 시나리오 |
|---|---|---|
| teamdocs | `rows read when canRead(...)` → `actor != null` | D3 D4 D5 D6 D7 D8 (PASS 3 / FAIL 6) |
| teamdocs | 공유 create에서 `isMember(doc.org, grantee)` 제거 | D5 D6 D7 |
| teamdocs | `internalNote` 읽기 정책을 열기 | D6 |
| teamdocs | ADMIN이 ADMIN 멤버 추가 허용 | D2 D3 D5 D6 D7 |
| teamdocs | 공유 가지에서 조직 소속 제거 | D8 |
| teamdocs | 편집 공유자가 `annotate` 가능 | D6 |
| storefront | `pay`의 `total = input.amount` 제거 | S5 S6 S7 |
| storefront | `unique providerRef` 제거 | S5 |
| storefront | `cost` 읽기 정책을 열기 | S9 |
| storefront | 직접 create의 `total = 0` 제거 | S10 |
| storefront | `pay` allow를 `actor != null`로 | S5 |
| storefront | 발송 뒤 취소 허용 | S7 |
| storefront | `take`의 `from stock >= input.qty` → `from stock >= 0` | **잡지 못함**(13개 PASS). 동등 변이로 판단: `stock`의 `Int(0..)` 범위가 음수를 같은 롤백으로 막아 최종 DB 상태가 같고, 달라지는 것은 오류 코드(`INVALID_STATE` 대 `BAD_VALUE`)뿐인데 확장 안에서는 둘 다 일반 `WIRE.EXTENSION_ERROR`로 보인다. `orders` A2는 직접 호출이라 코드를 단언해 잡는다 |

## 5. C4: 표현력 분류(현재 HEAD)

분류: **A** 선언만 / **B** 선언 가능하나 불필요하게 복잡 / **C** 공식 확장 필요 / **D** 여러 요청으로 나뉘어 원자성이 깨짐 / **E** 구현 불가. "실행"은 이번에 PG17에서 실제로 돌린 근거, "기존 시험"은 이번에 재실행하지 않은 저장소 시험이다. 이전 분류(`audit.md`, 기준 `4e132564`)와 다른 항목은 비고에 적었다.

| # | 기능 | 등급 | 근거 | 선언으로 안 되는 이유 / 비고 |
|---|---|---|---|---|
| 1 | 부분 수정 + 낙관적 잠금(patch+expect) | A | 실행: saas, taskshare, stockdesk, teamdocs D5 | |
| 2 | 상태 전이 + 역할 제한 | A | 실행: storefront S5~S7, community, saas | 같은 행을 두 전이가 경쟁하면 전이의 `from`이 판정(S8b) |
| 3 | 주문+줄 원자 생성 | A(합계 제외) | 실행: orders A1~A3 | `4e132564`의 D에서 상승(P0-A). 합계는 클라이언트 값(6번) |
| 4 | 가변 수량 재고 차감·증가 | A | 실행: orders A, storefront(`take`·`receive`) | `4e132564`의 C에서 상승(P0-B). 경쟁이 몰리는 전이를 묶음 앞에 둬야 한다(LIMITS E9) |
| 5 | 재고 부족 시 전체 롤백 | A | 실행: orders A2, storefront S3 | 선언 경로는 `from stock >= input.qty`가 판정 |
| 6 | 주문 합계 = Σ 수량×단가를 서버가 보장 | **C** | 실행: storefront S1, orders 변형 B | 전이 `to`·효과 값은 `f ± input` 항만 받고 곱셈이 없으며 다른 행의 합을 값으로 못 쓴다(E3). 선언 변형 A는 합계가 클라이언트 값(orders A6). **반복 패턴**: orders·stockdesk·storefront 세 곳에서 필요 |
| 7 | 취소 시 주문 줄 수량만큼 재고 반환 | **C** | 실행: storefront S7, S8b | 효과 값에 가변 수량·산술이 없고 줄을 순회하는 효과가 없다. bundle로 클라이언트가 줄 수량을 보내면 원자적이지만 호출자가 수량을 속일 수 있어 권한을 못 닫는다. 확장은 한 트랜잭션·한 key로 처리(`cancelAndRestock`) |
| 8 | 확장에 여러 줄 입력 | **C→제한** | 실행: storefront S0·S2 | 입력에 배열이 없고 `Json`은 어떤 값도 거절(7절 C-D2). 고정 슬롯(`product1..3`)이 유일한 우회. 줄 수 상한이 정의에 박힌다 |
| 9 | 결제 확정 콜백(금액 일치, 중복 방지) | A | 실행: storefront S5, S8c | 외부 결제 서명 검증은 AIP 밖(GATEWAY 주체가 대행). 외부 서비스 호출은 지원 밖 |
| 10 | 결제 원장 + 상태 전이를 한 번에 | A | 실행: storefront S5 | bundle로 원자(실패 시 원장 행도 롤백) |
| 11 | 다중 조직 소속, 조직별 역할 | A(반복 작성 부담) | 실행: teamdocs D1~D4 | 자동 테넌트 격리는 없고 resource마다 `exists` 술어를 반복. 술어 안에서 `this`를 쓸 수 없어(`DEF.THIS_IN_EXISTS`) 인자로 넘기는 형태를 알아야 한다. saas의 `actor.org` 단일 조직 모델보다 작성량이 늘어난다 → B에 가깝다 |
| 12 | 공유 권한(VIEW/EDIT, 해제, 조직 경계) | A | 실행: teamdocs D5, D8, taskshare | 공유 가지에 조직 소속을 AND하지 않으면 탈퇴 뒤에도 읽힌다(정의 작성 함정, D8) |
| 13 | 필드 단위 읽기 제한 | A | 실행: storefront S9, teamdocs D6 | 읽기 정책과 별개로 쓰기 전이를 분리하지 않으면 읽을 수 없는 필드를 쓸 수 있다(D6의 `annotate`) |
| 14 | 관계 조회(to-one·to-many traverse) | A | 실행: storefront S6, teamdocs D7 | |
| 15 | 관계 필드로 필터·정렬(`owner.org.eq`) | E | 실행: `service check` → `DEF.PARSE_EXPECTED` | `audit.md` #13 유지. 후속 후보 아님(값 노출 경로 설계 필요) |
| 16 | count·Int sum 집계 | A | 실행: community, teamdocs `editorCount` | 공개 count 의미는 Track A 결정(A-02) |
| 17 | Decimal sum | B | `service check` ok만 이번에 확인(실행 근거는 기존 시험 `7e367ead`, 미재실행) | `4e132564`의 E에서 상승 |
| 18 | `Σ 수량×단가` 집계 | E | 실행: `DEF.LEX_UNEXPECTED_CHAR`(`*`) | 6번과 같은 뿌리 |
| 19 | 효과 값 산술(`m = m + 1`, `bal = bal - this.total`), Decimal 증감(`input.amount`) | B | `service check` ok만 확인, PG 실행 안 함(재고 정수 증감 `input`은 4번에서 실행 확인) | `4e132564`의 C(E2)에서 상승 가능. 곱셈(`input.q * 2`)은 `DEF.LEX_UNEXPECTED_CHAR` |
| 20 | cursor + nullable 정렬 | E | 실행: `DEF.CURSOR_NULLABLE_SORT` | `audit.md` #12 유지. 신규 schema에서 `default database` 필드는 `check`가 통과하고 `init`이 `EXTERNAL_DEFAULT_REQUIRED`로 거절(7절 C-D3)이라 서버 생성 시각 정렬도 막힘 |
| 21 | 멱등 재전송, 장애 후 복구 | A | 실행: run.py 복구 3종, storefront S4 | 서버 SIGKILL·DB 연결 끊김 뒤 같은 key가 정확히 한 번 |
| 22 | 확장 내부 `ctx.data.apply` 실패의 업무 사유 전달 | B | 실행: storefront S7·S8a INFO(`WIRE.EXTENSION_ERROR`) | 확장이 던지지 않고 중첩 전이가 `from`에서 거절되면 호출자는 사유·재시도 가능 여부를 모른다(원인은 서버 로그 `cause: ctx.apply.invalid_state`). `spec/wire-errors.md`가 문서화한 동작이며 `ctx.reject`는 확장 코드가 직접 판정할 때만 쓸 수 있다. 7절 C-D4 |
| 23 | 비동기 job·외부 효과·파일·실시간 | D/E | 미실행 | `audit.md` #19·#20 그대로(지원 표 밖). 이번 범위 밖 |
| 24 | 확장 호출 SDK 타입(TS·Python) | B | 실행: 두 앱 모두 `writeExtension`/`apply({"extension":…})` | 입력에 `Json`이 있으면 TS SDK가 요청 전에 거절(C-D2) |

### 5.1 선언만으로 안 되는 이유와 후속 후보(반복 패턴만)

무제한 표현식·루프·함수는 후보로 두지 않았다. 아래는 세 앱 이상에서 같은 모양으로 나온 반복 패턴이다.

| 후보 | 반복 근거 | 선언으로 안 되는 이유 | 형태(제안, 확정 아님) |
|---|---|---|---|
| 줄 합계 롤업 `total = Σ qty×unitPrice` | orders 변형 A·B, stockdesk `place`, storefront `checkout` | 곱셈·행 집계 값을 쓰는 전이 값이 없음 | 고정 형태 하나만: 자식 행의 `sum(a * b)`를 부모 필드에 쓰는 선언(식 일반화 없음). 결정 필요(공개 의미 추가) |
| 줄 수량 기반 부모 행동의 역반영(취소 시 재고 반환) | orders 우회, storefront `cancelAndRestock` | 효과가 줄 단위로 값을 바꾸지 못함 | `update many R … { f = f + <자식 행의 필드> }` 같은 줄 단위 효과. 결정 필요 |
| 확장 입력의 배열 | orders F14, storefront S0 | 배열 타입 없음, Json 불가 | 우선 C-D2 수정(Json 입력 허용 또는 문서에서 안내 삭제). 배열 타입은 별도 결정 |
| 확장 안 중첩 전이 거절의 사유 전달 | storefront S7·S8, orders P-7 | 호출자에게는 일반 오류 | 중첩 `ctx.data.apply`의 `INVALID_STATE`·`CONFLICT`를 업무 코드로 전달하는 옵트인. 확장 ABI 확정 전이라 소유자 결정 |

## 6. 처리하지 못한 것

- `independent-consumer-rc.yml`·`release-candidate.yml`의 실제 GitHub Actions 실행: 하지 않음. 이 환경에서 dispatch·release 브랜치 push는 하지 않았다(`미실행`). Linux 러너 전용 단계(`bwrap`, AppArmor, `apt-get`)도 이 Mac에서 실행하지 못했다.
- `actionlint`: 설치돼 있지 않아 `미실행`(YAML 구조만 Ruby로 파싱).
- storefront·teamdocs의 Python SDK 시나리오: 하지 않음(C2 소비자 두 앱은 두 언어를 모두 실행. C3 신규 시나리오는 TypeScript만).
- 비동기 job·외부 효과·파일·실시간(`audit.md` #19·#20): 이번 범위 밖이라 실행하지 않았다.
- `aip --version`에 빌드 커밋 노출: Core 변경이라 하지 않았다(2.2절 후속 후보).
- storefront `take` 동등 변이(4.4절): 잡지 못했고 이유를 적었다.

## 7. 발견한 Core 제품 결함(소유 Track 추정 포함)

### C-D1: SQL 예약어 필드명은 `service check`를 통과하고 `service init`에서 실패

- 증상: `grant`, `order`, `group`, `limit`, `check`, `user`, `table`, `column`, `default`, `offset`, `select`, `desc`를 필드명으로 쓰면 DDL에서 컬럼이 따옴표 없이 나가 PG 구문 오류가 난다. 호출자는 `{"code":"DB_INIT","message":"초기화 SQL 실패; 트랜잭션 취소"}`만 본다(원인은 PG 로그에만 있다). `qty`·`amount` 같은 일반 이름은 통과(대조).
- 재현(`/tmp`에서 실행, 이번에 12개 단어로 확인):

```
cat > kw.aip <<'EOF'
actor Member
resource Member { fields { id: Id; name: Text(1..50) } rows read when id = actor }
resource Item {
  fields { id: Id; owner: Member; grant: Int(0..10) }
  rows read when owner = actor
  expose read { select id, grant; sort id; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
EOF
aip service check kw.aip      # {"ok":true, …}
aip service init kw.aip --config cfg.json    # exit 1, DB_INIT
# PG 로그: ERROR: syntax error at or near "grant"
#          STATEMENT: CREATE TABLE …doc_share (… grant text CHECK (grant IN ('EDIT','VIEW')) NOT NULL, …)
```

- 소유 추정: D(정의→DDL 의미 정합, baseline D-02), 오류 메시지는 E. 수정 후보: 컬럼 식별자 인용, 아니면 `check`에서 `DEF.*`로 거부.
- 우회: 소비자가 필드 이름을 바꾼다(teamdocs는 `level`로 바꿨다). 테이블 이름은 영향 없음(`order`, `user` 테이블은 정상).

### C-D2: 확장 입력의 `Json` 필드는 어떤 값도 받지 못하고, 문서는 `Json`으로 받으라고 안내한다

- 증상: `extension write … { input { payload: Json } }`에 배열·객체·문자열·숫자·불리언을 보내면 서버가 전부 `WIRE.BAD_VALUE` "`Json` 타입에 맞지 않는 값"을 돌려준다. TypeScript SDK는 요청을 보내기 전에 `확장 값이 생성 계약과 다름`(`WIRE.BAD_VALUE`)으로 던진다.
- 재현: `node product/tests/fixtures/expr-apps/run.mjs --app storefront`의 출력 줄
  `INFO storefront/ts S0-json-input -- server array=WIRE.BAD_VALUE object=WIRE.BAD_VALUE string=WIRE.BAD_VALUE number=WIRE.BAD_VALUE bool=WIRE.BAD_VALUE | typescript sdk threw before sending: 확장 값이 생성 계약과 다름`
- 코드 근거(읽기만 함): 서버 `spikes/spike-v2-read/src/scalar.rs:5-10` `supported()`에 `Json`이 없음, SDK `spikes/spike-v6-transport/client/typed.ts:169-190` `validScalar`에 `Json` 분기가 없음.
- 문서 충돌: `product/LIMITS.md` E8(16행, 167~169행)과 `consumer-apps.md` F14가 "`Json`으로 받고 확장이 모양을 검증"을 우회로 안내한다. 정의 `check`만 통과해서 그렇게 적혔고 실행 근거가 없었다.
- 소유 추정: D(SDK·의미 정합). 수정 방향은 두 가지(Json 입력 허용 / 안내 삭제)로 공개 의미 결정이 걸린다. 이 시나리오는 INFO라 고쳐지면 줄이 `OK`로 바뀐다.
- 우회: 고정 슬롯 입력(`product1..3`, `qty1..3`, 빈 슬롯은 nullable).

### C-D3: 신규 schema의 `default database` 필드는 `check` 통과, `init`에서 거절(낮음)

- 재현: `resource Item { fields { id: Id; owner: Member; created: Time default database } … }`에 `service check` ok, `service init` → `{"code":"EXTERNAL_DEFAULT_REQUIRED","message":"default database는 외부 채택에만 사용한다"}`(exit 1). 오류 메시지는 구체적이다.
- `consumer-apps.md` F1과 같은 현상이 HEAD에서도 그대로다. 소유 추정: D(check↔init 정합). 서버가 생성 시각을 채울 방법이 없다는 표현력 한계(20번)와 연결된다.

### C-D4: 확장 안 중첩 전이 거절이 호출자에게 일반 `WIRE.EXTENSION_ERROR`(문서된 동작, 한계)

- 재현: `run.mjs --app storefront`의 `INFO S7-cancel-shipped -- WIRE.EXTENSION_ERROR`, `INFO S8b-double-cancel -- OK|WIRE.EXTENSION_ERROR`. 서버 로그 `cause`는 `ctx.apply.invalid_state`.
- `spec/wire-errors.md:341`이 문서화했고 `consumer-apps.md` P-7이 이미 기록했다. 결함이라기보다 표현력 한계라 5절 22번에 올렸고 5.1절의 후속 후보로 남겼다. 소유 추정: B(`spike-v4-worker/src/write.rs`)와 소유자 결정(확장 ABI).

### 결함이 아닌 관찰

- DB 연결이 모두 끊긴 직후 첫 요청은 `WIRE.DB_UNAVAILABLE`이고 같은 key로 재시도하면 성공한다(서버 재시작 불필요).
- 읽기가 돌려주는 `Time`은 `+00:00` 형식이고 확장 출력 문자열은 확장이 만든 그대로(`Z`)다. 비교는 시각으로 해야 한다.

## 8. F1에 연결 요청한 시험

없음. `product/tests/fixtures/expr-apps/run.mjs`는 `verify.yml`이 이미 인자 없이 실행하므로 신규 단위가 자동으로 돈다. `verification/1.0-consumers/test_provenance.py`는 `independent-consumer-rc.yml` 안에서 돌린다. 빠른 단위로 `verify.yml`에도 넣고 싶으면 명령은 `python3 verification/1.0-consumers/test_provenance.py`다(F1 소유 파일이라 C는 연결하지 않았다).
