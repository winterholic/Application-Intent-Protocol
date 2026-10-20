# 인증 경계: 로그인·SRP·세션 (WBS 7.3.3.2, I01/I02) — 2026-10-18 정정판

원본 근거는 소스 판독뿐이다(원본 미실행). revision `47246becc445869ba73077baa0a53a683e3508ce`, 파일은 `backend/src/services/auth/auth-login-service.ts`.

## 원본 로그인의 실제 모양

| 경로 | 위치 | 상태 |
|---|---|---|
| 이메일과 비밀번호 로그인 `login` | `:523-600` | **기본 경로**. 서버가 `hashedPassword`를 해시 비교하고(`:562`), 실패 횟수를 올리고(`consecutiveFailedPasswordAttempts`), 필요하면 captcha를 요구한다. 성공하면 서버가 토큰을 발급한다(`generateUserTokens`) |
| SRP 2단계 `loginGenServerPublicKey`, `loginExchangeClientProof` | `:373`, `:426` | **레거시**. `:410` 주석 "Deprecated: provider token validation removed. This path is only reachable from legacy SRP clients.", `:444` 주석 "Deprecated SRP path: default to email auth." |
| `srpCheckClientProof`, `getUserPrivateKey` | import `:16-17`, 사용 `:454`, `:480` | SRP 경로 안에서만 호출된다 |
| 외부 로그인 방식 | `services/auth/auth-type.ts`의 `AuthMethod` | google, github, gitlab, 각종 SAML, ldap, oidc. SAML·LDAP·OIDC 서비스는 `ee/`에 있다 |

이전판은 SRP 검사를 "서버가 실행하는 공식 인증 확장"으로 옮기는 것을 결정으로 적었다. 그 전제(SRP가 로그인의 본체)가 틀렸다. 본체는 서버측 비밀번호 해시 비교와 서버 발급 토큰이고 SRP는 호환용 꼬리다.

## AIP 쪽 경계

`aip service`는 토큰 **검증만** 한다(`product/OPERATIONS.md`, `product/IDP-RUNBOOK.md`). 로그인, refresh, 사용자 관리, 비밀번호 보관과 해시, MFA, captcha, 실패 횟수 잠금은 AIP의 책임이 아니라 외부 IdP나 앱의 몫이다. 따라서 I01은 다음처럼 나뉜다.

| 원본 기능 | AIP 포팅 처리 | 상태 |
|---|---|---|
| 이메일/비밀번호 로그인, 비밀번호 해시, 실패 잠금, captcha | AIP 밖의 IdP가 담당. 자체 구현하지 않는다 | 포팅 대상 아님(경계 결정). 대체 IdP 선택은 사용자 몫 |
| 토큰 발급, refresh, 로그아웃 | IdP 담당. AIP는 `at+jwt`(RS256, 수명 900초 이하)를 검증하고 subject를 actor에 연결(`principal bind`)한다 | AIP 쪽은 동작(`porting/infisical/DB-RUNBOOK.md` 7단계, 10개 점검(2026-10-20 재실행)) |
| 로그아웃 후 구토큰, 정지 계정 | 운영자 `principal revoke`가 연결을 비활성화하고 발급 시각 하한을 올린다 | 제품 서비스의 읽기·쓰기·멱등 재시도 거부와 재바인딩 경계를 실DB로 확인(`tests/test_principal_lifecycle.py`). 앱 로그아웃·IdP 연결은 미착수 |
| MFA, WebAuthn, TOTP | IdP 담당. 등록 row를 AIP resource로 둘지는 미정 | 미착수 |
| SRP | 신규 구현하지 않는다. 레거시 클라이언트 호환이 필요하면 별도 서비스로 두고 AIP는 그 서비스가 발급한 토큰만 본다 | 미착수(결정: 승격 금지) |
| 사용자 목록 가시성 | `User` resource의 행 정책(본인과 같은 조직 구성원만) | **동작 확인**(`tests/test_live_db.py`) |

## 결정

1. 비밀번호·SRP verifier·MFA 비밀을 AIP resource나 job 입출력으로 다루지 않는다. 보관과 검증은 IdP에 둔다.
2. 세션·토큰 수명과 해지는 서버가 강제한다. 제품 프로필(`exp - iat <= 900`)과 `principal revoke`를 쓴다. 호출자가 만든 토큰 주장(role, actorId)은 권한으로 쓰지 않는다.
3. 신규 구현에서 SRP는 AIP 안에 넣지 않는다. 이전판의 "SRP 검증을 AIP 확장에 둔다"는 결정은 철회한다.
4. 사용자 개인키 복호(`getUserPrivateKey`)는 SRP 레거시 경로의 부산물이며 서버측 KMS 기본 경로와 무관하다([INFISICAL-KEYMODEL.md](INFISICAL-KEYMODEL.md)). 포팅하지 않는다.

## I02 확장 spike 상태

`porting/infisical/workers/infisical_jobs.py`의 `verify_identity_token`/`verify_identity_jwt`는 JWT 1종의 서명(JWKS), 만료·nbf, issuer·audience를 검증하는 순수 함수이고 `tests/test_identity_jwt.py` 17건으로 확인한다(소켓·외부 호출 없음, DB 없이 실행).

- **서비스에서 도달할 수 없다.** 이 함수를 가리키는 `operation`이 `app.aip`에 선언돼 있지 않다. 단위 시험이 검증한 것은 함수일 뿐 AIP 서비스 경로가 아니다.
- 원본의 머신 아이덴티티 인증 서비스 디렉터리는 13개다(`identity-ua`(universal-auth)와 `identity-*-auth` 12개: alicloud, aws, azure, gcp, jwt, kubernetes, ldap, oci, oidc, spiffe, tls-cert, token). 단위 시험이 있는 jwt 외 12개는 구현이 없다.
- 외부 IdP 검증은 서버(확장) 안에서 끝나고 선언 세계에는 검증된 `(issuer, subject, claims)`만 들어간다는 방침은 유지한다.

### 2026-10-20 JWT claim 경계 보완

서명 검증에는 계속 `joserfc`를 사용한다. `decode`만으로 claim 검증이 끝나는 것은 아니다([라이브러리 문서](https://jose.authlib.org/en/guide/jwt/)). 시험 함수는 필수 `exp`와 선택 `nbf`·`iat`를 유한한 JSON 숫자로 제한하고, Python에서 정수의 하위 타입인 boolean도 거부한다. `sub`는 비어 있지 않은 문자열, `aud`는 문자열 또는 문자열 배열만 허용한다. 서명된 JSON의 최상위 값도 객체여야 한다.

만료는 설정된 clock skew를 더한 경계에서도 거부한다(`now >= exp + skew`). `nbf` 경계는 허용하고 소수 시각도 허용한다([RFC 7519 §4.1.2–4.1.6](https://www.rfc-editor.org/rfc/rfc7519.html#section-4.1.2)). 음수·숫자가 아닌 skew 설정은 `RuntimeError`로 실패한다. 잘못된 날짜·subject·audience를 넣은 실제 RSA 서명 토큰으로 수정 전 실패를 확인했다(15개 시험 실행, 실패 31개·오류 2개; subtest 합계). 보완 뒤 17개 시험이 로컬에서 통과했다. 큰 정수 날짜도 float 변환 없이 처리한다. 이 함수에는 제품 access token의 최대 수명 프로필을 추가하지 않았으며, 기존 선택적 audience 설정도 유지한다.

이 변경은 서비스 연결, 머신 주체 발급·매핑, 토큰 폐기, 나머지 12개 인증 방식 또는 전체 Infisical 인증 동등성을 증명하지 않는다. 이 함수가 서비스에서 도달할 수 없다는 위 상태는 그대로다.

## 2026-10-20 제품 principal 해지·재바인딩 검증

JWT spike와 분리해, 실제 `aip service`와 공식 Python SDK·운영자 CLI·전용 PostgreSQL에서 8개 시험을 실행했다(`test_principal_lifecycle.py`, 로컬 `Ran 8 tests ... OK`). worker는 실행하지 않는다. 임시 배포가 생성한 키로 서명하며 실제 IdP·자격 증명은 사용하지 않는다.

- 사용하던 SDK client도 `principal revoke` 뒤 `/read`, 새 쓰기, 성공한 쓰기의 같은 idempotency key 재시도를 `WIRE.UNAUTHENTICATED`로 거부한다. 기존 성공 행만 남고 새 행은 없다. SDK의 캐시된 `session()`을 재인증 증거로 사용하지 않는다.
- 연결이 disabled인 동안 새 토큰의 더 늦은 발급 시각으로 활성화할 수 없다. 본인 actor로 `bind`하면 enabled가 복원되지만 기존 `min_iat`는 낮아지지 않는다. 구토큰은 계속 거부하고 하한과 같은 발급 시각의 새 토큰은 허용한다. revoke의 실제 하한은 DB에서 읽고 그 시각까지 조건부로 기다린다(최대 3초).
- 명시한 `--not-before`의 직전은 거부, 같은 시각·직후는 허용한다. 낮은 not-before로 재바인딩해도 하한을 되돌리지 않는다.
- 같은 issuer/subject를 다른 actor로 바꾸면 활성·해지 상태 모두 `PRINCIPAL_CONFLICT`다. 없는 연결의 revoke는 `PRINCIPAL_NOT_FOUND`이고 다른 연결을 건드리지 않는다.
- 서명된 token에 `actorId`, `actor_id`, `role`, `roles`, `admin`을 넣어도 연결된 viewer actor로만 실행한다. 900초 수명은 허용, 901초는 거부하며 만료 토큰은 `WIRE.TOKEN_EXPIRED`다. 연결되지 않은 subject도 거부한다.

운영자 명령을 시험한 것이며 앱의 로그아웃 endpoint, IdP 관리 연동, 서비스 미연결 I02 spike, 나머지 12개 인증이나 전체 인증 동등성을 완료로 세지 않는다. Linux에서도 같은 suite를 실행하도록 기존 worker CI에 추가했다. CI 실행 결과는 해당 커밋의 원시 로그로 별도 확인한다.
