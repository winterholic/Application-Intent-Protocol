# Infisical 공개 버전 전체 포팅 — 선정·범위·기준선 (WBS 7.3.1)

원본을 실행하지 않고 소스만 읽은 조사 단계다. 실행·동등성 판정은 아니다.

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

**범위 결정**: 포팅 분모는 MIT 범위로 잠근다. EE 77개 서비스·122개 라우터 파일은
분모 밖(참조용 목록만 `infisical-inventory.json` I13에 보존). EE를 실행·배포하지 않는다.

## MIT 기능 목록 (기계 추출, `tools/infisical_inventory.py`)

추출물: [infisical-inventory.json](infisical-inventory.json) (재생성 테스트
`tools/test_infisical_inventory.py` 통과). Fastify `server.route` 블록에서
METHOD+url을 추출하고 라우터 파일을 모듈 1개에 정확히 배정(중복 0, 미배정 0).
MIT 라우터 파일 425개 중 EE 제외 후 집계.

| ID | 모듈 | 라우트 | 서비스 파일 | 비고 |
|---|---|---|---|---|
| I01 | 사용자 인증 | 208 | 46 | auth, user, totp, webauthn, MFA, SRP 로그인, OAuth client |
| I02 | 머신 아이덴티티 | 40 | 26 | identity, universal-auth 계열 12종, signer 제외 |
| I03 | 시크릿·폴더 | 113 | 32 | secret, 폴더/env, approval-policy(공유), blind-index |
| I04 | 조직·RBAC | 189 | 43 | org, membership, role, CASL 폴더 권한 |
| I05 | 암호화·KMS | 25 | 50 | lib/crypto(E2E/SRP/PQC), 내부 KMS, CMEK |
| I06 | 비동기·알림 | 38 | 44 | BullMQ 큐, SMTP/webhook/슬랙, 리마인더 |
| I07 | 연동 | 38 | 564 | 92개 provider 라우터 + 공용 helper CRUD 6종(실노출은 수십 배) |
| I08 | PKI·인증서 | 179 | 147 | certificate, pki, signer(발급 주체), EST/SCEP/ACME |
| I09 | 관리·운영 | 68 | 47 | admin, dashboard, license-client |
| I10 | 감사·이벤트 | 7 | 48 | event-outbox(MIT), audit-log 본체는 EE |
| I11 | 프론트엔드 | — | tsx 3088 | Vite + React 18 SPA (`frontend-v2`) |
| I12 | CLI·SDK | — | — | 별도 저장소(원본 docs가 `github.com/Infisical/cli` 참조). 분모 밖 |
| I13 | EE 전용 | — | — | 서비스 77·라우터 파일 122, 범위 밖 |
| I14 | Go 사이드카 | — | go 142 | `backend-go`: go-sidecar(HSM·license·queue) |

MIT 라우트 합계 **903**. 종류가 다른 수치를 합산해 대체율로 부르지 않는다
(페이스리뷰 관례와 동일).

## 원본 실행 환경 기준선 (읽기만)

| 층 | 원본 | 근거 |
|---|---|---|
| 런타임 | Node `>=26`, npm `>=11.10.0` | `backend/package.json` engines |
| HTTP | Fastify + zod (`server.route`, operationId) | `backend/src/server/routes/v1/auth-router.ts` 등 |
| RDB | PostgreSQL (Knex, 마이그레이션 ~580 파일) | `backend/src/db/migrations`, `docker-compose.prod.yml`의 `postgres:14-alpine` |
| 큐·캐시 | Redis + BullMQ (`queue-service.ts`, 큐 20종 이상) | MIT 큐: SecretReminder·SecretWebhook·IntegrationSync·PkiSync 등, EE 큐: rotation·scanning·PAM |
| 감사 저장 | ClickHouse (감사 로그) | `backend/src/db/clickhouse-migration-runner.ts` |
| 인가 | CASL ability (폴더 권한 조건식) | `services/folder-permission`, `lib/casl` |
| 로그인 암호 | SRP (`srpCheckClientProof`, 레거시 경로 표기) | `services/auth/auth-login-service.ts` (구현 내부는 읽지 않음) |
| 프론트 | Vite + React 18 | `frontend/package.json` |
| 사이드카 | Go 1.26 (`go-sidecar`: HSM·license·queue) | `backend-go/cmd/infisical/main.go` |
| 배포 | docker-compose(dev/prod/test/e2e), helm, cloudformation, standalone Dockerfile | 레포 루트 |

## 범위 밖으로 확정된 것

- `ee/` 전체 (라이선스상 production 사용 불가): rotation·scanning·PAM·gateway·KMIP·HSM·SCIM·SAML 등
- CLI·SDK 별도 저장소, Go 사이드카의 EE 부분(HSM·license)
- Swagger/정적 자산 같은 프레임워크 생성물은 분모에 넣지 않음 (관례 유지)
