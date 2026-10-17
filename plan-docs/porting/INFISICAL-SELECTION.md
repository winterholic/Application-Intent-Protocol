# Infisical 공개 버전 전체 포팅 — 선정·범위·기준선 (WBS 7.3.1)

원본을 실행하지 않고 소스만 읽은 조사 단계다. 실행·동등성 판정은 아니다. 2026-10-18 독립 검증 결과로 수치와 범위 서술을 정정했다.

## 고정 revision

| 항목 | 값 |
|---|---|
| 저장소 | `https://github.com/Infisical/infisical` |
| 고정 SHA | `47246becc445869ba73077baa0a53a683e3508ce` |
| 고정 시점 | 2026-10-17 12:45:29 -0300 (merge #8746) |
| 취득 방식 | `git clone --depth 1` (읽기 전용, 실행 없음) |
| 로컬 경로 | `~/development/projects/aip/infisical-upstream` (AIP 레포 밖, 원본 불변) |
| 규모 | 15,396 파일 / 약 2.4G |

## 라이선스 경계 (확인됨)

| 범위 | 라이선스 | 근거 |
|---|---|---|
| `ee/` 제외 전부 | MIT Expat | `LICENSE` (“All content that resides under any `ee/` directory … are licensed under … `ee/LICENSE`”) |
| `backend/src/ee/` (routes v1/v2/v3·서비스 77개) | Enterprise License | `backend/src/ee/LICENSE.md`: production 사용에 구독 필요, 개발·테스트 수정 허용 |
| `backend-go/internal/ee/` | Enterprise License | 동일 조건 |
| 서드파티 | 각 원 라이선스 | `LICENSE` 명시 |

**범위 결정(잠정)**: 포팅 분모는 MIT 범위로 두고 EE 77개 서비스·122개 라우터 파일은 분모 밖(참조용 목록만 `infisical-inventory.json` I13에 보존)으로 둔다. 다만 MIT 코드가 EE 모듈에 깊이 의존한다. 권한 평가 핵심이 `ee/services/permission`에 있어 "MIT만 포팅"이 성립하는지는 사용자 판단 대기다. [INFISICAL-EE-BOUNDARY.md](INFISICAL-EE-BOUNDARY.md)를 본다. EE를 실행·배포하지 않는다.

## MIT 기능 목록 (기계 추출, `tools/infisical_inventory.py`)

추출물: [infisical-inventory.json](infisical-inventory.json). 재생성 테스트 `tools/test_infisical_inventory.py`가 JSON 동일성, 라우터·서비스 디렉터리 배정, 이 문서의 표와 JSON의 일치를 검사한다.

### 수치 정정

이전판의 "MIT 라우트 903", "MIT 라우터 파일 425개 중 EE 제외 후"는 틀렸다.

| 항목 | 이전 서술 | 정정값 | 근거 |
|---|---|---|---|
| 라우터 파일 | MIT 425 | 전체 425 = MIT **303** + EE 122. 모듈에 배정한 것은 302(테스트 전용 `bdd-nock-router.ts` 1개 제외) | `totals.router_files_*` |
| MIT 라우트 정의 | 903 | **1002** = 따옴표 문자열 url 903 + 템플릿 리터럴 url 99 | `totals.mit_route_url_kinds` |
| 이전 903의 성격 | 정확한 합계 | 큰따옴표 url만 센 **하한**이다. 템플릿 리터럴 url을 놓쳤고, 주석 처리된 라우트 1건을 잘못 셌다 | 아래 "추출 도구 변경" |
| 서비스 디렉터리 배정 | 일부 누락 | 126개 전부 모듈 1개에 배정(미배정 0). 이전 도구는 47개를 어느 모듈에도 넣지 않았고(`identity-*-auth`, `secret-v2-bridge` 등) 1개를 두 모듈에 넣었다 | `service_dir_coverage` |
| BullMQ 큐 | 20종 이상 | `QueueName` enum **54개**. 참조 위치 분류로 MIT 30, EE 17, 양쪽 4, 미참조 3 | `bullmq` |

### 모듈별 수치

`route_count`는 라우터 파일의 `route({...})` 정의 수이며 노출 endpoint 수가 아니다.

| ID | 모듈 | 라우트 | 서비스 파일 | 비고 |
|---|---|---|---|---|
| I01 | 사용자 인증 | 114 | 47 | auth, user, totp, webauthn, MFA, SRP 로그인(레거시), OAuth client |
| I02 | 머신 아이덴티티 | 88 | 84 | identity, `identity-*-auth` 계열(aws, azure, gcp, jwt, k8s, ldap, oidc, spiffe, tls-cert, token 등), service-token |
| I03 | 시크릿·폴더 | 113 | 119 | secret, secret-v2-bridge, 폴더, env, approval-policy, blind-index |
| I04 | 조직·RBAC | 202 | 71 | org, membership, role, project, 폴더 권한(CASL). 권한 평가 핵심은 `ee/services/permission`에 있다 |
| I05 | 암호화·KMS | 25 | 54 | lib/crypto, 내부 KMS, CMEK, project-bot |
| I06 | 비동기·알림 | 38 | 47 | BullMQ 큐 보조 서비스, SMTP/webhook/슬랙, 리마인더 |
| I07 | 연동 | 169 | 846 | provider 라우터와 공용 helper, secret-sync, app-connection(서비스 파일 수는 provider 디렉터리 때문에 큼) |
| I08 | PKI·인증서 | 178 | 307 | certificate-*, pki-*, signer, EST/SCEP/ACME |
| I09 | 관리·운영 | 68 | 47 | admin, dashboard, license-client |
| I10 | 감사·이벤트 | 7 | 48 | event-outbox(MIT), audit-log 본체는 EE |
| I11 | 프론트엔드 | — | tsx 3088 | Vite + React 18 SPA (`frontend-v2`) |
| I12 | CLI·SDK | — | — | 별도 저장소(원본 docs가 `github.com/Infisical/cli` 참조). 분모 밖 |
| I13 | EE 전용 | — | — | 서비스 77, 라우터 파일 122, 라우트 정의 533. 범위 밖 |
| I14 | Go 사이드카 | — | go 142 | `backend-go`: go-sidecar(HSM·license·queue) |

종류가 다른 수치를 합산해 대체율로 부르지 않는다(페이스리뷰 관례와 동일). 라우트 정의 합계 1002도 호출 가능한 endpoint 수가 아니다. 라우터 밖 파일 12개에 정의 70개가 더 있고(`route_definitions_outside_router_files`), helper 함수가 provider 라우터마다 같은 모양을 한 번씩 등록하므로 노출 endpoint는 더 많다.

### 추출 도구 변경

이전 도구는 `server.route({...})` 본문을 정규식으로 잘라 `url: "..."`(큰따옴표)만 읽었다. 지금은 괄호 균형 스캐너가 모든 `<receiver>.route({...})` 객체를 찾고 문자열, 템플릿 리터럴, 주석, 정규식 리터럴 안의 괄호를 건너뛰며 `method`와 `url` 표현식의 종류(`string`, `template`, `dynamic`)를 기록한다. 이전 결과와 라우트 단위로 비교하면 다음과 같다.

- 주석 처리된 라우트 1건(`certificate-authority-router.ts`의 `/:caId/crl/rotate`)이 빠진다.
- 템플릿 리터럴 url 99건을 포함해 100건이 늘어 1002건이 된다. `dynamic`은 0건이다.
- `identity-*-auth` 라우터 11개가 I01에서 I02로, `identity-org-membership`·`deprecated-identity-project`가 I02에서 I04로, `integration-auth`가 I01에서 I07로 옮겨졌다. 서비스 디렉터리를 모두 배정해 라우터 키가 늘었기 때문이다.

### BullMQ 큐

`QueueName`은 54개이고 `bullmq.queues`에 큐마다 분류가 있다. 분류는 `QueueName.<이름>`을 어느 파일이 참조하는지로 정한다(생산자 또는 worker). `mit`은 `ee/` 밖에서만, `ee`는 `ee/` 안에서만, `mixed`는 양쪽에서 참조한다. `unreferenced` 3개(`AuditLogPrune` 등 레거시 반복 큐)는 enum에만 있다. "MIT 전용 약 33"은 `mit` 30 + `unreferenced` 3이며 `mixed` 4는 EE 코드와 얽혀 있어 제외했다. 참조 분류는 라이선스 판정이 아니다. 큐별 포팅 상태는 [INFISICAL-JOBS.md](INFISICAL-JOBS.md)에 있다.

## 원본 실행 환경 기준선 (읽기만)

| 층 | 원본 | 근거 |
|---|---|---|
| 런타임 | Node `>=26`, npm `>=11.10.0` | `backend/package.json` engines |
| HTTP | Fastify + zod (`server.route`, operationId) | `backend/src/server/routes/v1/auth-router.ts` 등 |
| RDB | PostgreSQL (Knex, 마이그레이션 ~580 파일) | `backend/src/db/migrations`, `docker-compose.prod.yml`의 `postgres:14-alpine` |
| 큐·캐시 | Redis + BullMQ (`queue-service.ts`, 큐 54종) | 위 BullMQ 절 |
| 감사 저장 | ClickHouse (감사 로그) | `backend/src/db/clickhouse-migration-runner.ts` |
| 인가 | CASL ability (폴더 권한 조건식) | `ee/services/permission`, `services/folder-permission`, `lib/casl` |
| 로그인 암호 | 이메일 인증이 기본, SRP는 레거시 경로 | `services/auth/auth-login-service.ts` (주석 "Deprecated SRP path") |
| 프론트 | Vite + React 18 | `frontend/package.json` |
| 사이드카 | Go 1.26 (`go-sidecar`: HSM·license·queue) | `backend-go/cmd/infisical/main.go` |
| 배포 | docker-compose(dev/prod/test/e2e), helm, cloudformation, standalone Dockerfile | 레포 루트 |

## 범위 밖으로 둔 것 (잠정)

- `ee/` 전체 (라이선스상 production 사용에 구독 필요): rotation·scanning·PAM·gateway·KMIP·HSM·SCIM·SAML 등. 단 권한 평가 핵심이 여기에 있다는 점은 [INFISICAL-EE-BOUNDARY.md](INFISICAL-EE-BOUNDARY.md)에서 사용자 판단 항목으로 다룬다.
- CLI·SDK 별도 저장소, Go 사이드카의 EE 부분(HSM·license)
- Swagger/정적 자산 같은 프레임워크 생성물은 분모에 넣지 않음 (관례 유지)
