# 외부 IdP 연결 runbook

`aip service`는 IdP가 발급한 access token을 검증만 한다. 로그인, refresh, 사용자 관리는 IdP와 앱의 몫이다. 이 문서는 실제 IdP(Auth0, Keycloak, Entra ID 등)를 연결하는 절차와, 계정 없이 로컬에서 검증한 범위를 구분해 적는다.

## 검증 상태

| 항목 | 상태 | 근거 |
|---|---|---|
| JWKS를 HTTPS로 받아 서명 검증, issuer·audience·`typ`·수명 검사 | 로컬 mock으로 검증됨 | `crates/aip-auth/src/idp_tests.rs`, `tests/jwt.rs` |
| discovery 문서(`/.well-known/openid-configuration`)의 `issuer`·`jwks_uri`를 설정에 옮기는 절차 | 로컬 mock으로 검증됨 | `discovery_document_yields_the_jwks_url_and_the_issuer_the_service_enforces` |
| 서명 키 회전(겹침 구간, kid 교체, 폐기된 kid 거부) | 로컬 mock으로 검증됨 | `a_new_signing_key_is_picked_up_without_a_restart_and_the_retired_key_stops_working` |
| 모르는 kid에 대한 위조 토큰, 같은 kid에 다른 키 | 로컬 mock으로 검증됨 | `a_forged_token_for_an_unknown_kid_is_rejected_and_never_accepts_the_attacker_key` |
| 실제 Auth0·Keycloak·Entra ID가 이 토큰 프로필(`typ: at+jwt`, 수명 900초 이하)을 발급하는지 | 확인 못함: 계정 없음 | 아래 제공자별 설정은 각 제품 문서 기준의 절차이며 실행해 보지 않았다 |

mock은 저장소 테스트 CA로 서명한 HTTPS 서버다. 서비스 바이너리가 쓰는 `reqwest` rustls 경로(redirect 거부, 크기 상한, timeout)를 같은 코드로 지나가지만, 운영 서비스 프로세스는 공개 CA만 신뢰하므로 서비스 전체를 mock IdP에 연결하는 end-to-end는 하지 않았다.

## 서비스가 요구하는 토큰 프로필

- 서명: RS256, 공개키는 JWKS의 RSA 키(2048~4096비트, `kid` 필수, 공개 지수는 3 이상 홀수).
- header: `typ` 정확히 `at+jwt`, `kid` 있음. `jku`·`x5u`·`x5c`·`crit`·`zip`·`cty`는 거부한다.
- claim: `iss`(설정의 issuer와 문자 그대로 일치, 끝 슬래시 포함), `aud`(설정의 audience), `sub`, `iat`, `nbf`, `exp`. `exp - iat`는 1~900초. 시계 여유(leeway)는 0이므로 서버와 IdP의 시계를 맞춘다.
- `sub`는 `aip service principal bind`로 actor에 연결된 값이어야 한다. 연결이 없으면 인증은 `UNAUTHENTICATED`다.

## 연결 절차

1. IdP에서 API(리소스 서버)를 만들고 audience를 정한다. access token을 JWT(RS256)로 발급하게 하고 `typ`을 `at+jwt`로 설정한다. 최대 수명은 900초 이하로 둔다.
2. discovery 문서를 받는다: `curl -s https://<issuer>/.well-known/openid-configuration`. 다음 두 값을 옮긴다.
   - `issuer` → `auth.issuer` (한 글자도 바꾸지 않는다)
   - `jwks_uri` → `auth.jwks.url` (`kind: "https"`)
3. 설정 예:

   ```json
   "auth": {
     "issuer": "https://login.example.com/",
     "audience": "aip-api",
     "jwks": {"kind": "https", "url": "https://login.example.com/.well-known/jwks.json"}
   }
   ```

4. 서비스를 시작하기 전에 DB에서 사용자를 연결한다: `aip service principal --config service.json bind --subject <IdP의 sub> --actor <actor id>`.
5. 시작하면 서비스가 JWKS를 한 번 받아 검증한다. 실패하면 설정 오류의 원인 문장(`jwks.url에서 공개키 집합을 받지 못함` 등)이 나온다. 값은 로그에 남지 않는다.
6. 토큰 한 장으로 `POST /session`을 호출해 `principal.actorId`가 기대와 같은지 본다.

## 제공자별 메모 (확인 못함: 계정 없음)

아래는 각 제품의 공개 문서를 바탕으로 한 확인 대상 목록이다. 실제 테넌트에서 토큰을 열어 `typ`·`iss`·`aud`·`exp - iat`를 확인한 뒤 쓴다.

- **Auth0**: API의 signing algorithm을 RS256, "JWT Access Token Profile"을 RFC 9068로 설정해야 `typ`이 `at+jwt`가 된다 `확인 필요`. issuer는 `https://<tenant>/`로 끝 슬래시가 붙는다. JWKS는 `https://<tenant>/.well-known/jwks.json`.
- **Keycloak**: issuer는 `https://<host>/realms/<realm>`. JWKS는 `.../protocol/openid-connect/certs`. 기본 access token의 `typ`은 `Bearer`라서 이 서비스가 거부한다. 클라이언트 설정에서 `at+jwt` header type을 쓰도록 바꿔야 하며 제공하는 버전이 있는지 `확인 필요`. audience는 audience mapper로 넣는다.
- **Entra ID**: v2 토큰의 issuer와 audience 형식, `typ` 값이 위 프로필과 맞는지 `확인 필요`. 맞지 않으면 사이에 토큰을 다시 발급하는 token exchange 계층이 필요하다.

## 서명 키 회전

서비스는 JWKS를 60초 TTL로 캐시한다. 캐시에 없는 `kid`의 토큰이 오면 5초에 한 번까지 즉시 갱신을 시도한다.

1. 새 키를 JWKS에 **먼저 추가**하고 아직 그 키로 서명하지 않는다. 최소 1분(캐시 TTL) 기다린다.
2. IdP가 새 키로 서명하기 시작한다.
3. 이전 키로 서명한 토큰이 모두 만료될 때까지(최대 900초) 이전 키를 JWKS에 남긴다. 그 뒤에 제거한다.
4. `kid`를 재사용하지 않는다. 같은 `kid`에 다른 키를 넣으면 서비스는 TTL이 끝날 때까지 캐시된 옛 키로 검증해 새 토큰을 거부한다.

회전 중 JWKS 요청이 실패하면 HTTPS 모드는 5초 동안 `UNAVAILABLE`로 응답한다. 이미 캐시된 키의 토큰까지 이 5초 동안 막히는 한계가 있고, 감사 문서 O-06의 조치 항목이다.

## 장애 대응

| 증상 | 확인 |
|---|---|
| 모든 요청이 `UNAUTHENTICATED` | `iss`가 설정과 한 글자라도 다른지(끝 슬래시), `typ`, `aud`, 시계 오차, `sub`의 principal 연결 |
| 특정 시점부터 일부 사용자만 `UNAUTHENTICATED` | 회전으로 `kid`가 바뀌었는데 JWKS에 새 키가 없는지 |
| `UNAVAILABLE` | 서버에서 JWKS URL 도달 여부, IdP 인증서, 응답 크기(256KiB), 키 개수(32개) |
